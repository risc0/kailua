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

use crate::db::KailuaDB;
use crate::providers::beacon::{hash_to_fe, BlobProvider};
use crate::providers::optimism::OpNodeProvider;
use crate::KAILUA_GAME_TYPE;
use alloy::consensus::Blob;
use alloy::eips::eip4844::FIELD_ELEMENTS_PER_BLOB;
use alloy::eips::{BlockId, BlockNumberOrTag};
use alloy::network::primitives::BlockTransactionsKind;
use alloy::network::{BlockResponse, EthereumWallet, HeaderResponse};
use alloy::primitives::{Address, Bytes};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::LocalSigner;
use alloy::sol_types::SolValue;
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
        OpNodeProvider(ProviderBuilder::new().on_http(args.op_node_address.as_str().try_into()?));
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
    info!("Proposer address: {proposer_address}");

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
            .gameImpls(KAILUA_GAME_TYPE)
            .call()
            .await?
            .impl_,
        &proposer_provider,
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
    // Sanity check
    if kailua_game_implementation
        .treasury()
        .call()
        .await?
        .treasury_
        != kailua_db.treasury.address
    {
        error!("Invalid treasury address in KailuaGame implementation");
        exit(1);
    }
    // Run the proposer loop to sync and post
    info!(
        "Starting from proposal at factory index {}",
        kailua_db.next_factory_index
    );
    loop {
        // Wait for new data on every iteration
        sleep(Duration::from_secs(1)).await;
        // fetch latest games
        kailua_db
            .load_proposals(&anchor_state_registry, &op_node_provider, &cl_node_provider)
            .await
            .context("load_proposals")?;
        // Stack unresolved ancestors
        let mut unresolved_proposal_indices = kailua_db
            .unresolved_canonical_proposals(&proposer_provider)
            .await?;
        // Resolve in reverse order
        if !unresolved_proposal_indices.is_empty() {
            info!(
                "Attempting to resolve {} ancestors.",
                unresolved_proposal_indices.len()
            );
        }
        while let Some(proposal_index) = unresolved_proposal_indices.pop() {
            let proposal = kailua_db.proposals.get_mut(&proposal_index).unwrap();
            // Skip resolved games
            if proposal
                .fetch_finality(&proposer_provider)
                .await?
                .unwrap_or_default()
            {
                info!("Reached resolved ancestor proposal.");
                continue;
            }

            // Check if claim won in tournament
            if !proposal
                .fetch_parent_tournament_survivor_status(&proposer_provider)
                .await?
                .unwrap_or_default()
            {
                info!("Waiting for more proofs to resolve proposer as survivor");
                break;
            }

            // Check for timeout
            let challenger_duration = proposal
                .fetch_current_challenger_duration(&proposer_provider)
                .await?;
            if challenger_duration > 0 {
                info!("Waiting for {challenger_duration} more seconds before resolution.");
                break;
            }

            // resolve
            info!(
                "Resolving game at index {} and height {}.",
                proposal.index, proposal.output_block_number
            );
            proposal.resolve(&proposer_provider).await?;
        }
        // Submit proposal to extend canonical chain
        let Some(canonical_tip) = kailua_db.canonical_tip() else {
            warn!("No canonical proposal chain to extend!");
            continue;
        };
        // Query op-node to get latest safe l2 head
        let sync_status = op_node_provider.sync_status().await?;
        debug!("sync_status[safe_l2] {:?}", &sync_status["safe_l2"]);
        let output_block_number = sync_status["safe_l2"]["number"].as_u64().unwrap();
        if output_block_number < canonical_tip.output_block_number {
            warn!(
                "op-node is still {} blocks behind safe l2 head.",
                canonical_tip.output_block_number - output_block_number
            );
            continue;
        } else if output_block_number - canonical_tip.output_block_number
            < kailua_db.config.proposal_block_count
        {
            info!(
                "Waiting for safe l2 head to advance by {} more blocks before submitting proposal.",
                kailua_db.config.proposal_block_count
                    - (output_block_number - canonical_tip.output_block_number)
            );
            continue;
        }
        // Wait for L1 timestamp to advance beyond the safety gap for proposals
        let proposed_block_number =
            canonical_tip.output_block_number + kailua_db.config.proposal_block_count;
        let chain_time = proposer_provider
            .get_block(
                BlockId::Number(BlockNumberOrTag::Latest),
                BlockTransactionsKind::Hashes,
            )
            .await
            .context("get_block")?
            .expect("Could not fetch latest L1 block")
            .header()
            .timestamp();
        if !kailua_db
            .config
            .allows_proposal(proposed_block_number, chain_time)
        {
            let min_proposal_time = kailua_db.config.min_proposal_time(proposed_block_number);
            let time_to_wait = min_proposal_time.saturating_sub(chain_time);
            info!("Waiting for {time_to_wait} more seconds of chain time for proposal gap.");
            continue;
        }

        // Prepare proposal
        let proposed_output_root = op_node_provider
            .output_at_block(proposed_block_number)
            .await?;
        // Prepare intermediate outputs
        let mut io_hashes = vec![];
        let first_io_number = canonical_tip.output_block_number + 1;
        for i in first_io_number..proposed_block_number {
            let output = op_node_provider.output_at_block(i).await?;
            io_hashes.push(hash_to_fe(output));
        }
        let mut io_blobs = vec![];
        loop {
            let start = io_blobs.len() * FIELD_ELEMENTS_PER_BLOB as usize;
            if start >= io_hashes.len() {
                break;
            }
            let end = (start + FIELD_ELEMENTS_PER_BLOB as usize).min(io_hashes.len());
            let io_bytes = io_hashes[start..end].concat();
            // Encode as blob sidecar
            let blob = Blob::right_padding_from(io_bytes.as_slice());
            io_blobs.push(blob);
        }
        let sidecar = crate::providers::beacon::blob_sidecar(io_blobs)?;

        // todo calculate required duplication counter (factor proposer honesty into correctness)

        // compute extra data with block number, parent factory index, and blob hash
        let extra_data = [
            proposed_block_number.abi_encode_packed(),
            canonical_tip.index.abi_encode_packed(),
            0u64.abi_encode_packed(),
        ]
        .concat();
        // Check collateral requirements
        let bond_value = kailua_db.treasury.fetch_bond(&proposer_provider).await?;
        let paid_in = kailua_db
            .treasury
            .fetch_balance(&proposer_provider, proposer_address)
            .await?;
        let balance = proposer_provider.get_balance(proposer_address).await?;
        let owed_collateral = bond_value.saturating_sub(paid_in);
        if balance < owed_collateral {
            error!("INSUFFICIENT BALANCE! Need to lock in at least {owed_collateral}.");
            continue;
        }
        // Submit proposal
        info!("Proposing output {proposed_output_root} at {proposed_block_number} with {owed_collateral} additional collateral.");
        kailua_db
            .treasury
            .treasury_contract_instance(&proposer_provider)
            .propose(proposed_output_root, Bytes::from(extra_data))
            .value(owed_collateral)
            .sidecar(sidecar)
            .send()
            .await
            .context("propose (send)")?
            .get_receipt()
            .await
            .context("propose (get_receipt)")?;
    }
}
