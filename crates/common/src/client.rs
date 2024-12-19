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

use crate::blobs;
use crate::precondition::PreconditionValidationData;
use alloy_consensus::Header;
use alloy_eips::eip4844::FIELD_ELEMENTS_PER_BLOB;
use alloy_primitives::{Address, Sealed, B256};
use anyhow::{bail, Context};
use kona_derive::traits::BlobProvider;
use kona_driver::Driver;
use kona_executor::TrieDBProvider;
use kona_preimage::{CommsClient, PreimageKey, PreimageKeyType};
use kona_proof::errors::OracleProviderError;
use kona_proof::executor::KonaExecutor;
use kona_proof::l1::{OracleL1ChainProvider, OraclePipeline};
use kona_proof::l2::OracleL2ChainProvider;
use kona_proof::sync::new_pipeline_cursor;
use kona_proof::{BootInfo, FlushableCache, HintType};
use op_alloy_genesis::RollupConfig;
use risc0_zkvm::sha::{Impl as SHA2, Sha256};
use std::fmt::Debug;
use std::sync::Arc;

pub fn run_client<
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
>(
    precondition_validation_data_hash: B256,
    oracle: Arc<O>,
    boot: Arc<BootInfo>,
    mut beacon: B,
) -> anyhow::Result<(B256, Option<B256>)>
where
    <B as BlobProvider>::Error: Debug,
{
    kona_proof::block_on(async move {
        ////////////////////////////////////////////////////////////////
        //                        PRECONDITION                        //
        ////////////////////////////////////////////////////////////////

        log("PRECONDITION");
        let precondition_hash = validate_precondition(
            precondition_validation_data_hash,
            oracle.clone(),
            boot.clone(),
            &mut beacon,
        )
        .await?;

        ////////////////////////////////////////////////////////////////
        //                          PROLOGUE                          //
        ////////////////////////////////////////////////////////////////
        log("PROLOGUE");

        let mut l1_provider = OracleL1ChainProvider::new(boot.clone(), oracle.clone());
        let mut l2_provider = OracleL2ChainProvider::new(boot.clone(), oracle.clone());

        // If the claimed L2 block number is less than the safe head of the L2 chain, the claim is
        // invalid.
        let safe_head = fetch_safe_head(oracle.as_ref(), boot.as_ref(), &mut l2_provider).await?;
        if boot.claimed_l2_block_number < safe_head.number {
            bail!("Invalid Claim");
        }

        // In the case where the agreed upon L2 output root is the same as the claimed L2 output root,
        // trace extension is detected and we can skip the derivation and execution steps.
        if boot.agreed_l2_output_root == boot.claimed_l2_output_root {
            return Ok((precondition_hash, Some(boot.claimed_l2_output_root)));
        }

        ////////////////////////////////////////////////////////////////
        //                   DERIVATION & EXECUTION                   //
        ////////////////////////////////////////////////////////////////
        log("DERIVATION");
        // Create a new derivation driver with the given boot information and oracle.
        let cursor =
            new_pipeline_cursor(&boot, safe_head, &mut l1_provider, &mut l2_provider).await?;
        let cfg = Arc::new(boot.rollup_config.clone());
        let pipeline = OraclePipeline::new(
            cfg.clone(),
            cursor.clone(),
            oracle.clone(),
            beacon,
            l1_provider.clone(),
            l2_provider.clone(),
        );
        let executor = KonaExecutor::new(&cfg, l2_provider.clone(), l2_provider, None, None);
        let mut driver = Driver::new(cursor, executor, pipeline);

        // Run the derivation pipeline until we are able to produce the output root of the claimed
        // L2 block.
        log("ADVANCE");
        let (number, output_root) = driver
            .advance_to_target(&boot.rollup_config, Some(boot.claimed_l2_block_number))
            .await?;

        // None indicates that there is insufficient L1 data available to produce an L2
        // output root at the claimed block number
        log(&format!(
            "OUTPUT: {number}|{}",
            boot.claimed_l2_block_number
        ));

        if number < boot.claimed_l2_block_number {
            Ok((precondition_hash, None))
        } else {
            Ok((precondition_hash, Some(output_root)))
        }
    })
}

/// Fetches the safe head of the L2 chain based on the agreed upon L2 output root in the
/// [BootInfo].
async fn fetch_safe_head<O: CommsClient>(
    caching_oracle: &O,
    boot_info: &BootInfo,
    l2_chain_provider: &mut OracleL2ChainProvider<O>,
) -> Result<Sealed<Header>, OracleProviderError> {
    caching_oracle
        .write(&HintType::StartingL2Output.encode_with(&[boot_info.agreed_l2_output_root.as_ref()]))
        .await
        .map_err(OracleProviderError::Preimage)?;
    let mut output_preimage = [0u8; 128];
    caching_oracle
        .get_exact(
            PreimageKey::new(*boot_info.agreed_l2_output_root, PreimageKeyType::Keccak256),
            &mut output_preimage,
        )
        .await
        .map_err(OracleProviderError::Preimage)?;

    let safe_hash = output_preimage[96..128]
        .try_into()
        .map_err(OracleProviderError::SliceConversion)?;
    l2_chain_provider
        .header_by_hash(safe_hash)
        .map(|header| Sealed::new_unchecked(header, safe_hash))
}

