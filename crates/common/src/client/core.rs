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
use std::sync::{Arc, Mutex};

/// Runs the Kailua client to process rollup operations, including block derivation,
/// precondition validation, safe head fetching, execution caching, and pipeline execution.
///
/// # Type Parameters
/// * `O` - Represents a communication client implementing `CommsClient` and `FlushableCache`.
/// * `B` - Represents a blob provider implementing `BlobProvider`.
///
/// # Arguments
/// * `precondition_validation_data_hash` - A 256-bit hash used for precondition validation.
/// * `oracle` - An atomic reference-counted (`Arc`) wrapper around the communication client.
/// * `stream` - An atomic reference-counted (`Arc`) wrapper around the communication stream client.
/// * `beacon` - An instance of the blob provider, cloning a data source for beacon usage.
/// * `execution_cache` - A vector of cached executions.
/// * `collection_target` - An optional, thread-safe, target to dump uncached executions.
///
/// # Returns
/// Returns a result containing a tuple (`BootInfo`, `B256`) upon success, or an error of type `anyhow::Error`.
///
/// `BootInfo` contains essential configuration information for bootstrapping the rollup client.
/// `B256` represents a 256-bit hash of the results or preconditions, dependent on the actions performed.
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
                    .await?;
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

            // Validate executed chain
            for execution in execution_cache {
                // Verify initial state
                assert_eq!(
                    execution.agreed_output,
                    kona_executor.compute_output_root()?
                );
                // Verify transition
                assert_eq!(
                    execution.artifacts.header,
                    kona_executor
                        .execute_payload(execution.attributes.clone())
                        .await?
                        .header
                );
                assert_eq!(
                    execution.artifacts.execution_result,
                    kona_executor
                        .execute_payload(execution.attributes.clone())
                        .await?
                        .execution_result
                );
                // Update safe head
                kona_executor.update_safe_head(execution.artifacts.header.clone());
                // Verify post state
                assert_eq!(
                    execution.claimed_output,
                    kona_executor.compute_output_root()?
                );
                client::log(&format!(
                    "OUTPUT: {}/{}",
                    execution.artifacts.header.number, boot.claimed_l2_block_number
                ));
            }

            // Validate final output against claimed output hash
            return Ok((
                boot,
                precondition_hash,
                Some(kona_executor.compute_output_root()?),
            ));
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
        .await?;

        client::log("DERIVATION & EXECUTION");
        // Create a new derivation driver with the given boot information and oracle.
        let cursor = new_oracle_pipeline_cursor(
            rollup_config.as_ref(),
            safe_head,
            &mut l1_provider,
            &mut l2_provider,
        )
        .await?;
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
        .await?;
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
                .await?;
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

#[cfg(test)]
pub mod tests {
    #[tokio::test]
    pub async fn test_kona() {
        // use kona oracle to drive client
    }
}
