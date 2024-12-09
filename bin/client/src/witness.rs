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
use alloy::eips::eip4844::{kzg_to_versioned_hash, IndexedBlobHash};
use alloy_primitives::map::hash_set::HashSet;
use alloy_primitives::B256;
use async_trait::async_trait;
use kailua_common::blobs::BlobWitnessData;
use kailua_common::oracle::OracleWitnessData;
use kona_derive::prelude::BlobProvider;
use kona_preimage::errors::PreimageOracleResult;
use kona_preimage::{PreimageKey, PreimageOracleClient};
use op_alloy_protocol::BlockInfo;
use std::fmt::Debug;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub struct BlobWitnessProvider<T: BlobProvider> {
    pub provider: T,
    pub witness: Arc<Mutex<BlobWitnessData>>,
    pub cache: HashSet<B256>,
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
            let hash = kzg_to_versioned_hash(commitment.as_slice());
            if self.cache.contains(&hash) {
                continue;
            }
            self.cache.insert(hash);
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
pub struct OracleWitnessProvider<P: PreimageOracleClient + Send + Sync + Debug + Clone> {
    pub preimage_oracle: P,
    pub witness: Arc<Mutex<OracleWitnessData>>,
    pub cache: Arc<Mutex<HashSet<PreimageKey>>>,
}

impl<P> OracleWitnessProvider<P>
where
    P: PreimageOracleClient + Send + Sync + Debug + Clone,
{
    pub fn save(&self, key: PreimageKey, value: &[u8]) {
        let mut cache = self.cache.lock().unwrap();
        if !cache.contains(&key) {
            cache.insert(key);
            let mut witness = self.witness.lock().unwrap();
            witness.keys.push(key);
            witness.data.push(value.to_vec());
        }
    }
}

#[async_trait]
impl<P> PreimageOracleClient for OracleWitnessProvider<P>
where
    P: PreimageOracleClient + Send + Sync + Debug + Clone,
{
    async fn get(&self, key: PreimageKey) -> PreimageOracleResult<Vec<u8>> {
        let value = self.preimage_oracle.get(key).await?;
        self.save(key, &value);
        Ok(value)
    }

    async fn get_exact(&self, key: PreimageKey, buf: &mut [u8]) -> PreimageOracleResult<()> {
        self.preimage_oracle.get_exact(key, buf).await?;
        let value = buf.to_vec();
        self.save(key, &value);
        Ok(())
    }
}
