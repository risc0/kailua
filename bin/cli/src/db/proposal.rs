use crate::db::config::Config;
use crate::providers::beacon::blob_fe_proof;
use crate::providers::beacon::{blob_sidecar, BlobProvider};
use crate::providers::optimism::OpNodeProvider;
use crate::stall::Stall;
use alloy::consensus::{Blob, BlobTransactionSidecar, BlockHeader};
use alloy::eips::eip4844::FIELD_ELEMENTS_PER_BLOB;
use alloy::eips::{BlockId, BlockNumberOrTag};
use alloy::network::primitives::BlockTransactionsKind;
use alloy::network::{BlockResponse, Network};
use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::providers::Provider;
use alloy::transports::Transport;
use alloy_rpc_types_beacon::sidecar::BlobData;
use anyhow::{bail, Context};
use kailua_common::blobs::{hash_to_fe, intermediate_outputs};
use kailua_contracts::{
    KailuaGame::KailuaGameInstance, KailuaTournament::KailuaTournamentInstance,
    KailuaTreasury::KailuaTreasuryInstance, *,
};
use serde::{Deserialize, Serialize};
use std::iter::repeat;
use tracing::{error, info, warn};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proposal {
    // pointers
    pub contract: Address,
    pub index: u64,
    pub parent: u64,
    pub proposer: Address,
    // claim data
    pub created_at: u64,
    pub io_blobs: Vec<(B256, BlobData)>,
    pub io_field_elements: Vec<B256>,
    pub output_root: B256,
    pub output_block_number: u64,
    pub l1_head: B256,
    // tournament data
    pub children: Vec<u64>,
    pub survivor: Option<u64>,
    pub contender: Option<u64>,
    // correctness
    pub correct_io: Vec<Option<bool>>,
    pub correct_claim: Option<bool>,
    pub correct_parent: Option<bool>,
    // resolution
    pub canonical: Option<bool>,
}

impl Proposal {
    pub async fn load<T: Transport + Clone, P: Provider<T, N>, N: Network>(
        config: &Config,
        blob_provider: &BlobProvider,
        tournament_instance: &KailuaTournamentInstance<T, P, N>,
    ) -> anyhow::Result<Self> {
        let instance_address = *tournament_instance.address();
        let parent_address = tournament_instance.parentGame().stall().await.parentGame_;
        if parent_address == instance_address {
            info!("Loading KailuaTreasury instance");
            Self::load_treasury(&KailuaTreasury::new(
                instance_address,
                tournament_instance.provider(),
            ))
            .await
        } else {
            info!("Loading KailuaGame with parent {parent_address}");
            Self::load_game(
                config,
                blob_provider,
                &KailuaGame::new(instance_address, tournament_instance.provider()),
            )
            .await
        }
    }

    async fn load_treasury<T: Transport + Clone, P: Provider<T, N>, N: Network>(
        treasury_instance: &KailuaTreasuryInstance<T, P, N>,
    ) -> anyhow::Result<Self> {
        let index = treasury_instance.gameIndex().stall().await._0.to();
        let created_at = treasury_instance.createdAt().stall().await._0;
        // claim data
        let output_root = treasury_instance
            .rootClaim()
            .stall()
            .await
            .rootClaim_
            .0
            .into();
        let output_block_number = treasury_instance
            .l2BlockNumber()
            .stall()
            .await
            .l2BlockNumber_
            .to();
        let l1_head = treasury_instance.l1Head().stall().await.l1Head_.0.into();
        Ok(Self {
            contract: *treasury_instance.address(),
            index,
            parent: index,
            proposer: *treasury_instance.address(),
            created_at,
            io_blobs: vec![],
            io_field_elements: vec![],
            output_root,
            output_block_number,
            l1_head,
            children: Default::default(),
            survivor: None,
            contender: None,
            correct_io: vec![],
            correct_claim: Some(true),
            correct_parent: Some(true),
            canonical: None,
        })
    }

