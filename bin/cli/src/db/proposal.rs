use crate::db::config::Config;
use crate::provider::{blob_fe_proof, blob_sidecar, get_block, BlobProvider};
use crate::retry_with_context;
use crate::stall::Stall;
use crate::transact::Transact;
use alloy::consensus::{Blob, BlobTransactionSidecar, BlockHeader};
use alloy::eips::eip4844::FIELD_ELEMENTS_PER_BLOB;
use alloy::eips::BlockNumberOrTag;
use alloy::network::{BlockResponse, Network, ReceiptResponse};
use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::providers::Provider;
use alloy_rpc_types_beacon::sidecar::BlobData;
use anyhow::{bail, Context};
use kailua_client::provider::OpNodeProvider;
use kailua_client::{await_tel, await_tel_res};
use kailua_common::blobs::{hash_to_fe, intermediate_outputs, trail_data};
use kailua_common::precondition::blobs_hash;
use kailua_contracts::{
    KailuaGame::KailuaGameInstance, KailuaTournament::KailuaTournamentInstance,
    KailuaTreasury::KailuaTreasuryInstance, *,
};
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::future::IntoFuture;
use std::iter::repeat;
use tracing::{error, info};

pub const ELIMINATIONS_LIMIT: u64 = 128;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proposal {
    // pointers
    /// Address of the contract instance
    pub contract: Address,
    /// Address of the tied treasury
    pub treasury: Address,
    /// DGF Index of the game
    pub index: u64,
    /// DGF Index of the game's parent
    pub parent: u64,
    /// Address of the proposer
    pub proposer: Address,
    // claim data
    /// Contract creation timestamp
    pub created_at: u64,
    /// All intermediate output blobs
    pub io_blobs: Vec<(B256, BlobData)>,
    /// Individual intermediate output field elements
    pub io_field_elements: Vec<U256>,
    /// Individual trailing data field elements
    pub trail_field_elements: Vec<U256>,
    /// Claimed output root
    pub output_root: B256,
    /// Claimed output root block number
    pub output_block_number: u64,
    /// Proposal L1 head
    pub l1_head: B256,
    /// Proposal IO/Claim signature
    pub signature: B256,
    // tournament data
    /// List of child proposals
    pub children: Vec<u64>,
    /// Index of successor proposal
    pub successor: Option<u64>,
    // correctness
    /// Correctness of each intermediate output in proposal
    pub correct_io: Vec<Option<bool>>,
    /// Correctness of each trailing data element in proposal
    pub correct_trail: Vec<Option<bool>>,
    /// Correctness of claimed output root
    pub correct_claim: Option<bool>,
    /// Correctness of parent proposal
    pub correct_parent: Option<bool>,
    // resolution
    /// Whether the proposal is canonical
    pub canonical: Option<bool>,
}

impl Proposal {
    pub async fn load<P: Provider<N>, N: Network>(
        blob_provider: &BlobProvider,
        tournament_instance: &KailuaTournamentInstance<(), P, N>,
    ) -> anyhow::Result<Self> {
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(tracer.start("Proposal::load"));

        let instance_address = *tournament_instance.address();
        let parent_address = tournament_instance
            .parentGame()
            .stall_with_context(context.clone(), "KailuaTournament::parentGame")
            .await
            .parentGame_;
        if parent_address == instance_address {
            info!("Loading KailuaTreasury instance");
            await_tel!(
                context,
                Self::load_treasury(&KailuaTreasury::new(
                    instance_address,
                    tournament_instance.provider(),
                ))
            )
        } else {
            info!("Loading KailuaGame with parent {parent_address}");
            await_tel!(
                context,
                Self::load_game(
                    blob_provider,
                    &KailuaGame::new(instance_address, tournament_instance.provider()),
                )
            )
        }
    }

