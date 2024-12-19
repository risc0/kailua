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
use crate::db::proposal::Proposal;
use crate::db::KailuaDB;
use crate::providers::beacon::BlobProvider;
use crate::providers::optimism::OpNodeProvider;
use crate::{stall::Stall, CoreArgs, KAILUA_GAME_TYPE};
use alloy::eips::eip4844::IndexedBlobHash;
use alloy::eips::BlockNumberOrTag;
use alloy::network::primitives::BlockTransactionsKind;
use alloy::network::EthereumWallet;
use alloy::primitives::{Bytes, FixedBytes, U256};
use alloy::providers::{Provider, ProviderBuilder, ReqwestProvider};
use alloy::signers::local::LocalSigner;
use anyhow::{anyhow, bail, Context};
use boundless_market::storage::StorageProviderConfig;
use kailua_client::proof::{fpvm_proof_file_name, Proof};
use kailua_client::BoundlessArgs;
use kailua_common::blobs::hash_to_fe;
use kailua_common::blobs::BlobFetchRequest;
use kailua_common::client::config_hash;
use kailua_common::journal::ProofJournal;
use kailua_common::precondition::{precondition_hash, PreconditionValidationData};
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

    /// Secret key of L1 wallet to use for challenging and proving outputs
    #[clap(long, env)]
    pub validator_key: String,

    #[clap(flatten)]
    pub boundless_args: Option<BoundlessArgs>,
    /// Storage provider to use for elf and input
    #[clap(flatten)]
    pub boundless_storage_config: Option<StorageProviderConfig>,
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
    let handle_proofs = spawn(handle_proofs(channel_pair.1, args, data_dir));

    let (proposals_task, proofs_task) = try_join!(handle_proposals, handle_proofs)?;
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
        for proposal_index in loaded_proposals {
            let Some(proposal) = kailua_db.get_local_proposal(&proposal_index) else {
                error!("Proposal {proposal_index} missing from database.");
                continue;
            };
            // skip this proposal if it has no contender
            let Some(contender) = proposal.contender else {
                continue;
            };
            // request a proof of the match results
            let Some(contender) = kailua_db.get_local_proposal(&contender) else {
                error!("Contender {contender} missing from database.");
                continue;
            };
            // Look up parent proposal
            let Some(proposal_parent) = kailua_db.get_local_proposal(&proposal.parent) else {
                error!(
                    "Proposal {} parent {} missing from database.",
                    proposal.index, proposal.parent
                );
                continue;
            };
            let proposal_parent_contract =
                proposal_parent.tournament_contract_instance(&validator_provider);
            // Look up indices of children in parent
            let Some(u_index) = proposal_parent.child_index(contender.index) else {
                error!(
                    "Could not look up contender {} index in parent tournament {}",
                    contender.index, proposal_parent.index
                );
                continue;
            };
            let Some(v_index) = proposal_parent.child_index(proposal.index) else {
                error!(
                    "Could not look up proposal {} index in parent tournament {}",
                    proposal.index, proposal_parent.index
                );
                continue;
            };
            // Check that proof had not already been posted
            let proof_status = proposal_parent_contract
                .proofStatus(U256::from(u_index), U256::from(v_index))
                .stall()
                .await
                ._0;
            // Prove if unproven
            if proof_status == 0 {
                request_proof(
                    &mut channel,
                    &contender,
                    &proposal,
                    &eth_rpc_provider,
                    &op_geth_provider,
                    &op_node_provider,
                )
                .await?;
            } else {
                info!(
                    "Match between children {u_index} and {v_index} already proven {proof_status}"
                );
            }
        }

        // publish computed proofs and resolve proven challenges
        while !channel.receiver.is_empty() {
            let Message::Proof(proposal_index, proof) = channel
                .receiver
                .recv()
                .await
                .ok_or(anyhow!("proposals receiver channel closed"))?
            else {
                bail!("Unexpected message type.");
            };
            let proposal = kailua_db.get_local_proposal(&proposal_index).unwrap();
            let proposal_parent = kailua_db.get_local_proposal(&proposal.parent).unwrap();
            let proposal_parent_contract =
                proposal_parent.tournament_contract_instance(&validator_provider);
            let proof_journal = ProofJournal::decode_packed(proof.journal().as_ref())?;
            info!("Proof journal: {:?}", proof_journal);
            let contender_index = proposal.contender.unwrap();
            let contender = kailua_db.get_local_proposal(&contender_index).unwrap();

            let u_index = proposal_parent
                .child_index(contender_index)
                .expect("Could not look up contender's index in parent tournament");
            let v_index = proposal_parent
                .child_index(proposal.index)
                .expect("Could not look up contender's index in parent tournament");

            let challenge_position =
                proof_journal.claimed_l2_block_number - proposal_parent.output_block_number - 1;

            let expected_image_id = proposal_parent_contract.imageId().stall().await.imageId_.0;

            // patch the proof if in dev mode
            #[cfg(feature = "devnet")]
            let proof = if is_dev_mode() || needs_selector_patch(&proof) {
                use alloy::sol_types::SolValue;
                use risc0_zkvm::sha::Digestible;

                let mut proof = proof;
                match &mut proof {
                    Proof::ZKVMReceipt(receipt) => {
                        // Patch the image id of the receipt to match the expected one
                        if let risc0_zkvm::InnerReceipt::Fake(fake_inner_receipt) =
                            &mut receipt.inner
                        {
                            if let risc0_zkvm::MaybePruned::Value(claim) =
                                &mut fake_inner_receipt.claim
                            {
                                warn!("DEVNET-ONLY: Patching fake receipt image id to match game contract.");
                                claim.pre =
                                    risc0_zkvm::MaybePruned::Pruned(expected_image_id.into());
                            }
                        }
                    }
                    Proof::BoundlessSeal(seal_data, journal) => {
                        // Amend the seal with a fake proof for the set root
                        match kailua_contracts::SetVerifierSeal::abi_decode(&seal_data[4..], true) {
                            Ok(mut seal) => {
                                if seal.rootSeal.is_empty() {
                                    // build the claim for the fpvm
                                    let fpvm_claim_digest = risc0_zkvm::ReceiptClaim::ok(
                                        risc0_zkvm::sha::Digest::from(kailua_build::KAILUA_FPVM_ID),
                                        journal.bytes.clone(),
                                    )
                                    .digest();
                                    // convert the merkle path into Digest instances
                                    let set_builder_siblings: Vec<_> = seal
                                        .path
                                        .iter()
                                        .map(|n| risc0_zkvm::sha::Digest::from(n.0))
                                        .collect();
                                    // derive the root
                                    let set_builder_root = risc0_aggregation::merkle_path_root(
                                        &fpvm_claim_digest,
                                        set_builder_siblings,
                                    );
                                    // construct set builder root from merkle proof
                                    let set_builder_journal = risc0_aggregation::GuestOutput::new(
                                        risc0_zkvm::sha::Digest::from(crate::SET_BUILDER_ID.0),
                                        set_builder_root,
                                    )
                                    .abi_encode();
                                    // create fake proof for the root
                                    let set_builder_seal = risc0_ethereum_contracts::encode_seal(
                                        &risc0_zkvm::Receipt::new(
                                            risc0_zkvm::InnerReceipt::Fake(
                                                risc0_zkvm::FakeReceipt::new(
                                                    risc0_zkvm::ReceiptClaim::ok(
                                                        risc0_zkvm::sha::Digest::from(
                                                            crate::SET_BUILDER_ID.0,
                                                        ),
                                                        set_builder_journal.clone(),
                                                    ),
                                                ),
                                            ),
                                            set_builder_journal.clone(),
                                        ),
                                    )
                                    .context("encode_seal (fake boundless)")?;
                                    // replace empty root seal with constructed fake proof
                                    seal.rootSeal = set_builder_seal.into();
                                    // amend proof
                                    warn!(
                                        "DEVNET-ONLY: Patching proof with faux set verifier seal."
                                    );
                                    let selector =
                                        kailua_client::set_verifier_selector(crate::SET_BUILDER_ID);
                                    *seal_data =
                                        [selector.as_slice(), seal.abi_encode().as_slice()]
                                            .concat();
                                }
                            }
                            Err(e) => {
                                error!("Could not abi decode seal from boundless: {e:?}")
                            }
                        }
                    }
                }
                proof
            } else {
                proof
            };

            // verify that the zkvm receipt is valid
            if let Some(receipt) = proof.as_receipt() {
                if let Err(e) = receipt.verify(expected_image_id) {
                    error!("Could not verify receipt against image id in contract: {e:?}");
                } else {
                    info!("Receipt validated.");
                }
            }

            let contender_output = contender.output_at(challenge_position);
            if contender_output != hash_to_fe(proof_journal.claimed_l2_output_root) {
                warn!(
                    "Contender output fe {contender_output} doesn't match proof fe {}",
                    hash_to_fe(proof_journal.claimed_l2_output_root)
                );
            }
            let proposal_output = proposal.output_at(challenge_position);
            if proposal_output != hash_to_fe(proof_journal.claimed_l2_output_root) {
                warn!(
                    "Proposal output fe {proposal_output} doesn't match proof fe {}",
                    hash_to_fe(proof_journal.claimed_l2_output_root)
                );
            }
            let op_node_output = op_node_provider
                .output_at_block(proof_journal.claimed_l2_block_number)
                .await?;
            if op_node_output != proof_journal.claimed_l2_output_root {
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

            // only prove unproven games
            let proof_status = proposal_parent_contract
                .proofStatus(U256::from(u_index), U256::from(v_index))
                .stall()
                .await
                ._0;
            if proof_status != 0 {
                warn!("Skipping proof submission for already proven game at local index {proposal_index}.");
                continue;
            } else {
                info!("Proof status: {proof_status}");
            }

            let encoded_seal = Bytes::from(proof.encoded_seal()?);

            // create kzg proofs
            let mut proofs = [vec![], vec![]];
            let mut commitments = [vec![], vec![]];

            // kzg proofs for agreed output hashes
            if challenge_position > 0 {
                commitments[0].push(contender.io_commitment_for(challenge_position - 1));
                proofs[0].push(contender.io_proof_for(challenge_position - 1)?);

                commitments[1].push(proposal.io_commitment_for(challenge_position - 1));
                proofs[1].push(proposal.io_proof_for(challenge_position - 1)?);
            }
            // kzg proofs for claimed output hashes
            if proof_journal.claimed_l2_block_number < proposal.output_block_number {
                commitments[0].push(contender.io_commitment_for(challenge_position));
                proofs[0].push(contender.io_proof_for(challenge_position)?);

                commitments[1].push(proposal.io_commitment_for(challenge_position));
                proofs[1].push(proposal.io_proof_for(challenge_position)?);
            }

            info!(
                "Submitting proof to tournament at index {} for match between children {u_index} and {v_index} over output {challenge_position} with {} kzg proof(s).",
                proposal_parent.index,
                proofs[0].len() + proofs[1].len()
            );

            let contender_contract = contender.tournament_contract_instance(&validator_provider);
            let proposal_contract = proposal.tournament_contract_instance(&validator_provider);

            if proof_journal.claimed_l2_block_number == proposal.output_block_number {
                if contender.output_root != contender.output_at(challenge_position) {
                    warn!(
                        "Contender proposed output root {} does not match submitted {}",
                        contender.output_root,
                        contender.output_at(challenge_position)
                    );
                } else {
                    info!("Contender proposed output confirmed.");
                }
                if proposal.output_root != proposal.output_at(challenge_position) {
                    warn!(
                        "Proposal proposed output root {} does not match submitted {}",
                        proposal.output_root,
                        proposal.output_at(challenge_position)
                    );
                } else {
                    info!("Proposal proposed output confirmed.");
                }
            } else {
                let contender_has_output = contender_contract
                    .verifyIntermediateOutput(
                        challenge_position,
                        contender.output_at(challenge_position),
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
                let proposal_has_output = proposal_contract
                    .verifyIntermediateOutput(
                        challenge_position,
                        proposal.output_at(challenge_position),
                        commitments[1].last().unwrap().clone(),
                        proofs[1].last().unwrap().clone(),
                    )
                    .stall()
                    .await
                    .success;
                if !proposal_has_output {
                    warn!("Could not verify proposed output for proposal");
                } else {
                    info!("Proposal proposed output confirmed.");
                }
            }

            let is_agreed_output_confirmed = if challenge_position == 0 {
                let parent_output_matches =
                    proposal_parent.output_root == proof_journal.agreed_l2_output_root;
                if !parent_output_matches {
                    warn!(
                        "Parent claim {} is last common output and does not match {}",
                        proposal_parent.output_root, proof_journal.agreed_l2_output_root
                    );
                }
                parent_output_matches
            } else {
                let contender_has_output = contender_contract
                    .verifyIntermediateOutput(
                        challenge_position - 1,
                        proof_journal.agreed_l2_output_root,
                        commitments[0].first().unwrap().clone(),
                        proofs[0].first().unwrap().clone(),
                    )
                    .stall()
                    .await
                    .success;
                if !contender_has_output {
                    warn!("Could not verify last common output for contender");
                } else {
                    info!("Contender common output confirmed.");
                }
                let proposal_has_output = proposal_contract
                    .verifyIntermediateOutput(
                        challenge_position - 1,
                        proof_journal.agreed_l2_output_root,
                        commitments[1].first().unwrap().clone(),
                        proofs[1].first().unwrap().clone(),
                    )
                    .stall()
                    .await
                    .success;
                if !proposal_has_output {
                    warn!("Could not verify last common output for proposal");
                } else {
                    info!("Proposal common output confirmed.");
                }
                contender_has_output && proposal_has_output
            };
            if is_agreed_output_confirmed {
                info!(
                    "Confirmed last common output: {}",
                    proof_journal.agreed_l2_output_root
                );
            }

            let possible_precondition_hash = precondition_hash(
                &contender.io_blob_for(challenge_position).0,
                &proposal.io_blob_for(challenge_position).0,
            );
            if proofs[0].len() == 2
                && possible_precondition_hash != proof_journal.precondition_output
            {
                warn!("Possible precondition hash mismatch. Found {}, computed {possible_precondition_hash}", proof_journal.precondition_output);
            } else {
                info!("Proof Precondition hash confirmed.")
            }

            let config_hash = proposal_parent_contract
                .configHash()
                .stall()
                .await
                .configHash_;
            if config_hash != proof_journal.config_hash {
                warn!(
                    "Config hash mismatch. Found {}, expected {config_hash}.",
                    proof_journal.config_hash
                );
            } else {
                info!("Proof Config hash confirmed.");
            }

            if proposal.l1_head != proof_journal.l1_head {
                warn!(
                    "L1 head mismatch. Found {}, expected {}.",
                    proof_journal.l1_head, proposal.l1_head
                );
            } else {
                info!("Proof L1 head confirmed.");
            }

            let expected_block_number =
                proposal_parent.output_block_number + challenge_position + 1;
            if expected_block_number != proof_journal.claimed_l2_block_number {
                warn!(
                    "Claimed l2 block number mismatch. Found {}, expected {expected_block_number}.",
                    proof_journal.claimed_l2_block_number
                );
            } else {
                info!("Claimed l2 block number confirmed.");
            }

            match proposal_parent_contract
                .prove(
                    [u_index, v_index, challenge_position],
                    encoded_seal.clone(),
                    proof_journal.agreed_l2_output_root,
                    [
                        contender.output_at(challenge_position),
                        proposal.output_at(challenge_position),
                    ],
                    proof_journal.claimed_l2_output_root,
                    commitments,
                    proofs,
                )
                .send()
                .await
                .context("prove (send)")
            {
                Ok(txn) => match txn.get_receipt().await.context("prove (get_receipt)") {
                    Ok(receipt) => {
                        info!("Proof submitted: {receipt:?}");
                        let proof_status = proposal_parent_contract
                            .proofStatus(U256::from(u_index), U256::from(v_index))
                            .stall()
                            .await
                            ._0;
                        info!(
                            "Match between {contender_index} and {} proven: {proof_status}",
                            proposal.index
                        );
                    }
                    Err(e) => {
                        error!("Failed to confirm proof txn: {e:?}");
                    }
                },
                Err(e) => {
                    error!("Failed to send proof txn: {e:?}");
                }
            }
        }
    }
}

