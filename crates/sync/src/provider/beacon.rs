// Copyright 2024, 2025 RISC Zero, Inc.
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

use crate::{await_tel, retry_res_timeout};
use alloy::consensus::{Blob, BlobTransactionSidecar};
use alloy::eips::eip4844::kzg_to_versioned_hash;
use alloy::primitives::B256;
use alloy_rpc_types_beacon::sidecar::{BeaconBlobBundle, BlobData};
use anyhow::{bail, Context};
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tracing::debug;

#[derive(Clone, Debug)]
pub struct BlobProvider {
    pub cl_node_endpoint: String,
    pub client: Client,
    pub genesis_time: u64,
    pub seconds_per_slot: u64,
}

impl BlobProvider {
    pub async fn new(cl_node_endpoint: String) -> anyhow::Result<Self> {
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(tracer.start("BlobProvider::new"));

        let cl_node_endpoint = cl_node_endpoint.trim_end_matches('/').to_owned();
        let client = Client::new();

        let genesis = await_tel!(
            context,
            tracer,
            "BlobProvider::client_get (genesis)",
            retry_res_timeout!(
                10,
                Self::client_get::<Value>(&client, &cl_node_endpoint, "eth/v1/beacon/genesis")
                    .with_context(context.clone())
                    .await
            )
        );
        debug!("genesis {:?}", &genesis);
        let genesis_time = genesis["data"]["genesis_time"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("genesis time is None"))?
            .parse::<u64>()?;
        let spec = await_tel!(
            context,
            tracer,
            "BlobProvider::client_get (spec)",
            retry_res_timeout!(
                10,
                Self::client_get::<Value>(&client, &cl_node_endpoint, "eth/v1/config/spec")
                    .with_context(context.clone())
                    .await
            )
        );
        debug!("spec {:?}", &spec);
        let seconds_per_slot = spec["data"]["SECONDS_PER_SLOT"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("seconds per slot is None"))?
            .parse::<u64>()?;
        Ok(Self {
            cl_node_endpoint,
            client,
            genesis_time,
            seconds_per_slot,
        })
    }

    pub fn slot(&self, timestamp: u64) -> u64 {
        (timestamp - self.genesis_time) / self.seconds_per_slot
    }

    pub async fn client_get<T: DeserializeOwned>(
        client: &Client,
        endpoint: &str,
        path: &str,
    ) -> anyhow::Result<T> {
        let tracer = tracer("kailua");
        let context =
            opentelemetry::Context::current_with_span(tracer.start("BlobProvider::client_get"));

        client
            .get(format!("{endpoint}/{path}"))
            .send()
            .with_context(context.with_span(tracer.start_with_context("Client::send", &context)))
            .await
            .context("send")?
            .json::<T>()
            .with_context(context.with_span(tracer.start_with_context("Response::json", &context)))
            .await
            .context("json")
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        Self::client_get(&self.client, &self.cl_node_endpoint, path).await
    }

    pub async fn get_blob(&self, timestamp: u64, blob_hash: B256) -> anyhow::Result<BlobData> {
        let tracer = tracer("kailua");
        let context =
            opentelemetry::Context::current_with_span(tracer.start("BlobProvider::get_blob"));

        let slot = self.slot(timestamp);
        let blobs = await_tel!(
            context,
            tracer,
            "BlobProvider::get",
            retry_res_timeout!(
                10,
                self.get::<BeaconBlobBundle>(&format!("eth/v1/beacon/blob_sidecars/{slot}"))
                    .with_context(context.clone())
                    .await
            )
        );

        let blob_count = blobs.len();
        for blob in blobs {
            let versioned_hash = kzg_to_versioned_hash(blob.kzg_commitment.as_slice());
            if versioned_hash == blob_hash {
                return Ok(blob);
            }
        }

        bail!("Blob {blob_hash} @ {timestamp} not found in slot ({blob_count} blobs found)!");
    }
}

pub fn blob_sidecar(blob_data: Vec<Blob>) -> anyhow::Result<BlobTransactionSidecar> {
    let mut blobs = Vec::with_capacity(blob_data.len());
    let mut commitments = Vec::with_capacity(blob_data.len());
    let mut proofs = Vec::with_capacity(blob_data.len());
    let settings = alloy::consensus::EnvKzgSettings::default();
    for blob in blob_data {
        let c_kzg_blob = c_kzg::Blob::from_bytes(blob.as_slice())?;
        let commitment = settings
            .get()
            .blob_to_kzg_commitment(&c_kzg_blob)
            .expect("Failed to convert blob to commitment");
        let proof = settings
            .get()
            .compute_blob_kzg_proof(&c_kzg_blob, &commitment.to_bytes())?;
        blobs.push(blob);
        commitments.push(commitment.to_bytes().into_inner().into());
        proofs.push(proof.to_bytes().into_inner().into());
    }
    Ok(BlobTransactionSidecar::new(blobs, commitments, proofs))
}
