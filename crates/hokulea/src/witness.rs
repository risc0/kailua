// Copyright 2025 RISC Zero, Inc.
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

use alloy_primitives::B256;
use hokulea_proof::eigenda_blob_witness::EigenDABlobWitnessData;
use kona_proof::BootInfo;

pub fn da_witness_precondition(eigen_da: &EigenDABlobWitnessData) -> Option<(B256, u64, u64)> {
    // Enforce sufficient data requirements
    assert!(eigen_da.recency.len() >= eigen_da.validity.len());
    assert!(eigen_da.validity.len() >= eigen_da.blob.len());
    // Enforce L1 chain consistency
    let (l1_head, l1_chain_id) = eigen_da
        .validity
        .first()
        .map(|(_, cert)| (cert.l1_head_block_hash, cert.l1_chain_id))?;
    for (_, cert) in &eigen_da.validity {
        assert_eq!(l1_head, cert.l1_head_block_hash);
        assert_eq!(l1_chain_id, cert.l1_chain_id);
    }
    // Enforce L2 configuration consistency
    let recency = eigen_da.recency.first().map(|(_, r)| *r)?;
    for (_, r) in &eigen_da.recency {
        assert_eq!(recency, *r);
    }
    Some((l1_head, l1_chain_id, recency))
}

pub fn da_witness_postcondition(precondition: Option<(B256, u64, u64)>, boot_info: &BootInfo) {
    if let Some((l1_head, l1_chain_id, recency)) = precondition {
        assert_eq!(l1_head, boot_info.l1_head);
        assert_eq!(l1_chain_id, boot_info.rollup_config.l1_chain_id);
        assert_eq!(recency, boot_info.rollup_config.seq_window_size);
    }
}
