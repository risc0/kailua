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
use kailua_prover::channel::AsyncChannel;
use kailua_prover::proof::read_proof_file;
use kailua_sync::await_tel_res;
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use risc0_zkvm::is_dev_mode;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::mpsc::Sender;
use tokio::time::sleep;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone)]
pub struct Task {
    pub proposal_index: u64,
    pub proving_args: Vec<String>,
    pub proof_file_name: String,
}

pub async fn handle_proving_tasks(
    kailua_cli: PathBuf,
    task_channel: AsyncChannel<Task>,
    proof_sender: Sender<Message>,
) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("handle_proving_tasks"));

    loop {
        let Ok(Task {
            proposal_index,
            proving_args,
            proof_file_name,
        }) = task_channel.1.recv().await
        else {
            // The task queueing channel has been closed so no more work to do
            warn!("handle_proving_tasks terminated");
            break Ok(());
        };

        // Prove (note: dev-mode/bonsai env vars are inherited!)
        let mut kailua_cli_command = Command::new(&kailua_cli);
        // get fake receipts when building under devnet
        if is_dev_mode() {
            kailua_cli_command.env("RISC0_DEV_MODE", "1");
        }
        // pass arguments to point at target block
        kailua_cli_command.args(proving_args.clone());
        debug!("kailua_cli_command {:?}", &kailua_cli_command);
        // call the prover to generate a proof
        let insufficient_l1_data = match await_tel_res!(
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
        };
        // wait for io then read computed proof from disk
        sleep(Duration::from_secs(1)).await;
        match read_proof_file(&proof_file_name).await {
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
                    task_channel
                        .0
                        .send(Task {
                            proposal_index,
                            proving_args,
                            proof_file_name,
                        })
                        .await
                        .context("task channel closed")?;
                }
            }
        }
    }
}