    async fn load_treasury<P: Provider<N>, N: Network>(
        treasury_instance: &KailuaTreasuryInstance<(), P, N>,
    ) -> anyhow::Result<Self> {
        let tracer = tracer("kailua");
        let context =
            opentelemetry::Context::current_with_span(tracer.start("Proposal::load_treasury"));

        let treasury = treasury_instance
            .KAILUA_TREASURY()
            .stall_with_context(context.clone(), "KailuaGame::KAILUA_TREASURY")
            .await
            ._0;
        let index = treasury_instance
            .gameIndex()
            .stall_with_context(context.clone(), "KailuaTreasury::gameIndex")
            .await
            ._0
            .to();
        let created_at = treasury_instance
            .createdAt()
            .stall_with_context(context.clone(), "KailuaTreasury::createdAt")
            .await
            ._0;
        // claim data
        let output_root = treasury_instance
            .rootClaim()
            .stall_with_context(context.clone(), "KailuaTreasury::rootClaim")
            .await
            .rootClaim_
            .0
            .into();
        let output_block_number = treasury_instance
            .l2BlockNumber()
            .stall_with_context(context.clone(), "KailuaTreasury::l2BlockNumber")
            .await
            .l2BlockNumber_
            .to();
        let l1_head = treasury_instance
            .l1Head()
            .stall_with_context(context.clone(), "KailuaTreasury::l1Head")
            .await
            .l1Head_
            .0
            .into();
        let signature = treasury_instance
            .signature()
            .stall_with_context(context.clone(), "KailuaTreasury::signature")
            .await
            .signature_
            .0
            .into();
        Ok(Self {
            contract: *treasury_instance.address(),
            treasury,
            index,
            parent: index,
            proposer: *treasury_instance.address(),
            created_at,
            io_blobs: vec![],
            io_field_elements: vec![],
            trail_field_elements: vec![],
            output_root,
            output_block_number,
            l1_head,
            signature,
            children: Default::default(),
            successor: None,
            correct_io: vec![],
            correct_trail: vec![],
            correct_claim: Some(true),
            correct_parent: Some(true),
            canonical: None,
        })
    }

    async fn load_game<P: Provider<N>, N: Network>(
        blob_provider: &BlobProvider,
        game_instance: &KailuaGameInstance<(), P, N>,
    ) -> anyhow::Result<Self> {
        let tracer = tracer("kailua");
        let context =
            opentelemetry::Context::current_with_span(tracer.start("Proposal::load_game"));

        let treasury = game_instance
            .KAILUA_TREASURY()
            .stall_with_context(context.clone(), "KailuaGame::KAILUA_TREASURY")
            .await
            ._0;
        let index = game_instance
            .gameIndex()
            .stall_with_context(context.clone(), "KailuaGame::gameIndex")
            .await
            ._0
            .to();
        let parent = game_instance
            .parentGameIndex()
            .stall_with_context(context.clone(), "KailuaGame::parentGameIndex")
            .await
            .parentGameIndex_;
        let proposer = game_instance
            .proposer()
            .stall_with_context(context.clone(), "KailuaGame::proposer")
            .await
            .proposer_;
        let created_at = game_instance
            .createdAt()
            .stall_with_context(context.clone(), "KailuaGame::createdAt")
            .await
            ._0;
        // fetch blob data
        let mut io_blobs = Vec::new();
        let mut io_field_elements = Vec::new();
        let mut trail_field_elements = Vec::new();
        let proposal_blobs: u64 = game_instance
            .PROPOSAL_BLOBS()
            .stall_with_context(context.clone(), "KailuaGame::PROPOSAL_BLOBS")
            .await
            ._0
            .to();
        let proposal_output_count: u64 = game_instance
            .PROPOSAL_OUTPUT_COUNT()
            .stall_with_context(context.clone(), "KailuaGame::PROPOSAL_OUTPUT_COUNT")
            .await
            ._0
            .to();
        for _ in 0..proposal_blobs {
            let blob_kzg_hash = game_instance
                .proposalBlobHashes(U256::from(io_blobs.len()))
                .stall_with_context(context.clone(), "KailuaGame::proposalBlobHashes")
                .await
                ._0;
            let blob_data = await_tel!(context, blob_provider.get_blob(created_at, blob_kzg_hash))
                .context("get_blob")?;
            // save data
            let io_remaining = proposal_output_count - (io_field_elements.len() as u64) - 1;
            let io_in_blob = io_remaining.min(FIELD_ELEMENTS_PER_BLOB) as usize;
            io_field_elements.extend(intermediate_outputs(&blob_data, io_in_blob)?);
            trail_field_elements.extend(trail_data(&blob_data, io_in_blob)?);
            io_blobs.push((blob_kzg_hash, blob_data));
        }
        // claim data
        let output_root = game_instance
            .rootClaim()
            .stall_with_context(context.clone(), "KailuaGame::rootClaim")
            .await
            .rootClaim_
            .0
            .into();
        let output_block_number: u64 = game_instance
            .l2BlockNumber()
            .stall_with_context(context.clone(), "KailuaGame::l2BlockNumber")
            .await
            .l2BlockNumber_
            .to();
        let l1_head = game_instance
            .l1Head()
            .stall_with_context(context.clone(), "KailuaGame::l1Head")
            .await
            .l1Head_
            .0
            .into();
        let signature = game_instance
            .signature()
            .stall_with_context(context.clone(), "KailuaGame::signature")
            .await
            .signature_
            .0
            .into();
        let trail_len = trail_field_elements.len();
        Ok(Self {
            contract: *game_instance.address(),
            treasury,
            index,
            parent,
            proposer,
            created_at,
            io_blobs,
            io_field_elements,
            trail_field_elements,
            output_root,
            output_block_number,
            l1_head,
            signature,
            children: Default::default(),
            successor: None,
            correct_io: repeat(None)
                .take((proposal_output_count - 1) as usize)
                .collect(),
            correct_trail: repeat(None).take(trail_len).collect(),
            correct_claim: None,
            correct_parent: None,
            canonical: None,
        })
    }

