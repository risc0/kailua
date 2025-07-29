// Copyright 2025 RISC Zero, Inc.
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

use alloy::eips::BlockNumberOrTag;
use alloy::providers::Provider;
use anyhow::{anyhow, Context};
use kailua_prover::args::ProvingArgs;
use kailua_prover::backends::boundless::BoundlessArgs;
use kailua_sync::args::SyncArgs;
use kailua_sync::provider::{ProviderArgs, SyncProvider};
use kailua_sync::telemetry::TelemetryArgs;
use kailua_sync::transact::signer::ValidatorSignerArgs;
use kailua_sync::transact::TransactArgs;
use kailua_sync::{await_tel, await_tel_res, retry_res_ctx_timeout};
use kailua_validator::args::ValidateArgs;
use kailua_validator::channel::{DuplexChannel, Message};
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;
use tokio::{spawn, try_join};
use tracing::{debug, error, info};

/// Validity prove any running OP Stack rollup
#[derive(clap::Args, Debug, Clone)]
pub struct DemoArgs {
    #[clap(flatten)]
    pub provider: ProviderArgs,

    /// Path to the prover binary to use for proving
    #[clap(long, env)]
    pub kailua_cli: Option<PathBuf>,
    /// How many proofs to compute simultaneously
    #[clap(long, env, default_value_t = 1)]
    pub num_concurrent_provers: u64,
    /// Optionally enables the use of `debug_executePayload` to collect the execution witness from
    /// the execution layer.
    #[arg(long, env, default_value_t = false)]
    pub enable_experimental_witness_endpoint: bool,

    /// The L2 block to start proving from.
    /// Defaults to `nth_proof_to_process` times `num_blocks_per_proof` before latest safe block.
    #[clap(long, env)]
    pub starting_block_height: Option<u64>,
    /// The number of L2 blocks to cover per proof
    #[clap(long, env)]
    pub num_blocks_per_proof: u64,
    /// The nth proof to process. Defaults to every proof (=1).
    #[clap(long, env, default_value_t = 1)]
    pub nth_proof_to_process: u64,

    /// Directory to use for caching data
    #[clap(long, env)]
    pub data_dir: Option<PathBuf>,

    #[clap(flatten)]
    pub proving: ProvingArgs,
    #[clap(flatten)]
    pub boundless: BoundlessArgs,
    #[clap(flatten)]
    pub telemetry: TelemetryArgs,
}

pub async fn demo(args: DemoArgs, verbosity: u8, data_dir: PathBuf) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("demo"));

    debug!("{args:?}");

    let channel_pair = DuplexChannel::new_pair(4096);

    let handle_blocks =
        spawn(handle_blocks(channel_pair.0, args.clone()).with_context(context.clone()));

    let validate_args = ValidateArgs {
        sync: SyncArgs {
            provider: args.provider,
            kailua_game_implementation: None,
            kailua_anchor_address: None,
            #[cfg(feature = "devnet")]
            delay_l2_blocks: 0,
            final_l2_block: None,
            data_dir: args.data_dir,
            telemetry: args.telemetry,
        },
        kailua_cli: args.kailua_cli,
        fast_forward_start: 0,
        fast_forward_target: 0,
        num_concurrent_provers: args.num_concurrent_provers,
        enable_experimental_witness_endpoint: args.enable_experimental_witness_endpoint,
        #[cfg(feature = "devnet")]
        l1_head_jump_back: 0,
        validator_signer: ValidatorSignerArgs {
            validator_key: Some(String::from(
                "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba",
            )),
            ..Default::default()
        },
        txn_args: TransactArgs {
            txn_timeout: 0,
            exec_gas_premium: 0,
            blob_gas_premium: 0,
        },
        proving: args.proving,
        boundless: args.boundless,
    };
    let handle_proof_requests = spawn(
        kailua_validator::requests::handle_proof_requests(
            channel_pair.1,
            validate_args,
            verbosity,
            data_dir,
        )
        .with_context(context.clone()),
    );

    let (blocks_task, proofs_task) = try_join!(handle_blocks, handle_proof_requests)?;
    blocks_task.context("handle_blocks")?;
    proofs_task.context("handle_proof_requests")?;

    Ok(())
}

