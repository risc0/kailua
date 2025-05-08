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

use crate::blobs::{hash_to_fe, BlobFetchRequest};
use alloy_eips::eip4844::{Blob, FIELD_ELEMENTS_PER_BLOB};
use alloy_primitives::B256;
use anyhow::bail;
use kona_derive::prelude::BlobProvider;
use kona_preimage::{CommsClient, PreimageKey, PreimageKeyType};
use kona_proof::errors::OracleProviderError;
use risc0_zkvm::sha::{Impl as SHA2, Sha256};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt::Debug;
use std::iter::once;
use std::sync::Arc;

/// Represents the data required to validate the output roots published in a proposal.
///
/// # Variants
///
/// - `Validity`:
///   Contains information required to verify a validity proof for a proposal:
///   - `proposal_l2_head_number`: Represents the block height of the starting l2 root of the proposal.
///   - `proposal_output_count`: Represents the number of output roots expected in the proposal.
///   - `output_block_span`: Represents the number of blocks covered by each output root.
///   - `blob_hashes`: A list of `BlobFetchRequest` instances, one for each blob published in the proposal.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PreconditionValidationData {
    Validity {
        proposal_l2_head_number: u64,
        proposal_output_count: u64,
        output_block_span: u64,
        blob_hashes: Vec<BlobFetchRequest>,
    },
}

impl PreconditionValidationData {
    /// Converts the current instance of the object into a `Vec<u8>` (a vector of bytes).
    ///
    /// This function serializes the `self` object using the `pot::to_vec` method and
    /// returns the resulting byte representation. The serialization process is expected
    /// to succeed, and any errors during the process will cause the function to panic.
    ///
    /// # Returns
    /// A `Vec<u8>` containing the serialized byte representation of the object.
    ///
    /// # Panics
    /// This function will panic if the `pot::to_vec` method returns an error during serialization,
    /// which is then unwrapped.
    pub fn to_vec(&self) -> Vec<u8> {
        pot::to_vec(self).unwrap()
    }

    /// Computes the hash of the current object using the SHA-256 algorithm.
    ///
    /// This method converts the object into its vector representation, hashes it
    /// using the `SHA2::hash_bytes` function, and then returns the result as a `B256` type.
    ///
    /// # Returns
    /// * `B256` - The 256-bit hash of the object generated using the SHA-256 algorithm.
    ///
    /// # Notes
    /// * It is assumed that the object implementing this function has a `to_vec` method
    ///   which represents the object as a byte vector.
    /// * This hash cannot be used to authenticate the precondition, but may be used to
    ///   reference the `PreconditionValidationData` instance in storage.
    pub fn hash(&self) -> B256 {
        let digest = *SHA2::hash_bytes(&self.to_vec());
        B256::from_slice(digest.as_bytes())
    }

    /// This method provides access to the `BlobFetchRequest` objects
    /// contained within the `PreconditionValidationData::Validity` variant.
    ///
    /// # Panics
    ///
    /// This method will panic if called on a variant of
    /// `PreconditionValidationData` other than `Validity`.
    pub fn blob_fetch_requests(&self) -> &[BlobFetchRequest] {
        match self {
            PreconditionValidationData::Validity {
                proposal_l2_head_number: _,
                proposal_output_count: _,
                output_block_span: _,
                blob_hashes: requests,
            } => requests.as_slice(),
        }
    }

    /// This function retrieves the `blob_hash` associated with each blob fetch request
    /// and computes a consolidated hash using the `blobs_hash` function.
    ///
    /// # Returns
    ///
    /// * `B256` - The computed hash value representing all blob fetch requests.
    pub fn blobs_hash(&self) -> B256 {
        blobs_hash(self.blob_fetch_requests().iter().map(|b| &b.blob_hash.hash))
    }

