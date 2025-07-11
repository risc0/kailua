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

use kailua_sync::telemetry::TelemetryArgs;
use kailua_validator::args;
use std::path::PathBuf;

pub mod bench;
pub mod config;
pub mod demo;
pub mod fast_track;
pub mod fault;

#[derive(clap::Parser, Debug, Clone)]
#[command(name = "kailua-cli")]
#[command(bin_name = "kailua-cli")]
#[command(author, version, about, long_about = None)]
#[allow(clippy::large_enum_variant)]
pub enum KailuaCli {
    Config {
        #[clap(flatten)]
        args: config::ConfigArgs,
        #[clap(flatten)]
        cli: CliArgs,
    },
    FastTrack {
        #[clap(flatten)]
        args: fast_track::FastTrackArgs,
        #[clap(flatten)]
        cli: CliArgs,
    },
    Propose {
        #[clap(flatten)]
        args: kailua_proposer::args::ProposeArgs,
        #[clap(flatten)]
        cli: CliArgs,
    },
    Validate {
        #[clap(flatten)]
        args: args::ValidateArgs,
        #[clap(flatten)]
        cli: CliArgs,
    },
    Prove {
        #[clap(flatten)]
        args: kailua_prover::args::ProveArgs,
        #[clap(flatten)]
        cli: CliArgs,
    },
    TestFault {
        #[clap(flatten)]
        args: fault::FaultArgs,
        #[clap(flatten)]
        cli: CliArgs,
    },
    Benchmark {
        #[clap(flatten)]
        args: bench::BenchArgs,
        #[clap(flatten)]
        cli: CliArgs,
    },
    Demo {
        #[clap(flatten)]
        args: demo::DemoArgs,
        #[clap(flatten)]
        cli: CliArgs,
    },
}

#[derive(clap::Args, Debug, Clone)]
pub struct CliArgs {
    #[arg(long, short, help = "Verbosity level (0-4)", action = clap::ArgAction::Count)]
    pub v: u8,
}

impl KailuaCli {
    pub fn verbosity(&self) -> u8 {
        match self {
            KailuaCli::Config { cli, .. } => cli.v,
            KailuaCli::FastTrack { cli, .. } => cli.v,
            KailuaCli::Propose { cli, .. } => cli.v,
            KailuaCli::Validate { cli, .. } => cli.v,
            KailuaCli::Prove { cli, .. } => cli.v,
            KailuaCli::TestFault { cli, .. } => cli.v,
            KailuaCli::Benchmark { cli, .. } => cli.v,
            KailuaCli::Demo { cli, .. } => cli.v,
        }
    }

    pub fn data_dir(&self) -> Option<PathBuf> {
        match self {
            KailuaCli::Propose { args, .. } => args.sync.data_dir.clone(),
            KailuaCli::Validate { args, .. } => args.sync.data_dir.clone(),
            KailuaCli::Prove { args, .. } => args.kona.data_dir.clone(),
            KailuaCli::Demo { args, .. } => args.data_dir.clone(),
            _ => None,
        }
    }

    pub fn telemetry_args(&self) -> &TelemetryArgs {
        match self {
            KailuaCli::Config { args, .. } => &args.telemetry,
            KailuaCli::FastTrack { args, .. } => &args.telemetry,
            KailuaCli::Propose { args, .. } => &args.sync.telemetry,
            KailuaCli::Validate { args, .. } => &args.sync.telemetry,
            KailuaCli::Prove { args, .. } => &args.telemetry,
            KailuaCli::TestFault { args, .. } => &args.propose_args.sync.telemetry,
            KailuaCli::Benchmark { args, .. } => &args.sync.telemetry,
            KailuaCli::Demo { args, .. } => &args.telemetry,
        }
    }
}
