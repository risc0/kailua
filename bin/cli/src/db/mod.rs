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
use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use alloy::transports::Transport;
use anyhow::Context;
use config::Config;
use kailua_contracts::IAnchorStateRegistry::IAnchorStateRegistryInstance;
use kailua_contracts::IDisputeGameFactory::{gameAtIndexReturn, IDisputeGameFactoryInstance};
use kailua_contracts::KailuaGame::KailuaGameInstance;
use kailua_contracts::KailuaTournament::KailuaTournamentInstance;
use kailua_contracts::KailuaTreasury::KailuaTreasuryInstance;
use proposal::Proposal;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use tracing::{error, info, warn};
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
    pub eliminations: HashMap<Address, u64>,
    pub next_factory_index: u64,
    pub canonical_tip_index: Option<u64>,
}

impl KailuaDB {
    pub async fn init<T: Transport + Clone, P: Provider<T, N>, N: Network>(
        anchor_state_registry: &IAnchorStateRegistryInstance<T, P, N>,
    ) -> anyhow::Result<Self> {
        let dispute_game_factory = IDisputeGameFactoryInstance::new(
            anchor_state_registry.disputeGameFactory().call().await?._0,
            anchor_state_registry.provider(),
        );
        let game_implementation = KailuaGameInstance::new(
            dispute_game_factory
                .gameImpls(KAILUA_GAME_TYPE)
                .call()
                .await?
                .impl_,
            anchor_state_registry.provider(),
        );
        let config = Config::load(&game_implementation).await?;
        let treasury_implementation =
            KailuaTreasuryInstance::new(config.treasury, anchor_state_registry.provider());
        let treasury = Treasury::init(&treasury_implementation).await?;
        Ok(Self {
            config,
            treasury,
            ..Default::default()
        })
    }

    pub async fn load_proposals<T: Transport + Clone, P: Provider<T, N>, N: Network>(
        &mut self,
        anchor_state_registry: &IAnchorStateRegistryInstance<T, P, N>,
        op_node_provider: &OpNodeProvider,
        blob_provider: &BlobProvider,
    ) -> anyhow::Result<usize> {
        let dispute_game_factory = IDisputeGameFactoryInstance::new(
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
        while self.next_factory_index < game_count {
            let factory_index = self.next_factory_index;
            // process game
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
                info!("Skipping proposal of different game type {game_type} at factory index {factory_index}");
                self.next_factory_index += 1;
                continue;
            }
            info!("Processing tournament proposal at factory index {factory_index}");
            let tournament_instance =
                KailuaTournamentInstance::new(game_address, anchor_state_registry.provider());
            let mut proposal =
                Proposal::load(&self.config, blob_provider, &tournament_instance).await?;
            let is_correct_proposal = if proposal.has_parent() {
                // Validate game instance data
                info!("Assessing proposal correctness..");
                let is_parent_correct = self
                    .proposals
                    .get(&proposal.parent)
                    .expect("Attempted to process child before registering parent.")
                    .is_correct()
                    .expect("Attempted to process child before deciding parent correctness");
                let is_correct_proposal = match proposal
                    .assess_correctness(&self.config, op_node_provider, is_parent_correct)
                    .await?
                {
                    None => {
                        error!("Failed to assess correctness. Is op-node synced far enough?");
                        break;
                    }
                    Some(correct) => {
                        if correct {
                            info!("Assessed proposal as {correct}.");
                        } else {
                            warn!("Assessed proposal as {correct}.");
                        }
                        correct
                    }
                };
                // Append child to parent
                let parent = self.proposals.get_mut(&proposal.parent).unwrap();
                if parent.children.last().is_none()
                    || parent.children.last().unwrap() < &proposal.index
                {
                    parent.children.push(proposal.index);
                }
                is_correct_proposal
            } else {
                // Accept treasury instance data
                info!("Accepting initial treasury proposal as true.");
                true
            };

            // todo: take into account avoidance of unfinalizeable repeated proposals
            if is_correct_proposal
                && !self.was_proposer_eliminated_before(proposal.proposer, proposal.index)
            {
                // Consider updating canonical chain tip
                let canonical_tip_height = self.canonical_tip_height();
                if canonical_tip_height.is_none()
                    || canonical_tip_height.unwrap() < proposal.output_block_number
                {
                    info!(
                        "Updating canonical proposal chain tip to game at index {}.",
                        proposal.index
                    );
                    self.canonical_tip_index = Some(proposal.index);
                }
            } else if let Entry::Vacant(entry) = self.eliminations.entry(proposal.proposer) {
                // Record proposal as first elimination cause
                entry.insert(proposal.index);
            }

            // Insert proposal in db
            self.proposals.insert(proposal.index, proposal);

            // Update next game index
            self.next_factory_index += 1;
        }

        Ok(self.proposals.len() - initial_proposals)
    }

    pub fn is_proposer_eliminated(&self, proposer: Address) -> bool {
        self.eliminations.contains_key(&proposer)
    }

    pub fn was_proposer_eliminated_before(&self, proposer: Address, index: u64) -> bool {
        self.eliminations
            .get(&proposer)
            .map(|p| p < &index)
            .unwrap_or_default()
    }

    pub fn canonical_tip(&self) -> Option<&Proposal> {
        self.canonical_tip_index
            .map(|i| self.proposals.get(&i).unwrap())
    }

    pub fn canonical_tip_height(&self) -> Option<u64> {
        self.canonical_tip_index
            .map(|i| self.proposals.get(&i).unwrap().output_block_number)
    }

    pub async fn unresolved_canonical_proposals<
        T: Transport + Clone,
        P: Provider<T, N>,
        N: Network,
    >(
        &mut self,
        l1_node_provider: &P,
    ) -> anyhow::Result<Vec<u64>> {
        // Nothing to do without a canonical tip
        if self.canonical_tip_index.is_none() {
            return Ok(Vec::new());
        }
        // traverse up chain starting from canonical tip
        let mut unresolved_proposal_indices = vec![self.canonical_tip_index.unwrap()];
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
