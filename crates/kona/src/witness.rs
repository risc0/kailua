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
use crate::boot::StitchedBootInfo;
use crate::executor::Execution;
use crate::oracle::vec::VecOracle;
use crate::oracle::WitnessOracle;
use crate::rkyv::primitives::{AddressDef, B256Def};
use alloy_primitives::{Address, B256};
use std::fmt::Debug;

/// Represents the complete structure of a `Witness`, which is used to hold
/// the necessary data for authenticating a rollup state transition in the FPVM.
#[derive(Clone, Debug, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Witness<O: WitnessOracle> {
    /// The witness oracle for preimage data preloaded in memory.
    pub oracle_witness: O,
    /// The witness oracle for preimage data streamed in on demand.
    pub stream_witness: O,
    /// Represents the witness data for blobs.
    pub blobs_witness: BlobWitnessData,
    /// Represents the address of the proof's payout recipient.
    #[rkyv(with = AddressDef)]
    pub payout_recipient_address: Address,
    /// Represents a hash value used for loading precondition validation data.
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
    /// - Ensure all `Execution` objects within the groups are properly sorted.
    pub stitched_executions: Vec<Vec<Execution>>,
    /// A list of `StitchedBootInfo` instances to be stitched together from other proofs.
    pub stitched_boot_info: Vec<StitchedBootInfo>,
    /// Represents the fault-proof virtual machine program image id.
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod tests {
    use super::*;
    use crate::blobs::tests::gen_blobs;
    use crate::boot::tests::gen_boot_infos;
    use crate::executor::tests::gen_executions;
    use crate::oracle::vec::tests::{exhaust_vec_oracle, prepare_vec_oracle};
    use alloy_primitives::keccak256;
    use std::ops::Deref;

    pub fn create_test_witness() -> (Witness<VecOracle>, Vec<Vec<u8>>) {
        let (vec_oracle, values) = prepare_vec_oracle(512, 1);
        let blobs_witness = BlobWitnessData::from(gen_blobs(10));
        let witness = Witness {
            oracle_witness: vec_oracle.deep_clone(),
            stream_witness: vec_oracle.deep_clone(),
            blobs_witness,
            payout_recipient_address: Address::from([0xb0; 20]),
            precondition_validation_data_hash: keccak256(b"precondition_validation_data_hash"),
            stitched_executions: vec![gen_executions(64)
                .into_iter()
                .map(|e| e.deref().clone())
                .collect()],
            stitched_boot_info: gen_boot_infos(32, 128),
            fpvm_image_id: keccak256(b"fpvm_image_id"),
        };

        (witness, values)
    }

    #[tokio::test]
    pub async fn test_witness() {
        let (witness, values) = create_test_witness();
        // test serde
        {
            let recoded = rkyv::from_bytes::<Witness<VecOracle>, rkyv::rancor::Error>(
                &rkyv::to_bytes::<rkyv::rancor::Error>(&witness).unwrap(),
            )
            .unwrap();
            assert_eq!(
                witness.oracle_witness.preimages.lock().unwrap().to_vec(),
                recoded.oracle_witness.preimages.lock().unwrap().to_vec()
            );
            assert_eq!(
                witness.stream_witness.preimages.lock().unwrap().to_vec(),
                recoded.stream_witness.preimages.lock().unwrap().to_vec()
            );
            assert_eq!(witness.blobs_witness, recoded.blobs_witness);
            assert_eq!(
                witness.payout_recipient_address,
                recoded.payout_recipient_address
            );
            assert_eq!(
                witness.precondition_validation_data_hash,
                recoded.precondition_validation_data_hash
            );
            assert_eq!(witness.stitched_boot_info, recoded.stitched_boot_info);
            assert_eq!(witness.fpvm_image_id, recoded.fpvm_image_id);
        }
        // test deep clone
        let regular_clone = witness.clone();
        let deep_clone = witness.deep_clone();
        let preimage_count = regular_clone.oracle_witness.preimage_count();
        exhaust_vec_oracle(1, witness.oracle_witness, values).await;
        assert_eq!(regular_clone.oracle_witness.preimage_count(), 0);
        assert_eq!(deep_clone.oracle_witness.preimage_count(), preimage_count);
    }
}
