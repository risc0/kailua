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
use crate::sync::cursor::SyncCursor;
use crate::sync::deployment::SyncDeployment;
use crate::sync::proposal::Proposal;
use crate::sync::provider::SyncProvider;
use crate::sync::telemetry::SyncTelemetry;
use crate::{retry, retry_with_context, CoreArgs, KAILUA_GAME_TYPE};
use alloy::network::Network;
use alloy::primitives::{Address, B256, U256};
use alloy_provider::Provider;
use anyhow::{anyhow, bail, Context};
use itertools::Itertools;
use kailua_client::{await_tel, await_tel_res};
use kailua_common::blobs::hash_to_fe;
use kailua_common::config::config_hash;
use kailua_contracts::{
    IDisputeGameFactory::{gameAtIndexReturn, IDisputeGameFactoryInstance},
    *,
};
use kailua_host::config::fetch_rollup_config;
use kona_genesis::RollupConfig;
use opentelemetry::global::tracer;
use opentelemetry::trace::FutureExt;
use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::KeyValue;
// use rayon::prelude::{IntoParallelIterator, ParallelIterator};
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use std::path::PathBuf;
use tracing::{error, info, warn};

pub struct SyncAgent {
    pub provider: SyncProvider,
    pub telemetry: SyncTelemetry,
    pub config: RollupConfig,
    pub deployment: SyncDeployment,
    pub db: rocksdb::DB,
    pub cursor: SyncCursor,
    pub outputs: BTreeMap<u64, B256>,
    pub proposals: BTreeMap<u64, Proposal>,
    pub eliminations: BTreeMap<Address, u64>,
}

impl Drop for SyncAgent {
    fn drop(&mut self) {
        let _ = rocksdb::DB::destroy(&Self::db_options(), self.db.path());
    }
}

impl SyncAgent {
    pub async fn new(
        core_args: &CoreArgs,
        mut data_dir: PathBuf,
        game_impl_address: Option<Address>,
    ) -> anyhow::Result<Self> {
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(tracer.start("SymcAgemt::new"));
        // Initialize telemetry first
        let telemetry = SyncTelemetry::new();

        // Connect to RPC providers
        let provider = await_tel_res!(context, SyncProvider::new(core_args), "SyncProvider::new")?;

        // fetch rollup config
        info!("Fetching rollup configuration from rpc endpoints.");
        let config = await_tel_res!(
            context,
            fetch_rollup_config(&core_args.op_node_url, &core_args.op_geth_url, None),
            "fetch_rollup_config"
        )?;
        let rollup_config_hash = config_hash(&config).expect("Configuration hash derivation error");
        info!("RollupConfigHash({})", hex::encode(rollup_config_hash));

        // Load target deployment data
        let deployment = await_tel_res!(
            context,
            SyncDeployment::load(&provider, &config, game_impl_address),
            "Deployment::load"
        )?;

        // Initialize persistent DB
        data_dir.push(deployment.cfg_hash.to_string());
        data_dir.push(deployment.treasury.to_string());
        let db = rocksdb::DB::open(&Self::db_options(), &data_dir).context("rocksdb::DB::open")?;

        // Create cursor
        let treasury = KailuaTreasury::new(deployment.treasury, &provider.l1_provider);

        let last_resolved_game_address = treasury
            .lastResolved()
            .stall_with_context(context.clone(), "KailuaTreasury::lastResolved")
            .await;

        if last_resolved_game_address.is_zero() {
            bail!("No resolved games found. Deployment has not been fully configured.");
        }

        let last_resolved_game =
            KailuaTournament::new(last_resolved_game_address, &provider.l1_provider);

        let last_game_index: u64 = last_resolved_game
            .gameIndex()
            .stall_with_context(context.clone(), "KailuaTournament::gameIndex")
            .await
            .to();

        let lrg_parent_address = last_resolved_game
            .parentGame()
            .stall_with_context(context.clone(), "KailuaTournament::parentGame")
            .await;

        let last_output_index: u64 = last_resolved_game
            .l2BlockNumber()
            .stall_with_context(context.clone(), "KailuaTournament::l2BlockNumber")
            .await
            .to();

        let next_output_index = if lrg_parent_address == last_resolved_game_address {
            // get block height of treasury instance
            last_output_index
        } else {
            // get starting block height of game instance
            last_output_index - deployment.proposal_output_count * deployment.output_block_span
        };

        let cursor = SyncCursor {
            canonical_proposal_tip: None,
            last_resolved_proposal: last_game_index,
            next_factory_index: last_game_index,
            last_output_index: next_output_index,
        };

        Ok(Self {
            provider,
            telemetry,
            config,
            deployment,
            db,
            cursor,
            outputs: Default::default(),
            proposals: Default::default(),
            eliminations: Default::default(),
        })
    }

    fn db_options() -> rocksdb::Options {
        let mut options = rocksdb::Options::default();
        options.create_if_missing(true);
        options
    }

