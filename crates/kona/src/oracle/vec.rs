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

use crate::client::log;
use crate::oracle::WitnessOracle;
use crate::oracle::{needs_validation, validate_preimage};
use crate::rkyv::vec::PreimageVecStoreRkyv;
use alloy_primitives::map::HashMap;
use anyhow::bail;
use async_trait::async_trait;
use kona_preimage::errors::PreimageOracleResult;
use kona_preimage::{HintWriterClient, PreimageKey, PreimageOracleClient};
use kona_proof::FlushableCache;
use lazy_static::lazy_static;
use std::collections::VecDeque;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex};
use tracing::info;

/// A type alias representing an indexed preimage.
///
/// This type is a tuple consisting of the following elements:
///
/// 1. `PreimageKey`:
///    - The key associated with the preimage, serving as its identifier or unique reference.
///
/// 2. `Vec<u8>`:
///    - The actual bytes representing the preimage content.
///
/// 3. `Option<(usize, usize)>`:
///    - An optional tuple specifying metadata about a duplicate of the preimage:
///        - The first `usize` represents the shard index.
///        - The second `usize` represents the index within the shard.
///    - If `None`, the position metadata is not available.
pub type IndexedPreimage = (PreimageKey, Vec<u8>, Option<(usize, usize)>);
pub type PreimageVecEntry = Vec<IndexedPreimage>;
pub type PreimageVecStore = Arc<Mutex<Vec<PreimageVecEntry>>>;

/// A structure representing a vector-based oracle for storing preimages.
///
/// This struct is equipped with the necessary implementations to support cloning, debugging,
/// and (de)serialization using the `rkyv` crate. It defines a storage for preimages
/// with additional serialization handling.
#[derive(Clone, Debug, Default, rkyv::Serialize, rkyv::Archive, rkyv::Deserialize)]
pub struct VecOracle {
    /// A `PreimageVecStore` instance that contains the stored preimages.
    #[rkyv(with = PreimageVecStoreRkyv)]
    pub preimages: PreimageVecStore,
}

impl VecOracle {
    /// Creates a deep clone of the current instance.
    ///
    /// This method performs a deep clone of the object, ensuring that all
    /// nested data structures or components shared via `Arc` or `Mutex` are
    /// also uniquely cloned. This is particularly relevant for structures
    /// where a simple `clone` would result in shared references instead of
    /// creating truly independent copies.
    ///
    /// # Returns
    ///
    /// A new instance of the same type, containing independent clones of all
    /// fields, including those wrapped in `Arc` and `Mutex`.
    ///
    /// # Notes
    ///
    /// - For this method to work correctly, the types within the struct must
    ///   also support cloning (e.g., contained elements must implement `Clone`).
    ///
    /// - This method is useful in concurrent programming scenarios where `Arc`
    ///   and `Mutex` are frequently used to provide shared access while ensuring
    ///   thread safety. A deep clone ensures that the new instance does not
    ///   share any mutable state with the original.
    pub fn deep_clone(&self) -> Self {
        let mut cloned_with_arc = self.clone();
        cloned_with_arc.preimages = Arc::new(Mutex::new(self.preimages.lock().unwrap().clone()));
        cloned_with_arc
    }

