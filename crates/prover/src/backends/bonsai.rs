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

use crate::ProvingError;
use anyhow::{anyhow, Context};
use bonsai_sdk::non_blocking::{Client, SessionId, SnarkId};
use bonsai_sdk::responses::SessionStats;
use human_bytes::human_bytes;
use kailua_build::{KAILUA_FPVM_ELF, KAILUA_FPVM_ID};
use kailua_sync::{retry_res, retry_res_timeout};
use risc0_zkvm::serde::to_vec;
use risc0_zkvm::sha::Digest;
use risc0_zkvm::{is_dev_mode, InnerReceipt, Receipt};
use std::time::Duration;
use tokio::time::sleep;
use tracing::log::warn;
use tracing::{error, info};

pub async fn run_bonsai_client(
    witness_frames: Vec<Vec<u8>>,
    stitched_proofs: Vec<Receipt>,
    prove_snark: bool,
    skip_await_proof: bool,
) -> Result<Receipt, ProvingError> {
    info!("Running Bonsai client.");
    // Instantiate client
    let client =
        Client::from_env(risc0_zkvm::VERSION).map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    // Prepare input payload
    let mut input = Vec::new();
    // Load witness data
    for frame in witness_frames {
        let witness_len = frame.len() as u32;
        input.extend_from_slice(&witness_len.to_le_bytes());
        input.extend_from_slice(frame.as_slice());
    }
    // Load recursive proofs and upload succinct receipts
    let mut assumption_receipt_ids = vec![];
    for receipt in stitched_proofs {
        if std::env::var("KAILUA_FORCE_RECURSION").is_ok() {
            warn!("(KAILUA_FORCE_RECURSION) Forcibly loading receipt as guest input.");
            input.extend_from_slice(bytemuck::cast_slice(
                &to_vec(&receipt).map_err(|e| ProvingError::OtherError(anyhow!(e)))?,
            ));
            continue;
        }

        if matches!(receipt.inner, InnerReceipt::Groth16(_)) {
            input.extend_from_slice(bytemuck::cast_slice(
                &to_vec(&receipt).map_err(|e| ProvingError::OtherError(anyhow!(e)))?,
            ));
        } else {
            let serialized_receipt =
                bincode::serialize(&receipt).map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
            let receipt_id = client
                .upload_receipt(serialized_receipt)
                .await
                .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
            assumption_receipt_ids.push(receipt_id);
        }
    }

    // Create a session on Bonsai
    let mut stark_session =
        create_stark_session(&client, input.clone(), assumption_receipt_ids.clone()).await;

    if skip_await_proof {
        warn!("Skipping awaiting proof on Bonsai and exiting process.");
        std::process::exit(0);
    }

    let polling_interval = if let Ok(ms) = std::env::var("BONSAI_POLL_INTERVAL_MS") {
        Duration::from_millis(
            ms.parse()
                .context("invalid bonsai poll interval")
                .map_err(|e| ProvingError::OtherError(anyhow!(e)))?,
        )
    } else {
        Duration::from_secs(1)
    };

    let stark_receipt = loop {
        // The session has already been started in the executor. Poll bonsai to check if
        // the proof request succeeded.
        let res = retry_res!(stark_session.status(&client).await).await;

        match res.status.as_str() {
            "RUNNING" => tokio::time::sleep(polling_interval).await,
            "SUCCEEDED" => {
                // Download the receipt, containing the output
                let Some(receipt_url) = res.receipt_url else {
                    error!("API error, missing receipt on completed session");
                    continue;
                };

                let stats = res.stats.unwrap_or_else(|| {
                    error!("Missing stats object on Bonsai response.");
                    SessionStats {
                        segments: 0,
                        total_cycles: 0,
                        cycles: 0,
                    }
                });

                info!(
                    "Bonsai usage: user_cycles: {} total_cycles: {}",
                    stats.cycles, stats.total_cycles
                );

                info!("Downloading Bonsai receipt from {receipt_url}.");
                let Ok(receipt_buf) = client.download(&receipt_url).await else {
                    error!("Failed to download STARK receipt at {receipt_url}");
                    continue;
                };

                info!("Verifying receipt received from Bonsai.");
                let Ok(receipt) = bincode::deserialize::<Receipt>(&receipt_buf) else {
                    error!("Failed to deserialize receipt at {receipt_url}");
                    continue;
                };
                let Ok(()) = receipt.verify(KAILUA_FPVM_ID) else {
                    error!("Failed to verify receipt at {receipt_url}.");
                    continue;
                };

                break receipt;
            }
            _ => {
                error!(
                    "Bonsai prover session [{}] exited: {} err: {:?}. Retrying.",
                    stark_session.uuid, res.status, res.error_msg
                );
                // Retry and create another session
                stark_session =
                    create_stark_session(&client, input.clone(), assumption_receipt_ids.clone())
                        .await;
            }
        }
    };

    if !prove_snark {
        return Ok(stark_receipt);
    }
    info!("Wrapping STARK as SNARK on Bonsai.");
    let stark_receipt_bincoded =
        bincode::serialize(&stark_receipt).map_err(|e| ProvingError::OtherError(anyhow!(e)))?;

    // Request that Bonsai compress further, to Groth16.
    let mut snark_session = create_snark_session(
        &client,
        stark_receipt_bincoded.clone(),
        Some(stark_session.uuid),
    )
    .await;

    let groth16_receipt = loop {
        let res = retry_res!(snark_session.status(&client).await).await;

        match res.status.as_str() {
            "RUNNING" => sleep(polling_interval).await,
            "SUCCEEDED" => {
                let Some(receipt_url) = res.output else {
                    error!("SNARK API error, missing output url.");
                    continue;
                };

                info!("Downloading Groth16 receipt from Bonsai.");
                let Ok(receipt_buf) = client.download(&receipt_url).await else {
                    error!("Failed to download SNARK receipt at {receipt_url}");
                    continue;
                };

                info!("Verifying receipt received from Bonsai.");
                let Ok(receipt) = bincode::deserialize::<Receipt>(&receipt_buf) else {
                    error!("Failed to deserialize SNARK receipt at {receipt_url}");
                    continue;
                };
                let Ok(()) = receipt.verify(KAILUA_FPVM_ID) else {
                    error!("Failed to verify SNARK receipt at {receipt_url}.");
                    continue;
                };

                break receipt;
            }
            _ => {
                error!(
                    "Bonsai prover workflow [{}] exited: {} err: {:?}",
                    snark_session.uuid, res.status, res.error_msg
                );
                snark_session =
                    create_snark_session(&client, stark_receipt_bincoded.clone(), None).await;
            }
        }
    };

    Ok(groth16_receipt)
}

