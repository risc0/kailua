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

use crate::blobs::PreloadedBlobProvider;
use crate::client::log;
use crate::journal::ProofJournal;
use crate::oracle::WitnessOracle;
use crate::witness::Witness;
use std::sync::Arc;

/// Executes a stateless client workflow by validating witness data, and running the stitching
/// client to produce a unified proof journal.
///
/// # Arguments
/// * `witness`: A `Witness<O>` object that contains all the input data required to execute the stateless client.
///
/// # Returns
/// * `ProofJournal`: The resulting proof journal from running the stitching client.
///
/// # Function Details
/// 1. Logs information about the number of "preimages" in the oracle witness.
/// 2. Validates the oracle witness's preimages through `validate_preimages`. If validation fails, the program will panic with an error message.
/// 3. Wraps the constructed oracle witness in an `Arc` for shared ownership and thread safety.
/// 4. Initializes a default stream witness of type `O` (provided by the generic parameter) and wraps it in an `Arc`.
/// 5. Logs information about the number of blobs in the blob witness.
/// 6. Constructs a `PreloadedBlobProvider` instance from the blob witness to manage the blobs.
/// 7. Executes the stitching client via `run_stitching_client`, which combines witness data, preconditions, headers,
///    and execution details. The result is a `ProofJournal` representing the proof output.
/// 8. Checks if any additional preimages have been discovered beyond what was initially provided, logging a warning if so.
///
/// # Panics
/// This function will panic if:
/// * The `validate_preimages` function call on the oracle witness fails, indicating invalid witness data.
///
/// # Logging
/// * Logs the count of preimages provided via the `oracle_witness`.
/// * Logs the count of blobs contained in the `blobs_witness`.
/// * Logs a warning if any extra preimages are found during execution.
pub fn run_stateless_client<O: WitnessOracle>(
    witness: Witness<O>,
    #[cfg(feature = "eigen-da")]
    eigen_da_witness: hokulea_proof::eigenda_blob_witness::EigenDABlobWitnessData,
) -> ProofJournal {
    log(&format!(
        "ORACLE: {} PREIMAGES",
        witness.oracle_witness.preimage_count()
    ));
    witness
        .oracle_witness
        .validate_preimages()
        .expect("Failed to validate preimages");
    let oracle = Arc::new(witness.oracle_witness);
    // ignore the provided stream witness if any
    let stream = Arc::new(O::default());
    log(&format!(
        "BEACON: {} BLOBS",
        witness.blobs_witness.blobs.len()
    ));
    let beacon = PreloadedBlobProvider::from(witness.blobs_witness);

    #[cfg(feature = "eigen-da")]
    let eigen_da_precondition = crate::eigen::da_witness_precondition(&eigen_da_witness);

    #[cfg(feature = "eigen-da")]
    let eigen_da =
        hokulea_proof::preloaded_eigenda_provider::PreloadedEigenDABlobProvider::from_witness(
            eigen_da_witness,
            crate::eigen::KailuaCanoeVerifier(witness.canoe_image_id.0),
        );

    let stitching_output = crate::client::stitching::run_stitching_client(
        witness.precondition_validation_data_hash,
        oracle.clone(),
        stream,
        beacon,
        #[cfg(feature = "eigen-da")]
        eigen_da,
        witness.fpvm_image_id,
        #[cfg(feature = "eigen-da")]
        witness.canoe_image_id,
        witness.payout_recipient_address,
        witness.stitched_executions,
        witness.stitched_boot_info,
    );

    if oracle.preimage_count() > 0 {
        log(&format!("EXTRA PREIMAGES: {}", oracle.preimage_count()));
    }

    #[cfg(feature = "eigen-da")]
    crate::eigen::da_witness_postcondition(eigen_da_precondition, stitching_output.0);

    stitching_output.1
}

#[cfg(test)]
#[cfg(not(feature = "eigen-da"))]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod tests {
    use super::*;
    use crate::client::core::tests::test_derivation;
    use crate::client::tests::TestOracle;
    use alloy_primitives::{b256, B256};
    use anyhow::Context;
    use kona_proof::BootInfo;

    #[test]
    fn test_stateless_client() -> anyhow::Result<()> {
        let mut boot_info = BootInfo {
            l1_head: b256!("0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"),
            agreed_l2_output_root: b256!(
                "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
            ),
            claimed_l2_output_root: b256!(
                "0x6984e5ae4d025562c8a571949b985692d80e364ddab46d5c8af5b36a20f611d1"
            ),
            claimed_l2_block_number: 16491349,
            chain_id: 11155420,
            rollup_config: Default::default(),
        };
        let stitched_executions = test_derivation(boot_info.clone(), None)
            .context("test_derivation")?
            .into_iter()
            .map(|e| e.as_ref().clone())
            .collect::<Vec<_>>();
        boot_info.l1_head = B256::ZERO;
        let oracle_witness = TestOracle::new(boot_info.clone());
        let stream_witness = oracle_witness.clone();
        let witness = Witness {
            oracle_witness,
            stream_witness,
            blobs_witness: Default::default(),
            payout_recipient_address: Default::default(),
            precondition_validation_data_hash: Default::default(),
            stitched_executions: vec![stitched_executions],
            stitched_boot_info: vec![],
            fpvm_image_id: Default::default(),
        };

        run_stateless_client(witness);

        Ok(())
    }
}
