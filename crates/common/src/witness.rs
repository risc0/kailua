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

use crate::blobs::BlobWitnessData;
use crate::executor::Execution;
use crate::journal::ProofJournal;
use crate::oracle::vec::VecOracle;
use crate::rkyv::primitives::{AddressDef, B256Def};
use alloy_primitives::{Address, B256};
use kona_preimage::{CommsClient, PreimageKey};
use kona_proof::FlushableCache;
use risc0_zkvm::Receipt;
use std::fmt::Debug;

/// Represents the complete structure of a `Witness`, which is used to hold
/// the necessary data for authenticating a rollup state transition in the FPVM.
///
/// This structure is designed to work with the `rkyv` framework for zero-copy (de)serialization
/// and enables the Witness to archive, serialize, and deserialize its fields efficiently.
///
/// # Generics
/// `O` - A type that implements the `WitnessOracle` trait, defining functionality for a witness oracle.
///
/// # Fields
/// - `oracle_witness: O`
///   The witness oracle that holds preloaded preimage data in memory.
///
/// - `stream_witness: O`
///   The witness oracle for on-demand streamed preimage data.
///
/// - `blobs_witness: BlobWitnessData`
///   Stores witness data specifically associated with blobs.
///
/// - `payout_recipient_address: Address`
///   The address associated with the payout recipient of the proof execution.
///   - Annotated with `#[rkyv(with = AddressDef)]` to define a custom serialization/deserialization process using the `AddressDef` configuration.
///
/// - `precondition_validation_data_hash: B256`
///   A cryptographic hash value used for loading and validating preconditions before execution.
///   - Annotated with `#[rkyv(with = B256Def)]` for customized handling via the `B256Def` serializer/deserializer in `rkyv`.
///
/// - `stitched_executions: Vec<Vec<Execution>>`
///   A collection of stitched execution groups, represented as a two-dimensional vector.
///   - The outer vector (`Vec`) groups multiple executions that are logically stitched together.
///   - Each inner vector (`Vec<Execution>`) contains a continuous sequence of individual `Execution` instances, sorted properly.
///
/// - `stitched_boot_info: Vec<StitchedBootInfo>`
///   A vector containing boot information to be stitched together from other proofs.
///   - If no stitching is required, this vector can be empty.
#[derive(Clone, Debug, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Witness<O: WitnessOracle> {
    /// The witness oracle for preimage data preloaded in memory.
    pub oracle_witness: O,
    /// The witness oracle for preimage data streamed in on demand.
    pub stream_witness: O,
    /// Represents the witness data for blobs.
    pub blobs_witness: BlobWitnessData,
    /// This field represents the address of the proof's payout recipient.
    ///
    /// # Attribute
    /// - `#[rkyv(with = AddressDef)]`: Specifies a custom transformation or serialization
    ///   logic for the `Address` field as defined in `AddressDef`.
    #[rkyv(with = AddressDef)]
    pub payout_recipient_address: Address,
    /// Represents a hash value used for loading precondition validation data.
    ///
    /// # Attributes
    /// - `#[rkyv(with = B256Def)]`: Indicates that the `precondition_validation_data_hash`
    ///   field should use the `B256Def` serializer/deserializer for the `rkyv` framework during (de)serialization.
    #[rkyv(with = B256Def)]
    pub precondition_validation_data_hash: B256,
    /// A collection of stitched executions represented as a two-dimensional vector.
    ///
    /// # Structure:
    /// - The outer `Vec` represents a collection of execution groups.
    /// - Each inner `Vec<Execution>` contains a continuous series of `Execution` objects that
    ///   represent individual executions within a specific stitched group.
    ///
    /// # Notes:
    /// - Ensure all individual `Execution` objects within the groups are properly sorted.
    pub stitched_executions: Vec<Vec<Execution>>,
    /// A list of `StitchedBootInfo` instances to be stitched together from other proofs.
    ///
    /// # Notes
    /// - The vector can be safely empty if no stitching is required.
    pub stitched_boot_info: Vec<StitchedBootInfo>,
    /// A field representing the fault-proof virtual machine program image id.
    /// # Attributes
    /// - `#[rkyv(with = B256Def)]`: Indicates the use of the `B256Def` configuration for handling the `B256` type during
    ///    rkyv (archive, serialize, and deserialize) operations.
    #[rkyv(with = B256Def)]
    pub fpvm_image_id: B256,
}

