// Copyright 2024, 2025 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::args::ValidateArgs;
use crate::channel::DuplexChannel;
use crate::channel::Message;
use crate::requests::{request_fault_proof, request_validity_proof};
use alloy::network::{Ethereum, TxSigner};
use alloy::primitives::Bytes;
use alloy::primitives::B256;
use anyhow::{bail, Context};
use kailua_common::blobs::hash_to_fe;
use kailua_common::journal::ProofJournal;
use kailua_common::precondition::validity_precondition_hash;
use kailua_contracts::*;
use kailua_sync::agent::{SyncAgent, FINAL_L2_BLOCK_RESOLVED};
use kailua_sync::proposal::Proposal;
use kailua_sync::stall::Stall;
use kailua_sync::transact::provider::SafeProvider;
use kailua_sync::transact::Transact;
use kailua_sync::{await_tel, await_tel_res, retry_res_ctx_timeout};
use opentelemetry::global::{meter, tracer};
use opentelemetry::trace::FutureExt;
use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::KeyValue;
use risc0_zkvm::sha::Digestible;
use risc0_zkvm::InnerReceipt;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{error, info, warn};

pub async fn handle_proposals(
    mut channel: DuplexChannel<Message>,
    args: ValidateArgs,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    // Telemetry
    let meter = meter("kailua");
    let meter_fault_count = meter.u64_counter("validator.fault.count").build();
    let meter_fault_latest = meter.u64_gauge("validator.fault.latest").build();
    let meter_correct_count = meter.u64_counter("validator.correct.count").build();
    let meter_correct_latest = meter.u64_gauge("validator.correct.latest").build();
    let meter_skipped_count = meter.u64_counter("validator.skipped.count").build();
    let meter_skipped_latest = meter.u64_gauge("validator.skipped.latest").build();
    let meter_proofs_requested = meter.u64_counter("validator.proofs.requested").build();
    let meter_proofs_completed = meter.u64_counter("validator.proofs.complete").build();
    let meter_proofs_published = meter.u64_counter("validator.proofs.published").build();
    let meter_proofs_fail = meter.u64_counter("validator.proofs.errs").build();
    let meter_proofs_discarded = meter.u64_counter("validator.proofs.discarded").build();
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("handle_proposals"));

    // initialize sync agent
    let mut agent = SyncAgent::new(
        &args.sync.provider,
        data_dir,
        args.sync.kailua_game_implementation,
        args.sync.kailua_anchor_address,
        args.proving.bypass_chain_registry,
    )
    .await?;
    info!("KailuaTreasury({:?})", agent.deployment.treasury);

    // initialize validator wallet
    info!("Initializing validator wallet.");
    let validator_wallet = await_tel_res!(
        context,
        tracer,
        "ValidatorSigner::wallet",
        args.validator_signer.wallet(Some(agent.config.l1_chain_id))
    )?;
    let validator_address = validator_wallet.default_signer().address();
    let validator_provider = SafeProvider::new(
        args.txn_args
            .premium_provider::<Ethereum>()
            .wallet(validator_wallet)
            .connect_http(args.sync.provider.eth_rpc_url.as_str().try_into()?),
    );
    info!("Validator address: {validator_address}");

    // Run the validator loop
    info!(
        "Starting from proposal at factory index {}",
        agent.cursor.next_factory_index
    );
    // init channel buffers
    let mut computed_proof_buffer = VecDeque::new();
    let mut output_fault_buffer = VecDeque::new();
    let mut trail_fault_buffer = VecDeque::new();
    let mut valid_buffer = VecDeque::new();
    let mut last_proof_l1_head = BTreeMap::new();
    loop {
        // Wait for new data on every iteration
        sleep(Duration::from_secs(1)).await;
        // fetch latest games
        let loaded_proposals = match await_tel!(
            context,
            agent.sync(
                #[cfg(feature = "devnet")]
                args.sync.delay_l2_blocks,
                args.sync.final_l2_block
            )
        )
        .context("SyncAgent::sync")
        {
            Ok(result) => result,
            Err(err) => {
                if err
                    .root_cause()
                    .to_string()
                    .contains(FINAL_L2_BLOCK_RESOLVED)
                {
                    warn!("handle_proposals terminated");
                    return Ok(());
                }
                error!("Synchronization error: {err:?}");
                vec![]
            }
        };

        // check new proposals for fault and queue potential responses
        for proposal_index in loaded_proposals {
            let Some(proposal) = agent.proposals.get(&proposal_index) else {
                error!("Proposal {proposal_index} missing from database.");
                continue;
            };
            // Skip Treasury instance
            if !proposal.has_parent() {
                info!("Skipping proving for treasury instance.");
                continue;
            }
            // Skip resolved games
            if proposal.resolved_at != 0 {
                info!("Skipping proving for resolved game.");
                continue;
            }
            // Telemetry
            if proposal.is_correct().unwrap_or_default() {
                meter_correct_count.add(
                    1,
                    &[
                        KeyValue::new("proposal", proposal.contract.to_string()),
                        KeyValue::new("l2_height", proposal.output_block_number.to_string()),
                    ],
                );
                meter_correct_latest.record(
                    proposal.index,
                    &[
                        KeyValue::new("proposal", proposal.contract.to_string()),
                        KeyValue::new("l2_height", proposal.output_block_number.to_string()),
                    ],
                );
            } else {
                meter_fault_count.add(
                    1,
                    &[
                        KeyValue::new("proposal", proposal.contract.to_string()),
                        KeyValue::new("l2_height", proposal.output_block_number.to_string()),
                    ],
                );
                meter_fault_latest.record(
                    proposal.index,
                    &[
                        KeyValue::new("proposal", proposal.contract.to_string()),
                        KeyValue::new("l2_height", proposal.output_block_number.to_string()),
                    ],
                );
            }
            // Look up parent proposal
            let Some(parent) = agent.proposals.get(&proposal.parent) else {
                error!(
                    "Proposal {} parent {} missing from database.",
                    proposal.index, proposal.parent
                );
                continue;
            };
            let parent_contract =
                KailuaTournament::new(parent.contract, &agent.provider.l1_provider);
            // Check termination condition
            if let Some(final_l2_block) = args.sync.final_l2_block {
                if parent.output_block_number >= final_l2_block {
                    warn!(
                        "Dropping proposal {} with parent output height {} past final l2 block {}.",
                        proposal.index, parent.output_block_number, final_l2_block
                    );
                    continue;
                }
            }
            // Check that a validity proof has not already been posted
            let is_validity_proven = await_tel!(
                context,
                parent.fetch_is_successor_validity_proven(&agent.provider.l1_provider)
            )
            .context("is_validity_proven")?;
            if is_validity_proven {
                info!(
                    "Validity proof settling all disputes in tournament {} already submitted",
                    parent.index
                );
                meter_skipped_count.add(
                    1,
                    &[
                        KeyValue::new("proposal", proposal.contract.to_string()),
                        KeyValue::new("tournament", parent.contract.to_string()),
                        KeyValue::new("reason", "parent_successor_proven"),
                    ],
                );
                meter_skipped_latest.record(
                    proposal.index,
                    &[
                        KeyValue::new("proposal", proposal.contract.to_string()),
                        KeyValue::new("tournament", parent.contract.to_string()),
                        KeyValue::new("reason", "parent_successor_proven"),
                    ],
                );
                continue;
            }
            // fetch canonical status of proposal
            let Some(is_proposal_canonical) = proposal.canonical else {
                error!("Canonical status of proposal {proposal_index} unknown");
                continue;
            };
            // utilize validity proofs for proposals of height below the ff target
            if proposal.output_block_number <= args.fast_forward_target {
                // prove the validity of this proposal if it is canon
                if is_proposal_canonical {
                    // Prove full validity
                    valid_buffer.push_back(proposal_index);
                    continue;
                }
                // skip fault proving if a validity proof is en-route
                if let Some(successor) = parent.successor {
                    info!(
                        "Skipping proving for proposal {proposal_index} assuming ongoing \
                        validity proof generation for proposal {successor}."
                    );
                    continue;
                }
            }

            // Switch to validity proving if only one output is admissible
            if agent.deployment.proposal_output_count == 1 {
                // Check if there is a faulty predecessor
                let is_prior_fault =
                    parent
                        .children
                        .iter()
                        .filter(|p| **p < proposal_index)
                        .any(|p| {
                            // Fetch predecessor from db
                            let Some(predecessor) = agent.proposals.get(p) else {
                                error!("Proposal {p} missing from database.");
                                return false;
                            };
                            if agent.was_proposer_eliminated_before(predecessor) {
                                return false;
                            }
                            if predecessor.is_correct().unwrap_or_default() {
                                return false;
                            }
                            info!("Found invalid predecessor proposal {p}");
                            true
                        });
                // Check canonical proposal status
                match parent.successor {
                    Some(p) if p == proposal.index && is_prior_fault => {
                        // Compute validity proof on arrival of correct proposal after faulty proposal
                        info!(
                            "Computing validity proof for {proposal_index} to discard invalid predecessors."
                        );
                        valid_buffer.push_back(p);
                    }
                    Some(p) if p == proposal.index => {
                        // Skip proving as no conflicts exist
                        info!("Skipping proving for proposal {proposal_index} with no invalid predecessors.");
                    }
                    Some(p) if proposal.is_correct() == Some(false) && !is_prior_fault => {
                        // Compute validity proof on arrival of faulty proposal after correct proposal
                        info!("Computing validity proof for {p} to discard invalid successor.");
                        valid_buffer.push_back(p);
                    }
                    Some(p) if proposal.is_correct() == Some(false) => {
                        // is_prior_fault is true and a successor exists, so some proof must be queued
                        info!(
                            "Skipping proving for proposal {proposal_index} assuming ongoing validity proof for proposal {p}."
                        );
                    }
                    Some(p) => {
                        info!(
                            "Skipping proving for correct proposal {proposal_index} replicating {p}."
                        );
                    }
                    None => {
                        info!(
                            "Skipping fault proving for proposal {proposal_index} with no valid sibling."
                        );
                    }
                }
                continue;
            }

            // Skip proving on repeat signature
            let is_repeat_signature =
                parent
                    .children
                    .iter()
                    .filter(|p| **p < proposal_index)
                    .any(|p| {
                        // Fetch predecessor from db
                        let Some(predecessor) = agent.proposals.get(p) else {
                            error!("Proposal {p} missing from database.");
                            return false;
                        };
                        if agent.was_proposer_eliminated_before(predecessor) {
                            return false;
                        }
                        if predecessor.signature != proposal.signature {
                            return false;
                        }
                        info!("Found duplicate predecessor proposal {p}");
                        true
                    });
            if is_repeat_signature {
                info!(
                    "Skipping fault proving for proposal {proposal_index} with repeat signature {}",
                    proposal.signature
                );
                continue;
            }

            // Skip attempting to fault prove correct proposals
            if let Some(true) = proposal.is_correct() {
                info!(
                    "Skipping fault proving for proposal {proposal_index} with valid signature {}",
                    proposal.signature
                );
                continue;
            }

            // Check that a fault proof had not already been posted
            let proof_status = parent_contract
                .proofStatus(proposal.signature)
                .stall_with_context(context.clone(), "KailuaTournament::proofStatus")
                .await;
            if proof_status != 0 {
                info!(
                    "Proposal {} signature {} already proven {proof_status}",
                    proposal.index, proposal.signature
                );
                meter_skipped_count.add(
                    1,
                    &[
                        KeyValue::new("proposal", proposal.contract.to_string()),
                        KeyValue::new("tournament", parent.contract.to_string()),
                        KeyValue::new("reason", "proof_status"),
                    ],
                );
                meter_skipped_latest.record(
                    proposal.index,
                    &[
                        KeyValue::new("proposal", proposal.contract.to_string()),
                        KeyValue::new("tournament", parent.contract.to_string()),
                        KeyValue::new("reason", "proof_status"),
                    ],
                );
                continue;
            }

            // Get divergence point
            let Some(fault) = proposal.fault() else {
                error!("Attempted to request fault proof for correct proposal {proposal_index}");
                continue;
            };
            // Queue fault proof
            if fault.is_output() {
                // Queue output fault proof request
                output_fault_buffer.push_back(proposal_index);
            } else {
                // Queue trail fault proof submission
                trail_fault_buffer.push_back(proposal_index);
            }
        }

        // dispatch buffered output fault proof requests
        let output_fault_proof_requests = output_fault_buffer.len();
        for _ in 0..output_fault_proof_requests {
            let proposal_index = output_fault_buffer.pop_front().unwrap();
            let Some(proposal) = agent.proposals.get(&proposal_index) else {
                if agent.cursor.last_resolved_game < proposal_index {
                    error!("Proposal {proposal_index} missing from database.");
                    output_fault_buffer.push_back(proposal_index);
                } else {
                    warn!("Skipping fault proof request for freed proposal {proposal_index}.");
                };
                continue;
            };
            // Look up parent proposal
            let Some(parent) = agent.proposals.get(&proposal.parent) else {
                if agent.cursor.last_resolved_game < proposal.parent {
                    error!(
                        "Proposal {} parent {} missing from database.",
                        proposal.index, proposal.parent
                    );
                    output_fault_buffer.push_back(proposal_index);
                } else {
                    warn!(
                        "Skipping fault proof request for proposal {} with freed parent {}.",
                        proposal.index, proposal.parent
                    );
                };
                continue;
            };

            let Some(l1_head) = get_next_l1_head(
                &agent,
                &mut last_proof_l1_head,
                proposal,
                #[cfg(feature = "devnet")]
                args.l1_head_jump_back,
            ) else {
                error!("Could not choose an L1 head to fault prove proposal {proposal_index}");
                output_fault_buffer.push_back(proposal_index);
                continue;
            };

            if let Err(err) = await_tel!(
                context,
                request_fault_proof(&agent, &mut channel, parent, proposal, l1_head)
            ) {
                error!("Could not request fault proof for {proposal_index}: {err:?}");
                output_fault_buffer.push_back(proposal_index);
            } else {
                meter_proofs_requested.add(
                    1,
                    &[
                        KeyValue::new("type", "fault"),
                        KeyValue::new("proposal", proposal.contract.to_string()),
                    ],
                );
            }
        }
        // dispatch buffered validity proof requests
        let validity_proof_requests = valid_buffer.len();
        for _ in 0..validity_proof_requests {
            let proposal_index = valid_buffer.pop_front().unwrap();
            let Some(proposal) = agent.proposals.get(&proposal_index) else {
                if agent.cursor.last_resolved_game < proposal_index {
                    error!("Proposal {proposal_index} missing from database.");
                    valid_buffer.push_front(proposal_index);
                } else {
                    warn!("Skipping validity proof request for freed proposal {proposal_index}");
                }
                continue;
            };
            // Look up parent proposal
            let Some(parent) = agent.proposals.get(&proposal.parent) else {
                if agent.cursor.last_resolved_game < proposal.parent {
                    error!(
                        "Proposal {} parent {} missing from database.",
                        proposal.index, proposal.parent
                    );
                    valid_buffer.push_front(proposal_index);
                } else {
                    warn!(
                        "Skipping validity proof request for proposal {} with freed parent {}",
                        proposal.index, proposal.parent
                    );
                }
                continue;
            };

            let parent_contract =
                KailuaTournament::new(parent.contract, &agent.provider.l1_provider);
            // Check that a validity proof had not already been posted
            let proof_status = parent_contract
                .proofStatus(proposal.signature)
                .stall_with_context(context.clone(), "KailuaTournament::proofStatus")
                .await;
            if proof_status != 0 {
                info!(
                    "Proposal {} signature {} already proven {proof_status}",
                    proposal.index, proposal.signature
                );
                continue;
            }

            let Some(l1_head) = get_next_l1_head(
                &agent,
                &mut last_proof_l1_head,
                proposal,
                #[cfg(feature = "devnet")]
                args.l1_head_jump_back,
            ) else {
                error!("Could not choose an L1 head to validity prove proposal {proposal_index}");
                valid_buffer.push_back(proposal_index);
                continue;
            };

            if let Err(err) = await_tel!(
                context,
                request_validity_proof(&agent, &mut channel, parent, proposal, l1_head)
            ) {
                error!("Could not request validity proof for {proposal_index}: {err:?}");
                valid_buffer.push_front(proposal_index);
            } else {
                meter_proofs_requested.add(
                    1,
                    &[
                        KeyValue::new("type", "validity"),
                        KeyValue::new("proposal", proposal.contract.to_string()),
                    ],
                );
            }
        }

        // load newly received proofs into buffer
        while !channel.receiver.is_empty() {
            let Some(message) = channel.receiver.recv().await else {
                error!("Proofs receiver channel closed");
                break;
            };
            meter_proofs_completed.add(1, &[]);
            computed_proof_buffer.push_back(message);
        }

        // publish computed output fault proofs
        let computed_proofs = computed_proof_buffer.len();
        for _ in 0..computed_proofs {
            let Some(Message::Proof(proposal_index, receipt)) = computed_proof_buffer.pop_front()
            else {
                error!("Validator loop received an unexpected message.");
                continue;
            };
            let Some(proposal) = agent.proposals.get(&proposal_index) else {
                if agent.cursor.last_resolved_game < proposal_index {
                    error!("Proposal {proposal_index} missing from database.");
                    computed_proof_buffer.push_back(Message::Proof(proposal_index, receipt));
                } else {
                    warn!("Skipping proof submission for freed proposal {proposal_index}.")
                }
                continue;
            };
            let Some(parent) = agent.proposals.get(&proposal.parent) else {
                if agent.cursor.last_resolved_game < proposal.parent {
                    error!("Parent proposal {} missing from database.", proposal.parent);
                    computed_proof_buffer.push_back(Message::Proof(proposal_index, receipt));
                } else {
                    warn!(
                        "Skipping proof submission for proposal {} with freed parent {}.",
                        proposal.index, proposal.parent
                    );
                }
                continue;
            };
            // Abort early if a validity proof is already submitted in this tournament
            if await_tel!(
                context,
                parent.fetch_is_successor_validity_proven(&agent.provider.l1_provider)
            )? {
                info!(
                    "Skipping proof submission in tournament {} with validity proof.",
                    parent.index
                );
                meter_proofs_discarded.add(
                    1,
                    &[
                        KeyValue::new("proposal", proposal.contract.to_string()),
                        KeyValue::new("reason", "redundant"),
                    ],
                );
                continue;
            }
            let parent_contract = KailuaTournament::new(parent.contract, &validator_provider);
            let expected_fpvm_image_id = parent_contract
                .FPVM_IMAGE_ID()
                .stall_with_context(context.clone(), "KailuaTournament::FPVM_IMAGE_ID")
                .await
                .0;
            // advance l1 head if insufficient data
            let Some(receipt) = receipt else {
                // request another proof with new head
                if let Some(true) = proposal.canonical {
                    valid_buffer.push_back(proposal_index);
                } else {
                    output_fault_buffer.push_back(proposal_index);
                }
                continue;
            };
            // patch the proof if in dev mode
            #[cfg(feature = "devnet")]
            let receipt = maybe_patch_proof(receipt, expected_fpvm_image_id)?;
            // verify that the zkvm receipt is valid
            if let Err(e) = receipt.verify(expected_fpvm_image_id) {
                error!("Could not verify receipt against image id in contract: {e:?}");
            } else {
                info!("Receipt validated.");
            }
            // Decode ProofJournal
            let proof_journal = ProofJournal::decode_packed(receipt.journal.as_ref());
            info!("Proof journal: {:?}", proof_journal);
            // get pointer to proposal with l1 head if okay
            let Some((l1_head_contract, _)) = agent.l1_heads_inv.get(&proof_journal.l1_head) else {
                error!(
                    "Failed to look up proposal contract with l1 head {}",
                    proof_journal.l1_head
                );
                computed_proof_buffer.push_back(Message::Proof(proposal_index, Some(receipt)));
                continue;
            };
            // encode seal data
            let encoded_seal = Bytes::from(encode_seal(&receipt)?);

            let child_index = parent
                .child_index(proposal.index)
                .expect("Could not look up proposal's index in parent tournament");
            let proposal_contract =
                KailuaTournament::new(proposal.contract, &agent.provider.l1_provider);
            // Check if proof is a viable validity proof
            if proof_journal.agreed_l2_output_root == parent.output_root
                && proof_journal.claimed_l2_output_root == proposal.output_root
            {
                info!(
                    "Submitting validity proof to tournament at index {} for child at index {child_index}.",
                    parent.index,
                );

                // sanity check proof journal fields
                {
                    let contract_blobs_hash = proposal_contract
                        .blobsHash()
                        .stall_with_context(context.clone(), "KailuaGame::blobsHash")
                        .await;
                    if proposal.blobs_hash() != contract_blobs_hash {
                        warn!(
                            "Local proposal blobs hash {} doesn't match contract blobs hash {}",
                            proposal.blobs_hash(),
                            contract_blobs_hash
                        )
                    } else {
                        info!("Blobs hash {} confirmed", contract_blobs_hash);
                    }
                    let precondition_hash = validity_precondition_hash(
                        &parent.output_block_number,
                        &agent.deployment.proposal_output_count,
                        &agent.deployment.output_block_span,
                        contract_blobs_hash,
                    );
                    if proof_journal.precondition_hash != precondition_hash {
                        warn!(
                            "Proof precondition hash {} does not match expected value {}",
                            proof_journal.precondition_hash, precondition_hash
                        );
                    } else {
                        info!("Precondition hash {precondition_hash} confirmed.")
                    }
                    let config_hash = proposal_contract
                        .ROLLUP_CONFIG_HASH()
                        .stall_with_context(context.clone(), "KailuaGame::ROLLUP_CONFIG_HASH")
                        .await;
                    if proof_journal.config_hash != config_hash {
                        warn!(
                            "Proof config hash {} does not match contract hash {config_hash}",
                            proof_journal.config_hash
                        );
                    } else {
                        info!("Config hash {} confirmed.", proof_journal.config_hash);
                    }
                    if proof_journal.fpvm_image_id.0 != expected_fpvm_image_id {
                        warn!(
                            "Proof FPVM Image ID {} does not match expected {}",
                            proof_journal.fpvm_image_id,
                            B256::from(expected_fpvm_image_id)
                        );
                    } else {
                        info!("FPVM Image ID {} confirmed", proof_journal.fpvm_image_id);
                    }
                    let expected_block_number = parent.output_block_number
                        + agent.deployment.proposal_output_count
                            * agent.deployment.output_block_span;
                    if proof_journal.claimed_l2_block_number != expected_block_number {
                        warn!(
                            "Proof block number {} does not match expected {expected_block_number}",
                            proof_journal.claimed_l2_block_number
                        );
                    } else {
                        info!("Block number {expected_block_number} confirmed.");
                    }
                }

                match parent_contract
                    .proveValidity(
                        proof_journal.payout_recipient,
                        *l1_head_contract,
                        child_index,
                        encoded_seal.clone(),
                    )
                    .timed_transact_with_context(
                        context.clone(),
                        "KailuaTournament::proveValidity",
                        Some(Duration::from_secs(args.txn_args.txn_timeout)),
                    )
                    .await
                    .context("KailuaTournament::proveValidity")
                {
                    Ok(receipt) => {
                        info!("Validity proof submitted: {:?}", receipt.transaction_hash);
                        let proof_status = parent_contract
                            .provenAt(proposal.signature)
                            .stall_with_context(context.clone(), "KailuaTournament::provenAt")
                            .await;
                        info!("Validity proof timestamp: {proof_status}");
                        info!("KailuaTournament::proveValidity: {} gas", receipt.gas_used);

                        meter_proofs_published.add(
                            1,
                            &[
                                KeyValue::new("type", "validity"),
                                KeyValue::new("proposal", proposal.contract.to_string()),
                                KeyValue::new(
                                    "l2_height",
                                    proposal.output_block_number.to_string(),
                                ),
                                KeyValue::new("txn_hash", receipt.transaction_hash.to_string()),
                                KeyValue::new("txn_from", receipt.from.to_string()),
                                KeyValue::new("txn_to", receipt.to.unwrap_or_default().to_string()),
                                KeyValue::new("txn_gas_used", receipt.gas_used.to_string()),
                                KeyValue::new(
                                    "txn_gas_price",
                                    receipt.effective_gas_price.to_string(),
                                ),
                                KeyValue::new(
                                    "txn_blob_gas_used",
                                    receipt.blob_gas_used.unwrap_or_default().to_string(),
                                ),
                                KeyValue::new(
                                    "txn_blob_gas_price",
                                    receipt.blob_gas_price.unwrap_or_default().to_string(),
                                ),
                            ],
                        );
                    }
                    Err(e) => {
                        error!("Failed to confirm validity proof txn: {e:?}");
                        meter_proofs_fail.add(
                            1,
                            &[
                                KeyValue::new("type", "validity"),
                                KeyValue::new("proposal", proposal.contract.to_string()),
                                KeyValue::new(
                                    "l2_height",
                                    proposal.output_block_number.to_string(),
                                ),
                                KeyValue::new("msg", e.to_string()),
                            ],
                        );
                        computed_proof_buffer
                            .push_back(Message::Proof(proposal_index, Some(receipt)));
                    }
                }
                // Skip fault proof submission logic
                continue;
            }

            // The index of the non-zero intermediate output to challenge
            let Some(fault) = proposal.fault() else {
                error!("Attempted output fault proof for correct proposal!");
                meter_proofs_discarded.add(
                    1,
                    &[
                        KeyValue::new("proposal", proposal.contract.to_string()),
                        KeyValue::new("reason", "unfalsifiable"),
                    ],
                );
                continue;
            };
            if !fault.is_output() {
                error!("Received output fault proof for trail fault!");
            }
            let divergence_point = fault.divergence_point() as u64;

            // Proofs of faulty trail data do not derive outputs beyond the parent proposal claim
            let output_fe = proposal.output_fe_at(divergence_point);

            // Sanity check proof data
            {
                let proof_output_root_fe = hash_to_fe(proof_journal.claimed_l2_output_root);
                if proof_output_root_fe != output_fe {
                    warn!(
                            "Proposal output fe {output_fe} doesn't match proof fe {proof_output_root_fe}",
                        );
                }
                let op_node_output = await_tel!(
                    context,
                    tracer,
                    "op_node_output",
                    retry_res_ctx_timeout!(
                        agent
                            .provider
                            .op_provider
                            .output_at_block(proof_journal.claimed_l2_block_number)
                            .await
                    )
                );
                if proof_journal.claimed_l2_output_root != op_node_output {
                    error!(
                        "Local op node output {op_node_output} doesn't match proof {}",
                        proof_journal.claimed_l2_output_root
                    );
                } else {
                    info!(
                        "Proven output matches local op node output {}:{op_node_output}.",
                        proof_journal.claimed_l2_block_number
                    );
                }

                let expected_block_number = parent.output_block_number
                    + (divergence_point + 1) * agent.deployment.output_block_span;
                if proof_journal.claimed_l2_block_number != expected_block_number {
                    warn!(
                            "Claimed l2 block number mismatch. Found {}, expected {expected_block_number}.",
                            proof_journal.claimed_l2_block_number
                        );
                } else {
                    info!("Claimed l2 block number {expected_block_number} confirmed.");
                }
            }

            // Skip proof submission if already proven
            let fault_proof_status = parent_contract
                .proofStatus(proposal.signature)
                .stall_with_context(context.clone(), "KailuaTournament::proofStatus")
                .await;
            if fault_proof_status != 0 {
                warn!("Skipping proof submission for already proven game at local index {proposal_index}.");
                meter_proofs_discarded.add(
                    1,
                    &[
                        KeyValue::new("proposal", proposal.contract.to_string()),
                        KeyValue::new("reason", "proven"),
                    ],
                );
                continue;
            } else {
                info!("Fault proof status: {fault_proof_status}");
            }

            // create kzg proofs
            let mut proofs = vec![];
            let mut commitments = vec![];

            // kzg proofs for agreed output hashes
            if divergence_point > 0 {
                commitments.push(proposal.io_commitment_for(divergence_point - 1));
                proofs.push(proposal.io_proof_for(divergence_point - 1)?);
            }

            // kzg proofs for claimed output hashes
            if proof_journal.claimed_l2_block_number != proposal.output_block_number {
                commitments.push(proposal.io_commitment_for(divergence_point));
                proofs.push(proposal.io_proof_for(divergence_point)?);
            }

            // sanity check kzg proofs
            {
                // check claimed output
                if proof_journal.claimed_l2_block_number == proposal.output_block_number {
                    if hash_to_fe(proposal.output_root) != output_fe {
                        warn!(
                            "Proposal proposed output root fe {} does not match submitted {}",
                            hash_to_fe(proposal.output_root),
                            output_fe
                        );
                    } else {
                        info!("Proposal proposed output confirmed.");
                    }
                } else {
                    let proposal_has_output = proposal_contract
                        .verifyIntermediateOutput(
                            divergence_point,
                            output_fe,
                            commitments.last().unwrap().clone(),
                            proofs.last().unwrap().clone(),
                        )
                        .stall_with_context(context.clone(), "KailuaGame::verifyIntermediateOutput")
                        .await;
                    if !proposal_has_output {
                        warn!("Could not verify proposed output");
                    } else {
                        info!("Proposed output confirmed.");
                    }
                }
                // check agreed output
                let is_agreed_output_confirmed = if divergence_point == 0 {
                    let parent_output_matches =
                        parent.output_root == proof_journal.agreed_l2_output_root;
                    if !parent_output_matches {
                        warn!(
                            "Parent claim {} is last common output and does not match {}",
                            parent.output_root, proof_journal.agreed_l2_output_root
                        );
                    } else {
                        info!(
                            "Parent output claim {} confirmed as last common output.",
                            parent.output_root
                        );
                    }
                    parent_output_matches
                } else {
                    let agreed_l2_output_root_fe = hash_to_fe(proof_journal.agreed_l2_output_root);
                    let proposal_has_output = proposal_contract
                        .verifyIntermediateOutput(
                            divergence_point - 1,
                            agreed_l2_output_root_fe,
                            commitments.first().unwrap().clone(),
                            proofs.first().unwrap().clone(),
                        )
                        .stall_with_context(context.clone(), "KailuaGame::verifyIntermediateOutput")
                        .await;
                    if !proposal_has_output {
                        warn!("Could not verify last common output for proposal");
                    } else {
                        info!("Proposal common output publication confirmed.");
                    }
                    proposal_has_output
                };
                if is_agreed_output_confirmed {
                    info!(
                        "Confirmed last common output: {}",
                        proof_journal.agreed_l2_output_root
                    );
                }
            }

            // sanity check precondition hash
            {
                if !proof_journal.precondition_hash.is_zero() {
                    warn!(
                        "Possible precondition hash mismatch. Expected {}, found {}",
                        B256::ZERO,
                        proof_journal.precondition_hash
                    );
                } else {
                    info!("Proof Precondition hash {} confirmed.", B256::ZERO)
                }
            }

            // sanity check config hash
            {
                let config_hash = parent_contract
                    .ROLLUP_CONFIG_HASH()
                    .stall_with_context(context.clone(), "KailuaTournament::ROLLUP_CONFIG_HASH")
                    .await;
                if proof_journal.config_hash != config_hash {
                    warn!(
                        "Config hash mismatch. Found {}, expected {config_hash}.",
                        proof_journal.config_hash
                    );
                } else {
                    info!("Proof Config hash confirmed.");
                }
            }

            info!(
                "Submitting output fault proof to tournament at index {} for child {child_index} with \
                divergence position {divergence_point} with {} kzg proof(s).",
                parent.index,
                proofs.len()
            );

            let transaction_dispatch = parent_contract
                .proveOutputFault(
                    [proof_journal.payout_recipient, *l1_head_contract],
                    [child_index, divergence_point],
                    encoded_seal.clone(),
                    [
                        proof_journal.agreed_l2_output_root,
                        proof_journal.claimed_l2_output_root,
                    ],
                    output_fe,
                    [commitments, proofs],
                )
                .timed_transact_with_context(
                    context.clone(),
                    "KailuaTournament::proveOutputFault",
                    Some(Duration::from_secs(args.txn_args.txn_timeout)),
                )
                .await
                .context("KailuaTournament::proveOutputFault");

            match transaction_dispatch {
                Ok(receipt) => {
                    info!("Output fault proof submitted: {receipt:?}");
                    let proof_status = parent_contract
                        .proofStatus(proposal.signature)
                        .stall_with_context(context.clone(), "KailuaTournament::proofStatus")
                        .await;
                    info!("Proposal {} proven: {proof_status}", proposal.index);
                    info!(
                        "KailuaTournament::proveOutputFault: {} gas",
                        receipt.gas_used
                    );

                    meter_proofs_published.add(
                        1,
                        &[
                            KeyValue::new("type", "fault_output"),
                            KeyValue::new("proposal", proposal.contract.to_string()),
                            KeyValue::new("l2_height", proposal.output_block_number.to_string()),
                            KeyValue::new("txn_hash", receipt.transaction_hash.to_string()),
                            KeyValue::new("txn_from", receipt.from.to_string()),
                            KeyValue::new("txn_to", receipt.to.unwrap_or_default().to_string()),
                            KeyValue::new("txn_gas_used", receipt.gas_used.to_string()),
                            KeyValue::new("txn_gas_price", receipt.effective_gas_price.to_string()),
                            KeyValue::new(
                                "txn_blob_gas_used",
                                receipt.blob_gas_used.unwrap_or_default().to_string(),
                            ),
                            KeyValue::new(
                                "txn_blob_gas_price",
                                receipt.blob_gas_price.unwrap_or_default().to_string(),
                            ),
                        ],
                    );
                }
                Err(e) => {
                    error!("Failed to confirm fault proof txn: {e:?}");
                    meter_proofs_fail.add(
                        1,
                        &[
                            KeyValue::new("type", "fault_output"),
                            KeyValue::new("proposal", proposal.contract.to_string()),
                            KeyValue::new("l2_height", proposal.output_block_number.to_string()),
                            KeyValue::new("msg", e.to_string()),
                        ],
                    );
                    computed_proof_buffer.push_back(Message::Proof(proposal_index, Some(receipt)));
                }
            }
        }
        // publish trail fault proofs
        let trail_fault_proof_count = trail_fault_buffer.len();
        for _ in 0..trail_fault_proof_count {
            let proposal_index = trail_fault_buffer.pop_front().unwrap();
            // Fetch proposal from db
            let Some(proposal) = agent.proposals.get(&proposal_index) else {
                if agent.cursor.last_resolved_game < proposal_index {
                    error!("Proposal {proposal_index} missing from database.");
                    trail_fault_buffer.push_back(proposal_index);
                } else {
                    warn!(
                        "Skipping trail fault proof submission for freed proposal {proposal_index}."
                    );
                }
                continue;
            };
            let proposal_contract =
                KailuaTournament::new(proposal.contract, &agent.provider.l1_provider);
            // Fetch proposal parent from db
            let Some(parent) = agent.proposals.get(&proposal.parent) else {
                if agent.cursor.last_resolved_game < proposal_index {
                    error!("Parent proposal {} missing from database.", proposal.parent);
                    trail_fault_buffer.push_back(proposal_index);
                } else {
                    warn!(
                        "Skipping trail fault proof submission for proposal {} with freed parent {}.",
                        proposal.index, proposal.parent
                    );
                }
                continue;
            };
            let parent_contract = KailuaTournament::new(parent.contract, &validator_provider);

            let Some(fault) = proposal.fault() else {
                error!("Attempted trail proof for correct proposal!");
                meter_proofs_discarded.add(
                    1,
                    &[
                        KeyValue::new("proposal", proposal.contract.to_string()),
                        KeyValue::new("reason", "unfalsifiable"),
                    ],
                );
                continue;
            };
            if !fault.is_trail() {
                error!("Attempting trail fault proof for output fault!");
            }
            let divergence_point = fault.divergence_point() as u64;
            let output_fe = proposal.output_fe_at(divergence_point);
            let fe_position = divergence_point - 1;

            if output_fe.is_zero() {
                error!("Proposal fe {output_fe} zeroness as expected.");
            } else {
                warn!("Proposal fe {output_fe} zeroness divergent.");
            }

            // Skip proof submission if already proven
            let fault_proof_status = parent_contract
                .proofStatus(proposal.signature)
                .stall_with_context(context.clone(), "KailuaTournament::proofStatus")
                .await;
            if fault_proof_status != 0 {
                warn!("Skipping proof submission for already proven game at local index {proposal_index}.");
                meter_proofs_discarded.add(
                    1,
                    &[
                        KeyValue::new("proposal", proposal.contract.to_string()),
                        KeyValue::new("reason", "proven"),
                    ],
                );
                continue;
            } else {
                info!("Fault proof status: {fault_proof_status}");
            }

            let blob_commitment = proposal.io_commitment_for(fe_position);
            let kzg_proof = proposal.io_proof_for(fe_position)?;

            // sanity check kzg proof
            {
                // check trail data
                if !proposal_contract
                    .verifyIntermediateOutput(
                        fe_position,
                        output_fe,
                        blob_commitment.clone(),
                        kzg_proof.clone(),
                    )
                    .stall_with_context(context.clone(), "KailuaGame::verifyIntermediateOutput")
                    .await
                {
                    warn!("Could not verify divergent trail output for proposal");
                } else {
                    info!("Proposal divergent trail output confirmed.");
                }
            }

            let child_index = parent
                .child_index(proposal.index)
                .expect("Could not look up proposal's index in parent tournament");

            info!(
                "Submitting trail fault proof to tournament at index {} for child {child_index} with \
                divergence position {divergence_point}.",
                parent.index
            );

            let transaction_dispatch = parent_contract
                .proveTrailFault(
                    validator_address,
                    [child_index, divergence_point],
                    output_fe,
                    blob_commitment,
                    kzg_proof,
                )
                .timed_transact_with_context(
                    context.clone(),
                    "KailuaTournament::proveTrailFault",
                    Some(Duration::from_secs(args.txn_args.txn_timeout)),
                )
                .await
                .context("KailuaTournament::proveTrailFault");

            match transaction_dispatch {
                Ok(receipt) => {
                    info!("Trail fault proof submitted: {receipt:?}");
                    let proof_status = parent_contract
                        .proofStatus(proposal.signature)
                        .stall_with_context(context.clone(), "KailuaTournament::proofStatus")
                        .await;
                    info!("Proposal {} proven: {proof_status}", proposal.index);
                    info!(
                        "KailuaTournament::proveTrailFault: {} gas",
                        receipt.gas_used
                    );

                    meter_proofs_published.add(
                        1,
                        &[
                            KeyValue::new("type", "fault_trail"),
                            KeyValue::new("proposal", proposal.contract.to_string()),
                            KeyValue::new("l2_height", proposal.output_block_number.to_string()),
                            KeyValue::new("txn_hash", receipt.transaction_hash.to_string()),
                            KeyValue::new("txn_from", receipt.from.to_string()),
                            KeyValue::new("txn_to", receipt.to.unwrap_or_default().to_string()),
                            KeyValue::new("txn_gas_used", receipt.gas_used.to_string()),
                            KeyValue::new("txn_gas_price", receipt.effective_gas_price.to_string()),
                            KeyValue::new(
                                "txn_blob_gas_used",
                                receipt.blob_gas_used.unwrap_or_default().to_string(),
                            ),
                            KeyValue::new(
                                "txn_blob_gas_price",
                                receipt.blob_gas_price.unwrap_or_default().to_string(),
                            ),
                        ],
                    );
                }
                Err(e) => {
                    error!("Failed to confirm fault proof txn: {e:?}");
                    meter_proofs_fail.add(
                        1,
                        &[
                            KeyValue::new("type", "fault_trail"),
                            KeyValue::new("proposal", proposal.contract.to_string()),
                            KeyValue::new("l2_height", proposal.output_block_number.to_string()),
                            KeyValue::new("msg", e.to_string()),
                        ],
                    );
                    trail_fault_buffer.push_back(proposal_index);
                }
            }
        }
    }
}

