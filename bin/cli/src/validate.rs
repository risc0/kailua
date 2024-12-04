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
use crate::KAILUA_GAME_TYPE;
use alloy::eips::eip4844::IndexedBlobHash;
use alloy::eips::BlockNumberOrTag;
use alloy::network::primitives::BlockTransactionsKind;
use alloy::network::EthereumWallet;
use alloy::primitives::{Address, FixedBytes, U256};
use alloy::providers::{Provider, ProviderBuilder, ReqwestProvider};
use alloy::signers::local::LocalSigner;
use anyhow::{anyhow, bail, Context};
use kailua_client::fpvm_proof_file_name;
use kailua_common::oracle::BlobFetchRequest;
use kailua_common::precondition::{precondition_hash, PreconditionValidationData};
use kailua_common::ProofJournal;
use kailua_contracts::{IAnchorStateRegistry, IDisputeGameFactory, KailuaGame};
use kailua_host::fetch_rollup_config;
use op_alloy_protocol::BlockInfo;
use risc0_zkvm::Receipt;
use std::env;
use std::path::Path;
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
    #[arg(long, short, help = "Verbosity level (0-4)", action = clap::ArgAction::Count)]
    pub v: u8,

    /// Address of OP-NODE endpoint to use
    #[clap(long)]
    pub op_node_address: String,
    /// Address of L2 JSON-RPC endpoint to use (eth and debug namespace required).
    #[clap(long)]
    pub l2_node_address: String,
    /// Address of L1 JSON-RPC endpoint to use (eth namespace required)
    #[clap(long)]
    pub l1_node_address: String,
    /// Address of the L1 Beacon API endpoint to use.
    #[clap(long)]
    pub l1_beacon_address: String,

    /// Address of the L1 `AnchorStateRegistry` contract
    #[clap(long)]
    pub registry_contract: String,

    /// Secret key of L1 wallet to use for challenging and proving outputs
    #[clap(long)]
    pub validator_key: String,
}

pub async fn validate(args: ValidateArgs) -> anyhow::Result<()> {
    // We run two concurrent tasks, one for the chain, and one for the prover.
    // Both tasks communicate using the duplex channel
    let channel_pair = DuplexChannel::new_pair(4096);

    let handle_proposals = spawn(handle_proposals(channel_pair.0, args.clone()));
    let handle_proofs = spawn(handle_proofs(channel_pair.1, args));

    let (proposals_task, proofs_task) = try_join!(handle_proposals, handle_proofs)?;
    proposals_task.context("handle_proposals")?;
    proofs_task.context("handle_proofs")?;

    Ok(())
}

