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

use crate::validate::ValidateArgs;
use alloy::primitives::{Address, B256};
use kailua_common::precondition::PreconditionValidationData;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Task {
    pub proposal_index: u64,
    pub proving_args: Vec<String>,
    pub proof_file_name: String,
}

#[allow(clippy::too_many_arguments)]
pub fn create_proving_args(
    args: &ValidateArgs,
    data_dir: PathBuf,
    l2_chain_id: String,
    payout_recipient: Address,
    precondition_validation_data: Option<PreconditionValidationData>,
    l1_head: B256,
    agreed_l2_head_hash: B256,
    agreed_l2_output_root: B256,
    claimed_l2_block_number: u64,
    claimed_l2_output_root: B256,
) -> Vec<String> {
    // Prepare kailua-host parameters
    let verbosity = [
        String::from("-"),
        (0..args.core.v).map(|_| 'v').collect::<String>(),
    ]
    .concat();
    let mut proving_args = vec![
        // wallet address for payouts
        String::from("--payout-recipient-address"),
        payout_recipient.to_string(),
        // l2 el node
        String::from("--op-node-address"),
        args.core.op_node_url.clone(),
    ];
    // precondition data
    if let Some(precondition_data) = precondition_validation_data {
        let (block_hashes, blob_hashes): (Vec<_>, Vec<_>) = precondition_data
            .blob_fetch_requests()
            .iter()
            .map(|r| (r.block_ref.hash.to_string(), r.blob_hash.hash.to_string()))
            .unzip();
        let params = match precondition_data {
            PreconditionValidationData::Validity(
                global_l2_head_number,
                proposal_output_count,
                output_block_span,
                _,
            ) => vec![
                global_l2_head_number,
                proposal_output_count,
                output_block_span,
            ],
        }
        .into_iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>();

        proving_args.extend(vec![
            String::from("--precondition-params"),
            params.join(","),
            String::from("--precondition-block-hashes"),
            block_hashes.join(","),
            String::from("--precondition-blob-hashes"),
            blob_hashes.join(","),
        ]);
    }
    // data directory
    let data_dir = data_dir.join(format!(
        "{}-{}",
        &agreed_l2_output_root.to_string()[..10].to_string(),
        &claimed_l2_output_root.to_string()[..10].to_string()
    ));
    // kona args
    proving_args.extend(vec![
        // l1 head from on-chain proposal
        String::from("--l1-head"),
        l1_head.to_string(),
        // l2 starting block hash from on-chain proposal
        String::from("--agreed-l2-head-hash"),
        agreed_l2_head_hash.to_string(),
        // l2 starting output root
        String::from("--agreed-l2-output-root"),
        agreed_l2_output_root.to_string(),
        // proposed output root
        String::from("--claimed-l2-output-root"),
        claimed_l2_output_root.to_string(),
        // proposed block number
        String::from("--claimed-l2-block-number"),
        claimed_l2_block_number.to_string(),
        // rollup chain id
        String::from("--l2-chain-id"),
        l2_chain_id.clone(),
        // l1 el node
        String::from("--l1-node-address"),
        args.core.eth_rpc_url.clone(),
        // l1 cl node
        String::from("--l1-beacon-address"),
        args.core.beacon_rpc_url.clone(),
        // l2 el node
        String::from("--l2-node-address"),
        args.core.op_geth_url.clone(),
        // path to cache
        String::from("--data-dir"),
        data_dir.to_str().unwrap().to_string(),
        // run the client natively
        String::from("--native"),
    ]);
    // verbosity level
    if args.core.v > 0 {
        proving_args.push(verbosity);
    }
    proving_args
}

#[cfg(feature = "devnet")]
pub fn maybe_patch_proof(
    mut receipt: risc0_zkvm::Receipt,
    expected_fpvm_image_id: [u8; 32],
) -> anyhow::Result<risc0_zkvm::Receipt> {
    // Return the proof if we can't patch it
    if !risc0_zkvm::is_dev_mode() {
        return Ok(receipt);
    }

    let expected_fpvm_image_id = risc0_zkvm::sha::Digest::from(expected_fpvm_image_id);

    // Patch the image id of the receipt to match the expected one
    if let risc0_zkvm::InnerReceipt::Fake(fake_inner_receipt) = &mut receipt.inner {
        if let risc0_zkvm::MaybePruned::Value(claim) = &mut fake_inner_receipt.claim {
            tracing::warn!("DEV-MODE ONLY: Patching fake receipt image id to match game contract.");
            claim.pre = risc0_zkvm::MaybePruned::Pruned(expected_fpvm_image_id);
            if let risc0_zkvm::MaybePruned::Value(Some(output)) = &mut claim.output {
                if let risc0_zkvm::MaybePruned::Value(journal) = &mut output.journal {
                    let n = journal.len();
                    journal[n - 32..n].copy_from_slice(expected_fpvm_image_id.as_bytes());
                    receipt.journal.bytes[n - 32..n]
                        .copy_from_slice(expected_fpvm_image_id.as_bytes());
                }
            }
        }
    }
    Ok(receipt)
}
