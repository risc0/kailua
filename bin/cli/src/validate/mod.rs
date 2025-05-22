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

pub mod proving;

use crate::channel::DuplexChannel;
use crate::db::config::Config;
use crate::db::proposal::Proposal;
use crate::db::KailuaDB;
use crate::transact::blob::BlobProvider;
use crate::transact::provider::SafeProvider;
use crate::transact::rpc::{get_block_by_number, get_next_block};
use crate::transact::signer::ValidatorSignerArgs;
use crate::transact::{Transact, TransactArgs};
use crate::validate::proving::{create_proving_args, encode_seal, Task};
use crate::{retry_with_context, stall::Stall, CoreArgs, KAILUA_GAME_TYPE};
use alloy::eips::eip4844::IndexedBlobHash;
use alloy::network::primitives::HeaderResponse;
use alloy::network::{BlockResponse, Ethereum};
use alloy::primitives::{Address, Bytes, FixedBytes, B256};
use alloy::providers::RootProvider;
use anyhow::{anyhow, bail, Context};
use kailua_build::KAILUA_FPVM_ID;
use kailua_client::args::parse_address;
use kailua_client::boundless::BoundlessArgs;
use kailua_client::proof::{proof_file_name, read_proof_file};
use kailua_client::provider::OpNodeProvider;
use kailua_client::telemetry::TelemetryArgs;
use kailua_client::{await_tel, await_tel_res};
use kailua_common::blobs::hash_to_fe;
use kailua_common::blobs::BlobFetchRequest;
use kailua_common::config::config_hash;
use kailua_common::journal::ProofJournal;
use kailua_common::precondition::{validity_precondition_hash, PreconditionValidationData};
use kailua_contracts::*;
use kailua_host::channel::AsyncChannel;
use kailua_host::config::fetch_rollup_config;
use kona_protocol::BlockInfo;
use opentelemetry::global::{meter, tracer};
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use opentelemetry::KeyValue;
use risc0_zkvm::{is_dev_mode, Receipt};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::exit;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::mpsc::Sender;
use tokio::time::sleep;
use tokio::{spawn, try_join};
use tracing::{debug, error, info, warn};

#[derive(clap::Args, Debug, Clone)]
pub struct ValidateArgs {
    #[clap(flatten)]
    pub core: CoreArgs,

    /// Path to the kailua host binary to use for proving
    #[clap(long, env)]
    pub kailua_host: PathBuf,
    /// Fast-forward block height
    #[clap(long, env, required = false, default_value_t = 0)]
    pub fast_forward_target: u64,
    /// How many proofs to compute simultaneously
    #[clap(long, env, default_value_t = 1)]
    pub num_concurrent_hosts: u64,

    /// Secret key of L1 wallet to use for challenging and proving outputs
    #[clap(flatten)]
    pub validator_signer: ValidatorSignerArgs,
    /// Transaction publication configuration
    #[clap(flatten)]
    pub txn_args: TransactArgs,
    /// Address of the recipient account to use for bond payouts
    #[clap(long, env, value_parser = parse_address)]
    pub payout_recipient_address: Option<Address>,
    /// Address of the KailuaGame implementation to use
    #[clap(long, env, value_parser = parse_address)]
    pub kailua_game_implementation: Option<Address>,

    #[clap(flatten)]
    pub boundless: BoundlessArgs,

    #[clap(flatten)]
    pub telemetry: TelemetryArgs,
}