async fn request_proof(
    channel: &mut DuplexChannel<Message>,
    contender: &Proposal,
    proposal: &Proposal,
    l1_node_provider: &ReqwestProvider,
    l2_node_provider: &ReqwestProvider,
    op_node_provider: &OpNodeProvider,
) -> anyhow::Result<()> {
    let challenge_point = contender
        .divergence_point(proposal)
        .expect("Contender does not diverge from proposal.") as u64;

    // Read additional data for Kona invocation
    info!("Requesting proof for proposal {}.", proposal.index);
    let agreed_l2_head_number =
        proposal.output_block_number - proposal.io_field_elements.len() as u64 - 1
            + challenge_point; // the challenge point is zero indexed, so it cancels out
    debug!("l2_head_number {:?}", &agreed_l2_head_number);
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
    let agreed_l2_output_root = op_node_provider
        .output_at_block(agreed_l2_head_number)
        .await
        .context("output_at_block")?;
    let claimed_l2_block_number = agreed_l2_head_number + 1;
    let claimed_l2_output_root = op_node_provider
        .output_at_block(claimed_l2_block_number)
        .await
        .context("output_at_block")?;

    // Prepare precondition validation data
    let precondition_validation_data = if proposal.has_precondition_for(challenge_point) {
        let (u_blob_hash, u_blob) = contender.io_blob_for(challenge_point);
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

        let (v_blob_hash, v_blob) = proposal.io_blob_for(challenge_point);
        let v_blob_block_parent = l1_node_provider
            .get_block_by_hash(proposal.l1_head, BlockTransactionsKind::Hashes)
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
            "Fetched blobs {}:{u_blob_hash} and {}:{v_blob_hash} for challenge point {challenge_point}",
            u_blob.index,
            v_blob.index,
        );

        Some(PreconditionValidationData {
            validated_blobs: [
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
            ],
        })
    } else {
        None
    };
    // Message proving task
    channel
        .sender
        .send(Message::Proposal {
            index: proposal.index,
            precondition_validation_data,
            l1_head: proposal.l1_head,
            agreed_l2_head_hash,
            agreed_l2_output_root,
            claimed_l2_block_number,
            claimed_l2_output_root,
        })
        .await?;
    Ok(())
}