    /// Validates the collection of preimage vector entries.
    ///
    /// # Arguments
    ///
    /// * `preimages` - A slice of `PreimageVecEntry`, where each entry consists of a vector
    ///   containing tuples of key, value, and potentially a reference (`prev`) to a prior key-value pair.
    ///
    /// # Returns
    ///
    /// * `Result<()>` - Returns `Ok(())` if all validations pass, or an error wrapped in `anyhow::Error` if any validation fails.
    ///
    /// # Behavior
    ///
    /// 1. Iterates through each `PreimageVecEntry` in the `preimages` slice.
    /// 2. For each entry, iterates through its pairs, extracting the key, value, and optionally a `prev` reference.
    /// 3. Skips validation for keys where `needs_validation` indicates validation is not required.
    /// 4. If a `prev` reference is present:
    ///     - Ensures that the reference does not point to a future entry or an invalid sequence in the current entry.
    ///     - Validates referenced key-value matches the cached preimage.
    /// 5. If no `prev` reference exists, validates the current key and value using `validate_preimage`.
    ///
    /// # Errors
    ///
    /// This function returns an error in the following cases:
    /// - If a `prev` reference points to a future entry or preimage, violating causal consistency.
    /// - If the key or value of the current pair does not match the cached preimage at the referenced location.
    /// - If `validate_preimage` fails for any key-value pair requiring validation.
    ///
    /// # Notes
    ///
    /// - The function assumes `key` and `value` in a `PreimageVecEntry` are consistent types.
    /// - It is the caller's responsibility to populate `prev` references accurately to ensure valid
    ///   preimage relationships.
    ///
    /// # Dependencies
    ///
    /// Requires the following external functions:
    /// - `needs_validation(key_type: &KeyType) -> bool`: Determines whether a key type requires validation.
    /// - `validate_preimage(key: &Key, value: &Value) -> Result<()>`: Performs validation on a single key-value pair.
    ///
    /// # See Also
    ///
    /// This function is part of a broader mechanism for ensuring data integrity in cryptographic or
    /// state-based systems relying on preimages for verification.
    pub fn validate(preimages: &[PreimageVecEntry]) -> anyhow::Result<()> {
        for (e, entry) in preimages.iter().enumerate() {
            for (p, (key, value, prev)) in entry.iter().enumerate() {
                if !needs_validation(&key.key_type()) {
                    continue;
                } else if let Some((i, j)) = prev {
                    if e < *i {
                        bail!("Attempted to validate preimage against future vec entry.");
                    } else if e == *i && p <= *j {
                        bail!(
                            "Attempted to validate preimage against future preimage in vec entry."
                        );
                    } else if key != &preimages[*i][*j].0 {
                        bail!("Cached preimage key comparison failed");
                    } else if value != &preimages[*i][*j].1 {
                        bail!("Cached preimage value comparison failed");
                    } else {
                        continue;
                    }
                }
                validate_preimage(key, value)?;
            }
        }
        Ok(())
    }
}

impl WitnessOracle for VecOracle {
    fn preimage_count(&self) -> usize {
        self.preimages.lock().unwrap().iter().map(Vec::len).sum()
    }

    fn validate_preimages(&self) -> anyhow::Result<()> {
        let preimages = self.preimages.lock().unwrap();
        Self::validate(preimages.deref())
    }

    /// Inserts a preimage into the preimages collection.
    ///
    /// This method validates the given `key` and `value` before inserting them into the
    /// collection. If the validation fails, the function will panic with an error message
    /// "Attempted to save invalid preimage". The `preimages` collection is thread-safe
    /// through the use of a mutex.
    ///
    /// # Parameters
    ///
    /// - `key`: A `PreimageKey` representing the identifier for the preimage.
    /// - `value`: A `Vec<u8>` containing the data associated with the preimage.
    ///
    /// # Behavior
    ///
    /// - The preimage (a tuple of `key`, `value`, and `None`) is appended to the last
    ///   vector inside the `preimages` collection.
    /// - If the `preimages` collection is empty, a new inner vector is initialized before
    ///   the insertion takes place.
    ///
    /// # Panics
    ///
    /// This function will panic if:
    /// - The `validate_preimage` function determines that the provided `key` and `value`
    ///   are invalid.
    /// - The mutex guarding the `preimages` collection is poisoned (i.e., another thread
    ///   panicked while holding the lock).
    ///
    /// Notes:
    /// - Ensure that the provided `key` and `value` adhere to the expected format, as
    ///   enforced by `validate_preimage`.
    /// - This method is not thread-safe on its own, so ensure that concurrent access
    ///   to the containing structure is properly synchronized if needed.
    fn insert_preimage(&mut self, key: PreimageKey, value: Vec<u8>) {
        validate_preimage(&key, &value).expect("Attempted to save invalid preimage");
        let mut preimages = self.preimages.lock().unwrap();
        if preimages.is_empty() {
            preimages.push(Vec::new());
        }
        preimages.last_mut().unwrap().push((key, value, None));
    }

