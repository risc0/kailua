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

use crate::blob_provider::BlobProvider;
use crate::{hash_to_fe, output_at_block, KAILUA_GAME_TYPE};
use alloy::network::Network;
use alloy::primitives::{Address, FixedBytes, U256};
use alloy::providers::{Provider, ReqwestProvider};
use alloy::transports::Transport;
use alloy_rpc_types_beacon::sidecar::BlobData;
use anyhow::{bail, Context};
use kailua_common::intermediate_outputs;
use kailua_contracts::IDisputeGameFactory::{gameAtIndexReturn, IDisputeGameFactoryInstance};
use kailua_contracts::{KailuaGame, KailuaGame::KailuaGameInstance};
use std::collections::{HashMap, HashSet};
use tracing::{error, info, warn};

#[derive(Clone, Debug)]
pub struct Proposal {
    pub local_index: usize,
    pub factory_index: u64,
    pub game_address: Address,
    pub parent_local_index: usize,
    pub intermediate_output_blob: Option<BlobData>,
    pub output_root: FixedBytes<32>,
    pub output_block_number: u64,
    pub challenged: HashSet<u32>,
    pub unresolved_challenges: u32,
    pub proven: HashMap<u32, bool>,
    pub resolved: HashSet<u32>,
    pub correct: Vec<bool>,
    pub is_correct_parent: bool,
}

impl Proposal {
    pub fn is_correct(&self) -> bool {
        self.is_correct_parent && self.correct.iter().all(|&x| x)
    }

    pub fn is_challenged(&self) -> bool {
        !self.challenged.is_empty() || self.has_unresolved_challenges()
    }

    pub fn has_unresolved_challenges(&self) -> bool {
        self.unresolved_challenges > 0
    }

    pub fn is_game_resolved(&self) -> bool {
        self.resolved.contains(&0)
    }

    pub fn is_game_challenged(&self) -> bool {
        self.challenged.contains(&0)
    }

    pub fn is_game_proven(&self) -> Option<bool> {
        self.proven.get(&0).copied()
    }

    pub fn is_output_resolved(&self, output_number: u32) -> bool {
        self.resolved.contains(&output_number)
    }

    pub fn is_output_challenged(&self, output_number: u32) -> bool {
        self.challenged.contains(&output_number)
    }

    pub fn is_output_proven(&self, output_number: u32) -> Option<bool> {
        self.proven.get(&output_number).copied()
    }

    pub fn canonical_challenge_position(&self) -> Option<u32> {
        if self.is_correct() {
            None
        } else if !self.is_correct_parent {
            Some(0u32)
        } else {
            Some(
                self.correct
                    .iter()
                    // skip the first flag which denotes invalid root claim
                    .skip(1)
                    .position(|v| !v)
                    .map(|p| p + 1)
                    .unwrap_or(self.correct.len()) as u32,
            )
        }
    }

    pub fn game_contract<T: Transport + Clone, P: Provider<T, N>, N: Network>(
        &self,
        l1_node_provider: P,
    ) -> KailuaGameInstance<T, P, N> {
        KailuaGame::new(self.game_address, l1_node_provider)
    }
}

#[derive(Clone, Debug, Default)]
pub struct ProposalDB {
    pub proposals: Vec<Proposal>,
    pub index_map: HashMap<u64, usize>,
    pub latest_factory_index: u64,
    pub canonical_tip_index: Option<usize>,
}

