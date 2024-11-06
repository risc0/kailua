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

use alloy::eips::eip4844::kzg_to_versioned_hash;
use alloy::primitives::B256;
use alloy::providers::{Provider, ProviderBuilder, ReqwestProvider};
use alloy_rpc_types_beacon::sidecar::{BeaconBlobBundle, BlobData};
use anyhow::{bail, Context};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tracing::debug;

#[derive(Clone, Debug)]
pub struct BlobProvider {
    pub cl_node_provider: ReqwestProvider,
    pub genesis_time: u64,
    pub seconds_per_slot: u64,
}

impl BlobProvider {
    pub async fn new(url: &str) -> anyhow::Result<Self> {
        let cl_node_provider = ProviderBuilder::new().on_http(url.try_into()?);
        let genesis =
            Self::provider_get::<Value>(&cl_node_provider, "eth/v1/beacon/genesis").await?;
        debug!("genesis {:?}", &genesis);
        let genesis_time = genesis["data"]["genesis_time"]
            .as_str()
            .unwrap()
            .parse::<u64>()?;
        let spec = Self::provider_get::<Value>(&cl_node_provider, "eth/v1/config/spec").await?;
        debug!("spec {:?}", &spec);
        let seconds_per_slot = spec["data"]["SECONDS_PER_SLOT"]
            .as_str()
            .unwrap()
            .parse::<u64>()?;
        Ok(Self {
            cl_node_provider,
            genesis_time,
            seconds_per_slot,
        })
    }

    pub fn provider_url(provider: &ReqwestProvider) -> &str {
        provider.client().transport().url().trim_end_matches('/')
    }

    pub fn url(&self) -> &str {
        self.cl_node_provider
            .client()
            .transport()
            .url()
            .trim_end_matches('/')
    }

    pub fn slot(&self, timestamp: u64) -> u64 {
        (timestamp - self.genesis_time) / self.seconds_per_slot
    }

    pub async fn provider_get<T: DeserializeOwned>(
        provider: &ReqwestProvider,
        path: &str,
    ) -> anyhow::Result<T> {
        provider
            .client()
            .transport()
            .client()
            .get(format!("{}/{}", Self::provider_url(provider), path))
            .send()
            .await
            .context("get")?
            .json::<T>()
            .await
            .context("json")
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        Self::provider_get(&self.cl_node_provider, path).await
    }

    pub async fn get_blob(&self, timestamp: u64, blob_hash: B256) -> anyhow::Result<BlobData> {
        let slot = self.slot(timestamp);
        let blobs = self
            .get::<BeaconBlobBundle>(&format!("eth/v1/beacon/blob_sidecars/{slot}"))
            .await
            .context(format!("blob_sidecars {slot}"))?;

        for blob in blobs {
            let versioned_hash = kzg_to_versioned_hash(blob.kzg_commitment.as_slice());
            if versioned_hash == blob_hash {
                return Ok(blob);
            }
        }

        bail!("Blob {blob_hash} not found in block {timestamp}!");
    }
}
