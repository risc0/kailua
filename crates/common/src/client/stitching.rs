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

use crate::boot::StitchedBootInfo;
use crate::client::log;
use crate::executor::Execution;
use crate::journal::ProofJournal;
use alloy_primitives::map::HashSet;
use alloy_primitives::{Address, B256};
use kona_derive::prelude::BlobProvider;
use kona_preimage::CommsClient;
use kona_proof::{BootInfo, FlushableCache};
use risc0_zkvm::serde::Deserializer;
use risc0_zkvm::sha::{Digest, Digestible};
use risc0_zkvm::Receipt;
use serde::Deserialize;
use std::fmt::Debug;
use std::sync::Arc;

/// A data structure representing stitched information, including executions,
/// boot information, and proofs, intended for caching, combining, and validation.
///
/// # Fields
///
/// * `stitched_executions (Vec<Vec<Execution>>)`
///   - A collection of execution traces, organized into groups, which serve as a cache
///     of recorded execution sequences. Each `Vec<Execution>` represents a single group
///     of executions.
///
/// * `stitched_boot_info (Vec<StitchedBootInfo>)`
///   - A list of continuous bootstrap information objects, which are combined
///     to support a unified bootstrapping record.
///
/// * `stitched_proofs (Vec<Receipt>)`
///   - A collection of receipt objects used for validation.
///     This field is marked with `rkyv::with::Skip` to avoid including it
///     during serialization or deserialization when using the `rkyv` library.
///
/// # Usage
///
/// This struct can be used in scenarios where execution traces and boot information
/// need to be aggregated and later validated using proofs. Its efficient structure
/// and support for serialization/deserialization facilitates its use in contexts
/// requiring storage, transport, and archival.
#[derive(Clone, Debug, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct StitchedData {
    /// Collection of execution traces to use as a cache
    pub stitched_executions: Vec<Vec<Execution>>,
    /// List of contiguous bootstrap information to combine
    pub stitched_boot_info: Vec<StitchedBootInfo>,
    /// Collection of receipts to validate
    #[rkyv(with = rkyv::with::Skip)]
    pub stitched_proofs: Vec<Receipt>,
}

