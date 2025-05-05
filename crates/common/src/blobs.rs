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
use alloy_rpc_types_beacon::sidecar::BlobData;
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
///
/// # Fields
///
/// * `block_ref` (`BlockInfo`):
///   A reference to the block metadata that the requested blob is associated with. This includes
///   relevant information about the block, ensuring the correct context for blob retrieval.
///
/// * `blob_hash` (`IndexedBlobHash`):
///   A unique hash identifying the specific blob to be fetched. This ensures the exact blob can
///   be located and retrieved.
///
/// # Derives
///
/// This struct derives the following traits:
///
/// - `Clone`: Allows creating a duplicate of `BlobFetchRequest`.
/// - `Debug`: Implements formatting for debugging purposes.
/// - `Serialize`: Enables serialization of the struct into formats such as JSON.
/// - `Deserialize`: Enables deserialization of the struct from serialized formats like JSON.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlobFetchRequest {
    /// Contains the block height, hash, timestamp, and parent hash.
    pub block_ref: BlockInfo,
    /// Represents the versioned hash of a blob, and its index in the slot.
    pub blob_hash: IndexedBlobHash,
}

/// The `BlobWitnessData` struct represents a data model for handling collections of blobs,
/// commitments, and proofs with efficient serialization and deserialization using the `rkyv`
/// framework. It leverages archive and map utilities from `rkyv` to optimize data storage and
/// retrieval while maintaining a seamless transition between in-memory and archived states.
///
/// This struct is designed for scenarios that require zero-copy deserialization or performant
/// serialization for complex or nested data types commonly used in distributed systems, blockchain
/// applications, or other high-performance computing domains.
///
/// # Derives
/// - `Clone`: Allows producing a copy of the `BlobWitnessData`.
/// - `Debug`: Implements formatting for debugging purposes.
/// - `Default`: Provides a default implementation for constructing an empty or zeroed `BlobWitnessData`.
/// - `Serialize` and `Deserialize`: Enable standard serialization and deserialization.
/// - `rkyv::Archive`, `rkyv::Serialize`, and `rkyv::Deserialize`: Optimize for archiving and
///   zero-copy use cases with the `rkyv` library.
///
/// # Fields
/// - `blobs`:
///   - A vector (`Vec<Blob>`) containing `Blob` objects.
///   - Serialized and deserialized using the `rkyv::with::Map` wrapper for `BlobDef`.
///   - Provides efficient handling of transformation between in-memory and serialized states, even for
///     complex nested types.
///   - **Usage**: Optimized for high-performance applications requiring custom serialization logic.
/// - `commitments`:
///   - A vector (`Vec<Bytes48>`) containing 48-byte commitment values.
///   - Serialized and deserialized using the `rkyv::with::Map` wrapper for `Bytes48Def`.
///   - **Usage**: Designed for securely and efficiently storing cryptographic commitments.
/// - `proofs`:
///   - A vector (`Vec<Bytes48>`) containing 48-byte proof values.
///   - Serialized and deserialized using the `rkyv::with::Map` wrapper for `Bytes48Def`.
///   - **Usage**: Useful for storing verification proofs efficiently in performance-critical systems.
#[derive(
    Clone, Debug, Default, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct BlobWitnessData {
    /// A collection of `Blob` objects serialized with `rkyv` using the `Map` wrapper for
    /// `BlobDef`. This configuration ensures that the `Blob` objects are serialized and
    /// deserialized correctly, efficiently handling their transformation between in-memory
    /// and archived states.
    ///
    /// # Attributes
    /// - `#[rkyv(with = rkyv::with::Map<BlobDef>)]`: Specifies that the `blobs` field should
    ///   be archived and deserialized with a specific mapping behavior, where `BlobDef` is
    ///   mapped appropriately using the `rkyv` framework.
    /// - `blobs`: A `Vec` (vector) containing instances of `Blob`.
    #[rkyv(with = rkyv::with::Map<BlobDef>)]
    pub blobs: Vec<Blob>,
    /// A vector of `Bytes48` elements that are archived using the `rkyv` crate.
    ///
    /// # Attributes
    /// - `commitments`: A `Vec<Bytes48>` which represents a collection of `Bytes48` values.
    /// This field uses the `rkyv` attribute for zero-copy serialization and deserialization.
    /// Specifically, it applies the custom serialization handler `Map<Bytes48Def>` for the `Bytes48` type,
    /// allowing for efficient archiving and retrieval of data.
    #[rkyv(with = rkyv::with::Map<Bytes48Def>)]
    pub commitments: Vec<Bytes48>,
    /// A structure to hold proofs, where each proof is represented as a `Bytes48` instance.
    ///
    /// This field uses the `rkyv` crate for zero-copy serialization and deserialization,
    /// applying the `rkyv::with` attribute with the `Map<Bytes48Def>` implementation to
    /// customize how the `Vec<Bytes48>` is serialized and deserialized.
    ///
    /// The `Bytes48` type presumably represents a 48-byte fixed size binary data structure,
    /// and the mapping allows for interoperability with the `rkyv` serialization framework.
    ///
    /// # Attributes
    /// - `#[rkyv(with = rkyv::with::Map<Bytes48Def>)]`:
    ///     Indicates that the `rkyv` crate will serialize or deserialize the `proofs` field
    ///     using the `Map<Bytes48Def>` attribute for each element in the vector.
    #[rkyv(with = rkyv::with::Map<Bytes48Def>)]
    pub proofs: Vec<Bytes48>,
}

