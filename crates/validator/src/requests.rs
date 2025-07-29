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

use crate::args::ValidateArgs;
use crate::channel::{DuplexChannel, Message};
use crate::tasks::{handle_proving_tasks, Task};
use alloy::eips::eip4844::IndexedBlobHash;
use alloy::network::primitives::HeaderResponse;
use alloy::network::{BlockResponse, TxSigner};
use alloy::primitives::B256;
use anyhow::{bail, Context};
use kailua_build::KAILUA_FPVM_ID;
use kailua_common::blobs::BlobFetchRequest;
use kailua_common::config::config_hash;
use kailua_common::journal::ProofJournal;
use kailua_common::precondition::PreconditionValidationData;
use kailua_prover::args::{ProveArgs, ProvingArgs};
use kailua_prover::channel::AsyncChannel;
use kailua_prover::proof::proof_file_name;
use kailua_sync::agent::SyncAgent;
use kailua_sync::proposal::Proposal;
use kailua_sync::provider::optimism::fetch_rollup_config;
use kailua_sync::transact::rpc::{get_block_by_number, get_next_block};
use kailua_sync::{await_tel, await_tel_res};
use kona_protocol::BlockInfo;
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use std::path::PathBuf;
use tokio::spawn;
use tracing::{debug, error, info, warn};

pub async fn handle_proof_requests(
    mut channel: DuplexChannel<Message>,
    args: ValidateArgs,
    verbosity: u8,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    // Telemetry
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("handle_proof_requests"));

    // Fetch rollup configuration
    let rollup_config = await_tel!(
        context,
        fetch_rollup_config(
            &args.sync.provider.op_node_url,
            &args.sync.provider.op_geth_url,
            None,
            args.proving.bypass_chain_registry
        )
    )
    .context("fetch_rollup_config")?;
    let config_hash = B256::from(config_hash(&rollup_config)?);
    let fpvm_image_id = B256::from(bytemuck::cast::<[u32; 8], [u8; 32]>(KAILUA_FPVM_ID));
    // Set payout recipient
    let validator_wallet = await_tel_res!(
        context,
        tracer,
        "ValidatorSigner::wallet",
        args.validator_signer
            .wallet(Some(rollup_config.l1_chain_id))
    )?;
    let payout_recipient = args
        .proving
        .payout_recipient_address
        .unwrap_or_else(|| validator_wallet.default_signer().address());
    info!("Proof payout recipient: {payout_recipient}");

    let task_channel: AsyncChannel<Task> = async_channel::unbounded();
    let mut proving_handlers = vec![];
    // instantiate worker pool
    for _ in 0..args.num_concurrent_provers {
        proving_handlers.push(spawn(handle_proving_tasks(
            args.kailua_cli.clone(),
            task_channel.clone(),
            channel.sender.clone(),
            verbosity,
        )));
    }

    // Run task generator loop
    loop {
        // Dequeue messages
        let Some(Message::Proposal {
            index: proposal_index,
            precondition_validation_data,
            l1_head,
            agreed_l2_head_hash,
            agreed_l2_output_root,
            claimed_l2_block_number,
            claimed_l2_output_root,
        }) = channel.receiver.recv().await
        else {
            // The channel was closed because the handle_proposals loop ended
            break;
        };
        info!("Processing proof for local index {proposal_index}.");
        // Compute proof file name
        let precondition_hash = precondition_validation_data
            .as_ref()
            .map(|d| d.precondition_hash())
            .unwrap_or_default();
        let proof_journal = ProofJournal {
            payout_recipient,
            precondition_hash,
            l1_head,
            agreed_l2_output_root,
            claimed_l2_output_root,
            claimed_l2_block_number,
            config_hash,
            fpvm_image_id,
        };
        let file_name = proof_file_name(&proof_journal);
        // Prepare proving args
        let (precondition_params, precondition_block_hashes, precondition_blob_hashes) =
            precondition_validation_data
                .map(|data| {
                    let (block_hashes, blob_hashes): (Vec<_>, Vec<_>) = data
                        .blob_fetch_requests()
                        .iter()
                        .map(|r| (r.block_ref.hash, r.blob_hash.hash))
                        .unzip();
                    let PreconditionValidationData::Validity {
                        proposal_l2_head_number,
                        proposal_output_count,
                        output_block_span,
                        ..
                    } = data;
                    let params = vec![
                        proposal_l2_head_number,
                        proposal_output_count,
                        output_block_span,
                    ];
                    (params, block_hashes, blob_hashes)
                })
                .unwrap_or_default();
        let data_dir = data_dir.join(format!(
            "{}-{}",
            &agreed_l2_output_root.to_string()[..10].to_string(),
            &claimed_l2_output_root.to_string()[..10].to_string()
        ));
        let prove_args = ProveArgs {
            kona: kona_host::single::SingleChainHost {
                l1_head,
                agreed_l2_head_hash,
                agreed_l2_output_root,
                claimed_l2_output_root,
                claimed_l2_block_number,
                l2_node_address: Some(args.sync.provider.op_geth_url.clone()),
                l1_node_address: Some(args.sync.provider.eth_rpc_url.clone()),
                l1_beacon_address: Some(args.sync.provider.beacon_rpc_url.clone()),
                data_dir: Some(data_dir),
                native: true,
                server: false,
                l2_chain_id: Some(rollup_config.l2_chain_id),
                rollup_config_path: None,
                enable_experimental_witness_endpoint: args.enable_experimental_witness_endpoint,
            },
            op_node_address: Some(args.sync.provider.op_node_url.clone()),
            proving: ProvingArgs {
                payout_recipient_address: Some(payout_recipient),
                ..args.proving.clone()
            },
            boundless: args.boundless.clone(),
            precondition_params,
            precondition_block_hashes,
            precondition_blob_hashes,
            telemetry: args.sync.telemetry.clone(),
        };
        // Send to task pool
        task_channel
            .0
            .send(Task {
                proposal_index,
                prove_args,
                proof_file_name: file_name,
            })
            .await
            .context("task channel closed")?;
    }

    // Close the task queuing channel to prevent retries
    task_channel.0.close();

    // Wait for all proving tasks to finish
    for handler in proving_handlers {
        if let Err(err) = handler.await {
            error!("Error joining proving task handler: {err:?}");
        }
    }

    warn!("handle_proof_requests terminated");
    Ok(())
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
