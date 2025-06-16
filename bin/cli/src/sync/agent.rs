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
use crate::{retry_res_ctx_timeout, retry_res_timeout, CoreArgs, KAILUA_GAME_TYPE};
use alloy::network::Network;
use alloy::primitives::{Address, B256, U256};
use alloy_provider::Provider;
use anyhow::{anyhow, bail, Context};
use futures::future::join_all;
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
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{error, info, warn};

/// A stateful agent object for synchronizing with an on-chain Kailua deployment.
pub struct SyncAgent {
    /// RPC providers to use for querying chain data
    pub provider: SyncProvider,
    /// Telemetry object for reporting synchronization state
    pub telemetry: SyncTelemetry,
    /// L2 Configuration of the rollup being monitored
    pub config: RollupConfig,
    /// Kailua deployment configuration for instance being synchronized
    pub deployment: SyncDeployment,
    /// Local persistent key-value store
    pub db: Arc<rocksdb::DB>,
    /// Pointers to the latest synchronized items
    pub cursor: SyncCursor,
    /// In-memory cache of op-node query results
    pub outputs: BTreeMap<u64, B256>,
    /// In-memory cache of on-chain proposal data
    pub proposals: BTreeMap<u64, Proposal>,
    /// In-memory cache of proposer elimination rounds
    pub eliminations: BTreeMap<Address, u64>,
}

