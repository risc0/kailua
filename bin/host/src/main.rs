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

use anyhow::Context;
use clap::Parser;
use kailua_host::args::KailuaHostArgs;
use kona_host::init_tracing_subscriber;
use std::env::set_var;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = KailuaHostArgs::parse();
    init_tracing_subscriber(args.kona.v)?;
    set_var("KAILUA_VERBOSITY", args.kona.v.to_string());

    // compute receipt if uncached
    kailua_host::prove::compute_fpvm_proof(args, vec![])
        .await
        .context("Failed to compute FPVM proof.")?;

    info!("Exiting host program.");
    Ok(())
}
