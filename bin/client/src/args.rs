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

use alloy_primitives::B256;
use clap::Parser;
use kailua_prover::backends::boundless::BoundlessArgs;
use kailua_prover::ProvingArgs;
use kailua_sync::args::parse_b256;

/// The client binary CLI application arguments.
#[derive(Parser, Clone, Debug)]
pub struct KailuaClientArgs {
    #[arg(long, env, action = clap::ArgAction::Count)]
    pub kailua_verbosity: u8,

    #[clap(long, env, value_parser = parse_b256)]
    pub precondition_validation_data_hash: Option<B256>,

    #[clap(flatten)]
    pub proving: ProvingArgs,

    #[clap(flatten)]
    pub boundless: BoundlessArgs,
}
