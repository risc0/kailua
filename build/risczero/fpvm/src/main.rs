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
use kailua_common::client::run_client;
use kailua_common::ProofJournal;
use kona_client::BootInfo;
use risc0_zkvm::guest::env;
use std::sync::Arc;

fn main() {
    let oracle = Arc::new(kailua_common::oracle::RISCZERO_POSIX_ORACLE);
    let boot = Arc::new(kona_common::block_on(async {
        BootInfo::load(oracle.as_ref())
            .await
            .expect("Failed to load BootInfo")
    }));
    // todo: bypass oracle using provider with preloaded data
    // let l2_oracle_provider = OracleL2ChainProvider::new(boot.clone(), oracle.clone());
    // let execution_provider = ExecutionProvider {
    //     tries: Arc::new(Mutex::new(Default::default())),
    //     contracts: Arc::new(Mutex::new(Default::default())),
    //     headers: Arc::new(Mutex::new(Default::default())),
    //     fallback: l2_oracle_provider,
    // };
    // Attempt to recompute the output hash at the target block number using kona
    let real_output_hash = run_client(
        oracle.clone(),
        boot.clone(),
        kailua_common::blobs::RISCZERO_POSIX_BLOB_PROVIDER,
        // execution_provider,
    )
    .expect("Failed to compute output hash.");
    // Write the proof journal
    let mut proof_journal = ProofJournal::from(boot.as_ref());
    if let Some(computed_output) = real_output_hash {
        // With sufficient data, the input l2_claim must be true
        if computed_output != boot.claimed_l2_output_root {
            panic!("Invalid l2 claim.");
        }
    } else {
        // We use the zero claim hash to denote that the data as of l1 head is insufficient
        proof_journal.claimed_l2_output_root = B256::ZERO;
    }
    env::commit_slice(&proof_journal.encode_packed());
}
