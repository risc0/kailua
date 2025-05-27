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

use crate::client::log;
use crate::rkyv::kzg::{BlobDef, Bytes48Def};
use alloy_eips::eip4844::{
    kzg_to_versioned_hash, Blob, IndexedBlobHash, BLS_MODULUS, FIELD_ELEMENTS_PER_BLOB,
};
use alloy_primitives::{B256, U256};
use async_trait::async_trait;
use c_kzg::{ethereum_kzg_settings, Bytes48};
use kona_derive::errors::BlobProviderError;
use kona_derive::traits::BlobProvider;
use kona_protocol::BlockInfo;
use serde::{Deserialize, Serialize};

/// A struct representing a request to fetch a specific blob based on its hash and associated block reference.
///
/// The `BlobFetchRequest` is used to request a specific blob by providing both the unique identifier
/// of the blob (`blob_hash`) and the block metadata (`block_ref`) it is associated with.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BlobFetchRequest {
    /// Contains the block height, hash, timestamp, and parent hash.
    pub block_ref: BlockInfo,
    /// Represents the versioned hash of a blob, and its index in the slot.
    pub blob_hash: IndexedBlobHash,
}

/// The `BlobWitnessData` struct represents a data model for handling collections of blobs,
/// commitments, and proofs with efficient serialization and deserialization using the `rkyv`
/// framework.
#[derive(
    Clone,
    Debug,
    Default,
    Eq,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub struct BlobWitnessData {
    /// A vector of `Blob` instances.
    #[rkyv(with = rkyv::with::Map<BlobDef>)]
    pub blobs: Vec<Blob>,
    /// A vector of `Bytes48` elements representing KZG commitments for each blob instance.
    #[rkyv(with = rkyv::with::Map<Bytes48Def>)]
    pub commitments: Vec<Bytes48>,
    /// A vector of `Bytes48` instances representing KZG blob proofs for each blob instance.
    #[rkyv(with = rkyv::with::Map<Bytes48Def>)]
    pub proofs: Vec<Bytes48>,
}

impl<T: Into<Blob>> From<Vec<T>> for BlobWitnessData {
    /// Converts a vector of blobs into a `BlobWitnessData` instance by processing each blob and
    /// generating corresponding KZG commitments and proofs.
    ///
    /// This function performs the following steps for each blob in the input vector:
    /// 1. Converts the blob into the required c-kzg crate `Blob` structure.
    /// 2. Uses the alloy-eips `EnvKzgSettings` to compute a KZG commitment for the blob.
    /// 3. Computes the KZG proof corresponding to the blob and its commitment.
    /// 4. Stores the processed blob, commitment, and proof in the resulting instance.
    ///
    /// # Parameters
    /// - `blobs`: A vector of elements of type `T` which hold the blob data to be processed. These
    ///   will be converted into `c_kzg::Blob` objects for KZG commitment and proof generation.
    ///
    /// # Assumptions
    /// - The implementation assumes that `Blob::from` and `Blob.into` facilitate the conversion
    ///   between the input data type and the required KZG-compatible `Blob` structure.
    ///
    /// # Panics
    /// - This function panics if the conversion of a blob to a KZG commitment fails.
    /// - Panics if proof computation fails.
    fn from(blobs: Vec<T>) -> Self {
        let mut result = Self::default();
        let settings = alloy_eips::eip4844::env_settings::EnvKzgSettings::default();
        let settings_ref = settings.get();
        for blob in blobs {
            let blob: Blob = blob.into();
            let c_kzg_blob = c_kzg::Blob::new(blob.0);
            let commitment = settings_ref
                .blob_to_kzg_commitment(&c_kzg_blob)
                .expect("Failed to convert blob to commitment");
            let proof = settings_ref
                .compute_blob_kzg_proof(&c_kzg_blob, &commitment.to_bytes())
                .unwrap();
            // save values
            result.blobs.push(Blob::from(*c_kzg_blob));
            result.commitments.push(commitment.to_bytes());
            result.proofs.push(proof.to_bytes());
        }
        result
    }
}

/// Provides preloaded blobs and their corresponding identifiers.
#[derive(Clone, Debug, Default)]
pub struct PreloadedBlobProvider {
    /// Pairs of blob hashes and their respective blob data.
    entries: Vec<(B256, Blob)>,
}

