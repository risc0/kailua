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
use crate::proposal::ProposalDB;
use crate::{hash_to_fe, output_at_block, FAULT_PROOF_GAME_TYPE};
use alloy::consensus::Blob;
use alloy::network::{EthereumWallet, Network};
use alloy::primitives::{Address, Bytes};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::LocalSigner;
use alloy::sol_types::SolValue;
use alloy::transports::Transport;
use anyhow::Context;
use std::process::exit;
use std::str::FromStr;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

#[derive(clap::Args, Debug, Clone)]
pub struct ProposeArgs {
    #[arg(long, short, help = "Verbosity level (0-4)", action = clap::ArgAction::Count)]
    pub v: u8,

    /// Address of OP-NODE endpoint to use
    #[clap(long)]
    pub op_node_address: String,
    /// Address of L1 JSON-RPC endpoint to use (eth namespace required)
    #[clap(long)]
    pub l1_node_address: String,
    /// Address of the L1 Beacon API endpoint to use.
    #[clap(long)]
    pub l1_beacon_address: String,

    /// Address of the L1 `AnchorStateRegistry` contract
    #[clap(long)]
    pub registry_contract: String,

    /// Secret key of L1 wallet to use for proposing outputs
    #[clap(long)]
    pub proposer_key: String,
}

