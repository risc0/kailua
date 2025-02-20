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
use crate::rkyv::{AddressDef, B256Def};
use alloy_primitives::{Address, B256};
use kona_preimage::{CommsClient, PreimageKey};
use kona_proof::FlushableCache;
use std::fmt::Debug;

#[derive(Clone, Debug, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Witness<O: WitnessOracle> {
    pub oracle_witness: O,
    pub blobs_witness: BlobWitnessData,
    #[rkyv(with = AddressDef)]
    pub payout_recipient_address: Address,
    #[rkyv(with = B256Def)]
    pub precondition_validation_data_hash: B256,
    pub stitched_executions: Vec<Vec<Execution>>,
    pub stitched_boot_info: Vec<StitchedBootInfo>,
    #[rkyv(with = B256Def)]
    pub fpvm_image_id: B256,
}

impl Witness<VecOracle> {
    pub fn deep_clone(&self) -> Self {
        let mut cloned_with_arc = self.clone();
        cloned_with_arc.oracle_witness = cloned_with_arc.oracle_witness.deep_clone();
        cloned_with_arc
    }
}

pub trait WitnessOracle: CommsClient + FlushableCache + Send + Sync + Debug {
    fn preimage_count(&self) -> usize;
    fn validate_preimages(&self) -> anyhow::Result<()>;
    fn insert_preimage(&mut self, key: PreimageKey, value: Vec<u8>);
    fn finalize_preimages(&mut self, shard_size: usize);
}

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
    fn from(value: ProofJournal) -> Self {
        Self {
            l1_head: value.l1_head,
            agreed_l2_output_root: value.agreed_l2_output_root,
            claimed_l2_output_root: value.claimed_l2_output_root,
            claimed_l2_block_number: value.claimed_l2_block_number,
        }
    }
}
