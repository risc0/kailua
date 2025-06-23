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

use crate::channel::DuplexChannel;
use crate::validate::{Message, ValidateArgs};
use alloy::eips::eip4844::IndexedBlobHash;
use alloy::network::primitives::HeaderResponse;
use alloy::network::BlockResponse;
use alloy::primitives::{Address, B256};
use anyhow::{bail, Context};
use kailua_common::blobs::BlobFetchRequest;
use kailua_common::precondition::PreconditionValidationData;
use kailua_sync::agent::SyncAgent;
use kailua_sync::await_tel;
use kailua_sync::proposal::Proposal;
use kailua_sync::transact::rpc::{get_block_by_number, get_next_block};
use kona_protocol::BlockInfo;
use opentelemetry::global::tracer;
use opentelemetry::trace::FutureExt;
use opentelemetry::trace::{TraceContextExt, Tracer};
use risc0_zkvm::sha::Digestible;
use risc0_zkvm::InnerReceipt;
use std::path::PathBuf;
use tracing::{debug, info};

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
        args.core.provider.op_node_url.clone(),
    ];
    // precondition data
    if let Some(precondition_data) = precondition_validation_data {
        let (block_hashes, blob_hashes): (Vec<_>, Vec<_>) = precondition_data
            .blob_fetch_requests()
            .iter()
            .map(|r| (r.block_ref.hash.to_string(), r.blob_hash.hash.to_string()))
            .unzip();
        let params = match precondition_data {
            PreconditionValidationData::Validity {
                proposal_l2_head_number,
                proposal_output_count,
                output_block_span,
                blob_hashes: _,
            } => vec![
                proposal_l2_head_number,
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
    // boundless args
    if let Some(market) = &args.boundless.market {
        proving_args.extend(market.to_arg_vec(&args.boundless.storage));
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
        args.core.provider.eth_rpc_url.clone(),
        // l1 cl node
        String::from("--l1-beacon-address"),
        args.core.provider.beacon_rpc_url.clone(),
        // l2 el node
        String::from("--l2-node-address"),
        args.core.provider.op_geth_url.clone(),
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

/// Encode the seal of the given receipt for use with EVM smart contract verifiers.
///
/// Appends the verifier selector, determined from the first 4 bytes of the verifier parameters
/// including the Groth16 verification key and the control IDs that commit to the RISC Zero
/// circuits.
///
/// Copied from crate risc0-ethereum-contracts v2.0.2
pub fn encode_seal(receipt: &risc0_zkvm::Receipt) -> anyhow::Result<Vec<u8>> {
    let seal = match receipt.inner.clone() {
        InnerReceipt::Fake(receipt) => {
            let seal = receipt.claim.digest().as_bytes().to_vec();
            let selector = &[0xFFu8; 4];
            // Create a new vector with the capacity to hold both selector and seal
            let mut selector_seal = Vec::with_capacity(selector.len() + seal.len());
            selector_seal.extend_from_slice(selector);
            selector_seal.extend_from_slice(&seal);
            selector_seal
        }
        InnerReceipt::Groth16(receipt) => {
            let selector = &receipt.verifier_parameters.as_bytes()[..4];
            // Create a new vector with the capacity to hold both selector and seal
            let mut selector_seal = Vec::with_capacity(selector.len() + receipt.seal.len());
            selector_seal.extend_from_slice(selector);
            selector_seal.extend_from_slice(receipt.seal.as_ref());
            selector_seal
        }
        _ => bail!("Unsupported receipt type"),
        // TODO(victor): Add set verifier seal here.
    };
    Ok(seal)
}

pub async fn request_fault_proof(
    agent: &SyncAgent,
    channel: &mut DuplexChannel<Message>,
    parent: &Proposal,
    proposal: &Proposal,
    l1_head: B256,
) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("request_fault_proof"));

    let Some(fault) = proposal.fault() else {
        bail!("Proposal {} does not diverge from canon.", proposal.index);
    };
    let divergence_point = fault.divergence_point() as u64;

    // Read additional data for Kona invocation
    info!(
        "Requesting fault proof for proposal {} at point {divergence_point}.",
        proposal.index
    );

    // Set L2 Head Number: start from the last common transition
    let agreed_l2_head_number =
        parent.output_block_number + agent.deployment.output_block_span * divergence_point;
    debug!("l2_head_number {:?}", &agreed_l2_head_number);

    // Get L2 head hash
    let agreed_l2_head_hash = await_tel!(
        context,
        get_block_by_number(&agent.provider.l2_provider, agreed_l2_head_number,)
    )?
    .header()
    .hash();
    debug!("l2_head {:?}", &agreed_l2_head_hash);

    // Get L2 head output root
    let Some(agreed_l2_output_root) = agent.outputs.get(&agreed_l2_head_number).copied() else {
        bail!("Output root for agreed block {agreed_l2_head_number} not in memory.");
    };

    // Prepare expected output commitment: target the first bad transition
    let claimed_l2_block_number = agreed_l2_head_number + agent.deployment.output_block_span;
    let Some(claimed_l2_output_root) = agent.outputs.get(&claimed_l2_block_number).copied() else {
        bail!("Output root for claimed block {claimed_l2_block_number} not in memory.");
    };

    // Message proving task
    channel
        .sender
        .send(Message::Proposal {
            index: proposal.index,
            precondition_validation_data: None,
            l1_head,
            agreed_l2_head_hash,
            agreed_l2_output_root,
            claimed_l2_block_number,
            claimed_l2_output_root,
        })
        .await?;
    Ok(())
}

pub async fn request_validity_proof(
    agent: &SyncAgent,
    channel: &mut DuplexChannel<Message>,
    parent: &Proposal,
    proposal: &Proposal,
    l1_head: B256,
) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("request_validity_proof"));

    let precondition_validation_data = if agent.deployment.proposal_output_count > 1 {
        let mut validated_blobs = Vec::with_capacity(proposal.io_blobs.len());
        debug_assert!(!proposal.io_blobs.is_empty());
        for (blob_hash, blob) in &proposal.io_blobs {
            let block = await_tel!(
                context,
                get_next_block(&agent.provider.l1_provider, proposal.l1_head)
            )
            .context("block")?;

            validated_blobs.push(BlobFetchRequest {
                block_ref: BlockInfo {
                    hash: block.header.hash,
                    number: block.header.number,
                    parent_hash: block.header.parent_hash,
                    timestamp: block.header.timestamp,
                },
                blob_hash: IndexedBlobHash {
                    index: blob.index,
                    hash: *blob_hash,
                },
            })
        }
        debug_assert!(!validated_blobs.is_empty());
        Some(PreconditionValidationData::Validity {
            proposal_l2_head_number: parent.output_block_number,
            proposal_output_count: agent.deployment.proposal_output_count,
            output_block_span: agent.deployment.output_block_span,
            blob_hashes: validated_blobs,
        })
    } else {
        None
    };
    // Get L2 head hash
    let agreed_l2_head_hash = await_tel!(
        context,
        get_block_by_number(&agent.provider.l2_provider, parent.output_block_number)
    )?
    .header
    .hash;
    debug!("l2_head {:?}", &agreed_l2_head_hash);
    // Message proving task
    channel
        .sender
        .send(Message::Proposal {
            index: proposal.index,
            precondition_validation_data,
            l1_head,
            agreed_l2_head_hash,
            agreed_l2_output_root: parent.output_root,
            claimed_l2_block_number: proposal.output_block_number,
            claimed_l2_output_root: proposal.output_root,
        })
        .await?;
    Ok(())
}
