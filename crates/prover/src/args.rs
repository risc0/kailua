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

use crate::backends::boundless::BoundlessArgs;
use alloy_primitives::{Address, B256};
use clap::Parser;
use kailua_sync::args::{parse_address, parse_b256};
use kailua_sync::telemetry::TelemetryArgs;
use std::cmp::Ordering;

#[derive(Parser, Clone, Debug)]
pub struct ProvingArgs {
    #[clap(long, env, value_parser = parse_address)]
    pub payout_recipient_address: Option<Address>,
    #[clap(long, env, required = false, default_value_t = 21)]
    pub segment_limit: u32,
    #[clap(long, env, required = false, default_value_t = 2_684_354_560)]
    pub max_witness_size: usize,
    #[clap(long, env, default_value_t = false)]
    pub skip_derivation_proof: bool,
    #[clap(long, env, default_value_t = false)]
    pub skip_await_proof: bool,
}

/// The prover arguments
#[derive(Parser, Clone, Debug)]
pub struct ProveArgs {
    #[clap(flatten)]
    pub kona: kona_host::single::SingleChainHost,

    /// Address of OP-NODE endpoint to use
    #[clap(long, env)]
    pub op_node_address: Option<String>,
    /// How many threads to use for fetching preflight data
    #[clap(long, env, default_value_t = 4)]
    pub num_concurrent_preflights: u64,
    /// How many threads to use for computing proofs
    #[clap(long, env, default_value_t = 1)]
    pub num_concurrent_proofs: u64,

    #[clap(flatten)]
    pub proving: ProvingArgs,
    #[clap(flatten)]
    pub boundless: BoundlessArgs,
    #[clap(long, env, default_value_t = false)]
    pub bypass_chain_registry: bool,

    #[clap(long, env, value_delimiter = ',')]
    pub precondition_params: Vec<u64>,
    #[clap(long, env, value_parser = parse_b256, value_delimiter = ',')]
    pub precondition_block_hashes: Vec<B256>,
    #[clap(long, env, value_parser = parse_b256, value_delimiter = ',')]
    pub precondition_blob_hashes: Vec<B256>,

    #[clap(flatten)]
    pub telemetry: TelemetryArgs,
}

impl PartialEq<Self> for ProveArgs {
    fn eq(&self, other: &Self) -> bool {
        self.kona
            .claimed_l2_block_number
            .eq(&other.kona.claimed_l2_block_number)
    }
}

impl Eq for ProveArgs {}

impl PartialOrd<Self> for ProveArgs {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ProveArgs {
    fn cmp(&self, other: &Self) -> Ordering {
        self.kona
            .claimed_l2_block_number
            .cmp(&other.kona.claimed_l2_block_number)
    }
}
