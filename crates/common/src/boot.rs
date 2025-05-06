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

use crate::journal::ProofJournal;
use crate::rkyv::primitives::B256Def;
use alloy_primitives::B256;
use risc0_zkvm::Receipt;

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
