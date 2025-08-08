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

use crate::channel::Message;
use anyhow::Context;
use futures::FutureExt;
use kailua_prover::args::ProveArgs;
use kailua_prover::channel::AsyncChannel;
use kailua_prover::proof::read_bincoded_file;
use kailua_prover::prove::prove;
use kailua_sync::await_tel_res;
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt as TeleFutureExt, TraceContextExt, Tracer};
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::mpsc::Sender;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone)]
pub struct Task {
    pub proposal_index: u64,
    pub prove_args: ProveArgs,
    pub proof_file_name: String,
}

#[allow(deprecated)]
pub async fn handle_proving_tasks(
    kailua_cli: Option<PathBuf>,
    task_channel: AsyncChannel<Task>,
    proof_sender: Sender<Message>,
    verbosity: u8,
) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("handle_proving_tasks"));

    loop {
        let Ok(Task {
            proposal_index,
            prove_args,
            proof_file_name,
        }) = task_channel.1.recv().await
        else {
            // The task queueing channel has been closed so no more work to do
            warn!("handle_proving_tasks terminated");
            break Ok(());
        };
        info!("Handling proof request for local index {proposal_index}.");

        let insufficient_l1_data = if let Some(kailua_cli) = &kailua_cli {
            info!("Invoking prover binary.");
            // Prove (note: dev-mode/bonsai env vars are inherited!)
            let mut kailua_cli_command = Command::new(kailua_cli);
            // get fake receipts when building under devnet
            if risc0_zkvm::is_dev_mode() {
                kailua_cli_command.env("RISC0_DEV_MODE", "1");
            }
            // pass arguments to point at target block
            kailua_cli_command.args(create_proving_args(&prove_args, verbosity));
            debug!("kailua_cli_command {:?}", &kailua_cli_command);
            // call the prover to generate a proof
            match await_tel_res!(
                context,
                tracer,
                "kailua_cli_command",
                kailua_cli_command
                    .kill_on_drop(true)
                    .spawn()
                    .context("Invoking prover")?
                    .wait()
            ) {
                Ok(proving_task) => {
                    if !proving_task.success() {
                        error!("Proving task failure. Exit code: {proving_task}");
                    } else {
                        info!("Proving task successful.");
                    }
                    proving_task.code().unwrap_or_default() == 111
                }
                Err(e) => {
                    error!("Failed to invoke prover: {e:?}");
                    false
                }
            }
        } else {
            info!("Proving internally.");
            // catch any proving errors
            let result_fut = async {
                match await_tel_res!(context, tracer, "prove", prove(prove_args.clone())) {
                    Ok(_) => false,
                    Err(err) => {
                        error!("Prover encountered error: {err:?}");
                        err.root_cause()
                            .to_string()
                            .contains("Expected zero claim hash")
                    }
                }
            };
            // catch panics
            AssertUnwindSafe(result_fut)
                .catch_unwind()
                .await
                .unwrap_or_else(|err| {
                    error!("Prover panicked! {err:?}");
                    false
                })
        };

        // we do not get a stitched proof w/o all proofs
        if !insufficient_l1_data && prove_args.proving.skip_stitching() {
            info!("Skipping proving task.");
            continue;
        }

        // wait for io then read computed proof from disk
        sleep(Duration::from_secs(1)).await;
        match read_bincoded_file(&proof_file_name).await {
            Ok(proof) => {
                // Send proof via the channel
                proof_sender
                    .send(Message::Proof(proposal_index, Some(proof)))
                    .await?;
                info!("Proof for local index {proposal_index} complete.");
            }
            Err(e) => {
                error!("Failed to read proof file: {e:?}");
                if insufficient_l1_data {
                    // Complain about unprovability
                    proof_sender
                        .send(Message::Proof(proposal_index, None))
                        .await?;
                    warn!("Cannot prove local index {proposal_index} due to insufficient l1 head.");
                } else {
                    // retry proving task
                    info!("Resubmitting proving task for local index {proposal_index}.");
                    task_channel
                        .0
                        .send(Task {
                            proposal_index,
                            prove_args,
                            proof_file_name,
                        })
                        .await
                        .context("task channel closed")?;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn create_proving_args(args: &ProveArgs, verbosity: u8) -> Vec<String> {
    // Prepare prover parameters
    let mut prove_args = args.to_arg_vec();

    // run the client natively
    prove_args.extend(vec![String::from("--native")]);

    // verbosity level
    if verbosity > 0 {
        prove_args.push(
            [
                String::from("-"),
                (0..verbosity).map(|_| 'v').collect::<String>(),
            ]
            .concat(),
        );
    }

    prove_args
}