    pub async fn fetch_parent_tournament_survivor<P: Provider<N>, N: Network>(
        &self,
        provider: P,
    ) -> anyhow::Result<Option<Address>> {
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(
            tracer.start("Proposal::fetch_parent_tournament_survivor"),
        );

        if !self.has_parent() {
            return Ok(Some(self.contract));
        }
        let parent_tournament: Address = self
            .tournament_contract_instance(&provider)
            .parentGame()
            .stall_with_context(context.clone(), "KailuaTournament::parentGame")
            .await
            .parentGame_;
        let parent_tournament_instance = KailuaTournament::new(parent_tournament, &provider);
        let children = parent_tournament_instance
            .childCount()
            .stall_with_context(context.clone(), "KailuaTournament::childCount")
            .await
            .count_;
        let survivor = await_tel_res!(
            context,
            tracer,
            "KailuaTournament::pruneChildren",
            parent_tournament_instance
                .pruneChildren(children * U256::from(2))
                .call()
                .into_future()
        )?
        ._0;
        if survivor.is_zero() {
            Ok(None)
        } else {
            Ok(Some(survivor))
        }
    }

    pub async fn fetch_parent_tournament_survivor_status<P: Provider<N>, N: Network>(
        &self,
        provider: P,
    ) -> anyhow::Result<Option<bool>> {
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(
            tracer.start("Proposal::fetch_parent_tournament_survivor_status"),
        );

        let survivor = await_tel!(context, self.fetch_parent_tournament_survivor(provider))
            .context("Proposal::fetch_parent_tournament_survivor")?;
        let is_survivor_expected = survivor.map(|survivor| survivor == self.contract);
        if !is_survivor_expected.unwrap_or_default() {
            error!(
                "Current survivor: {survivor:?} (expecting {})",
                self.contract
            );
        } else {
            info!("Survivor: {}", self.contract);
        }
        Ok(is_survivor_expected)
    }

    pub async fn fetch_finality<P: Provider<N>, N: Network>(
        &self,
        provider: P,
    ) -> anyhow::Result<Option<bool>> {
        let tracer = tracer("kailua");
        let context =
            opentelemetry::Context::current_with_span(tracer.start("Proposal::fetch_finality"));

        Self::parse_finality(
            self.tournament_contract_instance(provider)
                .status()
                .stall_with_context(context.clone(), "KailuaTournament::status")
                .await
                ._0,
        )
    }

    pub async fn fetch_is_successor_validity_proven<P: Provider<N>, N: Network>(
        &self,
        provider: P,
    ) -> anyhow::Result<bool> {
        let tracer = tracer("kailua");
        let context =
            opentelemetry::Context::current_with_span(tracer.start("Proposal::fetch_finality"));

        Ok(!self
            .tournament_contract_instance(provider)
            .validChildSignature()
            .stall_with_context(context.clone(), "KailuaTournament::validChildSignature")
            .await
            ._0
            .is_zero())
    }

