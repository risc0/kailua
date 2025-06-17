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

use crate::executor::{exec_precondition_hash, new_execution_cursor, CachedExecutor, Execution};
use crate::kona::OracleL1ChainProvider;
use crate::{client, precondition};
use alloy_op_evm::OpEvmFactory;
use alloy_primitives::{Sealed, B256};
use anyhow::{bail, Context};
use kona_derive::prelude::{BlobProvider, EthereumDataSource};
use kona_driver::{Driver, Executor};
use kona_executor::TrieDBProvider;
use kona_preimage::{CommsClient, PreimageKey};
use kona_proof::errors::OracleProviderError;
use kona_proof::executor::KonaExecutor;
use kona_proof::l1::OraclePipeline;
use kona_proof::l2::OracleL2ChainProvider;
use kona_proof::sync::new_oracle_pipeline_cursor;
use kona_proof::{BootInfo, FlushableCache, HintType};
use std::fmt::Debug;
use std::mem::take;
use std::sync::{Arc, Mutex};

/// Runs the Kailua client to drive rollup state transition derivation using Kona.
///
/// # Arguments
/// * `precondition_validation_data_hash` - A 256-bit hash used for fetching precondition data.
/// * `oracle` - The client for communicating with the host environment.
/// * `stream` - The client for streamed communication with the host.
/// * `beacon` - An instance of the blob provider.
/// * `execution_cache` - A vector of cached executions to reuse.
/// * `collection_target` - An optional target to dump uncached executions.
///
/// # Returns
/// A result containing a tuple (`BootInfo`, `B256`) upon success, or an error of type `anyhow::Error`.
/// - `BootInfo` contains essential configuration information for bootstrapping the rollup client.
/// - `B256` represents a 256-bit hash of the computed output state.
///
/// # Errors
/// This function can return an error in any of the following cases:
/// * Failure to load `BootInfo`.
/// * Invalid `claimed_l2_block_number` value compared to the safe L2 head number.
/// * Assertion failures during execution trace validation, block derivations, and outputs validation.
/// * Insufficient L1 data to derive L2 output roots for the claimed block height.
///
/// # Workflow
///
/// ## 1. Bootstrapping & Safe Head Validation
/// - Loads `BootInfo` from the oracle.
/// - Fetches the safe head hash and constructs chain providers for both L1 and L2.
/// - Validates that the claimed L2 block number is greater than or equal to the L2 safe head.
///
/// ## 2. Execution Caching
/// - If the L1 head is a zero hash, the function operates in "execution only" mode:
///     - Initializes the execution cursor and uses a `KonaExecutor` for execution validation.
///     - Validates the consistency of execution traces against the expected results derived from `execution_cache`.
///
/// ## 3. Derivation and Execution
/// - Loads precondition data based on the provided hash, if any.
/// - Initializes the pipeline cursor and an `OraclePipeline`.
/// - Combines execution caching with pipeline-driven iteration to derive L2 outputs incrementally until the claimed L2 height:
///     - Validates outputs, ensuring sufficient L1 data exists for subsequent derivations.
///     - Adjusts the executor state for consecutive computation and output production.
///     - Logs the progress and appends derived output roots.
///
/// ## 4. Final Validation & Output
/// - Verifies the computed outputs:
///     - Ensures the final output hash matches the claimed L2 output root.
///     - Handles insufficient data to derive output roots by returning a matching zero hash.
pub fn run_core_client<
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
>(
    precondition_validation_data_hash: B256,
    oracle: Arc<O>,
    stream: Arc<O>,
    mut beacon: B,
    execution_cache: Vec<Arc<Execution>>,
    collection_target: Option<Arc<Mutex<Vec<Execution>>>>,
) -> anyhow::Result<(BootInfo, B256)>
where
    <B as BlobProvider>::Error: Debug,
{
    let (boot, precondition_hash, output_hash) = kona_proof::block_on(async move {
        ////////////////////////////////////////////////////////////////
        //                          PROLOGUE                          //
        ////////////////////////////////////////////////////////////////
        client::log("BOOT");
        let boot = BootInfo::load(oracle.as_ref())
            .await
            .context("BootInfo::load")?;
        let rollup_config = Arc::new(boot.rollup_config.clone());

        client::log("SAFE HEAD HASH");
        let safe_head_hash =
            fetch_safe_head_hash(oracle.as_ref(), boot.agreed_l2_output_root).await?;

        let mut l1_provider = OracleL1ChainProvider::new(boot.l1_head, stream).await?;
        let mut l2_provider =
            OracleL2ChainProvider::new(safe_head_hash, rollup_config.clone(), oracle.clone());

        // The claimed L2 block number must be greater than or equal to the L2 safe head.
        // Fetch the safe head's block header.
        client::log("SAFE HEAD");
        let safe_head = l2_provider
            .header_by_hash(safe_head_hash)
            .map(|header| Sealed::new_unchecked(header, safe_head_hash))?;

        if boot.claimed_l2_block_number < safe_head.number {
            bail!("Invalid claim");
        }
        let safe_head_number = safe_head.number;
        let expected_output_count = (boot.claimed_l2_block_number - safe_head_number) as usize;

        ////////////////////////////////////////////////////////////////
        //                     EXECUTION CACHING                      //
        ////////////////////////////////////////////////////////////////
        if boot.l1_head.is_zero() {
            client::log("EXECUTION ONLY");
            let cursor =
                new_execution_cursor(rollup_config.as_ref(), safe_head.clone(), &mut l2_provider)
                    .await
                    .context("new_execution_cursor")?;
            l2_provider.set_cursor(cursor.clone());

            let mut kona_executor = KonaExecutor::new(
                rollup_config.as_ref(),
                l2_provider.clone(),
                l2_provider.clone(),
                OpEvmFactory::default(),
                None,
            );
            kona_executor.update_safe_head(safe_head);

            // Validate expected block count
            assert_eq!(expected_output_count, execution_cache.len());

            // Validate non-empty execution trace
            assert!(!execution_cache.is_empty());

            // Calculate precondition hash
            let precondition_hash = exec_precondition_hash(execution_cache.as_slice());

            // Validate terminating block number
            assert_eq!(
                execution_cache.last().unwrap().artifacts.header.number,
                boot.claimed_l2_block_number
            );

            let mut latest_output_root = boot.agreed_l2_output_root;
            // Validate executed chain
            for execution in execution_cache {
                // Verify initial state
                assert_eq!(execution.agreed_output, latest_output_root);
                // Verify transition
                let executor_result = kona_executor
                    .execute_payload(execution.attributes.clone())
                    .await?;
                assert_eq!(execution.artifacts.header, executor_result.header);
                assert_eq!(
                    execution.artifacts.execution_result,
                    executor_result.execution_result
                );
                // Update state
                kona_executor.update_safe_head(execution.artifacts.header.clone());
                latest_output_root = kona_executor
                    .compute_output_root()
                    .context("compute_output_root: Verify post state")?;
                // Verify post state
                assert_eq!(execution.claimed_output, latest_output_root);
                client::log(&format!(
                    "OUTPUT: {}/{}",
                    execution.artifacts.header.number, boot.claimed_l2_block_number
                ));
            }

            // Return latest_output_root from closure to be validated against claimed_l2_output_root
            return Ok((boot, precondition_hash, Some(latest_output_root)));
        }

        ////////////////////////////////////////////////////////////////
        //                   DERIVATION & EXECUTION                   //
        ////////////////////////////////////////////////////////////////
        client::log("PRECONDITION");
        let precondition_data = precondition::load_precondition_data(
            precondition_validation_data_hash,
            oracle.clone(),
            &mut beacon,
        )
        .await
        .context("load_precondition_data")?;

        client::log("DERIVATION & EXECUTION");
        // Create a new derivation driver with the given boot information and oracle.
        let cursor = new_oracle_pipeline_cursor(
            rollup_config.as_ref(),
            safe_head,
            &mut l1_provider,
            &mut l2_provider,
        )
        .await
        .context("new_oracle_pipeline_cursor")?;
        l2_provider.set_cursor(cursor.clone());

        let da_provider =
            EthereumDataSource::new_from_parts(l1_provider.clone(), beacon, &rollup_config);
        let pipeline = OraclePipeline::new(
            rollup_config.clone(),
            cursor.clone(),
            oracle.clone(),
            da_provider,
            l1_provider.clone(),
            l2_provider.clone(),
        )
        .await
        .context("OraclePipeline::new")?;
        let cached_executor = CachedExecutor {
            cache: {
                // The cache elements will be popped from first to last
                let mut cache = execution_cache;
                cache.reverse();
                cache
            },
            executor: KonaExecutor::new(
                rollup_config.as_ref(),
                l2_provider.clone(),
                l2_provider.clone(),
                OpEvmFactory::default(),
                None,
            ),
            collection_target,
        };
        let mut driver = Driver::new(cursor, cached_executor, pipeline);

        // Run the derivation pipeline until we are able to produce the output root of the claimed
        // L2 block.
        let mut output_roots = Vec::with_capacity(expected_output_count);
        for starting_block in safe_head_number..boot.claimed_l2_block_number {
            // Advance to the next target
            let (output_block, output_root) = driver
                .advance_to_target(&boot.rollup_config, Some(starting_block + 1))
                .await
                .context("advance_to_target")?;
            // Stop if nothing new was derived
            if output_block.block_info.number == starting_block {
                // A mismatch indicates that there is insufficient L1 data available to produce
                // an L2 output root at the claimed block number
                client::log("HALT");
                break;
            } else {
                client::log(&format!(
                    "OUTPUT: {}/{}",
                    output_block.block_info.number, boot.claimed_l2_block_number
                ));
            }
            // Append newly computed output root
            output_roots.push(output_root);
        }

        ////////////////////////////////////////////////////////////////
        //                          EPILOGUE                          //
        ////////////////////////////////////////////////////////////////
        client::log("EPILOGUE");

        let precondition_hash = precondition_data
            .map(|(precondition_validation_data, blobs)| {
                precondition::validate_precondition(
                    precondition_validation_data,
                    blobs,
                    safe_head_number,
                    &output_roots,
                )
            })
            .unwrap_or(Ok(B256::ZERO))
            .context("validate_precondition")?;

        if output_roots.len() != expected_output_count {
            // Not enough data to derive output root at claimed height
            Ok((boot, precondition_hash, None))
        } else if output_roots.is_empty() {
            // note: This implies expected_output_count == 0
            // Claimed output height is equal to agreed output height
            let real_output_hash = boot.agreed_l2_output_root;
            Ok((boot, precondition_hash, Some(real_output_hash)))
        } else {
            // Derived output root at future height
            Ok((boot, precondition_hash, output_roots.pop()))
        }
    })?;

    // Check claimed_l2_output_root correctness
    if let Some(computed_output) = output_hash {
        // With sufficient data, the input l2_claim must be true
        assert_eq!(boot.claimed_l2_output_root, computed_output);
    } else if !boot.claimed_l2_output_root.is_zero() {
        // We use the zero claim hash to denote that the data as of l1 head is insufficient
        bail!("Expected zero claim hash.");
    }

    Ok((boot, precondition_hash))
}

