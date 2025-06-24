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

use crate::blobs::blob_fe_proof;
use crate::fault::Fault;
use crate::provider::beacon::blob_sidecar;
use crate::provider::SyncProvider;
use crate::stall::Stall;
use crate::{await_tel, await_tel_res};
use alloy::consensus::{Blob, BlobTransactionSidecar};
use alloy::eips::eip4844::FIELD_ELEMENTS_PER_BLOB;
use alloy::network::Network;
use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::providers::Provider;
use alloy_rpc_types_beacon::sidecar::BlobData;
use anyhow::{bail, Context};
use kailua_common::blobs::{hash_to_fe, intermediate_outputs, trail_data};
use kailua_common::precondition::blobs_hash;
use kailua_contracts::{KailuaTournament::KailuaTournamentInstance, *};
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BTreeSet;
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
    pub children: BTreeSet<u64>,
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
    /// When the proposal was resolved
    pub resolved_at: u64,
}

pub enum ProposalSync {
    SUCCESS(Address, B256),
    DELAYED(u64),
    IGNORED(Address, B256),
}

impl Proposal {
    pub async fn load(provider: &SyncProvider, address: Address) -> anyhow::Result<Self> {
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(tracer.start("Proposal::load"));

        let tournament_instance = KailuaTournament::new(address, &provider.l1_provider);
        let parent_address = tournament_instance
            .parentGame()
            .stall_with_context(context.clone(), "KailuaTournament::parentGame")
            .await;
        if parent_address == address {
            info!("Loading KailuaTreasury instance");
            await_tel!(context, Self::load_treasury(provider, address))
        } else {
            info!("Loading KailuaGame with parent {parent_address}");
            await_tel!(context, Self::load_game(provider, address))
        }
    }

    async fn load_treasury(provider: &SyncProvider, address: Address) -> anyhow::Result<Self> {
        let tracer = tracer("kailua");
        let context =
            opentelemetry::Context::current_with_span(tracer.start("Proposal::load_treasury"));

        let treasury = tokio::task::spawn({
            let context = context.clone();
            let treasury_instance = KailuaTreasury::new(address, provider.l1_provider.clone());
            async move {
                treasury_instance
                    .KAILUA_TREASURY()
                    .stall_with_context(context, "KailuaGame::KAILUA_TREASURY")
                    .await
            }
        });
        let index = tokio::task::spawn({
            let context = context.clone();
            let treasury_instance = KailuaTreasury::new(address, provider.l1_provider.clone());
            async move {
                treasury_instance
                    .gameIndex()
                    .stall_with_context(context.clone(), "KailuaTreasury::gameIndex")
                    .await
                    .to()
            }
        });
        let created_at = tokio::task::spawn({
            let context = context.clone();
            let treasury_instance = KailuaTreasury::new(address, provider.l1_provider.clone());
            async move {
                treasury_instance
                    .createdAt()
                    .stall_with_context(context.clone(), "KailuaTreasury::createdAt")
                    .await
            }
        });
        // claim data
        let output_root = tokio::task::spawn({
            let context = context.clone();
            let treasury_instance = KailuaTreasury::new(address, provider.l1_provider.clone());
            async move {
                treasury_instance
                    .rootClaim()
                    .stall_with_context(context.clone(), "KailuaTreasury::rootClaim")
                    .await
                    .0
                    .into()
            }
        });
        let output_block_number = tokio::task::spawn({
            let context = context.clone();
            let treasury_instance = KailuaTreasury::new(address, provider.l1_provider.clone());
            async move {
                treasury_instance
                    .l2BlockNumber()
                    .stall_with_context(context.clone(), "KailuaTreasury::l2BlockNumber")
                    .await
                    .to()
            }
        });
        let l1_head = tokio::task::spawn({
            let context = context.clone();
            let treasury_instance = KailuaTreasury::new(address, provider.l1_provider.clone());
            async move {
                treasury_instance
                    .l1Head()
                    .stall_with_context(context.clone(), "KailuaTreasury::l1Head")
                    .await
            }
        });
        let signature = tokio::task::spawn({
            let context = context.clone();
            let treasury_instance = KailuaTreasury::new(address, provider.l1_provider.clone());
            async move {
                treasury_instance
                    .signature()
                    .stall_with_context(context.clone(), "KailuaTreasury::signature")
                    .await
            }
        });
        let resolved_at = tokio::task::spawn({
            let context = context.clone();
            let treasury_instance = KailuaTreasury::new(address, provider.l1_provider.clone());
            async move {
                treasury_instance
                    .resolvedAt()
                    .stall_with_context(context.clone(), "KailuaTreasury::resolvedAt")
                    .await
            }
        });

        let index = index.await?;
        Ok(Self {
            contract: address,
            treasury: treasury.await?,
            index,
            parent: index,
            proposer: address,
            created_at: created_at.await?,
            io_blobs: vec![],
            io_field_elements: vec![],
            trail_field_elements: vec![],
            output_root: output_root.await?,
            output_block_number: output_block_number.await?,
            l1_head: l1_head.await?,
            signature: signature.await?,
            children: Default::default(),
            successor: None,
            correct_io: vec![],
            correct_trail: vec![],
            correct_claim: Some(true),
            correct_parent: Some(true),
            canonical: None,
            resolved_at: resolved_at.await?,
        })
    }