pub async fn handle_blocks(
    mut channel: DuplexChannel<Message>,
    args: DemoArgs,
) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("handle_blocks"));

    // Connect to RPC providers
    let provider = await_tel_res!(
        context,
        SyncProvider::new(&args.provider),
        "SyncProvider::new"
    )?;

    let mut last_proven = args.starting_block_height;
    let mut last_wait = 0;
    let mut n = 1u64;
    loop {
        // Wait for new data on every iteration
        sleep(Duration::from_secs(6)).await;
        // more output commitments
        let sync_status = await_tel!(
            context,
            tracer,
            "sync_status",
            retry_res_ctx_timeout!(provider.op_provider.sync_status().await)
        );
        let Some(safe_l2_number) = sync_status["safe_l2"]["number"].as_u64() else {
            error!("Failed to parse safe_l2_number");
            continue;
        };
        let l1_head = await_tel!(
            context,
            tracer,
            "l1_head",
            retry_res_ctx_timeout!(provider
                .l1_provider
                .get_block_by_number(BlockNumberOrTag::Latest)
                .await
                .context("get_block_by_number")?
                .ok_or_else(|| anyhow!("Failed to fetch l1 head")))
        );
        // start from most recent block if unspecified
        if last_proven.is_none() {
            last_proven = Some(
                safe_l2_number
                    .saturating_sub(args.nth_proof_to_process * args.num_blocks_per_proof + 1),
            );
        }
        // queue required proofs
        while last_proven.unwrap() + args.num_blocks_per_proof < safe_l2_number {
            let agreed_l2_block_number = last_proven.unwrap();
            let agreed_l2_block = await_tel!(
                context,
                tracer,
                "agreed_l2_block",
                retry_res_ctx_timeout!(provider
                    .l2_provider
                    .get_block_by_number(BlockNumberOrTag::Number(agreed_l2_block_number))
                    .await
                    .context("get_block_by_number")?
                    .ok_or_else(|| anyhow!("Failed to fetch agreed l2 block")))
            );
            let agreed_l2_output_root = await_tel!(
                context,
                tracer,
                "agreed_l2_output_root",
                retry_res_ctx_timeout!(
                    provider
                        .op_provider
                        .output_at_block(agreed_l2_block_number)
                        .await
                )
            );
            let claimed_l2_block_number = agreed_l2_block_number + args.num_blocks_per_proof;
            let claimed_l2_output_root = await_tel!(
                context,
                tracer,
                "claimed_l2_output_root",
                retry_res_ctx_timeout!(
                    provider
                        .op_provider
                        .output_at_block(claimed_l2_block_number)
                        .await
                )
            );
            // request proof
            if n == args.nth_proof_to_process {
                channel
                    .sender
                    .send(Message::Proposal {
                        index: last_proven.unwrap(),
                        precondition_validation_data: None,
                        l1_head: l1_head.header.hash,
                        agreed_l2_head_hash: agreed_l2_block.header.hash,
                        agreed_l2_output_root,
                        claimed_l2_block_number,
                        claimed_l2_output_root,
                    })
                    .await?;
                info!(
                    "Requested proof for blocks {} to {}. (N={n})",
                    agreed_l2_block_number, claimed_l2_block_number
                );
                n = 0;
            } else {
                info!(
                    "Skipped proof for blocks {} to {}. (N={n})",
                    agreed_l2_block_number, claimed_l2_block_number
                );
            }
            n += 1;
            // update state
            last_proven = Some(claimed_l2_block_number);
        }
        let wait = args
            .num_blocks_per_proof
            .saturating_sub(safe_l2_number.saturating_sub(last_proven.unwrap()));
        if wait != last_wait {
            info!("Waiting for {wait} more safe L2 blocks to request a new proof.",);
            last_wait = wait;
        }
        // report completed proofs
        while !channel.receiver.is_empty() {
            let Some(message) = channel.receiver.recv().await else {
                error!("Proofs receiver channel closed");
                break;
            };
            let Message::Proof(starting_block, _) = message else {
                error!("Received an unexpected message type");
                continue;
            };
            let ending_block = starting_block + args.num_blocks_per_proof;
            info!("Computed proof for blocks {starting_block} to {ending_block}.");
        }
    }
}
