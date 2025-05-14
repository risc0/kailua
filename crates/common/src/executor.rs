// Copyright 2025 RISC Zero, Inc.
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

use crate::client::log;
use crate::config::safe_default;
use crate::rkyv::execution::BlockBuildingOutcomeRkyv;
use crate::rkyv::optimism::OpPayloadAttributesRkyv;
use crate::rkyv::primitives::B256Def;
use alloy_consensus::Header;
use alloy_eips::eip4895::Withdrawal;
use alloy_primitives::{Bytes, Sealed, B256, B64};
use anyhow::Context;
use async_trait::async_trait;
use kona_driver::{Executor, PipelineCursor, TipCursor};
use kona_executor::BlockBuildingOutcome;
use kona_genesis::RollupConfig;
use kona_preimage::CommsClient;
use kona_proof::errors::OracleProviderError;
use kona_proof::l2::OracleL2ChainProvider;
use kona_proof::FlushableCache;
use kona_protocol::{BatchValidationProvider, BlockInfo};
use op_alloy_rpc_types_engine::OpPayloadAttributes;
use risc0_zkvm::sha::{Impl as SHA2, Sha256};
use spin::RwLock;
use std::fmt::Debug;
use std::sync::{Arc, Mutex};

/// A structure representing the execution process and its results.
///
/// This struct is designed to hold essential information about the execution,
/// including its initial state, the attributes associated with the execution,
/// the resulting artifacts, and the final state after execution.
///
/// # Fields
///
/// * `agreed_output`:
///   - The output root before execution starts.
///   - This value represents the agreed-upon initial state prior to processing.
///   - Serialized/deserialized using `B256Def`.
///
/// * `attributes`:
///   - The derived attributes that are to be used during the execution.
///   - Contains additional metadata or instructions defining the execution.
///   - Serialized/deserialized using `OpPayloadAttributesRkyv`.
///
/// * `artifacts`:
///   - The resulting output block or artifacts generated during the execution process.
///   - Describes the outcome of the execution process.
///   - Serialized/deserialized using `BlockBuildingOutcomeRkyv`.
///
/// * `claimed_output`:
///   - The final output root after execution completes.
///   - Expected to match the state produced by processing the execution's attributes and artifacts.
///   - Serialized/deserialized using `B256Def`.
///
/// # Derives
///
/// The struct derives the following traits:
/// * `Clone`: Allows duplication of an instance of this struct.
/// * `Debug`: Enables formatting the struct for debugging purposes.
/// * `rkyv::Archive`: Facilitates archiving this struct for zero-copy deserialization.
/// * `rkyv::Serialize`: Allows serialization of this struct for archival purposes.
/// * `rkyv::Deserialize`: Permits deserialization from its archived form.
///
/// This struct is designed to interoperate with the `rkyv` crate, which provides efficient serialization and immutable archiving capabilities.
#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Execution {
    /// Output root prior to execution
    #[rkyv(with = B256Def)]
    pub agreed_output: B256,
    /// Derived attributes to be executed
    #[rkyv(with = OpPayloadAttributesRkyv)]
    pub attributes: OpPayloadAttributes,
    /// Output block from execution
    #[rkyv(with = BlockBuildingOutcomeRkyv)]
    pub artifacts: BlockBuildingOutcome,
    /// Output root after execution
    #[rkyv(with = B256Def)]
    pub claimed_output: B256,
}

/// A structure that provides a caching layer for an `Executor` implementation.
///
/// The `CachedExecutor` is a generic struct that allows the caching of executed tasks
/// and their results. It is designed to work with executors that implement the `Executor`
/// trait and are thread-safe (`Send` and `Sync`).
///
/// # Type Parameters
/// * `E` - The type of the executor being wrapped, which must implement the `Executor` trait
///   and be thread-safe (`Send` and `Sync`).
///
/// # Fields
/// * `cache` - A vector of cached execution results. The cached results are wrapped in an
///   `Arc` to allow sharing across threads safely.
/// * `executor` - The underlying executor that performs the task execution. This is used
///   when a task is not found in the cache.
/// * `collection_target` - An optional shared target for collecting executed tasks. It is wrapped
///   in an `Arc<Mutex<_>>`, allowing concurrent access and modifications while ensuring thread safety.
///
/// # Debugging
/// The `CachedExecutor` derives the `Debug` trait for usability in debugging scenarios, allowing
/// the structure and its fields to be printed for inspection.
#[derive(Debug)]
pub struct CachedExecutor<E: Executor + Send + Sync + Debug> {
    pub cache: Vec<Arc<Execution>>,
    pub executor: E,
    pub collection_target: Option<Arc<Mutex<Vec<Execution>>>>,
}

