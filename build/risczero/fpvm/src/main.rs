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

use kailua_common::oracle::vec::VecOracle;
use kailua_common::{
    client::{log, run_witness_client},
    witness::Witness,
};
use risc0_zkvm::guest::env;
use rkyv::rancor::Error;

fn main() {
    // Read witness data
    let witness_data = env::read_frame();
    log("DESERIALIZE");
    let witness = rkyv::from_bytes::<Witness<VecOracle>, Error>(&witness_data)
        .expect("Failed to deserialize witness");
    // Run client using witness
    let proof_journal = run_witness_client(witness);
    // Write the final stitched journal
    env::commit_slice(&proof_journal.encode_packed());
}