pub async fn validate(args: ValidateArgs, data_dir: PathBuf) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("validate"));

    // We run two concurrent tasks, one for the chain, and one for the prover.
    // Both tasks communicate using the duplex channel
    let channel_pair = DuplexChannel::new_pair(4096);

    let handle_proposals = spawn(
        handle_proposals(channel_pair.0, args.clone(), data_dir.clone())
            .with_context(context.clone()),
    );
    let handle_proof_requests =
        spawn(handle_proof_requests(channel_pair.1, args, data_dir).with_context(context.clone()));

    let (proposals_task, proofs_task) = try_join!(handle_proposals, handle_proof_requests)?;
    proposals_task.context("handle_proposals")?;
    proofs_task.context("handle_proofs")?;

    Ok(())
}

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Message {
    // The proposal and its parent
    Proposal {
        index: u64,
        precondition_validation_data: Option<PreconditionValidationData>,
        l1_head: FixedBytes<32>,
        agreed_l2_head_hash: FixedBytes<32>,
        agreed_l2_output_root: FixedBytes<32>,
        claimed_l2_block_number: u64,
        claimed_l2_output_root: FixedBytes<32>,
    },
    Proof(u64, Receipt),
}

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
    let meter_sync_canonical = meter.u64_gauge("validator.sync.canonical").build();
    let meter_sync_next = meter.u64_gauge("validator.sync.next").build();
    let meter_proofs_requested = meter.u64_counter("validator.proofs.requested").build();
    let meter_proofs_completed = meter.u64_counter("validator.proofs.complete").build();
    let meter_proofs_published = meter.u64_counter("validator.proofs.published").build();
    let meter_proofs_fail = meter.u64_counter("validator.proofs.errs").build();
    let meter_proofs_discarded = meter.u64_counter("validator.proofs.discarded").build();
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("handle_proposals"));

    // initialize blockchain connections
    info!("Initializing rpc connections.");
    let op_node_provider = OpNodeProvider(RootProvider::new_http(
        args.core.op_node_url.as_str().try_into()?,
    ));
    let eth_rpc_provider = RootProvider::new_http(args.core.eth_rpc_url.as_str().try_into()?);
    let op_geth_provider = RootProvider::new_http(args.core.op_geth_url.as_str().try_into()?);
    let cl_node_provider = await_tel!(context, BlobProvider::new(args.core.beacon_rpc_url))
        .context("BlobProvier::new")?;

    info!("Fetching rollup configuration from rpc endpoints.");
    // fetch rollup config
    let config = await_tel!(
        context,
        fetch_rollup_config(&args.core.op_node_url, &args.core.op_geth_url, None)
    )
    .context("fetch_rollup_config")?;
    let rollup_config_hash = config_hash(&config).expect("Configuration hash derivation error");
    info!("RollupConfigHash({})", hex::encode(rollup_config_hash));

    // load system config
    let system_config = SystemConfig::new(config.l1_system_config_address, &eth_rpc_provider);
    let dgf_address = system_config
        .disputeGameFactory()
        .stall_with_context(context.clone(), "SystemConfig::disputeGameFactory")
        .await;

    // initialize validator wallet
    info!("Initializing validator wallet.");
    let validator_wallet = await_tel_res!(
        context,
        tracer,
        "ValidatorSigner::walet",
        args.validator_signer.wallet(Some(config.l1_chain_id))
    )?;
    let validator_address = validator_wallet.default_signer().address();
    let validator_provider = SafeProvider::new(
        args.txn_args
            .premium_provider::<Ethereum>()
            .wallet(validator_wallet)
            .connect_http(args.core.eth_rpc_url.as_str().try_into()?),
    );
    info!("Validator address: {validator_address}");

    // Init factory contract
    let dispute_game_factory = IDisputeGameFactory::new(dgf_address, &eth_rpc_provider);
    info!("DisputeGameFactory({:?})", dispute_game_factory.address());
    let game_count: u64 = dispute_game_factory
        .gameCount()
        .stall_with_context(context.clone(), "DisputeGameFactory::gameCount")
        .await
        .to();
    info!("There have been {game_count} games created using DisputeGameFactory");

    // Look up deployment to target
    let latest_game_impl_addr = dispute_game_factory
        .gameImpls(KAILUA_GAME_TYPE)
        .stall_with_context(context.clone(), "DisputeGameFactory::gameImpls")
        .await;
    let kailua_game_implementation_address = args
        .kailua_game_implementation
        .unwrap_or(latest_game_impl_addr);
    if args.kailua_game_implementation.is_some() {
        warn!("Using provided KailuaGame implementation {kailua_game_implementation_address}.");
    } else {
        info!("Using latest KailuaGame implementation {kailua_game_implementation_address} from DisputeGameFactory.");
    }

    let kailua_game_implementation =
        KailuaGame::new(kailua_game_implementation_address, &eth_rpc_provider);
    info!("KailuaGame({:?})", kailua_game_implementation.address());
    if kailua_game_implementation.address().is_zero() {
        error!("Fault proof game is not installed!");
        exit(1);
    }
    // Initialize empty DB
    info!("Initializing..");
    let mut kailua_db = await_tel!(
        context,
        KailuaDB::init(
            data_dir,
            &dispute_game_factory,
            kailua_game_implementation_address
        )
    )
    .context("KailuaDB::init")?;
    info!("KailuaTreasury({:?})", kailua_db.treasury.address);
    // Run the validator loop
    info!(
        "Starting from proposal at factory index {}",
        kailua_db.state.next_factory_index
    );
    // init channel buffers
    let mut output_fault_proof_buffer = VecDeque::new();
    let mut output_fault_buffer = VecDeque::new();
    let mut null_fault_buffer = VecDeque::new();
    let mut valid_buffer = VecDeque::new();
    loop {
        // Wait for new data on every iteration
        sleep(Duration::from_secs(1)).await;
        // fetch latest games
        let loaded_proposals = await_tel!(
            context,
            kailua_db.load_proposals(&dispute_game_factory, &op_node_provider, &cl_node_provider)
        )
        .context("load_proposals")?;

        // Update sync telemetry
        if let Some(canonical_tip) = kailua_db.canonical_tip() {
            meter_sync_canonical.record(
                canonical_tip.index,
                &[
                    KeyValue::new("proposal", canonical_tip.contract.to_string()),
                    KeyValue::new("l2_height", canonical_tip.output_block_number.to_string()),
                ],
            );
        };
        meter_sync_next.record(kailua_db.state.next_factory_index, &[]);

        // check new proposals for fault and queue potential responses
        for proposal_index in loaded_proposals {
            let Some(proposal) = kailua_db.get_local_proposal(&proposal_index) else {
                error!("Proposal {proposal_index} missing from database.");
                continue;
            };
            // Skip Treasury instance
            if !proposal.has_parent() {
                info!("Skipping proving for treasury instance.");
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
            let Some(parent) = kailua_db.get_local_proposal(&proposal.parent) else {
                error!(
                    "Proposal {} parent {} missing from database.",
                    proposal.index, proposal.parent
                );
                continue;
            };
            let parent_contract = parent.tournament_contract_instance(&eth_rpc_provider);
            // Check that a validity proof has not already been posted
            let is_validity_proven = await_tel!(
                context,
                parent.fetch_is_successor_validity_proven(&eth_rpc_provider)
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
            if kailua_db.config.proposal_output_count == 1 {
                // Check if there is a faulty predecessor
                let is_prior_fault =
                    parent
                        .children
                        .iter()
                        .filter(|p| **p < proposal_index)
                        .any(|p| {
                            // Fetch predecessor from db
                            let Some(predecessor) = kailua_db.get_local_proposal(p) else {
                                error!("Proposal {p} missing from database.");
                                return false;
                            };
                            if kailua_db.was_proposer_eliminated_before(&predecessor) {
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
                        let Some(predecessor) = kailua_db.get_local_proposal(p) else {
                            error!("Proposal {p} missing from database.");
                            return false;
                        };
                        if kailua_db.was_proposer_eliminated_before(&predecessor) {
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
                // Queue null fault proof submission
                null_fault_buffer.push_back(proposal_index);
            }
        }

        // dispatch buffered output fault proof requests
        let output_fault_proof_requests = output_fault_buffer.len();
        for _ in 0..output_fault_proof_requests {
            let proposal_index = output_fault_buffer.pop_front().unwrap();
            let Some(proposal) = kailua_db.get_local_proposal(&proposal_index) else {
                error!("Proposal {proposal_index} missing from database.");
                output_fault_buffer.push_back(proposal_index);
                continue;
            };
            // Look up parent proposal
            let Some(parent) = kailua_db.get_local_proposal(&proposal.parent) else {
                error!(
                    "Proposal {} parent {} missing from database.",
                    proposal.index, proposal.parent
                );
                output_fault_buffer.push_back(proposal_index);
                continue;
            };

            if let Err(err) = await_tel!(
                context,
                request_fault_proof(
                    &mut channel,
                    &kailua_db.config,
                    &parent,
                    &proposal,
                    &op_geth_provider,
                    &op_node_provider,
                )
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
            let Some(proposal) = kailua_db.get_local_proposal(&proposal_index) else {
                error!("Proposal {proposal_index} missing from database.");
                valid_buffer.push_front(proposal_index);
                continue;
            };
            // Look up parent proposal
            let Some(parent) = kailua_db.get_local_proposal(&proposal.parent) else {
                error!(
                    "Proposal {} parent {} missing from database.",
                    proposal.index, proposal.parent
                );
                valid_buffer.push_front(proposal_index);
                continue;
            };

            let parent_contract = parent.tournament_contract_instance(&eth_rpc_provider);
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

            if let Err(err) = await_tel!(
                context,
                request_validity_proof(
                    &mut channel,
                    &kailua_db.config,
                    &parent,
                    &proposal,
                    &eth_rpc_provider,
                    &op_geth_provider,
                )
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
            output_fault_proof_buffer.push_back(message);
        }

        // publish computed output fault proofs
        let computed_proofs = output_fault_proof_buffer.len();
        for _ in 0..computed_proofs {
            let Some(Message::Proof(proposal_index, receipt)) =
                output_fault_proof_buffer.pop_front()
            else {
                error!("Validator loop received an unexpected message.");
                continue;
            };
            let Some(proposal) = kailua_db.get_local_proposal(&proposal_index) else {
                error!("Proposal {proposal_index} missing from database.");
                output_fault_proof_buffer.push_back(Message::Proof(proposal_index, receipt));
                continue;
            };
            let Some(parent) = kailua_db.get_local_proposal(&proposal.parent) else {
                error!("Parent proposal {} missing from database.", proposal.parent);
                output_fault_proof_buffer.push_back(Message::Proof(proposal_index, receipt));
                continue;
            };
            // Abort early if a validity proof is already submitted in this tournament
            if await_tel!(
                context,
                parent.fetch_is_successor_validity_proven(&eth_rpc_provider)
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
            let parent_contract = parent.tournament_contract_instance(&validator_provider);
            let expected_fpvm_image_id = parent_contract
                .FPVM_IMAGE_ID()
                .stall_with_context(context.clone(), "KailuaTournament::FPVM_IMAGE_ID")
                .await
                .0;
            // patch the proof if in dev mode
            #[cfg(feature = "devnet")]
            let receipt = proving::maybe_patch_proof(receipt, expected_fpvm_image_id)?;
            // verify that the zkvm receipt is valid
            if let Err(e) = receipt.verify(expected_fpvm_image_id) {
                error!("Could not verify receipt against image id in contract: {e:?}");
            } else {
                info!("Receipt validated.");
            }
            // Decode ProofJournal
            let proof_journal = ProofJournal::decode_packed(receipt.journal.as_ref());
            info!("Proof journal: {:?}", proof_journal);
            // encode seal data
            let encoded_seal = Bytes::from(encode_seal(&receipt)?);

            let child_index = parent
                .child_index(proposal.index)
                .expect("Could not look up proposal's index in parent tournament");
            let proposal_contract = proposal.tournament_contract_instance(&eth_rpc_provider);
            // Check if proof is a viable validity proof
            if proof_journal.l1_head == proposal.l1_head
                && proof_journal.agreed_l2_output_root == parent.output_root
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
                        &kailua_db.config.proposal_output_count,
                        &kailua_db.config.output_block_span,
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
                        + kailua_db.config.proposal_output_count
                            * kailua_db.config.output_block_span;
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
                        child_index,
                        encoded_seal.clone(),
                    )
                    .transact_with_context(context.clone(), "KailuaTournament::proveValidity")
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
                        output_fault_proof_buffer
                            .push_back(Message::Proof(proposal_index, receipt));
                    }
                }
                // Skip fault proof submission logic
                continue;
            }

            // The index of the non-zero intermediate output to challenge
            let Some(fault) = proposal.fault() else {
                error!("Attempted output proof for correct proposal!");
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
                error!("Received computed proof for null fault!");
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
                let op_node_output = await_tel_res!(
                    context,
                    tracer,
                    "op_node_output",
                    retry_with_context!(
                        op_node_provider.output_at_block(proof_journal.claimed_l2_block_number)
                    )
                )?;
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

                if proof_journal.l1_head != proposal.l1_head {
                    warn!(
                        "L1 head mismatch. Found {}, expected {}.",
                        proof_journal.l1_head, proposal.l1_head
                    );
                } else {
                    info!("Proof L1 head {} confirmed.", proposal.l1_head);
                }

                let expected_block_number = parent.output_block_number
                    + (divergence_point + 1) * kailua_db.config.output_block_span;
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
                    proof_journal.payout_recipient,
                    [child_index, divergence_point],
                    encoded_seal.clone(),
                    proof_journal.agreed_l2_output_root,
                    output_fe,
                    proof_journal.claimed_l2_output_root,
                    commitments,
                    proofs,
                )
                .transact_with_context(context.clone(), "KailuaTournament::proveOutputFault")
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
                    output_fault_proof_buffer.push_back(Message::Proof(proposal_index, receipt));
                }
            }
        }
        // publish null fault proofs
        let null_fault_proof_count = null_fault_buffer.len();
        for _ in 0..null_fault_proof_count {
            let proposal_index = null_fault_buffer.pop_front().unwrap();
            // Fetch proposal from db
            let Some(proposal) = kailua_db.get_local_proposal(&proposal_index) else {
                error!("Proposal {proposal_index} missing from database.");
                null_fault_buffer.push_back(proposal_index);
                continue;
            };
            let proposal_contract = proposal.tournament_contract_instance(&eth_rpc_provider);
            // Fetch proposal parent from db
            let Some(parent) = kailua_db.get_local_proposal(&proposal.parent) else {
                error!("Parent proposal {} missing from database.", proposal.parent);
                null_fault_buffer.push_back(proposal_index);
                continue;
            };
            let parent_contract = parent.tournament_contract_instance(&validator_provider);

            let Some(fault) = proposal.fault() else {
                error!("Attempted null proof for correct proposal!");
                meter_proofs_discarded.add(
                    1,
                    &[
                        KeyValue::new("proposal", proposal.contract.to_string()),
                        KeyValue::new("reason", "unfalsifiable"),
                    ],
                );
                continue;
            };
            if !fault.is_null() {
                error!("Attempting null proof for output fault!");
            }
            let divergence_point = fault.divergence_point() as u64;
            let output_fe = proposal.output_fe_at(divergence_point);
            let expect_zero_fe = fault.expect_zero(&proposal);
            let fe_position = if expect_zero_fe {
                divergence_point - 1
            } else {
                divergence_point
            };

            if expect_zero_fe == output_fe.is_zero() {
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
                .proveNullFault(
                    validator_address,
                    [child_index, divergence_point],
                    output_fe,
                    blob_commitment,
                    kzg_proof,
                )
                .transact_with_context(context.clone(), "KailuaTournament::proveNullFault")
                .await
                .context("KailuaTournament::proveNullFault");

            match transaction_dispatch {
                Ok(receipt) => {
                    info!("Trail fault proof submitted: {receipt:?}");
                    let proof_status = parent_contract
                        .proofStatus(proposal.signature)
                        .stall_with_context(context.clone(), "KailuaTournament::proofStatus")
                        .await;
                    info!("Proposal {} proven: {proof_status}", proposal.index);
                    info!("KailuaTournament::proveNullFault: {} gas", receipt.gas_used);

                    meter_proofs_published.add(
                        1,
                        &[
                            KeyValue::new("type", "fault_null"),
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
                            KeyValue::new("type", "fault_null"),
                            KeyValue::new("proposal", proposal.contract.to_string()),
                            KeyValue::new("l2_height", proposal.output_block_number.to_string()),
                            KeyValue::new("msg", e.to_string()),
                        ],
                    );
                    null_fault_buffer.push_back(proposal_index);
                }
            }
        }
    }
}

async fn request_fault_proof(
    channel: &mut DuplexChannel<Message>,
    config: &Config,
    parent: &Proposal,
    proposal: &Proposal,
    l2_node_provider: &RootProvider,
    op_node_provider: &OpNodeProvider,
) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("request_fault_proof"));

    let Some(fault) = proposal.fault() else {
        error!("Proposal {} does not diverge from canon.", proposal.index);
        return Ok(());
    };
    let divergence_point = fault.divergence_point() as u64;

    // Read additional data for Kona invocation
    info!(
        "Requesting fault proof for proposal {} at point {divergence_point}.",
        proposal.index
    );

    // Set L2 Head Number: start from the last common transition
    let agreed_l2_head_number =
        parent.output_block_number + config.output_block_span * divergence_point;
    debug!("l2_head_number {:?}", &agreed_l2_head_number);

    // Get L2 head hash
    let agreed_l2_head_hash = await_tel!(
        context,
        get_block_by_number(&l2_node_provider, agreed_l2_head_number,)
    )?
    .header()
    .hash();
    debug!("l2_head {:?}", &agreed_l2_head_hash);

    // Get L2 head output root
    let agreed_l2_output_root = await_tel_res!(
        context,
        tracer,
        "output_at_block",
        retry_with_context!(op_node_provider.output_at_block(agreed_l2_head_number))
    )?;

    // Prepare expected output commitment: target the first bad transition
    let claimed_l2_block_number = agreed_l2_head_number + config.output_block_span;
    let claimed_l2_output_root = await_tel_res!(
        context,
        tracer,
        "claimed_l2_output_root",
        retry_with_context!(op_node_provider.output_at_block(claimed_l2_block_number))
    )?;

    // Set appropriate L1 head
    let l1_head = proposal.l1_head;

    // Message proving task
    channel
        .sender
        .send(Message::Proposal {
            index: proposal.index,
            precondition_validation_data: None,
            l1_head,
            agreed_l2_head_hash,
            agreed_l2_output_root,
            claimed_l2_block_number,
            claimed_l2_output_root,
        })
        .await?;
    Ok(())
}

async fn request_validity_proof(
    channel: &mut DuplexChannel<Message>,
    config: &Config,
    parent: &Proposal,
    proposal: &Proposal,
    l1_node_provider: &RootProvider,
    l2_node_provider: &RootProvider,
) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("request_validity_proof"));

    let precondition_validation_data = if config.proposal_output_count > 1 {
        let mut validated_blobs = Vec::with_capacity(proposal.io_blobs.len());
        debug_assert!(!proposal.io_blobs.is_empty());
        for (blob_hash, blob) in &proposal.io_blobs {
            let block = await_tel!(context, get_next_block(&l1_node_provider, proposal.l1_head))
                .context("block")?;

            validated_blobs.push(BlobFetchRequest {
                block_ref: BlockInfo {
                    hash: block.header.hash,
                    number: block.header.number,
                    parent_hash: block.header.parent_hash,
                    timestamp: block.header.timestamp,
                },
                blob_hash: IndexedBlobHash {
                    index: blob.index,
                    hash: *blob_hash,
                },
            })
        }
        debug_assert!(!validated_blobs.is_empty());
        Some(PreconditionValidationData::Validity {
            proposal_l2_head_number: parent.output_block_number,
            proposal_output_count: config.proposal_output_count,
            output_block_span: config.output_block_span,
            blob_hashes: validated_blobs,
        })
    } else {
        None
    };
    // Get L2 head hash
    let agreed_l2_head_hash = await_tel!(
        context,
        get_block_by_number(&l2_node_provider, parent.output_block_number)
    )?
    .header
    .hash;
    debug!("l2_head {:?}", &agreed_l2_head_hash);
    // Message proving task
    channel
        .sender
        .send(Message::Proposal {
            index: proposal.index,
            precondition_validation_data,
            l1_head: proposal.l1_head,
            agreed_l2_head_hash,
            agreed_l2_output_root: parent.output_root,
            claimed_l2_block_number: proposal.output_block_number,
            claimed_l2_output_root: proposal.output_root,
        })
        .await?;
    Ok(())
}

