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

use crate::boot::StitchedBootInfo;
use alloy_primitives::{Address, B256};
use kona_proof::BootInfo;
use risc0_zkvm::Receipt;
use serde::{Deserialize, Serialize};

/// Represents a (provable) state transition of a rollup ledger.
#[derive(PartialEq, Eq, Ord, PartialOrd, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ProofJournal {
    /// The recipient address for the proof payout
    pub payout_recipient: Address,
    /// The hash of the precondition for validating this proof.
    ///
    /// The value represents exactly one of the following cases:
    /// - Validity proving: Equal to [crate::precondition::PreconditionValidationData::precondition_hash].
    /// - Fault proving: Equal to [B256::ZERO].
    /// - Execution proving: Equal to [crate::executor::exec_precondition_hash].
    pub precondition_hash: B256,
    /// The L1 head hash containing the safe L2 chain data that may reproduce the L2 head hash.
    pub l1_head: B256,
    /// The latest finalized L2 output root.
    pub agreed_l2_output_root: B256,
    /// The L2 output root claim.
    pub claimed_l2_output_root: B256,
    /// The L2 claim block number.
    pub claimed_l2_block_number: u64,
    /// The configuration hash.
    pub config_hash: B256,
    /// The FPVM version
    pub fpvm_image_id: B256,
}

impl ProofJournal {
    /// Constructs a new stand-alone instance.
    pub fn new(
        fpvm_image_id: B256,
        payout_recipient: Address,
        precondition_output: B256,
        boot_info: &BootInfo,
    ) -> Self {
        Self {
            fpvm_image_id,
            payout_recipient,
            precondition_hash: precondition_output,
            l1_head: boot_info.l1_head,
            agreed_l2_output_root: boot_info.agreed_l2_output_root,
            claimed_l2_output_root: boot_info.claimed_l2_output_root,
            claimed_l2_block_number: boot_info.claimed_l2_block_number,
            config_hash: B256::from(crate::config::config_hash(&boot_info.rollup_config).unwrap()),
        }
    }

    /// Constructs a new instance used for stitching separate continuous journals together.
    pub fn new_stitched(
        fpvm_image_id: B256,
        payout_recipient: Address,
        precondition_output: B256,
        config_hash: B256,
        stitched_boot_info: &StitchedBootInfo,
    ) -> Self {
        Self {
            fpvm_image_id,
            payout_recipient,
            precondition_hash: precondition_output,
            l1_head: stitched_boot_info.l1_head,
            agreed_l2_output_root: stitched_boot_info.agreed_l2_output_root,
            claimed_l2_output_root: stitched_boot_info.claimed_l2_output_root,
            claimed_l2_block_number: stitched_boot_info.claimed_l2_block_number,
            config_hash,
        }
    }
}

impl ProofJournal {
    /// This function concatenates the fields of the struct into a contiguous byte vector
    /// to create a packed representation of the data. Each field is converted or sliced into
    /// a byte representation, and then the resulting slices are concatenated.
    ///
    /// # Returns:
    /// - A `Vec<u8>` containing the concatenated byte representation of the included fields.
    ///
    /// # Notes:
    /// - The method relies on the assumption that all involved fields have compatible
    ///   byte slice representations.
    /// - Ensure the individual lengths of fields do not exceed the intended size constraint
    ///   for the packed data.
    pub fn encode_packed(&self) -> Vec<u8> {
        [
            self.payout_recipient.as_slice(),
            self.precondition_hash.as_slice(),
            self.l1_head.as_slice(),
            self.agreed_l2_output_root.as_slice(),
            self.claimed_l2_output_root.as_slice(),
            self.claimed_l2_block_number.to_be_bytes().as_slice(),
            self.config_hash.as_slice(),
            self.fpvm_image_id.as_slice(),
        ]
        .concat()
    }

