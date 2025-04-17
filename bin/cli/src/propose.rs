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

use crate::db::proposal::{Proposal, ELIMINATIONS_LIMIT};
use crate::db::KailuaDB;
use crate::provider::{get_block, BlobProvider};
use crate::signer::ProposerSignerArgs;
use crate::transact::Transact;
use crate::{retry_with_context, stall::Stall, CoreArgs, KAILUA_GAME_TYPE};
use alloy::consensus::BlockHeader;
use alloy::eips::BlockNumberOrTag;
use alloy::network::{BlockResponse, Ethereum, TxSigner};
use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::{Provider, ProviderBuilder, RootProvider};
use alloy::sol_types::SolValue;
use anyhow::Context;
use kailua_client::args::parse_address;
use kailua_client::provider::OpNodeProvider;
use kailua_client::telemetry::TelemetryArgs;
use kailua_client::{await_tel, await_tel_res};
use kailua_common::blobs::hash_to_fe;
use kailua_common::config::config_hash;
use kailua_contracts::*;
use kailua_host::config::fetch_rollup_config;
use opentelemetry::global::{meter, tracer};
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use opentelemetry::KeyValue;
use std::future::IntoFuture;
use std::path::PathBuf;
use std::process::exit;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

#[derive(clap::Args, Debug, Clone)]
pub struct ProposeArgs {
    #[clap(flatten)]
    pub core: CoreArgs,

    /// L1 wallet to use for proposing outputs
    #[clap(flatten)]
    pub proposer_signer: ProposerSignerArgs,
    /// Address of the KailuaGame implementation to use
    #[clap(long, env, value_parser = parse_address)]
    pub kailua_game_implementation: Option<Address>,

    #[clap(flatten)]
    pub telemetry: TelemetryArgs,
}

