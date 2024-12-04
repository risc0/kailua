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

use crate::oracle::BlobFetchRequest;
use alloy_primitives::B256;
use risc0_zkvm::sha::{Impl as SHA2, Sha256};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreconditionValidationData {
    pub validated_blobs: [BlobFetchRequest; 2],
}

impl PreconditionValidationData {
    pub fn to_vec(&self) -> Vec<u8> {
        pot::to_vec(self).unwrap()
    }

    pub fn hash(&self) -> B256 {
        let digest = *SHA2::hash_bytes(&self.to_vec());
        B256::from_slice(digest.as_bytes())
    }

    pub fn precondition_hash(&self) -> B256 {
        precondition_hash(
            &self.validated_blobs[0].blob_hash.hash,
            &self.validated_blobs[1].blob_hash.hash,
        )
    }
}

pub fn precondition_hash(contender: &B256, proposal: &B256) -> B256 {
    let digest = *SHA2::hash_bytes(&[contender.as_slice(), proposal.as_slice()].concat());
    B256::from_slice(digest.as_bytes())
}
