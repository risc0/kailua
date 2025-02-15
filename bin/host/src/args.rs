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

use alloy_primitives::{Address, B256};
use clap::{ArgAction, Parser};
use kailua_client::telemetry::TelemetryArgs;
use kailua_client::{
    args::{parse_address, parse_b256},
    boundless::BoundlessArgs,
};
use std::cmp::Ordering;

/// The host binary CLI application arguments.
#[derive(Parser, Clone, Debug)]
pub struct KailuaHostArgs {
    #[clap(flatten)]
    pub kona: kona_host::single::SingleChainHost,

    /// Address of OP-NODE endpoint to use
    #[clap(long, env)]
    pub op_node_address: Option<String>,
    /// Whether to skip running the zeth preflight engine
    #[clap(long, env, default_value_t = false)]
    pub skip_zeth_preflight: bool,
    #[clap(long, env, value_parser = parse_address)]
    pub payout_recipient_address: Option<Address>,
    #[clap(long, env, required = false, default_value_t = 21)]
    pub segment_limit: u32,
    #[clap(long, env, required = false, default_value_t = 52_428_800)]
    pub max_witness_size: usize,

    #[clap(long, env, value_delimiter = ',')]
    pub precondition_params: Vec<u64>,
    #[clap(long, env, value_parser = parse_b256, value_delimiter = ',')]
    pub precondition_block_hashes: Vec<B256>,
    #[clap(long, env, value_parser = parse_b256, value_delimiter = ',')]
    pub precondition_blob_hashes: Vec<B256>,

    #[clap(flatten)]
    pub boundless: BoundlessArgs,
    #[clap(flatten)]
    pub telemetry: TelemetryArgs,

    /// Verbosity level (0-2)
    #[arg(long, short, action = ArgAction::Count)]
    pub v: u8,
}

impl PartialEq<Self> for KailuaHostArgs {
    fn eq(&self, other: &Self) -> bool {
        self.kona
            .claimed_l2_block_number
            .eq(&other.kona.claimed_l2_block_number)
    }
}

impl Eq for KailuaHostArgs {}

impl PartialOrd<Self> for KailuaHostArgs {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for KailuaHostArgs {
    fn cmp(&self, other: &Self) -> Ordering {
        self.kona
            .claimed_l2_block_number
            .cmp(&other.kona.claimed_l2_block_number)
    }
}
