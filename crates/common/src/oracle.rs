// Copyright 2024 RISC Zero, Inc.
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

use alloy_primitives::keccak256;
use async_trait::async_trait;
use kona_preimage::errors::PreimageOracleResult;
use kona_preimage::{HintWriterClient, PreimageKey, PreimageKeyType, PreimageOracleClient};
use kona_proof::FlushableCache;
use risc0_zkvm::sha::{Impl as SHA2, Sha256};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OracleWitnessData {
    pub data: Vec<Vec<u8>>,
    pub keys: Vec<PreimageKey>,
}

pub type PreimageStore = Arc<Mutex<Vec<(PreimageKey, Vec<u8>)>>>;

#[derive(Clone, Debug, Default)]
pub struct PreloadedOracle {
    preimages: PreimageStore,
}

impl From<OracleWitnessData> for PreloadedOracle {
    fn from(witness: OracleWitnessData) -> Self {
        let preimages = core::iter::zip(witness.keys, witness.data)
            .rev()
            .map(|(key, value)| {
                let key_type = key.key_type();
                let image = match key_type {
                    PreimageKeyType::Keccak256 => Some(keccak256(&value).0),
                    PreimageKeyType::Sha256 => {
                        let x = SHA2::hash_bytes(&value);
                        Some(x.as_bytes().try_into().unwrap())
                    }
                    PreimageKeyType::Precompile => {
                        unimplemented!("Precompile acceleration not yet supported");
                    }
                    PreimageKeyType::Local
                    | PreimageKeyType::GlobalGeneric
                    | PreimageKeyType::Blob => None,
                };
                if let Some(image) = image {
                    assert_eq!(key, PreimageKey::new(image, key_type));
                }
                (key, value)
            })
            .collect();
        Self {
            preimages: Arc::new(Mutex::new(preimages)),
        }
    }
}

impl FlushableCache for PreloadedOracle {
    fn flush(&self) {}
}

#[async_trait]
impl PreimageOracleClient for PreloadedOracle {
    async fn get(&self, key: PreimageKey) -> PreimageOracleResult<Vec<u8>> {
        let mut preimages = self.preimages.lock().unwrap();
        loop {
            let (k, v) = preimages.pop().unwrap();
            if k == key {
                break Ok(v);
            }
        }
    }

    async fn get_exact(&self, key: PreimageKey, buf: &mut [u8]) -> PreimageOracleResult<()> {
        let v = self.get(key).await?;
        buf.copy_from_slice(v.as_slice());
        Ok(())
    }
}

#[async_trait]
impl HintWriterClient for PreloadedOracle {
    async fn write(&self, _hint: &str) -> PreimageOracleResult<()> {
        Ok(())
    }
}