    async fn load_game(provider: &SyncProvider, address: Address) -> anyhow::Result<Self> {
        let tracer = tracer("kailua");
        let context =
            opentelemetry::Context::current_with_span(tracer.start("Proposal::load_game"));

        let treasury = tokio::task::spawn({
            let context = context.clone();
            let game_instance = KailuaGame::new(address, provider.l1_provider.clone());
            async move {
                game_instance
                    .KAILUA_TREASURY()
                    .stall_with_context(context, "KailuaGame::KAILUA_TREASURY")
                    .await
            }
        });
        let index = tokio::task::spawn({
            let context = context.clone();
            let game_instance = KailuaGame::new(address, provider.l1_provider.clone());
            async move {
                game_instance
                    .gameIndex()
                    .stall_with_context(context.clone(), "KailuaGame::gameIndex")
                    .await
                    .to()
            }
        });
        let parent = tokio::task::spawn({
            let context = context.clone();
            let game_instance = KailuaGame::new(address, provider.l1_provider.clone());
            async move {
                game_instance
                    .parentGameIndex()
                    .stall_with_context(context.clone(), "KailuaGame::parentGameIndex")
                    .await
            }
        });
        let proposer = tokio::task::spawn({
            let context = context.clone();
            let game_instance = KailuaGame::new(address, provider.l1_provider.clone());
            async move {
                game_instance
                    .proposer()
                    .stall_with_context(context.clone(), "KailuaGame::proposer")
                    .await
            }
        });
        let created_at = tokio::task::spawn({
            let context = context.clone();
            let game_instance = KailuaGame::new(address, provider.l1_provider.clone());
            async move {
                game_instance
                    .createdAt()
                    .stall_with_context(context.clone(), "KailuaGame::createdAt")
                    .await
            }
        });
        // claim data
        let output_root = tokio::task::spawn({
            let context = context.clone();
            let game_instance = KailuaGame::new(address, provider.l1_provider.clone());
            async move {
                game_instance
                    .rootClaim()
                    .stall_with_context(context.clone(), "KailuaGame::rootClaim")
                    .await
                    .0
                    .into()
            }
        });
        let output_block_number = tokio::task::spawn({
            let context = context.clone();
            let game_instance = KailuaGame::new(address, provider.l1_provider.clone());
            async move {
                game_instance
                    .l2BlockNumber()
                    .stall_with_context(context.clone(), "KailuaGame::l2BlockNumber")
                    .await
                    .to()
            }
        });
        let l1_head = tokio::task::spawn({
            let context = context.clone();
            let game_instance = KailuaGame::new(address, provider.l1_provider.clone());
            async move {
                game_instance
                    .l1Head()
                    .stall_with_context(context.clone(), "KailuaGame::l1Head")
                    .await
                    .0
                    .into()
            }
        });
        let signature = tokio::task::spawn({
            let context = context.clone();
            let game_instance = KailuaGame::new(address, provider.l1_provider.clone());
            async move {
                game_instance
                    .signature()
                    .stall_with_context(context.clone(), "KailuaGame::signature")
                    .await
                    .0
                    .into()
            }
        });
        let resolved_at = tokio::task::spawn({
            let context = context.clone();
            let game_instance = KailuaGame::new(address, provider.l1_provider.clone());
            async move {
                game_instance
                    .resolvedAt()
                    .stall_with_context(context.clone(), "KailuaTreasury::resolvedAt")
                    .await
            }
        });
        // fetch blob data
        let proposal_blobs = tokio::task::spawn({
            let context = context.clone();
            let game_instance = KailuaGame::new(address, provider.l1_provider.clone());
            async move {
                game_instance
                    .PROPOSAL_BLOBS()
                    .stall_with_context(context.clone(), "KailuaGame::PROPOSAL_BLOBS")
                    .await
            }
        });
        let proposal_output_count = tokio::task::spawn({
            let context = context.clone();
            let game_instance = KailuaGame::new(address, provider.l1_provider.clone());
            async move {
                game_instance
                    .PROPOSAL_OUTPUT_COUNT()
                    .stall_with_context(context.clone(), "KailuaGame::PROPOSAL_OUTPUT_COUNT")
                    .await
            }
        });
        let mut io_blobs = Vec::new();
        let mut io_field_elements = Vec::new();
        let mut trail_field_elements = Vec::new();
        let created_at: u64 = created_at.await?;
        let proposal_blobs: u64 = proposal_blobs.await?;
        let proposal_output_count: u64 = proposal_output_count.await?;
        let game_instance = KailuaGame::new(address, &provider.l1_provider);
        for _ in 0..proposal_blobs {
            let blob_kzg_hash = game_instance
                .proposalBlobHashes(U256::from(io_blobs.len()))
                .stall_with_context(context.clone(), "KailuaGame::proposalBlobHashes")
                .await;
            let blob_data = await_tel!(
                context,
                provider.da_provider.get_blob(created_at, blob_kzg_hash)
            )
            .context("get_blob")?;
            // save data
            let io_remaining = proposal_output_count - (io_field_elements.len() as u64) - 1;
            let io_in_blob = io_remaining.min(FIELD_ELEMENTS_PER_BLOB) as usize;
            io_field_elements.extend(intermediate_outputs(&blob_data.blob, io_in_blob)?);
            trail_field_elements.extend(trail_data(&blob_data.blob, io_in_blob)?);
            io_blobs.push((blob_kzg_hash, blob_data));
        }
        let trail_len = trail_field_elements.len();
        Ok(Self {
            contract: address,
            treasury: treasury.await?,
            index: index.await?,
            parent: parent.await?,
            proposer: proposer.await?,
            created_at,
            io_blobs,
            io_field_elements,
            trail_field_elements,
            output_root: output_root.await?,
            output_block_number: output_block_number.await?,
            l1_head: l1_head.await?,
            signature: signature.await?,
            children: Default::default(),
            successor: None,
            correct_io: repeat(None)
                .take((proposal_output_count - 1) as usize)
                .collect(),
            correct_trail: repeat(None).take(trail_len).collect(),
            correct_claim: None,
            correct_parent: None,
            canonical: None,
            resolved_at: resolved_at.await?,
        })
    }

