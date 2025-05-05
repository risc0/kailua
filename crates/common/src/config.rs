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

use alloy_primitives::{b256, Address, B256};
use anyhow::Context;
use kona_genesis::RollupConfig;
use risc0_zkvm::sha::{Impl as SHA2, Sha256};
use std::fmt::Debug;

pub const CONTROL_ROOT: B256 =
    b256!("539032186827b06719244873b17b2d4c122e2d02cfb1994fe958b2523b844576");
pub const BN254_CONTROL_ID: B256 =
    b256!("04446e66d300eb7fb45c9726bb53c793dda407a62e9601618bb43c5c14657ac0");

/// Returns a value based on the provided `Option` and a default value, with safety checks.
///
/// This function takes an optional value `opt` and a default value `default`.
/// If `opt` contains a value, it checks whether it is equal to the default value.
/// If they are equal, an error is returned indicating an unsafe condition.
/// Otherwise, the value inside `opt` is returned. If `opt` is `None`, the default value is returned.
///
/// # Type Parameters
/// - `V`: The type of the values, which must implement the `Debug` and `Eq` traits.
///
/// # Arguments
/// - `opt`: An `Option<V>` which may or may not contain a value.
/// - `default`: A default value of type `V` to use if `opt` is `None`.
///
/// # Returns
/// - `Ok(V)`: The value inside `opt` if it is present and not equal to the default value,
///   or the `default` value if `opt` is `None`.
/// - `Err(anyhow::Error)`: An error if `opt` contains a value that is equal to `default`.
///
/// # Errors
/// Returns an `anyhow::Error` if the optional value is present and equal to the default value.
///
/// # Examples
/// ```
/// use anyhow::Result;
///
/// let value = safe_default(Some(42), 0);
/// assert_eq!(value.unwrap(), 42);
///
/// let value = safe_default(None, 100);
/// assert_eq!(value.unwrap(), 100);
///
/// let err = safe_default(Some(10), 10);
/// assert!(err.is_err());
/// ```
pub fn safe_default<V: Debug + Eq>(opt: Option<V>, default: V) -> anyhow::Result<V> {
    if let Some(v) = opt {
        if v == default {
            anyhow::bail!(format!("Unsafe value! {v:?}"))
        }
        Ok(v)
    } else {
        Ok(default)
    }
}

