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

use crate::stall::Stall;
use alloy::network::Network;
use alloy::primitives::{Address, B256};
use alloy::providers::Provider;
use kailua_contracts::KailuaGame::KailuaGameInstance;
use opentelemetry::global::tracer;
use opentelemetry::trace::{TraceContextExt, Tracer};

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub treasury: Address,
    pub game: Address,
    pub verifier: Address,
    pub image_id: B256,
    pub cfg_hash: B256,
    pub proposal_output_count: u64,
    pub output_block_span: u64,
    pub proposal_blobs: u64,
    pub game_type: u8,
    pub factory: Address,
    pub timeout: u64,
    pub genesis_time: u64,
    pub block_time: u64,
    pub proposal_gap: u64,
}

impl Config {
    pub async fn load<P: Provider<N>, N: Network>(
        kailua_game_implementation: &KailuaGameInstance<(), P, N>,
    ) -> anyhow::Result<Self> {
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(tracer.start("Config::load"));

        let treasury = kailua_game_implementation
            .KAILUA_TREASURY()
            .stall_with_context(context.clone(), "KailuaGame::KAILUA_TREASURY")
            .await
            ._0;
        let game = *kailua_game_implementation.address();
        let verifier = kailua_game_implementation
            .RISC_ZERO_VERIFIER()
            .stall_with_context(context.clone(), "KailuaGame::RISC_ZERO_VERIFIER")
            .await
            ._0;
        let image_id = kailua_game_implementation
            .FPVM_IMAGE_ID()
            .stall_with_context(context.clone(), "KailuaGame::FPVM_IMAGE_ID")
            .await
            ._0;
        let cfg_hash = kailua_game_implementation
            .ROLLUP_CONFIG_HASH()
            .stall_with_context(context.clone(), "KailuaGame::ROLLUP_CONFIG_HASH")
            .await
            ._0;
        let proposal_output_count = kailua_game_implementation
            .PROPOSAL_OUTPUT_COUNT()
            .stall_with_context(context.clone(), "KailuaGame::PROPOSAL_OUTPUT_COUNT")
            .await
            ._0
            .to();
        let output_block_span = kailua_game_implementation
            .OUTPUT_BLOCK_SPAN()
            .stall_with_context(context.clone(), "KailuaGame::OUTPUT_BLOCK_SPAN")
            .await
            ._0
            .to();
        let proposal_blobs = kailua_game_implementation
            .PROPOSAL_BLOBS()
            .stall_with_context(context.clone(), "KailuaGame::PROPOSAL_BLOBS")
            .await
            ._0
            .to();
        let game_type = kailua_game_implementation
            .GAME_TYPE()
            .stall_with_context(context.clone(), "KailuaGame::GAME_TYPE")
            .await
            ._0 as u8;
        let factory = kailua_game_implementation
            .DISPUTE_GAME_FACTORY()
            .stall_with_context(context.clone(), "KailuaGame::DISPUTE_GAME_FACTORY")
            .await
            ._0;
        let timeout = kailua_game_implementation
            .MAX_CLOCK_DURATION()
            .stall_with_context(context.clone(), "KailuaGame::MAX_CLOCK_DURATION")
            .await
            ._0;
        let genesis_time = kailua_game_implementation
            .GENESIS_TIME_STAMP()
            .stall_with_context(context.clone(), "KailuaGame::GENESIS_TIME_STAMP")
            .await
            ._0
            .to();
        let block_time = kailua_game_implementation
            .L2_BLOCK_TIME()
            .stall_with_context(context.clone(), "KailuaGame::L2_BLOCK_TIME")
            .await
            ._0
            .to();
        let proposal_gap = kailua_game_implementation
            .PROPOSAL_TIME_GAP()
            .stall_with_context(context.clone(), "KailuaGame::PROPOSAL_TIME_GAP")
            .await
            ._0
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
            proposal_gap,
        })
    }

    pub fn allows_proposal(&self, proposal_block_number: u64, proposal_time: u64) -> bool {
        proposal_time >= self.min_proposal_time(proposal_block_number)
    }

    pub fn min_proposal_time(&self, proposal_block_number: u64) -> u64 {
        self.genesis_time + proposal_block_number * self.block_time + self.proposal_gap + 1
    }

    pub fn blocks_per_proposal(&self) -> u64 {
        self.proposal_output_count * self.output_block_span
    }
}