    pub async fn sync(&mut self) -> anyhow::Result<Vec<u64>> {
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(tracer.start("SyncAgent::sync"));

        // load all relevant output commitments
        let sync_status = await_tel_res!(
            context,
            tracer,
            "sync_status",
            retry_with_context!(self.provider.op_provider.sync_status())
        )?;
        let output_block_number = sync_status["safe_l2"]["number"].as_u64().unwrap();
        info!("Syncing with op-node until block {output_block_number}");
        await_tel!(
            context,
            tracer,
            "sync_outputs",
            self.sync_outputs(
                self.cursor.last_output_index,
                output_block_number,
                self.deployment.output_block_span
            )
        );

        // load new proposals
        let dispute_game_factory =
            IDisputeGameFactory::new(self.deployment.factory, self.provider.l1_provider.clone());
        let game_count: u64 = dispute_game_factory
            .gameCount()
            .stall_with_context(context.clone(), "DisputeGameFactory::gameCount")
            .await
            .to();
        let mut proposals = Vec::new();
        while self.cursor.next_factory_index < game_count {
            let proposal = match self
                .sync_proposal(&dispute_game_factory, self.cursor.next_factory_index)
                .with_context(context.clone())
                .await
            {
                Ok(processed) => {
                    if processed {
                        // append proposal to returned result
                        proposals.push(self.cursor.next_factory_index);
                        let proposal = self
                            .proposals
                            .get(&self.cursor.next_factory_index)
                            .ok_or_else(|| {
                                anyhow!("Failed to load immediately processed proposal")
                            })?;
                        Some(proposal)
                    } else {
                        None
                    }
                }
                Err(err) => {
                    error!(
                        "Error loading game at index {}: {err:?}",
                        self.cursor.next_factory_index
                    );
                    break;
                }
            };
            // Update state according to proposal
            if let Some(proposal) = proposal {
                // update next output index
                self.cursor.last_output_index = proposal.output_block_number;
                if let Some(true) = proposal.canonical {
                    // Update canonical chain tip
                    self.cursor.canonical_proposal_tip = Some(proposal.index);
                } else if let Some(false) = proposal.is_correct() {
                    // Update player eliminations
                    if let Entry::Vacant(entry) = self.eliminations.entry(proposal.proposer) {
                        entry.insert(proposal.index);
                    }
                }
            }

            // Process next game index
            self.cursor.next_factory_index += 1;
        }

        // Update sync telemetry
        if let Some(canonical_tip) = self
            .cursor
            .canonical_proposal_tip
            .map(|i| self.proposals.get(&i).unwrap())
        {
            self.telemetry.sync_canonical.record(
                canonical_tip.index,
                &[
                    KeyValue::new("proposal", canonical_tip.contract.to_string()),
                    KeyValue::new("l2_height", canonical_tip.output_block_number.to_string()),
                ],
            );
        };
        self.telemetry
            .sync_next
            .record(self.cursor.next_factory_index, &[]);

        Ok(proposals)
    }

