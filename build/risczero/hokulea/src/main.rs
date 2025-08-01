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

use kailua_hokulea::stitching::HokuleaStitchingClient;
use kailua_kona::client::stateless::run_stateless_client;
use kailua_kona::oracle::vec::VecOracle;
use kailua_kona::{client::log, witness::Witness};
use risc0_zkvm::guest::env;
use rkyv::rancor::Error;

const CANOE_IMAGE_ID: &str = env!("CANOE_IMAGE_ID");

fn main() {
    // Force blst linkage
    let _ = unsafe { blst::blst_p1_sizeof() };

    // Load EigenDA blob witness
    let eigen_da: hokulea_proof::eigenda_blob_witness::EigenDABlobWitnessData = {
        let data = env::read_frame();
        bincode::deserialize(&data).expect("EigenDABlobWitnessData deserialization failed")
    };

    // Load main witness
    let witness = {
        // Read serialized witness data
        let witness_data = env::read_frame();
        log("DESERIALIZE");
        rkyv::from_bytes::<Witness<VecOracle>, Error>(&witness_data)
            .expect("Failed to deserialize witness")
    };

    // Load extension shards
    for (i, entry) in witness
        .oracle_witness
        .preimages
        .lock()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        if !entry.is_empty() {
            continue;
        }
        log(&format!("DESERIALIZE SHARD {i}"));
        // read_shard is undefined on non-zkvm platforms
        #[cfg(target_os = "zkvm")]
        let _ = core::mem::replace(entry, kailua_kona::oracle::vec::read_shard());
    }

    // Run client using witness data
    let proof_journal = run_stateless_client(
        witness,
        HokuleaStitchingClient::new(eigen_da, CANOE_IMAGE_ID.parse().unwrap()),
    );

    // Prevent provability of insufficient data
    assert!(
        !proof_journal.claimed_l2_output_root.is_zero(),
        "Cannot prove proposal prematurity."
    );

    // Write the final stitched journal
    env::commit_slice(&proof_journal.encode_packed());
}
