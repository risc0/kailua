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
use kailua_common::blobs::PreloadedBlobProvider;
use kailua_common::client::log;
use kailua_common::journal::ProofJournal;
use kailua_common::oracle::PreloadedOracle;
use kailua_common::witness::{ArchivedWitness, Witness};
use kona_proof::BootInfo;
use risc0_zkvm::guest::env;
use rkyv::rancor::Error;
use std::sync::Arc;

fn main() {
    // Read witness data
    let witness_data = env::read_frame();
    log("ACCESS");
    let witness_access = rkyv::access::<ArchivedWitness, Error>(&witness_data)
        .expect("Failed to access witness data");
    log("DESERIALIZE");
    let witness =
        rkyv::deserialize::<Witness, Error>(witness_access).expect("Failed to deserialize witness");
    log(&format!(
        "ORACLE: {} PREIMAGES",
        witness.oracle_witness.data.len()
    ));
    let oracle = Arc::new(PreloadedOracle::from(witness.oracle_witness));
    log("BOOT");
    let boot = Arc::new(kona_proof::block_on(async {
        BootInfo::load(oracle.as_ref())
            .await
            .expect("Failed to load BootInfo")
    }));
    log(&format!(
        "BEACON: {} BLOBS",
        witness.blobs_witness.blobs.len()
    ));
    let beacon = PreloadedBlobProvider::from(witness.blobs_witness);

    // Attempt to recompute the output hash at the target block number using kona
    log("RUN");
    let (precondition_hash, computed_output_opt) = kailua_common::client::run_client(
        witness.precondition_validation_data_hash,
        oracle.clone(),
        boot.clone(),
        beacon,
    )
    .expect("Failed to compute output hash.");

    // Validate the output root
    if let Some(computed_output) = computed_output_opt {
        // With sufficient data, the input l2_claim must be true
        assert_eq!(boot.claimed_l2_output_root, computed_output);
    } else {
        // We use the zero claim hash to denote that the data as of l1 head is insufficient
        assert_eq!(boot.claimed_l2_output_root, B256::ZERO);
    }

    // Stitch boots together into a journal
    let mut stitched_journal = ProofJournal::new(
        witness.fpvm_image_id,
        witness.payout_recipient_address,
        precondition_hash,
        boot.as_ref(),
    );
    for stitched_boot in witness.stitched_boot_info {
        // Require equivalence in reference head
        assert_eq!(stitched_boot.l1_head, stitched_journal.l1_head);
        // Require progress in stitched boot
        assert_ne!(
            stitched_boot.agreed_l2_output_root,
            stitched_boot.claimed_l2_output_root
        );
        // Require proof assumption
        env::verify(
            witness.fpvm_image_id,
            &ProofJournal::new_stitched(
                witness.fpvm_image_id,
                witness.payout_recipient_address,
                precondition_hash,
                stitched_journal.config_hash,
                &stitched_boot,
            )
            .encode_packed(),
        )
        .expect("Failed to verify stitched boot assumption");
        // Require continuity
        if stitched_boot.claimed_l2_output_root == stitched_journal.agreed_l2_output_root {
            // Backward stitch
            stitched_journal.agreed_l2_output_root = stitched_boot.agreed_l2_output_root;
        } else if stitched_boot.agreed_l2_output_root == stitched_journal.claimed_l2_output_root {
            // Forward stitch
            stitched_journal.claimed_l2_output_root = stitched_boot.claimed_l2_output_root;
            stitched_journal.claimed_l2_block_number = stitched_boot.claimed_l2_block_number;
        } else {
            unimplemented!("No support for non-contiguous stitching.");
        }
    }

    // Write the final stitched journal
    env::commit_slice(&stitched_journal.encode_packed());
}
