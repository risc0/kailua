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
use crate::channel::{DuplexChannel, Message};
use crate::proposals::dispatch::current_time;
use crate::proposals::encode_seal;
use alloy::primitives::Bytes;
use alloy::primitives::B256;
use alloy::providers::Provider;
use anyhow::Context;
use kailua_common::blobs::hash_to_fe;
use kailua_common::journal::ProofJournal;
use kailua_common::precondition::validity_precondition_hash;
use kailua_contracts::*;
use kailua_sync::agent::SyncAgent;
use kailua_sync::stall::Stall;
use kailua_sync::transact::Transact;
use kailua_sync::{await_tel, retry_res_ctx_timeout};
use opentelemetry::global::tracer;
use opentelemetry::metrics::Counter;
use opentelemetry::trace::FutureExt;
use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::KeyValue;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, VecDeque};
use std::time::Duration;
use tracing::{error, info, warn};

#[allow(clippy::too_many_arguments)]
pub async fn publish_receipt_proofs<P: Provider>(
    args: &ValidateArgs,
    agent: &mut SyncAgent,
    computed_proof_buffer: &mut VecDeque<Message>,
    proposal_validity_buffer: &mut BinaryHeap<(Reverse<u64>, u64)>,
    output_fault_buffer: &mut BinaryHeap<(Reverse<u64>, u64)>,
    meter_proofs_completed: &Counter<u64>,
    meter_proofs_discarded: &Counter<u64>,
    meter_proofs_published: &Counter<u64>,
    meter_proofs_fail: &Counter<u64>,
    channel: &mut DuplexChannel<Message>,
    validator_provider: &P,
) {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("publish_receipt_proofs"));

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
        ) {
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
        let parent_contract = KailuaTournament::new(parent.contract, validator_provider);
        let expected_fpvm_image_id = parent_contract
            .FPVM_IMAGE_ID()
            .stall_with_context(context.clone(), "KailuaTournament::FPVM_IMAGE_ID")
            .await
            .0;

        // advance l1 head if insufficient data
        let Some(receipt) = receipt else {
            // request another proof with new head
            if proposal.canonical.unwrap_or_default() || proposal.io_blobs.is_empty() {
                proposal_validity_buffer.push((Reverse(current_time()), proposal_index));
            } else {
                output_fault_buffer.push((Reverse(current_time()), proposal_index));
            }
            continue;
        };

        // patch the proof if in dev mode
        #[cfg(feature = "devnet")]
        let receipt = match crate::proposals::devnet::maybe_patch_proof(
            receipt.clone(),
            expected_fpvm_image_id,
        ) {
            Ok(receipt) => receipt,
            Err(err) => {
                error!("(DEVNET) Failed to patch proof: {err:?}");
                receipt
            }
        };

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
        let Ok(encoded_seal) = encode_seal(&receipt).map(Bytes::from) else {
            error!("Skipped proof submission. Failed to encode receipt seal.");
            continue;
        };

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
                if proof_journal.fpvm_version.0 != expected_fpvm_image_id {
                    warn!(
                        "Proof FPVM Image ID {} does not match expected {}",
                        proof_journal.fpvm_version,
                        B256::from(expected_fpvm_image_id)
                    );
                } else {
                    info!("FPVM Image ID {} confirmed", proof_journal.fpvm_version);
                }
                let expected_block_number = parent.output_block_number
                    + agent.deployment.proposal_output_count * agent.deployment.output_block_span;
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
                    error!("Failed to confirm validity proof txn: {e:?}");
                    meter_proofs_fail.add(
                        1,
                        &[
                            KeyValue::new("type", "validity"),
                            KeyValue::new("proposal", proposal.contract.to_string()),
                            KeyValue::new("l2_height", proposal.output_block_number.to_string()),
                            KeyValue::new("msg", e.to_string()),
                        ],
                    );
                    computed_proof_buffer.push_back(Message::Proof(proposal_index, Some(receipt)));
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
            match proposal.io_proof_for(divergence_point - 1) {
                Ok(io_proof) => proofs.push(io_proof),
                Err(err) => {
                    error!(
                        "Failed to compute io proof at {}: {err:?}.",
                        divergence_point - 1
                    );
                }
            }
        }

        // kzg proofs for claimed output hashes
        if proof_journal.claimed_l2_block_number != proposal.output_block_number {
            commitments.push(proposal.io_commitment_for(divergence_point));
            match proposal.io_proof_for(divergence_point) {
                Ok(io_proof) => proofs.push(io_proof),
                Err(err) => {
                    error!("Failed to compute io proof at {divergence_point}: {err:?}.");
                }
            }
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
}