    async fn load_game<T: Transport + Clone, P: Provider<T, N>, N: Network>(
        config: &Config,
        blob_provider: &BlobProvider,
        game_instance: &KailuaGameInstance<T, P, N>,
    ) -> anyhow::Result<Self> {
        let index = game_instance.gameIndex().stall().await._0.to();
        let parent = game_instance
            .parentGameIndex()
            .stall()
            .await
            .parentGameIndex_;
        let proposer = game_instance.proposer().stall().await.proposer_;
        let created_at = game_instance.createdAt().stall().await._0;
        // fetch blob data
        let mut io_blobs = Vec::new();
        let mut io_field_elements = Vec::new();
        for _ in 0..config.proposal_blobs {
            let blob_kzg_hash = game_instance
                .proposalBlobHashes(U256::from(io_blobs.len()))
                .stall()
                .await
                ._0;
            let blob_data = blob_provider
                .get_blob(created_at, blob_kzg_hash)
                .await
                .context("get_blob")?;
            // save data
            let io_remaining = config.proposal_block_count - (io_field_elements.len() as u64) - 1;
            let io_in_blob = io_remaining.min(FIELD_ELEMENTS_PER_BLOB);
            io_field_elements.extend(intermediate_outputs(&blob_data, io_in_blob as usize)?);
            io_blobs.push((blob_kzg_hash, blob_data));
        }
        // claim data
        let output_root = game_instance.rootClaim().stall().await.rootClaim_.0.into();
        let output_block_number: u64 = game_instance
            .l2BlockNumber()
            .stall()
            .await
            .l2BlockNumber_
            .to();
        let l1_head = game_instance.l1Head().stall().await.l1Head_.0.into();
        Ok(Self {
            contract: *game_instance.address(),
            index,
            parent,
            proposer,
            created_at,
            io_blobs,
            io_field_elements,
            output_root,
            output_block_number,
            l1_head,
            children: Default::default(),
            survivor: None,
            contender: None,
            correct_io: repeat(None)
                .take((config.proposal_block_count - 1) as usize)
                .collect(),
            correct_claim: None,
            correct_parent: None,
            canonical: None,
        })
    }

    pub async fn fetch_parent_tournament_survivor<
        T: Transport + Clone,
        P: Provider<T, N>,
        N: Network,
    >(
        &self,
        provider: P,
    ) -> anyhow::Result<Option<Address>> {
        if !self.has_parent() {
            return Ok(None);
        }
        let parent_tournament: Address = self
            .tournament_contract_instance(&provider)
            .parentGame()
            .stall()
            .await
            .parentGame_;
        let parent_tournament_instance = KailuaTournament::new(parent_tournament, &provider);
        let survivor = parent_tournament_instance
            .pruneChildren()
            .call()
            .await?
            .survivor;
        if survivor.is_zero() {
            Ok(None)
        } else {
            Ok(Some(survivor))
        }
    }

    pub async fn fetch_parent_tournament_survivor_status<
        T: Transport + Clone,
        P: Provider<T, N>,
        N: Network,
    >(
        &self,
        provider: P,
    ) -> anyhow::Result<Option<bool>> {
        let survivor = self.fetch_parent_tournament_survivor(provider).await?;
        let is_survivor_expected = survivor.map(|survivor| survivor == self.contract);
        if !is_survivor_expected.unwrap_or_default() {
            warn!(
                "Current survivor: {survivor:?} (expecting {})",
                self.contract
            );
        } else {
            info!("Survivor: {}", self.contract);
        }
        Ok(survivor.map(|survivor| survivor == self.contract))
    }

    pub async fn fetch_finality<T: Transport + Clone, P: Provider<T, N>, N: Network>(
        &self,
        provider: P,
    ) -> anyhow::Result<Option<bool>> {
        Self::parse_finality(
            self.tournament_contract_instance(provider)
                .status()
                .stall()
                .await
                ._0,
        )
    }

