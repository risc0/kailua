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

use alloy_eips::eip4844::{kzg_to_versioned_hash, Blob, IndexedBlobHash, BYTES_PER_BLOB};
use alloy_primitives::B256;
use alloy_rpc_types_beacon::sidecar::BlobData;
use async_trait::async_trait;
use c_kzg::{ethereum_kzg_settings, Bytes48};
use kona_derive::errors::BlobProviderError;
use kona_derive::traits::BlobProvider;
use op_alloy_protocol::BlockInfo;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlobFetchRequest {
    pub block_ref: BlockInfo,
    pub blob_hash: IndexedBlobHash,
}

#[derive(
    Clone, Debug, Default, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct BlobWitnessData {
    #[rkyv(with = rkyv::with::Map<BlobDef>)]
    pub blobs: Vec<Blob>,
    #[rkyv(with = rkyv::with::Map<Bytes48Def>)]
    pub commitments: Vec<Bytes48>,
    #[rkyv(with = rkyv::with::Map<Bytes48Def>)]
    pub proofs: Vec<Bytes48>,
}

#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = Blob)]
#[rkyv(archived = ArchivedBlob)]
struct BlobDef(pub [u8; BYTES_PER_BLOB]);

impl From<BlobDef> for Blob {
    fn from(value: BlobDef) -> Self {
        Self(value.0)
    }
}

#[derive(
    rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Copy, Clone, Hash, PartialEq, Eq,
)]
#[rkyv(remote = Bytes48)]
#[rkyv(archived = ArchivedBytes48)]
pub struct Bytes48Def {
    #[rkyv(getter = get_48_bytes)]
    bytes: [u8; 48usize],
}

fn get_48_bytes(value: &Bytes48) -> [u8; 48] {
    value.into_inner()
}

impl From<Bytes48Def> for Bytes48 {
    fn from(value: Bytes48Def) -> Self {
        Self::from(value.bytes)
    }
}

#[derive(Clone, Debug, Default)]
pub struct PreloadedBlobProvider {
    entries: Vec<(B256, Blob)>,
}

impl From<BlobWitnessData> for PreloadedBlobProvider {
    fn from(value: BlobWitnessData) -> Self {
        let blobs = value
            .blobs
            .into_iter()
            .map(|b| c_kzg::Blob::new(b.0))
            .collect::<Vec<_>>();
        c_kzg::KzgProof::verify_blob_kzg_proof_batch(
            blobs.as_slice(),
            value.commitments.as_slice(),
            value.proofs.as_slice(),
            ethereum_kzg_settings(),
        )
        .expect("Failed to batch validate kzg proofs");
        let hashes = value
            .commitments
            .iter()
            .map(|c| kzg_to_versioned_hash(c.as_slice()))
            .collect::<Vec<_>>();
        let entries = core::iter::zip(hashes, blobs.into_iter().map(|b| Blob::from(*b)))
            .rev()
            .collect::<Vec<_>>();
        Self { entries }
    }
}

#[async_trait]
impl BlobProvider for PreloadedBlobProvider {
    type Error = BlobProviderError;

    async fn get_blobs(
        &mut self,
        _block_ref: &BlockInfo,
        blob_hashes: &[IndexedBlobHash],
    ) -> Result<Vec<Box<Blob>>, Self::Error> {
        let mut blobs = Vec::with_capacity(blob_hashes.len());
        for hash in blob_hashes {
            let (blob_hash, blob) = self.entries.pop().unwrap();
            if hash.hash == blob_hash {
                blobs.push(Box::new(blob));
            }
        }
        Ok(blobs)
    }
}

pub fn intermediate_outputs(blob_data: &BlobData, blocks: usize) -> anyhow::Result<Vec<B256>> {
    let mut outputs = vec![];
    for i in 0..blocks {
        let index = 32 * i;
        let bytes: [u8; 32] = blob_data.blob.0[index..index + 32].try_into()?;
        outputs.push(B256::from(bytes));
    }
    Ok(outputs)
}

pub fn hash_to_fe(mut hash: B256) -> B256 {
    hash.0[0] &= u8::MAX >> 2;
    hash
}