impl ProposalDB {
    pub async fn load_proposals<T: Transport + Clone, P: Provider<T, N>, N: Network>(
        &mut self,
        dispute_game_factory: &IDisputeGameFactoryInstance<T, P, N>,
        op_node_provider: &ReqwestProvider,
        // l2_node_provider: &ReqwestProvider,
        cl_node_provider: &BlobProvider,
    ) -> anyhow::Result<usize> {
        let initial_proposals = self.proposals.len();
        let game_count: u64 = dispute_game_factory
            .gameCount()
            .call()
            .await?
            .gameCount_
            .to();
        for factory_index in self.latest_factory_index..game_count {
            let gameAtIndexReturn {
                gameType_: game_type,
                proxy_: game_address,
                timestamp_: created_at,
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
            // Retrieve basic data
            let game_contract = KailuaGame::new(game_address, dispute_game_factory.provider());
            let output_root = game_contract
                .rootClaim()
                .call()
                .await
                .context("rootClaim")?
                .rootClaim_;
            let output_block_number: u64 = game_contract
                .l2BlockNumber()
                .call()
                .await
                .context("l2BlockNumber")?
                .l2BlockNumber_
                .to();
            // Instantiate sub-claim trackers
            let mut challenged = HashSet::new();
            let mut resolved = HashSet::new();
            let extra_data = game_contract
                .extraData()
                .call()
                .await
                .context("extraData")?
                .extraData_;
            let local_index = self.proposals.len();
            // Retrieve game/setup data
            let (parent_local_index, blob, unresolved_challenges) = match extra_data.len() {
                0x30 => {
                    // KailuaGame instance
                    info!("Retrieving basic KailuaGame proposal data");
                    // check if game was resolved
                    if game_contract.resolvedAt().call().await.context("resolvedAt")?._0 > 0 {
                        resolved.insert(0);
                    }
                    // check if parent validity was challenged
                    if game_contract.challengedAt(0).call().await.context("challengedAt(0)")?._0 > 0 {
                        challenged.insert(0);
                    }
                    let parent_factory_index = game_contract
                        .parentGameIndex()
                        .call()
                        .await
                        .context("parentGameIndex")?
                        .parentGameIndex_;
                    let Some(parent_local_index) = self.index_map.get(&parent_factory_index) else {
                        error!("SKIPPED: Could not find parent local index for game {game_address} at factory index {factory_index}.");
                        continue;
                    };
                    let blob_hash = game_contract.proposalBlobHash().call().await.context("proposalBlobHash")?.blobHash_;
                    let Ok(blob) = cl_node_provider.get_blob(
                        created_at,
                        blob_hash
                    ).await else {
                        // .context(format!("get_blob {created_at}/{blob_hash}"))?
                        error!("SKIPPED: Could not fetch blob {created_at}/{blob_hash} for game {game_address} at factory index {factory_index}.");
                        continue;
                    };
                    let unresolved_challenges = game_contract
                        .unresolvedClaimCount()
                        .call()
                        .await
                        .context("unresolvedClaimCount")?
                        ._0;
                    (*parent_local_index, Some(blob), unresolved_challenges)
                }
                0x20 => {
                    // KailuaTreasury instance
                    info!("Retrieving basic KailuaTreasury proposal data");
                    (local_index, None, 0u32)
                }
                len => bail!("Unexpected extra-data length {len} from game {game_address} at factory index {factory_index}")
            };
            // Get pointer to parent
            let parent = if parent_local_index != local_index {
                Some(&self.proposals[parent_local_index])
            } else {
                None
            };
            // Decide correctness according to op-node
            // todo: preform validations based on l1_head
            info!("Deciding proposal validity.");
            let local_output_root = output_at_block(op_node_provider, output_block_number).await?;
            // Parent must be correct if KailuaGame and the local output must match the proposed output
            let is_correct_parent = parent.map(|p| p.is_correct()).unwrap_or(true);
            info!("Parent correctness: {is_correct_parent}");
            let game_correctness = is_correct_parent && local_output_root == output_root;
            info!("Main proposal correctness: {game_correctness}");
            // initialize correctness vector with game value at position 0
            let mut correct = vec![game_correctness];
            if let Some(parent) = parent {
                // Calculate intermediate correctness values for KailuaGame
                let blob_data = blob.as_ref().expect("Missing blob data.");
                let starting_output_number = parent.output_block_number + 1;
                let num_intermediate = (output_block_number - starting_output_number) as usize;
                let outputs = intermediate_outputs(blob_data, num_intermediate)?;
                let mut bad_io = 0;
                for (i, output) in outputs.iter().enumerate().take(num_intermediate) {
                    let local_output =
                        output_at_block(op_node_provider, starting_output_number + i as u64)
                            .await?;
                    let io_correct = &hash_to_fe(local_output) == output;
                    correct.push(io_correct);
                    if !io_correct {
                        bad_io += 1;
                    }
                }
                if bad_io > 0 {
                    warn!("Found {bad_io} incorrect intermediate proposals.");
                } else {
                    info!("Intermediate proposals are correct.")
                }
            }
            // update local tree view
            info!("Storing proposal in memory at local index {local_index}.");
            self.index_map.insert(factory_index, local_index);
            self.proposals.push(Proposal {
                local_index,
                factory_index,
                game_address,
                parent_local_index,
                intermediate_output_blob: blob,
                output_root,
                output_block_number,
                challenged,
                unresolved_challenges,
                proven: HashMap::new(),
                resolved,
                correct,
                is_correct_parent,
            });
            let correct = self.proposals[local_index].is_correct();
            info!("Read {correct} proposal at factory index {factory_index}");
            // Update canonical chain tip if this proposal yields a longer valid chain
            let canonical_tip_height = self
                .canonical_tip_index
                .map(|idx| self.proposals[idx].output_block_number)
                .unwrap_or_default();
            if correct && output_block_number > canonical_tip_height {
                info!("Updating canonical proposal chain tip to local index {local_index}.");
                self.canonical_tip_index = Some(local_index);
            }
        }
        self.latest_factory_index = game_count;
        Ok(self.proposals.len() - initial_proposals)
    }

    pub async fn unresolved_canonical_proposals<
        T: Transport + Clone,
        P: Provider<T, N>,
        N: Network,
    >(
        &mut self,
        l1_node_provider: &P,
    ) -> anyhow::Result<Vec<usize>> {
        let Some(canonical_tip) = self.canonical_tip_index else {
            return Ok(vec![]);
        };
        let mut unresolved_proposal_indices = vec![canonical_tip];
        // traverse up tree
        loop {
            let local_index = *unresolved_proposal_indices.last().unwrap();
            let proposal = &mut self.proposals[local_index];
            let game_contract = proposal.game_contract(l1_node_provider);
            // Update on-chain resolution status
            if game_contract.resolvedAt().call().await?._0 > 0 {
                proposal.resolved.insert(0);
            }
            // break if we reach a resolved game or a setup game
            if proposal.is_game_resolved() {
                unresolved_proposal_indices.pop();
                break;
            } else if proposal.parent_local_index == local_index {
                // this is an unresolved setup game, keep in stack
                break;
            }
            unresolved_proposal_indices.push(proposal.parent_local_index);
        }
        Ok(unresolved_proposal_indices)
    }
}
