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

use crate::sync::agent::SyncAgent;
use crate::sync::proposal::{Proposal, ELIMINATIONS_LIMIT};
use crate::transact::provider::SafeProvider;
use crate::transact::rpc::get_block;
use crate::transact::signer::ProposerSignerArgs;
use crate::transact::{Transact, TransactArgs};
use crate::{retry_res_ctx_timeout, stall::Stall, CoreArgs, KAILUA_GAME_TYPE};
use alloy::consensus::BlockHeader;
use alloy::eips::BlockNumberOrTag;
use alloy::network::{BlockResponse, Ethereum, TxSigner};
use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::Provider;
use alloy::sol_types::SolValue;
use anyhow::{bail, Context};
use kailua_client::args::parse_address;
use kailua_client::telemetry::TelemetryArgs;
use kailua_client::{await_tel, await_tel_res};
use kailua_common::blobs::hash_to_fe;
use kailua_contracts::*;
use opentelemetry::global::{meter, tracer};
use opentelemetry::metrics::{Counter, Gauge};
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use opentelemetry::KeyValue;
use std::future::IntoFuture;
use std::path::PathBuf;
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
    /// Transaction publication configuration
    #[clap(flatten)]
    pub txn_args: TransactArgs,
    /// Address of the KailuaGame implementation to use
    #[clap(long, env, value_parser = parse_address)]
    pub kailua_game_implementation: Option<Address>,
    /// Address of the anchor proposal to start synchronization from
    #[clap(long, env, value_parser = parse_address)]
    pub kailua_anchor_address: Option<Address>,

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
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("propose"));

    // initialize sync agent
    let mut agent = SyncAgent::new(
        &args.core,
        data_dir,
        args.kailua_game_implementation,
        args.kailua_anchor_address,
    )
    .await?;
    info!("KailuaTreasury({:?})", agent.deployment.treasury);

    // initialize proposer wallet
    info!("Initializing proposer wallet.");
    let proposer_wallet = await_tel_res!(
        context,
        tracer,
        "ProposerSignerArgs::wallet",
        args.proposer_signer.wallet(Some(agent.config.l1_chain_id))
    )?;
    let proposer_address = proposer_wallet.default_signer().address();
    let proposer_provider = SafeProvider::new(
        args.txn_args
            .premium_provider::<Ethereum>()
            .wallet(&proposer_wallet)
            .connect_http(args.core.eth_rpc_url.as_str().try_into()?),
    );
    info!("Proposer address: {proposer_address}");

    // Run the proposer loop to sync and post
    info!(
        "Starting from proposal at factory index {}",
        agent.cursor.next_factory_index
    );

    let mut prioritize_proposing = false;
    loop {
        // Wait for new data on every iteration
        sleep(Duration::from_secs(1)).await;
        // fetch latest games
        if let Err(err) = await_tel!(context, agent.sync()).context("SyncAgent::sync") {
            error!("Synchronization error: {err:?}");
        }

        // alert on honesty compromise
        if let Some(elimination_index) = agent.eliminations.get(&proposer_address) {
            error!(
                "Proposer {proposer_address} honesty compromised at proposal {elimination_index}."
            );
            meter_propose_fault.record(
                *elimination_index,
                &[
                    KeyValue::new("treasury", agent.deployment.treasury.to_string()),
                    KeyValue::new("proposer", proposer_address.to_string()),
                ],
            );
        }

        // Resolve one proposal per iteration
        if !prioritize_proposing {
            if let Err(err) = await_tel_res!(
                context,
                tracer,
                "resolve_next_pending_proposal",
                resolve_next_pending_proposal(
                    &agent,
                    &args.txn_args,
                    &proposer_provider,
                    &meter_prune_num,
                    &meter_prune_fail,
                    &meter_resolve_num,
                    &meter_resolve_fail,
                    &meter_resolve_last
                )
            ) {
                error!("Failed to resolve proposal: {err:?}");
            }
        } else {
            warn!("Skipping resolving to prioritize proposing.");
        }

        // Reset priority
        prioritize_proposing = false;

        // Check if deployment is still valid
        let dispute_game_factory =
            IDisputeGameFactory::new(agent.deployment.factory, &agent.provider.l1_provider);
        let latest_game_impl_addr = dispute_game_factory
            .gameImpls(KAILUA_GAME_TYPE)
            .stall_with_context(context.clone(), "DisputeGameFactory::gameImpls")
            .await;
        if latest_game_impl_addr != agent.deployment.game {
            warn!("Not proposing. Implementation {} outdated. Found new implementation {latest_game_impl_addr}.", agent.deployment.game);
            continue;
        }

        // Submit proposal to extend canonical chain
        let Some(canonical_tip) = agent.canonical_tip() else {
            bail!("Canonical tip proposal missing from database!");
        };

        // Query op-node to get latest safe l2 head
        let sync_status = await_tel!(
            context,
            tracer,
            "sync_status",
            retry_res_ctx_timeout!(agent.provider.op_provider.sync_status().await)
        );
        debug!("sync_status[safe_l2] {:?}", &sync_status["safe_l2"]);
        let output_block_number = sync_status["safe_l2"]["number"].as_u64().unwrap();
        let proposal_block_number =
            canonical_tip.output_block_number + agent.deployment.blocks_per_proposal();
        if output_block_number < canonical_tip.output_block_number {
            warn!(
                "op-node is still {} blocks behind latest canonical proposal.",
                canonical_tip.output_block_number - output_block_number
            );
            continue;
        } else if output_block_number < proposal_block_number {
            info!(
                "Waiting for op-node safe l2 head to reach block {proposal_block_number} before proposing ({} more blocks needed).",
                proposal_block_number - output_block_number
            );
            continue;
        }
        info!(
            "Candidate proposal of {} blocks is available.",
            agent.deployment.blocks_per_proposal()
        );
        // Wait for L1 timestamp to advance beyond the safety gap for proposals
        let proposed_block_number =
            canonical_tip.output_block_number + agent.deployment.blocks_per_proposal();
        let chain_time = await_tel!(
            context,
            get_block(&agent.provider.l1_provider, BlockNumberOrTag::Latest)
        )?
        .header()
        .timestamp();

        let min_proposal_time = agent.deployment.min_proposal_time(proposed_block_number);
        if chain_time < min_proposal_time {
            let time_to_wait = min_proposal_time.saturating_sub(chain_time);
            info!("Waiting for {time_to_wait} more seconds of chain time for proposal gap.");
            continue;
        }

        // Wait for vanguard to make submission
        let vanguard = await_tel!(context, fetch_vanguard(&agent));
        let vanguard_advantage_timeout =
            if canonical_tip.requires_vanguard_advantage(proposer_address, vanguard) {
                let vanguard_advantage = await_tel!(context, fetch_vanguard_advantage(&agent));
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
        let proposed_output_root = await_tel!(
            context,
            tracer,
            "proposed_output_root",
            retry_res_ctx_timeout!(
                agent
                    .provider
                    .op_provider
                    .output_at_block(proposed_block_number)
                    .await
            )
        );
        // Prepare intermediate outputs
        let mut io_field_elements = vec![];
        for i in 1..agent.deployment.proposal_output_count {
            let io_block_number =
                canonical_tip.output_block_number + i * agent.deployment.output_block_span;
            let output_hash = await_tel!(
                context,
                tracer,
                "output_hash",
                retry_res_ctx_timeout!(
                    agent
                        .provider
                        .op_provider
                        .output_at_block(io_block_number)
                        .await
                )
            );
            io_field_elements.push(hash_to_fe(output_hash));
        }
        if io_field_elements.len() as u64 != agent.deployment.proposal_output_count - 1 {
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
            let dupe_game_index: u64 =
                KailuaTournament::new(dupe_game_address, &agent.provider.l1_provider)
                    .gameIndex()
                    .stall_with_context(context.clone(), "KailuaTournament::gameIndex")
                    .await
                    .to();
            if dupe_game_index >= agent.cursor.next_factory_index {
                // we need to fetch this proposal's data
                warn!("Duplicate proposal data not yet available.");
                break None;
            }
            if let Some(dupe_proposal) = agent.proposals.get(&dupe_game_index) {
                // check if proposal was made incorrectly or by an already eliminated player
                if dupe_proposal.is_correct().unwrap_or_default()
                    && !agent.was_proposer_eliminated_before(dupe_proposal)
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
            warn!("Skipping proposal attempt.");
            continue;
        };
        // Check collateral requirements
        let bond_value = await_tel!(context, fetch_participation_bond(&agent));
        let paid_in = await_tel!(context, fetch_paid_bond(&agent, proposer_address));
        let balance = await_tel!(
            context,
            tracer,
            "ReqwestProvider::get_balance",
            retry_res_ctx_timeout!(
                agent
                    .provider
                    .l1_provider
                    .get_balance(proposer_address)
                    .await
            )
        );
        let owed_collateral = bond_value.saturating_sub(paid_in);
        if balance < owed_collateral {
            error!("INSUFFICIENT BALANCE! Need to lock in at least {owed_collateral} more.");
            continue;
        }
        // Submit proposal
        info!("Proposing output {proposed_output_root} at l2 block number {proposed_block_number} with {owed_collateral} additional collateral and duplication counter {dupe_counter}.");

        let treasury_contract_instance =
            KailuaTreasury::new(agent.deployment.treasury, &proposer_provider);
        let mut transaction =
            treasury_contract_instance.propose(proposed_output_root, Bytes::from(extra_data));
        if !owed_collateral.is_zero() {
            transaction = transaction.value(owed_collateral);
        }
        if !sidecar.blobs.is_empty() {
            transaction = transaction.sidecar(sidecar);
        }
        match transaction
            .timed_transact_with_context(
                context.clone(),
                "KailuaTreasury::propose",
                Some(Duration::from_secs(args.txn_args.txn_timeout)),
            )
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
                // Skip resolve transactions on next iteration
                prioritize_proposing = true;
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

#[allow(clippy::too_many_arguments)]
pub async fn resolve_next_pending_proposal<P: Provider>(
    agent: &SyncAgent,
    txn_args: &TransactArgs,
    proposer_provider: P,
    meter_prune_num: &Counter<u64>,
    meter_prune_fail: &Counter<u64>,
    meter_resolve_num: &Counter<u64>,
    meter_resolve_fail: &Counter<u64>,
    meter_resolve_last: &Gauge<u64>,
) -> anyhow::Result<bool> {
    let tracer = tracer("kailua");
    let context =
        opentelemetry::Context::current_with_span(tracer.start("resolve_next_pending_proposal"));

    let Some(proposal_index) =
        unresolved_canonical_proposal(agent).context("unresolved_canonical_proposals")?
    else {
        return Ok(false);
    };

    let Some(proposal) = agent.proposals.get(&proposal_index) else {
        bail!("Unresolved proposal {proposal_index} missing from database.");
    };
    let Some(parent) = agent.proposals.get(&proposal.parent) else {
        bail!(
            "Unresolved proposal {proposal_index} parent {} missing from database.",
            proposal.parent
        );
    };
    let parent_contract = parent.tournament_contract_instance(&agent.provider.l1_provider);

    // Skip resolved games
    if await_tel!(
        context,
        proposal.fetch_finality(&agent.provider.l1_provider)
    )
    .context("Proposal::fetch_finality")?
    .unwrap_or_default()
    {
        return Ok(false);
    }

    // Check for timeout and fast-forward status
    let challenger_duration = await_tel!(
        context,
        proposal.fetch_current_challenger_duration(&agent.provider.l1_provider)
    )
    .context("challenger_duration")?;
    let is_validity_proven = await_tel!(
        context,
        parent.fetch_is_successor_validity_proven(&agent.provider.l1_provider)
    )
    .context("is_validity_proven")?;
    if !is_validity_proven && challenger_duration > 0 {
        info!("Waiting for {challenger_duration} more seconds of chain time before resolution of proposal {proposal_index}.");
        return Ok(false);
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
            if !result.0.is_zero() {
                break true;
            }

            // Prune next set of children
            info!("Eliminating {ELIMINATIONS_LIMIT} opponents before resolution.");
            match parent_contract
                .pruneChildren(U256::from(ELIMINATIONS_LIMIT))
                .timed_transact_with_context(
                    context.clone(),
                    "KailuaTournament::pruneChildren",
                    Some(Duration::from_secs(txn_args.txn_timeout)),
                )
                .await
                .context("KailuaTournament::pruneChildren transact")
            {
                Ok(receipt) => {
                    info!("KailuaTournament::pruneChildren: {} gas", receipt.gas_used);
                    meter_prune_num.add(
                        1,
                        &[
                            KeyValue::new("tournament", parent_contract.address().to_string()),
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
                Err(err) => {
                    error!("KailuaTournament::pruneChildren: {err:?}");
                    meter_prune_fail.add(
                        1,
                        &[
                            KeyValue::new("tournament", parent_contract.address().to_string()),
                            KeyValue::new("msg", err.to_string()),
                        ],
                    );
                    break false;
                }
            }
        };
        // Some disputes are still unresolved
        if !can_resolve {
            info!("Waiting for more proofs to resolve proposal.");
            return Ok(false);
        }
    }

    // Check if claim won in tournament
    if !await_tel!(
        context,
        proposal.fetch_parent_tournament_survivor_status(&agent.provider.l1_provider)
    )
    .unwrap_or_default()
    .unwrap_or_default()
    {
        error!(
            "Failed to determine proposal at {} as successor of proposal at {}.",
            proposal.contract, parent.contract
        );
        return Ok(false);
    }

    // resolve
    info!(
        "Resolving game at index {} and height {}.",
        proposal.index, proposal.output_block_number
    );

    match proposal
        .resolve(&proposer_provider, txn_args)
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
            Ok(true)
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
            Ok(false)
        }
    }
}

pub fn unresolved_canonical_proposal(agent: &SyncAgent) -> anyhow::Result<Option<u64>> {
    // Load last resolved proposal
    let Some(last_resolved_proposal) = agent.proposals.get(&agent.cursor.last_resolved_game) else {
        bail!(
            "Last resolved proposal {} missing from database.",
            agent.cursor.last_resolved_game
        );
    };
    // Return successor
    Ok(last_resolved_proposal.successor)
}

pub async fn fetch_vanguard(agent: &SyncAgent) -> Address {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("fetch_vanguard"));
    KailuaTreasury::new(agent.deployment.treasury, &agent.provider.l1_provider)
        .vanguard()
        .stall_with_context(context.clone(), "KailuaTreasury::vanguard")
        .await
}

pub async fn fetch_vanguard_advantage(agent: &SyncAgent) -> u64 {
    let tracer = tracer("kailua");
    let context =
        opentelemetry::Context::current_with_span(tracer.start("fetch_vanguard_advantage"));
    KailuaTreasury::new(agent.deployment.treasury, &agent.provider.l1_provider)
        .vanguardAdvantage()
        .stall_with_context(context.clone(), "KailuaTreasury::vanguardAdvantage")
        .await
}

pub async fn fetch_participation_bond(agent: &SyncAgent) -> U256 {
    let tracer = tracer("kailua");
    let context =
        opentelemetry::Context::current_with_span(tracer.start("fetch_participation_bond"));
    KailuaTreasury::new(agent.deployment.treasury, &agent.provider.l1_provider)
        .participationBond()
        .stall_with_context(context.clone(), "KailuaTreasury::participationBond")
        .await
}

pub async fn fetch_paid_bond(agent: &SyncAgent, address: Address) -> U256 {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("fetch_paid_bond"));
    KailuaTreasury::new(agent.deployment.treasury, &agent.provider.l1_provider)
        .paidBonds(address)
        .stall_with_context(context.clone(), "KailuaTreasury::paidBonds")
        .await
}