#[derive(Clone, Debug)]
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
) -> anyhow::Result<()> {
    // initialize blockchain connections
    info!("Initializing rpc connections.");
    let op_node_provider =
        OpNodeProvider(ProviderBuilder::new().on_http(args.op_node_address.as_str().try_into()?));
    let l1_node_provider =
        ProviderBuilder::new().on_http(args.l1_node_address.as_str().try_into()?);
    let l2_node_provider =
        ProviderBuilder::new().on_http(args.l2_node_address.as_str().try_into()?);
    let cl_node_provider = BlobProvider::new(args.l1_beacon_address.as_str()).await?;

    // initialize validator wallet
    info!("Initializing validator wallet.");
    let validator_signer = LocalSigner::from_str(&args.validator_key)?;
    let validator_address = validator_signer.address();
    let validator_wallet = EthereumWallet::from(validator_signer);
    let validator_provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(validator_wallet)
        .on_http(args.l1_node_address.as_str().try_into()?);
    info!("Validator address: {validator_address}");

    // Init registry and factory contracts
    let anchor_state_registry = IAnchorStateRegistry::new(
        Address::from_str(&args.registry_contract)?,
        &validator_provider,
    );
    info!("AnchorStateRegistry({:?})", anchor_state_registry.address());
    let dispute_game_factory = IDisputeGameFactory::new(
        anchor_state_registry.disputeGameFactory().call().await?._0,
        &validator_provider,
    );
    info!("DisputeGameFactory({:?})", dispute_game_factory.address());
    let game_count: u64 = dispute_game_factory
        .gameCount()
        .call()
        .await?
        .gameCount_
        .to();
    info!("There have been {game_count} games created using DisputeGameFactory");
    let kailua_game_implementation = KailuaGame::new(
        dispute_game_factory
            .gameImpls(KAILUA_GAME_TYPE)
            .call()
            .await?
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
    let mut kailua_db = KailuaDB::init(&anchor_state_registry).await?;
    info!("KailuaTreasury({:?})", kailua_db.treasury.address);
    // Run the validator loop
    info!(
        "Starting from proposal at factory index {}",
        kailua_db.next_factory_index
    );
    loop {
        // Wait for new data on every iteration
        sleep(Duration::from_secs(1)).await;
        // track last seen game
        let last_proposal_index = kailua_db
            .proposals
            .last_key_value()
            .map(|x| *x.0)
            .unwrap_or_default();
        // fetch latest games
        kailua_db
            .load_proposals(&anchor_state_registry, &op_node_provider, &cl_node_provider)
            .await
            .context("load_proposals")?;

        // check new proposals for fault and queue potential responses
        for (_, proposal) in kailua_db.proposals.range(last_proposal_index..u64::MAX) {
            // skip seen before proposal
            if proposal.index == last_proposal_index {
                continue;
            }
            // skip this proposal if it has no contender
            let Some(contender) = proposal.contender else {
                continue;
            };
            // request a proof of the match results
            let contender = kailua_db
                .proposals
                .get(&contender)
                .expect("Missing contender from database.");
            request_proof(
                &mut channel,
                contender,
                proposal,
                &l1_node_provider,
                &l2_node_provider,
                &op_node_provider,
            )
            .await?;
        }

        // publish computed proofs and resolve proven challenges
        while !channel.receiver.is_empty() {
            let Message::Proof(proposal_index, receipt) = channel
                .receiver
                .recv()
                .await
                .ok_or(anyhow!("proposals receiver channel closed"))?
            else {
                bail!("Unexpected message type.");
            };
            let proposal = kailua_db.proposals.get(&proposal_index).unwrap();
            let proposal_parent = kailua_db.proposals.get(&proposal.parent).unwrap();
            let proposal_parent_contract =
                proposal_parent.tournament_contract_instance(&validator_provider);
            let proof_journal = ProofJournal::decode_packed(receipt.journal.as_ref())?;
            info!("Proof journal: {:?}", proof_journal);
            let contender_index = proposal.contender.unwrap();
            let contender = kailua_db.proposals.get(&contender_index).unwrap();

            let u_index = proposal_parent
                .child_index(contender_index)
                .expect("Could not look up contender's index in parent tournament");
            let v_index = proposal_parent
                .child_index(proposal.index)
                .expect("Could not look up contender's index in parent tournament");

            let challenge_position =
                proof_journal.claimed_l2_block_number - proposal_parent.output_block_number - 1;

            // patch the receipt image id if in dev mode
            let expected_image_id = proposal_parent_contract.imageId().call().await?.imageId_.0;
            #[cfg(feature = "devnet")]
            let receipt = {
                let mut receipt = receipt;
                let risc0_zkvm::InnerReceipt::Fake(fake_inner_receipt) = &mut receipt.inner else {
                    bail!("Found real receipt under devmode");
                };
                let risc0_zkvm::MaybePruned::Value(claim) = &mut fake_inner_receipt.claim else {
                    bail!("Fake receipt claim is pruned.");
                };
                warn!("DEVNET-ONLY: Patching fake receipt image id to match game contract.");
                claim.pre = risc0_zkvm::MaybePruned::Pruned(expected_image_id.into());
                receipt
            };

            // verify that the receipt is valid
            if receipt.verify(expected_image_id).is_err() {
                error!("Could not verify receipt against image id in contract.");
            } else {
                info!("Receipt validated.");
            }

            // only prove unproven games
            let proof_status = proposal_parent_contract
                .proofStatus(U256::from(u_index), U256::from(v_index))
                .call()
                .await
                .context("proof_status")?
                ._0;
            if proof_status != 0 {
                warn!("Skipping proof submission for already proven game at local index {proposal_index}.");
                continue;
            } else {
                info!("Proof status: {proof_status}");
            }

            let encoded_seal = risc0_ethereum_contracts::encode_seal(&receipt)?;

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

            let possible_precondition_hash = precondition_hash(
                &contender.io_blob_for(challenge_position).0,
                &proposal.io_blob_for(challenge_position).0,
            );
            if possible_precondition_hash != proof_journal.precondition_output {
                warn!("Possible precondition hash mismatch. Found {}, computed {possible_precondition_hash}", proof_journal.precondition_output);
            } else {
                info!("Proof Precondition hash confirmed.")
            }

            let config_hash = proposal_parent_contract
                .configHash()
                .call()
                .await?
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

            proposal_parent_contract
                .prove(
                    [u_index, v_index, challenge_position],
                    encoded_seal.into(),
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
                .context("prove (send)")?
                .get_receipt()
                .await
                .context("prove (get_receipt)")?;

            let proof_status = proposal_parent_contract
                .proofStatus(U256::from(u_index), U256::from(v_index))
                .call()
                .await
                .context("proof_status (verify)")?
                ._0;
            info!(
                "Match between {contender_index} and {} proven: {proof_status}",
                proposal.index
            );
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
        proposal.output_block_number - proposal.io_hashes.len() as u64 - 1 + challenge_point; // the challenge point is zero indexed, so it cancels out
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
) -> anyhow::Result<()> {
    // Fetch rollup configuration
    let l2_chain_id = fetch_rollup_config(&args.op_node_address, &args.l2_node_address, None)
        .await?
        .l2_chain_id
        .to_string();
    // Read executable paths from env vars
    let kailua_host = env::var("KAILUA_HOST").unwrap_or_else(|_| {
        warn!("KAILUA_HOST set to default ./target/debug/kailua-host");
        String::from("./target/debug/kailua-host")
    });
    let data_dir = env::var("KAILUA_DATA").unwrap_or_else(|_| {
        warn!("KAILUA_DATA set to default .localtestdata");
        String::from(".localtestdata")
    });
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
            (0..args.v).map(|_| 'v').collect::<String>(),
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
            args.l1_node_address.clone(),
            String::from("--l1-beacon-address"), // l1 cl node
            args.l1_beacon_address.clone(),
            String::from("--l2-node-address"), // l2 el node
            args.l2_node_address.clone(),
            String::from("--op-node-address"), // l2 cl node
            args.op_node_address.clone(),
            String::from("--data-dir"), // path to cache
            data_dir.clone(),
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
        // verbosity level
        if args.v > 0 {
            proving_args.push(verbosity);
        }
        // Prove via kailua-host (re dev mode/bonsai: env vars inherited!)
        let mut kailua_host_command = Command::new(&kailua_host);
        // get fake receipts when building under devnet
        #[cfg(feature = "devnet")]
        kailua_host_command.env("RISC0_DEV_MODE", "1");
        // pass arguments to point at target block
        kailua_host_command.args(proving_args);
        debug!("kailua_host_command {:?}", &kailua_host_command);
        {
            let proving_task = kailua_host_command
                .kill_on_drop(true)
                .spawn()
                .context("Invoking kailua-host")?
                .wait()
                // .output()
                .await?;
            if !proving_task.success() {
                error!("Proving task failure.");
            } else {
                info!("Proving task successful.");
            }
        }
        sleep(Duration::from_secs(1)).await;
        // Read receipt file
        if !Path::new(&proof_file_name).exists() {
            error!("Receipt file {proof_file_name} not found.");
        } else {
            info!("Found receipt file.");
        }
        let mut receipt_file = File::open(proof_file_name.clone()).await?;
        info!("Opened receipt file {proof_file_name}.");
        let mut receipt_data = Vec::new();
        receipt_file.read_to_end(&mut receipt_data).await?;
        info!("Read entire receipt file.");
        let receipt: Receipt = bincode::deserialize(&receipt_data)?;
        // Send proof via the channel
        channel
            .sender
            .send(Message::Proof(proposal_index, receipt))
            .await?;
        info!("Proof for local index {proposal_index} complete.");
    }
}