    pub async fn fetch_current_challenger_duration<
        T: Transport + Clone,
        P: Provider<T, N>,
        N: Network,
    >(
        &self,
        provider: P,
    ) -> anyhow::Result<u64> {
        let chain_time = provider
            .get_block(
                BlockId::Number(BlockNumberOrTag::Latest),
                BlockTransactionsKind::Hashes,
            )
            .await
            .context("get_block")?
            .expect("Could not fetch latest L1 block")
            .header()
            .timestamp();
        Ok(self
            .tournament_contract_instance(provider)
            .getChallengerDuration(U256::from(chain_time))
            .stall()
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
        // Update parent status
        self.correct_parent = Some(is_correct_parent);
        // Check root claim correctness
        let local_claim = op_node_provider
            .output_at_block(self.output_block_number)
            .await
            .context("output_at_block")?;
        self.correct_claim = Some(local_claim == self.output_root);
        // Check intermediate output correctness for KailuaGame instances
        if self.has_parent() {
            let starting_block_number = self
                .output_block_number
                .saturating_sub(config.proposal_block_count);
            for (i, output_hash) in self.io_field_elements.iter().enumerate() {
                let io_number = starting_block_number + (i as u64) + 1;
                if let Ok(local_output) = op_node_provider.output_at_block(io_number).await {
                    self.correct_io[i] = Some(&hash_to_fe(local_output) == output_hash);
                } else {
                    error!("Could not get output hash {io_number} from op node");
                }
            }
        }
        // Return correctness
        Ok(self.is_correct())
    }

    pub fn is_correct(&self) -> Option<bool> {
        // False case
        if let Some(false) = self.correct_parent {
            return Some(false);
        }
        if let Some(false) = self.correct_claim {
            return Some(false);
        }
        if self.correct_io.iter().flatten().any(|c| !c) {
            return Some(false);
        }
        // Unknown case
        if self.correct_parent.is_none()
            || self.correct_claim.is_none()
            || self.correct_io.iter().any(|c| c.is_none())
        {
            return None;
        }
        // Correct!
        Some(true)
    }

    pub fn tournament_contract_instance<T: Transport + Clone, P: Provider<T, N>, N: Network>(
        &self,
        provider: P,
    ) -> KailuaTournamentInstance<T, P, N> {
        KailuaTournament::new(self.contract, provider)
    }

    pub async fn resolve<T: Transport + Clone, P: Provider<T, N>, N: Network>(
        &self,
        provider: P,
    ) -> anyhow::Result<N::ReceiptResponse> {
        self.tournament_contract_instance(provider)
            .resolve()
            .send()
            .await
            .context("KailuaTreasury::resolve (send)")?
            .get_receipt()
            .await
            .context("KailuaTreasury::resolve (get_receipt)")
    }

    pub fn has_parent(&self) -> bool {
        self.index != self.parent
    }

    pub fn divergence_point(&self, proposal: &Proposal) -> Option<usize> {
        // Check divergence in IO
        for i in 0..self.io_field_elements.len() {
            if self.io_field_elements[i] != proposal.io_field_elements[i] {
                return Some(i);
            }
        }
        // Check divergence in final claim
        if self.output_root != proposal.output_root {
            return Some(self.io_field_elements.len());
        }
        // Report equivalence
        None
    }

    pub fn wins_against(&self, proposal: &Proposal) -> bool {
        // todo: If the survivor hasn't been challenged for as long as the timeout, declare them winner
        match self.divergence_point(proposal) {
            // u wins if v is a duplicate
            None => true,
            // u wins if v is wrong (even if u is wrong)
            Some(point) => {
                if point < self.io_field_elements.len() {
                    !proposal.correct_io[point].unwrap()
                } else {
                    !proposal.correct_claim.unwrap()
                }
            }
        }
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

    pub fn has_precondition_for(&self, position: u64) -> bool {
        if position == self.io_field_elements.len() as u64 {
            false
        } else {
            // technically this can be > 1 instead
            (position % FIELD_ELEMENTS_PER_BLOB) > 0
        }
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
        let (proof, _) = blob_fe_proof(&io_blob.1.blob, position as usize)?;
        Ok(Bytes::from(proof.to_vec()))
    }

    pub fn output_at(&self, position: u64) -> B256 {
        self.io_field_elements
            .get(position as usize)
            .copied()
            .unwrap_or(self.output_root)
    }

    pub fn create_sidecar(io_field_elements: &[B256]) -> anyhow::Result<BlobTransactionSidecar> {
        let mut io_blobs = vec![];
        loop {
            let start = io_blobs.len() * FIELD_ELEMENTS_PER_BLOB as usize;
            if start >= io_field_elements.len() {
                break;
            }
            let end = (start + FIELD_ELEMENTS_PER_BLOB as usize).min(io_field_elements.len());
            let io_bytes = io_field_elements[start..end].concat();
            // Encode as blob sidecar
            let blob = Blob::right_padding_from(io_bytes.as_slice());
            io_blobs.push(blob);
        }
        blob_sidecar(io_blobs)
    }
}
