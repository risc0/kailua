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

use alloy::network::primitives::BlockTransactionsKind;
use alloy::providers::{Provider, RootProvider};
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::B256;
use anyhow::{anyhow, bail, Context};
use clap::Parser;
use kailua_client::provider::OpNodeProvider;
use kailua_client::proving::ProvingError;
use kailua_common::witness::StitchedBootInfo;
use kailua_host::args::KailuaHostArgs;
use kailua_host::channel::AsyncChannel;
use kailua_host::config::generate_rollup_config;
use kailua_host::preflight::{
    concurrent_execution_preflight, fetch_precondition_data, zeth_execution_preflight,
};
use kailua_host::server::create_disk_kv_store;
use kailua_host::tasks::{handle_oneshot_tasks, Oneshot};
use std::collections::BinaryHeap;
use std::env::set_var;
use tempfile::tempdir;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = KailuaHostArgs::parse();
    kona_host::cli::init_tracing_subscriber(args.v)?;
    set_var("KAILUA_VERBOSITY", args.v.to_string());

    // fetch starting block number
    let l2_provider = if args.kona.is_offline() {
        None
    } else {
        Some(args.kona.create_providers().await?.l2)
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
    let rollup_config = generate_rollup_config(&mut args, &tmp_dir)
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
    if args.num_preflight_threads > 1 {
        // run parallelized preflight instances to populate kv store
        info!(
            "Running concurrent preflights with {} threads",
            args.num_preflight_threads
        );
        concurrent_execution_preflight(
            &args,
            rollup_config.clone(),
            op_node_provider.as_ref().expect("Missing op_node_provider"),
            disk_kv_store.clone(),
        )
        .await
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    } else if !args.skip_zeth_preflight {
        // run zeth preflight to fetch all the necessary preimages
        zeth_execution_preflight(&args, rollup_config.clone())
            .await
            .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    }

    // spin up proving workers
    let task_channel: AsyncChannel<Oneshot> = async_channel::unbounded();
    let mut proving_handlers = vec![];
    for _ in 0..args.num_proving_threads {
        proving_handlers.push(tokio::spawn(handle_oneshot_tasks(task_channel.1.clone())));
    }

    // compute individual proofs
    let mut job_pq = BinaryHeap::new();
    let mut proofs = Vec::new();
    job_pq.push(args.clone());
    let mut have_split = false;
    while let Some(job_args) = job_pq.pop() {
        let starting_block = if let Some(l2_provider) = l2_provider.as_ref() {
            l2_provider
                .get_block_by_hash(
                    job_args.kona.agreed_l2_head_hash,
                    BlockTransactionsKind::Hashes,
                )
                .await?
                .unwrap()
                .header
                .number
        } else {
            0
        };

        let num_blocks = job_args.kona.claimed_l2_block_number - starting_block;
        if starting_block > 0 {
            info!(
                "Processing job with {} blocks from block {}",
                num_blocks, starting_block
            );
        }
        // Force the proving attempt regardless of witness size if we prove just one block
        let force_attempt = num_blocks == 1 || job_args.kona.is_offline();

        match kailua_host::prove::compute_fpvm_proof(
            job_args.clone(),
            rollup_config.clone(),
            disk_kv_store.clone(),
            precondition_hash,
            precondition_validation_data_hash,
            vec![],
            vec![],
            !have_split,
            task_channel.0.clone(),
        )
        .await
        {
            Ok(proof) => {
                if let Some(proof) = proof {
                    proofs.push(proof);
                }
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
                        warn!("Proof witness size {f} above safety threshold {t}. Splitting workload.")
                    }
                    ProvingError::ExecutionError(e) => {
                        if force_attempt {
                            bail!("Irrecoverable ZKVM execution error: {e:?}")
                        }
                        warn!("Splitting proof after ZKVM execution error: {e:?}")
                    }
                    ProvingError::OtherError(e) => {
                        bail!("Irrecoverable proving error: {e:?}")
                    }
                    ProvingError::SeekProofError(..) => unreachable!("SeekProofError bubbled up"),
                    ProvingError::DerivationProofError(proofs) => {
                        info!("Computed {proofs} execution-only proofs.");
                        continue;
                    }
                }
                // Split workload at midpoint (num_blocks > 1)
                have_split = true;
                let mid_point = starting_block + num_blocks / 2;
                let mid_output = op_node_provider
                    .as_ref()
                    .expect("Missing op_node_provider")
                    .output_at_block(mid_point)
                    .await?;
                let mid_block = l2_provider
                    .as_ref()
                    .expect("Missing l2_provider")
                    .get_block_by_number(
                        BlockNumberOrTag::Number(mid_point),
                        BlockTransactionsKind::Hashes,
                    )
                    .await?
                    .unwrap_or_else(|| panic!("Block {mid_point} not found"));
                // Lower half workload ends at midpoint (inclusive)
                let mut lower_job_args = job_args.clone();
                lower_job_args.kona.claimed_l2_output_root = mid_output;
                lower_job_args.kona.claimed_l2_block_number = mid_point;
                job_pq.push(lower_job_args);
                // upper half workload starts after midpoint
                let mut upper_job_args = job_args;
                upper_job_args.kona.agreed_l2_output_root = mid_output;
                upper_job_args.kona.agreed_l2_head_hash = mid_block.header.hash;
                job_pq.push(upper_job_args);
            }
        }
    }
    // stitch contiguous proofs together
    if proofs.len() > 1 {
        info!("Composing {} proofs together.", proofs.len());
        // construct a proving instruction with no blocks to derive
        let mut base_args = args;
        {
            // set last block as starting point
            base_args.kona.agreed_l2_output_root = base_args.kona.claimed_l2_output_root;
            base_args.kona.agreed_l2_head_hash = l2_provider
                .as_ref()
                .unwrap()
                .get_block_by_number(
                    BlockNumberOrTag::Number(base_args.kona.claimed_l2_block_number),
                    BlockTransactionsKind::Hashes,
                )
                .await?
                .unwrap_or_else(|| {
                    panic!("Block {} not found", base_args.kona.claimed_l2_block_number)
                })
                .header
                .hash;
        }
        // construct a list of boot info to backward stitch
        let stitched_boot_info = proofs
            .iter()
            .map(StitchedBootInfo::from)
            .collect::<Vec<_>>();
        kailua_host::prove::compute_fpvm_proof(
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

    info!("Exiting host program.");
    Ok(())
}
