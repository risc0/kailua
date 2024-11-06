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

use crate::blob_provider::BlobProvider;
use crate::channel::DuplexChannel;
use crate::proposal::{Proposal, ProposalDB};
use crate::{blob_fe_proof, block_hash, hash_to_fe, output_at_block, FAULT_PROOF_GAME_TYPE};
use alloy::eips::eip4844::kzg_to_versioned_hash;
use alloy::network::{EthereumWallet, Network};
use alloy::primitives::{Address, Bytes, FixedBytes, U256};
use alloy::providers::{Provider, ProviderBuilder, ReqwestProvider};
use alloy::signers::local::LocalSigner;
use alloy::transports::Transport;
use anyhow::{anyhow, bail, Context};
use kailua_client::fpvm_proof_file_name;
use kailua_common::{intermediate_outputs, ProofJournal};
use kailua_contracts::{FaultProofGame, IAnchorStateRegistry, IDisputeGameFactory};
use kailua_host::fetch_rollup_config;
use risc0_zkvm::Receipt;
use std::collections::{HashMap, HashSet};
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
        local_index: usize,
        l1_head: FixedBytes<32>,
        l2_head: FixedBytes<32>,
        l2_output_root: FixedBytes<32>,
        l2_block_number: u64,
        l2_claim: FixedBytes<32>,
    },
    Proof(usize, Receipt),
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
    let kailua_client = env::var("KAILUA_CLIENT").unwrap_or_else(|_| {
        warn!("KAILUA_CLIENT set to default ./target/debug/kailua-client");
        String::from("./target/debug/kailua-client")
    });
    let data_dir = env::var("KAILUA_DATA").unwrap_or_else(|_| {
        warn!("KAILUA_DATA set to default .localtestdata");
        String::from(".localtestdata")
    });
    // Run proof generator loop
    loop {
        // Dequeue messages
        // todo: priority goes to fault proofs for games where one is the challenger
        // todo: secondary priority is validity proofs for mis-challenged games
        let Message::Proposal {
            local_index,
            l1_head,
            l2_head,
            l2_output_root,
            l2_block_number,
            l2_claim,
        } = channel
            .receiver
            .recv()
            .await
            .ok_or(anyhow!("proof receiver channel closed"))?
        else {
            bail!("Unexpected message type.");
        };
        info!("Processing proof for local index {local_index}.");
        // Prepare kailua-host parameters
        let proof_file_name = fpvm_proof_file_name(l1_head, l2_claim, l2_output_root);
        let l1_head = l1_head.to_string();
        let l2_head = l2_head.to_string();
        let l2_output_root = l2_output_root.to_string();
        let l2_claim = l2_claim.to_string();
        let l2_block_number = l2_block_number.to_string();
        let verbosity = [
            String::from("-"),
            (0..args.v).map(|_| 'v').collect::<String>(),
        ]
        .concat();
        let mut proving_args = vec![
            "--l1-head", // l1 head from on-chain proposal
            &l1_head,
            "--l2-head", // l2 starting block hash from on-chain proposal
            &l2_head,
            "--l2-output-root", // l2 starting output root
            &l2_output_root,
            "--l2-claim", // proposed output root
            &l2_claim,
            "--l2-block-number", // proposed block number
            &l2_block_number,
            "--l2-chain-id", // rollup chain id
            &l2_chain_id,
            "--l1-node-address", // l1 el node
            &args.l1_node_address,
            "--l1-beacon-address", // l1 cl node
            &args.l1_beacon_address,
            "--l2-node-address", // l2 el node
            &args.l2_node_address,
            "--op-node-address", // l2 cl node
            &args.op_node_address,
            "--exec", // path to kailua-client
            &kailua_client,
            "--data-dir", // path to cache
            &data_dir,
        ];
        // verbosity level
        if args.v > 0 {
            proving_args.push(&verbosity);
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
            .send(Message::Proof(local_index, receipt))
            .await?;
        info!("Proof for local index {local_index} complete.");
    }
}

