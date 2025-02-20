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
use crate::config::generate_rollup_config;
use crate::preflight::{fetch_precondition_data, zeth_execution_preflight};
use alloy::network::primitives::BlockTransactionsKind;
use alloy::providers::Provider;
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::B256;
use anyhow::{anyhow, Context};
use kailua_build::KAILUA_FPVM_ID;
use kailua_client::proof::{proof_file_name, read_proof_file};
use kailua_client::proving::ProvingError;
use kailua_common::client::stitching::{split_executions, stitch_boot_info};
use kailua_common::executor::{exec_precondition_hash, Execution};
use kailua_common::proof::Proof;
use kailua_common::witness::StitchedBootInfo;
use kona_proof::BootInfo;
use maili_genesis::RollupConfig;
use std::collections::BinaryHeap;
use std::env::set_var;
use std::path::Path;
use tempfile::tempdir;
use tracing::{info, warn};

/// Computes a receipt if it is not cached
pub async fn compute_fpvm_proof(
    mut args: KailuaHostArgs,
    stitched_boot_info: Vec<StitchedBootInfo>,
    stitched_proofs: Vec<Proof>,
    prove_snark: bool,
    _force_attempt: bool,
) -> Result<Proof, ProvingError> {
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
    // fetch rollup config
    let tmp_dir = tempdir().map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    let rollup_config = generate_rollup_config(&mut args, &tmp_dir)
        .await
        .context("generate_rollup_config")
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    // set tmp data dir if unset
    if args.kona.data_dir.is_none() {
        args.kona.data_dir = Some(tmp_dir.path().to_path_buf());
    }
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
            transactions += providers
                .l2
                .get_block_transaction_count_by_number(BlockNumberOrTag::Number(
                    starting_block + i + 1,
                ))
                .await
                .map_err(|e| ProvingError::OtherError(anyhow!(e)))?
                .expect("Failed to get transaction count for block {i}");
        }
        info!(
            "Proving {} transactions over {} blocks.",
            transactions, block_count
        );
    }

    //  1. try entire proof
    //      on failure, take execution trace
    //  2. try derivation-only proof
    //      on failure, report witness size error unless forced attempt
    //  3. compute series of execution-only proofs
    //  4. compute derivation-proof with stitched executions

    // generate master proof
    let stitching_only = args.kona.agreed_l2_output_root == args.kona.claimed_l2_output_root;
    info!("Attempting complete proof.");
    let proof_result = compute_cached_proof(
        args.clone(),
        rollup_config.clone(),
        precondition_hash,
        precondition_validation_data_hash,
        vec![],
        stitched_boot_info.clone(),
        stitched_proofs.clone(),
        prove_snark,
        stitching_only,
        true,
    )
    .await;
    // on WitnessSizeError, extract execution trace
    let Err(ProvingError::WitnessSizeError(_, _, executed_blocks)) = proof_result else {
        return proof_result;
    };
    // perform derivation-only run without seeking a proof
    let (_, execution_cache) = split_executions(executed_blocks.clone());
    info!(
        "Performing derivation-only run for {} executions.",
        execution_cache.len()
    );
    let derivation_only_result = compute_cached_proof(
        args.clone(),
        rollup_config.clone(),
        precondition_hash,
        precondition_validation_data_hash,
        executed_blocks.clone(),
        stitched_boot_info.clone(),
        stitched_proofs.clone(),
        false,
        true,
        false,
    )
    .await;
    // propagate unexpected error up on failure to trigger higher-level division
    let Err(ProvingError::SeekProofError(..)) = derivation_only_result else {
        warn!(
            "Unexpected derivation-only result (is_ok={}).",
            derivation_only_result.is_ok()
        );
        return derivation_only_result;
    };
    // compute execution proofs
    let mut job_pq = BinaryHeap::new();
    let mut proofs = Vec::new();
    // start with full execution proof
    job_pq.push({
        let mut args = args.clone();
        args.kona.l1_head = B256::ZERO;
        args
    });
    // divide and conquer executions
    while let Some(job_args) = job_pq.pop() {
        let starting_block = execution_cache
            .iter()
            .find(|e| e.agreed_output == job_args.kona.agreed_l2_output_root)
            .expect("Failed to find the first execution.")
            .artifacts
            .block_header
            .number
            - 1;
        let num_blocks = job_args.kona.claimed_l2_block_number - starting_block;
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
                    && executed_block_number <= job_args.kona.claimed_l2_block_number
            })
            .cloned()
            .collect::<Vec<_>>();
        let precondition_hash = exec_precondition_hash(executed_blocks.as_slice());

        // Force the proving attempt regardless of witness size if we prove just one block
        let force_attempt = num_blocks == 1;
        let stitched_executions = vec![executed_blocks
            .iter()
            .map(|a| a.as_ref().clone())
            .collect::<Vec<_>>()];
        match compute_cached_proof(
            job_args.clone(),
            rollup_config.clone(),
            precondition_hash,
            B256::ZERO,
            stitched_executions,
            vec![],
            vec![],
            false,
            force_attempt,
            true,
        )
        .await
        {
            Ok(proof) => {
                // conquered
                proofs.push(proof);
            }
            Err(err) => {
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
                }
                // Split workload at midpoint (num_blocks > 1)
                let mid_point = starting_block + num_blocks / 2;
                let mid_exec = executed_blocks
                    .iter()
                    .find(|e| e.artifacts.block_header.number == mid_point)
                    .expect("Failed to find the midpoint of execution.");
                let mid_output = mid_exec.claimed_output;
                // Lower half workload ends at midpoint (inclusive)
                let mut lower_job_args = job_args.clone();
                lower_job_args.kona.claimed_l2_output_root = mid_output;
                lower_job_args.kona.claimed_l2_block_number = mid_point;
                job_pq.push(lower_job_args);
                // upper half workload starts after midpoint
                let mut upper_job_args = job_args;
                upper_job_args.kona.agreed_l2_output_root = mid_output;
                upper_job_args.kona.agreed_l2_head_hash = mid_exec.artifacts.block_header.hash();
                job_pq.push(upper_job_args);
            }
        }
    }

    // Combine execution proofs with derivation proof
    info!(
        "Combining {} execution proofs with derivation proof.",
        proofs.len()
    );
    compute_cached_proof(
        args,
        rollup_config,
        precondition_hash,
        precondition_validation_data_hash,
        executed_blocks,
        stitched_boot_info,
        [stitched_proofs, proofs].concat(),
        prove_snark,
        true,
        true,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn compute_cached_proof(
    args: KailuaHostArgs,
    rollup_config: RollupConfig,
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
        // run zeth preflight to fetch the necessary preimages
        if !args.skip_zeth_preflight {
            zeth_execution_preflight(&args, boot.rollup_config)
                .await
                .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
        }

        // generate a proof using the kailua client and kona server
        crate::server::start_server_and_native_client(
            args,
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
