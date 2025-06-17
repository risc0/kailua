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

pub mod proposals;
pub mod proving;

use crate::channel::DuplexChannel;
use crate::transact::signer::ValidatorSignerArgs;
use crate::transact::TransactArgs;
use crate::validate::proving::{create_proving_args, Task};
use crate::CoreArgs;
use alloy::primitives::{Address, FixedBytes, B256};
use anyhow::{anyhow, bail, Context};
use kailua_build::KAILUA_FPVM_ID;
use kailua_client::args::parse_address;
use kailua_client::boundless::BoundlessArgs;
use kailua_client::proof::{proof_file_name, read_proof_file};
use kailua_client::telemetry::TelemetryArgs;
use kailua_client::{await_tel, await_tel_res};
use kailua_common::config::config_hash;
use kailua_common::journal::ProofJournal;
use kailua_common::precondition::PreconditionValidationData;
use kailua_host::channel::AsyncChannel;
use kailua_host::config::fetch_rollup_config;
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use risc0_zkvm::{is_dev_mode, Receipt};
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::mpsc::Sender;
use tokio::time::sleep;
use tokio::{spawn, try_join};
use tracing::{debug, error, info, warn};

#[derive(clap::Args, Debug, Clone)]
pub struct ValidateArgs {
    #[clap(flatten)]
    pub core: CoreArgs,

    /// Path to the kailua host binary to use for proving
    #[clap(long, env)]
    pub kailua_host: PathBuf,
    /// Fast-forward block height
    #[clap(long, env, required = false, default_value_t = 0)]
    pub fast_forward_target: u64,
    /// How many proofs to compute simultaneously
    #[clap(long, env, default_value_t = 1)]
    pub num_concurrent_hosts: u64,

    /// Secret key of L1 wallet to use for challenging and proving outputs
    #[clap(flatten)]
    pub validator_signer: ValidatorSignerArgs,
    /// Transaction publication configuration
    #[clap(flatten)]
    pub txn_args: TransactArgs,
    /// Address of the recipient account to use for bond payouts
    #[clap(long, env, value_parser = parse_address)]
    pub payout_recipient_address: Option<Address>,
    /// Address of the KailuaGame implementation to use
    #[clap(long, env, value_parser = parse_address)]
    pub kailua_game_implementation: Option<Address>,
    /// Address of the anchor proposal to start synchronization from
    #[clap(long, env, value_parser = parse_address)]
    pub kailua_anchor_address: Option<Address>,

    #[clap(flatten)]
    pub boundless: BoundlessArgs,

    #[clap(flatten)]
    pub telemetry: TelemetryArgs,
}

pub async fn validate(args: ValidateArgs, data_dir: PathBuf) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("validate"));

    // We run two concurrent tasks, one for the chain, and one for the prover.
    // Both tasks communicate using the duplex channel
    let channel_pair = DuplexChannel::new_pair(4096);

    let handle_proposals = spawn(
        proposals::handle_proposals(channel_pair.0, args.clone(), data_dir.clone())
            .with_context(context.clone()),
    );
    let handle_proof_requests =
        spawn(handle_proof_requests(channel_pair.1, args, data_dir).with_context(context.clone()));

    let (proposals_task, proofs_task) = try_join!(handle_proposals, handle_proof_requests)?;
    proposals_task.context("handle_proposals")?;
    proofs_task.context("handle_proofs")?;

    Ok(())
}

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Message {
    // The proposal and its parent
    Proposal {
        index: u64,
        precondition_validation_data: Option<PreconditionValidationData>,
        l1_head: FixedBytes<32>,
        agreed_l2_head_hash: FixedBytes<32>,
        agreed_l2_output_root: FixedBytes<32>,
        claimed_l2_block_number: u64,
        claimed_l2_output_root: FixedBytes<32>,
    },
    Proof(u64, Option<Receipt>),
}