/// A struct that provides preloaded blobs and their corresponding identifiers.
///
/// The `PreloadedBlobProvider` manages a collection of blobs, each identified by a unique hash.
/// It is useful when you want to preload a set of blobs in memory for quick access.
///
/// # Fields
/// - `entries`: A vector of tuples where each tuple contains:
///     - `B256`: The versioned hash that uniquely identifies the blob.
///     - `Blob`: The blob data associated with the identifier.
///
/// # Derives
/// - `Clone`: Allows the struct to be cloned.
/// - `Debug`: Implements debugging support for the struct, enabling formatted debug output.
/// - `Default`: Provides a default implementation that initializes an empty `entries` vector.
#[derive(Clone, Debug, Default)]
pub struct PreloadedBlobProvider {
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
    ///
    /// # Returns
    /// A new instance of `Self`, containing the validated and processed entries.
    fn from(value: BlobWitnessData) -> Self {
        let blobs = value
            .blobs
            .into_iter()
            .map(|b| c_kzg::Blob::new(b.0))
            .collect::<Vec<_>>();
        ethereum_kzg_settings(0)
            .verify_blob_kzg_proof_batch(
                blobs.as_slice(),
                value.commitments.as_slice(),
                value.proofs.as_slice(),
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

    /// Asynchronously retrieves blobs associated with the provided indexed blob hashes.
    ///
    /// This function fetches blobs from an internal storage, ensuring that the blob's hash
    /// matches the provided hash. The function logs the total number of blobs requested and
    /// verifies each blob before adding it to the result. If the hash matches, the blob is
    /// included in the response. The blobs are returned in the same order as the input hashes.
    ///
    /// # Parameters
    /// - `&mut self`: A mutable reference to the current struct instance, which holds the
    ///   internal state required for fetching blobs.
    /// - `_block_ref`: A reference to a `BlockInfo` structure, which can represent metadata or
    ///   context for the operation. (Currently unused in this function.)
    /// - `blob_hashes`: A slice of `IndexedBlobHash` objects that represent the hashes identifying
    ///   the blobs to be retrieved.
    ///
    /// # Returns
    /// - `Ok(Vec<Box<Blob>>)`:
    ///   A vector of boxed `Blob` instances if all blobs are successfully fetched and processed.
    /// - `Err(Self::Error)`:
    ///   An error result in case of any failure specific to the implementation.
    ///
    /// # Errors
    /// This function propagates the error of the implementing type if there is an issue during blob retrieval.
    ///
    /// # Notes
    /// - This function assumes that `self.entries` contains blob data structured as pairs of
    ///   `(blob_hash, blob)`. If the order or structure of entries changes, this function's
    ///   behavior must be adapted accordingly.
    /// - The `_block_ref` parameter is currently unused.
    ///
    /// # Performance
    /// - The memory allocation for the `Vec` is optimized by pre-allocating the capacity based on
    ///   the `blob_hashes.len()`.
    /// - The current implementation uses a simple pattern of `pop()` from `self.entries`,
    ///   which enforces a last-in-first-out (LIFO) approach that must be harmonized with
    ///   upstream expectations.
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

pub fn intermediate_outputs(blob_data: &BlobData, blocks: usize) -> anyhow::Result<Vec<U256>> {
    field_elements(blob_data, 0..blocks)
}

pub fn trail_data(blob_data: &BlobData, blocks: usize) -> anyhow::Result<Vec<U256>> {
    field_elements(blob_data, blocks..FIELD_ELEMENTS_PER_BLOB as usize)
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
/// - The underlying slice operation fails to produce a valid 32-byte array.
pub fn field_elements(
    blob_data: &BlobData,
    iterator: impl Iterator<Item = usize>,
) -> anyhow::Result<Vec<U256>> {
    let mut field_elements = vec![];
    for index in iterator.map(|i| 32 * i) {
        let bytes: [u8; 32] = blob_data.blob.0[index..index + 32].try_into()?;
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
