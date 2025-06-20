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

use crate::stall::Stall;
use crate::sync::deployment::SyncDeployment;
use crate::sync::proposal::Proposal;
use crate::sync::provider::SyncProvider;
use alloy::primitives::Address;
use anyhow::bail;
use kailua_contracts::*;
use opentelemetry::global::tracer;
use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::Context;
use std::collections::VecDeque;

/// A collection of pointers to statefully track synchrony information
pub struct SyncCursor {
    /// Factory game index of the latest canonical proposal
    pub canonical_proposal_tip: u64,
    /// Index of the next proposal to query
    pub next_factory_index: u64,
    /// Queue of proposal indices whose processing is delayed
    pub delayed_factory_indices: VecDeque<u64>,
    /// Index of the last L2 block height whose output is known
    pub last_output_index: u64,
    /// Index of the last proposal resolved on chain
    pub last_resolved_game: u64,
}

impl SyncCursor {
    pub fn has_next(&self, game_count: u64) -> bool {
        !self.delayed_factory_indices.is_empty() || self.next_factory_index < game_count
    }

    pub fn next_index(&mut self) -> u64 {
        self.delayed_factory_indices
            .pop_front()
            .unwrap_or(self.next_factory_index)
    }

    pub fn load_delayed_indices(&mut self, new_indices: impl IntoIterator<Item = u64>) {
        let existing_indices = core::mem::take(&mut self.delayed_factory_indices).into_iter();
        self.delayed_factory_indices = new_indices.into_iter().chain(existing_indices).collect();
    }

    pub async fn load(
        deployment: &SyncDeployment,
        provider: &SyncProvider,
        starting_proposal: Option<Address>,
    ) -> anyhow::Result<Self> {
        let tracer = tracer("kailua");
        let context = Context::current_with_span(tracer.start("SyncCursor::load"));

        let anchor_address = match starting_proposal {
            Some(address) => address,
            None => {
                KailuaTreasury::new(deployment.treasury, &provider.l1_provider)
                    .lastResolved()
                    .stall_with_context(context.clone(), "KailuaTreasury::lastResolved")
                    .await
            }
        };

        if anchor_address.is_zero() {
            bail!("No resolved games found. Deployment has not been fully configured.");
        }

        let anchor = KailuaTournament::new(anchor_address, &provider.l1_provider);

        let anchor_treasury = anchor
            .KAILUA_TREASURY()
            .stall_with_context(context.clone(), "KailuaTournament::KAILUA_TREASURY")
            .await;
        if anchor_treasury != deployment.treasury {
            bail!("Anchor is not part of the correct deployment.");
        }

        let anchor_index: u64 = anchor
            .gameIndex()
            .stall_with_context(context.clone(), "KailuaTournament::gameIndex")
            .await
            .to();

        let Some(true) = Proposal::parse_finality(
            anchor
                .status()
                .stall_with_context(context.clone(), "KailuaTournament::status")
                .await,
        )?
        else {
            bail!("Anchor game is not finalized.");
        };

        let anchor_block_height: u64 = anchor
            .l2BlockNumber()
            .stall_with_context(context.clone(), "KailuaTournament::l2BlockNumber")
            .await
            .to();

        let parent_address = anchor
            .parentGame()
            .stall_with_context(context.clone(), "KailuaTournament::parentGame")
            .await;

        let last_output_index = if parent_address == anchor_address {
            // get block height of treasury instance
            anchor_block_height
        } else {
            // get starting block height of game instance
            anchor_block_height - deployment.proposal_output_count * deployment.output_block_span
        };

        Ok(SyncCursor {
            canonical_proposal_tip: anchor_index,
            next_factory_index: anchor_index,
            delayed_factory_indices: VecDeque::new(),
            last_output_index,
            last_resolved_game: anchor_index,
        })
    }
}
