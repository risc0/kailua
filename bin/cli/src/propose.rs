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

use crate::db::proposal::Proposal;
use crate::db::KailuaDB;
use crate::providers::beacon::BlobProvider;
use crate::providers::optimism::OpNodeProvider;
use crate::{stall::Stall, CoreArgs, KAILUA_GAME_TYPE};
use alloy::consensus::BlockHeader;
use alloy::eips::{BlockId, BlockNumberOrTag};
use alloy::network::primitives::BlockTransactionsKind;
use alloy::network::{BlockResponse, EthereumWallet};
use alloy::primitives::Bytes;
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::LocalSigner;
use alloy::sol_types::SolValue;
use anyhow::Context;
use kailua_common::blobs::hash_to_fe;
use kailua_common::client::config_hash;
use kailua_contracts::*;
use kailua_host::fetch_rollup_config;
use std::path::PathBuf;
use std::process::exit;
use std::str::FromStr;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

#[derive(clap::Args, Debug, Clone)]
pub struct ProposeArgs {
    #[clap(flatten)]
    pub core: CoreArgs,

    /// Secret key of L1 wallet to use for proposing outputs
    #[clap(long, env)]
    pub proposer_key: String,
}

pub async fn propose(args: ProposeArgs, data_dir: PathBuf) -> anyhow::Result<()> {
    // initialize blockchain connections
    let op_node_provider =
        OpNodeProvider(ProviderBuilder::new().on_http(args.core.op_node_url.as_str().try_into()?));
    let cl_node_provider = BlobProvider::new(args.core.beacon_rpc_url.as_str()).await?;
    let eth_rpc_provider =
        ProviderBuilder::new().on_http(args.core.eth_rpc_url.as_str().try_into()?);

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

    // initialize proposer wallet
    info!("Initializing proposer wallet.");
    let proposer_signer = LocalSigner::from_str(&args.proposer_key)?;
    let proposer_address = proposer_signer.address();
    let proposer_wallet = EthereumWallet::from(proposer_signer);
    let proposer_provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(&proposer_wallet)
        .on_http(args.core.eth_rpc_url.as_str().try_into()?);
    info!("Proposer address: {proposer_address}");

    // Init registry and factory contracts
    let dispute_game_factory =
        kailua_contracts::IDisputeGameFactory::new(dgf_address, &proposer_provider);
    info!("DisputeGameFactory({:?})", dispute_game_factory.address());
    let game_count: u64 = dispute_game_factory
        .gameCount()
        .stall()
        .await
        .gameCount_
        .to();
    info!("There have been {game_count} games created using DisputeGameFactory");
    let kailua_game_implementation = kailua_contracts::KailuaGame::new(
        dispute_game_factory
            .gameImpls(KAILUA_GAME_TYPE)
            .stall()
            .await
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
    let mut kailua_db = KailuaDB::init(data_dir, &dispute_game_factory).await?;
    info!("KailuaTreasury({:?})", kailua_db.treasury.address);
    // Run the proposer loop to sync and post
    info!(
        "Starting from proposal at factory index {}",
        kailua_db.state.next_factory_index
    );

    loop {
        // Wait for new data on every iteration
        sleep(Duration::from_secs(1)).await;
        // fetch latest games
        kailua_db
            .load_proposals(&dispute_game_factory, &op_node_provider, &cl_node_provider)
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
            let proposal = kailua_db.get_local_proposal(&proposal_index).unwrap();
            let parent = kailua_db.get_local_proposal(&proposal.parent).unwrap();
            let parent_contract = parent.tournament_contract_instance(&proposer_provider);
            info!("Parent Tournament Children:");
            for i in 0..u64::MAX {
                if let Ok(res) = parent_contract
                    .children(alloy::primitives::U256::from(i))
                    .call()
                    .await
                {
                    info!("{}", res._0);
                } else {
                    break;
                }
            }

            let proposal = kailua_db.get_local_proposal(&proposal_index).unwrap();
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
            if proposal.has_parent()
                && !proposal
                    .fetch_parent_tournament_survivor_status(&proposer_provider)
                    .await
                    .unwrap_or_default()
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

            if let Err(e) = proposal.resolve(&proposer_provider).await {
                error!("Failed to resolve proposal: {e:?}");
            }
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
        let mut io_field_elements = vec![];
        let first_io_number = canonical_tip.output_block_number + 1;
        for i in first_io_number..proposed_block_number {
            let output = op_node_provider.output_at_block(i).await?;
            io_field_elements.push(hash_to_fe(output));
        }
        let sidecar = Proposal::create_sidecar(&io_field_elements)?;

        // Calculate required duplication counter
        let mut dupe_counter = 0u64;
        let unique_extra_data = loop {
            // compute extra data with block number, parent factory index, and blob hash
            let extra_data = [
                proposed_block_number.abi_encode_packed(),
                canonical_tip.index.abi_encode_packed(),
                dupe_counter.abi_encode_packed(),
            ]
            .concat();
            // check if proposal exists
            let dupe_game_address = dispute_game_factory
                .games(
                    KAILUA_GAME_TYPE,
                    proposed_output_root,
                    Bytes::from(extra_data.clone()),
                )
                .stall()
                .await
                .proxy_;
            if dupe_game_address.is_zero() {
                // proposal was not made before using this dupe counter
                break Some(extra_data);
            }
            // fetch proposal from local data
            let dupe_game_index: u64 = KailuaTournament::new(dupe_game_address, &proposer_provider)
                .gameIndex()
                .stall()
                .await
                ._0
                .to();
            let Some(dupe_proposal) = kailua_db.get_local_proposal(&dupe_game_index) else {
                // we need to fetch this proposal's data
                break None;
            };
            // check if proposal was made incorrectly or by an already eliminated player
            if dupe_proposal.is_correct().unwrap_or_default()
                && !kailua_db.was_proposer_eliminated_before(&dupe_proposal)
            {
                break None;
            }
            // increment counter
            dupe_counter += 1;
        };

        let Some(extra_data) = unique_extra_data else {
            // this proposal was already correctly made or we need more data
            continue;
        };
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
        info!("Proposing output {proposed_output_root} at l2 block number {proposed_block_number} with {owed_collateral} additional collateral and duplication counter {dupe_counter}.");
        match kailua_db
            .treasury
            .treasury_contract_instance(&proposer_provider)
            .propose(proposed_output_root, Bytes::from(extra_data))
            .value(owed_collateral)
            .sidecar(sidecar)
            .send()
            .await
            .context("propose (send)")
        {
            Ok(txn) => match txn.get_receipt().await.context("propose (get_receipt)") {
                Ok(receipt) => {
                    info!("Proposal submitted: {receipt:?}")
                }
                Err(e) => {
                    error!("Failed to confirm proposal txn: {e:?}");
                }
            },
            Err(e) => {
                error!("Failed to send proposal txn: {e:?}");
            }
        }
    }
}