pub async fn handle_proposals(
    mut channel: DuplexChannel<Message>,
    args: ValidateArgs,
) -> anyhow::Result<()> {
    // initialize blockchain connections
    info!("Initializing rpc connections.");
    let op_node_provider =
        ProviderBuilder::new().on_http(args.op_node_address.as_str().try_into()?);
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
    let fault_proof_game_implementation = FaultProofGame::new(
        dispute_game_factory
            .gameImpls(FAULT_PROOF_GAME_TYPE)
            .call()
            .await?
            .impl_,
        &validator_provider,
    );
    info!(
        "FaultProofGame({:?})",
        fault_proof_game_implementation.address()
    );
    if fault_proof_game_implementation.address().is_zero() {
        error!("Fault proof game is not installed!");
        exit(1);
    }
    // load constants
    let bond_value = dispute_game_factory
        .initBonds(FAULT_PROOF_GAME_TYPE)
        .call()
        .await?
        .bond_;
    // Initialize empty state
    info!("Initializing..");
    let mut proposal_db = ProposalDB::default();
    let mut validity_proof_matrix: HashMap<usize, HashSet<u32>> = Default::default();
    // Run the validator loop
    loop {
        // Wait for new data on every iteration
        sleep(Duration::from_secs(1)).await;
        // fetch latest games
        let new_proposals = proposal_db
            .load_proposals(&dispute_game_factory, &op_node_provider, &cl_node_provider)
            .await
            .context("load_proposals")?;
        // Validity Proofs: Scan unresolved canonical chain for unjustified challenges
        let mut unresolved_proposal_indices = proposal_db
            .unresolved_canonical_proposals(&validator_provider)
            .await?;
        while let Some(local_index) = unresolved_proposal_indices.pop() {
            let proposal = &proposal_db.proposals[local_index];
            let proposal_parent_obn =
                proposal_db.proposals[proposal.parent_local_index].output_block_number;
            let proposal = &mut proposal_db.proposals[local_index];
            let game_contract = proposal.game_contract(&validator_provider);
            proposal.unresolved_challenges = game_contract
                .unresolvedClaimCount()
                .call()
                .await
                .context(format!("unresolvedClaimCount local_index {local_index}"))?
                ._0;
            if !proposal.has_unresolved_challenges() {
                continue;
            }
            let validity_proofs = validity_proof_matrix.entry(local_index).or_default();
            // todo: use events
            let proposal_span = (proposal.output_block_number - proposal_parent_obn) as u32;
            for challenged_position in 1..=proposal_span {
                if validity_proofs.contains(&challenged_position)
                    || proposal.is_output_proven(challenged_position).is_some()
                {
                    // proof already requested/submitted
                    continue;
                }
                // check challenge status
                if game_contract
                    .challengedAt(challenged_position)
                    .call()
                    .await
                    .context(format!("challengedAt({challenged_position})"))?
                    ._0
                    == 0
                {
                    // unchallenged
                    continue;
                }
                proposal.challenged.insert(challenged_position);
                // proven status
                let proof_status = game_contract
                    .provenAt(challenged_position)
                    .call()
                    .await
                    .context(format!("provenAt({challenged_position})"))?
                    ._0;
                if proof_status > 0 {
                    // proven
                    proposal
                        .proven
                        .insert(challenged_position, proof_status == 2);
                    continue;
                }
                // request new validity proof
                request_proof(
                    &mut channel,
                    proposal,
                    challenged_position,
                    &validator_provider,
                    &l2_node_provider,
                    &op_node_provider,
                )
                .await
                .context("request_proof")?;
            }
        }

        // Fault Proofs: Validate new proposals and issue new challenges
        let starting_index = proposal_db.proposals.len() - new_proposals;
        for local_index in starting_index..proposal_db.proposals.len() {
            let local_proposal = &mut proposal_db.proposals[local_index];
            // Issue possible challenge
            let Some(challenged_position) = local_proposal.canonical_challenge_position() else {
                // No challenges for correct proposals
                continue;
            };
            let factory_index = local_proposal.factory_index;
            let correct = local_proposal.is_correct();
            // query for on-chain challenge status
            let game_contract = local_proposal.game_contract(&validator_provider);
            if game_contract
                .challengedAt(challenged_position)
                .call()
                .await
                .context(format!("challengedAt({challenged_position})"))?
                ._0
                > 0
            {
                local_proposal.challenged.insert(challenged_position);
            }
            // issue challenge if needed
            if !local_proposal.is_output_challenged(challenged_position) {
                info!("Issuing challenge against position {challenged_position}.");
                game_contract
                    .challenge(challenged_position)
                    .value(bond_value / U256::from(2))
                    .send()
                    .await
                    .context(format!("challenge({challenged_position}) (send)"))?
                    .get_receipt()
                    .await
                    .context(format!("challenge({challenged_position}) (get_receipt)"))?;
            }
            // check challenger
            if game_contract
                .challenger(challenged_position)
                .call()
                .await?
                ._0
                != validator_address
            {
                info!("{correct} proposal at factory index {factory_index} was challenged by another validator.");
                continue;
            }
            // query for on-chain proof status
            let proof_status = game_contract
                .provenAt(challenged_position)
                .call()
                .await
                .context(format!("provenAt({challenged_position})"))?
                ._0;
            if proof_status > 0 {
                local_proposal
                    .proven
                    .insert(challenged_position, proof_status == 2);
            }
            // if the challenged output is unproven, enqueue a proving task
            if challenged_position != 0
                && local_proposal
                    .is_output_proven(challenged_position)
                    .is_none()
            {
                request_proof(
                    &mut channel,
                    local_proposal,
                    challenged_position,
                    &validator_provider,
                    &l2_node_provider,
                    &op_node_provider,
                )
                .await
                .context("request_proof")?;
            }
            info!("Validated {correct} proposal at factory index {factory_index}.");
        }

        // publish computed proofs and resolve proven challenges
        while !channel.receiver.is_empty() {
            let Message::Proof(local_index, receipt) = channel
                .receiver
                .recv()
                .await
                .ok_or(anyhow!("proposals receiver channel closed"))?
            else {
                bail!("Unexpected message type.");
            };
            let proposal = &proposal_db.proposals[local_index];
            let proposal_parent = &proposal_db.proposals[proposal.parent_local_index];
            let game_contract = proposal.game_contract(&validator_provider);
            let proof_journal = ProofJournal::decode_packed(receipt.journal.as_ref())?;
            let io_blob = proposal
                .intermediate_output_blob
                .clone()
                .expect("Missing blob data.");
            let proposal_span =
                (proposal.output_block_number - proposal_parent.output_block_number) as u32;
            let challenge_position = (proof_journal.claimed_l2_block_number
                - proposal_parent.output_block_number) as u32;
            let io_hashes = intermediate_outputs(&io_blob, proposal_span as usize - 1)?;

            let challenged_output = io_hashes
                .get(challenge_position as usize - 1)
                .copied()
                .unwrap_or(proposal.output_root);
            let is_fault_proof =
                hash_to_fe(proof_journal.claimed_l2_output_root) != challenged_output;
            let proof_label = if is_fault_proof { "fault" } else { "validity" };
            info!(
                "Utilizing {proof_label} proof in game at {}",
                proposal.game_address
            );
            // patch the receipt image id if in dev mode
            let expected_image_id = game_contract.imageId().call().await?.imageId_.0;
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
            let proof_status = game_contract
                .proofStatus(challenge_position)
                .call()
                .await
                .context(format!("proofStatus({challenge_position})"))?
                ._0;
            let proof_status = if proof_status == 0 {
                let encoded_seal = risc0_ethereum_contracts::encode_seal(&receipt)?;
                let mut proofs = vec![];

                if challenge_position > 1 {
                    let (proof, value) =
                        blob_fe_proof(&io_blob.blob, challenge_position as usize - 2)?;
                    proofs.push(Bytes::from(proof.to_vec()));
                    info!(
                        "Generating kzg proof for parent output at {}.",
                        challenge_position - 2
                    );
                    if value.to_vec() != hash_to_fe(proof_journal.agreed_l2_output_root).to_vec() {
                        error!(
                            "Invalid kzg proof {}/{}.",
                            hex::encode(value.to_vec()),
                            proof_journal.agreed_l2_output_root
                        );
                    }
                }
                if challenge_position < proposal_span {
                    let (proof, value) =
                        blob_fe_proof(&io_blob.blob, challenge_position as usize - 1)?;
                    proofs.push(Bytes::from(proof.to_vec()));
                    info!(
                        "Generating kzg proof for proposed output at {}.",
                        challenge_position - 1
                    );
                    if value.to_vec() != hash_to_fe(challenged_output).to_vec() {
                        error!(
                            "Invalid kzg proof {}/{challenged_output}.",
                            hex::encode(value.to_vec())
                        );
                    }
                }

                info!(
                    "Submitting proof for position {challenge_position}/{proposal_span} with {} kzg proof(s).",
                    proofs.len()
                );
                debug!("safeOutput: {}", proof_journal.agreed_l2_output_root);
                debug!(
                    "startingRootHash: {}",
                    game_contract
                        .startingRootHash()
                        .call()
                        .await?
                        .startingRootHash_
                );
                debug!("proposedOutput: {}", challenged_output);
                debug!(
                    "rootClaim: {}",
                    game_contract.rootClaim().call().await?.rootClaim_
                );
                debug!("computedOutput: {}", proof_journal.claimed_l2_output_root);
                debug!(
                    "blobCommitment: {}",
                    hex::encode(io_blob.kzg_commitment.as_slice())
                );
                debug!(
                    "versionedHash: {}",
                    kzg_to_versioned_hash(io_blob.kzg_commitment.as_slice())
                );
                debug!(
                    "proposalBlobHash: {}",
                    game_contract.proposalBlobHash().call().await?.blobHash_
                );
                debug!("proofs: {proofs:?}");

                game_contract
                    .prove(
                        challenge_position,
                        encoded_seal.into(),
                        proof_journal.agreed_l2_output_root,
                        challenged_output,
                        proof_journal.claimed_l2_output_root,
                        Bytes::from(io_blob.kzg_commitment.to_vec()),
                        proofs,
                    )
                    .send()
                    .await
                    .context("prove (send)")?
                    .get_receipt()
                    .await
                    .context("prove (get_receipt)")?;
                let proof_status = game_contract
                    .proofStatus(challenge_position)
                    .call()
                    .await
                    .context(format!("proofStatus({challenge_position})"))?
                    ._0;
                info!("Game proven: {proof_status}");
                proof_status
            } else {
                warn!("Skipping proof submission for already proven game at local index {local_index}.");
                proof_status
            };
            // only resolve unresolved games
            let game_status = game_contract
                .gameStatus(challenge_position)
                .call()
                .await
                .context(format!("gameStatus({challenge_position})"))?
                ._0;
            if proof_status != 0 && game_status == 0 {
                info!("Resolving output {challenge_position}.");
                game_contract
                    .resolveClaim(challenge_position)
                    .send()
                    .await
                    .context("resolveClaim (send)")?
                    .get_receipt()
                    .await
                    .context("resolveClaim (get_receipt)")?;
                let game_status = game_contract
                    .gameStatus(challenge_position)
                    .call()
                    .await
                    .context(format!("gameStatus({challenge_position})"))?
                    ._0;
                info!("Game resolved: {game_status}");
            }
        }
    }
}

