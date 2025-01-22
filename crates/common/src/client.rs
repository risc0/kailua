// Copyright 2024 RISC Zero, Inc.
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

use crate::blobs::PreloadedBlobProvider;
use crate::journal::ProofJournal;
use crate::precondition;
use crate::witness::{StitchedBootInfo, Witness, WitnessOracle};
use alloy_primitives::{Address, Sealed, B256};
use anyhow::{bail, Context};
use kona_derive::traits::BlobProvider;
use kona_driver::Driver;
use kona_executor::TrieDBProvider;
use kona_preimage::{CommsClient, PreimageKeyType};
use kona_proof::errors::OracleProviderError;
use kona_proof::executor::KonaExecutor;
use kona_proof::l1::{OracleL1ChainProvider, OraclePipeline};
use kona_proof::l2::OracleL2ChainProvider;
use kona_proof::sync::new_pipeline_cursor;
use kona_proof::{BootInfo, FlushableCache, HintType};
use std::fmt::Debug;
use std::sync::Arc;
use tracing::log::warn;

/// Executes the Kona client to compute a list of subsequent outputs. Additionally validates
/// the Kailua Fault/Validity preconditions on proposals for proof generation.
pub fn run_kailua_client<
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
>(
    precondition_validation_data_hash: B256,
    oracle: Arc<O>,
    mut beacon: B,
) -> anyhow::Result<(Arc<BootInfo>, B256, Option<B256>)>
where
    <B as BlobProvider>::Error: Debug,
{
    let (boot, precondition_hash, output_hash) = kona_proof::block_on(async move {
        ////////////////////////////////////////////////////////////////
        //                          PROLOGUE                          //
        ////////////////////////////////////////////////////////////////
        log("BOOT");
        let boot = Arc::new(
            BootInfo::load(oracle.as_ref())
                .await
                .context("BootInfo::load")?,
        );

        log("PRECONDITION");
        let precondition_data = precondition::load_precondition_data(
            precondition_validation_data_hash,
            oracle.clone(),
            &mut beacon,
        )
        .await?;

        let safe_head_hash = fetch_safe_head_hash(oracle.as_ref(), boot.as_ref()).await?;

        let mut l1_provider = OracleL1ChainProvider::new(boot.l1_head, oracle.clone());
        let mut l2_provider =
            OracleL2ChainProvider::new(safe_head_hash, boot.rollup_config.clone(), oracle.clone());

        // If the claimed L2 block number is less than or equal to the safe head of the L2 chain,
        // the claim is invalid.
        // Fetch the safe head's block header.
        let safe_head = l2_provider
            .header_by_hash(safe_head_hash)
            .map(|header| Sealed::new_unchecked(header, safe_head_hash))?;

        if boot.claimed_l2_block_number < safe_head.number {
            bail!("Invalid claim");
        }
        let safe_head_number = safe_head.number;

        ////////////////////////////////////////////////////////////////
        //                   DERIVATION & EXECUTION                   //
        ////////////////////////////////////////////////////////////////
        log("DERIVATION & EXECUTION");
        // Create a new derivation driver with the given boot information and oracle.
        let cursor = new_pipeline_cursor(
            &boot.rollup_config,
            safe_head,
            &mut l1_provider,
            &mut l2_provider,
        )
        .await?;
        l2_provider.set_cursor(cursor.clone());

        let cfg = Arc::new(boot.rollup_config.clone());
        let pipeline = OraclePipeline::new(
            cfg.clone(),
            cursor.clone(),
            oracle.clone(),
            beacon,
            l1_provider.clone(),
            l2_provider.clone(),
        );
        let executor =
            KonaExecutor::new(&cfg, l2_provider.clone(), l2_provider.clone(), None, None);
        let mut driver = Driver::new(cursor, executor, pipeline);

        // Run the derivation pipeline until we are able to produce the output root of the claimed
        // L2 block.
        let expected_output_count = (boot.claimed_l2_block_number - safe_head_number) as usize;
        let mut output_roots = Vec::with_capacity(expected_output_count);
        for starting_block in safe_head_number..boot.claimed_l2_block_number {
            // Advance to the next target
            let (output_number, _, output_root) = driver
                .advance_to_target(&boot.rollup_config, Some(starting_block + 1))
                .await?;
            // Stop if nothing new was derived
            if output_number == starting_block {
                // A mismatch indicates that there is insufficient L1 data available to produce
                // an L2 output root at the claimed block number
                log("HALT");
                break;
            } else {
                log(&format!(
                    "OUTPUT: {output_number}/{}",
                    boot.claimed_l2_block_number
                ));
            }
            // Append newly computed output root
            output_roots.push(output_root);
        }

        ////////////////////////////////////////////////////////////////
        //                          EPILOGUE                          //
        ////////////////////////////////////////////////////////////////
        log("EPILOGUE");

        let precondition_hash = precondition_data
            .map(|(precondition_validation_data, blobs)| {
                precondition::validate_precondition(
                    precondition_validation_data,
                    blobs,
                    safe_head_number,
                    &output_roots,
                )
            })
            .unwrap_or(Ok(B256::ZERO))?;

        if output_roots.len() != expected_output_count {
            // Not enough data to derive output root at claimed height
            Ok((boot, precondition_hash, None))
        } else if output_roots.is_empty() {
            // Claimed output height is equal to agreed output height
            let real_output_hash = boot.agreed_l2_output_root;
            Ok((boot, precondition_hash, Some(real_output_hash)))
        } else {
            // Derived output root at future height
            Ok((boot, precondition_hash, output_roots.pop()))
        }
    })?;

    // Check output
    if let Some(computed_output) = output_hash {
        // With sufficient data, the input l2_claim must be true
        assert_eq!(boot.claimed_l2_output_root, computed_output);
    } else {
        // We use the zero claim hash to denote that the data as of l1 head is insufficient
        assert_eq!(boot.claimed_l2_output_root, B256::ZERO);
    }

    Ok((boot, precondition_hash, output_hash))
}