    pub async fn sync_proposal<P: Provider<N>, N: Network>(
        &mut self,
        dispute_game_factory: &IDisputeGameFactoryInstance<P, N>,
        index: u64,
    ) -> anyhow::Result<bool> {
        let tracer = tracer("kailua");
        let context =
            opentelemetry::Context::current_with_span(tracer.start("SyncAgent::sync_proposal"));

        // process game
        let gameAtIndexReturn {
            gameType_: game_type,
            proxy_: game_address,
            ..
        } = dispute_game_factory
            .gameAtIndex(U256::from(index))
            .stall_with_context(context.clone(), "DisputeGameFactory::gameAtIndex")
            .await;
        // skip entries for other game types
        if game_type != KAILUA_GAME_TYPE {
            info!("Skipping proposal of different game type {game_type} at factory index {index}");
            return Ok(false);
        }
        info!("Processing tournament {index} at {game_address}");
        let tournament_instance =
            KailuaTournament::new(game_address, dispute_game_factory.provider());
        let mut proposal = Proposal::load(&self.provider.da_provider, &tournament_instance)
            .with_context(context.clone())
            .await?;
        // Skip proposals unrelated to current run
        if proposal.treasury != self.deployment.treasury {
            info!("Skipping proposal for different deployment.");
            return Ok(false);
        }

        // Determine inherited correctness
        if self.cursor.last_output_index < proposal.output_block_number {
            info!(
                "Syncing with op-node until block {}",
                proposal.output_block_number
            );
            await_tel!(
                context,
                tracer,
                "sync_outputs",
                self.sync_outputs(
                    self.cursor.last_output_index,
                    proposal.output_block_number,
                    self.deployment.output_block_span
                )
            );
        }
        self.assess_correctness(&mut proposal)
            .with_context(context.clone())
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
            self.proposals.insert(proposal.index, proposal);
            Ok(true)
        } else {
            warn!(
                "Ignoring proposal {} (no tournament participation)",
                proposal.index
            );
            Ok(false)
        }
    }

    pub async fn assess_correctness(&mut self, proposal: &mut Proposal) -> anyhow::Result<bool> {
        // Accept correctness of treasury instance data
        if !proposal.has_parent() {
            info!("Accepting initial treasury proposal as true.");
            return Ok(true);
        }

        // Validate game instance data
        info!("Assessing proposal correctness..");
        let is_parent_correct = if proposal.resolved_at == 0 {
            self.proposals
                .get(&proposal.parent)
                .map(|parent| {
                    parent
                        .is_correct()
                        .expect("Attempted to process child before deciding parent correctness")
                })
                .unwrap_or_default() // missing parent means it's not part of the tournament
        } else {
            true
        };

        // Update parent status
        proposal.correct_parent = Some(is_parent_correct);
        // Check root claim correctness
        let Some(local_claim) = self.cached_output_at_block(proposal.output_block_number) else {
            bail!("Failed to fetch local claim for proposal.");
        };

        // Update claim status
        proposal.correct_claim = Some(local_claim == proposal.output_root);
        // Check intermediate output correctness for KailuaGame instances
        if proposal.has_parent() {
            let starting_block_number = proposal
                .output_block_number
                .saturating_sub(self.deployment.blocks_per_proposal());
            // output commitments
            for (i, output_fe) in proposal.io_field_elements.iter().enumerate() {
                let io_number =
                    starting_block_number + (i as u64 + 1) * self.deployment.output_block_span;
                let Some(output_hash) = self.cached_output_at_block(io_number) else {
                    bail!("Failed to fetch output hash for block {io_number}.");
                };
                proposal.correct_io[i] = Some(&hash_to_fe(output_hash) == output_fe);
            }
            // trail data
            for (i, output_fe) in proposal.trail_field_elements.iter().enumerate() {
                proposal.correct_trail[i] = Some(output_fe.is_zero());
            }
        }
        // Return correctness
        let is_correct_proposal = match proposal.is_correct() {
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
            // Consider updating canonical chain tip if none exists or proposal has greater height
            if self
                .canonical_tip_height()
                .is_none_or(|h| h < proposal.output_block_number)
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

    pub fn was_proposer_eliminated_before(&self, proposal: &Proposal) -> bool {
        self.eliminations
            .get(&proposal.proposer)
            .map(|p| p < &proposal.index)
            .unwrap_or_default()
    }

    pub fn canonical_tip_height(&self) -> Option<u64> {
        self.cursor
            .canonical_proposal_tip
            .map(|i| self.proposals.get(&i).unwrap().output_block_number)
    }

    pub fn determine_tournament_participation(
        &mut self,
        proposal: &mut Proposal,
    ) -> anyhow::Result<bool> {
        if !proposal.has_parent() {
            // Treasury is accepted by default
            return Ok(true);
        } else if proposal.resolved_at != 0 {
            // Resolved games are part of the tournament
            return Ok(true);
        }

        // Scope for mutable access to parent
        {
            // Skipped parents imply skipped children
            let Some(parent) = self.proposals.get_mut(&proposal.parent) else {
                return Ok(false);
            };
            // Append child to parent tournament children list
            if !parent.append_child(proposal.index) {
                warn!(
                    "Attempted duplicate child {} insertion into parent {} ",
                    proposal.index, parent.index
                );
            }
        }

        // Scope for immutable access to parent
        {
            let parent = self.proposals.get(&proposal.parent).unwrap();
            // Participate in tournament only if this is not a post-bad proposal
            if self.was_proposer_eliminated_before(proposal) {
                return Ok(false);
            }
            // Skip proposals to extend non-canonical tournaments
            if !parent.canonical.unwrap_or_default() {
                return Ok(false);
            }
            // Ignore timed-out counter-proposals
            if let Some(successor) = parent
                .successor
                .map(|index| self.proposals.get(&index).unwrap())
            {
                // Skip proposals arriving after the timeout period for the correct proposal
                if proposal.created_at - successor.created_at >= self.deployment.timeout {
                    return Ok(false);
                }
            }
        }

        // Determine if opponent is the next successor
        if let Some(true) = proposal.canonical {
            let parent = self.proposals.get_mut(&proposal.parent).unwrap();
            parent.successor = Some(proposal.index);
        }
        Ok(true)
    }

    pub fn cached_output_at_block(&self, block_number: u64) -> Option<B256> {
        self.outputs.get(&block_number).cloned()
    }

    pub async fn sync_outputs(&mut self, start: u64, end: u64, step: u64) {
        // todo: persistence to disk
        // info!("Loading outputs from block {start} to block {end} with step {step}.");
        let outputs = (start..=end)
            .step_by(step as usize)
            .filter(|i| !self.outputs.contains_key(i))
            // .collect_vec()
            // .into_par_iter()
            .map(|o| {
                (
                    o,
                    kona_proof::block_on(async {
                        tracing::debug!("Loading output at block {o}");
                        let res = retry!(self.provider.op_provider.output_at_block(o).await)
                            .await
                            .unwrap();
                        tracing::debug!("Loaded output at block {o}: {res:?}");
                        res
                    }),
                )
            })
            .collect_vec();
        // .collect_vec_list();
        if !outputs.is_empty() {
            info!("Loaded {} outputs.", outputs.len());
        }
        // Store outputs in memory
        for (i, output) in outputs.into_iter() {
            // for (i, output) in outputs.into_iter().flatten() {
            self.outputs.insert(i, output);
            self.cursor.last_output_index = i + step;
        }
    }
}
