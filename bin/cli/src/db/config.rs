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

use alloy::network::Network;
use alloy::primitives::{Address, B256};
use alloy::providers::Provider;
use alloy::transports::Transport;
use anyhow::Context;
use kailua_contracts::IAnchorStateRegistry::IAnchorStateRegistryInstance;
use kailua_contracts::KailuaGame::KailuaGameInstance;

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub treasury: Address,
    pub verifier: Address,
    pub image_id: B256,
    pub cfg_hash: B256,
    pub proposal_block_count: u64,
    pub proposal_blobs: u64,
    pub game_type: u8,
    pub registry: Address,
    pub factory: Address,
    pub timeout: u64,
    pub genesis_time: u64,
    pub block_time: u64,
    pub proposal_gap: u64,
}

impl Config {
    pub async fn load<T: Transport + Clone, P: Provider<T, N>, N: Network>(
        kailua_game_implementation: &KailuaGameInstance<T, P, N>,
    ) -> anyhow::Result<Self> {
        let treasury = kailua_game_implementation
            .treasury()
            .call()
            .await
            .context("treasury")?
            .treasury_;
        let verifier = kailua_game_implementation
            .verifier()
            .call()
            .await
            .context("verifier")?
            .verifier_;
        let image_id = kailua_game_implementation
            .imageId()
            .call()
            .await
            .context("image_id")?
            .imageId_;
        let cfg_hash = kailua_game_implementation
            .configHash()
            .call()
            .await
            .context("config_hash")?
            .configHash_;
        let proposal_block_count = kailua_game_implementation
            .proposalBlockCount()
            .call()
            .await
            .context("proposal_block_count")?
            .proposalBlockCount_
            .to();
        let proposal_blobs = kailua_game_implementation
            .proposalBlobs()
            .call()
            .await
            .context("proposal_blobs")?
            .proposalBlobs_
            .to();
        let game_type = kailua_game_implementation
            .gameType()
            .call()
            .await
            .context("game_type")?
            .gameType_ as u8;
        let registry = kailua_game_implementation
            .anchorStateRegistry()
            .call()
            .await
            .context("registry")?
            .registry_;
        let factory =
            IAnchorStateRegistryInstance::new(registry, kailua_game_implementation.provider())
                .disputeGameFactory()
                .call()
                .await
                .context("dispute_game_factory")?
                ._0;
        let timeout = kailua_game_implementation
            .maxClockDuration()
            .call()
            .await
            .context("max_clock_duration")?
            .maxClockDuration_;
        let genesis_time = kailua_game_implementation
            .genesisTimeStamp()
            .call()
            .await
            .context("genesis_time")?
            .genesisTimeStamp_
            .to();
        let block_time = kailua_game_implementation
            .l2BlockTime()
            .call()
            .await
            .context("block_time")?
            .l2BlockTime_
            .to();
        let proposal_gap = kailua_game_implementation
            .proposalTimeGap()
            .call()
            .await
            .context("proposal_gap")?
            .proposalTimeGap_
            .to();
        Ok(Self {
            treasury,
            verifier,
            image_id,
            cfg_hash,
            proposal_block_count,
            proposal_blobs,
            game_type,
            registry,
            factory,
            timeout,
            genesis_time,
            block_time,
            proposal_gap,
        })
    }
}