/// Executes the primary operation of stitching together execution and boot information for a client,
/// while maintaining composable proofs for validation in a zero-knowledge environment.
///
/// # Type Parameters
///
/// * `O`: Generic type parameter representing an Oracle that implements the `CommsClient`,
///    `FlushableCache`, `Send`, `Sync`, and `Debug` traits.
/// * `B`: Generic type parameter representing a BlobProvider that implements the `Send`, `Sync`,
///    `Debug`, and `Clone` traits.
///
/// # Arguments
///
/// * `precondition_validation_data_hash` - A `B256` hash used for precondition validation.
/// * `oracle` - An `Arc` wrapped client that implements the `CommsClient` and `FlushableCache`
///    traits. This serves as the provider for external data communication.
/// * `stream` - An `Arc` wrapped client, similar to `oracle`, used for additional communication
///    and streaming purposes.
/// * `beacon` - A generic blob provider `B`, used as a shared dependency for validation
///    operations.
/// * `fpvm_image_id` - A `B256` identifier for the FPVM image to associate with the operations performed.
/// * `payout_recipient_address` - The Ethereum address (`Address`) where payout rewards are allocated.
/// * `stitched_executions` - A nested vector of `Execution` objects containing precomputed execution
///    proofs to be stitched.
/// * `stitched_boot_info` - A vector of `StitchedBootInfo` objects containing boot proofs
///    to be stitched together.
///
/// # Returns
///
/// Returns a `ProofJournal` combining the stitched proofs.
///
/// # Functionality
///
/// - **Execution Queueing:** Precomputed executions are split into direct executables and cache components
///   for intermediate processing.
/// - **Output Validation:** Computes the output hash of the target block using a helper method
///   (`run_kailua_client`) and validates the precondition against the provided hash.
/// - **Proof Loading (Conditional):** For zero-knowledge validations (`zkvm`), loads previously
///   proven FPVM journals to maintain composability and recursive proof validation.
/// - **Execution Stitching:** Merges the precomputed execution proofs into a single verifiable
///   entity while associating it with a target address.
/// - **Boot Info Stitching:** Stitches together boot proofs based on the precondition hash and FPVM image ID.
///
/// # Platform Specific Behavior
///
/// This function behaves differently on platforms supporting `zkvm`:
/// - It loads proven FPVM journals (`load_stitching_journals`) to ensure recursive zero-knowledge proofs
///   are intact.
/// - Passes the proven journals to the execution and boot info stitching processes for extended validation.
///
/// # Panics
///
/// This function will panic if:
/// - The output hash computation (`run_kailua_client`) fails.
///
/// # Dependencies
///
/// This operation relies on external functions such as:
/// - `split_executions` for precomputing execution splits.
/// - `run_kailua_client` for output hash computation.
/// - `load_stitching_journals` (conditionally) for loading proven journals.
/// - `stitch_executions` and `stitch_boot_info` for stitching different components of proof data.
///
/// # Notes
///
/// - The function is marked with `#[allow(clippy::too_many_arguments)]` due to the high number of input
///   parameters required for its operations.
/// - Supports compositional proof validation for complex systems such as zero-knowledge proofs and recursive
///   proof aggregation.
#[allow(clippy::too_many_arguments)]
pub fn run_stitching_client<
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
>(
    precondition_validation_data_hash: B256,
    oracle: Arc<O>,
    stream: Arc<O>,
    beacon: B,
    fpvm_image_id: B256,
    payout_recipient_address: Address,
    stitched_executions: Vec<Vec<Execution>>,
    stitched_boot_info: Vec<StitchedBootInfo>,
) -> ProofJournal
where
    <B as BlobProvider>::Error: Debug,
{
    // Queue up precomputed executions
    let (stitched_executions, execution_cache) = split_executions(stitched_executions);

    // Attempt to recompute the output hash at the target block number using kona
    log("RUN");
    let (boot, precondition_hash) = crate::client::run_kailua_client(
        precondition_validation_data_hash,
        oracle,
        stream,
        beacon,
        execution_cache,
        None,
    )
    .expect("Failed to compute output hash.");

    // Verify proofs recursively for boundless composition
    #[cfg(target_os = "zkvm")]
    let proven_fpvm_journals = load_stitching_journals(fpvm_image_id);

    // Stitch recursively composed execution-only proofs
    stitch_executions(
        &boot,
        fpvm_image_id,
        payout_recipient_address,
        &stitched_executions,
        #[cfg(target_os = "zkvm")]
        &proven_fpvm_journals,
    );

    // Stitch recursively composed proofs
    stitch_boot_info(
        &boot,
        fpvm_image_id,
        payout_recipient_address,
        precondition_hash,
        stitched_boot_info,
        #[cfg(target_os = "zkvm")]
        &proven_fpvm_journals,
    )
}

