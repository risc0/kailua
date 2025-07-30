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

use crate::args::ProveArgs;
use crate::channel::AsyncChannel;
use crate::config::generate_rollup_config_file;
use crate::kv::create_disk_kv_store;
use crate::preflight::{concurrent_execution_preflight, fetch_precondition_data};
use crate::tasks::{handle_oneshot_tasks, Cached, Oneshot, OneshotResult};
use crate::ProvingError;
use alloy::eips::BlockNumberOrTag;
use alloy::providers::{Provider, RootProvider};
use alloy_primitives::B256;
use anyhow::{anyhow, bail, Context};
use human_bytes::human_bytes;
use kailua_common::boot::StitchedBootInfo;
use kailua_sync::provider::optimism::OpNodeProvider;
use kailua_sync::{await_tel, retry_res_ctx_timeout};
use opentelemetry::global::tracer;
use opentelemetry::trace::FutureExt;
use opentelemetry::trace::{TraceContextExt, Tracer};
use std::collections::BinaryHeap;
use std::env::set_var;
use tempfile::tempdir;
use tracing::{error, info, warn};

pub async fn prove(mut args: ProveArgs) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("prove"));

    // fetch starting block number
    let l2_provider = if args.kona.is_offline() {
        None
    } else {
        Some(
            retry_res_ctx_timeout!(20, args.kona.create_providers().await)
                .await
                .l2,
        )
    };
    let op_node_provider = args.op_node_address.as_ref().map(|addr| {
        OpNodeProvider(RootProvider::new_http(
            addr.as_str()
                .try_into()
                .expect("Failed to parse op_node_address"),
        ))
    });

    // set tmp data dir if data dir unset
    let tmp_dir = tempdir().map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    if args.kona.data_dir.is_none() {
        args.kona.data_dir = Some(tmp_dir.path().to_path_buf());
    }

    // fetch rollup config
    let rollup_config = generate_rollup_config_file(&mut args, &tmp_dir)
        .await
        .context("generate_rollup_config")
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;

    // preload precondition data into KV store
    let (precondition_hash, precondition_validation_data_hash) =
        match fetch_precondition_data(&args)
            .await
            .map_err(|e| ProvingError::OtherError(anyhow!(e)))?
        {
            Some(data) => {
                let precondition_validation_data_hash = data.hash();
                set_var(
                    "PRECONDITION_VALIDATION_DATA_HASH",
                    precondition_validation_data_hash.to_string(),
                );
                (data.precondition_hash(), precondition_validation_data_hash)
            }
            None => (B256::ZERO, B256::ZERO),
        };

    // create concurrent db
    let disk_kv_store = create_disk_kv_store(&args.kona);
    // perform preflight to fetch data
    if args.proving.num_concurrent_preflights > 0 {
        // run parallelized preflight instances to populate kv store
        info!(
            "Running concurrent preflights with {} threads",
            args.proving.num_concurrent_preflights
        );
        concurrent_execution_preflight(
            &args,
            rollup_config.clone(),
            op_node_provider.as_ref().expect("Missing op_node_provider"),
            disk_kv_store.clone(),
        )
        .await
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    }
    // We only use executionWitness/executePayload during preflight.
    args.kona.enable_experimental_witness_endpoint = false;

    // spin up proving workers
    let task_channel: AsyncChannel<Oneshot> = async_channel::unbounded();
    let mut proving_handlers = vec![];
    for _ in 0..args.proving.num_concurrent_proofs {
        proving_handlers.push(tokio::spawn(handle_oneshot_tasks(task_channel.1.clone())));
    }
    let mut result_pq = BinaryHeap::new();
    let mut num_proofs = 1;

    // create channel for receiving proving results from handlers
    let result_channel = async_channel::unbounded();
    // create channel for receiving proof requests to process and dispatch to handlers
    let prover_channel = async_channel::unbounded();
    prover_channel
        .0
        .send((false, args.clone()))
        .await
        .expect("Failed to send prover task");
    while result_pq.len() < num_proofs {
        // dispatch all pending proofs
        while !prover_channel.1.is_empty() {
            let (have_split, job_args) = prover_channel
                .1
                .recv()
                .await
                .expect("Failed to recv prover task");

            let starting_block = if let Some(l2_provider) = l2_provider.as_ref() {
                await_tel!(
                    context,
                    tracer,
                    "l2_provider get_block_by_hash starting_block",
                    retry_res_ctx_timeout!(l2_provider
                        .get_block_by_hash(job_args.kona.agreed_l2_head_hash)
                        .await
                        .context("l2_provider get_block_by_hash starting_block")?
                        .ok_or_else(|| anyhow!("Failed to fetch starting block")))
                )
                .header
                .number
            } else {
                0
            };

            let num_blocks = job_args.kona.claimed_l2_block_number - starting_block;
            if starting_block > 0 {
                info!(
                    "Processing (split={have_split}) job with {} blocks from block {}",
                    num_blocks, starting_block
                );
            }
            // Force the proving attempt regardless of witness size if we prove just one block
            let force_attempt = num_blocks == 1 || job_args.kona.is_offline();

            // spawn a job that computes the proof and sends back the result to result_channel
            let rollup_config = rollup_config.clone();
            let disk_kv_store = disk_kv_store.clone();
            let task_channel = task_channel.clone();
            let result_channel = result_channel.clone();
            tokio::spawn(async move {
                let result = crate::tasks::compute_fpvm_proof(
                    job_args.clone(),
                    rollup_config,
                    disk_kv_store,
                    precondition_hash,
                    precondition_validation_data_hash,
                    vec![],
                    vec![],
                    !have_split,
                    task_channel.0.clone(),
                )
                .await;

                result_channel
                    .0
                    .clone()
                    .send((starting_block, job_args, force_attempt, result))
                    .await
                    .expect("Failed to send fpvm proof result");
            });
        }

        // receive and process new results
        let (starting_block, job_args, force_attempt, result) = result_channel
            .1
            .recv()
            .await
            .expect("Failed to recv prover task");
        let num_blocks = job_args.kona.claimed_l2_block_number - starting_block;
        let last_block = job_args.kona.claimed_l2_block_number;

        match result {
            Ok(proof) => {
                let cached = Cached {
                    // used for sorting
                    args: job_args.clone(),
                    // all unused
                    rollup_config: rollup_config.clone(),
                    disk_kv_store: disk_kv_store.clone(),
                    precondition_hash,
                    precondition_validation_data_hash,
                    stitched_executions: vec![],
                    stitched_boot_info: vec![],
                    stitched_proofs: vec![],
                    prove_snark: false,
                    force_attempt,
                    seek_proof: true,
                };
                if proof.is_some() {
                    info!(
                        "Successfully proved {num_blocks} blocks ({starting_block}..{last_block})",
                    );
                } else {
                    error!(
                        "Failed to create complete proof for {num_blocks} blocks ({starting_block}..{last_block})",
                    );
                }
                let result = proof
                    .ok_or_else(|| ProvingError::OtherError(anyhow!("Missing complete proof.")));
                // enqueue result to reach the termination condition
                result_pq.push(OneshotResult { cached, result });
            }
            Err(err) => {
                // Handle error case
                match err {
                    ProvingError::WitnessSizeError(f, t, ..) => {
                        if force_attempt {
                            bail!(
                                "Received WitnessSizeError({f},{t}) for a forced proving attempt: {err:?}"
                                );
                        }
                        warn!(
                            "Proof witness size {} above safety threshold {}. Splitting workload.",
                            human_bytes(f as f64),
                            human_bytes(t as f64),
                        )
                    }
                    ProvingError::ExecutionError(e) => {
                        if force_attempt {
                            bail!("Irrecoverable ZKVM execution error: {e:?}")
                        }
                        warn!("Splitting proof after ZKVM execution error: {e:?}")
                    }
                    ProvingError::OtherError(e) => {
                        if e.root_cause()
                            .to_string()
                            .contains("Expected zero claim hash")
                        {
                            // we use this special exit code to signal an insufficient l1 head
                            error!("Insufficient L1 head.");
                            // todo: revisit
                            std::process::exit(111);
                        }
                        bail!("Irrecoverable proving error: {e:?}")
                    }
                    ProvingError::NotSeekingProof(..) => {
                        unreachable!("NotSeekingProof bubbled up")
                    }
                    ProvingError::DerivationProofError(proofs) => {
                        info!(
                            "Successfully proved execution-only for {num_blocks} blocks ({starting_block}..{last_block}) over {proofs} proofs",
                        );
                        num_proofs -= 1;
                        continue;
                    }
                    ProvingError::NotAwaitingProof => {
                        info!(
                            "Skipped awaiting proof for {num_blocks} blocks ({starting_block}..{last_block})",
                        );
                        num_proofs -= 1;
                        continue;
                    }
                }
                // Require additional proof
                num_proofs += 1;
                // Split workload at midpoint (num_blocks > 1)
                let mid_point = starting_block + num_blocks / 2;
                let op_node_provider = op_node_provider.as_ref().expect("Missing op_node_provider");
                let mid_output = await_tel!(
                    context,
                    tracer,
                    "op_node_provider output_at_block mid_output",
                    retry_res_ctx_timeout!(op_node_provider
                        .output_at_block(mid_point)
                        .await
                        .context("op_node_provider output_at_block mid_output"))
                );
                let l2_provider = l2_provider.as_ref().expect("Missing l2_provider");
                let mid_block = await_tel!(
                    context,
                    tracer,
                    "l2_provider get_block_by_number mid_block",
                    retry_res_ctx_timeout!(l2_provider
                        .get_block_by_number(BlockNumberOrTag::Number(mid_point))
                        .await
                        .context("l2_provider get_block_by_number mid_block")?
                        .ok_or_else(|| anyhow!("Block {mid_point} not found")))
                );
                // Lower half workload ends at midpoint (inclusive)
                let mut lower_job_args = job_args.clone();
                lower_job_args.kona.claimed_l2_output_root = mid_output;
                lower_job_args.kona.claimed_l2_block_number = mid_point;
                prover_channel
                    .0
                    .send((true, lower_job_args))
                    .await
                    .expect("Failed to send prover task");
                // upper half workload starts after midpoint
                let mut upper_job_args = job_args;
                upper_job_args.kona.agreed_l2_output_root = mid_output;
                upper_job_args.kona.agreed_l2_head_hash = mid_block.header.hash;
                prover_channel
                    .0
                    .send((true, upper_job_args))
                    .await
                    .expect("Failed to send prover task");
            }
        }
    }

    if !args.proving.skip_stitching() {
        // gather sorted proofs into vec
        let proofs = result_pq
            .into_sorted_vec()
            .into_iter()
            .rev()
            .map(|r| r.result.expect("Failed to get result"))
            .collect::<Vec<_>>();

        // stitch contiguous proofs together
        if proofs.len() > 1 {
            info!("Composing {} proofs together.", proofs.len());
            // construct a proving instruction with no blocks to derive
            let mut base_args = args;
            {
                // set last block as starting point
                base_args.kona.agreed_l2_output_root = base_args.kona.claimed_l2_output_root;
                let l2_provider = l2_provider.as_ref().unwrap();
                base_args.kona.agreed_l2_head_hash = await_tel!(
                    context,
                    tracer,
                    "l2_provider get_block_by_number claimed_l2_block_number",
                    retry_res_ctx_timeout!(l2_provider
                        .get_block_by_number(BlockNumberOrTag::Number(
                            base_args.kona.claimed_l2_block_number,
                        ))
                        .await
                        .context("l2_provider get_block_by_number claimed_l2_block_number")?
                        .ok_or_else(|| anyhow!("Claimed L2 block not found")))
                )
                .header
                .hash;
            }
            // construct a list of boot info to backward stitch
            let stitched_boot_info = proofs
                .iter()
                .map(StitchedBootInfo::from)
                .collect::<Vec<_>>();

            crate::tasks::compute_fpvm_proof(
                base_args,
                rollup_config.clone(),
                disk_kv_store.clone(),
                precondition_hash,
                precondition_validation_data_hash,
                stitched_boot_info,
                proofs,
                true,
                task_channel.0.clone(),
            )
            .await
            .context("Failed to compute FPVM proof.")?;
        }
    }

    info!("Exiting prover program.");
    Ok(())
}
