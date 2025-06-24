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

use alloy::consensus::Blob;
use alloy::eips::eip4844::IndexedBlobHash;
use alloy_primitives::{Address, B256};
use async_trait::async_trait;
use kailua_build::KAILUA_FPVM_ID;
use kailua_common::blobs::BlobWitnessData;
use kailua_common::boot::StitchedBootInfo;
use kailua_common::executor::Execution;
use kailua_common::journal::ProofJournal;
use kailua_common::oracle::WitnessOracle;
use kailua_common::witness::Witness;
use kona_derive::prelude::BlobProvider;
use kona_preimage::errors::PreimageOracleResult;
use kona_preimage::{CommsClient, HintWriterClient, PreimageKey, PreimageOracleClient};
use kona_proof::FlushableCache;
use kona_protocol::BlockInfo;
use std::fmt::Debug;
use std::ops::DerefMut;
use std::sync::{Arc, Mutex};
use tracing::info;
use tracing::log::error;

pub async fn run_witgen_client<P, B, O>(
    preimage_oracle: Arc<P>,
    preimage_oracle_shard_size: usize,
    blob_provider: B,
    payout_recipient: Address,
    precondition_validation_data_hash: B256,
    execution_cache: Vec<Arc<Execution>>,
    stitched_boot_info: Vec<StitchedBootInfo>,
) -> anyhow::Result<(ProofJournal, Witness<O>)>
where
    P: CommsClient + FlushableCache + Send + Sync + Debug + Clone,
    B: BlobProvider + Send + Sync + Debug + Clone,
    <B as BlobProvider>::Error: Debug,
    O: WitnessOracle + Send + Sync + Debug + Clone + Default,
{
    let oracle_witness = Arc::new(Mutex::new(O::default()));
    let stream_witness = Arc::new(Mutex::new(O::default()));
    let blobs_witness = Arc::new(Mutex::new(BlobWitnessData::default()));
    info!("Preamble");
    let oracle = Arc::new(OracleWitnessProvider {
        oracle: preimage_oracle.clone(),
        witness: oracle_witness.clone(),
    });
    let stream = Arc::new(OracleWitnessProvider {
        oracle: preimage_oracle,
        witness: stream_witness.clone(),
    });
    let beacon = BlobWitnessProvider {
        provider: blob_provider,
        witness: blobs_witness.clone(),
    };
    // Run client
    let collection_target = Arc::new(Mutex::new(Vec::new()));
    let (boot, precondition_hash) = kailua_common::client::core::run_core_client(
        precondition_validation_data_hash,
        oracle,
        stream,
        beacon,
        execution_cache,
        Some(collection_target.clone()),
    )?;
    // Fix claimed output of captured executions
    // todo: use common::core::recover_collected_executions
    let mut executions = collection_target.lock().unwrap();
    for i in 1..executions.len() {
        executions[i - 1].claimed_output = executions[i].agreed_output;
    }
    if let Some(last_exec) = executions.last_mut() {
        last_exec.claimed_output = boot.claimed_l2_output_root;
    }
    let stitched_executions = vec![core::mem::take(executions.deref_mut())];
    // Construct witness
    let fpvm_image_id = B256::from(bytemuck::cast::<_, [u8; 32]>(KAILUA_FPVM_ID));
    let mut witness = Witness {
        oracle_witness: core::mem::take(oracle_witness.lock().unwrap().deref_mut()),
        stream_witness: core::mem::take(stream_witness.lock().unwrap().deref_mut()),
        blobs_witness: core::mem::take(blobs_witness.lock().unwrap().deref_mut()),
        payout_recipient_address: payout_recipient,
        precondition_validation_data_hash,
        stitched_executions,
        stitched_boot_info,
        fpvm_image_id,
    };
    witness
        .oracle_witness
        .finalize_preimages(preimage_oracle_shard_size, true);
    witness
        .stream_witness
        .finalize_preimages(preimage_oracle_shard_size, false);
    let journal_output =
        ProofJournal::new(fpvm_image_id, payout_recipient, precondition_hash, &boot);
    Ok((journal_output, witness))
}