    /// Computes the precondition hash for the current instance of `PreconditionValidationData`.
    ///
    /// # Returns
    /// A `B256` value representing the computed precondition hash.
    ///
    /// # Process
    /// - For a `PreconditionValidationData::Validity` variant, the method extracts its components:
    ///   - `proposal_l2_head_number`: A reference to the global Layer 2 head number.
    ///   - `proposal_output_count`: A reference to the count of proposal outputs.
    ///   - `output_block_span`: A reference to the output block span.
    ///   - `blobs`: A reference to a list of blobs.
    /// - It then calculates the `blobs_hash` using the hashes of individual blobs in the list.
    /// - The final precondition hash is derived by invoking the `equivalence_precondition_hash`
    ///   function with the above components.
    ///
    /// # Note
    /// This method assumes the `PreconditionValidationData` is in the `Validity` variant. If no other
    /// variants are expected or added, this function will only operate on the relevant data structure.
    pub fn precondition_hash(&self) -> B256 {
        match self {
            PreconditionValidationData::Validity {
                proposal_l2_head_number,
                proposal_output_count,
                output_block_span,
                blob_hashes: blobs,
            } => validity_precondition_hash(
                proposal_l2_head_number,
                proposal_output_count,
                output_block_span,
                blobs_hash(blobs.iter().map(|b| &b.blob_hash.hash)),
            ),
        }
    }
}

/// This function calculates a 256-bit hash that uniquely represents the precondition
/// for a particular state transition in a Layer 2 scaling solution. The hash is
/// computed based on the provided global L2 head number, the proposal output count,
/// the block span, and a hash of the associated data blobs. It uses the SHA-256
/// hashing algorithm to ensure the integrity of the state information.
///
/// # Parameters
/// - `proposal_l2_head_number`: A reference to a `u64` representing the current L2 head
///   block number in the rollup.
/// - `proposal_output_count`: A reference to a `u64` indicating the count of outputs
///   in the proposed block transition.
/// - `output_block_span`: A reference to a `u64` that represents the block range or
///   span covered by each output in the proposal.
/// - `blobs_hash`: A `B256` hash representing the combined contents or metadata
///   of data blobs associated with the proposal.
///
/// # Returns
/// A `B256` hash, which is the computed precondition hash that captures the state
/// transition requirements.
///
/// # Implementation
/// 1. Convert the `proposal_l2_head_number`, `proposal_output_count`, and
///    `output_block_span` to big-endian byte representations.
/// 2. Concatenate these byte arrays with the bytes of the `blobs_hash`.
/// 3. Hash the resulting concatenated byte array using the SHA-256 hashing algorithm.
/// 4. Return the resulting 256-bit hash as a `B256` type.
pub fn validity_precondition_hash(
    proposal_l2_head_number: &u64,
    proposal_output_count: &u64,
    output_block_span: &u64,
    blobs_hash: B256,
) -> B256 {
    let phn_bytes = proposal_l2_head_number.to_be_bytes();
    let poc_bytes = proposal_output_count.to_be_bytes();
    let obs_bytes = output_block_span.to_be_bytes();
    let all_bytes = once(phn_bytes.as_slice())
        .chain(once(poc_bytes.as_slice()))
        .chain(once(obs_bytes.as_slice()))
        .chain(once(blobs_hash.as_slice()))
        .collect::<Vec<_>>()
        .concat();
    let digest = *SHA2::hash_bytes(&all_bytes);
    B256::from_slice(digest.as_bytes())
}