/// Fetches the safe head hash of the L2 chain based on the agreed upon L2 output root in the
/// [BootInfo].
pub async fn fetch_safe_head_hash<O>(
    caching_oracle: &O,
    agreed_l2_output_root: B256,
) -> Result<B256, OracleProviderError>
where
    O: CommsClient,
{
    let mut output_preimage = [0u8; 128];
    HintType::StartingL2Output
        .with_data(&[agreed_l2_output_root.as_ref()])
        .send(caching_oracle)
        .await?;
    caching_oracle
        .get_exact(
            PreimageKey::new_keccak256(*agreed_l2_output_root),
            output_preimage.as_mut(),
        )
        .await?;

    output_preimage[96..128]
        .try_into()
        .map_err(OracleProviderError::SliceConversion)
}

/// Recovers a continuous execution trace from the collection target
pub fn recover_collected_executions(
    collection_target: Arc<Mutex<Vec<Execution>>>,
    claimed_l2_output_root: B256,
) -> Vec<Arc<Execution>> {
    let mut executions = collection_target.lock().unwrap();
    for i in 1..executions.len() {
        executions[i - 1].claimed_output = executions[i].agreed_output;
    }
    if let Some(last_exec) = executions.last_mut() {
        last_exec.claimed_output = claimed_l2_output_root;
    }
    take::<Vec<Execution>>(executions.as_mut())
        .into_iter()
        .map(Arc::new)
        .collect::<Vec<_>>()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod tests {
    use super::*;
    use crate::client::tests::TestOracle;
    use crate::precondition::PreconditionValidationData;
    use alloy_primitives::{b256, B256};
    use kona_proof::l1::OracleBlobProvider;
    use kona_proof::BootInfo;
    use std::sync::{Arc, Mutex};

    pub fn test_derivation(
        boot_info: BootInfo,
        precondition_validation_data: Option<PreconditionValidationData>,
    ) -> anyhow::Result<Vec<Arc<Execution>>> {
        let oracle = Arc::new(TestOracle::new(boot_info.clone()));
        let (expected_precondition_hash, precondition_validation_data_hash) =
            if let Some(data) = precondition_validation_data {
                (data.precondition_hash(), oracle.add_precondition_data(data))
            } else {
                Default::default()
            };
        let collection_target = Arc::new(Mutex::new(Vec::new()));
        let (result_boot_info, precondition_hash) = run_core_client(
            precondition_validation_data_hash,
            oracle.clone(),
            oracle.clone(),
            OracleBlobProvider::new(oracle.clone()),
            vec![],
            Some(collection_target.clone()),
        )
        .context("run_core_client")?;

        assert_eq!(result_boot_info.l1_head, boot_info.l1_head);
        assert_eq!(
            result_boot_info.agreed_l2_output_root,
            boot_info.agreed_l2_output_root
        );
        assert_eq!(
            result_boot_info.claimed_l2_output_root,
            boot_info.claimed_l2_output_root
        );
        assert_eq!(
            result_boot_info.claimed_l2_block_number,
            boot_info.claimed_l2_block_number
        );
        assert_eq!(result_boot_info.chain_id, boot_info.chain_id);

        assert_eq!(expected_precondition_hash, precondition_hash);

        let execution_cache =
            recover_collected_executions(collection_target, boot_info.claimed_l2_output_root);

        Ok(execution_cache)
    }

    pub fn test_execution(
        boot_info: BootInfo,
        execution_cache: Vec<Arc<Execution>>,
    ) -> anyhow::Result<B256> {
        // Ensure boot info triggers execution only
        assert!(boot_info.l1_head.is_zero());
        let expected_precondition_hash = exec_precondition_hash(execution_cache.as_slice());

        let oracle = Arc::new(TestOracle::new(boot_info.clone()));
        let (result_boot_info, precondition_hash) = run_core_client(
            B256::ZERO,
            oracle.clone(),
            oracle.clone(),
            OracleBlobProvider::new(oracle.clone()),
            execution_cache,
            None,
        )
        .expect("run_core_client");

        assert_eq!(result_boot_info.l1_head, boot_info.l1_head);
        assert_eq!(
            result_boot_info.agreed_l2_output_root,
            boot_info.agreed_l2_output_root
        );
        assert_eq!(
            result_boot_info.claimed_l2_output_root,
            boot_info.claimed_l2_output_root
        );
        assert_eq!(
            result_boot_info.claimed_l2_block_number,
            boot_info.claimed_l2_block_number
        );
        assert_eq!(result_boot_info.chain_id, boot_info.chain_id);
        assert_eq!(precondition_hash, expected_precondition_hash);

        Ok(precondition_hash)
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491250() {
        test_derivation(
            BootInfo {
                l1_head: b256!(
                    "0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"
                ),
                agreed_l2_output_root: b256!(
                    "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
                ),
                claimed_l2_output_root: b256!(
                    "0xa130fbfa315391b28668609252e4c09c3df3b77562281b996af30bf056cbb2c1"
                ),
                claimed_l2_block_number: 16491250,
                chain_id: 11155420,
                rollup_config: Default::default(),
            },
            None,
        )
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349() {
        let executions = test_derivation(
            BootInfo {
                l1_head: b256!(
                    "0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"
                ),
                agreed_l2_output_root: b256!(
                    "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
                ),
                claimed_l2_output_root: b256!(
                    "0x6984e5ae4d025562c8a571949b985692d80e364ddab46d5c8af5b36a20f611d1"
                ),
                claimed_l2_block_number: 16491349,
                chain_id: 11155420,
                rollup_config: Default::default(),
            },
            None,
        )
        .unwrap();
        let _ = test_execution(
            BootInfo {
                l1_head: B256::ZERO,
                agreed_l2_output_root: b256!(
                    "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
                ),
                claimed_l2_output_root: b256!(
                    "0x6984e5ae4d025562c8a571949b985692d80e364ddab46d5c8af5b36a20f611d1"
                ),
                claimed_l2_block_number: 16491349,
                chain_id: 11155420,
                rollup_config: Default::default(),
            },
            executions,
        )
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349_validity() {
        test_derivation(
            BootInfo {
                l1_head: b256!(
                    "0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"
                ),
                agreed_l2_output_root: b256!(
                    "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
                ),
                claimed_l2_output_root: b256!(
                    "0x6984e5ae4d025562c8a571949b985692d80e364ddab46d5c8af5b36a20f611d1"
                ),
                claimed_l2_block_number: 16491349,
                chain_id: 11155420,
                rollup_config: Default::default(),
            },
            Some(PreconditionValidationData::Validity {
                proposal_l2_head_number: 16491249,
                proposal_output_count: 1,
                output_block_span: 100,
                blob_hashes: vec![],
            }),
        )
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491349_insufficient() {
        // data wasn't published at l1 origin
        test_derivation(
            BootInfo {
                l1_head: b256!(
                    "0x78228b4f2d59ae1820b8b8986a875630cb32d88b298d78d0f25bcac8f3bdfbf3"
                ),
                agreed_l2_output_root: b256!(
                    "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
                ),
                claimed_l2_output_root: B256::ZERO,
                claimed_l2_block_number: 16491349,
                chain_id: 11155420,
                rollup_config: Default::default(),
            },
            None,
        )
        .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491248_failure() {
        test_derivation(
            BootInfo {
                l1_head: b256!(
                    "0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"
                ),
                agreed_l2_output_root: b256!(
                    "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
                ),
                claimed_l2_output_root: b256!(
                    "0xa130fbfa315391b28668609252e4c09c3df3b77562281b996af30bf056cbb2c1"
                ),
                claimed_l2_block_number: 16491248,
                chain_id: 11155420,
                rollup_config: Default::default(),
            },
            None,
        )
        .unwrap_err();
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_op_sepolia_16491249_16491249() {
        let executions = test_derivation(
            BootInfo {
                l1_head: b256!(
                    "0x417ffee9dd1ccbd35755770dd8c73dbdcd96ba843c532788850465bdd08ea495"
                ),
                agreed_l2_output_root: b256!(
                    "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
                ),
                claimed_l2_output_root: b256!(
                    "0x82da7204148ba4d8d59e587b6b3fdde5561dc31d9e726220f7974bf9f2158d75"
                ),
                claimed_l2_block_number: 16491249,
                chain_id: 11155420,
                rollup_config: Default::default(),
            },
            None,
        )
        .unwrap();
        assert!(executions.is_empty());
    }
}
