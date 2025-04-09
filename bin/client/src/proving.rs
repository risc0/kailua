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

use crate::args::parse_address;
use crate::{bonsai, proof, witgen, zkvm};
use alloy_primitives::{Address, B256};
use anyhow::anyhow;
use clap::Parser;
use kailua_common::client::stitching::split_executions;
use kailua_common::executor::Execution;
use kailua_common::journal::ProofJournal;
use kailua_common::oracle::vec::{PreimageVecEntry, VecOracle};
use kailua_common::witness::{StitchedBootInfo, Witness};
use kona_preimage::{HintWriterClient, PreimageOracleClient};
use kona_proof::l1::OracleBlobProvider;
use kona_proof::CachingOracle;
use risc0_zkvm::Receipt;
use std::fmt::Debug;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tracing::{error, info, warn};

/// The size of the LRU cache in the oracle.
pub const ORACLE_LRU_SIZE: usize = 1024;

#[derive(Parser, Clone, Debug)]
pub struct ProvingArgs {
    #[clap(long, env, value_parser = parse_address)]
    pub payout_recipient_address: Option<Address>,
    #[clap(long, env, required = false, default_value_t = 21)]
    pub segment_limit: u32,
    #[clap(long, env, required = false, default_value_t = 2_684_354_560)]
    pub max_witness_size: usize,
    #[clap(long, env, default_value_t = false)]
    pub skip_derivation_proof: bool,
}

impl ProvingArgs {
    pub fn can_fit_witness(&self, witness: &Witness<VecOracle>) -> bool {
        let (witness_frames, _) =
            encode_witness_frames(witness.deep_clone()).expect("Failed to encode VecOracle");
        let witness_size = witness_frames.iter().map(|f| f.len()).sum::<usize>();
        witness_size < self.max_witness_size
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProvingError {
    #[error("DerivationProofError error: execution proofs {0}")]
    DerivationProofError(usize),

    #[error("SeekProofError error: witness {0}")]
    SeekProofError(usize, Vec<Vec<Execution>>),

    #[error("WitnessSizeError error: size {0} limit {0}")]
    WitnessSizeError(usize, usize, Vec<Vec<Execution>>),

    #[error("ExecutionError error: ZKVM failed {0:?}")]
    ExecutionError(anyhow::Error),

    #[error("OtherError error: {0:?}")]
    OtherError(anyhow::Error),
}

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

#[allow(clippy::too_many_arguments)]
pub async fn run_proving_client<P, H>(
    proving: ProvingArgs,
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
    // preload all data natively into a hashmap
    let (_, execution_cache) = split_executions(stitched_executions.clone());
    info!(
        "Running vec witgen client with {} cached executions ({} traces).",
        execution_cache.len(),
        stitched_executions.len()
    );
    let (_, mut witness_vec): (ProofJournal, Witness<VecOracle>) = {
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
            proving.max_witness_size / 10,
            blob_provider,
            proving.payout_recipient_address.unwrap_or_default(),
            precondition_validation_data_hash,
            execution_cache.clone(),
            stitched_boot_info.clone(),
        )
        .await
        .expect("Failed to run vec witgen client.")
    };

    let execution_trace =
        core::mem::replace(&mut witness_vec.stitched_executions, stitched_executions);

    // check if we can prove this workload
    let (main_witness_size, witness_size) = sum_witness_size(&witness_vec);
    info!("Witness size: {witness_size} ({main_witness_size} main)");
    if witness_size > proving.max_witness_size {
        warn!(
            "Witness size {} exceeds limit {}.",
            witness_size, proving.max_witness_size
        );
        if !force_attempt {
            warn!("Aborting.");
            return Err(ProvingError::WitnessSizeError(
                witness_size,
                proving.max_witness_size,
                execution_trace,
            ));
        }
        warn!("Continuing..");
    }

    if !seek_proof {
        return Err(ProvingError::SeekProofError(witness_size, execution_trace));
    }

    let (preloaded_frames, streamed_frames) =
        encode_witness_frames(witness_vec).expect("Failed to encode VecOracle");
    seek_fpvm_proof(
        &proving,
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
    let shards = shard_witness_data(&mut preloaded_data[1..])?;
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
    let (witness_frames, _) =
        encode_witness_frames(witness.deep_clone()).expect("Failed to encode VecOracle");
    (
        witness_frames.first().map(|f| f.len()).unwrap(),
        witness_frames.iter().map(|f| f.len()).sum::<usize>(),
    )
}
pub async fn seek_fpvm_proof(
    proving: &ProvingArgs,
    witness_frames: Vec<Vec<u8>>,
    stitched_proofs: Vec<Receipt>,
    prove_snark: bool,
) -> Result<(), ProvingError> {
    // compute the zkvm proof
    let proof = if bonsai::should_use_bonsai() {
        bonsai::run_bonsai_client(witness_frames, stitched_proofs, prove_snark).await?
    } else {
        zkvm::run_zkvm_client(
            witness_frames,
            stitched_proofs,
            prove_snark,
            proving.segment_limit,
        )
        .await?
    };

    // Save proof file to disk
    save_proof_to_disk(&proof).await;

    Ok(())
}

pub async fn save_proof_to_disk(proof: &Receipt) {
    // Save proof file to disk
    let proof_journal =
        ProofJournal::decode_packed(proof.journal.as_ref()).expect("Failed to decode proof output");
    let mut output_file = File::create(proof::proof_file_name(&proof_journal))
        .await
        .expect("Failed to create proof output file");
    // Write proof data to file
    let proof_bytes = bincode::serialize(proof).expect("Could not serialize proof.");
    output_file
        .write_all(proof_bytes.as_slice())
        .await
        .expect("Failed to write proof to file");
    output_file
        .flush()
        .await
        .expect("Failed to flush proof output file data.");
}