pub fn get_next_l1_head(
    agent: &SyncAgent,
    last_proof_l1_head: &mut BTreeMap<u64, u64>,
    proposal: &Proposal,
    #[cfg(feature = "devnet")] jump_back: u64,
) -> Option<B256> {
    // fetch next l1 head to use
    let l1_head = match last_proof_l1_head.get(&proposal.index) {
        None => Some(proposal.l1_head),
        Some(last_block_no) => agent
            .l1_heads
            .range((last_block_no + 1)..)
            .next()
            .map(|(_, (_, l1_head))| *l1_head),
    }?;
    // delay if necessary
    #[cfg(feature = "devnet")]
    let l1_head = if last_proof_l1_head.contains_key(&proposal.index) {
        l1_head
    } else {
        let (_, block_no) = *agent.l1_heads_inv.get(&l1_head).unwrap();
        let delayed_l1_head = agent
            .l1_heads
            .range(..block_no)
            .rev()
            .take(jump_back as usize)
            .last()
            .map(|(_, (_, delayed_head))| *delayed_head)
            .unwrap_or(l1_head);
        if delayed_l1_head != l1_head {
            warn!("(DEVNET ONLY) Forced l1 head rollback from {l1_head} to {delayed_l1_head}. Expect a proving error.");
        }
        delayed_l1_head
    };
    // update last head used
    let block_no = agent
        .l1_heads_inv
        .get(&l1_head)
        .expect("Missing l1 head from db")
        .1;
    last_proof_l1_head.insert(proposal.index, block_no);

    Some(l1_head)
}

