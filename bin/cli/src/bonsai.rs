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

use anyhow::{anyhow, Context};
use bonsai_sdk::non_blocking::{Client, SessionId};
use kailua_build::KAILUA_FPVM_KONA_ID;
use kailua_prover::proof::save_to_bincoded_file;
use kailua_prover::proof::{proof_file_name, read_bincoded_file};
use kailua_prover::risczero::{KailuaProveInfo, KailuaSessionStats};
use kailua_prover::ProvingError;
use kailua_sync::telemetry::TelemetryArgs;
use risc0_zkvm::Receipt;
use std::time::Duration;
use tracing::{error, info, warn};

#[derive(clap::Args, Debug, Clone)]
pub struct BonsaiArgs {
    #[clap(long, env)]
    pub session_id: String,
    #[clap(flatten)]
    pub telemetry: TelemetryArgs,
}

pub async fn bonsai(args: BonsaiArgs) -> anyhow::Result<()> {
    // Instantiate client
    let client = Client::from_env(risc0_zkvm::VERSION)?;
    // fetch session
    let session = SessionId::new(args.session_id);
    // wait for receipt
    let kailua_prove_info = loop {
        let res = session.status(&client).await?;

        if res.status == "RUNNING" {
            tokio::time::sleep(Duration::from_secs(1)).await;
            warn!("Proof not yet completed: {:?}", res.status);
            continue;
        }

        if res.status != "SUCCEEDED" {
            error!(
                "Bonsai prover workflow [{}] exited: {} err: {}",
                session.uuid,
                res.status,
                res.error_msg
                    .unwrap_or("Bonsai workflow missing error_msg".into()),
            );
            return Ok(());
        }

        // Download the receipt, containing the output
        let receipt_url = res.receipt_url.ok_or(ProvingError::OtherError(anyhow!(
            "API error, missing receipt on completed session"
        )))?;
        info!("Downloading Bonsai receipt from {receipt_url}.");

        let stats = res
            .stats
            .context("Missing stats object on Bonsai status res")?;
        info!(
            "Bonsai usage: user_cycles: {} total_cycles: {}",
            stats.cycles, stats.total_cycles
        );

        let receipt_buf = client.download(&receipt_url).await?;
        let receipt: Receipt = bincode::deserialize(&receipt_buf)?;

        info!("Verifying receipt received from Bonsai.");
        receipt.verify(KAILUA_FPVM_KONA_ID)?;

        break KailuaProveInfo {
            receipt,
            stats: KailuaSessionStats {
                segments: stats.segments,
                total_cycles: stats.total_cycles,
                user_cycles: stats.cycles,
                // These are currently unavailable from Bonsai
                paging_cycles: 0,
                reserved_cycles: 0,
            },
        };
    };

    let file_name = proof_file_name(
        KAILUA_FPVM_KONA_ID,
        kailua_prove_info.receipt.journal.clone(),
    );

    info!("Writing proof to {file_name}.");
    if let Ok(prior_receipt) = read_bincoded_file::<Receipt>(&file_name).await {
        if prior_receipt.verify(KAILUA_FPVM_KONA_ID).is_ok() {
            info!("Skipping overwriting valid receipt file.");
            return Ok(());
        }
        info!("Overwriting invalid receipt file.");
    }

    if let Err(err) = save_to_bincoded_file(&kailua_prove_info.receipt, &file_name).await {
        error!("Failed to write proof to {file_name}: {err:?}");
    }

    info!("Proof written to {file_name}.");

    Ok(())
}
