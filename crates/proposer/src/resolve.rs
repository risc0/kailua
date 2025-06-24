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

use crate::fetch::fetch_current_challenger_duration;
use alloy::network::{Network, ReceiptResponse};
use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use anyhow::{bail, Context};
use kailua_contracts::*;
use kailua_sync::agent::SyncAgent;
use kailua_sync::proposal::{Proposal, ELIMINATIONS_LIMIT};
use kailua_sync::stall::Stall;
use kailua_sync::transact::{Transact, TransactArgs};
use kailua_sync::{await_tel, await_tel_res};
use opentelemetry::global::tracer;
use opentelemetry::metrics::{Counter, Gauge};
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use opentelemetry::KeyValue;
use std::future::IntoFuture;
use std::time::Duration;
use tracing::{debug, error, info};

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

    let Some(resolved_parent) = agent.proposals.get(&agent.cursor.last_resolved_game) else {
        bail!(
            "Last resolved proposal {} missing from database.",
            agent.cursor.last_resolved_game
        );
    };

    let Some(unresolved_successor_index) = resolved_parent.successor else {
        return Ok(false);
    };

    let Some(unresolved_successor) = agent.proposals.get(&unresolved_successor_index) else {
        bail!("Unresolved successor {unresolved_successor_index} missing from database.");
    };

    let resolved_parent_contract =
        resolved_parent.tournament_contract_instance(&agent.provider.l1_provider);

    // Skip resolved games
    if await_tel!(
        context,
        unresolved_successor.fetch_finality(&agent.provider.l1_provider)
    )
    .context("Proposal::fetch_finality")?
    .unwrap_or_default()
    {
        return Ok(false);
    }

    // Check for timeout and fast-forward status
    let challenger_duration = await_tel!(
        context,
        fetch_current_challenger_duration(agent, unresolved_successor)
    );
    let is_validity_proven = await_tel!(
        context,
        resolved_parent.fetch_is_successor_validity_proven(&agent.provider.l1_provider)
    )
    .context("is_validity_proven")?;
    if !is_validity_proven && challenger_duration > 0 {
        info!("Waiting for {challenger_duration} more seconds of chain time before resolution of proposal {unresolved_successor_index}.");
        return Ok(false);
    }

    // Check if can prune next set of children in parent tournament
    if unresolved_successor.has_parent() {
        let can_resolve = loop {
            let result = await_tel_res!(
                context,
                tracer,
                "KailuaTournament::pruneChildren",
                resolved_parent_contract
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
            match resolved_parent_contract
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
                            KeyValue::new(
                                "tournament",
                                resolved_parent_contract.address().to_string(),
                            ),
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
                            KeyValue::new(
                                "tournament",
                                resolved_parent_contract.address().to_string(),
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
            info!("Waiting for more proofs to resolve proposal.");
            return Ok(false);
        }
    }

    // Check if claim won in tournament
    if !await_tel!(
        context,
        unresolved_successor.fetch_parent_tournament_survivor_status(&agent.provider.l1_provider)
    )
    .unwrap_or_default()
    .unwrap_or_default()
    {
        error!(
            "Failed to determine proposal at {} as successor of proposal at {}.",
            unresolved_successor.contract, resolved_parent.contract
        );
        return Ok(false);
    }

    // resolve
    info!(
        "Resolving game at index {} and height {}.",
        unresolved_successor.index, unresolved_successor.output_block_number
    );

    match resolve_proposal(unresolved_successor, &proposer_provider, txn_args)
        .await
        .context("KailuaTournament::resolve transact")
    {
        Ok(receipt) => {
            info!("KailuaTournament::resolve: {} gas", receipt.gas_used);
            meter_resolve_num.add(
                1,
                &[
                    KeyValue::new("proposal", unresolved_successor.contract.to_string()),
                    KeyValue::new(
                        "l2_height",
                        unresolved_successor.output_block_number.to_string(),
                    ),
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
                unresolved_successor.index,
                &[
                    KeyValue::new("proposal", unresolved_successor.contract.to_string()),
                    KeyValue::new(
                        "l2_height",
                        unresolved_successor.output_block_number.to_string(),
                    ),
                ],
            );
            Ok(true)
        }
        Err(err) => {
            error!("KailuaTournament::resolve: {err:?}");
            meter_resolve_fail.add(
                1,
                &[
                    KeyValue::new("proposal", unresolved_successor.contract.to_string()),
                    KeyValue::new(
                        "l2_height",
                        unresolved_successor.output_block_number.to_string(),
                    ),
                    KeyValue::new("msg", err.to_string()),
                ],
            );
            Ok(false)
        }
    }
}

pub async fn resolve_proposal<P: Provider<N>, N: Network>(
    proposal: &Proposal,
    provider: P,
    txn_args: &TransactArgs,
) -> anyhow::Result<N::ReceiptResponse> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("Proposal::resolve"));

    let contract_instance = proposal.tournament_contract_instance(&provider);
    let parent_tournament: Address = contract_instance
        .parentGame()
        .stall_with_context(context.clone(), "KailuaTournament::parentGame")
        .await;
    let parent_tournament_instance = KailuaTournament::new(parent_tournament, &provider);

    // Issue any necessary pre-emptive pruning calls
    loop {
        // check if calling pruneChildren doesn't fail
        let survivor = await_tel_res!(
            context,
            tracer,
            "KailuaTournament::pruneChildren",
            parent_tournament_instance
                .pruneChildren(U256::from(ELIMINATIONS_LIMIT))
                .call()
                .into_future()
        )?
        .0;

        // If a survivor is returned we don't need pruning
        if !survivor.is_zero() {
            break;
        }

        info!("Eliminating {ELIMINATIONS_LIMIT} opponents before resolution.");
        let receipt = parent_tournament_instance
            .pruneChildren(U256::from(ELIMINATIONS_LIMIT))
            .timed_transact_with_context(
                context.clone(),
                "KailuaTournament::pruneChildren",
                Some(Duration::from_secs(txn_args.txn_timeout)),
            )
            .await
            .context("KailuaTournament::pruneChildren")?;
        info!(
            "KailuaTournament::pruneChildren: {} gas",
            receipt.gas_used()
        );
    }

    // Issue resolution call
    let receipt = contract_instance
        .resolve()
        .timed_transact_with_context(
            context.clone(),
            "KailuaTournament::resolve",
            Some(Duration::from_secs(txn_args.txn_timeout)),
        )
        .await
        .context("KailuaTournament::resolve")?;
    info!("KailuaTournament::resolve: {} gas", receipt.gas_used());

    Ok(receipt)
}
