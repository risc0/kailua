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

use crate::oracle::validate_preimage;
use crate::oracle::WitnessOracle;
use alloy_primitives::map::HashMap;
use async_trait::async_trait;
use kona_preimage::errors::PreimageOracleResult;
use kona_preimage::{HintWriterClient, PreimageKey, PreimageOracleClient};
use kona_proof::FlushableCache;

/// A type alias for a `HashMap` that maps a `PreimageKey` to a `Vec<u8>`.
///
/// This structure is used to store preimages, where:
/// - `PreimageKey` is the key associated with the preimage.
/// - `Vec<u8>` represents the associated preimage data as a byte vector.
pub type MapPreimageStore = HashMap<PreimageKey, Vec<u8>>;

/// `MapOracle` is a data structure that represents a mapping oracle.
///
/// This structure is used to store and manage preimages and supports
/// serialization and deserialization using various serialization frameworks,
/// making it easy to persist and transmit the data.
///
/// ## Derive Attributes
/// - `Clone`: Provides the ability to create a copy of a `MapOracle` instance.
/// - `Debug`: Allows formatting the instance with the `{:?}` formatter for debugging purposes.
/// - `Default`: Enables the creation of a default instance of `MapOracle`.
/// - `serde::Serialize` / `serde::Deserialize`: Enables serialization and deserialization
///   using the Serde library.
/// - `rkyv::Serialize`, `rkyv::Archive`, `rkyv::Deserialize`: Adds support for zero-copy
///   serialization and deserialization using the `rkyv` library.
///
/// ## Fields
/// - `preimages`: A `MapPreimageStore` instance, which holds the preimages associated
///   with the map oracle. This is the central data store within the `MapOracle`.
#[derive(
    Clone,
    Debug,
    Default,
    serde::Serialize,
    serde::Deserialize,
    rkyv::Serialize,
    rkyv::Archive,
    rkyv::Deserialize,
)]
pub struct MapOracle {
    pub preimages: MapPreimageStore,
}

impl WitnessOracle for MapOracle {
    fn preimage_count(&self) -> usize {
        self.preimages.len()
    }

    fn validate_preimages(&self) -> anyhow::Result<()> {
        for (key, value) in &self.preimages {
            validate_preimage(key, value)?;
        }
        Ok(())
    }

    fn insert_preimage(&mut self, key: PreimageKey, value: Vec<u8>) {
        validate_preimage(&key, &value).expect("Attempted to save invalid preimage");
        if let Some(existing) = self.preimages.insert(key, value.clone()) {
            assert_eq!(
                existing,
                value,
                "Attempted to overwrite oracle data for key {}.",
                key.key_value()
            );
        };
    }

    fn finalize_preimages(&mut self, _: usize, _: bool) {
        self.validate_preimages()
            .expect("Failed to validate preimages during finalization");
    }
}

impl FlushableCache for MapOracle {
    fn flush(&self) {}
}

#[async_trait]
impl PreimageOracleClient for MapOracle {
    /// Retrieves the preimage associated with the given key from the `preimages` map.
    ///
    /// # Arguments
    ///
    /// * `key` - A `PreimageKey` representing the key for which the associated preimage is to be retrieved.
    ///
    /// # Returns
    ///
    /// Returns an `Ok(Vec<u8>)` containing a cloned version of the preimage associated with the provided key.
    /// If the key does not exist in the `preimages` map, the function will panic with the message:
    /// "Preimage key must exist."
    ///
    /// # Panics
    ///
    /// The function will panic if the provided key does not exist in the `preimages` map.
    ///
    async fn get(&self, key: PreimageKey) -> PreimageOracleResult<Vec<u8>> {
        let Some(value) = self.preimages.get(&key) else {
            panic!("Preimage key must exist.");
        };
        Ok(value.clone())
    }

    /// Retrieves the exact preimage data for the given key and writes it into the provided buffer.
    ///
    /// This asynchronous function fetches the value associated with the specified `PreimageKey`
    /// by calling the `get` method, and then copies the fetched value into the given mutable
    /// buffer. The buffer must be of sufficient size to hold the retrieved data; otherwise,
    /// this operation will panic.
    ///
    /// # Parameters
    ///
    /// * `key`: A `PreimageKey` that represents the key associated with the desired preimage data.
    /// * `buf`: A mutable byte slice to which the retrieved preimage data will be written.
    ///
    /// # Returns
    ///
    /// Returns a `PreimageOracleResult<()>` which resolves successfully if the data is retrieved
    /// and copied into the buffer. If an error occurs during the retrieval, the result contains
    /// the corresponding error.
    ///
    /// # Errors
    ///
    /// This function propagates any error encountered during the `get` operation through
    /// the `PreimageOracleResult`. Also, if the provided buffer is not large enough to
    /// accommodate the retrieved data, this function will panic.
    async fn get_exact(&self, key: PreimageKey, buf: &mut [u8]) -> PreimageOracleResult<()> {
        let v = self.get(key).await?;
        buf.copy_from_slice(v.as_slice());
        Ok(())
    }
}

#[async_trait]
impl HintWriterClient for MapOracle {
    /// Asynchronously handles the writing process with a given hint.
    ///
    /// # Parameters
    /// - `_hint`: A string reference that provides a hint or context for the write operation.
    ///   (Currently unused in the provided implementation.)
    ///
    /// # Notes
    /// - This implementation currently performs no meaningful actions other than returning `Ok(())`.
    async fn write(&self, _hint: &str) -> PreimageOracleResult<()> {
        Ok(())
    }
}
