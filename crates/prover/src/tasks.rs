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
use crate::kv::RWLKeyValueStore;
use crate::proof::{proof_file_name, read_bincoded_file};
use crate::ProvingError;
use alloy::providers::RootProvider;
use alloy_primitives::B256;
use anyhow::{anyhow, Context};
use async_channel::{Receiver, Sender};
use human_bytes::human_bytes;
use kailua_build::KAILUA_FPVM_KONA_ID;
use kailua_kona::boot::StitchedBootInfo;
use kailua_kona::client::stitching::{split_executions, stitch_boot_info};
use kailua_kona::executor::{exec_precondition_hash, Execution};
use kailua_sync::provider::optimism::OpNodeProvider;
use kona_genesis::RollupConfig;
use kona_proof::BootInfo;
use risc0_zkvm::Receipt;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::convert::identity;
use std::path::Path;
use std::sync::Arc;
use tracing::{error, info, warn};

#[derive(Clone, Debug)]
pub struct Cached {
    pub args: ProveArgs,
    pub rollup_config: RollupConfig,
    pub disk_kv_store: Option<RWLKeyValueStore>,
    pub precondition_hash: B256,
    pub precondition_validation_data_hash: B256,
    pub stitched_executions: Vec<Vec<Execution>>,
    pub stitched_boot_info: Vec<StitchedBootInfo>,
    pub stitched_proofs: Vec<Receipt>,
    pub prove_snark: bool,
    pub force_attempt: bool,
    pub seek_proof: bool,
}

impl PartialEq for Cached {
    fn eq(&self, other: &Self) -> bool {
        self.args.eq(&other.args)
    }
}

impl Eq for Cached {}

