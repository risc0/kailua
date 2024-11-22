use crate::db::config::Config;
use crate::db::treasury::Treasury;
use crate::db::ProofStatus;
use crate::providers::beacon::hash_to_fe;
use crate::providers::beacon::BlobProvider;
use crate::providers::optimism::OpNodeProvider;
use alloy::eips::eip4844::FIELD_ELEMENTS_PER_BLOB;
use alloy::eips::{BlockId, BlockNumberOrTag};
use alloy::network::primitives::BlockTransactionsKind;
use alloy::network::{BlockResponse, HeaderResponse, Network};
use alloy::primitives::{Address, B256, U256};
use alloy::providers::Provider;
use alloy::transports::Transport;
use alloy_rpc_types_beacon::sidecar::BlobData;
use anyhow::{bail, Context};
use kailua_common::intermediate_outputs;
use kailua_contracts::KailuaGame::KailuaGameInstance;
use kailua_contracts::KailuaTournament::KailuaTournamentInstance;
use kailua_contracts::KailuaTreasury::KailuaTreasuryInstance;
use std::collections::HashMap;
use std::iter::repeat;

#[derive(Clone, Debug)]
pub struct Proposal {
    // pointers
    pub contract: Address,
    pub index: u64,
    pub parent: u64,
    // claim data
    pub created_at: u64,
    pub io_blobs: Vec<(B256, BlobData)>,
    pub io_hashes: Vec<B256>,
    pub output_root: B256,
    pub output_block_number: u64,
    pub l1_head: B256,
    // tournament data
    pub children: Vec<u64>,
    pub proven: HashMap<u64, ProofStatus>,
    pub prover: HashMap<u64, Address>,
    pub survivor: Option<Address>,
    // io assessment
    pub correct_io: Vec<Option<bool>>,
    pub correct_claim: Option<bool>,
    pub is_correct_parent: Option<bool>,
    // resolution
    pub finality: Option<bool>,
}

impl Proposal {
    pub async fn load_treasury<T: Transport + Clone, P: Provider<T, N>, N: Network>(
        config: &Config,
        treasury: &Treasury,
        treasury_instance: &KailuaTreasuryInstance<T, P, N>,
    ) -> anyhow::Result<Self> {
        let created_at = treasury_instance
            .createdAt()
            .call()
            .await
            .context("created_at")?
            ._0;
        // claim data
        let output_root = treasury_instance
            .rootClaim()
            .call()
            .await
            .context("root_claim")?
            .rootClaim_
            .0
            .into();
        let output_block_number = treasury_instance
            .l2BlockNumber()
            .call()
            .await
            .context("l2_block_number")?
            .l2BlockNumber_
            .to();
        let l1_head = treasury_instance
            .l1Head()
            .call()
            .await
            .context("l1_head")?
            .l1Head_
            .0
            .into();
        // finality
        let mut proposal = Self {
            contract: config.treasury,
            index: treasury.index,
            parent: treasury.index,
            created_at,
            io_blobs: vec![],
            io_hashes: vec![],
            output_root,
            output_block_number,
            l1_head,
            children: Default::default(),
            proven: Default::default(),
            prover: Default::default(),
            survivor: None,
            correct_io: vec![],
            correct_claim: None,
            is_correct_parent: Some(true),
            finality: None,
        };
        proposal
            .fetch_finality(treasury_instance.provider())
            .await?;
        Ok(proposal)
    }

