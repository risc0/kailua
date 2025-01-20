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

use crate::witness::StitchedBootInfo;
use alloy_primitives::{Address, B256};
use anyhow::Context;
use kona_proof::BootInfo;
use serde::{Deserialize, Serialize};

#[derive(PartialEq, Eq, Ord, PartialOrd, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ProofJournal {
    /// The recipient address for the payout
    pub payout_recipient: Address,
    /// The last finalized L2 output
    pub precondition_output: B256,
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
    /// The FPVM image id
    pub fpvm_image_id: B256,
}

impl ProofJournal {
    pub fn new(
        fpvm_image_id: B256,
        payout_recipient: Address,
        precondition_output: B256,
        boot_info: &BootInfo,
    ) -> Self {
        Self {
            fpvm_image_id,
            payout_recipient,
            precondition_output,
            l1_head: boot_info.l1_head,
            agreed_l2_output_root: boot_info.agreed_l2_output_root,
            claimed_l2_output_root: boot_info.claimed_l2_output_root,
            claimed_l2_block_number: boot_info.claimed_l2_block_number,
            config_hash: B256::from(crate::config::config_hash(&boot_info.rollup_config).unwrap()),
        }
    }

    pub fn new_stitched(
        fpvm_image_id: B256,
        payout_recipient: Address,
        precondition_output: B256,
        config_hash: B256,
        boot_info: &StitchedBootInfo,
    ) -> Self {
        Self {
            fpvm_image_id,
            payout_recipient,
            precondition_output,
            l1_head: boot_info.l1_head,
            agreed_l2_output_root: boot_info.agreed_l2_output_root,
            claimed_l2_output_root: boot_info.claimed_l2_output_root,
            claimed_l2_block_number: boot_info.claimed_l2_block_number,
            config_hash,
        }
    }
}

impl ProofJournal {
    pub fn encode_packed(&self) -> Vec<u8> {
        [
            self.payout_recipient.as_slice(),
            self.precondition_output.as_slice(),
            self.l1_head.as_slice(),
            self.agreed_l2_output_root.as_slice(),
            self.claimed_l2_output_root.as_slice(),
            self.claimed_l2_block_number.to_be_bytes().as_slice(),
            self.config_hash.as_slice(),
            self.fpvm_image_id.as_slice(),
        ]
        .concat()
    }

    pub fn decode_packed(encoded: &[u8]) -> Result<Self, anyhow::Error> {
        Ok(ProofJournal {
            payout_recipient: encoded[..20].try_into().context("payout_recipient")?,
            precondition_output: encoded[20..52].try_into().context("precondition_output")?,
            l1_head: encoded[52..84].try_into().context("l1_head")?,
            agreed_l2_output_root: encoded[84..116]
                .try_into()
                .context("agreed_l2_output_root")?,
            claimed_l2_output_root: encoded[116..148]
                .try_into()
                .context("claimed_l2_output_root")?,
            claimed_l2_block_number: u64::from_be_bytes(
                encoded[148..156]
                    .try_into()
                    .context("claimed_l2_block_number")?,
            ),
            config_hash: encoded[156..188].try_into().context("config_hash")?,
            fpvm_image_id: encoded[188..220].try_into().context("fpvm_image_id")?,
        })
    }
}