    /// Finalizes pre-images by validating them, sorting, sharding, and optionally adding validation pointers.
    ///
    /// # Arguments
    /// - `shard_size` - Specifies the maximum size limit for each shard of pre-images.
    /// - `with_validation_ptrs` - A boolean flag to determine whether validation pointers should be added.
    ///
    /// # Process
    /// 1. Validates all existing pre-images. Panics if validation fails.
    /// 2. Flattens and sorts the pre-image data. This includes reversing the order to optimize expected access.
    /// 3. Splits the flattened pre-images into shards, each fitting within the given `shard_size`.
    /// 4. If `with_validation_ptrs` is `true`, adds validation pointers to pre-images where necessary:
    ///     - Maintains a cache for already processed pre-images.
    ///     - Assigns pointers to link pre-images that require validation.
    ///
    /// # Panics
    /// This function will panic if the validation of pre-images fails during the call to `validate_preimages`.
    ///
    /// # Logs
    /// Logs the number of pre-images, shard size, and whether validation pointers are included (`with_validation_ptrs`) at the start of finalization.
    ///
    /// # Notes
    /// - Sharding ensures that no shard exceeds the given `shard_size` by aggregating pre-images until the limit is reached.
    /// - Only pre-images requiring validation, as determined by `needs_validation`, will have validation pointers added.
    fn finalize_preimages(&mut self, shard_size: usize, with_validation_ptrs: bool) {
        self.validate_preimages()
            .expect("Failed to validate preimages during finalization");
        let mut preimages = self.preimages.lock().unwrap();
        // flatten and sort
        let mut flat_vec = core::mem::take(preimages.deref_mut())
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        info!("Finalizing {} preimages with shard size {shard_size} and validation ptrs {with_validation_ptrs}", flat_vec.len());
        // sort by expected access
        flat_vec.reverse();
        // shard vectors by size limit
        let mut sharded_vec = vec![vec![]];
        let mut last_shard_size = 0;
        for value in flat_vec {
            if value.1.len() + last_shard_size > shard_size && last_shard_size > 0 {
                sharded_vec.push(vec![]);
                last_shard_size = 0;
            }
            last_shard_size += value.1.len();
            sharded_vec.last_mut().unwrap().push(value);
        }
        let _ = core::mem::replace(preimages.deref_mut(), sharded_vec);
        // add validation pointers
        if !with_validation_ptrs {
            return;
        }
        let mut cache: HashMap<PreimageKey, (usize, usize)> =
            HashMap::with_capacity(preimages.len());
        for (i, entry) in preimages.iter_mut().enumerate() {
            for (j, (key, _, pointer)) in entry.iter_mut().enumerate() {
                if !needs_validation(&key.key_type()) {
                    continue;
                } else if let Some(prev) = cache.insert(*key, (i, j)) {
                    pointer.replace(prev);
                }
            }
        }
    }
}

impl FlushableCache for VecOracle {
    fn flush(&self) {}
}

/// A type alias for a queue structure that stores `IndexedPreimage` elements.
pub type PreimageQueue = VecDeque<IndexedPreimage>;

lazy_static! {
    /// An object used for temporary storage of out-of-order preimages accessed randomly.
    static ref QUEUE: Arc<Mutex<PreimageQueue>> = Default::default();
}

