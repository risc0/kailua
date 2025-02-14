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
use crate::witness::WitnessOracle;
use alloy_primitives::map::HashMap;
use async_trait::async_trait;
use kona_preimage::errors::PreimageOracleResult;
use kona_preimage::{HintWriterClient, PreimageKey, PreimageOracleClient};
use kona_proof::FlushableCache;

pub type MapPreimageStore = HashMap<PreimageKey, Vec<u8>>;

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

    fn finalize_preimages(&mut self, _: usize) {
        self.validate_preimages()
            .expect("Failed to validate preimages during finalization");
    }
}

impl FlushableCache for MapOracle {
    fn flush(&self) {}
}

#[async_trait]
impl PreimageOracleClient for MapOracle {
    async fn get(&self, key: PreimageKey) -> PreimageOracleResult<Vec<u8>> {
        let Some(value) = self.preimages.get(&key) else {
            panic!("Preimage key must exist.");
        };
        Ok(value.clone())
    }

    async fn get_exact(&self, key: PreimageKey, buf: &mut [u8]) -> PreimageOracleResult<()> {
        let v = self.get(key).await?;
        buf.copy_from_slice(v.as_slice());
        Ok(())
    }
}

#[async_trait]
impl HintWriterClient for MapOracle {
    async fn write(&self, _hint: &str) -> PreimageOracleResult<()> {
        Ok(())
    }
}