/// Computes the hash of a RollupConfig, which summarizes various rollup configuration settings
/// into a single 32-byte hash value. This function utilizes components from the RollupConfig
/// struct, including genesis properties, system configuration details, and hardfork timings.
///
/// The hash is computed by serializing the relevant fields of RollupConfig and its sub-structures
/// into a contiguous byte array, then hashing the result using the SHA-256 algorithm.
///
/// # Arguments
///
/// * `rollup_config` - A reference to the `RollupConfig` struct, containing all configuration
///   parameters for a rollup.
///
/// # Returns
///
/// * `anyhow::Result<[u8; 32]>` - On success, returns a 32-byte array representing the hash of
///   the rollup configuration. If errors are encountered during field processing or conversions,
///   an error wrapped in `anyhow::Error` is returned.
///
/// # Errors
///
/// The function may return an error in the following scenarios:
/// * Parsing errors from the `safe_default` utility while processing optional fields, such as
///   `base_fee_scalar`, `blob_base_fee_scalar`, etc.
/// * Conversion failures when converting slices or numbers to their byte representations.
///
/// # Behavior
///
/// 1. Computes a `system_config_hash` from the system configuration settings in `rollup_config.genesis`.
///    If the system configuration is absent, a default zeroed 32-byte array is used.
/// 2. Serializes various fields of `RollupConfig`, including genesis information, block time settings,
///    protocol parameters, hardfork timings, and address-specific fields. These fields are concatenated
///    into a single byte array.
/// 3. The resulting byte array is hashed using SHA-256 to produce a 32-byte digest.
/// 4. Returns the computed hash if all operations succeed.
///
/// # Notes
///
/// * `safe_default` is used extensively to provide fallback values for optional configuration
///   fields, ensuring robust handling of missing or invalid data.
/// * All numeric values are serialized in big-endian format for consistency.
pub fn config_hash(rollup_config: &RollupConfig) -> anyhow::Result<[u8; 32]> {
    let system_config_hash: [u8; 32] = rollup_config
        .genesis
        .system_config
        .as_ref()
        .map(|system_config| {
            let fields = [
                system_config.batcher_address.0.as_slice(),
                system_config.overhead.to_be_bytes::<32>().as_slice(),
                system_config.scalar.to_be_bytes::<32>().as_slice(),
                system_config.gas_limit.to_be_bytes().as_slice(),
                safe_default(system_config.base_fee_scalar, u64::MAX)
                    .context("base_fee_scalar")?
                    .to_be_bytes()
                    .as_slice(),
                safe_default(system_config.blob_base_fee_scalar, u64::MAX)
                    .context("blob_base_fee_scalar")?
                    .to_be_bytes()
                    .as_slice(),
                safe_default(system_config.eip1559_denominator, u32::MAX)
                    .context("eip1559_denominator")?
                    .to_be_bytes()
                    .as_slice(),
                safe_default(system_config.eip1559_elasticity, u32::MAX)
                    .context("eip1559_elasticity")?
                    .to_be_bytes()
                    .as_slice(),
            ]
            .concat();
            let digest = SHA2::hash_bytes(fields.as_slice());

            Ok::<[u8; 32], anyhow::Error>(digest.as_bytes().try_into()?)
        })
        .unwrap_or(Ok([0u8; 32]))?;
    let rollup_config_bytes = [
        rollup_config.genesis.l1.hash.0.as_slice(),
        rollup_config.genesis.l1.number.to_be_bytes().as_slice(),
        rollup_config.genesis.l2.hash.0.as_slice(),
        rollup_config.genesis.l2.number.to_be_bytes().as_slice(),
        rollup_config.genesis.l2_time.to_be_bytes().as_slice(),
        system_config_hash.as_slice(),
        rollup_config.block_time.to_be_bytes().as_slice(),
        rollup_config.max_sequencer_drift.to_be_bytes().as_slice(),
        rollup_config.seq_window_size.to_be_bytes().as_slice(),
        rollup_config.channel_timeout.to_be_bytes().as_slice(),
        rollup_config
            .granite_channel_timeout
            .to_be_bytes()
            .as_slice(),
        rollup_config.l1_chain_id.to_be_bytes().as_slice(),
        rollup_config.l2_chain_id.to_be_bytes().as_slice(),
        rollup_config
            .chain_op_config
            .eip1559_denominator
            .to_be_bytes()
            .as_slice(),
        rollup_config
            .chain_op_config
            .eip1559_elasticity
            .to_be_bytes()
            .as_slice(),
        rollup_config
            .chain_op_config
            .eip1559_denominator_canyon
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.hardforks.regolith_time, u64::MAX)
            .context("regolith_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.hardforks.canyon_time, u64::MAX)
            .context("canyon_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.hardforks.delta_time, u64::MAX)
            .context("delta_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.hardforks.ecotone_time, u64::MAX)
            .context("ecotone_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.hardforks.fjord_time, u64::MAX)
            .context("fjord_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.hardforks.granite_time, u64::MAX)
            .context("granite_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.hardforks.holocene_time, u64::MAX)
            .context("holocene_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.hardforks.isthmus_time, u64::MAX)
            .context("isthmus_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.hardforks.interop_time, u64::MAX)
            .context("interop_time")?
            .to_be_bytes()
            .as_slice(),
        rollup_config.batch_inbox_address.0.as_slice(),
        rollup_config.deposit_contract_address.0.as_slice(),
        rollup_config.l1_system_config_address.0.as_slice(),
        rollup_config.protocol_versions_address.0.as_slice(),
        safe_default(rollup_config.superchain_config_address, Address::ZERO)
            .context("superchain_config_address")?
            .0
            .as_slice(),
        safe_default(rollup_config.blobs_enabled_l1_timestamp, u64::MAX)
            .context("blobs_enabled_timestamp")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.da_challenge_address, Address::ZERO)
            .context("da_challenge_address")?
            .0
            .as_slice(),
    ]
    .concat();
    let digest = SHA2::hash_bytes(rollup_config_bytes.as_slice());
    Ok::<[u8; 32], anyhow::Error>(digest.as_bytes().try_into()?)
}
