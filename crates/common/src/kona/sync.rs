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

// This file is a modified copy of kona_proof::l1::sync

use crate::kona::chain::OracleL1ChainProvider;
use alloy_consensus::Header;
use alloy_primitives::{Sealed, B256};
use kona_derive::prelude::ChainProvider;
use kona_driver::{PipelineCursor, TipCursor};
use kona_genesis::RollupConfig;
use kona_preimage::CommsClient;
use kona_proof::errors::OracleProviderError;
use kona_proof::l2::OracleL2ChainProvider;
use kona_proof::FlushableCache;
use kona_protocol::BatchValidationProvider;
use spin::RwLock;
use std::fmt::Debug;
use std::sync::Arc;

/// Constructs a [`PipelineCursor`] from the caching oracle, boot info, and providers.
pub async fn new_pipeline_cursor<O>(
    rollup_config: &RollupConfig,
    safe_header: Sealed<Header>,
    chain_provider: &mut OracleL1ChainProvider<O>,
    l2_chain_provider: &mut OracleL2ChainProvider<O>,
) -> Result<Arc<RwLock<PipelineCursor>>, OracleProviderError>
where
    O: CommsClient + FlushableCache + FlushableCache + Send + Sync + Debug,
{
    let safe_head_info = l2_chain_provider
        .l2_block_info_by_number(safe_header.number)
        .await?;
    let l1_origin = chain_provider
        .block_info_by_number(safe_head_info.l1_origin.number)
        .await?;

    // Walk back the starting L1 block by `channel_timeout` to ensure that the full channel is
    // captured.
    let channel_timeout = rollup_config.channel_timeout(safe_head_info.block_info.timestamp);
    let mut l1_origin_number = l1_origin.number.saturating_sub(channel_timeout);
    if l1_origin_number < rollup_config.genesis.l1.number {
        l1_origin_number = rollup_config.genesis.l1.number;
    }
    let origin = chain_provider
        .block_info_by_number(l1_origin_number)
        .await?;

    // Construct the cursor.
    let mut cursor = PipelineCursor::new(channel_timeout, origin);
    let tip = TipCursor::new(safe_head_info, safe_header, B256::ZERO);
    cursor.advance(origin, tip);

    // Wrap the cursor in a shared read-write lock
    Ok(Arc::new(RwLock::new(cursor)))
}
