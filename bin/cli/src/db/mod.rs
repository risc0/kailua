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

pub mod config;
pub mod proposal;
pub mod treasury;

use crate::providers::beacon::BlobProvider;
use crate::providers::optimism::OpNodeProvider;
use crate::KAILUA_GAME_TYPE;
use alloy::network::Network;
use alloy::primitives::U256;
use alloy::providers::Provider;
use alloy::transports::Transport;
use anyhow::Context;
use config::Config;
use kailua_contracts::IAnchorStateRegistry::IAnchorStateRegistryInstance;
use kailua_contracts::IDisputeGameFactory::gameAtIndexReturn;
use kailua_contracts::{IDisputeGameFactory, KailuaGame, KailuaTreasury};
use proposal::Proposal;
use std::collections::HashMap;
use tracing::{info, warn};
use treasury::Treasury;

#[derive(Clone, Debug, Default)]
pub enum ProofStatus {
    #[default]
    NONE,
    ULoseVLose,
    ULoseVWin,
    UWinVLose,
}

#[derive(Clone, Debug, Default)]
pub struct KailuaDB {
    pub config: Config,
    pub treasury: Treasury,
    pub proposals: HashMap<u64, Proposal>,
    pub last_factory_index: u64,
    pub canonical_tip_index: u64,
}

impl KailuaDB {
    pub async fn init<T: Transport + Clone, P: Provider<T, N>, N: Network>(
        anchor_state_registry: &IAnchorStateRegistryInstance<T, P, N>,
    ) -> anyhow::Result<Self> {
        let dispute_game_factory = IDisputeGameFactory::new(
            anchor_state_registry.disputeGameFactory().call().await?._0,
            anchor_state_registry.provider(),
        );
        let game_implementation = KailuaGame::new(
            dispute_game_factory
                .gameImpls(KAILUA_GAME_TYPE)
                .call()
                .await?
                .impl_,
            anchor_state_registry.provider(),
        );
        let config = Config::load(&game_implementation).await?;
        let treasury_instance =
            KailuaTreasury::new(config.treasury, anchor_state_registry.provider());
        let treasury = Treasury::init(&treasury_instance).await?;
        let treasury_index = treasury.index;
        let treasury_proposal = Proposal::load_treasury(&config, &treasury, &treasury_instance)
            .await
            .context("load_treasury")?;
        let proposals = [(treasury_index, treasury_proposal)].into_iter().collect();
        Ok(Self {
            config,
            treasury,
            proposals,
            last_factory_index: treasury_index,
            canonical_tip_index: treasury_index,
        })
    }

    pub async fn load_proposals<T: Transport + Clone, P: Provider<T, N>, N: Network>(
        &mut self,
        anchor_state_registry: &IAnchorStateRegistryInstance<T, P, N>,
        op_node_provider: &OpNodeProvider,
        blob_provider: &BlobProvider,
    ) -> anyhow::Result<usize> {
        let dispute_game_factory = IDisputeGameFactory::new(
            anchor_state_registry.disputeGameFactory().call().await?._0,
            anchor_state_registry.provider(),
        );
        let initial_proposals = self.proposals.len();
        let game_count: u64 = dispute_game_factory
            .gameCount()
            .call()
            .await?
            .gameCount_
            .to();
        for factory_index in (self.last_factory_index + 1)..game_count {
            let gameAtIndexReturn {
                gameType_: game_type,
                proxy_: game_address,
                ..
            } = dispute_game_factory
                .gameAtIndex(U256::from(factory_index))
                .call()
                .await
                .context(format!("gameAtIndex {factory_index}/{game_count}"))?;
            // skip entries for other game types
            if game_type != KAILUA_GAME_TYPE {
                continue;
            }
            info!("Processing proposal at factory index {factory_index}");
            let game_instance = KailuaGame::new(game_address, anchor_state_registry.provider());
            let mut proposal =
                Proposal::load_game(&self.config, &game_instance, blob_provider).await?;
            let is_parent_correct = if proposal.index == proposal.parent {
                true
            } else {
                self.proposals
                    .get(&proposal.parent)
                    .expect("Attempted to process child before registering parent.")
                    .is_correct()
                    .expect("Attempted to process child before deciding parent correctness")
            };
            info!("Assessing proposal correctness..");
            let is_correct_proposal = match proposal
                .assess_correctness(&self.config, op_node_provider, is_parent_correct)
                .await?
            {
                None => {
                    warn!("Failed to assess correctness. Is op-node synced far enough?");
                    break;
                }
                Some(correctness) => {
                    info!("Assessed proposal as {correctness}.");
                    correctness
                }
            };
            // Append child to parent
            if proposal.parent != proposal.index {
                let parent = self.proposals.get_mut(&proposal.parent).unwrap();
                if parent.children.last().is_none()
                    || parent.children.last().unwrap() < &proposal.index
                {
                    parent.children.push(proposal.index);
                }
            }
            // Update canonical chain tip
            if is_correct_proposal && proposal.output_block_number > self.canonical_tip_height() {
                info!(
                    "Updating canonical proposal chain tip to game at index {}.",
                    proposal.index
                );
                self.canonical_tip_index = proposal.index;
            }
            // Insert proposal in db
            self.proposals.insert(proposal.index, proposal);
            // Update last processed game
            self.last_factory_index = factory_index;
        }

        Ok(self.proposals.len() - initial_proposals)
    }

    pub fn canonical_tip(&self) -> &Proposal {
        self.proposals.get(&self.canonical_tip_index).unwrap()
    }
    pub fn canonical_tip_height(&self) -> u64 {
        self.proposals
            .get(&self.canonical_tip_index)
            .unwrap()
            .output_block_number
    }

    pub async fn unresolved_canonical_proposals<
        T: Transport + Clone,
        P: Provider<T, N>,
        N: Network,
    >(
        &mut self,
        l1_node_provider: &P,
    ) -> anyhow::Result<Vec<u64>> {
        let mut unresolved_proposal_indices = vec![self.canonical_tip_index];
        // traverse up tree
        loop {
            let proposal_index = *unresolved_proposal_indices.last().unwrap();
            let proposal = self.proposals.get_mut(&proposal_index).unwrap();
            // Update on-chain resolution status
            proposal.fetch_finality(l1_node_provider).await?;
            // break if we reach a resolved game or a setup game
            if proposal.finality.is_some() {
                unresolved_proposal_indices.pop();
                break;
            } else if proposal.parent == proposal_index {
                // this is an unresolved treasury, keep in stack
                break;
            }
            unresolved_proposal_indices.push(proposal.parent);
        }
        Ok(unresolved_proposal_indices)
    }
}
