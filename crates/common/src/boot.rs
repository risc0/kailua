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
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
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

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::journal::tests::{gen_proof_journals, to_fake_receipt};
    use alloy_primitives::keccak256;
    use rkyv::rancor::Error;

    pub fn gen_boot_infos(count: usize, gap: u64) -> Vec<StitchedBootInfo> {
        let l1_head = keccak256(b"l1_head");
        (0..count)
            .map(|i| StitchedBootInfo {
                l1_head,
                agreed_l2_output_root: keccak256(format!("l2_output_root {i}")),
                claimed_l2_output_root: keccak256(format!("l2_output_root {}", i + 1)),
                claimed_l2_block_number: i as u64 * gap,
            })
            .collect()
    }

    #[test]
    fn test_stitched_boot_info() {
        // test serde
        for info in gen_boot_infos(12, 64) {
            let recoded = rkyv::from_bytes::<StitchedBootInfo, Error>(
                rkyv::to_bytes::<Error>(&info).unwrap().as_ref(),
            )
            .unwrap();
            assert_eq!(info, recoded);
        }
    }

    #[test]
    fn test_stitched_boot_info_conversion() {
        for proof_journal in gen_proof_journals(12, 64, keccak256(b"config_hash")) {
            // from proof journal
            let from_journal = StitchedBootInfo::from(proof_journal);
            assert_eq!(from_journal.l1_head, proof_journal.l1_head);
            assert_eq!(
                from_journal.agreed_l2_output_root,
                proof_journal.agreed_l2_output_root
            );
            assert_eq!(
                from_journal.claimed_l2_output_root,
                proof_journal.claimed_l2_output_root
            );
            assert_eq!(
                from_journal.claimed_l2_block_number,
                proof_journal.claimed_l2_block_number
            );
            // from risc0 receipt
            let from_receipt = StitchedBootInfo::from(&to_fake_receipt(&proof_journal));
            assert_eq!(from_receipt.l1_head, proof_journal.l1_head);
            assert_eq!(
                from_receipt.agreed_l2_output_root,
                proof_journal.agreed_l2_output_root
            );
            assert_eq!(
                from_receipt.claimed_l2_output_root,
                proof_journal.claimed_l2_output_root
            );
            assert_eq!(
                from_receipt.claimed_l2_block_number,
                proof_journal.claimed_l2_block_number
            );
        }
    }
}
