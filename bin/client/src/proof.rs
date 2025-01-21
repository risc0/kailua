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

use alloy_primitives::{keccak256, B256};
use anyhow::{bail, Context};
use kailua_build::KAILUA_FPVM_ID;
use risc0_zkvm::{Journal, Receipt};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Proof {
    ZKVMReceipt(Box<Receipt>),
    BoundlessSeal(Vec<u8>, Journal),
}

impl Proof {
    pub fn journal(&self) -> &Journal {
        match self {
            Proof::ZKVMReceipt(receipt) => &receipt.journal,
            Proof::BoundlessSeal(_, journal) => journal,
        }
    }

    pub fn encoded_seal(&self) -> anyhow::Result<Vec<u8>> {
        match self {
            Proof::ZKVMReceipt(receipt) => risc0_ethereum_contracts::encode_seal(receipt),
            Proof::BoundlessSeal(seal, _) => Ok(seal.clone()),
        }
    }

    pub fn is_receipt(&self) -> bool {
        matches!(self, Proof::ZKVMReceipt(_))
    }

    pub fn as_receipt(&self) -> Option<&Receipt> {
        match self {
            Proof::ZKVMReceipt(receipt) => Some(receipt),
            _ => None,
        }
    }

    pub fn as_receipt_mut(&mut self) -> Option<&mut Receipt> {
        match self {
            Proof::ZKVMReceipt(receipt) => Some(receipt),
            _ => None,
        }
    }
}

pub fn fpvm_proof_file_name(
    precondition_output: B256,
    l1_head: B256,
    claimed_l2_output_root: B256,
    claimed_l2_block_number: u64,
    agreed_l2_output_root: B256,
) -> String {
    let version = risc0_zkvm::get_version().unwrap();
    let suffix = if risc0_zkvm::is_dev_mode() {
        "fake"
    } else {
        "zkp"
    };
    let claimed_l2_block_number = claimed_l2_block_number.to_be_bytes();
    let data = [
        bytemuck::cast::<_, [u8; 32]>(KAILUA_FPVM_ID).as_slice(),
        precondition_output.as_slice(),
        l1_head.as_slice(),
        claimed_l2_output_root.as_slice(),
        claimed_l2_block_number.as_slice(),
        agreed_l2_output_root.as_slice(),
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