/// Computes a single hash from an iterator of hashes.
///
/// This function accepts an iterator of references to `B256` hashes, concatenates their byte
/// representations, and computes a SHA-256 hash of the concatenated bytes. The resulting hash
/// is returned as a `B256`.
///
/// # Type Parameters
/// - `'a`: The lifetime of the references contained in the iterator.
///
/// # Parameters
/// - `blob_hashes`: An iterator over references to `B256` hashes.
///   Each hash is converted to its byte slice, concatenated with others,
///   and then hashed to produce the result.
///
/// # Returns
/// - `B256`: A new `B256` value representing the SHA-256 hash of the concatenated hash bytes.
///
/// # Example
/// ```
/// use alloy_primitives::B256;
/// use kailua_common::precondition::blobs_hash;
///
/// let hash1 = B256::from_slice(&[0u8; 32]);
/// let hash2 = B256::from_slice(&[1u8; 32]);
/// let hash3 = B256::from_slice(&[2u8; 32]);
///
/// let combined_hash = blobs_hash(vec![&hash1, &hash2, &hash3].into_iter());
/// ```
///
/// In the example above, the `combined_hash` is computed by concatenating `hash1`, `hash2`,
/// and `hash3` into a single byte sequence and then hashing it using SHA-256.
///
/// # Notes
/// - The function assumes that the input `blob_hashes` iterator contains valid `B256` hashes
///   and that the resulting concatenated byte vector does not exceed memory limitations.
///
/// # Dependencies
/// - This function relies on the `SHA2` hashing utility and the `B256` type for handling
///   byte-level data and hash construction.
///
/// # Panics
/// - This function does not explicitly handle cases where memory allocation for the concatenated
///   byte vector fails, potentially causing a panic.
pub fn blobs_hash<'a>(blob_hashes: impl Iterator<Item = &'a B256>) -> B256 {
    let blobs_hash_bytes = blob_hashes
        .map(|h| h.as_slice())
        .collect::<Vec<_>>()
        .concat();
    let digest = *SHA2::hash_bytes(&blobs_hash_bytes);
    B256::from_slice(digest.as_bytes())
}

/// This function retrieves and deserializes the precondition validation data from an oracle and fetches the associated blobs
/// necessary for further processing. If the `precondition_data_hash` is zero, the function will return `None`.
///
/// # Type Parameters
/// - `O`: Represents the oracle client type. It must implement the `CommsClient` trait and be `Send`, `Sync`, and `Debug`.
/// - `B`: Represents the blob provider type. It must implement the `BlobProvider` trait and be `Send`, `Sync`, `Debug`, and `Clone`.
///
/// # Parameters
/// - `precondition_data_hash`: A hash of type `B256` representing the identifier of the precondition data to load.
/// - `oracle`: An `Arc`-wrapped oracle that implements the `CommsClient`, used to retrieve the precondition validation data.
/// - `beacon`: A mutable reference to an object implementing the `BlobProvider` used for fetching blob data.
///
/// # Returns
/// Returns a `Result` containing:
/// - `Some((PreconditionValidationData, Vec<Blob>))` if the precondition data and blobs are successfully loaded.
/// - `None` if the `precondition_data_hash` is zero (indicating no data needs to be loaded).
///
/// If an error occurs during the data fetching or deserialization process, it will return an error wrapped in `anyhow::Result`.
///
/// # Errors
/// - Returns an error if there is an issue while retrieving the precondition validation data from the oracle.
/// - Returns an error if deserialization of the data fails.
/// - Returns an error if there is a problem fetching blobs from the blob provider.
///
/// # Notes
/// - The `precondition_data_hash` must not be zero if data needs to be loaded.
pub async fn load_precondition_data<
    O: CommsClient + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
>(
    precondition_data_hash: B256,
    oracle: Arc<O>,
    beacon: &mut B,
) -> anyhow::Result<Option<(PreconditionValidationData, Vec<Blob>)>>
where
    <B as BlobProvider>::Error: Debug,
{
    if precondition_data_hash.is_zero() {
        return Ok(None);
    }
    // Read the blob references to fetch
    let precondition_validation_data: PreconditionValidationData = pot::from_slice(
        &oracle
            .get(PreimageKey::new(
                *precondition_data_hash,
                PreimageKeyType::Sha256,
            ))
            .await
            .map_err(OracleProviderError::Preimage)?,
    )?;
    let mut blobs = Vec::new();
    // Read the blobs to validate divergence
    for request in precondition_validation_data.blob_fetch_requests() {
        blobs.push(
            *beacon
                .get_blobs(&request.block_ref, &[request.blob_hash.clone()])
                .await
                .unwrap()[0],
        );
    }

    Ok(Some((precondition_validation_data, blobs)))
}