/// Fetches the safe head hash of the L2 chain based on the agreed upon L2 output root in the
/// [BootInfo].
pub async fn fetch_safe_head_hash<O>(
    caching_oracle: &O,
    boot_info: &BootInfo,
) -> Result<B256, OracleProviderError>
where
    O: CommsClient,
{
    let mut output_preimage = [0u8; 128];
    HintType::StartingL2Output
        .get_exact_preimage(
            caching_oracle,
            boot_info.agreed_l2_output_root,
            PreimageKeyType::Keccak256,
            &mut output_preimage,
        )
        .await?;

    output_preimage[96..128]
        .try_into()
        .map_err(OracleProviderError::SliceConversion)
}

pub fn log(msg: &str) {
    #[cfg(target_os = "zkvm")]
    risc0_zkvm::guest::env::log(msg);
    #[cfg(not(target_os = "zkvm"))]
    tracing::info!("{msg}");
}

pub fn run_witness_client<O: WitnessOracle>(witness: Witness<O>) -> ProofJournal {
    log(&format!(
        "ORACLE: {} PREIMAGES",
        witness.oracle_witness.preimage_count()
    ));
    witness
        .oracle_witness
        .validate_preimages()
        .expect("Failed to validate preimages");
    let oracle = Arc::new(witness.oracle_witness);
    log(&format!(
        "BEACON: {} BLOBS",
        witness.blobs_witness.blobs.len()
    ));
    let beacon = PreloadedBlobProvider::from(witness.blobs_witness);

    let proof_journal = run_stitching_client(
        witness.precondition_validation_data_hash,
        oracle.clone(),
        beacon,
        witness.fpvm_image_id,
        witness.payout_recipient_address,
        witness.stitched_boot_info,
    );

    if oracle.preimage_count() > 0 {
        warn!(
            "Found {} extra preimages in witness",
            oracle.preimage_count()
        );
    }

    proof_journal
}