    pub async fn fetch_current_challenger_duration<P: Provider<N>, N: Network>(
        &self,
        provider: P,
    ) -> anyhow::Result<u64> {
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(
            tracer.start("Proposal::fetch_current_challenger_duration"),
        );

        let chain_time = await_tel!(context, get_block(&provider, BlockNumberOrTag::Latest))?
            .header()
            .timestamp();

        Ok(self
            .tournament_contract_instance(provider)
            .getChallengerDuration(U256::from(chain_time))
            .stall_with_context(context.clone(), "KailuaTournament::getChallengerDuration")
            .await
            .duration_)
    }

    pub fn parse_finality(game_status: u8) -> anyhow::Result<Option<bool>> {
        match game_status {
            0u8 => Ok(None),        // IN_PROGRESS
            1u8 => Ok(Some(false)), // CHALLENGER_WINS
            2u8 => Ok(Some(true)),  // DEFENDER_WINS
            _ => bail!("Invalid game status {game_status}"),
        }
    }

    pub async fn assess_correctness(
        &mut self,
        config: &Config,
        op_node_provider: &OpNodeProvider,
        is_correct_parent: bool,
    ) -> anyhow::Result<Option<bool>> {
        let tracer = tracer("kailua");
        let context =
            opentelemetry::Context::current_with_span(tracer.start("Proposal::assess_correctness"));

        // Update parent status
        self.correct_parent = Some(is_correct_parent);
        // Check root claim correctness
        let local_claim = await_tel_res!(
            context,
            tracer,
            "local_claim",
            retry_with_context!(op_node_provider.output_at_block(self.output_block_number))
        )?;

        self.correct_claim = Some(local_claim == self.output_root);
        // Check intermediate output correctness for KailuaGame instances
        if self.has_parent() {
            let starting_block_number = self
                .output_block_number
                .saturating_sub(config.blocks_per_proposal());
            debug_assert_eq!(
                self.io_field_elements.len() as u64,
                config.proposal_output_count - 1
            );
            // output commitments
            for (i, output_fe) in self.io_field_elements.iter().enumerate() {
                let io_number = starting_block_number + (i as u64 + 1) * config.output_block_span;
                let output_hash = await_tel_res!(
                    context,
                    tracer,
                    "output_hash",
                    retry_with_context!(op_node_provider.output_at_block(io_number))
                )?;
                self.correct_io[i] = Some(&hash_to_fe(output_hash) == output_fe);
            }
            // trail data
            for (i, output_fe) in self.trail_field_elements.iter().enumerate() {
                self.correct_trail[i] = Some(output_fe.is_zero());
            }
        }
        // Return correctness
        Ok(self.is_correct())
    }

    pub fn is_correct(&self) -> Option<bool> {
        // A proposal is false if it extends an incorrect parent proposal
        if let Some(false) = self.correct_parent {
            return Some(false);
        }
        // A proposal is false if its root claim is incorrect
        if let Some(false) = self.correct_claim {
            return Some(false);
        }
        // A proposal is false if any of the intermediate commitments can be proven false
        if self.correct_io.iter().flatten().any(|c| !c) {
            return Some(false);
        }
        // A proposal is false if it contains non-zero trailing io data
        if self.correct_trail.iter().flatten().any(|c| !c) {
            return Some(false);
        }
        // Unknown case
        if self.correct_parent.is_none()
            || self.correct_claim.is_none()
            || self.correct_io.iter().any(|c| c.is_none())
            || self.correct_trail.iter().any(|c| c.is_none())
        {
            return None;
        }
        // Correct!
        Some(true)
    }

    pub fn tournament_contract_instance<P: Provider<N>, N: Network>(
        &self,
        provider: P,
    ) -> KailuaTournamentInstance<(), P, N> {
        KailuaTournament::new(self.contract, provider)
    }

