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
use crate::backends::bonsai::{run_bonsai_client, should_use_bonsai};
use crate::backends::boundless::{run_boundless_client, BoundlessArgs};
use crate::backends::zkvm::run_zkvm_client;
use crate::client::witgen;
use crate::proof::proof_file_name;
use crate::ProvingError;
use alloy_primitives::B256;
use anyhow::{anyhow, Context};
use human_bytes::human_bytes;
use kailua_common::boot::StitchedBootInfo;
use kailua_common::client::stitching::split_executions;
use kailua_common::executor::Execution;
use kailua_common::journal::ProofJournal;
use kailua_common::oracle::vec::{PreimageVecEntry, VecOracle};
use kailua_common::witness::Witness;
use kona_preimage::{HintWriterClient, PreimageOracleClient};
use kona_proof::l1::OracleBlobProvider;
use kona_proof::CachingOracle;
use risc0_zkvm::{is_dev_mode, Receipt};
use serde::Serialize;
use std::fmt::Debug;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tracing::{info, warn};

/// The size of the LRU cache in the oracle.
pub const ORACLE_LRU_SIZE: usize = 1024;

#[allow(clippy::too_many_arguments)]
pub async fn run_proving_client<P, H>(
    proving: ProvingArgs,
    boundless: BoundlessArgs,
    oracle_client: P,
    hint_client: H,
    precondition_validation_data_hash: B256,
    stitched_executions: Vec<Vec<Execution>>,
    stitched_boot_info: Vec<StitchedBootInfo>,
    stitched_proofs: Vec<Receipt>,
    prove_snark: bool,
    force_attempt: bool,
    seek_proof: bool,
) -> Result<(), ProvingError>
where
    P: PreimageOracleClient + Send + Sync + Debug + Clone + 'static,
    H: HintWriterClient + Send + Sync + Debug + Clone + 'static,
{
    // preload all data into the vec oracle
    let (_, execution_cache) = split_executions(stitched_executions.clone());
    info!(
        "Running vec witgen client with {} cached executions ({} traces).",
        execution_cache.len(),
        stitched_executions.len()
    );
    let (proof_journal, mut witness_vec): (ProofJournal, Witness<VecOracle>) = {
        // Instantiate oracles
        let preimage_oracle = Arc::new(CachingOracle::new(
            ORACLE_LRU_SIZE,
            oracle_client,
            hint_client,
        ));
        let blob_provider = OracleBlobProvider::new(preimage_oracle.clone());
        // Run witness generation with oracles
        witgen::run_witgen_client(
            preimage_oracle,
            10 * 1024 * 1024, // default to 10MB chunks
            blob_provider,
            proving.payout_recipient_address.unwrap_or_default(),
            precondition_validation_data_hash,
            execution_cache.clone(),
            stitched_boot_info.clone(),
        )
        .await
        .context("Failed to run vec witgen client.")
        .map_err(ProvingError::OtherError)?
    };

    let execution_trace =
        core::mem::replace(&mut witness_vec.stitched_executions, stitched_executions);

    // sanity check kzg proofs
    let _ = kailua_common::blobs::PreloadedBlobProvider::from(witness_vec.blobs_witness.clone());

    // check if we can prove this workload
    let (preloaded_wit_size, streamed_wit_size) = sum_witness_size(&witness_vec);
    let total_wit_size = preloaded_wit_size + streamed_wit_size;
    info!(
        "Witness size: {} ({} preloaded, {} streamed.)",
        human_bytes(total_wit_size as f64),
        human_bytes(preloaded_wit_size as f64),
        human_bytes(streamed_wit_size as f64)
    );
    if total_wit_size > proving.max_witness_size {
        warn!(
            "Witness size {} exceeds limit {}.",
            human_bytes(total_wit_size as f64),
            human_bytes(proving.max_witness_size as f64)
        );
        if !force_attempt {
            warn!("Aborting.");
            return Err(ProvingError::WitnessSizeError(
                total_wit_size,
                proving.max_witness_size,
                execution_trace,
            ));
        }
        warn!("Continuing..");
    }

    if !seek_proof {
        return Err(ProvingError::NotSeekingProof(
            total_wit_size,
            execution_trace,
        ));
    }

    let (preloaded_frames, streamed_frames) =
        encode_witness_frames(witness_vec).expect("Failed to encode VecOracle");
    seek_fpvm_proof(
        &proving,
        boundless,
        proof_journal,
        [preloaded_frames, streamed_frames].concat(),
        stitched_proofs,
        prove_snark,
    )
    .await
}

#[allow(clippy::type_complexity)]
pub fn encode_witness_frames(
    witness_vec: Witness<VecOracle>,
) -> anyhow::Result<(Vec<Vec<u8>>, Vec<Vec<u8>>)> {
    // serialize preloaded shards
    let mut preloaded_data = witness_vec.oracle_witness.preimages.lock().unwrap();
    let shards = shard_witness_data(&mut preloaded_data)?;
    drop(preloaded_data);
    // serialize streamed data
    let mut streamed_data = witness_vec.stream_witness.preimages.lock().unwrap();
    let mut streams = shard_witness_data(&mut streamed_data)?;
    streams.reverse();
    streamed_data.clear();
    drop(streamed_data);
    // serialize main witness object
    let main_frame = rkyv::to_bytes::<rkyv::rancor::Error>(&witness_vec)
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?
        .to_vec();
    let preloaded_data = [vec![main_frame], shards].concat();

    Ok((preloaded_data, streams))
}

pub fn shard_witness_data(data: &mut [PreimageVecEntry]) -> anyhow::Result<Vec<Vec<u8>>> {
    let mut shards = vec![];
    for entry in data {
        let shard = core::mem::take(entry);
        shards.push(
            rkyv::to_bytes::<rkyv::rancor::Error>(&shard)
                .map_err(|e| ProvingError::OtherError(anyhow!(e)))?
                .to_vec(),
        )
    }
    Ok(shards)
}

pub fn sum_witness_size(witness: &Witness<VecOracle>) -> (usize, usize) {
    let (witness_frames, streamed_frames) =
        encode_witness_frames(witness.deep_clone()).expect("Failed to encode VecOracle");
    (
        witness_frames.iter().map(|f| f.len()).sum::<usize>(),
        streamed_frames.iter().map(|f| f.len()).sum::<usize>(),
    )
}
pub async fn seek_fpvm_proof(
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
                run_bonsai_client(
                    witness_frames,
                    stitched_proofs,
                    prove_snark,
                    proving.skip_await_proof,
                )
                .await?
            } else {
                run_zkvm_client(
                    witness_frames,
                    stitched_proofs,
                    prove_snark,
                    proving.segment_limit,
                )
                .await?
            }
        }
    };

    // Save proof file to disk
    let proof_journal = ProofJournal::decode_packed(proof.journal.as_ref());
    let file_name = proof_file_name(&proof_journal);
    save_to_bincoded_file(&proof, &file_name)
        .await
        .context("save_to_bincoded_file")
        .map_err(ProvingError::OtherError)?;
    info!("Saved proof to file {file_name}");

    Ok(())
}

pub async fn save_to_bincoded_file<T: Serialize>(value: &T, file_name: &str) -> anyhow::Result<()> {
    let mut file = File::create(file_name)
        .await
        .context("Failed to create output file.")?;
    let data = bincode::serialize(value).context("Could not serialize proving data.")?;
    file.write_all(data.as_slice())
        .await
        .context("Failed to write proof to file")?;
    file.flush()
        .await
        .context("Failed to flush proof output file data.")?;
    Ok(())
}
