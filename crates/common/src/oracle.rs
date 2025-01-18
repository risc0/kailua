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
use alloy_primitives::map::HashMap;
use async_trait::async_trait;
use kona_preimage::errors::{PreimageOracleError, PreimageOracleResult};
use kona_preimage::{HintWriterClient, PreimageKey, PreimageKeyType, PreimageOracleClient};
use kona_proof::FlushableCache;
use risc0_zkvm::sha::{Impl as SHA2, Sha256};
use std::hash::{BuildHasher, Hasher};

pub type PreimageStore = HashMap<PreimageKey, Vec<u8>, NoMapHasher<31>>;

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
pub struct PreloadedOracle {
    pub preimages: PreimageStore,
}

impl PreloadedOracle {
    pub fn validate(&self) -> PreimageOracleResult<()> {
        for (key, value) in &self.preimages {
            validate_preimage(key, value)?;
        }
        Ok(())
    }
}

pub fn validate_preimage(key: &PreimageKey, value: &[u8]) -> PreimageOracleResult<()> {
    let key_type = key.key_type();
    let image = match key_type {
        PreimageKeyType::Keccak256 => Some(keccak256(value).0),
        PreimageKeyType::Sha256 => {
            let x = SHA2::hash_bytes(value);
            Some(x.as_bytes().try_into().unwrap())
        }
        PreimageKeyType::Precompile => {
            unimplemented!("Precompile acceleration is not yet supported.");
        }
        PreimageKeyType::Blob => {
            unreachable!("Blob key types should not be witnessed.");
        }
        PreimageKeyType::Local | PreimageKeyType::GlobalGeneric => None,
    };
    if let Some(image) = image {
        if key != &PreimageKey::new(image, key_type) {
            return Err(PreimageOracleError::InvalidPreimageKey);
        }
    }
    Ok(())
}

impl FlushableCache for PreloadedOracle {
    fn flush(&self) {}
}

#[async_trait]
impl PreimageOracleClient for PreloadedOracle {
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
impl HintWriterClient for PreloadedOracle {
    async fn write(&self, _hint: &str) -> PreimageOracleResult<()> {
        Ok(())
    }
}

#[derive(Clone, Default)]
pub struct NoMapHasher<const L: usize>;

impl<const L: usize> BuildHasher for NoMapHasher<L> {
    type Hasher = NoHasher<L>;

    fn build_hasher(&self) -> Self::Hasher {
        NoHasher::<L>::default()
    }
}

#[derive(Default)]
pub struct NoHasher<const L: usize>([u8; 8]);

impl<const L: usize> Hasher for NoHasher<L> {
    fn finish(&self) -> u64 {
        u64::from_be_bytes(self.0)
    }

    fn write(&mut self, bytes: &[u8]) {
        if bytes.len() != L {
            return;
        }
        self.0[..].copy_from_slice(&bytes[L - 8..]);
    }
}