impl<E: Executor + Send + Sync + Debug> Drop for CachedExecutor<E> {
    fn drop(&mut self) {
        if !self.cache.is_empty() {
            #[cfg(target_os = "zkvm")]
            log(&format!("EXEC CACHE UNUSED: {}", self.cache.len()));
            #[cfg(not(target_os = "zkvm"))]
            tracing::error!("EXEC CACHE UNUSED: {}", self.cache.len());
        }
    }
}

#[async_trait]
impl<E: Executor + Send + Sync + Debug> Executor for CachedExecutor<E> {
    type Error = <E as Executor>::Error;

    /// An asynchronous function that waits until the executor is ready.
    ///
    /// This function calls the `wait_until_ready` method on the internal `executor`
    /// and awaits its completion. It ensures that the required state or prerequisites
    /// are ready before proceeding.
    ///
    /// This can be used in scenarios where subsequent operations depend on the
    /// readiness of the executor.
    async fn wait_until_ready(&mut self) {
        self.executor.wait_until_ready().await;
    }

    /// Updates the "safe head" of the blockchain to the specified sealed header.
    ///
    /// The "safe head" refers to the point in the blockchain that is considered
    /// finalized or safe for operations and is used as a reference for further
    /// progression or validation within the node.
    ///
    /// # Parameters
    /// - `header`: A `Sealed<Header>` representing the new "safe head" of the blockchain.
    ///   It is a sealed header, meaning it has gone through necessary validation and is immutable.
    ///
    /// # Behavior
    /// Delegates the update operation to the `executor` component that handles
    /// the internal logic for updating the safe head within the system.
    fn update_safe_head(&mut self, header: Sealed<Header>) {
        self.executor.update_safe_head(header);
    }

    /// Executes a given payload based on the specified `OpPayloadAttributes` and manages its outcomes.
    ///
    /// # Parameters
    /// - `attributes`: An instance of [`OpPayloadAttributes`] containing the attributes used to build or execute the payload.
    ///
    /// # Returns
    /// - `Result<BlockBuildingOutcome, Self::Error>`: On success, returns the resulting [`BlockBuildingOutcome`].
    ///   If an error occurs during the execution process or cache lookup, returns `Self::Error`.
    ///
    /// # Functionality
    /// 1. Calculates an agreed output root by invoking `compute_output_root`, which must succeed.
    /// 2. Checks if the execution can be optimized by matching the given attributes and the agreed output
    ///    with the last cached entry:
    ///    - If a match is found:
    ///      - Uses the cached artifacts and logs the cache hit details.
    ///      - Updates the safe head with the cached header.
    ///      - Returns the cached artifacts.
    /// 3. If no cache optimization is possible but a `collection_target` exists:
    ///    - Executes the payload with the given attributes using the executor.
    ///    - Updates the `collection_target` with the resulting artifacts and metadata.
    ///    - Returns the resulting artifacts.
    /// 4. If no additional conditions are met, directly delegates the execution of the payload to the executor and awaits its result.
    ///
    /// # Edge Cases
    /// - If no cached result is found (`self.cache.last()` fails or returns `None`):
    ///   - Verification of cache attributes defaults to `false`.
    /// - Handles locking and modification of the shared `collection_target` state safely.
    ///
    /// # Notes
    /// - If an entry exists in the cache, it is popped after a cache hit is confirmed.
    /// - The function assumes `self.executor.execute_payload` is an asynchronous operation meaning any errors or delays
    ///   during this invocation may affect execution flow.
    /// - Logging details about cache hits include the header number of the cached artifacts.
    ///
    /// # Errors
    /// - Returns an appropriate error (`Self::Error`) if:
    ///   - `compute_output_root` fails to calculate the agreed output root.
    ///   - Matching cached attributes comparison (or unwrap) fails.
    ///   - Payload execution via `self.executor` encounters an issue.
    async fn execute_payload(
        &mut self,
        attributes: OpPayloadAttributes,
    ) -> Result<BlockBuildingOutcome, Self::Error> {
        let agreed_output = self.compute_output_root()?;
        if self
            .cache
            .last()
            .map(|e| Ok(agreed_output == e.agreed_output && attributes == e.attributes))
            .unwrap_or(Ok(false))?
        {
            let artifacts = self.cache.pop().unwrap().artifacts.clone();
            log(&format!("CACHE {}", artifacts.header.number));
            self.update_safe_head(artifacts.header.clone());
            return Ok(artifacts);
        }
        if let Some(collection_target) = &self.collection_target {
            let artifacts = self.executor.execute_payload(attributes.clone()).await?;
            let mut collection_target = collection_target.lock().unwrap();
            collection_target.push(Execution {
                agreed_output,
                attributes,
                artifacts: artifacts.clone(),
                claimed_output: Default::default(),
            });
            return Ok(artifacts);
        }
        self.executor.execute_payload(attributes).await
    }

