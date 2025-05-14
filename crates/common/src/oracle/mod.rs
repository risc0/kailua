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

pub mod vec;

use alloy_primitives::keccak256;
use kona_preimage::errors::{PreimageOracleError, PreimageOracleResult};
use kona_preimage::{CommsClient, PreimageKey, PreimageKeyType};
use kona_proof::FlushableCache;
use risc0_zkvm::sha::{Impl as SHA2, Sha256};
use std::fmt::Debug;

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

/// A trait representing a Witness Oracle which manages and validates cryptographic preimages.
///
/// The `WitnessOracle` trait provides functionality to interact with and manage preimages.
/// Preimages are key-value pairs where the key is typically an identifier for the data,
/// and the value is the data itself stored as a `Vec<u8>`. This trait incorporates several features
/// including validation, insertion, count retrieval, and finalization of the stored preimages.
///
/// # Provided Methods
/// - `preimage_count`: Retrieve the number of preimages stored.
/// - `validate_preimages`: Validates that all stored preimages satisfy predefined constraints.
/// - `insert_preimage`: Inserts a new preimage into the oracle.
/// - `finalize_preimages`: Prepares the oracle's preimages for use under defined shard size and validation settings.
pub trait WitnessOracle: CommsClient + FlushableCache + Send + Sync + Debug + Default {
    /// Returns the count of preimages stored in the oracle.
    fn preimage_count(&self) -> usize;

    /// Ensures that the preimages stored in the oracle meet the required criteria or constraints
    /// defined by each `PreimageKeyType`. If the validation fails, an error is returned.
    fn validate_preimages(&self) -> anyhow::Result<()>;

    /// Inserts a preimage into the oracle.
    fn insert_preimage(&mut self, key: PreimageKey, value: Vec<u8>);

    /// This method finalizes the process of preparing the oracle preimages for a specific shard
    /// size and optional validation cache.
    fn finalize_preimages(&mut self, shard_size: usize, with_validation_cache: bool);
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_validate_preimage() {
        // Test Keccak256
        let key = PreimageKey::new(keccak256(b"test").0, PreimageKeyType::Keccak256);
        let value = b"test";
        assert!(validate_preimage(&key, value).is_ok());

        // Test invalid Keccak256
        let invalid_key = PreimageKey::new(keccak256(b"wrong").0, PreimageKeyType::Keccak256);
        assert!(validate_preimage(&invalid_key, value).is_err());

        // Test Sha256
        let sha_value = b"test";
        let sha_key = PreimageKey::new(
            SHA2::hash_bytes(sha_value).as_bytes().try_into().unwrap(),
            PreimageKeyType::Sha256,
        );
        assert!(validate_preimage(&sha_key, sha_value).is_ok());

        // Test invalid Sha256
        let invalid_sha_key = PreimageKey::new(
            SHA2::hash_bytes(b"wrong").as_bytes().try_into().unwrap(),
            PreimageKeyType::Sha256,
        );
        assert!(validate_preimage(&invalid_sha_key, sha_value).is_err());

        // Test Local (no validation)
        let local_key = PreimageKey::new([0u8; 32], PreimageKeyType::Local);
        assert!(validate_preimage(&local_key, b"any value").is_ok());

        // Test GlobalGeneric (no validation)
        let global_key = PreimageKey::new([0u8; 32], PreimageKeyType::GlobalGeneric);
        assert!(validate_preimage(&global_key, b"any value").is_ok());

        // Test Precompile (should panic)
        let precompile_key = PreimageKey::new([0u8; 32], PreimageKeyType::Precompile);
        let result = std::panic::catch_unwind(|| validate_preimage(&precompile_key, b"test"));
        assert!(result.is_err());

        // Test Blob (should panic)
        let blob_key = PreimageKey::new([0u8; 32], PreimageKeyType::Blob);
        let result = std::panic::catch_unwind(|| validate_preimage(&blob_key, b"test"));
        assert!(result.is_err());
    }

    #[test]
    fn test_needs_validation() {
        // Test all PreimageKeyType variants
        assert!(needs_validation(&PreimageKeyType::Keccak256));
        assert!(needs_validation(&PreimageKeyType::Sha256));
        assert!(needs_validation(&PreimageKeyType::Precompile));
        assert!(needs_validation(&PreimageKeyType::Blob));
        assert!(!needs_validation(&PreimageKeyType::Local));
        assert!(!needs_validation(&PreimageKeyType::GlobalGeneric));
    }
}
