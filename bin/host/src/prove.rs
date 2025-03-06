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

use crate::args::KailuaHostArgs;
use crate::kv::RWLKeyValueStore;
use crate::tasks;
use crate::tasks::{Cached, Oneshot};
use alloy::network::primitives::BlockTransactionsKind;
use alloy::providers::Provider;
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::B256;
use anyhow::{anyhow, Context};
use async_channel::Sender;
use kailua_build::KAILUA_FPVM_ID;
use kailua_client::proof::{proof_file_name, read_proof_file};
use kailua_client::proving::ProvingError;
use kailua_common::client::stitching::{split_executions, stitch_boot_info};
use kailua_common::executor::{exec_precondition_hash, Execution};
use kailua_common::proof::Proof;
use kailua_common::witness::StitchedBootInfo;
use kona_genesis::RollupConfig;
use kona_proof::BootInfo;
use std::collections::BinaryHeap;
use std::path::Path;
use std::sync::Arc;
use tracing::{info, warn};

/// Computes a receipt if it is not cached
#[allow(clippy::too_many_arguments)]
pub async fn compute_fpvm_proof(
    args: KailuaHostArgs,
    rollup_config: RollupConfig,
    disk_kv_store: Option<RWLKeyValueStore>,
    precondition_hash: B256,
    precondition_validation_data_hash: B256,
    stitched_boot_info: Vec<StitchedBootInfo>,
    stitched_proofs: Vec<Proof>,
    prove_snark: bool,
    task_sender: Sender<Oneshot>,
) -> Result<Option<Proof>, ProvingError> {
    // report transaction count
    if !stitched_boot_info.is_empty() {
        info!("Stitching {} sub-proofs", stitched_boot_info.len());
    }
    if !args.kona.is_offline() {
        let providers = args
            .kona
            .create_providers()
            .await
            .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
        let mut transactions = 0;
        let mut gas = 0;
        let starting_block = providers
            .l2
            .get_block_by_hash(args.kona.agreed_l2_head_hash, BlockTransactionsKind::Hashes)
            .await
            .map_err(|e| ProvingError::OtherError(anyhow!(e)))?
            .unwrap()
            .header
            .number;
        let block_count = args.kona.claimed_l2_block_number - starting_block;
        for i in 0..block_count {
            let block = providers
                .l2
                .get_block_by_number(
                    BlockNumberOrTag::Number(starting_block + i + 1),
                    BlockTransactionsKind::Hashes,
                )
                .await
                .map_err(|e| ProvingError::OtherError(anyhow!(e)))?
                .expect("Failed to get transaction count for block {i}");
            transactions += block.transactions.len();
            gas += block.header.gas_used;
        }
        info!("Proving {transactions} transactions for {gas} gas over {block_count} blocks.");
    }

    //  1. try entire proof
    //      on failure, take execution trace
    //  2. try derivation-only proof
    //      on failure, report error
    //  3. compute series of execution-only proofs
    //  4. compute derivation-proof with stitched executions

    let stitching_only = args.kona.agreed_l2_output_root == args.kona.claimed_l2_output_root;
    // generate master proof
    info!("Attempting complete proof.");
    let complete_proof_result = tasks::compute_oneshot_task(
        args.clone(),
        rollup_config.clone(),
        disk_kv_store.clone(),
        precondition_hash,
        precondition_validation_data_hash,
        vec![],
        stitched_boot_info.clone(),
        stitched_proofs.clone(),
        // pass through snark requirement
        prove_snark,
        // force attempting to compute the proof if it only combines boot infos
        stitching_only,
        // skip seeking a complete proof if skipping derivation
        !args.proving.skip_derivation_proof,
        task_sender.clone(),
    )
    .await;
    // on WitnessSizeError or SeekProofError, extract execution trace
    let executed_blocks = match complete_proof_result {
        Err(ProvingError::WitnessSizeError(_, _, executed_blocks)) => executed_blocks,
        Err(ProvingError::SeekProofError(_, executed_blocks)) => executed_blocks,
        other_result => return Ok(Some(other_result?)),
    };
    // flatten executed l2 blocks
    let (_, execution_cache) = split_executions(executed_blocks.clone());

    // perform a derivation-only run to check its provability
    if !args.proving.skip_derivation_proof {
        info!(
            "Performing derivation-only run for {} executions.",
            execution_cache.len()
        );
        let derivation_only_result = tasks::compute_oneshot_task(
            args.clone(),
            rollup_config.clone(),
            disk_kv_store.clone(),
            precondition_hash,
            precondition_validation_data_hash,
            executed_blocks.clone(),
            stitched_boot_info.clone(),
            stitched_proofs.clone(),
            false,
            true,
            false,
            task_sender.clone(),
        )
        .await;
        // propagate unexpected error up on failure to trigger higher-level division
        let Err(ProvingError::SeekProofError(..)) = derivation_only_result else {
            warn!(
                "Unexpected derivation-only result (is_ok={}).",
                derivation_only_result.is_ok()
            );
            return Ok(Some(derivation_only_result?));
        };
    }

    // create proofs channel
    let result_channel = async_channel::unbounded();
    let mut result_pq = BinaryHeap::new();
    // start with full execution proof
    task_sender
        .send(Oneshot {
            cached_task: create_cached_execution_task(
                {
                    let mut args = args.clone();
                    args.kona.l1_head = B256::ZERO;
                    args
                },
                rollup_config.clone(),
                disk_kv_store.clone(),
                &execution_cache,
            ),
            result_sender: result_channel.0.clone(),
        })
        .await
        .expect("task_channel should not be closed");
    // divide and conquer executions
    let mut num_proofs = 1;
    loop {
        // Break if we've received all proofs
        if result_pq.len() == num_proofs {
            break;
        }
        // Wait for more proving results
        let oneshot_result = result_channel
            .1
            .recv()
            .await
            .expect("result_channel should not be closed");
        let Err(err) = oneshot_result.result else {
            result_pq.push(oneshot_result);
            continue;
        };
        // Require additional proof
        num_proofs += 1;
        let executed_blocks = oneshot_result.cached.stitched_executions[0].clone();
        let starting_block = executed_blocks[0].artifacts.block_header.number - 1;
        let num_blocks = oneshot_result.cached.args.kona.claimed_l2_block_number - starting_block;
        let force_attempt = num_blocks == 1;
        // divide or bail out on error
        match err {
            ProvingError::WitnessSizeError(f, t, e) => {
                if force_attempt {
                    return Err(ProvingError::WitnessSizeError(f, t, e));
                }
                warn!("Proof witness size {f} above safety threshold {t}. Splitting workload.")
            }
            ProvingError::ExecutionError(e) => {
                if force_attempt {
                    return Err(ProvingError::ExecutionError(e));
                }
                warn!("Splitting proof after ZKVM execution error: {e:?}")
            }
            ProvingError::OtherError(e) => {
                return Err(ProvingError::OtherError(e));
            }
            ProvingError::SeekProofError(_, _) => {
                unreachable!("Sought proof, found SeekProofError {err:?}")
            }
            ProvingError::DerivationProofError(_) => {
                unreachable!("Sought proof, found DerivationProofError {err:?}")
            }
        }
        // Split workload at midpoint (num_blocks > 1)
        let mid_point = starting_block + num_blocks / 2;
        let mid_exec = executed_blocks
            .iter()
            .find(|e| e.artifacts.block_header.number == mid_point)
            .expect("Failed to find the midpoint of execution.");
        let mid_output = mid_exec.claimed_output;

        // Lower half workload ends at midpoint (inclusive)
        let mut lower_job_args = oneshot_result.cached.args.clone();
        lower_job_args.kona.claimed_l2_output_root = mid_output;
        lower_job_args.kona.claimed_l2_block_number = mid_point;
        task_sender
            .send(Oneshot {
                cached_task: create_cached_execution_task(
                    lower_job_args,
                    rollup_config.clone(),
                    disk_kv_store.clone(),
                    &execution_cache,
                ),
                result_sender: result_channel.0.clone(),
            })
            .await
            .expect("task_channel should not be closed");

        // upper half workload starts after midpoint
        let mut upper_job_args = oneshot_result.cached.args;
        upper_job_args.kona.agreed_l2_output_root = mid_output;
        upper_job_args.kona.agreed_l2_head_hash = mid_exec.artifacts.block_header.hash();
        task_sender
            .send(Oneshot {
                cached_task: create_cached_execution_task(
                    upper_job_args,
                    rollup_config.clone(),
                    disk_kv_store.clone(),
                    &execution_cache,
                ),
                result_sender: result_channel.0.clone(),
            })
            .await
            .expect("task_channel should not be closed");
    }
    // Read result_pq for stitched executions and proofs
    let (proofs, stitched_executions): (Vec<_>, Vec<_>) = result_pq
        .into_iter()
        .map(|mut r| {
            (
                r.result.expect("pushed failing result to queue"),
                r.cached.stitched_executions.pop().unwrap(),
            )
        })
        .unzip();

    // Return no proof if derivation is not required
    if args.proving.skip_derivation_proof {
        return Ok(None);
    }

    // Combine execution proofs with derivation proof
    let total_blocks = stitched_executions.iter().map(|e| e.len()).sum::<usize>();
    info!(
        "Combining {}/{} execution proofs for {total_blocks} blocks with derivation proof.",
        proofs.len(),
        stitched_executions.len()
    );
    Ok(Some(
        tasks::compute_oneshot_task(
            args,
            rollup_config,
            disk_kv_store,
            precondition_hash,
            precondition_validation_data_hash,
            stitched_executions,
            stitched_boot_info,
            [stitched_proofs, proofs].concat(),
            prove_snark,
            true,
            true,
            task_sender.clone(),
        )
        .await?,
    ))
}

