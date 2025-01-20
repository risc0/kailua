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

pub mod map;
pub mod vec;

use alloy_primitives::keccak256;
use kona_preimage::errors::{PreimageOracleError, PreimageOracleResult};
use kona_preimage::{PreimageKey, PreimageKeyType};
use risc0_zkvm::sha::{Impl as SHA2, Sha256};

/// Recomputes the [PreimageKey] for a piece of data to validate its authenticity
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
            unreachable!("Blob key types should not be validated.");
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
