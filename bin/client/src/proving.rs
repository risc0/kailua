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
use crate::{boundless, proof, witgen, zkvm};
use alloy_primitives::{Address, B256};
use anyhow::Context;
use kailua_common::blobs::PreloadedBlobProvider;
use kailua_common::client::run_witness_client;
use kailua_common::journal::ProofJournal;
use kailua_common::oracle::map::MapOracle;
use kailua_common::oracle::vec::VecOracle;
use kailua_common::witness::Witness;
use kona_preimage::{HintWriterClient, PreimageOracleClient};
use kona_proof::l1::OracleBlobProvider;
use kona_proof::CachingOracle;
use std::fmt::Debug;
use std::sync::{Arc, Mutex};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tracing::{error, info};

/// The size of the LRU cache in the oracle.
pub const ORACLE_LRU_SIZE: usize = 1024;

pub async fn run_proving_client<P, H>(
    boundless: BoundlessArgs,
    oracle_client: P,
    hint_client: H,
    payout_recipient: Address,
    precondition_validation_data_hash: B256,
) -> anyhow::Result<()>
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
    let proof = match boundless.market {
        Some(args) => {
            boundless::run_boundless_client(args, boundless.storage, journal, witness_vec)
                .await
                .context("Failed to run boundless client.")?
        }
        None => zkvm::run_zkvm_client(witness_vec)
            .await
            .context("Failed to run zkvm client.")?,
    };
    // Prepare proof file
    let proof_journal = ProofJournal::decode_packed(proof.journal().as_ref())
        .expect("Failed to decode proof output");
    let mut output_file = File::create(proof::fpvm_proof_file_name(
        proof_journal.precondition_output,
        proof_journal.l1_head,
        proof_journal.claimed_l2_output_root,
        proof_journal.claimed_l2_block_number,
        proof_journal.agreed_l2_output_root,
    ))
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