pub async fn propose(args: ProposeArgs) -> anyhow::Result<()> {
    // initialize blockchain connections
    let op_node_provider =
        ProviderBuilder::new().on_http(args.op_node_address.as_str().try_into()?);
    let cl_node_provider = BlobProvider::new(args.l1_beacon_address.as_str()).await?;

    // initialize proposer wallet
    info!("Initializing proposer wallet.");
    let proposer_signer = LocalSigner::from_str(&args.proposer_key)?;
    let proposer_address = proposer_signer.address();
    let proposer_wallet = EthereumWallet::from(proposer_signer);
    let proposer_provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(&proposer_wallet)
        .on_http(args.l1_node_address.as_str().try_into()?);
    // Init registry and factory contracts
    let anchor_state_registry = kailua_contracts::IAnchorStateRegistry::new(
        Address::from_str(&args.registry_contract)?,
        &proposer_provider,
    );
    info!("AnchorStateRegistry({:?})", anchor_state_registry.address());
    let dispute_game_factory = kailua_contracts::IDisputeGameFactory::new(
        anchor_state_registry.disputeGameFactory().call().await?._0,
        &proposer_provider,
    );
    info!("DisputeGameFactory({:?})", dispute_game_factory.address());
    let game_count: u64 = dispute_game_factory
        .gameCount()
        .call()
        .await?
        .gameCount_
        .to();
    info!("There have been {game_count} games created using DisputeGameFactory");
    let kailua_game_implementation = kailua_contracts::KailuaGame::new(
        dispute_game_factory
            .gameImpls(FAULT_PROOF_GAME_TYPE)
            .call()
            .await?
            .impl_,
        &proposer_provider,
    );
    info!(
        "KailuaGame({:?})",
        kailua_game_implementation.address()
    );
    if kailua_game_implementation.address().is_zero() {
        error!("Fault proof game is not installed!");
        exit(1);
    }
    // load constants
    let max_proposal_span: u64 = kailua_game_implementation
        .maxBlockCount()
        .call()
        .await?
        .maxBlockCount_
        .to();
    let max_clock_duration = kailua_game_implementation
        .maxClockDuration()
        .call()
        .await?
        .maxClockDuration_;
    let bond_value = dispute_game_factory
        .initBonds(FAULT_PROOF_GAME_TYPE)
        .call()
        .await?
        .bond_;
    // Initialize empty state
    info!("Initializing..");
    let mut proposal_db = ProposalDB::default();
    // Run the proposer loop
    loop {
        // Wait for new data on every iteration
        sleep(Duration::from_secs(1)).await;
        // fetch latest games
        proposal_db
            .load_proposals(&dispute_game_factory, &op_node_provider, &cl_node_provider)
            .await
            .context("load_proposals")?;
        // Maintain the canonical proposal chain
        let Some(canonical_tip_index) = proposal_db.canonical_tip_index else {
            warn!("No canonical proposal tip known!");
            continue;
        };
        // Stack unresolved ancestors
        let mut unresolved_proposal_indices = proposal_db
            .unresolved_canonical_proposals(&proposer_provider)
            .await?;
        // Resolve in reverse order
        if !unresolved_proposal_indices.is_empty() {
            info!(
                "Attempting to resolve {} ancestors.",
                unresolved_proposal_indices.len()
            );
        }
        while let Some(local_index) = unresolved_proposal_indices.pop() {
            let local_proposal = &mut proposal_db.proposals[local_index];
            let proposal_contract = local_proposal.game_contract(&proposer_provider);
            // Skip resolved games
            if local_proposal.is_game_resolved() {
                info!("Reached resolved ancestor proposal.");
                continue;
            }

            // Check for challenges
            if proposal_contract.challengedAt(0).call().await?._0 > 0 {
                local_proposal.challenged.insert(0);
            }
            local_proposal.unresolved_challenges = proposal_contract
                .unresolvedClaimCount()
                .call()
                .await
                .context(format!("unresolvedClaimCount local_index {local_index}"))?
                ._0;
            if local_proposal.has_unresolved_challenges() {
                info!(
                    "Waiting for {} challenges to be resolved.",
                    local_proposal.unresolved_challenges
                );
                break;
            }

            // Check for timeout
            let challenger_duration = proposal_contract
                .getChallengerDuration()
                .call()
                .await?
                .duration_;
            if challenger_duration < max_clock_duration {
                info!(
                    "Waiting for {} more seconds before resolution.",
                    max_clock_duration - challenger_duration
                );
                break;
            }

            // resolve
            resolve_game(proposal_contract).await?;
        }
        // Submit proposal to extend canonical chain
        let canonical_tip = &proposal_db.proposals[canonical_tip_index];
        // Query op-node to get latest safe l2 head
        let sync_status: serde_json::Value = op_node_provider
            .client()
            .request_noparams("optimism_syncStatus")
            .await?;
        debug!("sync_status[safe_l2] {:?}", &sync_status["safe_l2"]);
        let output_block_number = sync_status["safe_l2"]["number"].as_u64().unwrap();
        let balance = proposer_provider.get_balance(proposer_address).await?;
        if balance < bond_value {
            error!("INSUFFICIENT BALANCE!");
            continue;
        } else if output_block_number < canonical_tip.output_block_number {
            warn!(
                "op-node is still {} blocks behind safe l2 head.",
                canonical_tip.output_block_number - output_block_number
            );
            continue;
        } else if output_block_number - canonical_tip.output_block_number < max_proposal_span {
            info!(
                "Waiting for safe l2 head to advance by {} more blocks before submitting proposal.",
                max_proposal_span - (output_block_number - canonical_tip.output_block_number)
            );
            continue;
        }
        // Prepare proposal
        let proposed_block_number = canonical_tip.output_block_number + max_proposal_span;
        let proposed_output_root =
            output_at_block(&op_node_provider, proposed_block_number).await?;
        // Prepare intermediate outputs
        let mut intermediate_outputs = vec![];
        let first_io_number = canonical_tip.output_block_number + 1;
        for i in first_io_number..proposed_block_number {
            let output = output_at_block(&op_node_provider, i).await?;
            intermediate_outputs.push(hash_to_fe(output));
        }
        let io_bytes = intermediate_outputs.concat();
        // Encode as blob sidecar
        let blob = Blob::right_padding_from(io_bytes.as_slice());
        let sidecar = crate::blob_sidecar(blob)?;

        // compute extra data with block number, parent factory index, and blob hash
        let extra_data = [
            proposed_block_number.abi_encode_packed(),
            canonical_tip.factory_index.abi_encode_packed(),
            sidecar
                .versioned_hash_for_blob(0)
                .unwrap()
                .abi_encode_packed(),
        ]
        .concat();
        // Submit proposal
        info!("Proposing output {proposed_output_root} at {proposed_block_number}.");
        dispute_game_factory
            .create(
                FAULT_PROOF_GAME_TYPE,
                proposed_output_root,
                Bytes::from(extra_data),
            )
            .value(bond_value)
            .sidecar(sidecar)
            .send()
            .await
            .context("create KailuaGame (send)")?
            .get_receipt()
            .await
            .context("create KailuaGame (get_receipt)")?;
    }
}

pub async fn resolve_game<T: Transport + Clone, P: Provider<T, N>, N: Network>(
    game: kailua_contracts::KailuaGame::KailuaGameInstance<T, P, N>,
) -> anyhow::Result<N::ReceiptResponse> {
    info!("Resolving game.");
    game.resolve()
        .send()
        .await
        .context("KailuaTreasury::resolve (send)")?
        .get_receipt()
        .await
        .context("KailuaTreasury::resolve (get_receipt)")
}
