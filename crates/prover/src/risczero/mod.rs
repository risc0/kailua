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

use crate::args::ProvingArgs;
use crate::proof::proof_file_name;
use crate::risczero::bonsai::{run_bonsai_client, should_use_bonsai};
use crate::risczero::boundless::{run_boundless_client, BoundlessArgs};
use crate::risczero::zkvm::run_zkvm_client;
use crate::{proof, ProvingError};
use anyhow::Context;
use kailua_build::KAILUA_FPVM_KONA_ID;
use kailua_kona::journal::ProofJournal;
use risc0_zkvm::{is_dev_mode, Receipt};
use tracing::info;

pub mod bonsai;
pub mod boundless;
pub mod zkvm;

/// Use our own version of SessionStats to avoid non-exhaustive issues (risc0_zkvm::SessionStats)
#[derive(Debug, Clone)]
pub struct KailuaSessionStats {
    pub segments: usize,
    pub total_cycles: u64,
    pub user_cycles: u64,
    pub paging_cycles: u64,
    pub reserved_cycles: u64,
}

/// Our own version of ProveInfo to avoid non-exhaustive issues (risc0_zkvm::ProveInfo)
#[derive(Debug)]
pub struct KailuaProveInfo {
    pub receipt: Receipt,
    pub stats: KailuaSessionStats,
}

pub async fn seek_proof(
    proving: &ProvingArgs,
    boundless: BoundlessArgs,
    proof_journal: ProofJournal,
    witness_frames: Vec<Vec<u8>>,
    stitched_proofs: Vec<Receipt>,
    prove_snark: bool,
) -> Result<(), ProvingError> {
    // compute the zkvm proof
    let proof = match (boundless.market, boundless.storage) {
        (Some(marked_provider_config), Some(storage_provider_config)) if !is_dev_mode() => {
            run_boundless_client(
                marked_provider_config,
                storage_provider_config,
                proof_journal,
                witness_frames,
                stitched_proofs,
                proving,
            )
            .await?
        }
        _ => {
            if should_use_bonsai() {
                run_bonsai_client(witness_frames, stitched_proofs, prove_snark, proving).await?
            } else {
                run_zkvm_client(witness_frames, stitched_proofs, prove_snark, proving).await?
            }
        }
    };

    // Save proof file to disk
    let proof_journal = ProofJournal::decode_packed(proof.journal.as_ref());
    let file_name = proof_file_name(KAILUA_FPVM_KONA_ID, &proof_journal);
    proof::save_to_bincoded_file(&proof, &file_name)
        .await
        .context("save_to_bincoded_file")
        .map_err(ProvingError::OtherError)?;
    info!("Saved proof to file {file_name}");

    Ok(())
}