    pub fn as_delayed(&self) -> ProposalSync {
        ProposalSync::DELAYED(self.output_block_number)
    }

    pub fn as_ignored(&self) -> ProposalSync {
        ProposalSync::IGNORED(self.contract, self.l1_head)
    }

    pub fn as_success(&self) -> ProposalSync {
        ProposalSync::SUCCESS(self.contract, self.l1_head)
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

    pub fn fault(&self) -> Option<Fault> {
        // Check divergence in trail data
        for i in 0..self.correct_trail.len() {
            if let Some(false) = self.correct_trail[i] {
                return Some(Fault::Trail(self.io_field_elements.len() + i + 1));
            }
        }
        // Check divergence in IO
        for i in 0..self.correct_io.len() {
            if let Some(false) = self.correct_io[i] {
                return Some(Fault::Output(i));
            }
        }
        // Check divergence in final claim
        if let Some(false) = self.correct_claim {
            return Some(Fault::Output(self.io_field_elements.len()));
        }
        // Report equivalence
        None
    }

    pub fn has_parent(&self) -> bool {
        self.index != self.parent
    }

    pub fn append_child(&mut self, child_index: u64) -> bool {
        self.children.insert(child_index)
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

    pub fn tournament_contract_instance<P: Provider<N>, N: Network>(
        &self,
        provider: P,
    ) -> KailuaTournamentInstance<P, N> {
        KailuaTournament::new(self.contract, provider)
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
            .await;
        let parent_tournament_instance = KailuaTournament::new(parent_tournament, &provider);
        let children = parent_tournament_instance
            .childCount()
            .stall_with_context(context.clone(), "KailuaTournament::childCount")
            .await;
        let survivor = await_tel_res!(
            context,
            tracer,
            "KailuaTournament::pruneChildren",
            parent_tournament_instance
                .pruneChildren(children * U256::from(2))
                .call()
                .into_future()
        )?;
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
                .await,
        )
    }

    pub async fn fetch_resolved_at<P: Provider<N>, N: Network>(&self, provider: P) -> u64 {
        let tracer = tracer("kailua");
        let context =
            opentelemetry::Context::current_with_span(tracer.start("Proposal::fetch_resolved_at"));

        self.tournament_contract_instance(provider)
            .resolvedAt()
            .stall_with_context(context.clone(), "KailuaTournament::resolvedAt")
            .await
    }

    pub fn parse_finality(game_status: u8) -> anyhow::Result<Option<bool>> {
        match game_status {
            0u8 => Ok(None),        // IN_PROGRESS
            1u8 => Ok(Some(false)), // CHALLENGER_WINS
            2u8 => Ok(Some(true)),  // DEFENDER_WINS
            _ => bail!("Invalid game status {game_status}"),
        }
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
            .is_zero())
    }
}
