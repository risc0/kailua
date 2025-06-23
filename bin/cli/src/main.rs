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
use kailua_cli::KailuaCli;
use kailua_sync::await_tel;
use kailua_sync::telemetry::init_tracer_provider;
use opentelemetry::global::{shutdown_tracer_provider, tracer};
use opentelemetry::trace::{FutureExt, Status, TraceContextExt, Tracer};
use tempfile::tempdir;
use tracing::error;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = KailuaCli::parse();
    kona_cli::init_tracing_subscriber(cli.verbosity(), None::<EnvFilter>)?;
    init_tracer_provider(cli.telemetry_args())?;
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("cli"));

    let tmp_dir = tempdir()?;
    let data_dir = cli.data_dir().unwrap_or(tmp_dir.path().to_path_buf());

    let command_res = match cli {
        KailuaCli::Config(args) => {
            await_tel!(context, kailua_cli::config::config(args))
        }
        KailuaCli::FastTrack(args) => {
            await_tel!(context, kailua_cli::fast_track::fast_track(args))
        }
        KailuaCli::Propose(args) => {
            await_tel!(context, kailua_cli::propose::propose(args, data_dir))
        }
        KailuaCli::Validate(args) => {
            await_tel!(context, kailua_cli::validate::validate(args, data_dir))
        }
        KailuaCli::TestFault(_args) => {
            #[cfg(not(feature = "devnet"))]
            unimplemented!("Intentional faults are only available on devnet environments");
            #[cfg(feature = "devnet")]
            await_tel!(context, kailua_cli::fault::fault(_args))
        }
        KailuaCli::Benchmark(bench_args) => {
            await_tel!(context, kailua_cli::bench::benchmark(bench_args))
        }
    };

    let span = context.span();
    if let Err(err) = command_res {
        error!("Fatal error: {err:?}");
        span.record_error(err.as_ref());
        span.set_status(Status::error(format!("Fatal error: {err:?}")));
    } else {
        span.set_status(Status::Ok);
    }

    shutdown_tracer_provider();

    Ok(())
}