impl From<BlobWitnessData> for PreloadedBlobProvider {
    /// Converts a `BlobWitnessData` into a `PreloadedBlobProvider` by validating and processing its blobs, commitments,
    /// and proofs. This method performs KZG proof batch verification and then constructs a list of entries with hashed
    /// commitments and corresponding blobs.
    ///
    /// # Arguments
    /// - `value`: A `BlobWitnessData` instance containing the blobs, commitments, and associated proofs to be processed.
    ///
    /// # Panics
    /// - This function will panic if the KZG proof batch verification fails, with the message
    ///   "Failed to batch validate kzg proofs".
    ///
    /// # Process
    /// 1. Converts the blobs from the input into `c_kzg::Blob` type.
    /// 2. Performs a batch verification of KZG proofs using `ethereum_kzg_settings(0).verify_blob_kzg_proof_batch`
    ///    with the blobs, commitments, and proofs provided in the input.
    /// 3. Maps commitments into versioned hashes using `kzg_to_versioned_hash`.
    /// 4. Constructs entries by zipping the versioned hashes and blobs, then reverses the order of the resulting list.
    fn from(value: BlobWitnessData) -> Self {
        let blobs = value
            .blobs
            .into_iter()
            .map(|b| c_kzg::Blob::new(b.0))
            .collect::<Vec<_>>();
        assert!(
            ethereum_kzg_settings(0)
                .verify_blob_kzg_proof_batch(
                    blobs.as_slice(),
                    value.commitments.as_slice(),
                    value.proofs.as_slice(),
                )
                .expect("Failed to batch validate kzg proofs"),
            "Blob KZG proof batch verification failed"
        );

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

    /// Asynchronously retrieves blobs associated with the provided indexed blob hashes.
    ///
    /// This function fetches blobs from an internal storage, ensuring that the blob's hash
    /// matches the provided hash. The function logs the total number of blobs requested and
    /// verifies each blob before adding it to the result. If the hash matches, the blob is
    /// included in the response. The blobs are returned in the same order as the input hashes.
    ///
    /// # Parameters
    /// - `&mut self`: The internal state required for fetching blobs.
    /// - `_block_ref`: A reference to a `BlockInfo` structure, which can represent metadata or
    ///   context for the operation, but is unused in this function as the validation of the
    ///   inclusion of the requested blobs in the designated slots is assumed to have been performed.
    /// - `blob_hashes`: A slice of `IndexedBlobHash` objects that represent the hashes identifying
    ///   the blobs to be retrieved.
    ///
    /// # Returns
    /// - `Ok(Vec<Box<Blob>>)`:
    ///   A vector of boxed `Blob` instances if all blobs are successfully fetched and processed.
    /// - `Err(Self::Error)`:
    ///   An error result in case of any failure during blob retrieval.
    async fn get_blobs(
        &mut self,
        _block_ref: &BlockInfo,
        blob_hashes: &[IndexedBlobHash],
    ) -> Result<Vec<Box<Blob>>, Self::Error> {
        let blob_count = blob_hashes.len();
        log(&format!("FETCH {blob_count} BLOB(S)"));
        let mut blobs = Vec::with_capacity(blob_count);
        for hash in blob_hashes {
            let (blob_hash, blob) = self.entries.pop().unwrap();
            if hash.hash == blob_hash {
                blobs.push(Box::new(blob));
            }
        }
        Ok(blobs)
    }
}

/// Computes intermediate outputs from the provided blob data.
///
/// This function takes a reference to `BlobData` and a specified number of blocks
/// and computes the intermediate outputs as a vector of `U256` field elements.
///
/// # Arguments
///
/// * `blob` - A reference to the `Blob` structure containing the data to process.
/// * `blocks` - The number of blocks to use for computation.
pub fn intermediate_outputs(blob: impl AsRef<Blob>, blocks: usize) -> anyhow::Result<Vec<U256>> {
    field_elements(blob, 0..blocks)
}

/// Extracts a vector of field elements from the provided blob data within a specific range.
///
/// This function retrieves `FIELD_ELEMENTS_PER_BLOB` - `blocks` field elements from the given blob
/// data by extracting elements starting from the `blocks` index through the end of the blob data.
///
/// # Arguments
///
/// * `blob_data` - A reference to a `BlobData` structure containing the data to extract field elements from.
/// * `blocks` - The starting index for the extraction from the blob data.
pub fn trail_data(blob: impl AsRef<Blob>, blocks: usize) -> anyhow::Result<Vec<U256>> {
    field_elements(blob, blocks..FIELD_ELEMENTS_PER_BLOB as usize)
}

/// Extracts field elements from a given blob using specified indices.
///
/// This function processes a blob of data and extracts field elements
/// (represented by `U256`) based on the indices provided by the `iterator`.
/// For each index, it calculates the byte offset (index * 32), retrieves 32 bytes
/// from the blob, and converts them into a `U256` using big-endian interpretation.
///
/// # Arguments
///
/// * `blob_data` - A reference to a `BlobData` structure containing the blob
///   from which field elements will be extracted.
/// * `iterator` - An iterator of `usize` values, denoting the indices of
///   field elements to be extracted from the blob. Each index corresponds
///   to a 32-byte chunk.
///
/// # Returns
///
/// * `Ok(Vec<U256>)` - A vector containing the extracted field elements,
///   if the operation is successful.
/// * `Err(anyhow::Error)` - An error if any of the following occur:
///     - The index computation or slicing goes out of bounds.
///     - The byte slice cannot be converted into a `[u8; 32]`.
///
/// # Errors
///
/// This function will return an error in the following cases:
/// - The index calculated by `32 * i` exceeds the bounds of `blob_data.blob.0`.
pub fn field_elements(
    blob: impl AsRef<Blob>,
    iterator: impl Iterator<Item = usize>,
) -> anyhow::Result<Vec<U256>> {
    let mut field_elements = vec![];
    for index in iterator.map(|i| 32 * i) {
        let bytes: [u8; 32] = blob.as_ref().0[index..index + 32].try_into()?;
        field_elements.push(U256::from_be_bytes(bytes));
    }
    Ok(field_elements)
}

/// Converts a 256-bit hash (B256) into a field element (U256) within the bounds of a specific modulus (BLS_MODULUS).
///
/// # Arguments
/// - `hash` (`B256`): A 256-bit hash value represented as a struct containing an array of 32 bytes.
///
/// # Returns
/// - `U256`: A 256-bit unsigned integer representing the hash reduced modulo `BLS_MODULUS`.
///
/// # Behavior
/// - The function interprets the input hash as a big-endian byte sequence and converts it to a `U256` integer.
/// - It then reduces the resultant number modulo `BLS_MODULUS` to ensure it falls within the desired field range.
pub fn hash_to_fe(hash: B256) -> U256 {
    U256::from_be_bytes(hash.0).reduce_mod(BLS_MODULUS)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod tests {
    use super::*;
    use alloy_eips::eip4844::{BYTES_PER_BLOB, BYTES_PER_COMMITMENT, BYTES_PER_PROOF};
    use alloy_primitives::keccak256;
    use alloy_rpc_types_beacon::sidecar::BlobData;
    use rayon::prelude::*;
    use rkyv::rancor::Error;

    pub fn gen_blobs(count: usize) -> Vec<Blob> {
        (0..count)
            .map(|i| {
                (0..FIELD_ELEMENTS_PER_BLOB)
                    .map(|j| {
                        hash_to_fe(keccak256(format!("gen_blobs {i} {j}"))).to_be_bytes::<32>()
                    })
                    .collect::<Vec<_>>()
                    .concat()
                    .as_slice()
                    .try_into()
                    .unwrap()
            })
            .collect()
    }

    #[test]
    fn test_hash_to_fe() {
        for i in 0..1024 {
            let hash = keccak256(format!("test_hash_to_fe hash {i}"));
            let fe = hash_to_fe(hash);
            assert_eq!(fe, hash_to_fe(fe.to_be_bytes().into()));
        }
    }

    #[test]
    fn test_field_elements() {
        let blobs = gen_blobs(64);
        for (i, blob) in blobs.into_iter().enumerate() {
            let blob_data = BlobData {
                index: 0,
                blob: Box::new(blob),
                kzg_commitment: Default::default(),
                kzg_proof: Default::default(),
                signed_block_header: Default::default(),
                kzg_commitment_inclusion_proof: vec![],
            };
            let blocks = 64 * i;
            let recovered_bytes = [
                intermediate_outputs(&blob_data.blob, blocks).unwrap(),
                trail_data(&blob_data.blob, blocks).unwrap(),
            ]
            .concat()
            .into_iter()
            .map(|e| e.to_be_bytes::<32>())
            .collect::<Vec<_>>()
            .concat();
            assert_eq!(blob.0.as_slice(), recovered_bytes.as_slice());
        }
    }

    #[test]
    fn test_preloaded_blob_provider_tampering() {
        let witness_data = BlobWitnessData::from(gen_blobs(1));
        // Fail if any bit is wrong
        for i in 0..witness_data.blobs.len() {
            // Tamper with blob data
            (0..BYTES_PER_BLOB).into_par_iter().for_each(|j| {
                let mut tampered_witness_data = witness_data.clone();
                tampered_witness_data.blobs[i].0[j] ^= 1;

                assert_ne!(witness_data.blobs[i], tampered_witness_data.blobs[i]);
                let result =
                    std::panic::catch_unwind(|| PreloadedBlobProvider::from(tampered_witness_data));
                assert!(result.is_err());
            });
            // Tamper with commitment
            (0..BYTES_PER_COMMITMENT).into_par_iter().for_each(|j| {
                (0..8usize).into_par_iter().for_each(|k| {
                    let mut tampered_witness_data = witness_data.clone();
                    tampered_witness_data.commitments[i][j] ^= 1 << k;

                    assert_ne!(
                        witness_data.commitments[i],
                        tampered_witness_data.commitments[i]
                    );
                    let result = std::panic::catch_unwind(|| {
                        PreloadedBlobProvider::from(tampered_witness_data)
                    });
                    assert!(result.is_err());
                });
            });
            // Tamper with proof
            (0..BYTES_PER_PROOF).into_par_iter().for_each(|j| {
                (0..8usize).into_par_iter().for_each(|k| {
                    let mut tampered_witness_data = witness_data.clone();
                    tampered_witness_data.proofs[i][j] ^= 1 << k;

                    assert_ne!(witness_data.proofs[i], tampered_witness_data.proofs[i]);
                    let result = std::panic::catch_unwind(|| {
                        PreloadedBlobProvider::from(tampered_witness_data)
                    });
                    assert!(result.is_err());
                });
            });
        }
        // Succeed on genuine data
        let _ = PreloadedBlobProvider::from(witness_data);
    }

    #[tokio::test]
    async fn test_blob_provider() {
        let blobs = gen_blobs(32);
        let blob_witness_data = BlobWitnessData::from(blobs.clone());
        // serde
        let blob_witness_data = rkyv::from_bytes::<BlobWitnessData, Error>(
            rkyv::to_bytes::<Error>(&blob_witness_data)
                .unwrap()
                .as_ref(),
        )
        .unwrap();
        let indexed_hashes = blob_witness_data
            .commitments
            .iter()
            .map(|c| IndexedBlobHash {
                index: 0,
                hash: kzg_to_versioned_hash(c.as_slice()),
            })
            .collect::<Vec<_>>();
        let mut blob_provider = PreloadedBlobProvider::from(blob_witness_data);
        let retrieved = blob_provider
            .get_blobs(&Default::default(), &indexed_hashes)
            .await
            .unwrap()
            .into_iter()
            .map(|b| *b)
            .collect::<Vec<_>>();

        assert_eq!(blobs, retrieved);
    }

    #[tokio::test]
    async fn test_blob_provider_bad_query() {
        let blobs = gen_blobs(32);
        let blob_witness_data = BlobWitnessData::from(blobs.clone());
        // exhaust the provider and find nothing
        let indexed_hashes = blob_witness_data
            .commitments
            .iter()
            .map(|c| IndexedBlobHash {
                index: 0,
                hash: !kzg_to_versioned_hash(c.as_slice()), // invert the expected hash
            })
            .collect::<Vec<_>>();
        let mut blob_provider = PreloadedBlobProvider::from(blob_witness_data);
        let retrieved = blob_provider
            .get_blobs(&Default::default(), &indexed_hashes)
            .await
            .unwrap();

        assert!(retrieved.is_empty());
        assert!(blob_provider.entries.is_empty());
    }
}
