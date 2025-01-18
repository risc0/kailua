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

use alloy::consensus::Blob;
use alloy::eips::eip4844::IndexedBlobHash;
use async_trait::async_trait;
use kailua_common::blobs::BlobWitnessData;
use kailua_common::oracle::{validate_preimage, PreloadedOracle};
use kona_derive::prelude::BlobProvider;
use kona_preimage::errors::PreimageOracleResult;
use kona_preimage::{
    CommsClient, HintWriterClient, PreimageKey, PreimageKeyType, PreimageOracleClient,
};
use kona_proof::FlushableCache;
use maili_protocol::BlockInfo;
use std::fmt::Debug;
use std::sync::{Arc, Mutex};
use tracing::error;

#[derive(Clone, Debug)]
pub struct BlobWitnessProvider<T: BlobProvider> {
    pub provider: T,
    pub witness: Arc<Mutex<BlobWitnessData>>,
}

#[async_trait]
impl<T: BlobProvider + Send> BlobProvider for BlobWitnessProvider<T> {
    type Error = T::Error;

    async fn get_blobs(
        &mut self,
        block_ref: &BlockInfo,
        blob_hashes: &[IndexedBlobHash],
    ) -> Result<Vec<Box<Blob>>, Self::Error> {
        let blobs = self.provider.get_blobs(block_ref, blob_hashes).await?;
        for blob in &blobs {
            let c_kzg_blob = c_kzg::Blob::from_bytes(blob.as_slice()).unwrap();
            let settings = alloy::consensus::EnvKzgSettings::default();
            let commitment =
                c_kzg::KzgCommitment::blob_to_kzg_commitment(&c_kzg_blob, settings.get())
                    .expect("Failed to convert blob to commitment");
            let proof = c_kzg::KzgProof::compute_blob_kzg_proof(
                &c_kzg_blob,
                &commitment.to_bytes(),
                settings.get(),
            )
            .unwrap();
            let mut witness = self.witness.lock().unwrap();
            witness.blobs.push(Blob::from(*c_kzg_blob));
            witness.commitments.push(commitment.to_bytes());
            witness.proofs.push(proof.to_bytes());
        }
        Ok(blobs)
    }
}

#[derive(Clone, Debug)]
pub struct OracleWitnessProvider<P: CommsClient + FlushableCache + Send + Sync + Debug + Clone> {
    pub oracle: P,
    pub witness: Arc<Mutex<PreloadedOracle>>,
}

impl<P> OracleWitnessProvider<P>
where
    P: CommsClient + FlushableCache + Send + Sync + Debug + Clone,
{
    pub fn save(&self, key: PreimageKey, value: &[u8]) {
        if matches!(key.key_type(), PreimageKeyType::Blob) {
            return;
        }
        let mut witness = self.witness.lock().unwrap();
        validate_preimage(&key, value).expect("Attempted to save invalid preimage");
        witness.preimages.insert(key, value.to_vec());
    }
}

#[async_trait]
impl<P> PreimageOracleClient for OracleWitnessProvider<P>
where
    P: CommsClient + FlushableCache + Send + Sync + Debug + Clone,
{
    async fn get(&self, key: PreimageKey) -> PreimageOracleResult<Vec<u8>> {
        let res = self.oracle.get(key).await;
        if res.is_err() {
            error!("Oracle get failure: {:?}", res);
        }
        let value = res?;
        self.save(key, &value);
        Ok(value)
    }

    async fn get_exact(&self, key: PreimageKey, buf: &mut [u8]) -> PreimageOracleResult<()> {
        let res = self.oracle.get_exact(key, buf).await;
        if res.is_err() {
            error!("Oracle exact failure: {:?}", res);
        }
        res?;
        let value = buf.to_vec();
        self.save(key, &value);
        Ok(())
    }
}

#[async_trait]
impl<P> HintWriterClient for OracleWitnessProvider<P>
where
    P: CommsClient + FlushableCache + Send + Sync + Debug + Clone,
{
    async fn write(&self, hint: &str) -> PreimageOracleResult<()> {
        self.oracle.write(hint).await
    }
}

impl<P> FlushableCache for OracleWitnessProvider<P>
where
    P: CommsClient + FlushableCache + Send + Sync + Debug + Clone,
{
    fn flush(&self) {
        self.oracle.flush();
    }
}
