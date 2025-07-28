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

use crate::args::ValidateArgs;
use crate::channel::DuplexChannel;
use crate::{proposals, requests};
use anyhow::Context;
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use std::path::PathBuf;
use tokio::{spawn, try_join};
use tracing::warn;

pub async fn validate(
    mut args: ValidateArgs,
    verbosity: u8,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("validate"));

    // Sanitize proving arguments
    if args.proving.skip_await_proof {
        warn!("Validator ignores the skip-await-proof flag.");
        args.proving.skip_await_proof = false;
    }
    if args.proving.skip_derivation_proof {
        warn!("Validator ignores the skip-derivation-proof flag.");
        args.proving.skip_derivation_proof = false;
    }

    // We run two concurrent tasks, one for the chain, and one for the prover.
    // Both tasks communicate using the duplex channel
    let channel_pair = DuplexChannel::new_pair(4096);

    let handle_proposals = spawn(
        proposals::handle_proposals(channel_pair.0, args.clone(), data_dir.clone())
            .with_context(context.clone()),
    );
    let handle_proof_requests = spawn(
        requests::handle_proof_requests(channel_pair.1, args, verbosity, data_dir)
            .with_context(context.clone()),
    );

    let (proposals_task, proofs_task) = try_join!(handle_proposals, handle_proof_requests)?;
    proposals_task.context("handle_proposals")?;
    proofs_task.context("handle_proof_requests")?;

    Ok(())
}