impl PartialOrd for Cached {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Cached {
    fn cmp(&self, other: &Self) -> Ordering {
        self.args.cmp(&other.args)
    }
}

#[derive(Debug)]
pub struct OneshotResult {
    pub cached: Cached,
    pub result: Result<Receipt, ProvingError>,
}

impl PartialEq for OneshotResult {
    fn eq(&self, other: &Self) -> bool {
        self.cached.eq(&other.cached)
    }
}

impl Eq for OneshotResult {}

impl PartialOrd for OneshotResult {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OneshotResult {
    fn cmp(&self, other: &Self) -> Ordering {
        self.cached.cmp(&other.cached)
    }
}

#[derive(Debug)]
pub struct Oneshot {
    pub cached_task: Cached,
    pub result_sender: Sender<OneshotResult>,
}

pub async fn handle_oneshot_tasks(task_receiver: Receiver<Oneshot>) -> anyhow::Result<()> {
    loop {
        let Oneshot {
            cached_task,
            result_sender,
        } = task_receiver
            .recv()
            .await
            .context("task receiver channel closed")?;

        if let Err(res) = result_sender
            .send(OneshotResult {
                cached: cached_task.clone(),
                result: compute_cached_proof(
                    cached_task.args,
                    cached_task.rollup_config,
                    cached_task.disk_kv_store,
                    cached_task.precondition_hash,
                    cached_task.precondition_validation_data_hash,
                    cached_task.stitched_executions,
                    cached_task.stitched_boot_info,
                    cached_task.stitched_proofs,
                    cached_task.prove_snark,
                    cached_task.force_attempt,
                    cached_task.seek_proof,
                )
                .await,
            })
            .await
        {
            error!("failed to send task result: {res:?}");
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn compute_oneshot_task(
    args: ProveArgs,
    rollup_config: RollupConfig,
    disk_kv_store: Option<RWLKeyValueStore>,
    precondition_hash: B256,
    precondition_validation_data_hash: B256,
    stitched_executions: Vec<Vec<Execution>>,
    stitched_boot_info: Vec<StitchedBootInfo>,
    stitched_proofs: Vec<Receipt>,
    prove_snark: bool,
    force_attempt: bool,
    seek_proof: bool,
    task_sender: Sender<Oneshot>,
) -> Result<Receipt, ProvingError> {
    // create proving task
    let cached_task = Cached {
        args,
        rollup_config,
        disk_kv_store,
        precondition_hash,
        precondition_validation_data_hash,
        stitched_executions,
        stitched_boot_info,
        stitched_proofs,
        prove_snark,
        force_attempt,
        seek_proof,
    };
    // create onshot channel
    let oneshot_channel = async_channel::bounded(1);
    // dispatch task to pool
    task_sender
        .send(Oneshot {
            cached_task,
            result_sender: oneshot_channel.0,
        })
        .await
        .expect("Oneshot channel closed");
    // wait for result
    oneshot_channel
        .1
        .recv()
        .await
        .expect("oneshot_channel should never panic")
        .result
}

/// Computes a receipt if it is not cached
#[allow(clippy::too_many_arguments)]
pub async fn compute_fpvm_proof(
    args: ProveArgs,
    rollup_config: RollupConfig,
    disk_kv_store: Option<RWLKeyValueStore>,
    precondition_hash: B256,
    precondition_validation_data_hash: B256,
    stitched_boot_info: Vec<StitchedBootInfo>,
    stitched_proofs: Vec<Receipt>,
    prove_snark: bool,
    task_sender: Sender<Oneshot>,
) -> Result<Option<Receipt>, ProvingError> {
    // report transaction count
    if !stitched_boot_info.is_empty() {
        info!("Stitching {} sub-proofs", stitched_boot_info.len());
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
    let complete_proof_result = compute_oneshot_task(
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
    // on WitnessSizeError or NotSeekingProof, extract execution trace
    let executed_blocks = match complete_proof_result {
        Err(ProvingError::WitnessSizeError(_, _, executed_blocks)) => executed_blocks,
        Err(ProvingError::NotSeekingProof(_, executed_blocks)) => executed_blocks,
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
        let derivation_only_result = compute_oneshot_task(
            args.clone(),
            rollup_config.clone(),
            disk_kv_store.clone(),
            precondition_hash,
            precondition_validation_data_hash,
            executed_blocks.clone(),
            stitched_boot_info.clone(),
            stitched_proofs.clone(),
            false,
            false,
            false,
            task_sender.clone(),
        )
        .await;
        // propagate unexpected error up on failure to trigger higher-level division
        let Err(ProvingError::NotSeekingProof(witness_size, _)) = derivation_only_result else {
            warn!(
                "Unexpected derivation-only result (is_ok={}).",
                derivation_only_result.is_ok()
            );
            return Ok(Some(derivation_only_result?));
        };
        // abort if pure derivation may OOM
        if witness_size > args.proving.max_witness_size {
            warn!(
                "Derivation-only witness size {} exceeds limit {}.",
                human_bytes(witness_size as f64),
                human_bytes(args.proving.max_witness_size as f64)
            );
        } else {
            info!(
                "Derivation-only witness size {}.",
                human_bytes(witness_size as f64)
            );
        }
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
    while result_pq.len() < num_proofs {
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
        let starting_block = executed_blocks[0].artifacts.header.number - 1;
        let num_blocks = oneshot_result.cached.args.kona.claimed_l2_block_number - starting_block;
        let force_attempt = num_blocks == 1;
        // divide or bail out on error
        match err {
            ProvingError::WitnessSizeError(f, t, e) => {
                if force_attempt {
                    error!(
                        "Proof witness size {} above safety threshold {}.",
                        human_bytes(f as f64),
                        human_bytes(t as f64)
                    );
                    return Err(ProvingError::WitnessSizeError(f, t, e));
                }
                warn!(
                    "Proof witness size {} above safety threshold {}. Splitting workload.",
                    human_bytes(f as f64),
                    human_bytes(t as f64)
                )
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
            ProvingError::NotAwaitingProof => {
                // reduce required proofs by two to cancel out prior addition and one more proof
                num_proofs -= 2;
                continue;
            }
            ProvingError::NotSeekingProof(_, _) => {
                unreachable!("Sought proof, found NotSeekingProof {err:?}")
            }
            ProvingError::DerivationProofError(_) => {
                unreachable!("Sought proof, found DerivationProofError {err:?}")
            }
        }
        // Split workload at midpoint (num_blocks > 1)
        let mid_point = starting_block + num_blocks / 2;
        let mid_exec = executed_blocks
            .iter()
            .find(|e| e.artifacts.header.number == mid_point)
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
        upper_job_args.kona.agreed_l2_head_hash = mid_exec.artifacts.header.hash();
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
        .into_sorted_vec()
        .into_iter()
        .map(|mut r| {
            (
                r.result.expect("pushed failing result to queue"),
                r.cached.stitched_executions.pop().unwrap(),
            )
        })
        .unzip();

    // Return proof count without stitching if derivation is not required
    if args.proving.skip_await_proof {
        warn!("Skipping stitching unawaited execution proofs with derivation.");
        return Err(ProvingError::NotAwaitingProof);
    } else if args.proving.skip_derivation_proof {
        let num_proofs = proofs.len();
        warn!("Skipping stitching {num_proofs} execution proofs with derivation.");
        return Err(ProvingError::DerivationProofError(num_proofs));
    }

    // Combine execution proofs with derivation proof
    let total_blocks = stitched_executions.iter().map(|e| e.len()).sum::<usize>();
    info!(
        "Stitching {}/{} execution proofs for {total_blocks} blocks with derivation proof.",
        proofs.len(),
        stitched_executions.len()
    );
    Ok(Some(
        compute_oneshot_task(
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
    args: ProveArgs,
    rollup_config: RollupConfig,
    disk_kv_store: Option<RWLKeyValueStore>,
    execution_cache: &[Arc<Execution>],
) -> Cached {
    let starting_block = execution_cache
        .iter()
        .find(|e| e.agreed_output == args.kona.agreed_l2_output_root)
        .expect("Failed to find the first execution.")
        .artifacts
        .header
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
            let executed_block_number = e.artifacts.header.number;

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
    mut args: ProveArgs,
    rollup_config: RollupConfig,
    disk_kv_store: Option<RWLKeyValueStore>,
    precondition_hash: B256,
    precondition_validation_data_hash: B256,
    stitched_executions: Vec<Vec<Execution>>,
    stitched_boot_info: Vec<StitchedBootInfo>,
    stitched_proofs: Vec<Receipt>,
    prove_snark: bool,
    force_attempt: bool,
    seek_proof: bool,
) -> Result<Receipt, ProvingError> {
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
    let (boot, proof_journal) = stitch_boot_info(
        boot,
        bytemuck::cast::<[u32; 8], [u8; 32]>(KAILUA_FPVM_KONA_ID).into(),
        args.proving.payout_recipient_address.unwrap_or_default(),
        precondition_hash,
        stitched_boot_info.clone(),
    );
    let skip_await_proof = args.proving.skip_await_proof;
    // Skip computation if previously saved to disk
    let file_name = proof_file_name(&proof_journal);
    if Path::new(&file_name).try_exists().is_ok_and(identity) && seek_proof {
        info!("Proving skipped. Proof file {file_name} already exists.");
    } else {
        if seek_proof {
            info!("Computing uncached proof {file_name}.");
        } else {
            info!("Running native client.");
        }

        // preflight
        if args.kona.enable_experimental_witness_endpoint
            && !args.kona.is_offline()
            && args.op_node_address.is_some()
        {
            let l2_provider = args
                .kona
                .l2_node_address
                .as_ref()
                .map(|addr| {
                    RootProvider::new_http(
                        addr.as_str()
                            .try_into()
                            .expect("Failed to parse l2_node_address"),
                    )
                })
                .unwrap();
            let op_node_provider = args
                .op_node_address
                .as_ref()
                .map(|addr| {
                    OpNodeProvider(RootProvider::new_http(
                        addr.as_str()
                            .try_into()
                            .expect("Failed to parse op_node_address"),
                    ))
                })
                .unwrap();
            if crate::client::payload::run_payload_client(
                boot,
                l2_provider,
                op_node_provider,
                disk_kv_store.clone(),
            )
            .await
            .map_err(|e| ProvingError::OtherError(anyhow!(e)))?
            {
                // If we have used debug_executionWitness sucessfully then don't use Kona's
                // debug_executePayload logic as it doesn't have caching
                args.kona.enable_experimental_witness_endpoint = false;
            }
        }

        // generate a proof using the kailua client and kona server
        crate::client::native::run_native_client(
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

    if skip_await_proof {
        // this can be reached if proof file is cached
        return Err(ProvingError::NotAwaitingProof);
    }

    read_bincoded_file(&file_name)
        .await
        .context(format!("Failed to read proof file {file_name} contents."))
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))
}
