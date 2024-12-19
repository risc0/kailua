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
pub mod state;
pub mod treasury;

use crate::providers::beacon::BlobProvider;
use crate::providers::optimism::OpNodeProvider;
use crate::stall::Stall;
use crate::KAILUA_GAME_TYPE;
use alloy::network::Network;
use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use alloy::transports::Transport;
use anyhow::{bail, Context};
use config::Config;
use kailua_contracts::{
    IDisputeGameFactory::{gameAtIndexReturn, IDisputeGameFactoryInstance},
    *,
};
use proposal::Proposal;
use state::State;
use std::collections::hash_map::Entry;
use std::path::PathBuf;
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

#[derive(Debug)]
pub struct KailuaDB {
    pub config: Config,
    pub treasury: Treasury,
    pub db: rocksdb::DB,
    pub state: State,
}

impl Drop for KailuaDB {
    fn drop(&mut self) {
        let _ = rocksdb::DB::destroy(&Self::options(), self.db.path());
    }
}

impl KailuaDB {
    pub fn options() -> rocksdb::Options {
        let mut options = rocksdb::Options::default();
        options.create_if_missing(true);
        options
    }

    pub async fn init<T: Transport + Clone, P: Provider<T, N>, N: Network>(
        mut data_dir: PathBuf,
        dispute_game_factory: &IDisputeGameFactoryInstance<T, P, N>,
    ) -> anyhow::Result<Self> {
        let game_implementation = KailuaGame::new(
            dispute_game_factory
                .gameImpls(KAILUA_GAME_TYPE)
                .stall()
                .await
                .impl_,
            dispute_game_factory.provider(),
        );
        let config = Config::load(&game_implementation).await?;
        let treasury_implementation =
            KailuaTreasury::new(config.treasury, dispute_game_factory.provider());
        let treasury = Treasury::init(&treasury_implementation).await?;

        data_dir.push(config.cfg_hash.to_string());
        let db = rocksdb::DB::open(&Self::options(), &data_dir)?;
        Ok(Self {
            config,
            treasury,
            db,
            state: Default::default(),
        })
    }

    pub async fn load_proposals<T: Transport + Clone, P: Provider<T, N>, N: Network>(
        &mut self,
        dispute_game_factory: &IDisputeGameFactoryInstance<T, P, N>,
        op_node_provider: &OpNodeProvider,
        blob_provider: &BlobProvider,
    ) -> anyhow::Result<Vec<u64>> {
        let canonical_start = self.state.canonical_tip_index;
        let game_count: u64 = dispute_game_factory
            .gameCount()
            .stall()
            .await
            .gameCount_
            .to();
        let mut proposals =
            Vec::with_capacity((game_count - self.state.next_factory_index) as usize);
        while self.state.next_factory_index < game_count {
            let proposal = match self.get_local_proposal(&self.state.next_factory_index) {
                Some(proposal) => Some(proposal),
                None => {
                    match self
                        .load_game_at_index(
                            dispute_game_factory,
                            op_node_provider,
                            blob_provider,
                            self.state.next_factory_index,
                        )
                        .await
                    {
                        Ok(processed) => {
                            if processed {
                                proposals.push(self.state.next_factory_index);
                                Some(
                                    self.get_local_proposal(&self.state.next_factory_index)
                                        .expect("Failed to load immediately processed proposal"),
                                )
                            } else {
                                None
                            }
                        }
                        Err(err) => {
                            error!(
                                "Error loading game at index {}: {err:?}",
                                self.state.next_factory_index
                            );
                            break;
                        }
                    }
                }
            };

            // Update state according to proposal
            if let Some(proposal) = proposal {
                if let Some(true) = proposal.canonical {
                    // Update canonical chain tip
                    self.state.canonical_tip_index = Some(proposal.index);
                } else if let Some(false) = proposal.is_correct() {
                    // Update player eliminations
                    if let Entry::Vacant(entry) = self.state.eliminations.entry(proposal.proposer) {
                        entry.insert(proposal.index);
                    }
                }
            }

            // Process next game index
            self.state.next_factory_index += 1;
        }

        if canonical_start != self.state.canonical_tip_index {
            info!(
                "Updating canonical proposal chain tip to {:?}.",
                self.state.canonical_tip_index
            );
        }

        Ok(proposals)
    }

    pub async fn load_game_at_index<T: Transport + Clone, P: Provider<T, N>, N: Network>(
        &mut self,
        dispute_game_factory: &IDisputeGameFactoryInstance<T, P, N>,
        op_node_provider: &OpNodeProvider,
        blob_provider: &BlobProvider,
        index: u64,
    ) -> anyhow::Result<bool> {
        // process game
        let gameAtIndexReturn {
            gameType_: game_type,
            proxy_: game_address,
            ..
        } = dispute_game_factory
            .gameAtIndex(U256::from(index))
            .stall()
            .await;
        // skip entries for other game types
        if game_type != KAILUA_GAME_TYPE {
            info!("Skipping proposal of different game type {game_type} at factory index {index}");
            return Ok(false);
        }
        info!("Processing tournament {index} at {game_address}");
        let tournament_instance =
            KailuaTournament::new(game_address, dispute_game_factory.provider());
        let mut proposal =
            Proposal::load(&self.config, blob_provider, &tournament_instance).await?;

        // Determine inherited correctness
        self.determine_correctness(&mut proposal, op_node_provider)
            .await
            .context("Failed to determine proposal correctness")?;

        // Determine whether to follow or eliminate proposer
        if self.determine_if_canonical(&mut proposal).is_none() {
            bail!(
                "Failed to determine if proposal {} is canonical (correctness: {:?}).",
                proposal.index,
                proposal.is_correct()
            );
        }

        // Determine tournament performance
        if self
            .determine_tournament_participation(&mut proposal)
            .context("Failed to determine tournament participation")?
        {
            // Insert proposal in db
            self.set_local_proposal(proposal.index, &proposal)?;
            Ok(true)
        } else {
            warn!(
                "Ignoring proposal {} (no tournament participation)",
                proposal.index
            );
            Ok(false)
        }
    }

