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

use crate::proving::ProvingError;
use anyhow::{anyhow, Context};
use kailua_build::{KAILUA_FPVM_ELF, KAILUA_FPVM_ID};
use kailua_common::proof::Proof;
use risc0_zkvm::{default_prover, is_dev_mode, ExecutorEnv, ProverOpts};
use tracing::info;
use tracing::log::warn;

pub async fn run_zkvm_client(
    witness_frames: Vec<Vec<u8>>,
    stitched_proofs: Vec<Proof>,
    prove_snark: bool,
    segment_limit: u32,
) -> Result<Proof, ProvingError> {
    info!("Running zkvm client.");
    let prove_info = tokio::task::spawn_blocking(move || {
        let env = build_zkvm_env(witness_frames, stitched_proofs, segment_limit)?;
        let prover = default_prover();
        let prover_opts = if prove_snark {
            ProverOpts::groth16()
        } else {
            ProverOpts::succinct()
        };
        let prove_info = prover
            .prove_with_opts(env, KAILUA_FPVM_ELF, &prover_opts)
            .context("prove_with_opts")?;
        Ok::<_, anyhow::Error>(prove_info)
    })
    .await
    .map_err(|e| ProvingError::OtherError(anyhow!(e)))?
    .map_err(|e| ProvingError::ExecutionError(anyhow!(e)))?;

    info!(
        "Proof of {} total cycles ({} user cycles) computed.",
        prove_info.stats.total_cycles, prove_info.stats.user_cycles
    );
    prove_info
        .receipt
        .verify(KAILUA_FPVM_ID)
        .context("receipt verification")
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    info!("Receipt verified.");

    // todo: return Groth16Receipt variant if snark
    Ok(Proof::ZKVMReceipt(Box::new(prove_info.receipt)))
}

pub fn build_zkvm_env<'a>(
    witness_frames: Vec<Vec<u8>>,
    stitched_proofs: Vec<Proof>,
    segment_limit: u32,
) -> anyhow::Result<ExecutorEnv<'a>> {
    // Execution environment
    let mut builder = ExecutorEnv::builder();
    // Set segment po2
    builder.segment_limit_po2(segment_limit);
    // Pass in witness data
    for frame in &witness_frames {
        builder.write_frame(frame);
    }
    // Dev-mode for recursive proofs
    if is_dev_mode() {
        builder.env_var("RISC0_DEV_MODE", "1");
    }
    // Pass in proofs
    for proof in stitched_proofs {
        // Force in-guest verification (should be used for testing only)
        if std::env::var("KAILUA_FORCE_RECURSION").is_ok() {
            warn!("(KAILUA_FORCE_RECURSION) Forcibly loading receipt as guest input.");
            builder.write(&proof)?;
            continue;
        }

        if let Proof::ZKVMReceipt(receipt) = proof {
            builder.add_assumption(*receipt);
        } else {
            builder.write(&proof)?;
        }
    }
    builder.build()
}