pub async fn propose(args: ProposeArgs, data_dir: PathBuf) -> anyhow::Result<()> {
    // Telemetry
    let meter = meter("kailua");
    let meter_prune_num = meter.u64_counter("proposer.prune.count").build();
    let meter_prune_fail = meter.u64_counter("proposer.prune.errs").build();
    let meter_resolve_num = meter.u64_counter("proposer.resolve.count").build();
    let meter_resolve_last = meter.u64_gauge("proposer.resolve.last").build();
    let meter_resolve_fail = meter.u64_counter("proposer.resolve.errs").build();
    let meter_propose_num = meter.u64_counter("proposer.propose.count").build();
    let meter_propose_last = meter.u64_gauge("proposer.propose.last").build();
    let meter_propose_fail = meter.u64_counter("proposer.propose.errs").build();
    let meter_propose_fault = meter.u64_gauge("proposer.propose.fault").build();
    let meter_sync_canonical = meter.u64_gauge("proposer.sync.canonical").build();
    let meter_sync_next = meter.u64_gauge("proposer.sync.next").build();
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("propose"));

    // initialize blockchain connections
    let op_node_provider = OpNodeProvider(RootProvider::new_http(
        args.core.op_node_url.as_str().try_into()?,
    ));
    let cl_node_provider = await_tel!(context, BlobProvider::new(args.core.beacon_rpc_url))
        .context("BlobProvider::new")?;
    let eth_rpc_provider =
        RootProvider::<Ethereum>::new_http(args.core.eth_rpc_url.as_str().try_into()?);

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
        .await
        .addr_;

    // initialize proposer wallet
    info!("Initializing proposer wallet.");
    let proposer_wallet = await_tel_res!(
        context,
        tracer,
        "ProposerSignerArgs::wallet",
        args.proposer_signer.wallet(Some(config.l1_chain_id))
    )?;
    let proposer_address = proposer_wallet.default_signer().address();
    let proposer_provider = ProviderBuilder::new()
        .wallet(&proposer_wallet)
        .on_http(args.core.eth_rpc_url.as_str().try_into()?);
    info!("Proposer address: {proposer_address}");

    // Init registry and factory contracts
    let dispute_game_factory = IDisputeGameFactory::new(dgf_address, &proposer_provider);
    info!("DisputeGameFactory({:?})", dispute_game_factory.address());
    let game_count: u64 = dispute_game_factory
        .gameCount()
        .stall_with_context(context.clone(), "DisputeGameFactory::gameCount")
        .await
        .gameCount_
        .to();
    info!("There have been {game_count} games created using DisputeGameFactory");

    // Look up deployment to target
    let latest_game_impl_addr = dispute_game_factory
        .gameImpls(KAILUA_GAME_TYPE)
        .stall_with_context(context.clone(), "DisputeGameFactory::gameImpls")
        .await
        .impl_;
    let kailua_game_implementation_address = args
        .kailua_game_implementation
        .unwrap_or(latest_game_impl_addr);
    if args.kailua_game_implementation.is_some() {
        warn!("Using provided KailuaGame implementation {kailua_game_implementation_address}.");
    } else {
        info!("Using latest KailuaGame implementation {kailua_game_implementation_address} from DisputeGameFactory.");
    }

    let kailua_game_implementation =
        KailuaGame::new(kailua_game_implementation_address, &proposer_provider);
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
    // Run the proposer loop to sync and post
    info!(
        "Starting from proposal at factory index {}",
        kailua_db.state.next_factory_index
    );

    loop {
        // Wait for new data on every iteration
        sleep(Duration::from_secs(1)).await;
        // fetch latest games
        info!("Retrieving latest proposals..");
        await_tel!(
            context,
            kailua_db.load_proposals(&dispute_game_factory, &op_node_provider, &cl_node_provider)
        )
        .context("KailuaDB::load_proposals")?;

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

        // alert on honesty compromise
        if let Some(elimination_index) = kailua_db.state.eliminations.get(&proposer_address) {
            error!(
                "Proposer {proposer_address} honesty compromised at proposal {elimination_index}."
            );
            meter_propose_fault.record(
                *elimination_index,
                &[
                    KeyValue::new("treasury", kailua_db.treasury.address.to_string()),
                    KeyValue::new("proposer", proposer_address.to_string()),
                ],
            );
        }

        // Stack unresolved ancestors
        let mut unresolved_proposal_indices = await_tel!(
            context,
            kailua_db.unresolved_canonical_proposals(&proposer_provider)
        )
        .context("KailuaDB::unresolved_canonical_proposals")?;
        // Resolve in reverse order
        info!(
            "Found {} unresolved proposals.",
            unresolved_proposal_indices.len()
        );
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

            // Skip resolved games
            if await_tel!(context, proposal.fetch_finality(&proposer_provider))
                .context("Proposal::fetch_finality")?
                .unwrap_or_default()
            {
                info!("Reached resolved ancestor proposal.");
                continue;
            }

            // Check for timeout and fast-forward status
            let challenger_duration = await_tel!(
                context,
                proposal.fetch_current_challenger_duration(&proposer_provider)
            )
            .context("challenger_duration")?;
            let is_validity_proven = await_tel!(
                context,
                parent.fetch_is_successor_validity_proven(&proposer_provider)
            )
            .context("is_validity_proven")?;
            if !is_validity_proven && challenger_duration > 0 {
                info!("Waiting for {challenger_duration} more seconds before resolution.");
                break;
            }

            // Check if can prune next set of children in parent tournament
            if proposal.has_parent() {
                let can_resolve = loop {
                    let result = await_tel_res!(
                        context,
                        tracer,
                        "KailuaTournament::pruneChildren",
                        parent_contract
                            .pruneChildren(U256::from(ELIMINATIONS_LIMIT))
                            .call()
                            .into_future()
                    );

                    if let Err(err) = result {
                        // Pruning failure means unresolved disputes
                        debug!("pruneChildren: {err:?}");
                        break false;
                    };
                    let result = result.unwrap();

                    // Final prune will be during resolution
                    if !result._0.is_zero() {
                        break true;
                    }

                    // Prune next set of children
                    info!("Eliminating {ELIMINATIONS_LIMIT} opponents before resolution.");
                    match parent_contract
                        .pruneChildren(U256::from(ELIMINATIONS_LIMIT))
                        .transact_with_context(context.clone(), "KailuaTournament::pruneChildren")
                        .await
                        .context("KailuaTournament::pruneChildren transact")
                    {
                        Ok(receipt) => {
                            info!("KailuaTournament::pruneChildren: {} gas", receipt.gas_used);
                            meter_prune_num.add(
                                1,
                                &[
                                    KeyValue::new(
                                        "tournament",
                                        parent_contract.address().to_string(),
                                    ),
                                    KeyValue::new("txn_hash", receipt.transaction_hash.to_string()),
                                    KeyValue::new("txn_from", receipt.from.to_string()),
                                    KeyValue::new(
                                        "txn_to",
                                        receipt.to.unwrap_or_default().to_string(),
                                    ),
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
                        Err(err) => {
                            error!("KailuaTournament::pruneChildren: {err:?}");
                            meter_prune_fail.add(
                                1,
                                &[
                                    KeyValue::new(
                                        "tournament",
                                        parent_contract.address().to_string(),
                                    ),
                                    KeyValue::new("msg", err.to_string()),
                                ],
                            );
                            break false;
                        }
                    }
                };
                // Some disputes are still unresolved
                if !can_resolve {
                    info!("Waiting for more proofs to resolve proposer as survivor.");
                    break;
                }
            }

            // Check if claim won in tournament
            if !await_tel!(
                context,
                proposal.fetch_parent_tournament_survivor_status(&proposer_provider)
            )
            .unwrap_or_default()
            .unwrap_or_default()
            {
                error!(
                    "Failed to determine proposal at {} as successor of proposal at {}.",
                    proposal.contract, parent.contract
                );
                break;
            }

            // resolve
            info!(
                "Resolving game at index {} and height {}.",
                proposal.index, proposal.output_block_number
            );

            match proposal
                .resolve(&proposer_provider)
                .await
                .context("KailuaTournament::resolve transact")
            {
                Ok(receipt) => {
                    info!("KailuaTournament::resolve: {} gas", receipt.gas_used);
                    meter_resolve_num.add(
                        1,
                        &[
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
                    meter_resolve_last.record(
                        proposal.index,
                        &[
                            KeyValue::new("proposal", proposal.contract.to_string()),
                            KeyValue::new("l2_height", proposal.output_block_number.to_string()),
                        ],
                    );
                }
                Err(err) => {
                    error!("KailuaTournament::resolve: {err:?}");
                    meter_resolve_fail.add(
                        1,
                        &[
                            KeyValue::new("proposal", proposal.contract.to_string()),
                            KeyValue::new("l2_height", proposal.output_block_number.to_string()),
                            KeyValue::new("msg", err.to_string()),
                        ],
                    );
                    break;
                }
            }
        }

        // Check if deployment is still valid
        let latest_game_impl_addr = dispute_game_factory
            .gameImpls(KAILUA_GAME_TYPE)
            .stall_with_context(context.clone(), "DisputeGameFactory::gameImpls")
            .await
            .impl_;
        if latest_game_impl_addr != kailua_game_implementation_address {
            warn!("Not proposing. KailuaGame {kailua_game_implementation_address} outdated. Found new KailuaGame {latest_game_impl_addr}.");
            continue;
        }

        // Submit proposal to extend canonical chain
        let Some(canonical_tip) = kailua_db.canonical_tip() else {
            warn!("No canonical proposal chain to extend!");
            continue;
        };

        // Query op-node to get latest safe l2 head
        let sync_status = await_tel_res!(
            context,
            tracer,
            "sync_status",
            retry_with_context!(op_node_provider.sync_status())
        )?;
        debug!("sync_status[safe_l2] {:?}", &sync_status["safe_l2"]);
        let output_block_number = sync_status["safe_l2"]["number"].as_u64().unwrap();
        if output_block_number < canonical_tip.output_block_number {
            warn!(
                "op-node is still {} blocks behind latest canonical proposal.",
                canonical_tip.output_block_number - output_block_number
            );
            continue;
        } else if output_block_number - canonical_tip.output_block_number
            < kailua_db.config.blocks_per_proposal()
        {
            info!(
                "Waiting for safe l2 head to advance by {} more blocks before submitting proposal.",
                kailua_db.config.blocks_per_proposal()
                    - (output_block_number - canonical_tip.output_block_number)
            );
            continue;
        }
        info!(
            "Candidate proposal of {} blocks is available.",
            kailua_db.config.blocks_per_proposal()
        );
        // Wait for L1 timestamp to advance beyond the safety gap for proposals
        let proposed_block_number =
            canonical_tip.output_block_number + kailua_db.config.blocks_per_proposal();
        let chain_time = await_tel!(
            context,
            get_block(&proposer_provider, BlockNumberOrTag::Latest)
        )?
        .header()
        .timestamp();

        let min_proposal_time = kailua_db.config.min_proposal_time(proposed_block_number);
        if chain_time < min_proposal_time {
            let time_to_wait = min_proposal_time.saturating_sub(chain_time);
            info!("Waiting for {time_to_wait} more seconds of chain time for proposal gap.");
            continue;
        }

        // Wait for vanguard to make submission
        let vanguard = await_tel!(
            context,
            kailua_db.treasury.fetch_vanguard(&proposer_provider)
        );
        let vanguard_advantage_timeout =
            if canonical_tip.requires_vanguard_advantage(proposer_address, vanguard) {
                let vanguard_advantage = await_tel!(
                    context,
                    kailua_db
                        .treasury
                        .fetch_vanguard_advantage(&proposer_provider)
                );
                min_proposal_time + vanguard_advantage
            } else {
                min_proposal_time
            };
        if chain_time < vanguard_advantage_timeout {
            let time_to_wait = vanguard_advantage_timeout.saturating_sub(chain_time);
            warn!("Waiting for at most {time_to_wait} more seconds of chain time for vanguard.");
            continue;
        }

        // Prepare proposal
        let proposed_output_root = await_tel_res!(
            context,
            tracer,
            "proposed_output_root",
            retry_with_context!(op_node_provider.output_at_block(proposed_block_number))
        )?;
        // Prepare intermediate outputs
        let mut io_field_elements = vec![];
        for i in 1..kailua_db.config.proposal_output_count {
            let io_block_number =
                canonical_tip.output_block_number + i * kailua_db.config.output_block_span;
            let output_hash = await_tel_res!(
                context,
                tracer,
                "output_hash",
                retry_with_context!(op_node_provider.output_at_block(io_block_number))
            )?;
            io_field_elements.push(hash_to_fe(output_hash));
        }
        if io_field_elements.len() as u64 != kailua_db.config.proposal_output_count - 1 {
            error!("Could not gather all necessary intermediate outputs.");
            continue;
        }
        let sidecar = match Proposal::create_sidecar(&io_field_elements) {
            Ok(res) => res,
            Err(err) => {
                error!("Failed to create blob sidecar: {err:?}");
                continue;
            }
        };
        info!("Candidate proposal prepared");

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
                .stall_with_context(context.clone(), "DisputeGameFactory::games")
                .await
                .proxy_;
            if dupe_game_address.is_zero() {
                // proposal was not made before using this dupe counter
                info!("Dupe counter {dupe_counter} available.");
                break Some(extra_data);
            }
            // fetch proposal from local data
            let dupe_game_index: u64 = KailuaTournament::new(dupe_game_address, &proposer_provider)
                .gameIndex()
                .stall_with_context(context.clone(), "KailuaTournament::gameIndex")
                .await
                ._0
                .to();
            if dupe_game_index >= kailua_db.state.next_factory_index {
                // we need to fetch this proposal's data
                warn!("Duplicate proposal data not yet available.");
                break None;
            }
            if let Some(dupe_proposal) = kailua_db.get_local_proposal(&dupe_game_index) {
                // check if proposal was made incorrectly or by an already eliminated player
                if dupe_proposal.is_correct().unwrap_or_default()
                    && !kailua_db.was_proposer_eliminated_before(&dupe_proposal)
                {
                    info!("Correct proposal was already made honestly.");
                    break None;
                }
            };
            // this invalid proposal will not participate in the tournament
            warn!("Incrementing duplication counter");
            // increment counter
            dupe_counter += 1;
        };

        let Some(extra_data) = unique_extra_data else {
            // this proposal was already correctly made or we need more data
            continue;
        };
        // Check collateral requirements
        let bond_value = await_tel!(context, kailua_db.treasury.fetch_bond(&proposer_provider));
        let paid_in = await_tel!(
            context,
            kailua_db
                .treasury
                .fetch_balance(&proposer_provider, proposer_address)
        );
        let balance = await_tel_res!(
            context,
            tracer,
            "ReqwestProvider::get_balance",
            retry_with_context!(proposer_provider
                .get_balance(proposer_address)
                .into_future())
        )?;
        let owed_collateral = bond_value.saturating_sub(paid_in);
        if balance < owed_collateral {
            error!("INSUFFICIENT BALANCE! Need to lock in at least {owed_collateral} more.");
            continue;
        }
        // Submit proposal
        info!("Proposing output {proposed_output_root} at l2 block number {proposed_block_number} with {owed_collateral} additional collateral and duplication counter {dupe_counter}.");

        let treasury_contract_instance = kailua_db
            .treasury
            .treasury_contract_instance(&proposer_provider);
        let mut transaction =
            treasury_contract_instance.propose(proposed_output_root, Bytes::from(extra_data));
        if !owed_collateral.is_zero() {
            transaction = transaction.value(owed_collateral);
        }
        if !sidecar.blobs.is_empty() {
            transaction = transaction.sidecar(sidecar);
        }
        match transaction
            .transact_with_context(context.clone(), "KailuaTreasury::propose")
            .await
            .context("KailuaTreasury::propose")
        {
            Ok(receipt) => {
                info!("Proposal submitted: {:?}", receipt.transaction_hash);
                info!("KailuaTreasury::propose: {} gas", receipt.gas_used);
                meter_propose_num.add(
                    1,
                    &[
                        KeyValue::new("l2_height", proposed_block_number.to_string()),
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
                meter_propose_last.record(
                    proposed_block_number,
                    &[
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
                error!("Failed to confirm proposal txn: {e:?}");
                meter_propose_fail.add(
                    1,
                    &[
                        KeyValue::new("l2_height", proposed_block_number.to_string()),
                        KeyValue::new("msg", e.to_string()),
                    ],
                );
            }
        }
    }
}
