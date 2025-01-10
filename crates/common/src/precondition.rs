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

use crate::blobs::BlobFetchRequest;
use alloy_primitives::B256;
use risc0_zkvm::sha::{Impl as SHA2, Sha256};
use serde::{Deserialize, Serialize};
use std::iter::once;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PreconditionValidationData {
    Fault(u64, Box<[BlobFetchRequest; 2]>),
    Validity(u64, u64, Vec<BlobFetchRequest>),
}

impl PreconditionValidationData {
    pub fn to_vec(&self) -> Vec<u8> {
        pot::to_vec(self).unwrap()
    }

    pub fn hash(&self) -> B256 {
        let digest = *SHA2::hash_bytes(&self.to_vec());
        B256::from_slice(digest.as_bytes())
    }

    pub fn blob_fetch_requests(&self) -> &[BlobFetchRequest] {
        match self {
            PreconditionValidationData::Fault(_, requests) => requests.as_slice(),
            PreconditionValidationData::Validity(_, _, requests) => requests.as_slice(),
        }
    }

    pub fn precondition_hash(&self) -> B256 {
        match self {
            PreconditionValidationData::Fault(agreement_index, divergent_blobs) => {
                divergence_precondition_hash(
                    agreement_index,
                    &divergent_blobs[0].blob_hash.hash,
                    &divergent_blobs[1].blob_hash.hash,
                )
            }
            PreconditionValidationData::Validity(
                proposal_output_count,
                output_block_span,
                blobs,
            ) => equivalence_precondition_hash(
                proposal_output_count,
                output_block_span,
                blobs.iter().map(|b| &b.blob_hash.hash),
            ),
        }
    }
}

pub fn divergence_precondition_hash(
    agreement_index: &u64,
    contender_blob_hash: &B256,
    opponent_blob_hash: &B256,
) -> B256 {
    let agreement_index_bytes = agreement_index.to_be_bytes();
    let digest = *SHA2::hash_bytes(
        &[
            agreement_index_bytes.as_slice(),
            contender_blob_hash.as_slice(),
            opponent_blob_hash.as_slice(),
        ]
        .concat(),
    );
    B256::from_slice(digest.as_bytes())
}

pub fn equivalence_precondition_hash<'a>(
    proposal_output_count: &u64,
    output_block_span: &u64,
    blob_hashes: impl Iterator<Item = &'a B256>,
) -> B256 {
    let poc_bytes = proposal_output_count.to_be_bytes();
    let obs_bytes = output_block_span.to_be_bytes();
    let all_bytes = once(poc_bytes.as_slice())
        .chain(once(obs_bytes.as_slice()))
        .chain(blob_hashes.map(|h| h.as_slice()))
        .collect::<Vec<_>>()
        .concat();
    let digest = *SHA2::hash_bytes(&all_bytes);
    B256::from_slice(digest.as_bytes())
}