pub fn run_stitching_client<
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
>(
    precondition_validation_data_hash: B256,
    oracle: Arc<O>,
    beacon: B,
    fpvm_image_id: B256,
    payout_recipient_address: Address,
    stitched_boot_info: Vec<StitchedBootInfo>,
) -> ProofJournal
where
    <B as BlobProvider>::Error: Debug,
{
    // Attempt to recompute the output hash at the target block number using kona
    log("RUN");
    let (boot, precondition_hash, _) =
        run_kailua_client(precondition_validation_data_hash, oracle.clone(), beacon)
            .expect("Failed to compute output hash.");

    // Stitch boots together into a journal
    let mut stitched_journal = ProofJournal::new(
        fpvm_image_id,
        payout_recipient_address,
        precondition_hash,
        boot.as_ref(),
    );
    // Verify proofs recursively for boundless composition
    #[cfg(target_os = "zkvm")]
    let proven_fpvm_journals = {
        use crate::config::SET_BUILDER_ID;
        use crate::proof::Proof;
        use alloy_primitives::map::HashSet;
        use risc0_zkvm::serde::Deserializer;
        use risc0_zkvm::sha::{Digest, Digestible};
        use risc0_zkvm::{Groth16ReceiptVerifierParameters, MaybePruned, ReceiptClaim};
        use serde::Deserialize;

        let fpvm_image_id = Digest::from(fpvm_image_id.0);
        let mut proven_fpvm_journals = HashSet::new();
        let mut verifying_params: Option<Digest> = None;

        loop {
            let Ok(proof) =
                Proof::deserialize(&mut Deserializer::new(risc0_zkvm::guest::env::stdin()))
            else {
                log(&format!("PROOFS {}", proven_fpvm_journals.len()));
                break;
            };

            let journal_digest = proof.journal().digest();
            log(&format!("VERIFY {journal_digest}"));

            match proof {
                Proof::ZKVMReceipt(receipt) => {
                    receipt
                        .verify(fpvm_image_id)
                        .expect("Failed to verify receipt for {journal_digest}.");
                }
                Proof::BoundlessSeal(..) => {
                    unimplemented!("Convert BoundlessSeal to SetBuilderReceipt");
                }
                Proof::SetBuilderReceipt(receipt, set_builder_siblings, journal) => {
                    // Support only proofs with default verifier params
                    assert_eq!(
                        &receipt.verifier_parameters,
                        verifying_params.get_or_insert_with(|| {
                            Groth16ReceiptVerifierParameters::default().digest()
                        })
                    );
                    // build the claim for the fpvm
                    let fpvm_claim_digest =
                        ReceiptClaim::ok(fpvm_image_id, MaybePruned::Pruned(journal.digest()))
                            .digest();
                    // construct set builder root from merkle proof
                    let set_builder_journal = crate::proof::encoded_set_builder_journal(
                        &fpvm_claim_digest,
                        set_builder_siblings,
                        fpvm_image_id,
                    );
                    // Verify set builder claim digest equivalence
                    assert_eq!(
                        receipt.claim.digest(),
                        ReceiptClaim::ok(
                            SET_BUILDER_ID.0,
                            MaybePruned::Pruned(set_builder_journal.digest()),
                        )
                        .digest()
                    );
                    // Verify set builder receipt validity
                    receipt.verify_integrity().expect(&format!(
                        "Failed to verify Groth16Receipt for {journal_digest}."
                    ));
                }
            }

            proven_fpvm_journals.insert(journal_digest);
        }

        proven_fpvm_journals
    };
    for stitched_boot in stitched_boot_info {
        // Require equivalence in reference head
        assert_eq!(stitched_boot.l1_head, stitched_journal.l1_head);
        // Require progress in stitched boot
        assert_ne!(
            stitched_boot.agreed_l2_output_root,
            stitched_boot.claimed_l2_output_root
        );
        // Require proof assumption
        #[cfg(target_os = "zkvm")]
        {
            use risc0_zkvm::sha::Digestible;

            let proof_journal = ProofJournal::new_stitched(
                fpvm_image_id,
                payout_recipient_address,
                precondition_hash,
                stitched_journal.config_hash,
                &stitched_boot,
            )
            .encode_packed();
            let journal_digest = proof_journal.digest();
            if proven_fpvm_journals.contains(&journal_digest) {
                log(&format!("FOUND {journal_digest}"));
            } else {
                log(&format!("ASSUME {journal_digest}"));
                risc0_zkvm::guest::env::verify(fpvm_image_id.0, &proof_journal)
                    .expect("Failed to verify stitched boot assumption");
            }
        }
        // Require continuity
        if stitched_boot.claimed_l2_output_root == stitched_journal.agreed_l2_output_root {
            // Backward stitch
            stitched_journal.agreed_l2_output_root = stitched_boot.agreed_l2_output_root;
        } else if stitched_boot.agreed_l2_output_root == stitched_journal.claimed_l2_output_root {
            // Forward stitch
            stitched_journal.claimed_l2_output_root = stitched_boot.claimed_l2_output_root;
            stitched_journal.claimed_l2_block_number = stitched_boot.claimed_l2_block_number;
        } else {
            unimplemented!("No support for non-contiguous stitching.");
        }
    }

    stitched_journal
}