pub fn create_cached_execution_task(
    args: KailuaHostArgs,
    rollup_config: RollupConfig,
    disk_kv_store: Option<RWLKeyValueStore>,
    execution_cache: &[Arc<Execution>],
) -> Cached {
    let starting_block = execution_cache
        .iter()
        .find(|e| e.agreed_output == args.kona.agreed_l2_output_root)
        .expect("Failed to find the first execution.")
        .artifacts
        .block_header
        .number
        - 1;
    let num_blocks = args.kona.claimed_l2_block_number - starting_block;
    info!(
        "Processing execution-only job with {} blocks from block {}",
        num_blocks, starting_block
    );
    // Extract executed slice
    let executed_blocks = execution_cache
        .iter()
        .filter(|e| {
            let executed_block_number = e.artifacts.block_header.number;

            starting_block < executed_block_number
                && executed_block_number <= args.kona.claimed_l2_block_number
        })
        .cloned()
        .collect::<Vec<_>>();
    let precondition_hash = exec_precondition_hash(executed_blocks.as_slice());

    // Force the proving attempt regardless of witness size if we prove just one block
    let force_attempt = num_blocks == 1;
    let executed_blocks = executed_blocks
        .iter()
        .map(|a| a.as_ref().clone())
        .collect::<Vec<_>>();

    Cached {
        args,
        rollup_config,
        disk_kv_store,
        precondition_hash,
        precondition_validation_data_hash: B256::ZERO,
        stitched_executions: vec![executed_blocks],
        stitched_boot_info: vec![],
        stitched_proofs: vec![],
        prove_snark: false,
        force_attempt,
        seek_proof: true,
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn compute_cached_proof(
    args: KailuaHostArgs,
    rollup_config: RollupConfig,
    disk_kv_store: Option<RWLKeyValueStore>,
    precondition_hash: B256,
    precondition_validation_data_hash: B256,
    stitched_executions: Vec<Vec<Execution>>,
    stitched_boot_info: Vec<StitchedBootInfo>,
    stitched_proofs: Vec<Proof>,
    prove_snark: bool,
    force_attempt: bool,
    seek_proof: bool,
) -> Result<Proof, ProvingError> {
    // extract single chain kona config
    let boot = BootInfo {
        l1_head: args.kona.l1_head,
        agreed_l2_output_root: args.kona.agreed_l2_output_root,
        claimed_l2_output_root: args.kona.claimed_l2_output_root,
        claimed_l2_block_number: args.kona.claimed_l2_block_number,
        chain_id: rollup_config.l2_chain_id,
        rollup_config,
    };
    // Construct expected journal
    let proof_journal = stitch_boot_info(
        &boot,
        bytemuck::cast::<[u32; 8], [u8; 32]>(KAILUA_FPVM_ID).into(),
        args.proving.payout_recipient_address.unwrap_or_default(),
        precondition_hash,
        stitched_boot_info.clone(),
    );
    // Skip computation if previously saved to disk
    let proof_file_name = proof_file_name(&proof_journal);
    if matches!(Path::new(&proof_file_name).try_exists(), Ok(true)) && seek_proof {
        info!("Proving skipped. Proof file {proof_file_name} already exists.");
    } else {
        info!("Computing uncached proof.");

        // generate a proof using the kailua client and kona server
        crate::server::start_server_and_native_client(
            args,
            disk_kv_store,
            precondition_validation_data_hash,
            stitched_executions,
            stitched_boot_info,
            stitched_proofs,
            prove_snark,
            force_attempt,
            seek_proof,
        )
        .await?;
    }

    read_proof_file(&proof_file_name)
        .await
        .context(format!(
            "Failed to read proof file {proof_file_name} contents."
        ))
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))
}
