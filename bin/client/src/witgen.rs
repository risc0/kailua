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

use crate::witness::{BlobWitnessProvider, OracleWitnessProvider};
use alloy_primitives::{Address, B256};
use kailua_build::KAILUA_FPVM_ID;
use kailua_common::blobs::BlobWitnessData;
use kailua_common::journal::ProofJournal;
use kailua_common::witness::{StitchedBootInfo, Witness, WitnessOracle};
use kona_derive::prelude::BlobProvider;
use kona_preimage::CommsClient;
use kona_proof::FlushableCache;
use std::fmt::Debug;
use std::ops::DerefMut;
use std::sync::{Arc, Mutex};
use tracing::info;

pub async fn run_witgen_client<P, B, O>(
    preimage_oracle: Arc<P>,
    blob_provider: B,
    payout_recipient: Address,
    precondition_validation_data_hash: B256,
    stitched_boot_info: Vec<StitchedBootInfo>,
) -> anyhow::Result<(ProofJournal, Witness<O>)>
where
    P: CommsClient + FlushableCache + Send + Sync + Debug + Clone,
    B: BlobProvider + Send + Sync + Debug + Clone,
    <B as BlobProvider>::Error: Debug,
    O: WitnessOracle + Send + Sync + Debug + Clone + Default,
{
    let oracle_witness = Arc::new(Mutex::new(O::default()));
    let blobs_witness = Arc::new(Mutex::new(BlobWitnessData::default()));
    info!("Preamble");
    let oracle = Arc::new(OracleWitnessProvider {
        oracle: preimage_oracle,
        witness: oracle_witness.clone(),
    });
    let beacon = BlobWitnessProvider {
        provider: blob_provider,
        witness: blobs_witness.clone(),
    };
    // Run client
    let (boot, precondition_hash, _) = kailua_common::client::run_kailua_client(
        precondition_validation_data_hash,
        oracle,
        beacon,
    )?;
    // Construct witness
    let fpvm_image_id = B256::from(bytemuck::cast::<_, [u8; 32]>(KAILUA_FPVM_ID));
    let mut witness = Witness {
        oracle_witness: core::mem::take(oracle_witness.lock().unwrap().deref_mut()),
        blobs_witness: core::mem::take(blobs_witness.lock().unwrap().deref_mut()),
        payout_recipient_address: payout_recipient,
        precondition_validation_data_hash,
        stitched_boot_info,
        fpvm_image_id,
    };
    witness.oracle_witness.finalize_preimages();
    let journal_output =
        ProofJournal::new(fpvm_image_id, payout_recipient, precondition_hash, &boot);
    Ok((journal_output, witness))
}
