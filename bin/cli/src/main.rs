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

use clap::Parser;
use kailua_cli::Cli;
use kona_host::init_tracing_subscriber;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing_subscriber(cli.verbosity())?;

    match cli {
        Cli::Deploy(deploy_args) => kailua_cli::deploy::deploy(deploy_args).await?,
        Cli::Propose(propose_args) => kailua_cli::propose::propose(propose_args).await?,
        Cli::Validate(validate_args) => kailua_cli::validate::validate(validate_args).await?,
        Cli::TestFault(_fault_args) =>
        {
            #[cfg(feature = "devnet")]
            kailua_cli::fault::fault(_fault_args).await?
        }
        Cli::Benchmark(bench_args) => kailua_cli::bench::benchmark(bench_args).await?,
    }
    Ok(())
}
