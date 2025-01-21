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
use clap::Parser;
use kailua_client::{
    args::{parse_address, parse_b256},
    boundless::BoundlessArgs,
};

/// The host binary CLI application arguments.
#[derive(Parser, Clone, Debug)]
pub struct KailuaHostArgs {
    #[clap(flatten)]
    pub kona: kona_host::HostCli,

    /// Address of OP-NODE endpoint to use
    #[clap(long, env)]
    pub op_node_address: Option<String>,
    /// Whether to skip running the zeth preflight engine
    #[clap(long, default_value_t = false, env)]
    pub skip_zeth_preflight: bool,
    #[clap(long, value_parser = parse_address, env)]
    pub payout_recipient_address: Option<Address>,

    #[clap(long, value_delimiter = ',', env)]
    pub precondition_params: Vec<u64>,
    #[clap(long, value_parser = parse_b256, value_delimiter = ',', env)]
    pub precondition_block_hashes: Vec<B256>,
    #[clap(long, value_parser = parse_b256, value_delimiter = ',', env)]
    pub precondition_blob_hashes: Vec<B256>,

    #[clap(flatten)]
    pub boundless: BoundlessArgs,
}