pub async fn handle_proof_requests(
    mut channel: DuplexChannel<Message>,
    args: ValidateArgs,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    // Telemetry
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("handle_proof_requests"));

    // Fetch rollup configuration
    let rollup_config = await_tel!(
        context,
        fetch_rollup_config(&args.core.op_node_url, &args.core.op_geth_url, None)
    )
    .context("fetch_rollup_config")?;
    let l2_chain_id = rollup_config.l2_chain_id.to_string();
    let config_hash = B256::from(config_hash(&rollup_config)?);
    let fpvm_image_id = B256::from(bytemuck::cast::<[u32; 8], [u8; 32]>(KAILUA_FPVM_ID));
    // Set payout recipient
    let validator_wallet = await_tel_res!(
        context,
        tracer,
        "ValidatorSigner::wallet",
        args.validator_signer
            .wallet(Some(rollup_config.l1_chain_id))
    )?;
    let payout_recipient = args
        .payout_recipient_address
        .unwrap_or_else(|| validator_wallet.default_signer().address());
    info!("Proof payout recipient: {payout_recipient}");

    let task_channel: AsyncChannel<Task> = async_channel::unbounded();
    let mut proving_handlers = vec![];
    // instantiate worker pool
    for _ in 0..args.num_concurrent_hosts {
        proving_handlers.push(spawn(handle_proving_tasks(
            args.kailua_host.clone(),
            task_channel.clone(),
            channel.sender.clone(),
        )));
    }

    // Run proof generator loop
    loop {
        // Dequeue messages
        let Message::Proposal {
            index: proposal_index,
            precondition_validation_data,
            l1_head,
            agreed_l2_head_hash,
            agreed_l2_output_root,
            claimed_l2_block_number,
            claimed_l2_output_root,
        } = channel
            .receiver
            .recv()
            .await
            .ok_or(anyhow!("proof receiver channel closed"))?
        else {
            bail!("Unexpected message type.");
        };
        info!("Processing proof for local index {proposal_index}.");
        // Compute proof file name
        let precondition_hash = precondition_validation_data
            .as_ref()
            .map(|d| d.precondition_hash())
            .unwrap_or_default();
        let proof_journal = ProofJournal {
            payout_recipient,
            precondition_hash,
            l1_head,
            agreed_l2_output_root,
            claimed_l2_output_root,
            claimed_l2_block_number,
            config_hash,
            fpvm_image_id,
        };
        let proof_file_name = proof_file_name(&proof_journal);
        // Prepare kailua-host proving args
        let proving_args = create_proving_args(
            &args,
            data_dir.clone(),
            l2_chain_id.clone(),
            payout_recipient,
            precondition_validation_data,
            l1_head,
            agreed_l2_head_hash,
            agreed_l2_output_root,
            claimed_l2_block_number,
            claimed_l2_output_root,
        );
        // Send to task pool
        task_channel
            .0
            .send(Task {
                proposal_index,
                proving_args,
                proof_file_name,
            })
            .await
            .context("task channel closed")?;
    }
}