/// Loads and verifies stitching journals for a given FPVM image.
///
/// This function continuously reads receipts representing the proofs of computations from the
/// standard input (stdin). Each receipt is validated against the provided `fpvm_image_id`,
/// representing the image digest of the FPVM. Validated receipts' journal digests are stored
/// in a `HashSet` ensuring uniqueness. If deserialization of the receipt fails, the function
/// terminates and returns the set of proven journal digests.
///
/// # Parameters
/// - `fpvm_image_id`: A `B256` type identifier representing the hashed image ID of the FPVM.
///
/// # Returns
/// - A `HashSet<Digest>` containing the unique journal digests of all verified receipts.
///
/// # Behavior
/// 1. Converts the `fpvm_image_id` into a `Digest` for verification purposes.
/// 2. Reads receipts in a loop from the standard input until an `Err` occurs during deserialization.
///    - While reading receipts:
///      - Logs the verification process.
///      - Deserializes and verifies receipts against the provided `fpvm_image_id`.
///      - Inserts successfully verified journal digests into the `HashSet`.
/// 3. Logs the total number of successfully verified journal digests and exits with the result.
///
/// # Panics
/// Panics if:
/// - Receipt verification fails, indicating an invalid or tampered proof. The panic message will
///   include which journal digest's verification failed.
///
/// # Logging
/// - Logs "VERIFY" at the start of the method.
/// - Logs "VERIFY {journal_digest}" after calculating journal digests.
/// - Logs "PROOFS {count}" denoting the number of proven journal digests before exiting.
///
/// # Notes
/// - The `Receipt::deserialize` and `risc0_zkvm::guest::env::stdin` are used to process input
///   receipts.
/// - This function is designed for environments where proofs generated externally are verified
///   within the FPVM.
pub fn load_stitching_journals(fpvm_image_id: B256) -> HashSet<Digest> {
    log("VERIFY");

    let fpvm_image_id = Digest::from(fpvm_image_id.0);
    let mut proven_fpvm_journals = HashSet::new();

    loop {
        let Ok(receipt) =
            Receipt::deserialize(&mut Deserializer::new(risc0_zkvm::guest::env::stdin()))
        else {
            log(&format!("PROOFS {}", proven_fpvm_journals.len()));
            break proven_fpvm_journals;
        };

        let journal_digest = receipt.journal.digest();
        log(&format!("VERIFY {journal_digest}"));

        // Validate RISC Zero receipts natively
        receipt
            .verify(fpvm_image_id)
            .expect("Failed to verify receipt for {journal_digest}.");

        proven_fpvm_journals.insert(journal_digest);
    }
}

/// Verifies the stitching journal of an FPVM image.
///
/// This function checks the validity of a journal based on its digest and the existing
/// set of proven FPVM journal digests. The behavior of this function depends on the
/// target OS being `zkvm`. If the journal's digest exists in the set of verified digests,
/// it logs that the digest was found. Otherwise, it assumes the journal and attempts to
/// verify it using the RISC Zero ZKVM environment.
///
/// # Parameters
/// - `_fpvm_image_id`: The ID of the FPVM image represented as a `B256` hash. This
///   ID is used during the journal verification process.
/// - `_proof_journal`: The serialized proof journal as a `Vec<u8>`. It serves as
///   the data to be verified.
/// - `proven_fpvm_journals`: A reference to a `HashSet` of digests (of type `Digest`)
///   containing the previously verified journals. This parameter is only used when
///   the target OS is `zkvm`.
///
/// # Logs
/// - Logs a message indicating whether the given journal digest was "FOUND" in the proven
///   set or "ASSUME" if it is not present.
///
/// # Panics
/// - If the verification process fails (i.e., the journal does not match the
///   expected criteria for verification), the function will panic with the message:
///   `"Failed to verify stitched journal assumption"`.
///
/// # Notes
/// - This function contains conditional compilation code that is only executed when
///   the target operating system is `zkvm`. If the target OS is not `zkvm`, the function
///   does nothing.
pub fn verify_stitching_journal(
    _fpvm_image_id: B256,
    _proof_journal: Vec<u8>,
    #[cfg(target_os = "zkvm")] proven_fpvm_journals: &HashSet<Digest>,
) {
    #[cfg(target_os = "zkvm")]
    {
        let journal_digest = _proof_journal.digest();
        if proven_fpvm_journals.contains(&journal_digest) {
            crate::client::log(&format!("FOUND {journal_digest}"));
        } else {
            crate::client::log(&format!("ASSUME {journal_digest}"));
            risc0_zkvm::guest::env::verify(_fpvm_image_id.0, &_proof_journal)
                .expect("Failed to verify stitched journal assumption");
        }
    }
}