impl Witness<VecOracle> {
    /// Creates a deep copy of the current instance.
    ///
    /// This method performs a "deep clone" of the object by cloning all its fields,
    /// including any nested fields that implement the `deep_clone` method.
    /// This ensures that all references and internal data are duplicated,
    /// rather than pointing to the same objects.
    ///
    /// # Returns
    /// A new instance of the structure with all fields deeply cloned.
    pub fn deep_clone(&self) -> Self {
        let mut cloned_with_arc = self.clone();
        cloned_with_arc.oracle_witness = cloned_with_arc.oracle_witness.deep_clone();
        cloned_with_arc.stream_witness = cloned_with_arc.stream_witness.deep_clone();
        cloned_with_arc
    }
}

/// A trait representing a Witness Oracle which manages and validates cryptographic preimages.
///
/// The `WitnessOracle` trait provides functionality to interact with and manage preimages.
/// Preimages are key-value pairs where the key is typically an identifier for the data,
/// and the value is the data itself stored as a `Vec<u8>`. This trait incorporates several features
/// including validation, insertion, count retrieval, and finalization of the stored preimages.
///
/// # Provided Methods
/// - `preimage_count`: Retrieve the number of preimages stored.
/// - `validate_preimages`: Validates that all stored preimages satisfy predefined constraints.
/// - `insert_preimage`: Inserts a new preimage into the oracle.
/// - `finalize_preimages`: Prepares the oracle's preimages for use under defined shard size and validation settings.
pub trait WitnessOracle: CommsClient + FlushableCache + Send + Sync + Debug + Default {
    /// Returns the count of preimages stored in the oracle.
    fn preimage_count(&self) -> usize;

    /// Ensures that the preimages stored in the oracle meet the required criteria or constraints
    /// defined by each `PreimageKeyType`. If the validation fails, an error is returned.
    fn validate_preimages(&self) -> anyhow::Result<()>;

    /// Inserts a preimage into the oracle.
    fn insert_preimage(&mut self, key: PreimageKey, value: Vec<u8>);

    /// This method finalizes the process of preparing the oracle preimages for a specific shard
    /// size and optional validation cache.
    fn finalize_preimages(&mut self, shard_size: usize, with_validation_cache: bool);
}

/// Represents the stitched boot information, primarily containing data relevant to the safe L2 chain
/// and associated output roots in a blockchain context.
///
/// Note:
/// - Each `B256` field uses the custom serialization handling provided by `B256Def` to ensure proper
///   serialization/deserialization behavior.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    serde::Serialize,
    serde::Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub struct StitchedBootInfo {
    /// The L1 head hash containing the safe L2 chain data that may reproduce the L2 head hash.
    #[rkyv(with = B256Def)]
    pub l1_head: B256,
    /// The agreed upon safe L2 output root.
    #[rkyv(with = B256Def)]
    pub agreed_l2_output_root: B256,
    /// The L2 output root claim.
    #[rkyv(with = B256Def)]
    pub claimed_l2_output_root: B256,
    /// The L2 claim block number.
    pub claimed_l2_block_number: u64,
}

impl From<ProofJournal> for StitchedBootInfo {
    /// Converts a `ProofJournal` into a `StitchedBootInfo` by transferring its values.
    fn from(value: ProofJournal) -> Self {
        Self {
            l1_head: value.l1_head,
            agreed_l2_output_root: value.agreed_l2_output_root,
            claimed_l2_output_root: value.claimed_l2_output_root,
            claimed_l2_block_number: value.claimed_l2_block_number,
        }
    }
}

impl From<&Receipt> for StitchedBootInfo {
    /// Converts a `Receipt` reference into the calling type by leveraging the intermediate conversion to `ProofJournal`.
    fn from(value: &Receipt) -> Self {
        Self::from(ProofJournal::from(value))
    }
}
