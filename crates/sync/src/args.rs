// Copyright 2025 RISC Zero, Inc.
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

use crate::provider::ProviderArgs;
use alloy::primitives::{Address, B256};
use std::path::PathBuf;
use std::str::FromStr;

#[derive(clap::Args, Debug, Clone)]
pub struct SyncArgs {
    #[clap(flatten)]
    pub provider: ProviderArgs,

    #[cfg(feature = "devnet")]
    #[clap(long, env, default_value_t = 0)]
    pub delay_l2_blocks: u64,

    /// Directory to use for caching data
    #[clap(long, env)]
    pub data_dir: Option<PathBuf>,
}

pub fn parse_address(s: &str) -> Result<Address, String> {
    Address::from_str(s).map_err(|_| format!("Invalid Address value: {}", s))
}

pub fn parse_b256(s: &str) -> Result<B256, String> {
    B256::from_str(s).map_err(|_| format!("Invalid B256 value: {}", s))
}
