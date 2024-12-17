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
use alloy::transports::Transport;
use kailua_contracts::KailuaGame::KailuaGameInstance;

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub treasury: Address,
    pub game: Address,
    pub verifier: Address,
    pub image_id: B256,
    pub cfg_hash: B256,
    pub proposal_block_count: u64,
    pub proposal_blobs: u64,
    pub game_type: u8,
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
            .stall()
            .await
            .treasury_;
        let game = *kailua_game_implementation.address();
        let verifier = kailua_game_implementation
            .verifier()
            .stall()
            .await
            .verifier_;
        let image_id = kailua_game_implementation.imageId().stall().await.imageId_;
        let cfg_hash = kailua_game_implementation
            .configHash()
            .stall()
            .await
            .configHash_;
        let proposal_block_count = kailua_game_implementation
            .proposalBlockCount()
            .stall()
            .await
            .proposalBlockCount_
            .to();
        let proposal_blobs = kailua_game_implementation
            .proposalBlobs()
            .stall()
            .await
            .proposalBlobs_
            .to();
        let game_type = kailua_game_implementation
            .gameType()
            .stall()
            .await
            .gameType_ as u8;
        let factory = kailua_game_implementation
            .disputeGameFactory()
            .stall()
            .await
            .factory_;
        let timeout = kailua_game_implementation
            .maxClockDuration()
            .stall()
            .await
            .maxClockDuration_;
        let genesis_time = kailua_game_implementation
            .genesisTimeStamp()
            .stall()
            .await
            .genesisTimeStamp_
            .to();
        let block_time = kailua_game_implementation
            .l2BlockTime()
            .stall()
            .await
            .l2BlockTime_
            .to();
        let proposal_gap = kailua_game_implementation
            .proposalTimeGap()
            .stall()
            .await
            .proposalTimeGap_
            .to();
        Ok(Self {
            treasury,
            game,
            verifier,
            image_id,
            cfg_hash,
            proposal_block_count,
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
}