pub async fn handle_proving_tasks(
    kailua_host: PathBuf,
    task_channel: AsyncChannel<Task>,
    proof_sender: Sender<Message>,
) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("handle_proving_tasks"));

    loop {
        let Task {
            proposal_index,
            proving_args,
            proof_file_name,
        } = task_channel
            .1
            .recv()
            .await
            .context("task receiver channel closed")?;

        // Prove via kailua-host (re dev mode/bonsai: env vars inherited!)
        let mut kailua_host_command = Command::new(&kailua_host);
        // get fake receipts when building under devnet
        if is_dev_mode() {
            kailua_host_command.env("RISC0_DEV_MODE", "1");
        }
        // pass arguments to point at target block
        kailua_host_command.args(proving_args.clone());
        debug!("kailua_host_command {:?}", &kailua_host_command);
        // call the kailua-host binary to generate a proof
        match await_tel_res!(
            context,
            tracer,
            "KailuaHost",
            kailua_host_command
                .kill_on_drop(true)
                .spawn()
                .context("Invoking kailua-host")?
                .wait()
        ) {
            Ok(proving_task) => {
                if !proving_task.success() {
                    error!("Proving task failure. Exit code: {proving_task}");
                } else {
                    info!("Proving task successful.");
                }
            }
            Err(e) => {
                error!("Failed to invoke kailua-host: {e:?}");
            }
        }
        // wait for io then read computed proof from disk
        sleep(Duration::from_secs(1)).await;
        match read_proof_file(&proof_file_name).await {
            Ok(proof) => {
                // Send proof via the channel
                proof_sender
                    .send(Message::Proof(proposal_index, proof))
                    .await?;
                info!("Proof for local index {proposal_index} complete.");
            }
            Err(e) => {
                error!("Failed to read proof file: {e:?}");
                // retry proving task
                task_channel
                    .0
                    .send(Task {
                        proposal_index,
                        proving_args,
                        proof_file_name,
                    })
                    .await
                    .context("task channel closed")?;
            }
        }
    }
}