/// Splits a provided two-dimensional vector of `Execution` objects into two separate structures:
/// - A nested two-dimensional vector where each inner `Execution` is wrapped in an `Arc`.
/// - A flattened vector containing all the `Execution` objects, each wrapped in an `Arc`.
///
/// This function is useful for scenarios where you want to maintain the original structure
/// but also need a separate flattened cache to quickly access all `Execution` objects.
///
/// # Arguments
///
/// * `stitched_executions` - A two-dimensional vector of `Execution` objects (`Vec<Vec<Execution>>`)
///   representing grouped and stitched executions.
///
/// # Returns
///
/// A tuple containing:
/// 1. A two-dimensional vector (`Vec<Vec<Arc<Execution>>>`) where each `Execution` is wrapped in an `Arc`.
/// 2. A flattened vector (`Vec<Arc<Execution>>`) representing a cache of all `Execution` objects.
///
/// # Notes
///
/// * The use of `Arc` (atomic reference counting) ensures shared ownership of `Execution` objects,
///   which is especially useful in concurrent contexts where multiple threads may need to access
///   the same `Execution` objects.
///
/// * The function ensures that all `Execution` objects are wrapped in an `Arc` and cloned
///   so that both the grouped and flattened structures can be used independently.
pub fn split_executions(
    stitched_executions: Vec<Vec<Execution>>,
) -> (Vec<Vec<Arc<Execution>>>, Vec<Arc<Execution>>) {
    let stitched_executions = stitched_executions
        .into_iter()
        .map(|trace| trace.into_iter().map(Arc::new).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let execution_cache = stitched_executions
        .iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    (stitched_executions, execution_cache)
}

/// Stitches a collection of execution traces into a cohesive proof journal and validates the results.
/// This function ensures the integrity of execution traces and their compliance with the rollup configuration.
///
/// # Parameters
/// - `boot`: A reference to the `BootInfo` structure containing the rollup's configuration and state information.
/// - `fpvm_image_id`: The unique identifier of the FPVM (Fault-Proof Virtual Machine) image being used for proofs.
/// - `payout_recipient_address`: The address to receive the payout as a result of the execution.
/// - `stitched_executions`: A reference to a vector of vectors containing execution traces. Each inner vector represents
///     a sequence of linked execution steps (`Execution` objects).
/// - `proven_fpvm_journals` (*conditional*): A reference to a set of `Digest` values representing proven
///     journals from the FPVM. Only available when compiled for `zkvm` target (`#[cfg(target_os = "zkvm")]`).
///
/// # Behavior
/// - When the `boot.l1_head` is zero, it represents a special case where only one batch of execution is validated
///   by the Kailua client. If more than one batch is found, the function panics.
/// - Validates the `receipts_root` of each execution in all traces by comparing it with the computed root value
///   based on the execution result, rollup configuration, and payload attributes' timestamp.
/// - Constructs an expected proof journal for each execution trace, which includes precondition and configuration
///   hashes, and other state values derived from the execution trace (e.g., output roots and block numbers).
/// - When the system is targeting `zkvm`, the proof journal is verified using the `proven_fpvm_journals`.
///
/// # Panics
/// - When `boot.l1_head` is zero but the number of `stitched_executions` exceeds 1.
/// - When an execution trace is empty (used in `.first()` or `.last()` calls without valid elements).
///
/// # Notes
/// - This is a low-level function that performs key validation and proof journal creation for a batch of execution
///   traces. It relies on multiple helpers like `config_hash`, `exec_precondition_hash`, `compute_receipts_root`,
///   and `verify_stitching_journal`.
/// - This function ensures that transitions between executions are provable and consistent.
pub fn stitch_executions(
    boot: &BootInfo,
    fpvm_image_id: B256,
    payout_recipient_address: Address,
    stitched_executions: &Vec<Vec<Arc<Execution>>>,
    #[cfg(target_os = "zkvm")] proven_fpvm_journals: &HashSet<Digest>,
) {
    let config_hash = crate::config::config_hash(&boot.rollup_config).unwrap();
    // When running an execution-only proof, we may only have one batch validated by the kailua client
    if boot.l1_head.is_zero() {
        assert_eq!(1, stitched_executions.len());
        return;
    };
    for execution_trace in stitched_executions {
        let precondition_hash = crate::executor::exec_precondition_hash(execution_trace.as_slice());
        // Validate receipt roots
        for execution in execution_trace {
            assert_eq!(
                execution.artifacts.header.receipts_root,
                kona_executor::compute_receipts_root(
                    execution.artifacts.execution_result.receipts.as_slice(),
                    &boot.rollup_config,
                    execution.attributes.payload_attributes.timestamp
                )
            );
        }
        // Construct expected proof journal
        let encoded_journal = ProofJournal::new_stitched(
            fpvm_image_id,
            payout_recipient_address,
            precondition_hash,
            B256::from(config_hash),
            &StitchedBootInfo {
                l1_head: B256::ZERO,
                agreed_l2_output_root: execution_trace
                    .first()
                    .expect("Empty execution trace")
                    .agreed_output,
                claimed_l2_output_root: execution_trace
                    .last()
                    .expect("Empty execution trace")
                    .claimed_output,
                claimed_l2_block_number: execution_trace
                    .last()
                    .expect("Empty execution trace")
                    .artifacts
                    .header
                    .number,
            },
        )
        .encode_packed();
        // Require transition proof for entire batch
        verify_stitching_journal(
            fpvm_image_id,
            encoded_journal,
            #[cfg(target_os = "zkvm")]
            proven_fpvm_journals,
        )
    }
}

/// Stitches multiple boot information records into a unified `ProofJournal`.
///
/// This function consolidates and verifies multiple bootstrapping records, validating their
/// integrity and creating a coherent journal that reflects the intermediate states and outputs
/// of the bootstrapping process.
///
/// # Arguments
///
/// * `boot` - A reference to the base `BootInfo` structure used as the initial data point.
/// * `fpvm_image_id` - A 256-bit identifier representing the FPVM image being used.
/// * `payout_recipient_address` - The Ethereum address to which payouts should be sent.
/// * `precondition_hash` - A 256-bit hash representing the preconditions required for stitching.
/// * `stitched_boot_info` - A vector of `StitchedBootInfo` objects that are incrementally stitched
///   into the `ProofJournal`.
/// * `proven_fpvm_journals` - (Optional, only on `zkvm` platforms) A reference to a set of
///   precomputed and verified FPVM journal digests used for proof verification.
///
/// # Returns
///
/// A `ProofJournal` object that reflects the final stitched state after processing
/// all input records.
///
/// # Panics
///
/// This function will panic in the following scenarios:
///
/// 1. **Equivalence Check Failure**: If the `l1_head` values in the current and stitched boots
///    are inconsistent.
/// 2. **Progress Check Failure**: If there is no progress between the `agreed_l2_output_root` and
///    `claimed_l2_output_root` of a `stitched_boot` object.
/// 3. **Proof Assumption Failure**: If the stitching proof journal fails the `verify_stitching_journal`
///    check.
/// 4. **Non-contiguous Stitching**: If the claimed and agreed L2 output roots cannot be matched
///    in a forward or backward stitching configuration.
///
/// # Stitching Logic
///
/// 1. The function initializes a `ProofJournal` object using the base `BootInfo` structure and
///    additional parameters.
/// 2. For each `StitchedBootInfo` object in `stitched_boot_info`:
///     - Verify the equivalence of `l1_head`.
///     - Ensure progress is made between `agreed_l2_output_root` and `claimed_l2_output_root`.
///     - Validate the proof associated with the stitching via the `verify_stitching_journal` function.
///     - Perform continuity checks and update the journal in a forward or backward stitching
///       configuration. If stitching is non-contiguous, the function will panic.
///
/// # Platform-specific Behavior
///
/// * On `zkvm` platforms, the function requires access to `proven_fpvm_journals` to verify stitching
///   proofs. On other platforms, the verification step is omitted.
pub fn stitch_boot_info(
    boot: &BootInfo,
    fpvm_image_id: B256,
    payout_recipient_address: Address,
    precondition_hash: B256,
    stitched_boot_info: Vec<StitchedBootInfo>,
    #[cfg(target_os = "zkvm")] proven_fpvm_journals: &HashSet<Digest>,
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
        verify_stitching_journal(
            fpvm_image_id,
            ProofJournal::new_stitched(
                fpvm_image_id,
                payout_recipient_address,
                precondition_hash,
                stitched_journal.config_hash,
                &stitched_boot,
            )
            .encode_packed(),
            #[cfg(target_os = "zkvm")]
            proven_fpvm_journals,
        );
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
