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

use alloy::primitives::U256;
use alloy::signers::local::PrivateKeySigner;
use alloy::transports::http::reqwest::Url;
use anyhow::Context;
use boundless_market::{Client, StandardStorageProvider, StorageProviderConfig};
use kailua_build::KAILUA_FPVM_ID;
use kailua_common::journal::ProofJournal;
use kailua_prover::backends::boundless::retrieve_proof;
use kailua_prover::client::proving::save_to_bincoded_file;
use kailua_prover::proof::{proof_file_name, read_bincoded_file};
use kailua_sync::retry_res_timeout;
use kailua_sync::telemetry::TelemetryArgs;
use kailua_validator::proposals::dispatch::current_time;
use risc0_zkvm::Receipt;
use std::str::FromStr;
use tracing::{error, info};

#[derive(clap::Args, Debug, Clone)]
pub struct BoundlessArgs {
    /// URL of the Ethereum RPC endpoint.
    #[clap(long, env, required = false)]
    pub boundless_rpc_url: Url,

    #[clap(long, env)]
    pub request_id: String,
    #[clap(flatten)]
    pub telemetry: TelemetryArgs,
}

pub async fn boundless(args: BoundlessArgs) -> anyhow::Result<()> {
    let boundless_client = retry_res_timeout!(
        15,
        Client::builder()
            .with_private_key(PrivateKeySigner::random())
            .with_rpc_url(args.boundless_rpc_url.clone())
            .with_storage_provider(Some(StandardStorageProvider::from_config(
                &StorageProviderConfig::dev_mode()
            )?))
            .build()
            .await
            .context("ClientBuilder::build()")
    )
    .await;

    let receipt = retrieve_proof(
        &boundless_client,
        U256::from_str(args.request_id.as_str())?,
        1,
        current_time(),
    )
    .await?;

    let proof_journal = ProofJournal::decode_packed(receipt.journal.as_ref());
    let file_name = proof_file_name(&proof_journal);

    info!("Writing proof to {file_name}.");
    if let Ok(prior_receipt) = read_bincoded_file::<Receipt>(&file_name).await {
        if prior_receipt.verify(KAILUA_FPVM_ID).is_ok() {
            info!("Skipping overwriting valid receipt file.");
            return Ok(());
        }
        info!("Overwriting invalid receipt file.");
    }

    if let Err(err) = save_to_bincoded_file(&receipt, &file_name).await {
        error!("Failed to write proof to {file_name}: {err:?}");
    }

    info!("Proof written to {file_name}.");

    Ok(())
}