#[derive(Clone, Debug)]
pub struct BlobWitnessProvider<T: BlobProvider> {
    pub provider: T,
    pub witness: Arc<Mutex<BlobWitnessData>>,
}

#[async_trait]
impl<T: BlobProvider + Send> BlobProvider for BlobWitnessProvider<T> {
    type Error = T::Error;

    async fn get_blobs(
        &mut self,
        block_ref: &BlockInfo,
        blob_hashes: &[IndexedBlobHash],
    ) -> Result<Vec<Box<Blob>>, Self::Error> {
        let blobs = self.provider.get_blobs(block_ref, blob_hashes).await?;
        let settings = alloy::consensus::EnvKzgSettings::default();
        for blob in &blobs {
            let c_kzg_blob = c_kzg::Blob::from_bytes(blob.as_slice()).unwrap();
            let commitment = settings
                .get()
                .blob_to_kzg_commitment(&c_kzg_blob)
                .expect("Failed to convert blob to commitment");
            let proof = settings
                .get()
                .compute_blob_kzg_proof(&c_kzg_blob, &commitment.to_bytes())
                .unwrap();
            let mut witness = self.witness.lock().unwrap();
            witness.blobs.push(Blob::from(*c_kzg_blob));
            witness.commitments.push(commitment.to_bytes());
            witness.proofs.push(proof.to_bytes());
        }
        Ok(blobs)
    }
}

#[derive(Clone, Debug)]
pub struct OracleWitnessProvider<
    P: CommsClient + FlushableCache + Send + Sync + Debug + Clone,
    O: WitnessOracle,
> {
    pub oracle: Arc<P>,
    pub witness: Arc<Mutex<O>>,
}

impl<P, O> OracleWitnessProvider<P, O>
where
    P: CommsClient + FlushableCache + Send + Sync + Debug + Clone,
    O: WitnessOracle,
{
    pub fn save(&self, key: PreimageKey, value: &[u8]) {
        self.witness
            .lock()
            .unwrap()
            .insert_preimage(key, value.to_vec());
    }
}

#[async_trait]
impl<P, O> PreimageOracleClient for OracleWitnessProvider<P, O>
where
    P: CommsClient + FlushableCache + Send + Sync + Debug + Clone,
    O: WitnessOracle,
{
    async fn get(&self, key: PreimageKey) -> PreimageOracleResult<Vec<u8>> {
        match self.oracle.get(key).await {
            Ok(value) => {
                self.save(key, &value);
                Ok(value)
            }
            Err(e) => {
                error!(
                    "OracleWitnessProvider failed to get value for key {:?}/{}: {:?}",
                    key.key_type(),
                    key.key_value(),
                    e
                );
                Err(e)
            }
        }
    }

    async fn get_exact(&self, key: PreimageKey, buf: &mut [u8]) -> PreimageOracleResult<()> {
        match self.oracle.get_exact(key, buf).await {
            Ok(_) => {
                self.save(key, buf);
                Ok(())
            }
            Err(e) => {
                error!(
                    "OracleWitnessProvider failed to get exact value for key {:?}/{}: {:?}",
                    key.key_type(),
                    key.key_value(),
                    e
                );
                Err(e)
            }
        }
    }
}

#[async_trait]
impl<P, O> HintWriterClient for OracleWitnessProvider<P, O>
where
    P: CommsClient + FlushableCache + Send + Sync + Debug + Clone,
    O: WitnessOracle,
{
    async fn write(&self, hint: &str) -> PreimageOracleResult<()> {
        self.oracle.write(hint).await
    }
}

impl<P, O> FlushableCache for OracleWitnessProvider<P, O>
where
    P: CommsClient + FlushableCache + Send + Sync + Debug + Clone,
    O: WitnessOracle,
{
    fn flush(&self) {
        self.oracle.flush();
    }
}