pub fn log(msg: &str) {
    #[cfg(target_os = "zkvm")]
    risc0_zkvm::guest::env::log(msg);
    #[cfg(not(target_os = "zkvm"))]
    tracing::info!("{msg}");
}

fn safe_default<V: Debug + Eq>(opt: Option<V>, default: V) -> anyhow::Result<V> {
    if let Some(v) = opt {
        if v == default {
            anyhow::bail!(format!("Unsafe value! {v:?}"))
        }
        Ok(v)
    } else {
        Ok(default)
    }
}

pub fn config_hash(rollup_config: &RollupConfig) -> anyhow::Result<[u8; 32]> {
    // todo: check whether we need to include this, or if it is loaded from the config address
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
            ]
            .concat();
            let digest = SHA2::hash_bytes(fields.as_slice());

            Ok::<[u8; 32], anyhow::Error>(digest.as_bytes().try_into()?)
        })
        .unwrap_or(Ok([0u8; 32]))?;
    let rollup_config_bytes = [
        rollup_config.genesis.l1.hash.0.as_slice(),
        rollup_config.genesis.l2.hash.0.as_slice(),
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
            .base_fee_params
            .max_change_denominator
            .to_be_bytes()
            .as_slice(),
        rollup_config
            .base_fee_params
            .elasticity_multiplier
            .to_be_bytes()
            .as_slice(),
        rollup_config
            .canyon_base_fee_params
            .max_change_denominator
            .to_be_bytes()
            .as_slice(),
        rollup_config
            .canyon_base_fee_params
            .elasticity_multiplier
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.regolith_time, u64::MAX)
            .context("regolith_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.canyon_time, u64::MAX)
            .context("canyon_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.delta_time, u64::MAX)
            .context("delta_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.ecotone_time, u64::MAX)
            .context("ecotone_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.fjord_time, u64::MAX)
            .context("fjord_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.granite_time, u64::MAX)
            .context("granite_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.holocene_time, u64::MAX)
            .context("holocene_time")?
            .to_be_bytes()
            .as_slice(),
        safe_default(rollup_config.blobs_enabled_l1_timestamp, u64::MAX)
            .context("blobs_enabled_timestmap")?
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
        safe_default(rollup_config.da_challenge_address, Address::ZERO)
            .context("da_challenge_address")?
            .0
            .as_slice(),
    ]
    .concat();
    let digest = SHA2::hash_bytes(rollup_config_bytes.as_slice());
    Ok::<[u8; 32], anyhow::Error>(digest.as_bytes().try_into()?)
}

pub async fn validate_precondition<
    O: CommsClient + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
>(
    precondition_data_hash: B256,
    oracle: Arc<O>,
    boot: Arc<BootInfo>,
    beacon: &mut B,
) -> anyhow::Result<B256>
where
    <B as BlobProvider>::Error: Debug,
{
    // There is no condition to validate at blob boundaries
    if precondition_data_hash.is_zero() {
        return Ok(B256::ZERO);
    }
    // Read the blob references to fetch
    let precondition_validation_data: PreconditionValidationData = pot::from_slice(
        &oracle
            .get(PreimageKey::new(
                *precondition_data_hash,
                PreimageKeyType::Sha256,
            ))
            .await
            .map_err(OracleProviderError::Preimage)?,
    )?;
    let precondition_hash = precondition_validation_data.precondition_hash();
    // Read the blobs to validate
    let mut blobs = Vec::new();
    for request in precondition_validation_data.validated_blobs {
        #[cfg(not(target_os = "zkvm"))]
        let expected_hash = request.blob_hash.hash;

        let response = beacon
            .get_blobs(&request.block_ref, &[request.blob_hash])
            .await
            .unwrap();
        let blob = *response[0];
        #[cfg(not(target_os = "zkvm"))]
        {
            let blob = c_kzg::Blob::new(blob.0);
            let commitment = c_kzg::KzgCommitment::blob_to_kzg_commitment(
                &blob,
                c_kzg::ethereum_kzg_settings(),
            )?;
            let hash = alloy_eips::eip4844::kzg_to_versioned_hash(commitment.as_slice());
            assert_eq!(hash, expected_hash);
        }

        blobs.push(blob);
    }
    // Check equivalence until divergence point
    for i in 0..FIELD_ELEMENTS_PER_BLOB {
        let index = 32 * i as usize;
        if blobs[0][index..index + 32] != blobs[1][index..index + 32] {
            let agreed_l2_output_root_fe = blobs::hash_to_fe(boot.agreed_l2_output_root);
            if i == 0 {
                bail!("Precondition validation failed at first element");
            } else if &blobs[0][index - 32..index] != agreed_l2_output_root_fe.as_slice() {
                bail!(
                    "Agreed output {} not found in contender blob before sub-offset {i}",
                    boot.agreed_l2_output_root
                );
            } else if &blobs[1][index - 32..index] != agreed_l2_output_root_fe.as_slice() {
                bail!(
                    "Agreed output {} not found in proposal before sub-offset {i}",
                    boot.agreed_l2_output_root
                );
            }
            break;
        }
    }
    // Return the precondition hash
    Ok(precondition_hash)
}