#[async_trait]
impl PreimageOracleClient for VecOracle {
    /// Asynchronously retrieves a preimage for a given `key` using a `PreimageOracle`.
    ///
    /// # Arguments
    ///
    /// * `key` - The `PreimageKey` for which the associated preimage data is being sought.
    ///
    /// # Returns
    ///
    /// Returns a `PreimageOracleResult<Vec<u8>>` containing the corresponding preimage data
    /// if found, or an error if the lookup fails or the preimage is not available.
    ///
    /// # Logic
    ///
    /// - The method works with a locked mutable reference to `self.preimages`, which holds
    ///   precomputed preimages, and a global `QUEUE` used for temporarily queuing key-value
    ///   pairs for lookup.
    /// - Key preimages are validated and processed using various conditions governed by
    ///   target operating system configurations (`zkvm` vs. non-`zkvm` environments).
    ///     - On `zkvm` targets, if the preimage vector is empty, it logs a message, attempts
    ///       to deserialize a shard, validates the deserialized preimages, and adds them to
    ///       the preimage vector for further access.
    ///     - On non-`zkvm` targets, the function panics if the preimage vector is empty, with
    ///       a message indicating exhaustion of preimages in the oracle queue.
    /// - The outer loop checks and processes preimage vector entries until the desired preimage
    ///   is found or the vector is empty.
    /// - The inner loop iterates through preimage entries. If the desired key matches, the
    ///   associated preimage value is returned. Otherwise, the entry is shifted into the
    ///   temporary queue for later use.
    ///
    /// # Notes
    ///
    /// - Any variations in memory access operations or hashing requirements related to hash
    ///   maps are carefully handled, ensuring correctness and avoiding runtime errors.
    /// - Logging is triggered when the temporary queue is non-empty to inform about
    ///   queued elements.
    /// - If deserialization or validation operations fail on `zkvm` targets, the function
    ///   panics to notify an error in streamed shard processing.
    ///
    /// # Panics
    ///
    /// - If the method is called on a non-`zkvm` target and the preimages vector is empty, it
    ///   will panic with an appropriate error message.
    /// - If the shard validation fails (on `zkvm` targets), the function panics with a
    ///   descriptive message.
    ///
    /// # Configuration
    ///
    /// This function behavior depends on the target OS:
    /// - On `zkvm` targets: Processes shard deserialization and validation.
    /// - On non-`zkvm` targets: Panics when the preimages vector is depleted.
    async fn get(&self, key: PreimageKey) -> PreimageOracleResult<Vec<u8>> {
        let mut preimages = self.preimages.lock().unwrap();
        let mut queue = QUEUE.lock().unwrap();
        // handle variations in memory access operations due to hashmap usages
        loop {
            if preimages.is_empty() {
                #[cfg(target_os = "zkvm")]
                {
                    crate::client::log("DESERIALIZE STREAMED SHARD");
                    preimages.push(read_shard());
                    Self::validate(preimages.as_ref())
                        .expect("Failed to validate streamed preimages");
                    crate::client::log("STREAMED SHARD VALIDATED");
                }
                #[cfg(not(target_os = "zkvm"))]
                panic!(
                    "Exhausted VecOracle seeking {key} ({} queued preimages)",
                    queue.len()
                )
            }

            let entry = preimages.last_mut().unwrap();
            loop {
                let Some((last_key, value, _)) = entry.pop() else {
                    break;
                };

                if key == last_key {
                    if !queue.is_empty() {
                        log(&format!("TEMP ELEMENTS: {}", queue.len()));
                        entry.extend(core::mem::take(queue.deref_mut()));
                    }

                    return Ok(value);
                }
                // keep entry in queue for later use, pointer is no longer necessary
                queue.push_front((last_key, value, None));
            }
            preimages.pop();
        }
    }

    /// Asynchronously retrieves an exact preimage value for the given key and copies it into the provided buffer.
    ///
    /// # Parameters
    /// - `key`: The `PreimageKey` for which the preimage value is to be retrieved.
    /// - `buf`: A mutable byte slice to store the retrieved preimage value. The buffer must be large enough
    ///   to fit the retrieved value; otherwise, this method will panic.
    ///
    /// # Returns
    /// - `Ok(())` if the value is successfully retrieved and copied into the buffer.
    /// - `Err(PreimageOracleError)` if an error occurs while retrieving the value (e.g., the key is not found).
    ///
    /// # Errors
    /// This function will return an error if the underlying `get` method fails to retrieve the value associated
    /// with the provided key.
    ///
    /// # Panics
    /// This function will panic if the size of the given buffer does not match the size of the retrieved value.
    ///
    /// # Notes
    /// This function assumes that the size of the `buf` matches the size of the preimage value.
    /// Ensure the buffer is allocated with the correct size to avoid panics.
    async fn get_exact(&self, key: PreimageKey, buf: &mut [u8]) -> PreimageOracleResult<()> {
        let value = self.get(key).await?;
        buf.copy_from_slice(value.as_ref());
        Ok(())
    }
}

#[async_trait]
impl HintWriterClient for VecOracle {
    /// Asynchronously writes data or performs an operation based on the provided hint.
    ///
    /// This function serves as a placeholder implementation that currently does nothing
    /// and always returns `Ok(())`.
    ///
    /// # Notes
    /// This function is currently a no-op and may be extended in the future to perform
    /// meaningful write operations based on the provided hint.
    async fn write(&self, _hint: &str) -> PreimageOracleResult<()> {
        Ok(())
    }
}

