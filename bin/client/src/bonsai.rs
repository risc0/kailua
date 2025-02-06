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

use crate::proving::ProvingError;
use anyhow::{anyhow, Context};
use bonsai_sdk::non_blocking::Client;
use kailua_build::{KAILUA_FPVM_ELF, KAILUA_FPVM_ID};
use kailua_common::proof::Proof;
use risc0_zkvm::serde::to_vec;
use risc0_zkvm::sha::Digest;
use risc0_zkvm::{is_dev_mode, ProveInfo, Receipt, SessionStats};
use std::time::Duration;
use tracing::info;
use tracing::log::warn;

pub async fn run_bonsai_client(
    witness_frame: Vec<u8>,
    stitched_proofs: Vec<Proof>,
    prove_snark: bool,
) -> Result<Proof, ProvingError> {
    info!("Running Bonsai client.");
    // Instantiate client
    let client =
        Client::from_env(risc0_zkvm::VERSION).map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    // Prepare input payload
    let mut input = Vec::new();
    // Load witness data
    let witness_len = witness_frame.len() as u32;
    input.extend_from_slice(&witness_len.to_le_bytes());
    input.extend_from_slice(witness_frame.as_slice());
    // Load recursive proofs and upload succinct receipts
    let mut assumption_receipt_ids = vec![];
    for proof in stitched_proofs {
        if std::env::var("KAILUA_FORCE_RECURSION").is_ok() {
            warn!("(KAILUA_FORCE_RECURSION) Forcibly loading receipt as guest input.");
            // todo: convert boundless seals to groth16 receipts
            input.extend_from_slice(bytemuck::cast_slice(
                &to_vec(&proof).map_err(|e| ProvingError::OtherError(anyhow!(e)))?,
            ));
            continue;
        }

        if let Proof::ZKVMReceipt(receipt) = proof {
            let inner_receipt = *receipt;
            let serialized_receipt = bincode::serialize(&inner_receipt)
                .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
            let receipt_id = client
                .upload_receipt(serialized_receipt)
                .await
                .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
            assumption_receipt_ids.push(receipt_id);
        } else {
            // todo: convert boundless seals to groth16 receipts
            input.extend_from_slice(bytemuck::cast_slice(
                &to_vec(&proof).map_err(|e| ProvingError::OtherError(anyhow!(e)))?,
            ));
        }
    }

    // Upload the ELF with the image_id as its key.
    info!("Uploading Kailua ELF to Bonsai.");
    let image_id_hex = hex::encode(Digest::from(KAILUA_FPVM_ID));
    client
        .upload_img(&image_id_hex, KAILUA_FPVM_ELF.to_vec())
        .await
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;

    // Upload the input data
    info!("Uploading input data to Bonsai.");
    let input_id = client
        .upload_input(input)
        .await
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;

    // Create session on Bonsai
    info!("Creating Bonsai proving session.");
    let session = client
        .create_session_with_limit(image_id_hex, input_id, assumption_receipt_ids, false, None)
        .await
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    info!("Bonsai proving SessionID: {}", session.uuid);

    let polling_interval = if let Ok(ms) = std::env::var("BONSAI_POLL_INTERVAL_MS") {
        Duration::from_millis(
            ms.parse()
                .context("invalid bonsai poll interval")
                .map_err(|e| ProvingError::OtherError(anyhow!(e)))?,
        )
    } else {
        Duration::from_secs(1)
    };

    let succinct_prove_info = loop {
        // The session has already been started in the executor. Poll bonsai to check if
        // the proof request succeeded.
        let res = session
            .status(&client)
            .await
            .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
        if res.status == "RUNNING" {
            std::thread::sleep(polling_interval);
            continue;
        }
        if res.status == "SUCCEEDED" {
            // Download the receipt, containing the output
            info!("Downloading receipt from Bonsai.");
            let receipt_url = res.receipt_url.ok_or(ProvingError::OtherError(anyhow!(
                "API error, missing receipt on completed session"
            )))?;

            let stats = res
                .stats
                .context("Missing stats object on Bonsai status res")
                .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
            info!(
                "Bonsai usage: user_cycles: {} total_cycles: {}",
                stats.cycles, stats.total_cycles
            );

            let receipt_buf = client
                .download(&receipt_url)
                .await
                .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
            let receipt: Receipt = bincode::deserialize(&receipt_buf)
                .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;

            info!("Verifying receipt received from Bonsai.");
            receipt
                .verify(KAILUA_FPVM_ID)
                .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;

            break ProveInfo {
                receipt,
                stats: SessionStats {
                    segments: stats.segments,
                    total_cycles: stats.total_cycles,
                    user_cycles: stats.cycles,
                    // These are currently unavailable from Bonsai
                    paging_cycles: 0,
                    reserved_cycles: 0,
                },
            };
        } else {
            return Err(ProvingError::OtherError(anyhow!(
                "Bonsai prover workflow [{}] exited: {} err: {}",
                session.uuid,
                res.status,
                res.error_msg
                    .unwrap_or("Bonsai workflow missing error_msg".into()),
            )));
        }
    };

    if !prove_snark {
        return Ok(Proof::ZKVMReceipt(Box::new(succinct_prove_info.receipt)));
    }
    info!("Wrapping STARK as SNARK on Bonsai.");

    // Request that Bonsai compress further, to Groth16.
    let snark_session = client
        .create_snark(session.uuid)
        .await
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    let snark_receipt_url = loop {
        let res = snark_session
            .status(&client)
            .await
            .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
        match res.status.as_str() {
            "RUNNING" => {
                std::thread::sleep(polling_interval);
                continue;
            }
            "SUCCEEDED" => {
                break res
                    .output
                    .with_context(|| {
                        format!(
                            "Bonsai prover workflow [{}] reported success, but provided no receipt",
                            snark_session.uuid
                        )
                    })
                    .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
            }
            _ => {
                return Err(ProvingError::OtherError(anyhow!(
                    "Bonsai prover workflow [{}] exited: {} err: {}",
                    snark_session.uuid,
                    res.status,
                    res.error_msg
                        .unwrap_or("Bonsai workflow missing error_msg".into()),
                )))
            }
        }
    };

    info!("Downloading Groth16 receipt from Bonsai.");
    let receipt_buf = client
        .download(&snark_receipt_url)
        .await
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    let groth16_receipt: Receipt =
        bincode::deserialize(&receipt_buf).map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    groth16_receipt
        .verify(KAILUA_FPVM_ID)
        .context("failed to verify Groth16Receipt returned by Bonsai")
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;

    // todo: return Groth16Receipt variant
    Ok(Proof::ZKVMReceipt(Box::new(groth16_receipt)))
}

pub fn should_use_bonsai() -> bool {
    !is_dev_mode()
        && std::env::var("BONSAI_API_URL").is_ok()
        && std::env::var("BONSAI_API_KEY").is_ok()
}