    /// Computes the output root based on the current state of the executor.
    ///
    /// This method invokes the `compute_output_root` function on the executor associated
    /// with this instance. The output root is a cryptographic hash (of type `B256`) that
    /// represents the resulting state or output from the operations performed by the executor.
    ///
    /// This operation could potentially result in an error, in which case the error type
    /// associated with this instance (`Self::Error`) will be returned.
    ///
    /// # Returns
    /// - `Ok(B256)`: The computed output root if the operation succeeds.
    /// - `Err(Self::Error)`: If an error occurs during the computation.
    ///
    /// # Errors
    /// This method will return an error if the executor fails to compute the output root.
    fn compute_output_root(&mut self) -> Result<B256, Self::Error> {
        self.executor.compute_output_root()
    }
}

/// Initializes and constructs a new `PipelineCursor` for a given L2 chain.
///
/// This function sets up the execution cursor required for processing rollup blocks. It
/// adjusts the starting L1 block position based on the `channel_timeout` to ensure
/// that the entire channel data is included for processing.
///
/// # Arguments
/// * `rollup_config` - A reference to the rollup configuration containing chain-specific settings.
/// * `safe_header` - A sealed header representing the latest safe L1 block.
/// * `l2_chain_provider` - A mutable reference to an L2 chain provider for fetching L2 block data.
///
/// # Type Parameters
/// * `O` - A generic parameter representing the Oracle client implementation.
///   It must implement the traits `CommsClient`, `FlushableCache`, `Send`, `Sync`, and `Debug`.
///
/// # Returns
/// A shared reference to a thread-safe `PipelineCursor` wrapped in an `Arc` and `RwLock`,
/// or an error of type `OracleProviderError` if the cursor initialization fails.
///
/// # Errors
/// This function will return an error if:
/// - The L2 chain provider fails to fetch the block information for the given `safe_header`.
///
/// # Details
/// - Retrieves the L2 block information associated with the provided `safe_header`.
/// - Computes the `channel_timeout` based on the rollup configuration and L2 block timestamp.
/// - Creates a new `PipelineCursor` using the computed `channel_timeout`.
/// - Advances the cursor to the proper state based on default `BlockInfo` and the L2 tip.
/// - Returns the cursor wrapped in an `Arc<RwLock<PipelineCursor>>` for safe concurrent access.
pub async fn new_execution_cursor<O>(
    rollup_config: &RollupConfig,
    safe_header: Sealed<Header>,
    l2_chain_provider: &mut OracleL2ChainProvider<O>,
) -> Result<Arc<RwLock<PipelineCursor>>, OracleProviderError>
where
    O: CommsClient + FlushableCache + FlushableCache + Send + Sync + Debug,
{
    let safe_head_info = l2_chain_provider
        .l2_block_info_by_number(safe_header.number)
        .await?;

    // Walk back the starting L1 block by `channel_timeout` to ensure that the full channel is
    // captured.
    let channel_timeout = rollup_config.channel_timeout(safe_head_info.block_info.timestamp);

    // Construct the cursor.
    let mut cursor = PipelineCursor::new(channel_timeout, BlockInfo::default());
    let tip = TipCursor::new(safe_head_info, safe_header, B256::ZERO);
    cursor.advance(BlockInfo::default(), tip);

    // Wrap the cursor in a shared read-write lock
    Ok(Arc::new(RwLock::new(cursor)))
}

