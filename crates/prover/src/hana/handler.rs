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

use alloy::eips::BlockId;
use alloy::providers::Provider;
use anyhow::bail;
use async_trait::async_trait;
use hana_blobstream::blobstream::blobstream_address;
use hana_blobstream::blobstream::SP1Blobstream::SP1BlobstreamInstance;
use hana_host::celestia::{CelestiaChainHintHandler, CelestiaChainHost};
use hana_oracle::hint::HintWrapper;
use kailua_sync::stall::Stall;
use kona_host::{HintHandler, OnlineHostBackendCfg, SharedKeyValueStore};
use kona_proof::Hint;

/// The [HintHandler] for the [CelestiaChainHost].
#[derive(Debug, Clone, Copy)]
pub struct HanaHintHandler;

#[async_trait]
impl HintHandler for HanaHintHandler {
    type Cfg = CelestiaChainHost;

    async fn fetch_hint(
        hint: Hint<<Self::Cfg as OnlineHostBackendCfg>::HintType>,
        cfg: &Self::Cfg,
        providers: &<Self::Cfg as OnlineHostBackendCfg>::Providers,
        kv: SharedKeyValueStore,
    ) -> anyhow::Result<()> {
        let HintWrapper::CelestiaDA = hint.ty else {
            return CelestiaChainHintHandler::fetch_hint(hint, cfg, providers, kv).await;
        };

        anyhow::ensure!(hint.data.len() == 40, "Invalid hint data length");
        let height = u64::from_le_bytes(hint.data[0..8].try_into().unwrap());
        let l1_provider = providers.l1();
        let chain_id = l1_provider.get_chain_id().await?;
        let blobstream_address = blobstream_address(chain_id)
            .expect("No canonical Blobstream address found for chain id");
        let blobstream_contract = SP1BlobstreamInstance::new(blobstream_address, l1_provider);
        let latest_height = blobstream_contract
            .latestBlock()
            .block(BlockId::Hash(cfg.single_host.l1_head.into()))
            .stall("SP1Blobstream::latestBlock")
            .await;
        // Early abort if l1 head is insufficient
        if latest_height < height {
            bail!(
                "SP1Blobstream::latestBlock={latest_height} < {height} at L1_HEAD={}",
                cfg.single_host.l1_head
            );
        }

        CelestiaChainHintHandler::fetch_hint(hint, cfg, providers, kv).await
    }
}