    pub async fn load_game<T: Transport + Clone, P: Provider<T, N>, N: Network>(
        config: &Config,
        game_instance: &KailuaGameInstance<T, P, N>,
        blob_provider: &BlobProvider,
    ) -> anyhow::Result<Self> {
        let index = game_instance
            .gameIndex()
            .call()
            .await
            .context("game_index")?
            ._0
            .to();
        let parent = game_instance
            .parentGameIndex()
            .call()
            .await
            .context("parent_game_index")?
            .parentGameIndex_;
        let created_at = game_instance
            .createdAt()
            .call()
            .await
            .context("created_at")?
            ._0;
        // fetch blob data
        let mut io_blobs = Vec::new();
        let mut io_hashes = Vec::new();
        for _ in 0..config.proposal_blobs {
            let blob_kzg_hash = game_instance
                .proposalBlobHashes(U256::from(io_blobs.len()))
                .call()
                .await
                .context("proposal_blob_hashes")?
                ._0;
            let blob_data = blob_provider
                .get_blob(created_at, blob_kzg_hash)
                .await
                .context("get_blob")?;
            // save data
            let io_remaining = config.proposal_block_count - (io_hashes.len() as u64) - 1;
            let io_in_blob = io_remaining.max(FIELD_ELEMENTS_PER_BLOB);
            io_hashes.extend(intermediate_outputs(&blob_data, io_in_blob as usize)?);
            io_blobs.push((blob_kzg_hash, blob_data));
        }
        // claim data
        let output_root = game_instance
            .rootClaim()
            .call()
            .await
            .context("root_claim")?
            .rootClaim_
            .0
            .into();
        let output_block_number: u64 = game_instance
            .l2BlockNumber()
            .call()
            .await
            .context("l2_block_number")?
            .l2BlockNumber_
            .to();
        let l1_head = game_instance
            .l1Head()
            .call()
            .await
            .context("l1_head")?
            .l1Head_
            .0
            .into();
        // finality
        let mut proposal = Self {
            contract: *game_instance.address(),
            index,
            parent,
            created_at,
            io_blobs,
            io_hashes,
            output_root,
            output_block_number,
            l1_head,
            children: Default::default(),
            proven: Default::default(),
            prover: Default::default(),
            survivor: None,
            correct_io: repeat(None)
                .take((config.proposal_block_count - 1) as usize)
                .collect(),
            correct_claim: None,
            is_correct_parent: None,
            finality: None,
        };
        proposal.fetch_finality(game_instance.provider()).await?;
        Ok(proposal)
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
            .call()
            .await
            .context("parent_game")?
            .parentGame_;
        let parent_tournament_instance =
            KailuaTournamentInstance::new(parent_tournament, &provider);
        let survivor = parent_tournament_instance
            .pruneChildren()
            .call()
            .await
            .context("prune_children")?
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
        Ok(self
            .fetch_parent_tournament_survivor(provider)
            .await?
            .map(|survivor| survivor == self.contract))
    }

    pub async fn fetch_finality<T: Transport + Clone, P: Provider<T, N>, N: Network>(
        &mut self,
        provider: P,
    ) -> anyhow::Result<Option<bool>> {
        self.finality = Self::parse_finality(
            self.tournament_contract_instance(provider)
                .status()
                .call()
                .await
                .context("status")?
                ._0,
        )?;
        Ok(self.finality)
    }

    pub async fn fetch_current_challenger_duration<
        T: Transport + Clone,
        P: Provider<T, N>,
        N: Network,
    >(
        &mut self,
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
            .call()
            .await
            .context("get_challenger_duration")?
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
        self.is_correct_parent = Some(is_correct_parent);
        // Check root claim correctness
        let local_claim = op_node_provider
            .output_at_block(self.output_block_number)
            .await
            .context("output_at_block")?;
        self.correct_claim = Some(local_claim == self.output_root);
        // Check intermediate output correctness for KailuaGame instances
        if self.index != self.parent {
            let starting_block_number = self
                .output_block_number
                .saturating_sub(config.proposal_block_count);
            for (i, output_hash) in self.io_hashes.iter().enumerate() {
                let io_number = starting_block_number + (i as u64) + 1;
                if let Ok(local_output) = op_node_provider.output_at_block(io_number).await {
                    self.correct_io[io_number as usize] =
                        Some(&hash_to_fe(local_output) == output_hash);
                }
            }
        }
        // Return correctness
        Ok(self.is_correct())
    }

    pub fn is_correct(&self) -> Option<bool> {
        // False case
        if let Some(false) = self.is_correct_parent {
            return Some(false);
        }
        if let Some(false) = self.correct_claim {
            return Some(false);
        }
        if self.correct_io.iter().flatten().any(|c| !c) {
            return Some(false);
        }
        // Unknown case
        if self.is_correct_parent.is_none()
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
        KailuaTournamentInstance::new(self.contract, provider)
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
}