/// Computes the hash of the given payload attributes in the `OpPayloadAttributes` structure.
///
/// This function generates a unique hash by processing various fields in the `attributes`
/// structure. It uses cryptographic hashing to ensure consistency and reliability of the hash.
///
/// # Arguments
///
/// * `attributes` - A reference to an `OpPayloadAttributes` structure containing the payload
///   attributes and optional fields for computing the hash.
///
/// # Returns
///
/// * `Ok(B256)` - A 256-bit hash value (`B256`) representing the computed hash for the provided
///   attributes.
/// * `Err(anyhow::Error)` - If any of the hashing steps fail or if an invalid value is encountered,
///   the function returns an error.
///
/// # Details
///
/// The hash is computed as follows:
/// - The `timestamp` field is serialized into big-endian bytes.
/// - The `prev_randao` field is used directly as a byte slice.
/// - The `suggested_fee_recipient` field is used directly as a byte slice.
/// - The optional `withdrawals` field is hashed to provide a consistent value, or defaults to
///   `B256::ZERO` if missing.
/// - The `parent_beacon_block_root` field is hashed or defaults to `B256::ZERO` if unavailable.
/// - The `transactions` field is treated as an optional field and hashed similarly; it defaults
///   to `B256::ZERO`.
/// - The `no_tx_pool` field is hashed as a single byte that represents whether the pool is present.
/// - The `gas_limit` field is converted to its big-endian byte representation or defaults to the
///   maximum `u64` value.
/// - The `eip_1559_params` field is hashed from its value or defaults to a predefined value.
///
/// These fields are concatenated into a single byte array, which is hashed using the SHA-256
/// algorithm to produce the final `B256` hash.
///
/// # Errors
///
/// This function returns an error if:
/// - Any of the optional fields fail due to `safe_default` processing.
/// - The hashing process encounters an invalid value or an error during conversion.
///
pub fn attributes_hash(attributes: &OpPayloadAttributes) -> anyhow::Result<B256> {
    let hashed_bytes = [
        attributes
            .payload_attributes
            .timestamp
            .to_be_bytes()
            .as_slice(),
        attributes.payload_attributes.prev_randao.as_slice(),
        attributes
            .payload_attributes
            .suggested_fee_recipient
            .as_slice(),
        safe_default(
            attributes
                .payload_attributes
                .withdrawals
                .as_ref()
                .map(|wds| withdrawals_hash(wds.as_slice())),
            B256::ZERO,
        )
        .context("safe_default withdrawals")?
        .as_slice(),
        safe_default(
            attributes.payload_attributes.parent_beacon_block_root,
            B256::ZERO,
        )
        .context("safe_default parent_beacon_block_root")?
        .as_slice(),
        safe_default(
            attributes.transactions.as_ref().map(transactions_hash),
            B256::ZERO,
        )
        .context("safe_default transactions_hash")?
        .as_slice(),
        &[safe_default(attributes.no_tx_pool, false).context("safe_default no_tx_pool")? as u8],
        safe_default(attributes.gas_limit, u64::MAX)
            .context("safe_default gas_limit")?
            .to_be_bytes()
            .as_slice(),
        safe_default(attributes.eip_1559_params, B64::new([0xff; 8]))
            .context("safe_default eip_1559_params")?
            .as_slice(),
    ]
    .concat();
    let digest: [u8; 32] = SHA2::hash_bytes(hashed_bytes.as_slice())
        .as_bytes()
        .try_into()?;
    Ok(digest.into())
}