pub async fn handle_proofs(
    mut channel: DuplexChannel<Message>,
    args: ValidateArgs,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    // Fetch rollup configuration
    let l2_chain_id = fetch_rollup_config(&args.core.op_node_url, &args.core.op_geth_url, None)
        .await?
        .l2_chain_id
        .to_string();
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
            proving_args.extend(vec![
                String::from("--u-block-hash"),
                precondition_data.validated_blobs[0]
                    .block_ref
                    .hash
                    .to_string(),
                String::from("--u-blob-kzg-hash"),
                precondition_data.validated_blobs[0]
                    .blob_hash
                    .hash
                    .to_string(),
                String::from("--v-block-hash"),
                precondition_data.validated_blobs[1]
                    .block_ref
                    .hash
                    .to_string(),
                String::from("--v-blob-kzg-hash"),
                precondition_data.validated_blobs[1]
                    .blob_hash
                    .hash
                    .to_string(),
            ]);
        }
        // boundless args
        if let Some(boundless_args) = &args.boundless_args {
            proving_args.extend(boundless_args.to_arg_vec(&args.boundless_storage_config));
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
fn needs_selector_patch(proof: &Proof) -> bool {
    match proof {
        Proof::ZKVMReceipt(_) => false,
        Proof::BoundlessSeal(seal, _) => {
            &seal[..4] != kailua_client::set_verifier_selector(crate::SET_BUILDER_ID).as_slice()
        }
    }
}