async fn request_proof<T: Transport + Clone, P: Provider<T, N>, N: Network>(
    channel: &mut DuplexChannel<Message>,
    proposal: &Proposal,
    challenged_position: u32,
    l1_node_provider: P,
    l2_node_provider: &ReqwestProvider,
    op_node_provider: &ReqwestProvider,
) -> anyhow::Result<()> {
    let game_contract = proposal.game_contract(l1_node_provider);
    // Read additional data for Kona invocation
    info!("Requesting proof for local index {}.", proposal.local_index);
    let l1_head = game_contract
        .l1Head()
        .call()
        .await
        .context("l1Head")?
        .l1Head_;
    debug!("l1_head {:?}", &l1_head);
    let l2_head_number = game_contract
        .startingBlockNumber()
        .call()
        .await
        .context("startingBlockNumber")?
        .startingBlockNumber_
        .to::<u64>()
        + challenged_position as u64
        - 1;
    debug!("l2_head_number {:?}", &l2_head_number);
    let l2_head = block_hash(l2_node_provider, l2_head_number)
        .await
        .context("block_hash")?;
    debug!("l2_head {:?}", &l2_head);
    let l2_output_root = output_at_block(op_node_provider, l2_head_number)
        .await
        .context("output_at_block")?;
    let l2_block_number = l2_head_number + 1;
    let l2_claim = output_at_block(op_node_provider, l2_block_number)
        .await
        .context("output_at_block")?;
    // Message proving task
    channel
        .sender
        .send(Message::Proposal {
            local_index: proposal.local_index,
            l1_head,
            l2_head,
            l2_output_root,
            l2_block_number,
            l2_claim,
        })
        .await?;
    Ok(())
}