/// Generates a hash value (B256) for a given slice of `Withdrawal` objects.
///
/// This function computes a hash for a list of withdrawals by concatenating
/// specific fields from each `Withdrawal` into a byte array, hashing the
/// resulting array, and returning the hash.
///
/// # Arguments
///
/// * `withdrawals` - A slice of `Withdrawal` objects to be hashed. Each
///   `Withdrawal` must contain the following fields:
///   - `index` (u64): Index of the withdrawal.
///   - `validator_index` (u64): Index of the validator associated with the withdrawal.
///   - `address` (Vec<u8>): The address where the withdrawal is sent.
///   - `amount` (u64): The amount of the withdrawal.
///
/// # Returns
///
/// * A `B256`, which is a 256-bit hash value computed from the input data.
///
/// # Process
///
/// 1. Iterates over each `Withdrawal` in the input slice.
/// 2. For each `Withdrawal`, concatenates its `index`, `validator_index`,
///    `address`, and `amount` fields into a byte array, converting numeric
///    fields to big-endian byte representations.
/// 3. Concatenates the byte arrays of all withdrawals into a single array.
/// 4. Computes a 256-bit SHA2 hash on the concatenated bytes.
/// 5. Converts the resulting hash into a fixed-size array (`[u8; 32]`) and
///    returns it as a `B256`.
///
/// # Panics
///
/// This function will panic if the output of the hash function cannot be
/// converted into a `[u8; 32]`. This should not happen under normal
/// circumstances.
///
/// # Dependencies
///
/// * The function depends on a cryptographic hashing library (`SHA2::hash_bytes`)
///   for computing the hash.
/// * The `B256` and `Withdrawal` types must be imported from their respective
///   modules or libraries.
///
/// # Notes
///
/// * The use of `SHA2` implies this function adheres to cryptographic standards
///   for hash computations.
/// * Ensure that the input data for `address` is correctly formatted as required
///   by your application to avoid mismatches or errors in the hash computation.
pub fn withdrawals_hash(withdrawals: &[Withdrawal]) -> B256 {
    let hashed_bytes = withdrawals
        .iter()
        .map(|w| {
            [
                w.index.to_be_bytes().as_slice(),
                w.validator_index.to_be_bytes().as_slice(),
                w.address.as_slice(),
                w.amount.to_be_bytes().as_slice(),
            ]
            .concat()
        })
        .collect::<Vec<_>>()
        .concat();
    let digest: [u8; 32] = SHA2::hash_bytes(hashed_bytes.as_slice())
        .as_bytes()
        .try_into()
        .unwrap();
    digest.into()
}

/// Computes a 256-bit hash of a vector of transactions using RLP encoding and SHA-256.
///
/// # Arguments
///
/// * `transactions` - A reference to a vector of `Bytes`, where each element represents
///   a serialized transaction to be included in the hash computation.
///
/// # Returns
///
/// * `B256` - A 256-bit hash of the RLP-encoded transactions.
///
/// # Methodology
///
/// 1. The provided `transactions` vector is encoded into a single byte sequence using
///    Recursive Length Prefix (RLP) encoding via the `alloy_rlp::encode` function.
/// 2. The resulting byte sequence is hashed using the SHA-256 cryptographic hashing function,
///    which produces a 32-byte digest.
/// 3. The 32-byte digest is converted into a type `B256`, which represents the final hash.
///
/// # Panics
///
/// This function will panic if the conversion from the SHA-256 output to a `[u8; 32]` slice fails.
/// This is highly unlikely under normal operation as the digest size of SHA-256 is guaranteed to be 32 bytes.
pub fn transactions_hash(transactions: &Vec<Bytes>) -> B256 {
    let hashed_bytes = alloy_rlp::encode(transactions);
    let digest: [u8; 32] = SHA2::hash_bytes(hashed_bytes.as_slice())
        .as_bytes()
        .try_into()
        .unwrap();
    digest.into()
}

