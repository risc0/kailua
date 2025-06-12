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
use crate::sync::provider::SyncProvider;
use crate::KAILUA_GAME_TYPE;
use alloy::primitives::{Address, B256};
use kailua_contracts::*;
use kona_genesis::RollupConfig;
use opentelemetry::global::tracer;
use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::Context;
use std::process::exit;
use tracing::{error, info, warn};

/// A collection of information about a Kailua deployment
#[derive(Clone, Debug, Default)]
pub struct SyncDeployment {
    /// Address of the KailuaTreasury contract
    pub treasury: Address,
    /// Address of the KailuaGame contract
    pub game: Address,
    /// Address of the RISC Zero verifier contract
    pub verifier: Address,
    /// Image ID of the FPVM program
    pub image_id: B256,
    /// Hash of the rollup configuration
    pub cfg_hash: B256,
    /// Number of output commitments published per proposal
    pub proposal_output_count: u64,
    /// Number of L2 blocks covered by each output commitment
    pub output_block_span: u64,
    /// Number of blobs published per proposal
    pub proposal_blobs: u64,
    /// Game type ID used (Default: 1337)
    pub game_type: u8,
    /// Address of the DisputeGameFactory contract
    pub factory: Address,
    /// Proposal dispute timeout
    pub timeout: u64,
    /// L2 Chain genesis block timestamp
    pub genesis_time: u64,
    /// L2 Chain block time
    pub block_time: u64,
}

impl SyncDeployment {
    pub async fn load(
        provider: &SyncProvider,
        config: &RollupConfig,
        game_impl_address: Option<Address>,
    ) -> anyhow::Result<Self> {
        let tracer = tracer("kailua");
        let context = Context::current_with_span(tracer.start("SyncDeployment::load"));

        // load system config
        let system_config =
            SystemConfig::new(config.l1_system_config_address, &provider.l1_provider);
        let dgf_address = system_config
            .disputeGameFactory()
            .stall_with_context(context.clone(), "SystemConfig::disputeGameFactory")
            .await;

        // Init registry and factory contracts
        let dispute_game_factory = IDisputeGameFactory::new(dgf_address, &provider.l1_provider);
        info!("DisputeGameFactory({:?})", dispute_game_factory.address());
        let game_count: u64 = dispute_game_factory
            .gameCount()
            .stall_with_context(context.clone(), "DisputeGameFactory::gameCount")
            .await
            .to();
        info!("There have been {game_count} games created using DisputeGameFactory");

        // Look up deployment to target
        let latest_game_impl_addr = dispute_game_factory
            .gameImpls(KAILUA_GAME_TYPE)
            .stall_with_context(context.clone(), "DisputeGameFactory::gameImpls")
            .await;
        let kailua_game_implementation_address = game_impl_address.unwrap_or(latest_game_impl_addr);
        if game_impl_address.is_some() {
            warn!("Using provided KailuaGame implementation {kailua_game_implementation_address}.");
        } else {
            info!("Using the latest KailuaGame implementation {kailua_game_implementation_address} from DisputeGameFactory.");
        }

        let kailua_game_implementation =
            KailuaGame::new(kailua_game_implementation_address, &provider.l1_provider);
        info!("KailuaGame({:?})", kailua_game_implementation.address());
        if kailua_game_implementation_address.is_zero() {
            error!("Fault proof game is not installed!");
            exit(1);
        }

        let treasury = kailua_game_implementation
            .KAILUA_TREASURY()
            .stall_with_context(context.clone(), "KailuaGame::KAILUA_TREASURY")
            .await;
        let game = *kailua_game_implementation.address();
        let verifier = kailua_game_implementation
            .RISC_ZERO_VERIFIER()
            .stall_with_context(context.clone(), "KailuaGame::RISC_ZERO_VERIFIER")
            .await;
        let image_id = kailua_game_implementation
            .FPVM_IMAGE_ID()
            .stall_with_context(context.clone(), "KailuaGame::FPVM_IMAGE_ID")
            .await;
        let cfg_hash = kailua_game_implementation
            .ROLLUP_CONFIG_HASH()
            .stall_with_context(context.clone(), "KailuaGame::ROLLUP_CONFIG_HASH")
            .await;
        let proposal_output_count = kailua_game_implementation
            .PROPOSAL_OUTPUT_COUNT()
            .stall_with_context(context.clone(), "KailuaGame::PROPOSAL_OUTPUT_COUNT")
            .await;
        let output_block_span = kailua_game_implementation
            .OUTPUT_BLOCK_SPAN()
            .stall_with_context(context.clone(), "KailuaGame::OUTPUT_BLOCK_SPAN")
            .await;
        let proposal_blobs = kailua_game_implementation
            .PROPOSAL_BLOBS()
            .stall_with_context(context.clone(), "KailuaGame::PROPOSAL_BLOBS")
            .await;
        let game_type = kailua_game_implementation
            .GAME_TYPE()
            .stall_with_context(context.clone(), "KailuaGame::GAME_TYPE")
            .await as u8;
        let factory = kailua_game_implementation
            .DISPUTE_GAME_FACTORY()
            .stall_with_context(context.clone(), "KailuaGame::DISPUTE_GAME_FACTORY")
            .await;
        let timeout = kailua_game_implementation
            .MAX_CLOCK_DURATION()
            .stall_with_context(context.clone(), "KailuaGame::MAX_CLOCK_DURATION")
            .await;
        let genesis_time = kailua_game_implementation
            .GENESIS_TIME_STAMP()
            .stall_with_context(context.clone(), "KailuaGame::GENESIS_TIME_STAMP")
            .await
            .to();
        let block_time = kailua_game_implementation
            .L2_BLOCK_TIME()
            .stall_with_context(context.clone(), "KailuaGame::L2_BLOCK_TIME")
            .await
            .to();
        Ok(Self {
            treasury,
            game,
            verifier,
            image_id,
            cfg_hash,
            proposal_output_count,
            output_block_span,
            proposal_blobs,
            game_type,
            factory,
            timeout,
            genesis_time,
            block_time,
        })
    }

    pub fn allows_proposal(&self, proposal_block_number: u64, proposal_time: u64) -> bool {
        self.time_to_propose(proposal_block_number, proposal_time) == 0
    }

    pub fn time_to_propose(&self, proposal_block_number: u64, proposal_time: u64) -> u64 {
        self.min_proposal_time(proposal_block_number)
            .saturating_sub(proposal_time)
    }

    pub fn min_proposal_time(&self, proposal_block_number: u64) -> u64 {
        self.genesis_time + proposal_block_number * self.block_time + 1
    }

    pub fn blocks_per_proposal(&self) -> u64 {
        self.proposal_output_count * self.output_block_span
    }
}