/// Validates the precondition data against the provided output roots, blobs,
/// and local/global layer-2 (L2) head block numbers.
///
/// This function performs multiple checks to ensure the integrity and consistency
/// of the precondition data. If any validation rules are violated, errors are returned.
///
/// # Parameters
///
/// - `precondition_validation_data`:
///   The data encapsulating the precondition hash and other information
///   necessary to validate the blocks. These represent the validity or state
///   against which the blocks or outputs will be checked.
///
/// - `blobs`:
///   A vector of blobs that hold intermediate output roots, structured
///   in a specific manner for validation purposes. Each blob consists of
///   multiple 32-byte chunks holding a field element for each published output root.
///
/// - `proof_l2_head_number`:
///   The proof L2 head block number, which represents the current state of the locally
///   agreed-upon highest L2 block in the current proof.
///
/// - `output_roots`:
///   A slice of cryptographic hashes (B256) representing the expected output
///   roots in a proposal.
///
/// # Returns
///
/// - `Ok(B256)`:
///   Returns the precondition hash if all validations pass successfully.
///
/// - `Err(anyhow::Error)`:
///   Returns an error if there are any mismatches or violations within the precondition
///   validations, including value mismatches, out-of-bound conditions, or invalid data.
///
/// # Validation Steps
///
/// 1. **Block Range Verification**:
///    - Ensures that the `proposal_l2_head_number` is less than or equal to
///      the `proof_l2_head_number`. If the proposal L2 head number is ahead
///      of that of the current proof, validation fails.
///
/// 2. **Output Root Checks**:
///    - Skips validation if `output_roots` is empty.
///    - Verifies each output block root:
///      - Ensures that the block number does not exceed the maximum block number
///        derived from the proposed output root claim.
///      - Validates only blocks that are multiples of the specified `output_block_span`.
///
/// 3. **Blob Integrity Validation**:
///    - Checks to ensure the field elements (fe) derived from blobs correspond to
///      the expected field elements calculated from the output roots.
///    - For the last output:
///      - Ensures that the trail (remaining) blob data contains zeroed-out bytes,
///        indicating no unexpected data after the meaningful field elements.
///
/// 4. **Assertions**:
///    - If an inconsistency is logically impossible given the inputs, it indicates a
///      programming or internal invariant violation, and the function panics.
///
/// # Behavior
///
/// - For each output block root, the corresponding blob data is compared to ensure it
///   matches the field element representation of the hash.
/// - In case of mismatching field element values, the specific error points to the
///   exact field position, blob index, and block number where the mismatch occurs.
///
/// # Errors
///
/// - When the `proposal_l2_head_number` exceeds the `proof_l2_head_number`.
/// - When field element mismatches are detected between the calculated values and
///   the provided blob data.
/// - When unexpected non-zero trail values are encountered in the blobs.
/// - If an output block number exceeds the maximum allowed block number.
///
/// # Notes
///
/// - This function is critical for ensuring the integrity of L2 precondition proposal
///   validations by comparing locally constructed outputs to globally proposed data.
/// - It is assumed that the blob data structure properly aligns with the
///   `FIELD_ELEMENTS_PER_BLOB` constant and the specific use case.
///
pub fn validate_precondition(
    precondition_validation_data: PreconditionValidationData,
    blobs: Vec<Blob>,
    proof_l2_head_number: u64,
    output_roots: &[B256],
) -> anyhow::Result<B256> {
    let precondition_hash = precondition_validation_data.precondition_hash();
    match precondition_validation_data {
        PreconditionValidationData::Validity {
            proposal_l2_head_number,
            proposal_output_count,
            output_block_span,
            blob_hashes: _,
        } => {
            // Ensure local and global block ranges match
            if proposal_l2_head_number > proof_l2_head_number {
                bail!(
                    "Validity precondition proposal starting block #{} > proof agreed l2 head #{}",
                    proposal_l2_head_number,
                    proof_l2_head_number
                )
            } else if output_roots.is_empty() {
                // abort early if no validation is to take place
                return Ok(precondition_hash);
            }
            // Calculate blob index pointer
            let max_block_number =
                proposal_l2_head_number + proposal_output_count * output_block_span;
            for (i, output_hash) in output_roots.iter().enumerate() {
                let output_block_number = proof_l2_head_number + i as u64 + 1;
                if output_block_number > max_block_number {
                    // We should not derive outputs beyond the proposal root claim
                    bail!("Output block #{output_block_number} > max block #{max_block_number}.");
                }
                let offset = output_block_number - proposal_l2_head_number;
                if offset % output_block_span != 0 {
                    // We only check equivalence every output_block_span blocks
                    continue;
                }
                let intermediate_output_offset = (offset / output_block_span) - 1;
                let blob_index = (intermediate_output_offset / FIELD_ELEMENTS_PER_BLOB) as usize;
                let fe_position = (intermediate_output_offset % FIELD_ELEMENTS_PER_BLOB) as usize;
                let blob_fe_index = 32 * fe_position;
                // Verify fe equivalence to computed outputs for all but last output
                match intermediate_output_offset.cmp(&(proposal_output_count - 1)) {
                    Ordering::Less => {
                        // verify equivalence to blob
                        let blob_fe_slice = &blobs[blob_index][blob_fe_index..blob_fe_index + 32];
                        let output_fe = hash_to_fe(*output_hash);
                        let output_fe_bytes = output_fe.to_be_bytes::<32>();
                        if blob_fe_slice != output_fe_bytes.as_slice() {
                            bail!(
                                "Bad fe #{} in blob {} for block #{}: Expected {} found {} ",
                                fe_position,
                                blob_index,
                                output_block_number,
                                B256::try_from(output_fe_bytes.as_slice())?,
                                B256::try_from(blob_fe_slice)?
                            );
                        }
                    }
                    Ordering::Equal => {
                        // verify zeroed trail data
                        if blob_index != blobs.len() - 1 {
                            bail!(
                                "Expected trail data to begin at blob {blob_index}/{}",
                                blobs.len()
                            );
                        } else if blobs[blob_index][blob_fe_index..].iter().any(|b| b != &0u8) {
                            bail!("Found non-zero trail data in blob {blob_index} after {blob_fe_index}");
                        }
                    }
                    Ordering::Greater => {
                        // (output_block_number <= max_block_number) implies:
                        // (output_offset <= proposal_output_count)
                        unreachable!(
                            "Output offset {intermediate_output_offset} > output count {proposal_output_count}."
                        );
                    }
                }
            }
        }
    }
    // Return the precondition hash
    Ok(precondition_hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blobs::tests::gen_blobs;
    use crate::blobs::BlobWitnessData;
    use alloy_eips::eip4844::{kzg_to_versioned_hash, IndexedBlobHash, BYTES_PER_BLOB};

    #[test]
    fn test_validate_precondition() {
        let mut blobs = gen_blobs(1);
        for i in 3 * 32..BYTES_PER_BLOB {
            blobs[0][i] = 0;
        }
        let blobs_witness = BlobWitnessData::from(blobs);
        let blobs_hashes = blobs_witness
            .commitments
            .iter()
            .map(|c| kzg_to_versioned_hash(c.as_slice()))
            .collect::<Vec<_>>();
        let blobs_fetch_requests = blobs_hashes
            .iter()
            .copied()
            .map(|hash| BlobFetchRequest {
                block_ref: Default::default(),
                blob_hash: IndexedBlobHash { index: 0, hash },
            })
            .collect::<Vec<_>>();

        let precondition_validation_data = PreconditionValidationData::Validity {
            proposal_l2_head_number: 1,
            proposal_output_count: 4,
            output_block_span: 4,
            blob_hashes: blobs_fetch_requests,
        };

        // test serde
        {
            let recoded =
                pot::from_slice(precondition_validation_data.to_vec().as_slice()).unwrap();
            assert_eq!(precondition_validation_data, recoded);
        }

        let output_roots: Vec<B256> = (0..3)
            .flat_map(|i| {
                vec![
                    blobs_witness.blobs[0][i * 32..(i + 1) * 32]
                        .try_into()
                        .unwrap();
                    4
                ]
            })
            .collect();

        assert_eq!(
            precondition_validation_data.precondition_hash(),
            validate_precondition(
                precondition_validation_data,
                blobs_witness.blobs,
                1,
                &output_roots
            )
            .unwrap()
        );
    }
}