    pub async fn resolve<P: Provider<N>, N: Network>(
        &self,
        provider: P,
    ) -> anyhow::Result<N::ReceiptResponse> {
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(tracer.start("Proposal::resolve"));

        let contract_instance = self.tournament_contract_instance(&provider);
        let parent_tournament: Address = contract_instance
            .parentGame()
            .stall_with_context(context.clone(), "KailuaTournament::parentGame")
            .await
            .parentGame_;
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
            ._0;

            // If a survivor is returned we don't need pruning
            if !survivor.is_zero() {
                break;
            }

            info!("Eliminating {ELIMINATIONS_LIMIT} opponents before resolution.");
            let receipt = parent_tournament_instance
                .pruneChildren(U256::from(ELIMINATIONS_LIMIT))
                .transact_with_context(context.clone(), "KailuaTournament::pruneChildren")
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
            .transact_with_context(context.clone(), "KailuaTournament::resolve")
            .await
            .context("KailuaTournament::resolve")?;
        info!("KailuaTournament::resolve: {} gas", receipt.gas_used());

        Ok(receipt)
    }

    pub fn has_parent(&self) -> bool {
        self.index != self.parent
    }

    pub fn divergence_point(&self) -> Option<usize> {
        // Check divergence in IO
        for i in 0..self.correct_io.len() {
            if let Some(false) = self.correct_io[i] {
                return Some(i);
            }
        }
        // Check divergence in final claim
        if let Some(false) = self.correct_claim {
            return Some(self.io_field_elements.len());
        }
        // Check divergence in trail data
        for i in 0..self.correct_trail.len() {
            if let Some(false) = self.correct_trail[i] {
                return Some(self.io_field_elements.len() + i + 1);
            }
        }
        // Report equivalence
        None
    }

    pub fn append_child(&mut self, child_index: u64) -> bool {
        let should_insert = self.children.last().map_or(true, |i| i < &child_index);
        // The assumption is that this is always sorted and out of order insertions are duplicates
        if should_insert {
            self.children.push(child_index);
        }
        should_insert
    }

    pub fn child_index(&self, proposal_index: u64) -> Option<u64> {
        self.children
            .iter()
            .enumerate()
            .find(|(_, idx)| *idx == &proposal_index)
            .map(|r| r.0 as u64)
    }

    pub fn requires_vanguard_advantage(&self, proposer: Address, vanguard: Address) -> bool {
        if vanguard.is_zero() || vanguard == proposer {
            return false;
        }
        self.children.is_empty()
    }

    pub fn io_blob_for(&self, position: u64) -> (B256, BlobData) {
        let index = position / FIELD_ELEMENTS_PER_BLOB;
        self.io_blobs[index as usize].clone()
    }

    pub fn io_commitment_for(&self, position: u64) -> Bytes {
        let blob = self.io_blob_for(position);
        Bytes::from(blob.1.kzg_commitment.to_vec())
    }

    pub fn io_proof_for(&self, position: u64) -> anyhow::Result<Bytes> {
        let io_blob = self.io_blob_for(position);
        let (proof, _) = blob_fe_proof(
            &io_blob.1.blob,
            (position % FIELD_ELEMENTS_PER_BLOB) as usize,
        )?;
        Ok(Bytes::from(proof.to_vec()))
    }

    pub fn output_fe_at(&self, position: u64) -> U256 {
        let io_count = self.io_field_elements.len() as u64;
        match position.cmp(&io_count) {
            Ordering::Less => self
                .io_field_elements
                .get(position as usize)
                .copied()
                .unwrap(),
            Ordering::Equal => hash_to_fe(self.output_root),
            Ordering::Greater => self
                .trail_field_elements
                .get((position - io_count - 1) as usize)
                .copied()
                .unwrap(),
        }
    }

    pub fn create_sidecar(io_field_elements: &[U256]) -> anyhow::Result<BlobTransactionSidecar> {
        let mut io_blobs = vec![];
        loop {
            let start = io_blobs.len() * FIELD_ELEMENTS_PER_BLOB as usize;
            if start >= io_field_elements.len() {
                break;
            }
            let end = (start + FIELD_ELEMENTS_PER_BLOB as usize).min(io_field_elements.len());
            let io_bytes = io_field_elements[start..end]
                .iter()
                .map(|e| e.to_be_bytes::<32>())
                .collect::<Vec<_>>()
                .concat();
            // Encode as blob sidecar with zero-byte trail
            let blob = Blob::right_padding_from(io_bytes.as_slice());
            io_blobs.push(blob);
        }
        blob_sidecar(io_blobs)
    }

    pub fn blobs_hash(&self) -> B256 {
        blobs_hash(self.io_blobs.iter().map(|(h, _)| h))
    }

    pub fn signature(&self) -> B256 {
        self.signature
    }
}