pub async fn handle_proof_requests(
    mut channel: DuplexChannel<Message>,
    args: ValidateArgs,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    // Telemetry
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("handle_proof_requests"));

    // Fetch rollup configuration
    let rollup_config = await_tel!(
        context,
        fetch_rollup_config(&args.core.op_node_url, &args.core.op_geth_url, None)
    )
    .context("fetch_rollup_config")?;
    let l2_chain_id = rollup_config.l2_chain_id.to_string();
    let config_hash = B256::from(config_hash(&rollup_config)?);
    let fpvm_image_id = B256::from(bytemuck::cast::<[u32; 8], [u8; 32]>(KAILUA_FPVM_ID));
    // Set payout recipient
    let validator_wallet = await_tel_res!(
        context,
        tracer,
        "ValidatorSigner::wallet",
        args.validator_signer
            .wallet(Some(rollup_config.l1_chain_id))
    )?;
    let payout_recipient = args
        .payout_recipient_address
        .unwrap_or_else(|| validator_wallet.default_signer().address());
    info!("Proof payout recipient: {payout_recipient}");

    let task_channel: AsyncChannel<Task> = async_channel::unbounded();
    let mut proving_handlers = vec![];
    // instantiate worker pool
    for _ in 0..args.num_concurrent_hosts {
        proving_handlers.push(spawn(handle_proving_tasks(
            args.kailua_host.clone(),
            task_channel.clone(),
            channel.sender.clone(),
        )));
    }

    // Run proof generator loop
    loop {
        // Dequeue messages
        let Message::Proposal {
            index: proposal_index,
            precondition_validation_data,
            l1_head,
            agreed_l2_head_hash,
            agreed_l2_output_root,
            claimed_l2_block_number,
            claimed_l2_output_root,
        } = channel
            .receiver
            .recv()
            .await
            .ok_or(anyhow!("proof receiver channel closed"))?
        else {
            bail!("Unexpected message type.");
        };
        info!("Processing proof for local index {proposal_index}.");
        // Compute proof file name
        let precondition_hash = precondition_validation_data
            .as_ref()
            .map(|d| d.precondition_hash())
            .unwrap_or_default();
        let proof_journal = ProofJournal {
            payout_recipient,
            precondition_hash,
            l1_head,
            agreed_l2_output_root,
            claimed_l2_output_root,
            claimed_l2_block_number,
            config_hash,
            fpvm_image_id,
        };
        let proof_file_name = proof_file_name(&proof_journal);
        // Prepare kailua-host proving args
        let proving_args = create_proving_args(
            &args,
            data_dir.clone(),
            l2_chain_id.clone(),
            payout_recipient,
            precondition_validation_data,
            l1_head,
            agreed_l2_head_hash,
            agreed_l2_output_root,
            claimed_l2_block_number,
            claimed_l2_output_root,
        );
        // Send to task pool
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

pub async fn handle_proving_tasks(
    kailua_host: PathBuf,
    task_channel: AsyncChannel<Task>,
    proof_sender: Sender<Message>,
) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("handle_proving_tasks"));

    loop {
        let Task {
            proposal_index,
            proving_args,
            proof_file_name,
        } = task_channel
            .1
            .recv()
            .await
            .context("task receiver channel closed")?;

        // Prove via kailua-host (re dev mode/bonsai: env vars inherited!)
        let mut kailua_host_command = Command::new(&kailua_host);
        // get fake receipts when building under devnet
        if is_dev_mode() {
            kailua_host_command.env("RISC0_DEV_MODE", "1");
        }
        // pass arguments to point at target block
        kailua_host_command.args(proving_args.clone());
        debug!("kailua_host_command {:?}", &kailua_host_command);
        // call the kailua-host binary to generate a proof
        let insufficient_l1_data = match await_tel_res!(
            context,
            tracer,
            "KailuaHost",
            kailua_host_command
                .kill_on_drop(true)
                .spawn()
                .context("Invoking kailua-host")?
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
                error!("Failed to invoke kailua-host: {e:?}");
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
