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

use alloy_primitives::keccak256;
use anyhow::{bail, Context};
use kailua_common::journal::ProofJournal;
use kailua_common::proof::Proof;
use std::path::Path;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

pub fn encode_seal(proof: &Proof) -> anyhow::Result<Vec<u8>> {
    match proof {
        Proof::ZKVMReceipt(receipt) => risc0_ethereum_contracts::encode_seal(receipt),
        Proof::BoundlessSeal(seal, _) => Ok(seal.clone()),
        Proof::SetBuilderReceipt(..) => unimplemented!(),
    }
}

pub fn derive_set_builder_receipt(proof: &Proof) -> anyhow::Result<Proof> {
    let Proof::BoundlessSeal(_seal, _journal) = proof else {
        bail!("Expected Proof::BoundlessSeal instance");
    };

    // todo: wait for spec on how boundless will provide the seal
    todo!()
    // let seal = if let Ok(seal) = risc0_groth16::Seal::from_vec(&encoded_seal) {
    //     seal
    // } else {
    //     // todo: verify inclusion proof
    //     // todo: extract groth16 seal
    //     todo!()
    // };
    // // todo: create claim digest
    // // todo: get verifier parameters
    // // todo: create and verify groth16 receipt
    //
    // let n = encoded_seal.len() - 256;
    // Groth16Receipt::new(
    //     encoded_seal[n..].to_vec(),
    //     MaybePruned::Pruned(journal_digest),
    //     *verifying_params.get_or_insert_with(|| {
    //         Groth16ReceiptVerifierParameters::default().digest()
    //     }),
    // )
    // .verify_integrity()
    // .expect("Failed to verify Groth16Receipt for {journal_digest}.");
}

pub fn proof_file_name(proof_journal: &ProofJournal) -> String {
    let version = risc0_zkvm::get_version().unwrap();
    let suffix = if risc0_zkvm::is_dev_mode() {
        "fake"
    } else {
        "zkp"
    };
    let claimed_l2_block_number = proof_journal.claimed_l2_block_number.to_be_bytes();
    let data = [
        proof_journal.payout_recipient.as_slice(),
        proof_journal.precondition_hash.as_slice(),
        proof_journal.l1_head.as_slice(),
        proof_journal.agreed_l2_output_root.as_slice(),
        proof_journal.claimed_l2_output_root.as_slice(),
        claimed_l2_block_number.as_slice(),
        proof_journal.config_hash.as_slice(),
        proof_journal.fpvm_image_id.as_slice(),
    ]
    .concat();
    let file_name = keccak256(data);
    format!("risc0-{version}-{file_name}.{suffix}")
}

pub async fn read_proof_file(proof_file_name: &str) -> anyhow::Result<Proof> {
    // Read receipt file
    if !Path::new(proof_file_name).exists() {
        bail!("Proof file {proof_file_name} not found.");
    }
    let mut proof_file = File::open(proof_file_name)
        .await
        .context(format!("Failed to open proof file {proof_file_name}."))?;
    let mut proof_data = Vec::new();
    proof_file
        .read_to_end(&mut proof_data)
        .await
        .context(format!(
            "Failed to read proof file {proof_file_name} data until end."
        ))?;
    bincode::deserialize::<Proof>(&proof_data).context(format!(
        "Failed to deserialize proof file {proof_file_name} data with bincode"
    ))
}
