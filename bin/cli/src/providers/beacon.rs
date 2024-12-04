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

use alloy::consensus::{Blob, BlobTransactionSidecar};
use alloy::eips::eip4844::{kzg_to_versioned_hash, BLS_MODULUS, FIELD_ELEMENTS_PER_BLOB};
use alloy::primitives::{B256, U256};
use alloy::providers::{Provider, ProviderBuilder, ReqwestProvider};
use alloy_rpc_types_beacon::sidecar::{BeaconBlobBundle, BlobData};
use anyhow::{bail, Context};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::ops::{Div, Sub};
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

pub fn blob_sidecar(blob_data: Vec<Blob>) -> anyhow::Result<BlobTransactionSidecar> {
    let mut blobs = Vec::with_capacity(blob_data.len());
    let mut commitments = Vec::with_capacity(blob_data.len());
    let mut proofs = Vec::with_capacity(blob_data.len());
    for blob in blob_data {
        let c_kzg_blob = c_kzg::Blob::from_bytes(blob.as_slice())?;
        let settings = alloy::consensus::EnvKzgSettings::default();
        let commitment = c_kzg::KzgCommitment::blob_to_kzg_commitment(&c_kzg_blob, settings.get())
            .expect("Failed to convert blob to commitment");
        let proof = c_kzg::KzgProof::compute_blob_kzg_proof(
            &c_kzg_blob,
            &commitment.to_bytes(),
            settings.get(),
        )?;
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
    let (proof, value) = c_kzg::KzgProof::compute_kzg_proof(&c_kzg_blob, &z, settings.get())?;

    let commitment = c_kzg::KzgCommitment::blob_to_kzg_commitment(&c_kzg_blob, settings.get())?;

    let proof_bytes = proof.to_bytes();
    if c_kzg::KzgProof::verify_kzg_proof(
        &commitment.to_bytes(),
        &z,
        &value,
        &proof_bytes,
        settings.get(),
    )? {
        Ok((proof_bytes, value))
    } else {
        bail!("Generated invalid kzg proof.")
    }
}
