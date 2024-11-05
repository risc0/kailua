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

use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::Block;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::fs::OpenOptions;
use std::process::Command;
use tracing::{info, warn};

#[derive(clap::Args, Debug, Clone)]
pub struct BenchArgs {
    #[arg(long, short, help = "Verbosity level (0-4)", action = clap::ArgAction::Count)]
    pub v: u8,

    /// Address of OP-NODE endpoint to use
    #[clap(long)]
    pub op_node_address: String,
    /// Address of L2 JSON-RPC endpoint to use (eth and debug namespace required).
    #[clap(long)]
    pub l2_node_address: String,
    /// Address of L1 JSON-RPC endpoint to use (eth namespace required)
    #[clap(long)]
    pub l1_node_address: String,
    /// Address of the L1 Beacon API endpoint to use.
    #[clap(long)]
    pub l1_beacon_address: String,
    #[clap(long)]
    pub data_dir: String,

    /// The starting L2 block number to scan for blocks from
    #[clap(long)]
    pub bench_start: u64,
    /// The number of L2 blocks to scan as benchmark candidates
    #[clap(long)]
    pub bench_range: u64,
    /// The number of top candidate L2 blocks to benchmark
    #[clap(long)]
    pub bench_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateBlock {
    pub txn_count: usize,
    pub block: Block,
}

impl PartialOrd for CandidateBlock {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.txn_count.partial_cmp(&other.txn_count)
    }
}

impl Ord for CandidateBlock {
    fn cmp(&self, other: &Self) -> Ordering {
        self.txn_count.cmp(&other.txn_count)
    }
}

pub async fn benchmark(args: BenchArgs) -> anyhow::Result<()> {
    let l2_node_provider =
        ProviderBuilder::new().on_http(args.l2_node_address.as_str().try_into()?);
    // Scan L2 blocks for highest transaction counts
    let bench_end = args.bench_start + args.bench_range;
    let mut block_heap = BinaryHeap::new();
    info!("Scanning candidates.");
    for block_number in args.bench_start..bench_end {
        let Some(block) = l2_node_provider
            .get_block_by_number(block_number.into(), false)
            .await?
        else {
            warn!("Failed to fetch block #{block_number}");
            break;
        };
        block_heap.push(CandidateBlock {
            txn_count: block.transactions.len(),
            block,
        })
    }
    // Benchmark top candidates
    for _ in 0..args.bench_count {
        let Some(block) = block_heap.pop() else {
            warn!("Ran out of candidates too early.");
            break;
        };
        let block_number = block.block.header.number.to_string();
        let txn_count = block.txn_count;
        info!("Processing candidate block {block_number} with {txn_count} transactions.");
        // Derive output file name
        let version = risc0_zkvm::get_version().unwrap();
        let output_file_name = format!("bench-risc0-{version}-{block_number}-{txn_count}.out");
        let output_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&output_file_name)?;
        // Pipe outputs to file
        Command::new("just")
            .env("RISC0_DEV_MODE", "1")
            .args(vec![
                "prove",
                &block_number,
                &args.l1_node_address,
                &args.l1_beacon_address,
                &args.l2_node_address,
                &args.op_node_address,
                &args.data_dir,
                "-v",
            ])
            .stdout(output_file)
            .status()?;
        info!("Output written to {output_file_name}");
    }
    Ok(())
}