/// Reads and deserializes a shard into a `PreimageVecEntry` structure.
///
/// This function retrieves binary data representing a serialized shard from the environment.
/// It then attempts to deserialize the binary data into a `PreimageVecEntry` instance using the
/// `rkyv` deserialization framework. If the deserialization process fails, the function will panic
/// with an error message.
///
/// # Returns
/// - A `PreimageVecEntry` object that represents the deserialized shard.
///
/// # Panics
/// - The function panics if deserialization fails, with the message `"Failed to deserialize shard"`.
///
/// # Dependencies
/// This function uses:
/// - `env::read_frame()` to read binary data from the environment.
/// - `rkyv::from_bytes` for deserialization of the binary data into a `PreimageVecEntry`.
///
/// Ensure that the environment contains valid binary data for a `PreimageVecEntry` structure before
/// calling this function.
#[cfg(target_os = "zkvm")]
pub fn read_shard() -> PreimageVecEntry {
    let shard_data = risc0_zkvm::guest::env::read_frame();
    rkyv::from_bytes::<PreimageVecEntry, rkyv::rancor::Error>(&shard_data)
        .expect("Failed to deserialize shard")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod tests {
    use super::*;
    use alloy_primitives::keccak256;
    use kona_preimage::PreimageKeyType;
    use kona_proof::block_on;
    use risc0_zkvm::sha::{Impl as SHA2, Sha256};
    use rkyv::rancor::Error;
    use std::collections::HashSet;

    pub fn prepare_vec_oracle(value_count: usize, copies: usize) -> (VecOracle, Vec<Vec<u8>>) {
        QUEUE.clear_poison();
        let mut oracle = VecOracle::default();
        assert_eq!(oracle.preimage_count(), 0);

        let values = (0..value_count)
            .map(|i| format!("{i} test {i} value {i}").as_bytes().to_vec())
            .collect::<Vec<_>>();
        // insert sha3 keys
        for value in &values {
            let sha3_key = PreimageKey::new_keccak256(keccak256(value).0);
            for _ in 0..copies {
                oracle.insert_preimage(sha3_key, value.clone());
            }
        }
        oracle.validate_preimages().unwrap();
        assert_eq!(oracle.preimage_count(), values.len() * copies);
        // insert sha2 keys
        for value in &values {
            let sha2_key = PreimageKey::new(
                SHA2::hash_bytes(value).as_bytes().try_into().unwrap(),
                PreimageKeyType::Sha256,
            );
            for _ in 0..copies {
                oracle.insert_preimage(sha2_key, value.clone());
            }
        }
        oracle.validate_preimages().unwrap();
        assert_eq!(oracle.preimage_count(), values.len() * copies * 2);

        (oracle, values)
    }

    pub async fn exhaust_vec_oracle(copies: usize, oracle: VecOracle, values: Vec<Vec<u8>>) {
        let initial_size = oracle.preimage_count();
        for value in values.iter().rev() {
            let sha3_key = PreimageKey::new_keccak256(keccak256(value).0);
            let sha2_key = PreimageKey::new(
                SHA2::hash_bytes(value).as_bytes().try_into().unwrap(),
                PreimageKeyType::Sha256,
            );
            for _ in 0..copies {
                let mut sha3_val = vec![0u8; value.len()];
                oracle.get_exact(sha3_key, &mut sha3_val).await.unwrap();
                let mut sha2_val = vec![0u8; value.len()];
                oracle.get_exact(sha2_key, &mut sha2_val).await.unwrap();
                assert_eq!(sha3_val, sha2_val);
            }
        }
        // ensure exhaustion
        assert_eq!(
            oracle.preimage_count(),
            initial_size - 2 * copies * values.len()
        );
    }

    #[tokio::test]
    async fn test_deep_clone() {
        let (mut oracle, values) = prepare_vec_oracle(1024, 3);
        oracle.insert_preimage(
            PreimageKey::new([0xff; 32], PreimageKeyType::Local),
            vec![0xff; 32],
        );
        oracle.finalize_preimages(1, true);
        oracle.validate_preimages().unwrap();
        // assert initial equivalence
        let size = oracle.preimage_count();
        let cloned = oracle.deep_clone();
        assert_eq!(size, cloned.preimage_count());
        // regular cloning vs deep cloning
        exhaust_vec_oracle(3, oracle.clone(), values).await;
        assert_eq!(oracle.preimage_count(), 1);
        assert_eq!(size, cloned.preimage_count());
    }

    #[tokio::test]
    async fn test_vec_oracle_sharded() {
        let (mut oracle, values) = prepare_vec_oracle(1024, 1);
        // one key per shard
        oracle.finalize_preimages(1, true);
        oracle.validate_preimages().unwrap();
        // serde
        let oracle = rkyv::from_bytes::<VecOracle, Error>(
            rkyv::to_bytes::<Error>(&oracle).unwrap().as_ref(),
        )
        .unwrap();
        // validate
        {
            let preimage_vecs = oracle.preimages.lock().unwrap();
            assert_eq!(preimage_vecs.len(), values.len() * 2);
            for preimages in preimage_vecs.iter() {
                assert_eq!(preimages.len(), 1);
                for preimage in preimages.iter() {
                    assert_eq!(preimage.2, None);
                }
            }
        }
        // retrieve keys
        exhaust_vec_oracle(1, oracle, values).await;
    }

    #[tokio::test]
    async fn test_vec_oracle_unsharded() {
        let (mut oracle, values) = prepare_vec_oracle(1024, 1);
        // one shard for all keys
        oracle.finalize_preimages(usize::MAX, true);
        oracle.validate_preimages().unwrap();
        // serde
        let oracle = rkyv::from_bytes::<VecOracle, Error>(
            rkyv::to_bytes::<Error>(&oracle).unwrap().as_ref(),
        )
        .unwrap();
        // validate
        {
            let preimage_vecs = oracle.preimages.lock().unwrap();
            assert_eq!(preimage_vecs.len(), 1);
            for preimages in preimage_vecs.iter() {
                assert_eq!(preimages.len(), values.len() * 2);
                for preimage in preimages.iter() {
                    assert_eq!(preimage.2, None);
                }
            }
        }
        // retrieve keys
        exhaust_vec_oracle(1, oracle, values).await;
    }

    #[tokio::test]
    async fn test_vec_oracle_duplicates_sharded() {
        let (mut oracle, values) = prepare_vec_oracle(1024, 2);
        // one key per shard
        oracle.finalize_preimages(1, true);
        oracle.validate_preimages().unwrap();
        // serde
        let oracle = rkyv::from_bytes::<VecOracle, Error>(
            rkyv::to_bytes::<Error>(&oracle).unwrap().as_ref(),
        )
        .unwrap();
        // validate
        {
            let preimage_vecs = oracle.preimages.lock().unwrap();
            assert_eq!(preimage_vecs.len(), values.len() * 2 * 2);
            let mut seen_keys = HashSet::new();
            for preimages in preimage_vecs.iter() {
                assert_eq!(preimages.len(), 1);
                for preimage in preimages.iter() {
                    if seen_keys.contains(&preimage.0) {
                        let ptr = preimage.2.unwrap();
                        assert_eq!(&preimage_vecs[ptr.0][ptr.1].0, &preimage.0);
                    } else {
                        assert!(preimage.2.is_none());
                        seen_keys.insert(preimage.0);
                    }
                }
            }
        }
        // retrieve keys
        exhaust_vec_oracle(2, oracle, values).await;
    }

    #[tokio::test]
    async fn test_vec_oracle_duplicates_unsharded() {
        let (mut oracle, values) = prepare_vec_oracle(1024, 2);
        // one shard
        oracle.finalize_preimages(usize::MAX, true);
        oracle.validate_preimages().unwrap();
        // serde
        let oracle = rkyv::from_bytes::<VecOracle, Error>(
            rkyv::to_bytes::<Error>(&oracle).unwrap().as_ref(),
        )
        .unwrap();
        // validate
        {
            let preimage_vecs = oracle.preimages.lock().unwrap();
            assert_eq!(preimage_vecs.len(), 1);
            let mut seen_keys = HashSet::new();
            for preimages in preimage_vecs.iter() {
                assert_eq!(preimages.len(), values.len() * 2 * 2);
                for preimage in preimages.iter() {
                    if seen_keys.contains(&preimage.0) {
                        let ptr = preimage.2.unwrap();
                        assert_eq!(&preimage_vecs[ptr.0][ptr.1].0, &preimage.0);
                    } else {
                        assert!(preimage.2.is_none());
                        seen_keys.insert(preimage.0);
                    }
                }
            }
        }
        // retrieve keys
        exhaust_vec_oracle(2, oracle, values).await;
    }

    #[tokio::test]
    async fn test_vec_oracle_duplicates_unsharded_no_cache() {
        let (mut oracle, values) = prepare_vec_oracle(1024, 2);
        // one shard
        oracle.finalize_preimages(usize::MAX, false);
        oracle.validate_preimages().unwrap();
        // serde
        let oracle = rkyv::from_bytes::<VecOracle, Error>(
            rkyv::to_bytes::<Error>(&oracle).unwrap().as_ref(),
        )
        .unwrap();
        // validate
        {
            let preimage_vecs = oracle.preimages.lock().unwrap();
            assert_eq!(preimage_vecs.len(), 1);
            for preimages in preimage_vecs.iter() {
                assert_eq!(preimages.len(), values.len() * 2 * 2);
                for preimage in preimages.iter() {
                    assert!(preimage.2.is_none());
                }
            }
        }
        // retrieve keys
        exhaust_vec_oracle(2, oracle, values).await;
    }

    #[test]
    fn test_vec_oracle_tamper() {
        let (mut oracle, _) = prepare_vec_oracle(1, 4);
        // one key pre shard
        oracle.finalize_preimages(1, true);
        oracle.validate_preimages().unwrap();

        // point first entry to future entry
        {
            let oracle = oracle.deep_clone();
            {
                let mut preimages = oracle.preimages.lock().unwrap();
                let preimage_vec = preimages.first_mut().unwrap();
                let preimage = preimage_vec.first_mut().unwrap();
                preimage.2 = Some((1, 0));
            }
            // fail to validate
            let result = oracle.validate_preimages().unwrap_err();
            assert!(result.to_string().contains("future vec entry"));
        }
        // point first entry to self
        {
            let oracle = oracle.deep_clone();
            {
                let mut preimages = oracle.preimages.lock().unwrap();
                let preimage_vec = preimages.first_mut().unwrap();
                let preimage = preimage_vec.first_mut().unwrap();
                preimage.2 = Some((0, 0));
            }
            // fail to validate
            let result = oracle.validate_preimages().unwrap_err();
            assert!(result.to_string().contains("future preimage"));
        }
        // invalidate key
        {
            let oracle = oracle.deep_clone();
            {
                let mut preimages = oracle.preimages.lock().unwrap();
                let preimage_vec = preimages.first_mut().unwrap();
                let preimage = preimage_vec.first_mut().unwrap();
                preimage.0 = PreimageKey::new([0xff; 32], PreimageKeyType::Local);
            }
            // fail to validate
            let result = oracle.validate_preimages().unwrap_err();
            assert!(result.to_string().contains("key comparison failed"));
        }
        // invalidate value
        {
            let oracle = oracle.deep_clone();
            {
                let mut preimages = oracle.preimages.lock().unwrap();
                let preimage_vec = preimages.last_mut().unwrap();
                let preimage = preimage_vec.first_mut().unwrap();
                preimage.1 = vec![0xff; 32];
            }
            // fail to validate
            let result = oracle.validate_preimages().unwrap_err();
            assert!(result.to_string().contains("value comparison failed"));
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_exhaustion() {
        let (mut oracle, values) = prepare_vec_oracle(1, 1);
        oracle.finalize_preimages(usize::MAX, true);
        oracle.validate_preimages().unwrap();
        // fail to refetch key after exhaustion
        let only_key = oracle
            .preimages
            .lock()
            .unwrap()
            .first()
            .unwrap()
            .first()
            .unwrap()
            .0;
        exhaust_vec_oracle(1, oracle.clone(), values).await;
        assert!(std::panic::catch_unwind(|| block_on(oracle.get(only_key))).is_err());
        // clear position state
        assert!(oracle.preimages.is_poisoned());
        QUEUE.clear_poison();
    }

    #[tokio::test]
    async fn test_noop() {
        let oracle = prepare_vec_oracle(0, 0).0;
        oracle.write("noop").await.unwrap();
        oracle.flush();
        assert_eq!(oracle.preimage_count(), 0);
    }
}
