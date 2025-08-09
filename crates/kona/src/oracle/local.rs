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

use alloy_evm::revm::primitives::hash_map::Entry;
use alloy_primitives::map::HashMap;
use async_trait::async_trait;
use kona_preimage::errors::PreimageOracleResult;
use kona_preimage::{
    CommsClient, HintWriterClient, PreimageKey, PreimageKeyType, PreimageOracleClient,
};
use kona_proof::FlushableCache;
use spin::Mutex;
use std::fmt::Debug;
use std::sync::Arc;

/// Ensures the prover cannot change unauthenticated local key values
#[derive(Clone, Debug)]
pub struct LocalOnceOracle<O: CommsClient + FlushableCache + Send + Sync + Debug> {
    pub oracle: Arc<O>,
    pub cache: Arc<Mutex<HashMap<PreimageKey, Vec<u8>>>>,
}

impl<O: CommsClient + FlushableCache + Send + Sync + Debug> LocalOnceOracle<O> {
    pub fn new(oracle: Arc<O>) -> Self {
        Self {
            oracle,
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl<O: CommsClient + FlushableCache + Send + Sync + Debug> PreimageOracleClient
    for LocalOnceOracle<O>
{
    async fn get(&self, key: PreimageKey) -> PreimageOracleResult<Vec<u8>> {
        // Bypass cache for non-local keys
        if !matches!(key.key_type(), PreimageKeyType::Local) {
            return self.oracle.get(key).await;
        }
        // Make sure local key values can only be fetched once
        let mut cache = self.cache.lock();
        match cache.entry(key) {
            Entry::Occupied(entry) => Ok(entry.get().clone()),
            Entry::Vacant(vacancy) => {
                let result = self.oracle.get(key).await?;
                vacancy.insert(result.clone());
                Ok(result)
            }
        }
    }

    async fn get_exact(&self, key: PreimageKey, buf: &mut [u8]) -> PreimageOracleResult<()> {
        // Bypass cache for non-local keys
        if !matches!(key.key_type(), PreimageKeyType::Local) {
            return self.oracle.get_exact(key, buf).await;
        }
        // Make sure local key values can only be fetched once
        let mut cache = self.cache.lock();
        match cache.entry(key) {
            Entry::Occupied(entry) => {
                let result = entry.get();
                buf.copy_from_slice(result.as_slice());
            }
            Entry::Vacant(vacancy) => {
                self.oracle.get_exact(key, buf).await?;
                vacancy.insert(buf.to_vec());
            }
        }
        Ok(())
    }
}

#[async_trait]
impl<O: CommsClient + FlushableCache + Send + Sync + Debug> HintWriterClient
    for LocalOnceOracle<O>
{
    async fn write(&self, hint: &str) -> PreimageOracleResult<()> {
        self.oracle.write(hint).await
    }
}

#[async_trait]
impl<O: CommsClient + FlushableCache + Send + Sync + Debug> FlushableCache for LocalOnceOracle<O> {
    fn flush(&self) {
        self.oracle.flush();
    }
}
