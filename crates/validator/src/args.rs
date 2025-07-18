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

use kailua_prover::args::ProvingArgs;
use kailua_prover::backends::boundless::BoundlessArgs;
use kailua_sync::args::SyncArgs;
use kailua_sync::transact::signer::ValidatorSignerArgs;
use kailua_sync::transact::TransactArgs;
use std::path::PathBuf;

/// Start the agent for resolving on-chain Kailua disputes
#[derive(clap::Args, Debug, Clone)]
pub struct ValidateArgs {
    #[clap(flatten)]
    pub sync: SyncArgs,

    /// Path to the prover binary to use for proving
    #[clap(long, env)]
    pub kailua_cli: Option<PathBuf>,
    /// Fast-forward block height
    #[clap(long, env, required = false, default_value_t = 0)]
    pub fast_forward_target: u64,
    /// How many proofs to compute simultaneously
    #[clap(long, env, default_value_t = 1)]
    pub num_concurrent_provers: u64,
    /// Optionally enables the use of `debug_executePayload` to collect the execution witness from
    /// the execution layer.
    #[arg(long, env, default_value_t = false)]
    pub enable_experimental_witness_endpoint: bool,

    /// The number of l1 heads to jump back when initially proving
    #[cfg(feature = "devnet")]
    #[clap(long, env, default_value_t = 0)]
    pub l1_head_jump_back: u64,

    /// Secret key of L1 wallet to use for challenging and proving outputs
    #[clap(flatten)]
    pub validator_signer: ValidatorSignerArgs,
    /// Transaction publication configuration
    #[clap(flatten)]
    pub txn_args: TransactArgs,

    #[clap(flatten)]
    pub proving: ProvingArgs,
    #[clap(flatten)]
    pub boundless: BoundlessArgs,
}
