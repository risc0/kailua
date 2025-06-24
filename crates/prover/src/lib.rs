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

use alloy_primitives::Address;
use clap::Parser;
use kailua_common::executor::Execution;
use kailua_sync::args::parse_address;

pub mod backends;
pub mod client;
pub mod proof;

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

#[derive(Debug, thiserror::Error)]
pub enum ProvingError {
    #[error("DerivationProofError error: execution proofs {0}")]
    DerivationProofError(usize),

    #[error("NotSeekingProof error: witness {0}")]
    NotSeekingProof(usize, Vec<Vec<Execution>>),

    #[error("WitnessSizeError error: size {0} limit {0}")]
    WitnessSizeError(usize, usize, Vec<Vec<Execution>>),

    #[error("ExecutionError error: ZKVM failed {0:?}")]
    ExecutionError(anyhow::Error),

    #[error("OtherError error: {0:?}")]
    OtherError(anyhow::Error),
}