pub async fn create_snark_session(
    client: &Client,
    receipt: Vec<u8>,
    mut stark_id: Option<String>,
) -> SnarkId {
    loop {
        // Reupload receipt if not first attempt
        let stark_id = match stark_id.take() {
            Some(id) => id,
            None => retry_res!(client.upload_receipt(receipt.clone()).await).await,
        };

        // Request that Bonsai compress further, to Groth16.
        if let Ok(result) = client.create_snark(stark_id.clone()).await {
            break result;
        }
    }
}

pub async fn create_stark_session(
    client: &Client,
    input: Vec<u8>,
    assumption_receipt_ids: Vec<String>,
) -> SessionId {
    // Upload the ELF with the image_id as its key.
    let elf = KAILUA_FPVM_ELF.to_vec();
    let image_id_hex = hex::encode(Digest::from(KAILUA_FPVM_ID));
    let is_image_present = retry_res_timeout!(client.has_img(&image_id_hex).await).await;
    if !is_image_present {
        info!(
            "Uploading {} Kailua ELF to Bonsai.",
            human_bytes(elf.len() as f64)
        );
        retry_res!(client.upload_img(&image_id_hex, elf.clone()).await).await;
    } else {
        info!("Kailua ELF already exists on Bonsai.");
    }

    // Retry session creation w/ fresh input each time just in case.
    loop {
        // Upload the input data
        info!(
            "Uploading {} input data to Bonsai.",
            human_bytes(input.len() as f64)
        );
        let input_id = retry_res!(client.upload_input(input.clone()).await).await;

        // Create session on Bonsai
        info!("Creating Bonsai proving session.");
        let session = match client
            .create_session_with_limit(
                image_id_hex.clone(),
                input_id.clone(),
                assumption_receipt_ids.clone(),
                false,
                None,
            )
            .await
        {
            Ok(session) => session,
            Err(err) => {
                error!("Could not create Bonsai session ID: {err:?}");
                sleep(Duration::from_millis(500)).await;
                continue;
            }
        };

        info!("Bonsai proving SessionID: {}", session.uuid);

        sleep(Duration::from_millis(500)).await;
        break session;
    }
}

pub fn should_use_bonsai() -> bool {
    !is_dev_mode()
        && std::env::var("BONSAI_API_URL").is_ok()
        && std::env::var("BONSAI_API_KEY").is_ok()
}
