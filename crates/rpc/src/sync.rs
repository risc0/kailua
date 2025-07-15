// Copyright 2025 RISC Zero, Inc.
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

use crate::api::KailuaServerCache;
use crate::args::RpcArgs;
use anyhow::Context;
use kailua_contracts::*;
use kailua_sync::agent::{SyncAgent, FINAL_L2_BLOCK_RESOLVED};
use kailua_sync::stall::Stall;
use kailua_sync::{await_tel, KAILUA_GAME_TYPE};
use opentelemetry::global::tracer;
use opentelemetry::trace::FutureExt;
use opentelemetry::trace::{TraceContextExt, Tracer};
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{error, info, warn};

pub async fn handle_sync(
    args: RpcArgs,
    data_dir: PathBuf,
    server_cache: KailuaServerCache,
) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("handle_sync"));

    // todo: auto update deployment
    // check dgf for set implementation events
    // order by their game index

    // todo: init from beginning instead of last resolved game

    // initialize sync agent
    let mut agent = SyncAgent::new(
        &args.sync.provider,
        data_dir,
        args.sync.kailua_game_implementation,
        args.sync.kailua_anchor_address,
        args.bypass_chain_registry,
    )
    .await?;
    info!("KailuaTreasury({:?})", agent.deployment.treasury);

    // Check if deployment is still valid
    let dispute_game_factory =
        IDisputeGameFactory::new(agent.deployment.factory, &agent.provider.l1_provider);
    let latest_game_impl_addr = dispute_game_factory
        .gameImpls(KAILUA_GAME_TYPE)
        .stall_with_context(context.clone(), "DisputeGameFactory::gameImpls")
        .await;
    if latest_game_impl_addr != agent.deployment.game {
        warn!(
            "Deployment {} outdated. Found new deployment {latest_game_impl_addr}.",
            agent.deployment.game
        );
    }

    loop {
        // Wait for new data on every iteration
        sleep(Duration::from_secs(1)).await;
        // fetch latest games
        let loaded_proposals = match await_tel!(
            context,
            agent.sync(
                #[cfg(feature = "devnet")]
                args.sync.delay_l2_blocks,
                args.sync.final_l2_block
            )
        )
        .context("SyncAgent::sync")
        {
            Ok(result) => result,
            Err(err) => {
                if err
                    .root_cause()
                    .to_string()
                    .contains(FINAL_L2_BLOCK_RESOLVED)
                {
                    warn!("handle_proposals terminated");
                    return Ok(());
                }
                error!("Synchronization error: {err:?}");
                vec![]
            }
        };

        let mut new_entries = vec![];
        for proposal_index in loaded_proposals {
            // Load proposal from db
            let Some(proposal) = agent.proposals.get(&proposal_index) else {
                error!("Proposal {proposal_index} missing from database.");
                continue;
            };
            // Ignore non-canonical proposals that will never be resolved
            if !proposal.canonical.unwrap_or_default() {
                warn!(
                    "Ignoring non-canonical proposal {proposal_index} at {}",
                    proposal.contract
                );
                continue;
            }
            // Check termination condition
            if let Some(final_l2_block) = args.sync.final_l2_block {
                if proposal.output_block_number >= final_l2_block {
                    warn!(
                        "Dropping proposal {} with output height {} past final l2 block {}.",
                        proposal.index, proposal.output_block_number, final_l2_block
                    );
                    continue;
                }
            }
            // Print proposal index/address/height/output to stdout
            println!(
                "TRACKED\t{proposal_index}\t{}\t{}\t{}",
                proposal.contract, proposal.output_block_number, proposal.output_root
            );
            // Queue proposal for submission to rpc cache
            new_entries.push((proposal.output_block_number, proposal.contract));
        }

        // Send new entries to cache
        if !new_entries.is_empty() {
            server_cache.write().await.extend(new_entries);
        }
    }
}
