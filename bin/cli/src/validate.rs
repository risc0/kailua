// Copyright 2024 RISC Zero, Inc.
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

use crate::channel::DuplexChannel;
use crate::db::config::Config;
use crate::db::proposal::Proposal;
use crate::db::KailuaDB;
use crate::providers::beacon::BlobProvider;
use crate::providers::optimism::OpNodeProvider;
use crate::{stall::Stall, CoreArgs, KAILUA_GAME_TYPE, SET_BUILDER_ID};
use alloy::eips::eip4844::{IndexedBlobHash, FIELD_ELEMENTS_PER_BLOB};
use alloy::eips::BlockNumberOrTag;
use alloy::network::primitives::BlockTransactionsKind;
use alloy::network::EthereumWallet;
use alloy::primitives::{Address, Bytes, FixedBytes, B256, U256};
use alloy::providers::{Provider, ProviderBuilder, ReqwestProvider};
use alloy::signers::local::LocalSigner;
use anyhow::{anyhow, bail, Context};
use kailua_client::boundless::BoundlessArgs;
use kailua_client::parse_address;
use kailua_client::proof::{fpvm_proof_file_name, Proof};
use kailua_common::blobs::hash_to_fe;
use kailua_common::blobs::BlobFetchRequest;
use kailua_common::client::config_hash;
use kailua_common::journal::ProofJournal;
use kailua_common::precondition::{
    divergence_precondition_hash, equivalence_precondition_hash, PreconditionValidationData,
};
use kailua_contracts::*;
use kailua_host::fetch_rollup_config;
use op_alloy_protocol::BlockInfo;
use risc0_zkvm::is_dev_mode;
use std::path::{Path, PathBuf};
use std::process::exit;
use std::str::FromStr;
use std::time::Duration;
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
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

    /// Secret key of L1 wallet to use for challenging and proving outputs
    #[clap(long, env)]
    pub validator_key: String,
    #[clap(long, value_parser = parse_address, env)]
    pub payout_recipient_address: Option<Address>,

    #[clap(flatten)]
    pub boundless: BoundlessArgs,
}

pub async fn validate(args: ValidateArgs, data_dir: PathBuf) -> anyhow::Result<()> {
    // We run two concurrent tasks, one for the chain, and one for the prover.
    // Both tasks communicate using the duplex channel
    let channel_pair = DuplexChannel::new_pair(4096);

    let handle_proposals = spawn(handle_proposals(
        channel_pair.0,
        args.clone(),
        data_dir.clone(),
    ));
    let handle_proof_requests = spawn(handle_proof_requests(channel_pair.1, args, data_dir));

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
    Proof(u64, Proof),
}

