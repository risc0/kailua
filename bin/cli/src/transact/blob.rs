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

use crate::retry_res_timeout;
use alloy::consensus::{Blob, BlobTransactionSidecar};
use alloy::eips::eip4844::{kzg_to_versioned_hash, BLS_MODULUS, FIELD_ELEMENTS_PER_BLOB};
use alloy::primitives::{B256, U256};
use alloy_rpc_types_beacon::sidecar::{BeaconBlobBundle, BlobData};
use anyhow::{bail, Context};
use kailua_client::await_tel;
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::ops::{Div, Sub};
use tracing::debug;

#[derive(Clone, Debug)]
pub struct BlobProvider {
    pub cl_node_endpoint: String,
    pub client: reqwest::Client,
    pub genesis_time: u64,
    pub seconds_per_slot: u64,
}

impl BlobProvider {
    pub async fn new(cl_node_endpoint: String) -> anyhow::Result<Self> {
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(tracer.start("BlobProvider::new"));

        let cl_node_endpoint = cl_node_endpoint.trim_end_matches('/').to_owned();
        let client = reqwest::Client::new();

        let genesis = await_tel!(
            context,
            tracer,
            "BlobProvider::client_get (genesis)",
            retry_res_timeout!(
                Self::client_get::<Value>(&client, &cl_node_endpoint, "eth/v1/beacon/genesis")
                    .with_context(context.clone())
                    .await
            )
        );
        debug!("genesis {:?}", &genesis);
        let genesis_time = genesis["data"]["genesis_time"]
            .as_str()
            .unwrap()
            .parse::<u64>()?;
        let spec = await_tel!(
            context,
            tracer,
            "BlobProvider::client_get (spec)",
            retry_res_timeout!(
                Self::client_get::<Value>(&client, &cl_node_endpoint, "eth/v1/config/spec")
                    .with_context(context.clone())
                    .await
            )
        );
        debug!("spec {:?}", &spec);
        let seconds_per_slot = spec["data"]["SECONDS_PER_SLOT"]
            .as_str()
            .unwrap()
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
        client: &reqwest::Client,
        endpoint: &str,
        path: &str,
    ) -> anyhow::Result<T> {
        let tracer = tracer("kailua");
        let context =
            opentelemetry::Context::current_with_span(tracer.start("BlobProvider::client_get"));

        client
            .get(format!("{}/{}", endpoint, path))
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

pub fn reverse_bits(index: u128, order_po2: u32) -> u128 {
    index.reverse_bits() >> (u128::BITS - order_po2)
}

pub const PRIMITIVE_ROOT_OF_UNITY: U256 = U256::from_limbs([7, 0, 0, 0]);
// primitive_root = 7
// bls_mod = 52435875175126190479447740508185965837690552500527637822603658699938581184513
// pow(primitive_root, (bls_mod - 1) // (2 ** 12), bls_mod)
// 39033254847818212395286706435128746857159659164139250548781411570340225835782
pub const FE_ORDER_PO2: u32 = 12;

pub fn root_of_unity(index: usize) -> U256 {
    let primitive_root_exponent = BLS_MODULUS
        .sub(U256::from(1))
        .div(U256::from(FIELD_ELEMENTS_PER_BLOB));
    let root = PRIMITIVE_ROOT_OF_UNITY.pow_mod(primitive_root_exponent, BLS_MODULUS);
    let root_exponent = reverse_bits(index as u128, FE_ORDER_PO2);
    root.pow_mod(U256::from(root_exponent), BLS_MODULUS)
}

pub fn blob_fe_proof(
    blob: &Blob,
    index: usize,
) -> anyhow::Result<(c_kzg::Bytes48, c_kzg::Bytes32)> {
    let bytes = root_of_unity(index).to_be_bytes();
    let z = c_kzg::Bytes32::new(bytes);
    let c_kzg_blob = c_kzg::Blob::from_bytes(blob.as_slice())?;
    let settings = alloy::consensus::EnvKzgSettings::default();
    let (proof, value) = settings.get().compute_kzg_proof(&c_kzg_blob, &z)?;

    let commitment = settings.get().blob_to_kzg_commitment(&c_kzg_blob)?;

    let proof_bytes = proof.to_bytes();
    if settings
        .get()
        .verify_kzg_proof(&commitment.to_bytes(), &z, &value, &proof_bytes)?
    {
        Ok((proof_bytes, value))
    } else {
        bail!("Generated invalid kzg proof.")
    }
}
