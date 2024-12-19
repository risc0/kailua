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
use tempfile::tempdir;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing_subscriber(cli.verbosity())?;

    let tmp_dir = tempdir()?;
    let data_dir = cli.data_dir().unwrap_or(tmp_dir.path().to_path_buf());

    match cli {
        Cli::Config(args) => kailua_cli::config::config(args).await?,
        Cli::FastTrack(args) => kailua_cli::fast_track::fast_track(args).await?,
        Cli::Propose(args) => kailua_cli::propose::propose(args, data_dir).await?,
        Cli::Validate(args) => kailua_cli::validate::validate(args, data_dir).await?,
        Cli::TestFault(_args) =>
        {
            #[cfg(feature = "devnet")]
            kailua_cli::fault::fault(_args).await?
        } // Cli::Benchmark(bench_args) => kailua_cli::bench::benchmark(bench_args).await?,
    }
    Ok(())
}
