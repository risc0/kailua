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

use alloy_primitives::B256;
use kailua_common::journal::ProofJournal;
use kona_proof::BootInfo;
use risc0_zkvm::guest::env;
use std::sync::Arc;
use kailua_common::blobs::PreloadedBlobProvider;
use kailua_common::oracle::PreloadedOracle;
use kailua_common::witness::Witness;

fn main() {
    let witness: Witness = env::read();
    let oracle = Arc::new(PreloadedOracle::from(witness.oracle_witness));
    let boot = Arc::new(kona_proof::block_on(async {
        BootInfo::load(oracle.as_ref())
            .await
            .expect("Failed to load BootInfo")
    }));
    let beacon = PreloadedBlobProvider::from(witness.blobs_witness);
    // Attempt to recompute the output hash at the target block number using kona
    let (precondition_hash, real_output_hash) = kailua_common::client::run_client(
        witness.precondition_validation_data_hash,
        oracle.clone(),
        boot.clone(),
        beacon,
    )
    .expect("Failed to compute output hash.");
    // Validate the output root
    if let Some(computed_output) = real_output_hash {
        // With sufficient data, the input l2_claim must be true
        assert_eq!(boot.claimed_l2_output_root, computed_output);
    } else {
        // We use the zero claim hash to denote that the data as of l1 head is insufficient
        assert_eq!(boot.claimed_l2_output_root, B256::ZERO);
    }
    // Write the proof journal
    env::commit_slice(&ProofJournal::new(precondition_hash, boot.as_ref()).encode_packed());
}