    /// Decodes a byte slice representing a packed `ProofJournal` structure into its constituent fields.
    ///
    /// The method extracts fixed-width byte segments from the provided input slice, interprets them
    /// as the respective fields of `ProofJournal`, and validates the integrity of the input.
    ///
    /// # Arguments
    ///
    /// * `encoded` - A byte slice containing the serialized representation of the `ProofJournal` fields in order.
    ///
    /// # Expected Encoding Layout
    ///
    /// The `encoded` byte slice is expected to follow this layout:
    ///
    /// - Bytes `[0..20]`: `payout_recipient` (20 bytes)
    /// - Bytes `[20..52]`: `precondition_hash` (32 bytes)
    /// - Bytes `[52..84]`: `l1_head` (32 bytes)
    /// - Bytes `[84..116]`: `agreed_l2_output_root` (32 bytes)
    /// - Bytes `[116..148]`: `claimed_l2_output_root` (32 bytes)
    /// - Bytes `[148..156]`: `claimed_l2_block_number` (8 bytes - `u64` in big-endian format)
    /// - Bytes `[156..188]`: `config_hash` (32 bytes)
    /// - Bytes `[188..220]`: `fpvm_image_id` (32 bytes)
    pub fn decode_packed(encoded: &[u8]) -> Self {
        ProofJournal {
            payout_recipient: encoded[..20].try_into().unwrap(),
            precondition_hash: encoded[20..52].try_into().unwrap(),
            l1_head: encoded[52..84].try_into().unwrap(),
            agreed_l2_output_root: encoded[84..116].try_into().unwrap(),
            claimed_l2_output_root: encoded[116..148].try_into().unwrap(),
            claimed_l2_block_number: u64::from_be_bytes(encoded[148..156].try_into().unwrap()),
            config_hash: encoded[156..188].try_into().unwrap(),
            fpvm_image_id: encoded[188..220].try_into().unwrap(),
        }
    }
}

impl From<&Receipt> for ProofJournal {
    /// Converts a `Receipt` reference into the implementing type by decoding its packed journal.
    ///
    /// # Arguments
    ///
    /// * `value` - A reference to a `Receipt` object. The function uses the `journal` field of the `Receipt`
    ///   to decode and derive the desired type.
    ///
    /// # Returns
    ///
    /// The decoded type, obtained by unpacking the journal field of the given `Receipt`.
    ///
    /// # Panics
    ///
    /// This function will panic if the decoding of the packed journal fails. Ensure that the `journal`
    /// field in the `Receipt` contains valid encoded data before calling this method.
    fn from(value: &Receipt) -> Self {
        Self::decode_packed(value.journal.as_ref())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod tests {
    use super::*;
    use crate::boot::tests::gen_boot_infos;
    use alloy_primitives::{keccak256, Address, B256};
    use risc0_zkvm::{FakeReceipt, InnerReceipt, ReceiptClaim};

    pub fn gen_proof_journals(count: usize, gap: u64, config_hash: B256) -> Vec<ProofJournal> {
        let payout_recipient = Address::from([0xb0; 20]);
        let precondition_hash = keccak256(b"precondition_hash");
        let fpvm_image_id = keccak256(b"fpvm_image_id");

        gen_boot_infos(count, gap)
            .into_iter()
            .map(|b| {
                ProofJournal::new_stitched(
                    fpvm_image_id,
                    payout_recipient,
                    precondition_hash,
                    config_hash,
                    &b,
                )
            })
            .collect()
    }

    pub fn to_fake_receipt(proof_journal: &ProofJournal) -> Receipt {
        let encoded = proof_journal.encode_packed();
        Receipt::new(
            InnerReceipt::Fake(FakeReceipt::new(ReceiptClaim::ok(
                proof_journal.fpvm_image_id.0,
                encoded.clone(),
            ))),
            encoded,
        )
    }

    #[test]
    fn test_proof_journal_coding() {
        let proof_journals = gen_proof_journals(512, 64, keccak256(b"config_hash"));
        // test serde
        for journal in proof_journals {
            let encoded = journal.encode_packed();
            let decoded = ProofJournal::decode_packed(&encoded);
            assert_eq!(journal, decoded);
        }
    }

    #[test]
    fn test_proof_journal_constructor() {
        let config_hash =
            B256::from(crate::config::config_hash(&kona_genesis::RollupConfig::default()).unwrap());
        let proof_journals = gen_proof_journals(512, 64, config_hash);
        // Test constructor
        for journal in proof_journals {
            let boot_info = BootInfo {
                l1_head: journal.l1_head,
                agreed_l2_output_root: journal.agreed_l2_output_root,
                claimed_l2_output_root: journal.claimed_l2_output_root,
                claimed_l2_block_number: journal.claimed_l2_block_number,
                chain_id: 0,
                rollup_config: Default::default(),
            };
            let new_journal = ProofJournal::new(
                journal.fpvm_image_id,
                journal.payout_recipient,
                journal.precondition_hash,
                &boot_info,
            );
            assert_eq!(new_journal, journal);
        }
    }

    #[test]
    fn test_receipt_conversion() {
        let proof_journals = gen_proof_journals(512, 64, keccak256(b"config_hash"));
        // Test conversion
        for proof_journal in proof_journals {
            let receipt = to_fake_receipt(&proof_journal);
            assert_eq!(proof_journal, (&receipt).into());
        }
    }
}
