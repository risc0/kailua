use crate::client::witgen;
use alloy_primitives::{Address, B256};
use kailua_kona::boot::StitchedBootInfo;
use kailua_kona::executor::Execution;
use kailua_kona::journal::ProofJournal;
use kailua_kona::oracle::WitnessOracle;
use kailua_kona::witness::Witness;
use kona_derive::prelude::BlobProvider;
use kona_preimage::CommsClient;
use kona_proof::FlushableCache;
use std::fmt::Debug;
use std::ops::DerefMut;
use std::sync::{Arc, Mutex};

pub async fn run_hokulea_witgen_client<P, B, O>(
    preimage_oracle: Arc<P>,
    preimage_oracle_shard_size: usize,
    blob_provider: B,
    payout_recipient: Address,
    precondition_validation_data_hash: B256,
    execution_cache: Vec<Arc<Execution>>,
    stitched_boot_info: Vec<StitchedBootInfo>,
) -> anyhow::Result<(
    ProofJournal,
    Witness<O>,
    hokulea_proof::eigenda_blob_witness::EigenDABlobWitnessData,
)>
where
    P: CommsClient + FlushableCache + Send + Sync + Debug + Clone,
    B: BlobProvider + Send + Sync + Debug + Clone,
    <B as BlobProvider>::Error: Debug,
    O: WitnessOracle + Send + Sync + Debug + Clone + Default,
{
    // Create witness target
    let eigen_witness = Arc::new(Mutex::new(Default::default()));
    // Create provider around witness
    let eigen = kailua_hokulea::da::EigenDADataSourceProvider(
        hokulea_witgen::witness_provider::OracleEigenDAWitnessProvider {
            provider: hokulea_proof::eigenda_provider::OracleEigenDAProvider::new(
                preimage_oracle.clone(),
            ),
            witness: eigen_witness.clone(),
        },
    );
    // Run regular witgen client
    let (boot, mut proof_journal, mut witness) = witgen::run_witgen_client(
        preimage_oracle,
        preimage_oracle_shard_size,
        blob_provider,
        eigen,
        payout_recipient,
        precondition_validation_data_hash,
        execution_cache,
        stitched_boot_info,
    )
    .await?;
    // Set expected values
    let mut eigen_witness = core::mem::take(eigen_witness.lock().unwrap().deref_mut());
    for (_, validity) in &mut eigen_witness.validity {
        validity.l1_head_block_hash = boot.l1_head;
        validity.l1_chain_id = boot.rollup_config.l1_chain_id;
    }
    for (_, recency) in &mut eigen_witness.recency {
        *recency = boot.rollup_config.seq_window_size;
    }
    proof_journal.fpvm_image_id = B256::from(bytemuck::cast::<_, [u8; 32]>(
        kailua_build::KAILUA_FPVM_HOKULEA_ID,
    ));
    witness.fpvm_image_id = proof_journal.fpvm_image_id;
    // Return extended result
    Ok((proof_journal, witness, eigen_witness))
}