pub async fn handle_proposals(
    mut channel: DuplexChannel<Message>,
    args: ValidateArgs,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    // initialize blockchain connections
    info!("Initializing rpc connections.");
    let op_node_provider =
        OpNodeProvider(ProviderBuilder::new().on_http(args.core.op_node_url.as_str().try_into()?));
    let eth_rpc_provider =
        ProviderBuilder::new().on_http(args.core.eth_rpc_url.as_str().try_into()?);
    let op_geth_provider =
        ProviderBuilder::new().on_http(args.core.op_geth_url.as_str().try_into()?);
    let cl_node_provider = BlobProvider::new(args.core.beacon_rpc_url.as_str()).await?;

    info!("Fetching rollup configuration from rpc endpoints.");
    // fetch rollup config
    let config = fetch_rollup_config(&args.core.op_node_url, &args.core.op_geth_url, None)
        .await
        .context("fetch_rollup_config")?;
    let rollup_config_hash = config_hash(&config).expect("Configuration hash derivation error");
    info!("RollupConfigHash({})", hex::encode(rollup_config_hash));

    // load system config
    let system_config = SystemConfig::new(config.l1_system_config_address, &eth_rpc_provider);
    let dgf_address = system_config.disputeGameFactory().stall().await.addr_;

    // initialize validator wallet
    info!("Initializing validator wallet.");
    let validator_signer = LocalSigner::from_str(&args.validator_key)?;
    let validator_address = validator_signer.address();
    let validator_wallet = EthereumWallet::from(validator_signer);
    let validator_provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(validator_wallet)
        .on_http(args.core.eth_rpc_url.as_str().try_into()?);
    info!("Validator address: {validator_address}");

    // Init factory contract
    let dispute_game_factory = IDisputeGameFactory::new(dgf_address, &validator_provider);
    info!("DisputeGameFactory({:?})", dispute_game_factory.address());
    let game_count: u64 = dispute_game_factory
        .gameCount()
        .stall()
        .await
        .gameCount_
        .to();
    info!("There have been {game_count} games created using DisputeGameFactory");
    let kailua_game_implementation = KailuaGame::new(
        dispute_game_factory
            .gameImpls(KAILUA_GAME_TYPE)
            .stall()
            .await
            .impl_,
        &validator_provider,
    );
    info!("KailuaGame({:?})", kailua_game_implementation.address());
    if kailua_game_implementation.address().is_zero() {
        error!("Fault proof game is not installed!");
        exit(1);
    }
    // Initialize empty DB
    info!("Initializing..");
    let mut kailua_db = KailuaDB::init(data_dir, &dispute_game_factory).await?;
    info!("KailuaTreasury({:?})", kailua_db.treasury.address);
    // Run the validator loop
    info!(
        "Starting from proposal at factory index {}",
        kailua_db.state.next_factory_index
    );
    loop {
        // Wait for new data on every iteration
        sleep(Duration::from_secs(1)).await;
        // fetch latest games
        let loaded_proposals = kailua_db
            .load_proposals(&dispute_game_factory, &op_node_provider, &cl_node_provider)
            .await
            .context("load_proposals")?;

        // check new proposals for fault and queue potential responses
        for opponent_index in loaded_proposals {
            let Some(opponent) = kailua_db.get_local_proposal(&opponent_index) else {
                error!("Proposal {opponent_index} missing from database.");
                continue;
            };
            // Look up parent proposal
            let Some(proposal_parent) = kailua_db.get_local_proposal(&opponent.parent) else {
                error!(
                    "Proposal {} parent {} missing from database.",
                    opponent.index, opponent.parent
                );
                continue;
            };
            let proposal_parent_contract =
                proposal_parent.tournament_contract_instance(&validator_provider);
            // Check that a validity proof has not already been posted
            if proposal_parent_contract
                .proofStatus(U256::ZERO, U256::ZERO)
                .stall()
                .await
                ._0
                != 0
            {
                info!(
                    "Validity proof settling all disputes in tournament {} already submitted",
                    proposal_parent.index
                );
                continue;
            }
            // fetch canonical status of proposal
            let Some(is_opponent_canonical) = opponent.canonical else {
                error!("Canonical status of proposal {opponent_index} unknown");
                continue;
            };
            // prove the validity of this proposal if it is canon game of height below the ff-target
            if opponent.has_parent()
                && is_opponent_canonical
                && opponent.output_block_number <= args.fast_forward_target
            {
                // Prove full validity
                request_validity_proof(
                    &mut channel,
                    &kailua_db.config,
                    &proposal_parent,
                    &opponent,
                    &eth_rpc_provider,
                    &op_geth_provider,
                )
                .await?;
                continue;
            }
            // skip fault proving this proposal if it has no contender
            let Some(contender_index) = opponent.contender else {
                info!("Skipping proposal {opponent_index} with no contender.");
                continue;
            };
            // fetch contender
            let Some(contender) = kailua_db.get_local_proposal(&contender_index) else {
                error!("Contender {contender_index} missing from database.");
                continue;
            };
            // fetch canonical status of contender
            let Some(is_contender_canonical) = contender.canonical else {
                error!("Contender {contender_index} canonical status unknown.");
                continue;
            };
            // skip this proposal if it has a canonical contender being fast-forwarded
            if is_contender_canonical && opponent.output_block_number <= args.fast_forward_target {
                info!(
                    "Skipping fault proving for proposal {opponent_index} assuming ongoing \
                    validity proof generation for {contender_index}"
                );
                continue;
            };
            // Look up indices of children in parent
            let Some(u_index) = proposal_parent.child_index(contender.index) else {
                error!(
                    "Could not look up contender {} index in parent tournament {}",
                    contender.index, proposal_parent.index
                );
                continue;
            };
            let Some(v_index) = proposal_parent.child_index(opponent.index) else {
                error!(
                    "Could not look up proposal {} index in parent tournament {}",
                    opponent.index, proposal_parent.index
                );
                continue;
            };
            // Check that a fault proof had not already been posted
            let proof_status = proposal_parent_contract
                .proofStatus(U256::from(u_index), U256::from(v_index))
                .stall()
                .await
                ._0;
            if proof_status != 0 {
                info!(
                    "Match between children {u_index} and {v_index} already proven {proof_status}"
                );
                continue;
            }
            // Prove fault
            request_fault_proof(
                &mut channel,
                &kailua_db.config,
                &proposal_parent,
                &contender,
                &opponent,
                &eth_rpc_provider,
                &op_geth_provider,
                &op_node_provider,
            )
            .await?;
        }

        // publish computed fault proofs and resolve proven challenges
        while !channel.receiver.is_empty() {
            let Message::Proof(proposal_index, proof) = channel
                .receiver
                .recv()
                .await
                .ok_or(anyhow!("proposals receiver channel closed"))?
            else {
                bail!("Unexpected message type.");
            };
            let opponent = kailua_db.get_local_proposal(&proposal_index).unwrap();
            let parent = kailua_db.get_local_proposal(&opponent.parent).unwrap();
            // Abort early if a validity proof is already submitted in this tournament
            if parent
                .fetch_is_successor_validity_proven(&validator_provider)
                .await?
            {
                info!(
                    "Skipping proof submission in tournament {} with validity proof.",
                    parent.index
                );
                continue;
            }
            let parent_contract = parent.tournament_contract_instance(&validator_provider);
            let expected_fpvm_image_id = parent_contract.imageId().stall().await.imageId_.0;
            // patch the proof if in dev mode
            #[cfg(feature = "devnet")]
            let proof = maybe_patch_proof(proof, expected_fpvm_image_id, SET_BUILDER_ID.0)?;
            // verify that the zkvm receipt is valid
            if let Some(receipt) = proof.as_receipt() {
                if let Err(e) = receipt.verify(expected_fpvm_image_id) {
                    error!("Could not verify receipt against image id in contract: {e:?}");
                } else {
                    info!("Receipt validated.");
                }
            }
            // Decode ProofJournal
            let proof_journal = ProofJournal::decode_packed(proof.journal().as_ref())?;
            info!("Proof journal: {:?}", proof_journal);
            // encode seal data
            let encoded_seal = Bytes::from(proof.encoded_seal()?);

            let opponent_contract = opponent.tournament_contract_instance(&validator_provider);
            // Check if proof is a viable validity proof
            if proof_journal.l1_head == opponent.l1_head
                && proof_journal.agreed_l2_output_root == parent.output_root
                && proof_journal.claimed_l2_output_root == opponent.output_root
            {
                let v_index = parent
                    .child_index(opponent.index)
                    .expect("Could not look up contender's index in parent tournament");
                info!(
                    "Submitting validity proof to tournament at index {} for child at index {v_index}.",
                    parent.index,
                );

                // sanity check proof journal fields
                {
                    let contract_blobs_hash =
                        opponent_contract.blobsHash().stall().await.blobsHash_;
                    if opponent.blobs_hash() != contract_blobs_hash {
                        warn!(
                            "Local proposal blobs hash {} doesn't match contract blobs hash {}",
                            opponent.blobs_hash(),
                            contract_blobs_hash
                        )
                    } else {
                        info!("Blobs hash {} confirmed", contract_blobs_hash);
                    }
                    let precondition_hash = equivalence_precondition_hash(
                        &parent.output_block_number,
                        &kailua_db.config.proposal_output_count,
                        &kailua_db.config.output_block_span,
                        contract_blobs_hash,
                    );
                    if proof_journal.precondition_output != precondition_hash {
                        warn!(
                            "Proof precondition hash {} does not match expected value {}",
                            proof_journal.precondition_output, precondition_hash
                        );
                    } else {
                        info!("Precondition hash {precondition_hash} confirmed.")
                    }
                    let config_hash = opponent_contract.configHash().stall().await.configHash_;
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
                        v_index,
                        encoded_seal.clone(),
                    )
                    .send()
                    .await
                    .context("proveValidity (send)")
                {
                    Ok(txn) => match txn
                        .get_receipt()
                        .await
                        .context("proveValidity (get_receipt)")
                    {
                        Ok(receipt) => {
                            info!("Validity proof submitted: {receipt:?}");
                            let proof_status = parent_contract
                                .proofStatus(U256::ZERO, U256::ZERO)
                                .stall()
                                .await
                                ._0;
                            info!("Validity proven: {proof_status}");
                        }
                        Err(e) => {
                            error!("Failed to confirm validity proof txn: {e:?}");
                        }
                    },
                    Err(e) => {
                        error!("Failed to send validity proof txn: {e:?}");
                    }
                }
                // Skip fault proof submission logic
                continue;
            }

            // Disputing children indices
            let contender_index = opponent.contender.unwrap();
            let contender = kailua_db.get_local_proposal(&contender_index).unwrap();
            let u_index = parent
                .child_index(contender_index)
                .expect("Could not look up contender's index in parent tournament");
            let v_index = parent
                .child_index(opponent.index)
                .expect("Could not look up contender's index in parent tournament");

            // The index of the intermediate output to challenge
            // ZERO for trail data challenges
            let divergence_point = contender
                .divergence_point(&opponent)
                .expect("Equivalent proposals") as u64;

            // Proofs of faulty trail data do not derive outputs beyond the parent proposal claim
            let is_output_fault_proof =
                proof_journal.claimed_l2_block_number > parent.output_block_number;
            let contender_output_fe = contender.output_fe_at(divergence_point);
            let opponent_output_fe = opponent.output_fe_at(divergence_point);

            // Sanity check proof data
            {
                if is_output_fault_proof {
                    let proof_output_root_fe = hash_to_fe(proof_journal.claimed_l2_output_root);
                    if proof_output_root_fe != contender_output_fe {
                        warn!(
                            "Contender output fe {contender_output_fe} doesn't match proof fe {proof_output_root_fe}",
                        );
                    }
                    if proof_output_root_fe != opponent_output_fe {
                        warn!(
                            "Proposal output fe {opponent_output_fe} doesn't match proof fe {proof_output_root_fe}",
                        );
                    }
                    let op_node_output = op_node_provider
                        .output_at_block(proof_journal.claimed_l2_block_number)
                        .await?;
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

                    if proof_journal.l1_head != opponent.l1_head {
                        warn!(
                            "L1 head mismatch. Found {}, expected {}.",
                            proof_journal.l1_head, opponent.l1_head
                        );
                    } else {
                        info!("Proof L1 head {} confirmed.", opponent.l1_head);
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
                } else {
                    if !contender_output_fe.is_zero() {
                        warn!("Contender has non-zero output {contender_output_fe} in trail data.");
                    } else {
                        info!("Contender trail output is zero.")
                    }
                    if !opponent_output_fe.is_zero() {
                        warn!("Opponent has non-zero output {opponent_output_fe} in trail data.");
                    } else {
                        info!("Opponent trail output is zero.")
                    }
                    if proof_journal.claimed_l2_output_root != parent.output_root {
                        warn!(
                            "Proof output root {} doesn't equal parent claim {}.",
                            proof_journal.claimed_l2_output_root, parent.output_root
                        );
                    } else {
                        info!("Proof claimed l2 output is same as parent claim.")
                    }
                    if proof_journal.l1_head != parent.l1_head {
                        warn!(
                            "Proof l1 head {} doesn't match parent's l1 head {}",
                            proof_journal.l1_head, parent.l1_head
                        )
                    } else {
                        info!("Proof L1 head confirmed.");
                    }
                    if proof_journal.claimed_l2_block_number != parent.output_block_number {
                        warn!(
                            "Proof L2 block number {} doesnt equal that of parent {}.",
                            proof_journal.claimed_l2_block_number, parent.output_block_number
                        )
                    }
                }
            }

            // Skip proof submission if already proven
            let fault_proof_status = parent_contract
                .proofStatus(U256::from(u_index), U256::from(v_index))
                .stall()
                .await
                ._0;
            if fault_proof_status != 0 {
                warn!("Skipping proof submission for already proven game at local index {proposal_index}.");
                continue;
            } else {
                info!("Fault proof status: {fault_proof_status}");
            }

            // create kzg proofs
            let mut proofs = [vec![], vec![]];
            let mut commitments = [vec![], vec![]];

            // kzg proofs for agreed output hashes (or for conflicting trail data)
            if !is_output_fault_proof || divergence_point > 0 {
                commitments[0].push(contender.io_commitment_for(divergence_point - 1));
                proofs[0].push(contender.io_proof_for(divergence_point - 1)?);

                commitments[1].push(opponent.io_commitment_for(divergence_point - 1));
                proofs[1].push(opponent.io_proof_for(divergence_point - 1)?);
            }

            // kzg proofs for claimed output hashes (no trail data)
            if is_output_fault_proof
                && proof_journal.claimed_l2_block_number != opponent.output_block_number
            {
                commitments[0].push(contender.io_commitment_for(divergence_point));
                proofs[0].push(contender.io_proof_for(divergence_point)?);

                commitments[1].push(opponent.io_commitment_for(divergence_point));
                proofs[1].push(opponent.io_proof_for(divergence_point)?);
            }

            // sanity check kzg proofs
            {
                let contender_contract =
                    contender.tournament_contract_instance(&validator_provider);

                if is_output_fault_proof {
                    if proof_journal.claimed_l2_block_number == opponent.output_block_number {
                        if hash_to_fe(contender.output_root) != contender_output_fe {
                            warn!(
                                "Contender proposed output root fe {} does not match submitted {}",
                                hash_to_fe(contender.output_root),
                                contender_output_fe
                            );
                        } else {
                            info!("Contender proposed output confirmed.");
                        }
                        if hash_to_fe(opponent.output_root) != opponent_output_fe {
                            warn!(
                                "Proposal proposed output root {} does not match submitted {}",
                                hash_to_fe(opponent.output_root),
                                opponent_output_fe
                            );
                        } else {
                            info!("Proposal proposed output confirmed.");
                        }
                    } else {
                        let contender_has_output = contender_contract
                            .verifyIntermediateOutput(
                                divergence_point,
                                contender_output_fe,
                                commitments[0].last().unwrap().clone(),
                                proofs[0].last().unwrap().clone(),
                            )
                            .stall()
                            .await
                            .success;
                        if !contender_has_output {
                            warn!("Could not verify proposed output for contender");
                        } else {
                            info!("Contender proposed output confirmed.");
                        }
                        let opponent_has_output = opponent_contract
                            .verifyIntermediateOutput(
                                divergence_point,
                                opponent_output_fe,
                                commitments[1].last().unwrap().clone(),
                                proofs[1].last().unwrap().clone(),
                            )
                            .stall()
                            .await
                            .success;
                        if !opponent_has_output {
                            warn!("Could not verify proposed output for proposal");
                        } else {
                            info!("Proposal proposed output confirmed.");
                        }
                    }

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
                        let agreed_l2_output_root_fe =
                            hash_to_fe(proof_journal.agreed_l2_output_root);
                        let contender_has_output = contender_contract
                            .verifyIntermediateOutput(
                                divergence_point - 1,
                                agreed_l2_output_root_fe,
                                commitments[0].first().unwrap().clone(),
                                proofs[0].first().unwrap().clone(),
                            )
                            .stall()
                            .await
                            .success;
                        if !contender_has_output {
                            warn!("Could not verify last common output for contender");
                        } else {
                            info!("Contender common output publication confirmed.");
                        }
                        let proposal_has_output = opponent_contract
                            .verifyIntermediateOutput(
                                divergence_point - 1,
                                agreed_l2_output_root_fe,
                                commitments[1].first().unwrap().clone(),
                                proofs[1].first().unwrap().clone(),
                            )
                            .stall()
                            .await
                            .success;
                        if !proposal_has_output {
                            warn!("Could not verify last common output for proposal");
                        } else {
                            info!("Proposal common output publication confirmed.");
                        }
                        contender_has_output && proposal_has_output
                    };
                    if is_agreed_output_confirmed {
                        info!(
                            "Confirmed last common output: {}",
                            proof_journal.agreed_l2_output_root
                        );
                    }
                } else {
                    let divergent_trail_point = divergence_point - 1;
                    if !contender_contract
                        .verifyIntermediateOutput(
                            divergent_trail_point,
                            contender_output_fe,
                            commitments[0].first().unwrap().clone(),
                            proofs[0].first().unwrap().clone(),
                        )
                        .stall()
                        .await
                        .success
                    {
                        warn!("Could not verify divergent trail output for contender");
                    } else {
                        info!("Contender divergent trail output confirmed.");
                    }
                    if !opponent_contract
                        .verifyIntermediateOutput(
                            divergent_trail_point,
                            opponent_output_fe,
                            commitments[1].first().unwrap().clone(),
                            proofs[1].first().unwrap().clone(),
                        )
                        .stall()
                        .await
                        .success
                    {
                        warn!("Could not verify divergent trail output for proposal");
                    } else {
                        info!("Proposal divergent trail output confirmed.");
                    }
                }
            }

            // sanity check precondition hash
            {
                let expected_precondition_hash = if !opponent.has_precondition_for(divergence_point)
                {
                    B256::ZERO
                } else {
                    // Normalize the conflicting blob fe index
                    let normalized_position = divergence_point - !is_output_fault_proof as u64;
                    divergence_precondition_hash(
                        &(normalized_position % FIELD_ELEMENTS_PER_BLOB),
                        &contender.io_blob_for(normalized_position).0,
                        &opponent.io_blob_for(normalized_position).0,
                    )
                };

                if proof_journal.precondition_output != expected_precondition_hash {
                    warn!(
                        "Possible precondition hash mismatch. Found {}, computed {}",
                        expected_precondition_hash, proof_journal.precondition_output
                    );
                } else {
                    info!("Proof Precondition hash {expected_precondition_hash} confirmed.")
                }
            }

            // sanity check config hash
            {
                let config_hash = parent_contract.configHash().stall().await.configHash_;
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
                "Submitting fault proof to tournament at index {} for match between children \
                {u_index} and {v_index} over challenge position {divergence_point} with {} kzg \
                proof(s).",
                parent.index,
                proofs[0].len() + proofs[1].len()
            );

            let transaction_dispatch = if is_output_fault_proof {
                parent_contract
                    .proveOutputFault(
                        proof_journal.payout_recipient,
                        [u_index, v_index, divergence_point],
                        encoded_seal.clone(),
                        proof_journal.agreed_l2_output_root,
                        [contender_output_fe, opponent_output_fe],
                        proof_journal.claimed_l2_output_root,
                        commitments,
                        proofs,
                    )
                    .send()
                    .await
                    .context("proveOutputFault (send)")
            } else {
                parent_contract
                    .proveTrailFault(
                        proof_journal.payout_recipient,
                        [u_index, v_index, divergence_point],
                        encoded_seal.clone(),
                        [contender_output_fe, opponent_output_fe],
                        commitments,
                        proofs,
                    )
                    .send()
                    .await
                    .context("proveTrailFault (send)")
            };

            match transaction_dispatch {
                Ok(txn) => match txn.get_receipt().await.context("prove (get_receipt)") {
                    Ok(receipt) => {
                        info!("Fault proof submitted: {receipt:?}");
                        let proof_status = parent_contract
                            .proofStatus(U256::from(u_index), U256::from(v_index))
                            .stall()
                            .await
                            ._0;
                        info!(
                            "Match between {contender_index} and {} proven: {proof_status}",
                            opponent.index
                        );
                    }
                    Err(e) => {
                        error!("Failed to confirm fault proof txn: {e:?}");
                    }
                },
                Err(e) => {
                    error!("Failed to send fault proof txn: {e:?}");
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn request_fault_proof(
    channel: &mut DuplexChannel<Message>,
    config: &Config,
    parent: &Proposal,
    contender: &Proposal,
    opponent: &Proposal,
    l1_node_provider: &ReqwestProvider,
    l2_node_provider: &ReqwestProvider,
    op_node_provider: &OpNodeProvider,
) -> anyhow::Result<()> {
    let Some(divergence_point) = contender.divergence_point(opponent) else {
        error!(
            "Contender {} does not diverge from opponent {}.",
            contender.index, opponent.index
        );
        return Ok(());
    };
    let divergence_point = divergence_point as u64;

    // Read additional data for Kona invocation
    info!(
        "Requesting proof to settle dispute between {} and {} at point {divergence_point}.",
        contender.index, opponent.index
    );
    let io_count = opponent.io_field_elements.len() as u64;
    let is_output_fault = divergence_point <= io_count;

    // Set L2 Head Number
    let agreed_l2_head_number = if is_output_fault {
        // output commitment challenges start from the last common transition
        parent.output_block_number + config.output_block_span * divergence_point
    // jump to zero-indexed challenge point
    } else {
        // trail data challenges assume the starting output
        parent.output_block_number
    };
    debug!("l2_head_number {:?}", &agreed_l2_head_number);

    // Get L2 head hash
    let agreed_l2_head_hash = l2_node_provider
        .get_block_by_number(
            BlockNumberOrTag::Number(agreed_l2_head_number),
            BlockTransactionsKind::Hashes,
        )
        .await
        .context("agreed_l2_head_hash")?
        .expect("Agreed l2 head not found")
        .header
        .hash;
    debug!("l2_head {:?}", &agreed_l2_head_hash);

    // Get L2 head output root
    let agreed_l2_output_root = op_node_provider
        .output_at_block(agreed_l2_head_number)
        .await
        .context("output_at_block")?;

    // Prepare expected output commitment
    let claimed_l2_block_number = if is_output_fault {
        // output commitment challenges target the first bad transition
        agreed_l2_head_number + config.output_block_span
    } else {
        // trail data challenges do not derive any l2 blocks
        agreed_l2_head_number
    };
    let claimed_l2_output_root = op_node_provider
        .output_at_block(claimed_l2_block_number)
        .await
        .context("output_at_block")?;

    // Prepare precondition validation data
    let precondition_validation_data = if opponent.has_precondition_for(divergence_point) {
        // Normalize the challenge_position as the blob field element index
        let normalized_position = divergence_point - !is_output_fault as u64;

        let (u_blob_hash, u_blob) = contender.io_blob_for(normalized_position);
        let u_blob_block_parent = l1_node_provider
            .get_block_by_hash(contender.l1_head, BlockTransactionsKind::Hashes)
            .await
            .context("u_blob_block_parent get_block_by_hash")?
            .expect("u_blob_block_parent not found");
        let u_blob_block = l1_node_provider
            .get_block_by_number(
                BlockNumberOrTag::Number(u_blob_block_parent.header.number + 1),
                BlockTransactionsKind::Hashes,
            )
            .await
            .context("u_blob_block get_block_by_number")?
            .expect("u_blob_block not found");

        let (v_blob_hash, v_blob) = opponent.io_blob_for(normalized_position);
        let v_blob_block_parent = l1_node_provider
            .get_block_by_hash(opponent.l1_head, BlockTransactionsKind::Hashes)
            .await
            .context("v_blob_block_parent get_block_by_hash")?
            .expect("v_blob_block_parent not found");
        let v_blob_block = l1_node_provider
            .get_block_by_number(
                BlockNumberOrTag::Number(v_blob_block_parent.header.number + 1),
                BlockTransactionsKind::Hashes,
            )
            .await
            .context("v_blob_block get_block_by_number")?
            .expect("v_blob_block not found");

        info!(
            "Fetched blobs {}:{u_blob_hash} and {}:{v_blob_hash} for challenge point {normalized_position}/{is_output_fault}",
            u_blob.index,
            v_blob.index,
        );

        let validated_blobs = [
            // u's blob (contender)
            BlobFetchRequest {
                block_ref: BlockInfo {
                    hash: u_blob_block.header.hash,
                    number: u_blob_block.header.number,
                    parent_hash: u_blob_block.header.parent_hash,
                    timestamp: u_blob_block.header.timestamp,
                },
                blob_hash: IndexedBlobHash {
                    index: u_blob.index,
                    hash: u_blob_hash,
                },
            },
            // v's blob (proposal)
            BlobFetchRequest {
                block_ref: BlockInfo {
                    hash: v_blob_block.header.hash,
                    number: v_blob_block.header.number,
                    parent_hash: v_blob_block.header.parent_hash,
                    timestamp: v_blob_block.header.timestamp,
                },
                blob_hash: IndexedBlobHash {
                    index: v_blob.index,
                    hash: v_blob_hash,
                },
            },
        ];

        Some(PreconditionValidationData::Fault(
            normalized_position % FIELD_ELEMENTS_PER_BLOB,
            Box::new(validated_blobs),
        ))
    } else {
        None
    };

    // Set appropriate L1 head
    let l1_head = if is_output_fault {
        opponent.l1_head
    } else {
        parent.l1_head
    };

    // Message proving task
    channel
        .sender
        .send(Message::Proposal {
            index: opponent.index,
            precondition_validation_data,
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
    l1_node_provider: &ReqwestProvider,
    l2_node_provider: &ReqwestProvider,
) -> anyhow::Result<()> {
    let precondition_validation_data = if config.proposal_output_count > 1 {
        let mut validated_blobs = Vec::with_capacity(proposal.io_blobs.len());
        debug_assert!(!proposal.io_blobs.is_empty());
        for (blob_hash, blob) in &proposal.io_blobs {
            let block_parent = l1_node_provider
                .get_block_by_hash(proposal.l1_head, BlockTransactionsKind::Hashes)
                .await
                .context("block_parent get_block_by_hash")?
                .expect("block_parent not found");
            let block = l1_node_provider
                .get_block_by_number(
                    BlockNumberOrTag::Number(block_parent.header.number + 1),
                    BlockTransactionsKind::Hashes,
                )
                .await
                .context("block get_block_by_number")?
                .expect("block not found");
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
        Some(PreconditionValidationData::Validity(
            parent.output_block_number,
            config.proposal_output_count,
            config.output_block_span,
            validated_blobs,
        ))
    } else {
        None
    };
    // Get L2 head hash
    let agreed_l2_head_hash = l2_node_provider
        .get_block_by_number(
            BlockNumberOrTag::Number(parent.output_block_number),
            BlockTransactionsKind::Hashes,
        )
        .await
        .context("agreed_l2_head_hash")?
        .expect("Agreed l2 head not found")
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
    // Fetch rollup configuration
    let l2_chain_id = fetch_rollup_config(&args.core.op_node_url, &args.core.op_geth_url, None)
        .await?
        .l2_chain_id
        .to_string();
    // Set payout recipient
    let payout_recipient = args.payout_recipient_address.unwrap_or_else(|| {
        LocalSigner::from_str(&args.validator_key)
            .unwrap()
            .address()
    });
    info!("Proof payout recipient: {payout_recipient}");
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
        // Prepare kailua-host parameters
        let precondition_hash = precondition_validation_data
            .as_ref()
            .map(|d| d.precondition_hash())
            .unwrap_or_default();
        let proof_file_name = fpvm_proof_file_name(
            precondition_hash,
            l1_head,
            claimed_l2_output_root,
            claimed_l2_block_number,
            agreed_l2_output_root,
        );
        let payout_recipient = payout_recipient.to_string();
        let l1_head = l1_head.to_string();
        let agreed_l2_head_hash = agreed_l2_head_hash.to_string();
        let agreed_l2_output_root = agreed_l2_output_root.to_string();
        let claimed_l2_output_root = claimed_l2_output_root.to_string();
        let claimed_l2_block_number = claimed_l2_block_number.to_string();
        let verbosity = [
            String::from("-"),
            (0..args.core.v).map(|_| 'v').collect::<String>(),
        ]
        .concat();
        let mut proving_args = vec![
            String::from("--payout-recipient-address"), // wallet address for payouts
            payout_recipient,
            String::from("--l1-head"), // l1 head from on-chain proposal
            l1_head,
            String::from("--agreed-l2-head-hash"), // l2 starting block hash from on-chain proposal
            agreed_l2_head_hash,
            String::from("--agreed-l2-output-root"), // l2 starting output root
            agreed_l2_output_root,
            String::from("--claimed-l2-output-root"), // proposed output root
            claimed_l2_output_root,
            String::from("--claimed-l2-block-number"), // proposed block number
            claimed_l2_block_number,
            String::from("--l2-chain-id"), // rollup chain id
            l2_chain_id.clone(),
            String::from("--l1-node-address"), // l1 el node
            args.core.eth_rpc_url.clone(),
            String::from("--l1-beacon-address"), // l1 cl node
            args.core.beacon_rpc_url.clone(),
            String::from("--l2-node-address"), // l2 el node
            args.core.op_geth_url.clone(),
            String::from("--op-node-address"), // l2 cl node
            args.core.op_node_url.clone(),
            String::from("--data-dir"), // path to cache
            data_dir.to_str().unwrap().to_string(),
            String::from("--native"), // run the client natively
        ];
        // precondition data
        if let Some(precondition_data) = precondition_validation_data {
            let (block_hashes, blob_hashes): (Vec<_>, Vec<_>) = precondition_data
                .blob_fetch_requests()
                .iter()
                .map(|r| (r.block_ref.hash.to_string(), r.blob_hash.hash.to_string()))
                .unzip();
            let params = match precondition_data {
                PreconditionValidationData::Fault(agreement_index, _) => vec![agreement_index],
                PreconditionValidationData::Validity(
                    global_l2_head_number,
                    proposal_output_count,
                    output_block_span,
                    _,
                ) => vec![
                    global_l2_head_number,
                    proposal_output_count,
                    output_block_span,
                ],
            }
            .into_iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>();

            proving_args.extend(vec![
                String::from("--precondition-params"),
                params.join(","),
                String::from("--precondition-block-hashes"),
                block_hashes.join(","),
                String::from("--precondition-blob-hashes"),
                blob_hashes.join(","),
            ]);
        }
        // boundless args
        if let Some(market) = &args.boundless.market {
            proving_args.extend(market.to_arg_vec(&args.boundless.storage));
        }
        // verbosity level
        if args.core.v > 0 {
            proving_args.push(verbosity);
        }
        // Prove via kailua-host (re dev mode/bonsai: env vars inherited!)
        let mut kailua_host_command = Command::new(&args.kailua_host);
        // get fake receipts when building under devnet
        if is_dev_mode() {
            kailua_host_command.env("RISC0_DEV_MODE", "1");
        }
        // pass arguments to point at target block
        kailua_host_command.args(proving_args);
        debug!("kailua_host_command {:?}", &kailua_host_command);
        {
            match kailua_host_command
                .kill_on_drop(true)
                .spawn()
                .context("Invoking kailua-host")?
                .wait()
                .await
            {
                Ok(proving_task) => {
                    if !proving_task.success() {
                        error!("Proving task failure.");
                    } else {
                        info!("Proving task successful.");
                    }
                }
                Err(e) => {
                    error!("Failed to invoke kailua-host: {e:?}");
                }
            }
        }
        sleep(Duration::from_secs(1)).await;
        // Read receipt file
        if !Path::new(&proof_file_name).exists() {
            error!("Proof file {proof_file_name} not found.");
        } else {
            info!("Found proof file.");
        }
        let mut proof_file = match File::open(proof_file_name.clone()).await {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to open proof file {proof_file_name}: {e:?}");
                continue;
            }
        };
        info!("Opened proof file {proof_file_name}.");
        let mut proof_data = Vec::new();
        if let Err(e) = proof_file.read_to_end(&mut proof_data).await {
            error!("Failed to read proof file {proof_file_name}: {e:?}");
            continue;
        }
        info!("Read entire proof file.");
        match bincode::deserialize::<Proof>(&proof_data) {
            Ok(proof) => {
                // Send proof via the channel
                channel
                    .sender
                    .send(Message::Proof(proposal_index, proof))
                    .await?;
                info!("Proof for local index {proposal_index} complete.");
            }
            Err(e) => {
                error!("Failed to deserialize proof: {e:?}");
            }
        }
    }
}

#[cfg(feature = "devnet")]
fn maybe_patch_proof(
    mut proof: Proof,
    expected_fpvm_image_id: [u8; 32],
    expected_set_builder_image_id: [u8; 32],
) -> anyhow::Result<Proof> {
    // Return the proof if we can't patch it
    if !is_dev_mode() {
        return Ok(proof);
    }

    use alloy::sol_types::SolValue;
    use risc0_zkvm::sha::Digestible;

    let expected_fpvm_image_id = risc0_zkvm::sha::Digest::from(expected_fpvm_image_id);

    match &mut proof {
        Proof::ZKVMReceipt(receipt) => {
            // Patch the image id of the receipt to match the expected one
            if let risc0_zkvm::InnerReceipt::Fake(fake_inner_receipt) = &mut receipt.inner {
                if let risc0_zkvm::MaybePruned::Value(claim) = &mut fake_inner_receipt.claim {
                    warn!("DEVNET-ONLY: Patching fake receipt image id to match game contract.");
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
        }
        Proof::BoundlessSeal(seal_data, journal) => {
            let expected_boundless_selector = kailua_client::boundless::set_verifier_selector(
                expected_set_builder_image_id.into(),
            );
            let expected_set_builder_image_id =
                risc0_zkvm::sha::Digest::from(expected_set_builder_image_id);
            // Just use the proof if everything is in order
            if &seal_data[..4] == expected_boundless_selector.as_slice() {
                return Ok(proof);
            }
            // Amend the seal with a fake proof for the set root
            match kailua_contracts::SetVerifierSeal::abi_decode(&seal_data[4..], true) {
                Ok(mut seal) => {
                    if seal.rootSeal.is_empty() {
                        // build the claim for the fpvm
                        let fpvm_claim_digest = risc0_zkvm::ReceiptClaim::ok(
                            expected_fpvm_image_id,
                            journal.bytes.clone(),
                        )
                        .digest();
                        // convert the merkle path into Digest instances
                        let set_builder_siblings: Vec<_> = seal
                            .path
                            .iter()
                            .map(|n| risc0_zkvm::sha::Digest::from(n.0))
                            .collect();
                        // construct set builder root from merkle proof
                        let set_builder_journal =
                            kailua_client::boundless::encoded_set_builder_journal(
                                &fpvm_claim_digest,
                                set_builder_siblings,
                                expected_set_builder_image_id,
                            );
                        // create fake proof for the root
                        let set_builder_seal =
                            risc0_ethereum_contracts::encode_seal(&risc0_zkvm::Receipt::new(
                                risc0_zkvm::InnerReceipt::Fake(risc0_zkvm::FakeReceipt::new(
                                    risc0_zkvm::ReceiptClaim::ok(
                                        expected_set_builder_image_id,
                                        set_builder_journal.clone(),
                                    ),
                                )),
                                set_builder_journal.clone(),
                            ))
                            .context("encode_seal (fake boundless)")?;
                        // replace empty root seal with constructed fake proof
                        seal.rootSeal = set_builder_seal.into();
                        // amend proof
                        warn!("DEVNET-ONLY: Patching proof with faux set verifier seal.");
                        *seal_data = [
                            expected_boundless_selector.as_slice(),
                            seal.abi_encode().as_slice(),
                        ]
                        .concat();
                    }
                }
                Err(e) => {
                    error!("Could not abi decode seal from boundless: {e:?}")
                }
            }
        }
    }
    Ok(proof)
}