/// Computes a precondition hash from a list of `Execution` objects.
///
/// This function generates a cryptographic hash (of type `B256`) by aggregating
/// and hashing a combination of key attributes from the provided `Execution`
/// objects. The hash can be used for verifying consistency, detecting changes,
/// or as an identifier for execution-related data.
///
/// # Parameters
/// - `executions`: A slice of `Arc<Execution>` objects. Each `Execution`
///   contains the data necessary to compute the hash.
///
/// # Returns
/// - A `B256` hash value representing the precondition hash of the executions.
///
/// # Panics
/// - The function will panic if the attributes of any `Execution` cannot be hashed
///   (i.e., `attributes_hash(&e.attributes)` returns an error).
/// - The function will also panic if the computed digest cannot be converted into
///   a 32-byte array (`[u8; 32]`).
///
/// # Logic
/// 1. Iterates through each `Execution` object in the slice.
/// 2. For each execution:
///    - Extracts its `agreed_output`, `attributes` (after hashing them),
///      `artifacts.header.hash()`, and `claimed_output`.
///    - Concatenates these components into a byte array.
/// 3. Combines the resulting byte arrays from all `Execution` objects into
///    a single byte array.
/// 4. Hashes the resulting byte array using the SHA256 cryptographic hash algorithm.
/// 5. Converts the resulting hash bytes into a fixed-size 32-byte array and returns it.
///
/// # Notes
/// - The `attributes_hash` function is expected to generate a hash for `Execution`
///   attributes, and an error from this function will result in a panic.
/// - The function assumes that all `Execution` objects in the input slice are valid,
///   and their internal states are consistent.
pub fn exec_precondition_hash(executions: &[Arc<Execution>]) -> B256 {
    let hashed_bytes = executions
        .iter()
        .map(|e| {
            [
                e.agreed_output.0,
                attributes_hash(&e.attributes)
                    .expect("Unhashable attributes.")
                    .0,
                e.artifacts.header.hash().0,
                e.claimed_output.0,
            ]
            .concat()
        })
        .collect::<Vec<_>>()
        .concat();
    let digest: [u8; 32] = SHA2::hash_bytes(hashed_bytes.as_slice())
        .as_bytes()
        .try_into()
        .unwrap();
    digest.into()
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::oracle::vec::VecOracle;
    use crate::oracle::WitnessOracle;
    use crate::rkyv::execution::tests::gen_execution_outcomes;
    use alloy_primitives::{keccak256, Address, Sealable};
    use alloy_rpc_types_engine::PayloadAttributes;
    use kona_mpt::TrieNode;
    use kona_preimage::PreimageKey;
    use rayon::prelude::{IntoParallelIterator, ParallelIterator};
    use rkyv::rancor::Error;
    use std::collections::HashSet;

    pub fn gen_executions(count: usize) -> Vec<Arc<Execution>> {
        gen_execution_outcomes(count)
            .into_iter()
            .enumerate()
            .map(|(i, artifacts)| {
                Arc::new(Execution {
                    agreed_output: keccak256(format!("output {i}")),
                    attributes: OpPayloadAttributes {
                        payload_attributes: PayloadAttributes {
                            timestamp: i as u64 * 1024,
                            prev_randao: keccak256(format!("prev_randao {i}")),
                            suggested_fee_recipient: Address::from_slice(&[0xf1; 20]),
                            withdrawals: Some(vec![Withdrawal {
                                index: 0,
                                validator_index: 0,
                                address: Address::from_slice(&[0xf2; 20]),
                                amount: i as u64 * 1024,
                            }]),
                            parent_beacon_block_root: Some(keccak256(format!(
                                "parent_beacon_block_root {i}"
                            ))),
                        },
                        transactions: Some(vec![format!("transactions {i}")
                            .as_bytes()
                            .to_vec()
                            .into()]),
                        no_tx_pool: Some(true),
                        gas_limit: Some(u64::MAX / 2),
                        eip_1559_params: Some(B64::new([0xb0; 8])),
                    },
                    artifacts,
                    claimed_output: keccak256(format!("output {}", i + 1)),
                })
            })
            .collect()
    }

    #[derive(Clone, Debug)]
    pub struct TestExecutor {
        pub outcomes: Vec<BlockBuildingOutcome>,
        pub output_roots: Vec<B256>,
    }

    #[async_trait]
    impl Executor for TestExecutor {
        type Error = kona_executor::ExecutorError;

        async fn wait_until_ready(&mut self) {}

        fn update_safe_head(&mut self, _header: Sealed<Header>) {}

        async fn execute_payload(
            &mut self,
            _attributes: OpPayloadAttributes,
        ) -> Result<BlockBuildingOutcome, Self::Error> {
            self.outcomes
                .pop()
                .ok_or(kona_executor::ExecutorError::MissingExecutor)
        }

        fn compute_output_root(&mut self) -> Result<B256, Self::Error> {
            self.output_roots
                .pop()
                .ok_or(kona_executor::ExecutorError::MissingExecutor)
        }
    }

    #[test]
    fn test_exec_precondition_hash() {
        let executions = gen_executions(16);
        // check hash uniqueness over all subsequences
        let hashes = Arc::new(Mutex::new(HashSet::new()));
        rayon::scope(|_| {
            (0..executions.len()).into_par_iter().for_each(|i| {
                ((i + 1)..executions.len()).into_par_iter().for_each(|j| {
                    // test hashing uniqueness
                    let hash = exec_precondition_hash(&executions[i..j]);
                    {
                        assert!(hashes.lock().unwrap().insert(hash));
                    }
                    // test serde
                    let trace = executions[i..j].to_vec();
                    let recoded = rkyv::from_bytes::<Vec<Arc<Execution>>, Error>(
                        rkyv::to_bytes::<Error>(&trace).unwrap().as_ref(),
                    )
                    .unwrap();
                    assert_eq!(hash, exec_precondition_hash(&recoded));
                });
            });
        });
    }

    #[tokio::test]
    async fn test_cached_executor() {
        let executions = gen_executions(128);
        let (outcomes, mut output_roots): (Vec<_>, Vec<_>) = executions
            .iter()
            .rev()
            .map(|e| (e.artifacts.clone(), e.claimed_output))
            .unzip();
        output_roots.push(keccak256(String::from("output 0")));
        let test_executor = TestExecutor {
            outcomes,
            output_roots,
        };

        // test without cache or collection target
        let mut cached_executor = CachedExecutor {
            cache: vec![],
            executor: test_executor.clone(),
            collection_target: None,
        };
        for execution in &executions {
            assert_eq!(
                cached_executor
                    .execute_payload(execution.attributes.clone())
                    .await
                    .unwrap()
                    .header,
                execution.artifacts.header.clone()
            );
            cached_executor.wait_until_ready().await;
            cached_executor.update_safe_head(execution.artifacts.header.clone());
        }

        // test with collection target
        let collection_target = Arc::new(Mutex::new(vec![]));
        let mut cached_executor = CachedExecutor {
            cache: vec![],
            executor: test_executor.clone(),
            collection_target: Some(collection_target.clone()),
        };
        for execution in &executions {
            assert_eq!(
                cached_executor
                    .execute_payload(execution.attributes.clone())
                    .await
                    .unwrap()
                    .header,
                execution.artifacts.header.clone()
            );
            {
                let collected = collection_target.lock().unwrap();
                assert_eq!(
                    collected.last().unwrap().artifacts.header,
                    execution.artifacts.header.clone()
                );
            }
            cached_executor.wait_until_ready().await;
            cached_executor.update_safe_head(execution.artifacts.header.clone());
        }

        // test with caching
        let collection_target = Arc::new(Mutex::new(vec![]));
        let cache = {
            let mut cache = executions.clone();
            cache.reverse();
            cache
        };
        let mut cached_executor = CachedExecutor {
            cache,
            executor: test_executor.clone(),
            collection_target: Some(collection_target.clone()),
        };
        for execution in &executions {
            assert_eq!(
                cached_executor
                    .execute_payload(execution.attributes.clone())
                    .await
                    .unwrap()
                    .header,
                execution.artifacts.header.clone()
            );
            {
                let collected = collection_target.lock().unwrap();
                assert!(collected.is_empty());
            }
            cached_executor.wait_until_ready().await;
            cached_executor.update_safe_head(execution.artifacts.header.clone());
        }

        // test with faulty caching
        let collection_target = Arc::new(Mutex::new(vec![]));
        let cache = executions.clone();
        let mut cached_executor = CachedExecutor {
            cache,
            executor: test_executor.clone(),
            collection_target: Some(collection_target.clone()),
        };
        for execution in &executions {
            assert_eq!(
                cached_executor
                    .execute_payload(execution.attributes.clone())
                    .await
                    .unwrap()
                    .header,
                execution.artifacts.header.clone()
            );
            cached_executor.wait_until_ready().await;
            cached_executor.update_safe_head(execution.artifacts.header.clone());
        }
        // only the last execution was a cache hit
        assert_eq!(
            collection_target.lock().unwrap().len(),
            executions.len() - 1
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    pub async fn test_execution_cursor() {
        // prepare oracle data
        let mut vec_oracle = VecOracle::default();
        let safe_head = Header {
            number: 0,
            ..Default::default()
        };
        let safe_head_hash = safe_head.hash_slow();
        vec_oracle.insert_preimage(
            PreimageKey::new_keccak256(safe_head_hash.0),
            alloy_rlp::encode(&safe_head),
        );
        vec_oracle.insert_preimage(
            PreimageKey::new_keccak256(TrieNode::Empty.blind().0),
            alloy_rlp::encode(&TrieNode::Empty),
        );
        vec_oracle.insert_preimage(
            PreimageKey::new_keccak256(safe_head_hash.0),
            alloy_rlp::encode(&safe_head),
        );
        // create cursor
        let rollup_config = Arc::new({
            let mut config = RollupConfig::default();
            config.genesis.l2.hash = safe_head_hash;
            config
        });
        let mut provider =
            OracleL2ChainProvider::new(safe_head_hash, rollup_config.clone(), Arc::new(vec_oracle));
        new_execution_cursor(rollup_config.as_ref(), safe_head.seal_slow(), &mut provider)
            .await
            .unwrap();
    }
}
