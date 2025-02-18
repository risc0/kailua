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

use crate::blobs::PreloadedBlobProvider;
use crate::client;
use crate::client::stitching;
use crate::journal::ProofJournal;
use crate::witness::{Witness, WitnessOracle};
use std::sync::Arc;
use tracing::log::warn;

pub fn run_stateless_client<O: WitnessOracle>(witness: Witness<O>) -> ProofJournal {
    client::log(&format!(
        "ORACLE: {} PREIMAGES",
        witness.oracle_witness.preimage_count()
    ));
    witness
        .oracle_witness
        .validate_preimages()
        .expect("Failed to validate preimages");
    let oracle = Arc::new(witness.oracle_witness);
    client::log(&format!(
        "BEACON: {} BLOBS",
        witness.blobs_witness.blobs.len()
    ));
    let beacon = PreloadedBlobProvider::from(witness.blobs_witness);

    let proof_journal = stitching::run_stitching_client(
        witness.precondition_validation_data_hash,
        oracle.clone(),
        beacon,
        witness.fpvm_image_id,
        witness.payout_recipient_address,
        witness.stitched_boot_info,
    );

    if oracle.preimage_count() > 0 {
        warn!(
            "Found {} extra preimages in witness",
            oracle.preimage_count()
        );
    }

    proof_journal
}