/// Encode the seal of the given receipt for use with EVM smart contract verifiers.
///
/// Appends the verifier selector, determined from the first 4 bytes of the verifier parameters
/// including the Groth16 verification key and the control IDs that commit to the RISC Zero
/// circuits.
///
/// Copied from crate risc0-ethereum-contracts v2.0.2
pub fn encode_seal(receipt: &risc0_zkvm::Receipt) -> anyhow::Result<Vec<u8>> {
    let seal = match receipt.inner.clone() {
        InnerReceipt::Fake(receipt) => {
            let seal = receipt.claim.digest().as_bytes().to_vec();
            let selector = &[0xFFu8; 4];
            // Create a new vector with the capacity to hold both selector and seal
            let mut selector_seal = Vec::with_capacity(selector.len() + seal.len());
            selector_seal.extend_from_slice(selector);
            selector_seal.extend_from_slice(&seal);
            selector_seal
        }
        InnerReceipt::Groth16(receipt) => {
            let selector = &receipt.verifier_parameters.as_bytes()[..4];
            // Create a new vector with the capacity to hold both selector and seal
            let mut selector_seal = Vec::with_capacity(selector.len() + receipt.seal.len());
            selector_seal.extend_from_slice(selector);
            selector_seal.extend_from_slice(receipt.seal.as_ref());
            selector_seal
        }
        _ => bail!("Unsupported receipt type"),
        // TODO(victor): Add set verifier seal here.
    };
    Ok(seal)
}

