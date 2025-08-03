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

use crate::args::ProvingArgs;
use crate::risczero::{KailuaProveInfo, KailuaSessionStats};
use crate::ProvingError;
use anyhow::{anyhow, Context};
use bytemuck::NoUninit;
use risc0_zkvm::{default_prover, Digest, ExecutorEnv, InnerReceipt, ProverOpts, Receipt};
use tracing::info;
use tracing::log::warn;

pub async fn run_zkvm_client<A: NoUninit + Into<Digest>>(
    image: (A, &[u8]),
    witness_slices: Vec<Vec<u32>>,
    witness_frames: Vec<Vec<u8>>,
    stitched_proofs: Vec<Receipt>,
    prove_snark: bool,
    proving_args: &ProvingArgs,
) -> Result<Receipt, ProvingError> {
    info!("Running zkvm client.");
    if proving_args.skip_await_proof {
        warn!("Skipping awaiting proof locally.");
        return Err(ProvingError::NotAwaitingProof);
    }

    let segment_limit = proving_args.segment_limit;
    let elf = image.1.to_vec();
    let prove_info = tokio::task::spawn_blocking(move || {
        let env = build_zkvm_env(
            witness_slices,
            witness_frames,
            stitched_proofs,
            segment_limit,
        )?;
        let prover = default_prover();
        let prover_opts = if prove_snark {
            ProverOpts::groth16()
        } else {
            ProverOpts::succinct()
        };
        let risc0_prove_info = prover
            .prove_with_opts(env, &elf, &prover_opts)
            .context("prove_with_opts")?;

        // Convert to our own KailuaProveInfo
        let kailua_prove_info = KailuaProveInfo {
            receipt: risc0_prove_info.receipt,
            stats: KailuaSessionStats {
                segments: risc0_prove_info.stats.segments,
                total_cycles: risc0_prove_info.stats.total_cycles,
                user_cycles: risc0_prove_info.stats.user_cycles,
                paging_cycles: risc0_prove_info.stats.paging_cycles,
                reserved_cycles: risc0_prove_info.stats.reserved_cycles,
            },
        };

        Ok::<_, anyhow::Error>(kailua_prove_info)
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
        .verify(image.0)
        .context("receipt verification")
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    info!("Receipt verified.");

    Ok(prove_info.receipt)
}

#[allow(deprecated)]
pub fn build_zkvm_env<'a>(
    witness_slices: Vec<Vec<u32>>,
    witness_frames: Vec<Vec<u8>>,
    stitched_proofs: Vec<Receipt>,
    segment_limit: u32,
) -> anyhow::Result<ExecutorEnv<'a>> {
    // Execution environment
    let mut builder = ExecutorEnv::builder();
    // Set segment po2
    builder.segment_limit_po2(segment_limit);
    // Pass in witness data slices
    for slice in &witness_slices {
        builder.write_slice(slice);
    }
    // Pass in witness data frames
    for frame in &witness_frames {
        builder.write_frame(frame);
    }
    // Dev-mode for recursive proofs
    if risc0_zkvm::is_dev_mode() {
        builder.env_var("RISC0_DEV_MODE", "1");
    }
    // Pass in proofs
    for receipt in stitched_proofs {
        // Force in-guest verification (should be used for testing only)
        if std::env::var("KAILUA_FORCE_RECURSION").is_ok() {
            warn!("(KAILUA_FORCE_RECURSION) Forcibly loading receipt as guest input.");
            builder.write(&receipt)?;
            continue;
        }

        if matches!(receipt.inner, InnerReceipt::Groth16(_)) {
            builder.write(&receipt)?;
        } else {
            builder.add_assumption(receipt);
        }
    }

    builder.build()
}
