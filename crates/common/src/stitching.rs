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

use crate::client;
use crate::journal::ProofJournal;
use crate::witness::StitchedBootInfo;
use alloy_primitives::{Address, B256};
use kona_derive::prelude::BlobProvider;
use kona_preimage::CommsClient;
use kona_proof::{BootInfo, FlushableCache};
use std::fmt::Debug;
use std::sync::Arc;

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
    client::log("RUN");
    let (boot, precondition_hash, _) =
        client::run_kailua_client(precondition_validation_data_hash, oracle.clone(), beacon)
            .expect("Failed to compute output hash.");

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
                client::log(&format!("PROOFS {}", proven_fpvm_journals.len()));
                break;
            };

            let journal_digest = proof.journal().digest();
            client::log(&format!("VERIFY {journal_digest}"));

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

    stitch_boot_info(
        boot.as_ref(),
        fpvm_image_id,
        payout_recipient_address,
        precondition_hash,
        stitched_boot_info,
        #[cfg(target_os = "zkvm")]
        proven_fpvm_journals,
    )
}

pub fn stitch_boot_info(
    boot: &BootInfo,
    fpvm_image_id: B256,
    payout_recipient_address: Address,
    precondition_hash: B256,
    stitched_boot_info: Vec<StitchedBootInfo>,
    #[cfg(target_os = "zkvm")] proven_fpvm_journals: alloy_primitives::map::HashSet<
        risc0_zkvm::sha::Digest,
    >,
) -> ProofJournal {
    // Stitch boots together into a journal
    let mut stitched_journal = ProofJournal::new(
        fpvm_image_id,
        payout_recipient_address,
        precondition_hash,
        boot,
    );

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
                client::log(&format!("FOUND {journal_digest}"));
            } else {
                client::log(&format!("ASSUME {journal_digest}"));
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