#[cfg(feature = "devnet")]
pub fn maybe_patch_proof(
    mut receipt: risc0_zkvm::Receipt,
    expected_fpvm_image_id: [u8; 32],
) -> anyhow::Result<risc0_zkvm::Receipt> {
    // Return the proof if we can't patch it
    if !risc0_zkvm::is_dev_mode() {
        return Ok(receipt);
    }

    let expected_fpvm_image_id = risc0_zkvm::sha::Digest::from(expected_fpvm_image_id);

    // Patch the image id of the receipt to match the expected one
    if let risc0_zkvm::InnerReceipt::Fake(fake_inner_receipt) = &mut receipt.inner {
        if let risc0_zkvm::MaybePruned::Value(claim) = &mut fake_inner_receipt.claim {
            tracing::warn!("DEV-MODE ONLY: Patching fake receipt image id to match game contract.");
            claim.pre = risc0_zkvm::MaybePruned::Pruned(expected_fpvm_image_id);
            if let risc0_zkvm::MaybePruned::Value(Some(output)) = &mut claim.output {
                if let risc0_zkvm::MaybePruned::Value(journal) = &mut output.journal {
                    let n = journal.len();
                    journal[n - 32..n].copy_from_slice(expected_fpvm_image_id.as_bytes());
                    receipt.journal.bytes[n - 32..n]
                        .copy_from_slice(expected_fpvm_image_id.as_bytes());
                }
            }
        }
    }
    Ok(receipt)
}