impl SyncAgent {
    pub async fn new(
        core_args: &CoreArgs,
        mut data_dir: PathBuf,
        game_impl_address: Option<Address>,
        anchor_address: Option<Address>,
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
        #[cfg(not(feature = "devnet"))]
        {
            let image_id: B256 =
                bytemuck::cast::<[u32; 8], [u8; 32]>(kailua_build::KAILUA_FPVM_ID).into();
            if deployment.image_id != image_id {
                bail!(
                    "Deployment image ID mismatch. Expected {:?}, got {:?}.",
                    image_id,
                    deployment.image_id
                );
            }
        }

        // Initialize persistent DB
        data_dir.push(deployment.cfg_hash.to_string());
        data_dir.push(deployment.treasury.to_string());
        let db = Arc::new(
            rocksdb::DB::open(&Self::db_options(), &data_dir).context("rocksdb::DB::open")?,
        );

        // Create cursor
        let cursor = await_tel_res!(
            context,
            SyncCursor::load(&deployment, &provider, anchor_address),
            "SyncCursor::load"
        )?;

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

    pub fn free_cached_data(&mut self) -> anyhow::Result<BTreeSet<u64>> {
        // delete all proposals prior to last resolved proposal
        let Some(earliest_proposal) = self.proposals.first_key_value().map(|(k, _)| *k) else {
            return Ok(Default::default());
        };
        let mut proposals = BTreeSet::new();
        for i in earliest_proposal..self.cursor.last_resolved_game {
            if self.proposals.remove(&i).is_some() {
                info!("Freed proposal {i} from memory.");
                proposals.insert(i);
            }
        }
        // delete all output commitments prior to last resolved proposal
        let Some(earliest_output) = self
            .proposals
            .get(&self.cursor.last_resolved_game)
            .map(|p| p.output_block_number)
        else {
            bail!("Last resolved game is missing from database.");
        };
        let mut commitments = 0;
        for i in 1..=earliest_output {
            let output_number =
                earliest_output.saturating_sub(i * self.deployment.output_block_span);
            if self.outputs.remove(&output_number).is_none() {
                // abort early once we hit an output commitment we never stored during this run
                break;
            }
            if let Err(err) = self.db.delete(output_number.to_be_bytes()) {
                error!("Failed to delete output commitment {output_number} from storage: {err:?}.");
            }
            commitments += 1;
        }
        if commitments > 0 {
            info!("Freed {commitments} output commitments from storage.");
        }
        Ok(proposals)
    }

    pub async fn sync(&mut self) -> anyhow::Result<Vec<u64>> {
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(tracer.start("SyncAgent::sync"));

        // load all relevant output commitments
        let sync_status = await_tel!(
            context,
            tracer,
            "sync_status",
            retry_res_ctx_timeout!(self.provider.op_provider.sync_status().await)
        );
        let output_block_number = sync_status["safe_l2"]["number"]
            .as_u64()
            .unwrap()
            .min(self.cursor.last_output_index + self.deployment.blocks_per_proposal());
        if self.cursor.last_output_index + self.deployment.output_block_span < output_block_number {
            info!(
                "Syncing with op-node from block {} until block {output_block_number}",
                self.cursor.last_output_index
            );
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
        }

        // load new proposals
        let dispute_game_factory =
            IDisputeGameFactory::new(self.deployment.factory, self.provider.l1_provider.clone());
        let game_count: u64 = dispute_game_factory
            .gameCount()
            .stall_with_context(context.clone(), "DisputeGameFactory::gameCount")
            .await
            .to();
        let first_factory_index = self.cursor.next_factory_index;
        while self.cursor.next_factory_index < game_count {
            let proposal = match self
                .sync_proposal(&dispute_game_factory, self.cursor.next_factory_index)
                .with_context(context.clone())
                .await
            {
                Ok(processed) => {
                    if processed {
                        // append proposal to returned result
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
                if let Some(true) = proposal.canonical {
                    // Update canonical chain tip
                    self.cursor.canonical_proposal_tip = proposal.index;
                    // Update last resolved index
                    if proposal.resolved_at != 0 {
                        self.cursor.last_resolved_game = proposal.index;
                    }
                } else if let Some(false) = proposal.is_correct() {
                    // Update player eliminations
                    if let Entry::Vacant(entry) = self.eliminations.entry(proposal.proposer) {
                        entry.insert(proposal.index);
                    }
                }
            }

            // Prune memory and storage
            if let Err(err) = self.free_cached_data() {
                error!("Failed to free cached data: {err:?}.");
            }

            // Process next game index
            self.cursor.next_factory_index += 1;
        }

        // update proposal resolutions
        loop {
            let Some(last_unresolved_proposal_index) = self
                .proposals
                .get(&self.cursor.last_resolved_game)
                .ok_or_else(|| {
                    anyhow!(
                        "Last resolved proposal {} missing from database.",
                        self.cursor.last_resolved_game
                    )
                })?
                .successor
            else {
                break;
            };

            let Some(last_unresolved_proposal) =
                self.proposals.get_mut(&last_unresolved_proposal_index)
            else {
                bail!("Last unresolved proposal {last_unresolved_proposal_index} missing from database.");
            };

            let resolved_at = last_unresolved_proposal
                .fetch_resolved_at(&self.provider.l1_provider)
                .await;

            // stop at last unresolved proposal
            if resolved_at == 0 {
                break;
            }
            // update resolved status
            last_unresolved_proposal.resolved_at = resolved_at;
            // move cursor forward
            self.cursor.last_resolved_game = last_unresolved_proposal_index;
        }

        // Update sync telemetry
        if let Some(canonical_tip) = self.proposals.get(&self.cursor.canonical_proposal_tip) {
            self.telemetry.sync_canonical.record(
                canonical_tip.index,
                &[
                    KeyValue::new("proposal", canonical_tip.contract.to_string()),
                    KeyValue::new("l2_height", canonical_tip.output_block_number.to_string()),
                ],
            );
        } else {
            error!(
                "Telemetry update failed. Canonical proposal tip {} missing from database.",
                self.cursor.canonical_proposal_tip
            );
        };
        self.telemetry
            .sync_next
            .record(self.cursor.next_factory_index, &[]);

        // Collect newly processed and retained proposals
        let proposals = (first_factory_index..self.cursor.next_factory_index)
            .filter(|p| self.proposals.contains_key(p))
            .collect();

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
        let mut proposal = Proposal::load(&self.provider, game_address)
            .with_context(context.clone())
            .await?;
        // Skip proposals unrelated to current run
        if proposal.treasury != self.deployment.treasury {
            info!("Skipping proposal for different deployment.");
            return Ok(false);
        }
        // Skip dangling proposals
        if !self.proposals.is_empty() && !self.proposals.contains_key(&proposal.parent) {
            warn!("Ignoring dangling proposal.");
            return Ok(false);
        }

        // Check if the proposer elimination round is non-zero
        if let Entry::Vacant(vacancy) = self.eliminations.entry(proposal.proposer) {
            let treasury_contract =
                KailuaTreasury::new(self.deployment.treasury, &self.provider.l1_provider);
            let elimination_round: u64 = treasury_contract
                .eliminationRound(proposal.proposer)
                .stall_with_context(context.clone(), "KailuaTreasury::eliminationRound")
                .await
                .to();
            if elimination_round > 0 {
                vacancy.insert(elimination_round);
            }
        }

        // Skip irrelevant proposals
        if !self
            .determine_tournament_participation(&mut proposal)
            .context("Failed to determine tournament participation")?
        {
            warn!(
                "Ignoring proposal {} (no tournament participation)",
                proposal.index
            );
            return Ok(false);
        }

        // Fetch any relevant data from op-node
        if self.cursor.last_output_index + self.deployment.output_block_span
            < proposal.output_block_number
        {
            bail!(
                "Cannot yet process proposal until op-node safely derives L2 block {}.",
                proposal.output_block_number
            );
        }

        // Determine inherited correctness
        self.assess_correctness(&mut proposal)
            .with_context(context.clone())
            .await
            .context("Failed to determine proposal correctness")?;

        // Determine whether to follow or eliminate proposer
        let is_proposal_canonical = self
            .determine_if_canonical(&mut proposal)
            .ok_or_else(|| {
                anyhow!(
                    "Failed to determine if proposal {} is canonical (correctness: {:?}).",
                    proposal.index,
                    proposal.is_correct()
                )
            })
            .context("Failed to determine if proposal is canonical.")?;

        // Determine if the proposal is its parent's successor
        if is_proposal_canonical {
            if let Some(parent) = self.proposals.get_mut(&proposal.parent) {
                parent.successor = Some(proposal.index);
            }
        }

        // Store proposal and return inclusion
        self.proposals.insert(proposal.index, proposal);
        Ok(true)
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
            // Consider updating canonical chain tip if proposal has greater height
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

    pub fn canonical_tip(&self) -> Option<&Proposal> {
        self.proposals.get(&self.cursor.canonical_proposal_tip)
    }

    pub fn canonical_tip_height(&self) -> Option<u64> {
        self.proposals
            .get(&self.cursor.canonical_proposal_tip)
            .map(|p| p.output_block_number)
    }

    pub fn determine_tournament_participation(
        &mut self,
        proposal: &mut Proposal,
    ) -> anyhow::Result<bool> {
        // Treasury is accepted by default
        if !proposal.has_parent() {
            return Ok(true);
        }
        // Resolved games are accepted by default
        if proposal.resolved_at != 0 {
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
                    "Attempted to append duplicate child {} to parent {}.",
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
        Ok(true)
    }

    pub fn cached_output_at_block(&self, block_number: u64) -> Option<B256> {
        self.outputs.get(&block_number).cloned()
    }

    pub async fn sync_outputs(&mut self, mut start: u64, end: u64, step: u64) {
        while start <= end {
            // perform at most 1024 tasks at a time
            let end = end.min(start + 1024 * step);

            // check persisted data
            for i in (start..=end).step_by(step as usize) {
                if let Ok(Some(output)) = self.db.get(i.to_be_bytes()) {
                    let output = B256::from_slice(&output);
                    self.outputs.insert(i, output);
                    if self.cursor.last_output_index < i {
                        self.cursor.last_output_index = i;
                    }
                }
            }

            let outputs = (start..=end)
                .step_by(step as usize)
                .filter(|i| !self.outputs.contains_key(i))
                .map(|i| {
                    let provider = self.provider.op_provider.clone();
                    Box::pin(async move {
                        (
                            i,
                            retry_res_timeout!(provider.output_at_block(i).await).await,
                        )
                    })
                })
                .collect_vec();
            let outputs = join_all(outputs).await;

            if !outputs.is_empty() {
                info!("Fetched {} outputs.", outputs.len());
            }
            // Store outputs in memory and database
            for (i, output) in outputs.into_iter() {
                self.outputs.insert(i, output);
                self.db
                    .put(i.to_be_bytes(), output.0)
                    .expect("Database error");
                if self.cursor.last_output_index < i {
                    self.cursor.last_output_index = i;
                }
            }

            // jump forward
            start = end + step;
        }
    }
}
