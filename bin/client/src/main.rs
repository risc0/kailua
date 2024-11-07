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

use kailua_client::fpvm_proof_file_name;
use kailua_common::ProofJournal;
use std::env::var;
use std::str::FromStr;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if let Ok(Ok(verbosity_level)) = var("KAILUA_VERBOSITY").map(|s| u8::from_str(&s)) {
        kona_host::init_tracing_subscriber(verbosity_level)?;
    }
    // preload all data natively
    info!("Running native client.");
    kailua_client::run_native_client()
        .await
        .expect("Failed to run native client.");
    // compute the receipt in the zkvm
    info!("Running zk client.");
    let prove_info = kailua_client::prove_zkvm_client()
        .await
        .expect("Failed to run zk client.");
    // Prepare receipt file
    let proof_journal = ProofJournal::decode_packed(prove_info.receipt.journal.as_ref())
        .expect("Failed to decode receipt output");
    let mut output_file = File::create(fpvm_proof_file_name(
        proof_journal.l1_head,
        proof_journal.claimed_l2_output_root,
        proof_journal.claimed_l2_block_number,
        proof_journal.agreed_l2_output_root,
    ))
    .await
    .expect("Failed to create receipt output file");
    // Write receipt data to file
    let receipt_bytes =
        bincode::serialize(&prove_info.receipt).expect("Could not serialize receipt.");
    output_file
        .write_all(receipt_bytes.as_slice())
        .await
        .expect("Failed to write receipt to file");
    output_file
        .flush()
        .await
        .expect("Failed to flush receipt output file data.");

    Ok(())
}
