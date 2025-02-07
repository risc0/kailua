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

use crate::CoreArgs;
use alloy::primitives::map::{Entry, HashMap};
use alloy::providers::{Provider, ProviderBuilder};
use kailua_client::telemetry::TelemetryArgs;
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, Span, Status, TraceContextExt, Tracer};
use risc0_zkvm::is_dev_mode;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fs::OpenOptions;
use std::process::Command;
use tracing::{info, warn};

#[derive(clap::Args, Debug, Clone)]
pub struct BenchArgs {
    #[clap(flatten)]
    pub core: CoreArgs,

    /// The starting L2 block number to scan for blocks from
    #[clap(long, env)]
    pub bench_start: u64,
    /// The length of the sequence of blocks to benchmark
    #[clap(long, env)]
    pub bench_length: u64,
    /// The number of L2 blocks to scan as benchmark candidates
    #[clap(long, env)]
    pub bench_range: u64,
    /// The number of top candidate L2 blocks to benchmark
    #[clap(long, env)]
    pub bench_count: u64,

    #[clap(flatten)]
    pub telemetry: TelemetryArgs,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateBlock {
    pub txn_count: u64,
    pub block_number: u64,
}

impl PartialOrd for CandidateBlock {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CandidateBlock {
    fn cmp(&self, other: &Self) -> Ordering {
        self.txn_count.cmp(&other.txn_count)
    }
}

pub async fn benchmark(args: BenchArgs) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("benchmark"));

    let l2_node_provider =
        ProviderBuilder::new().on_http(args.core.op_geth_url.as_str().try_into()?);
    let mut cache: HashMap<u64, u64> = HashMap::new();
    // Scan L2 blocks for highest transaction counts
    let bench_end = args.bench_start + args.bench_range;
    let mut block_heap = BinaryHeap::new();
    info!("Scanning candidates.");
    for block_number in args.bench_start..bench_end {
        let mut txn_count = 0;
        for i in 0..args.bench_length {
            let block_number = block_number + i;
            txn_count += match cache.entry(block_number) {
                Entry::Occupied(e) => *e.get(),
                Entry::Vacant(e) => {
                    let x = l2_node_provider
                        .get_block_transaction_count_by_number(block_number.into())
                        .with_context(context.with_span(tracer.start_with_context(
                            "ReqwestProvider::get_block_transaction_count_by_number",
                            &context,
                        )))
                        .await?
                        .unwrap_or_else(|| {
                            panic!("Failed to fetch transaction count for block {block_number}")
                        });
                    *e.insert(x)
                }
            }
        }
        block_heap.push(CandidateBlock {
            txn_count,
            block_number,
        })
    }
    // Benchmark top candidates
    for _ in 0..args.bench_count {
        let Some(CandidateBlock {
            txn_count,
            block_number,
        }) = block_heap.pop()
        else {
            warn!("Ran out of candidates too early.");
            break;
        };
        let end = block_number + args.bench_length;
        info!("Processing blocks {block_number}-{end} with {txn_count} transactions.");
        // Derive output file name
        let version = risc0_zkvm::get_version()?;
        let output_file_name =
            format!("bench-risc0-{version}-{block_number}-{end}-{txn_count}.out");
        let output_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&output_file_name)?;
        // Pipe outputs to file
        let verbosity_level = if args.core.v > 0 {
            format!("-{}", "v".repeat(args.core.v as usize))
        } else {
            String::new()
        };
        let mut cmd = Command::new("just");
        if is_dev_mode() {
            cmd.env("RISC0_DEV_MODE", "1");
        }
        let block_number = block_number.to_string();
        let block_count = args.bench_length.to_string();
        let data_dir = args.core.data_dir.clone().unwrap();
        cmd.args(vec![
            "prove",
            &block_number,
            &block_count,
            &args.core.eth_rpc_url,
            &args.core.beacon_rpc_url,
            &args.core.op_geth_url,
            &args.core.op_node_url,
            data_dir.to_str().unwrap(),
            "debug",
            &verbosity_level,
        ]);
        println!("Executing: {cmd:?}");

        let mut sub_span = tracer.start_with_context("prove", &context);
        let res = cmd.stdout(output_file).status();
        if let Err(err) = &res {
            sub_span.record_error(err);
            Span::set_status(
                &mut sub_span,
                Status::error(format!("Fatal error: {err:?}")),
            );
        } else {
            Span::set_status(&mut sub_span, Status::Ok);
        }
        res?;

        info!("Output written to {output_file_name}");
    }
    Ok(())
}
