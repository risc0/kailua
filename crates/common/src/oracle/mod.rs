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

/// Determines if a given `PreimageKeyType` requires validation.
///
/// # Parameters
/// - `key_type`: A reference to a `PreimageKeyType` enum variant that specifies the type of key
///   involved.
///
/// # Returns
/// - `true` if the `key_type` requires validation.
/// - `false` if the `key_type` is either `PreimageKeyType::Local` or `PreimageKeyType::GlobalGeneric`,
///   as these types do not require validation.
pub fn needs_validation(key_type: &PreimageKeyType) -> bool {
    !matches!(
        key_type,
        PreimageKeyType::Local | PreimageKeyType::GlobalGeneric
    )
}

/// Recomputes the [PreimageKey] for a piece of data to validate its authenticity
///
/// This function ensures that the provided `value` is consistent with the `key`
/// for the specified key type. It computes the hash of the `value` using the
/// appropriate hashing algorithm based on the specified key type, and compares
/// it against the given `key` to verify its validity.
///
/// # Arguments
///
/// * `key` - A reference to a `PreimageKey` that contains the hash and key type
///   against which the `value` should be validated.
/// * `value` - A byte slice representing the data whose hash will be calculated
///   and validated against the given `key`.
///
/// # Returns
///
/// * `Ok(())` - If the `key` is consistent with the hashed `value`.
/// * `Err(PreimageOracleError::InvalidPreimageKey)` - If the computed hash of
///   the `value` does not match the given `key`.
///
/// # Key Types
///
/// The function supports the following key types:
///
/// * `PreimageKeyType::Keccak256` - Computes a Keccak-256 hash of the `value`.
/// * `PreimageKeyType::Sha256` - Computes a SHA-256 hash of the `value`.
/// * `PreimageKeyType::Local` or `PreimageKeyType::GlobalGeneric` - These key
///   types bypass hash validation and do not compute or compare hashes.
///
/// # Panics
///
/// * Panics with `unimplemented!` if called with `PreimageKeyType::Precompile`,
///   as precompile acceleration is not yet supported.
/// * Panics with `unreachable!` if called with `PreimageKeyType::Blob`, since
///   blob key types should not be loaded.
///
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
            unreachable!("Blob key types should not be loaded.");
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
