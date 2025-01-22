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
use alloy::providers::{Provider, ProviderBuilder};
use anyhow::{bail, Context};
use clap::Parser;
use kailua_client::provider::OpNodeProvider;
use kailua_client::proving::ProvingError;
use kailua_common::witness::StitchedBootInfo;
use kailua_host::args::KailuaHostArgs;
use kona_host::cli::HostMode;
use kona_host::init_tracing_subscriber;
use std::collections::BinaryHeap;
use std::env::set_var;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = KailuaHostArgs::parse();
    init_tracing_subscriber(args.kona.v)?;
    set_var("KAILUA_VERBOSITY", args.kona.v.to_string());

    // fetch starting block number
    let HostMode::Single(kona_cfg) = &args.kona.mode;
    let (.., l2_provider) = kona_cfg.create_providers().await?;
    let op_node_provider = OpNodeProvider(
        ProviderBuilder::new().on_http(args.op_node_address.clone().unwrap().as_str().try_into()?),
    );
    // compute individual proofs
    let mut job_pq = BinaryHeap::new();
    let mut proofs = Vec::new();
    job_pq.push(args.clone());
    let mut have_split = false;
    while let Some(job_args) = job_pq.pop() {
        let HostMode::Single(kona_cfg) = &job_args.kona.mode;
        let starting_block = l2_provider
            .get_block_by_hash(kona_cfg.agreed_l2_head_hash, BlockTransactionsKind::Hashes)
            .await?
            .unwrap()
            .header
            .number;
        let num_blocks = kona_cfg.claimed_l2_block_number - starting_block;
        // Force the proving attempt regardless of witness size if we prove just one block
        let force_attempt = num_blocks == 1;

        match kailua_host::prove::compute_fpvm_proof(
            job_args.clone(),
            vec![],
            vec![],
            !have_split,
            force_attempt,
        )
        .await
        {
            Ok(proof) => {
                proofs.push(proof);
            }
            Err(e) => {
                match e {
                    ProvingError::WitnessSizeError(f, e) => {
                        if force_attempt {
                            unreachable!(
                                "Received WitnessSizeError({f},{e}) for a forced proving attempt."
                            );
                        }
                        warn!("Proof witness size {f} above safety threshold {e}. Splitting workload.")
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
                }
                // Split workload at midpoint (num_blocks > 1)
                have_split = true;
                let mid_point = starting_block + num_blocks / 2;
                let mid_output = op_node_provider.output_at_block(mid_point).await?;
                // Lower half workload ends at midpoint
                let mut lower_job_args = job_args.clone();
                let HostMode::Single(lower_kona_cfg) = &mut lower_job_args.kona.mode;
                lower_kona_cfg.claimed_l2_output_root = mid_output;
                lower_kona_cfg.claimed_l2_block_number = mid_point;
                job_pq.push(lower_job_args);
                // upper half workload starts at midpoint
                let mut upper_job_args = job_args;
                let HostMode::Single(upper_kona_cfg) = &mut upper_job_args.kona.mode;
                upper_kona_cfg.agreed_l2_output_root = mid_output;
                job_pq.push(upper_job_args);
            }
        }
    }
    // stitch contiguous proofs together
    if proofs.len() > 1 {
        // construct a proving instruction with no blocks to derive
        let mut base_args = args.clone();
        {
            // set last block as starting point
            let HostMode::Single(base_kona_args) = &mut base_args.kona.mode;
            base_kona_args.agreed_l2_output_root = base_kona_args.claimed_l2_output_root;
        }
        // construct a reversed list of boot info to backward stitch
        let stitched_boot_info = proofs
            .iter()
            .rev()
            .map(StitchedBootInfo::from)
            .collect::<Vec<_>>();
        kailua_host::prove::compute_fpvm_proof(base_args, stitched_boot_info, proofs, true, true)
            .await
            .context("Failed to compute FPVM proof.")?;
    }

    info!("Exiting host program.");
    Ok(())
}
