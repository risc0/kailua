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

use crate::risczero::boundless::BoundlessArgs;
use alloy_primitives::{Address, B256};
use clap::Parser;
use kailua_sync::args::{parse_address, parse_b256};
use kailua_sync::telemetry::TelemetryArgs;
use std::cmp::Ordering;

#[derive(Parser, Clone, Debug)]
pub struct ProvingArgs {
    /// Address of the recipient account to use for bond payouts
    #[clap(long, env, value_parser = parse_address)]
    pub payout_recipient_address: Option<Address>,
    /// ZKVM Proving Segment Limit
    #[clap(long, env, required = false, default_value_t = 21)]
    pub segment_limit: u32,
    /// Maximum input data size per proof
    #[clap(long, env, required = false, default_value_t = 2_684_354_560)]
    pub max_witness_size: usize,
    /// How many threads to use for fetching preflight data
    #[clap(long, env, default_value_t = 4)]
    pub num_concurrent_preflights: u64,
    /// How many threads to use for computing proofs
    #[clap(long, env, default_value_t = 1)]
    pub num_concurrent_proofs: u64,
    /// Whether to bypass loading rollup chain configurations from the kona registry
    #[clap(long, env, default_value_t = false)]
    pub bypass_chain_registry: bool,
    /// Whether to only prove L2 block execution without referring to the L1
    #[clap(long, env, default_value_t = false)]
    pub skip_derivation_proof: bool,
    /// Whether to skip waiting for the proof generation process to complete
    #[clap(long, env, default_value_t = false)]
    pub skip_await_proof: bool,
    /// URL of the EigenDA RPC endpoint.
    #[clap(
        long,
        visible_alias = "eigenda",
        requires = "l2_node_address",
        requires = "l1_node_address",
        requires = "l1_beacon_address",
        env
    )]
    pub eigenda_proxy_address: Option<String>,
}

impl ProvingArgs {
    pub fn skip_stitching(&self) -> bool {
        self.skip_derivation_proof || self.skip_await_proof
    }
}

/// Run the prover to generate an execution/fault/validity proof
#[derive(Parser, Clone, Debug)]
pub struct ProveArgs {
    #[clap(flatten)]
    pub kona: kona_host::single::SingleChainHost,

    /// Address of OP-NODE endpoint to use
    #[clap(long, env)]
    pub op_node_address: Option<String>,

    #[clap(flatten)]
    pub proving: ProvingArgs,
    #[clap(flatten)]
    pub boundless: BoundlessArgs,

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