    pub async fn determine_correctness(
        &mut self,
        proposal: &mut Proposal,
        op_node_provider: &OpNodeProvider,
    ) -> anyhow::Result<bool> {
        // Accept correctness of treasury instance data
        if !proposal.has_parent() {
            info!("Accepting initial treasury proposal as true.");
            return Ok(true);
        }

        // Validate game instance data
        info!("Assessing proposal correctness..");
        let is_parent_correct = self
            .get_local_proposal(&proposal.parent)
            .expect("Attempted to process child before registering parent.")
            .is_correct()
            .expect("Attempted to process child before deciding parent correctness");
        let is_correct_proposal = match proposal
            .assess_correctness(&self.config, op_node_provider, is_parent_correct)
            .await?
        {
            None => {
                bail!("Failed to assess correctness. Is op-node synced far enough?");
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
        Ok(is_correct_proposal)
    }

    pub fn determine_if_canonical(&mut self, proposal: &mut Proposal) -> Option<bool> {
        if proposal.is_correct()? && !self.was_proposer_eliminated_before(proposal) {
            // Consider updating canonical chain tip
            if self
                .canonical_tip_height()
                .map_or(true, |h| h < proposal.output_block_number)
            {
                proposal.canonical = Some(true);
            } else {
                proposal.canonical = Some(false);
            }
        } else {
            // Set as non-canonical
            proposal.canonical = Some(false);
        }
        proposal.canonical
    }

    pub fn determine_tournament_participation(
        &mut self,
        proposal: &mut Proposal,
    ) -> anyhow::Result<bool> {
        if !proposal.has_parent() {
            return Ok(true);
        }

        let mut parent = self.get_local_proposal(&proposal.parent).unwrap().clone();
        // Ignore self-conflict
        if parent
            .survivor
            .map(|contender| {
                self.get_local_proposal(&contender).unwrap().proposer == proposal.proposer
            })
            .unwrap_or_default()
        {
            return Ok(false);
        }
        // Participate in tournament only if this is a correct or first bad proposal
        if self.was_proposer_eliminated_before(proposal) {
            return Ok(false);
        }
        // Skip non-canonical tournaments
        if !parent.canonical.unwrap_or_default() {
            return Ok(false);
        }
        // Update the contender
        proposal.contender = parent.survivor;
        // Append child to parent
        if !parent.append_child(proposal.index) {
            warn!(
                "Attempted out of order child {} insertion into parent {} ",
                proposal.index, parent.index
            );
        }
        // Determine survivorship
        if parent
            .survivor
            .map(|contender| {
                !self
                    .get_local_proposal(&contender)
                    .unwrap()
                    .wins_against(proposal)
            })
            .unwrap_or(true)
        {
            // If the old survivor (if any) is defeated,
            // set this proposal as the new survivor
            parent.survivor = Some(proposal.index);
        }
        // Commit updated parent data
        self.set_local_proposal(proposal.parent, &parent)?;
        Ok(true)
    }

    pub fn get_local_proposal(&self, index: &u64) -> Option<Proposal> {
        self.db
            .get(index.to_be_bytes())
            .ok()?
            .and_then(|data| bincode::deserialize(&data).ok())
    }

    pub fn set_local_proposal(&mut self, index: u64, proposal: &Proposal) -> anyhow::Result<()> {
        Ok(self
            .db
            .put(index.to_be_bytes(), bincode::serialize(proposal)?)?)
    }

    pub fn is_proposer_eliminated(&self, proposer: Address) -> bool {
        self.state.eliminations.contains_key(&proposer)
    }

    pub fn was_proposer_eliminated_before(&self, proposal: &Proposal) -> bool {
        self.state
            .eliminations
            .get(&proposal.proposer)
            .map(|p| p < &proposal.index)
            .unwrap_or_default()
    }

    pub fn canonical_tip(&self) -> Option<Proposal> {
        self.state
            .canonical_tip_index
            .map(|i| self.get_local_proposal(&i).unwrap())
    }

    pub fn canonical_tip_height(&self) -> Option<u64> {
        self.state
            .canonical_tip_index
            .map(|i| self.get_local_proposal(&i).unwrap().output_block_number)
    }

    pub async fn unresolved_canonical_proposals<
        T: Transport + Clone,
        P: Provider<T, N>,
        N: Network,
    >(
        &self,
        l1_node_provider: &P,
    ) -> anyhow::Result<Vec<u64>> {
        // Nothing to do without a canonical tip
        if self.state.canonical_tip_index.is_none() {
            return Ok(Vec::new());
        }
        // traverse up chain starting from canonical tip
        let mut unresolved_proposal_indices = vec![self.state.canonical_tip_index.unwrap()];
        loop {
            let proposal_index = *unresolved_proposal_indices.last().unwrap();
            let proposal = self.get_local_proposal(&proposal_index).unwrap();
            // break if we reach a resolved game or a setup game
            if proposal.fetch_finality(l1_node_provider).await?.is_some() {
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
