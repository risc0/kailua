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

use crate::boundless::BoundlessArgs;
use crate::{bonsai, boundless, proof, witgen, zkvm};
use alloy_primitives::{Address, B256};
use anyhow::anyhow;
use kailua_common::blobs::PreloadedBlobProvider;
use kailua_common::client::run_witness_client;
use kailua_common::journal::ProofJournal;
use kailua_common::oracle::map::MapOracle;
use kailua_common::oracle::vec::VecOracle;
use kailua_common::proof::Proof;
use kailua_common::witness::{StitchedBootInfo, Witness};
use kona_preimage::{HintWriterClient, PreimageOracleClient};
use kona_proof::l1::OracleBlobProvider;
use kona_proof::CachingOracle;
use std::fmt::Debug;
use std::sync::{Arc, Mutex};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tracing::{error, info, warn};

/// The size of the LRU cache in the oracle.
pub const ORACLE_LRU_SIZE: usize = 1024;

#[derive(thiserror::Error, Debug)]
pub enum ProvingError {
    #[error("WitnessSizeError error: found {0} expected {0}")]
    WitnessSizeError(usize, usize),

    #[error("ExecutionError error: ZKVM failed {0:?}")]
    ExecutionError(anyhow::Error),

    #[error("OtherError error: {0:?}")]
    OtherError(anyhow::Error),
}

#[allow(clippy::too_many_arguments)]
pub async fn run_proving_client<P, H>(
    boundless: BoundlessArgs,
    oracle_client: P,
    hint_client: H,
    payout_recipient: Address,
    precondition_validation_data_hash: B256,
    stitched_boot_info: Vec<StitchedBootInfo>,
    stitched_proofs: Vec<Proof>,
    prove_snark: bool,
    force_attempt: bool,
    segment_limit: u32,
    max_witness_size: usize,
) -> Result<(), ProvingError>
where
    P: PreimageOracleClient + Send + Sync + Debug + Clone + 'static,
    H: HintWriterClient + Send + Sync + Debug + Clone + 'static,
{
    // preload all data natively into a hashmap
    info!("Running map witgen client.");
    let (journal, witness_map): (ProofJournal, Witness<MapOracle>) = {
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
            blob_provider,
            payout_recipient,
            precondition_validation_data_hash,
            stitched_boot_info.clone(),
        )
        .await
        .expect("Failed to run map witgen client.")
    };

    // unroll map witness into a vec witness
    info!("Running vec witgen client.");
    let (journal_map, witness_vec): (ProofJournal, Witness<VecOracle>) = witgen::run_witgen_client(
        Arc::new(witness_map.oracle_witness.clone()),
        PreloadedBlobProvider::from(witness_map.blobs_witness.clone()),
        payout_recipient,
        precondition_validation_data_hash,
        stitched_boot_info.clone(),
    )
    .await
    .expect("Failed to run vec witgen client.");
    if journal != journal_map {
        error!("Native journal does not match journal backed by map witness");
    }
    info!("Running vec witness client.");
    let cloned_witness_vec = {
        let mut cloned_with_arc = witness_vec.clone();
        cloned_with_arc.oracle_witness.preimages = Arc::new(Mutex::new(
            witness_vec.oracle_witness.preimages.lock().unwrap().clone(),
        ));
        cloned_with_arc
    };
    let journal_vec = run_witness_client(cloned_witness_vec);
    if journal != journal_vec {
        error!("Native journal does not match journal backed by vec witness");
    }

    // compute the receipt in the zkvm
    let witness_frame = rkyv::to_bytes::<rkyv::rancor::Error>(&witness_vec)
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?
        .to_vec();
    info!("Witness size: {}", witness_frame.len());
    if witness_frame.len() > max_witness_size {
        warn!("Witness too large.");
        if !force_attempt {
            warn!("Aborting.");
            return Err(ProvingError::WitnessSizeError(
                witness_frame.len(),
                max_witness_size,
            ));
        }
        warn!("Continuing..");
    }
    let proof = match boundless.market {
        Some(args) => {
            boundless::run_boundless_client(
                args,
                boundless.storage,
                journal,
                witness_frame,
                stitched_proofs,
                segment_limit,
            )
            .await?
        }
        None => {
            if bonsai::should_use_bonsai() {
                bonsai::run_bonsai_client(witness_frame, stitched_proofs, prove_snark).await?
            } else {
                zkvm::run_zkvm_client(witness_frame, stitched_proofs, prove_snark, segment_limit)
                    .await?
            }
        }
    };

    // Prepare proof file
    let proof_journal = ProofJournal::decode_packed(proof.journal().as_ref())
        .expect("Failed to decode proof output");
    let mut output_file = File::create(proof::proof_file_name(&proof_journal))
        .await
        .expect("Failed to create proof output file");
    // Write proof data to file
    let proof_bytes = bincode::serialize(&proof).expect("Could not serialize proof.");
    output_file
        .write_all(proof_bytes.as_slice())
        .await
        .expect("Failed to write proof to file");
    output_file
        .flush()
        .await
        .expect("Failed to flush proof output file data.");

    Ok(())
}
