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

use crate::precondition::PreconditionValidationData;
use alloy_eips::eip4844::FIELD_ELEMENTS_PER_BLOB;
use alloy_primitives::{Address, B256};
use alloy_rpc_types_beacon::sidecar::BlobData;
use anyhow::{bail, Context};
use core::fmt::Debug;
use kona_client::errors::OracleProviderError;
use kona_client::{BootInfo, FlushableCache};
use kona_derive::prelude::BlobProvider;
use kona_preimage::{CommsClient, PreimageKey, PreimageKeyType};
use op_alloy_genesis::RollupConfig;
use risc0_zkvm::sha::{Impl as SHA2, Sha256};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub mod blobs;
pub mod client;
pub mod oracle;
pub mod precondition;
pub mod provider;

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct ProofJournal {
    /// The last finalized L2 output
    pub precondition_output: B256,
    /// The L1 head hash containing the safe L2 chain data that may reproduce the L2 head hash.
    pub l1_head: B256,
    /// The latest finalized L2 output root.
    pub agreed_l2_output_root: B256,
    /// The L2 output root claim.
    pub claimed_l2_output_root: B256,
    /// The L2 claim block number.
    pub claimed_l2_block_number: u64,
    /// The configuration hash.
    pub config_hash: [u8; 32],
}

impl ProofJournal {
    pub fn new(precondition_output: B256, boot_info: &BootInfo) -> Self {
        Self {
            precondition_output,
            l1_head: boot_info.l1_head,
            agreed_l2_output_root: boot_info.agreed_l2_output_root,
            claimed_l2_output_root: boot_info.claimed_l2_output_root,
            claimed_l2_block_number: boot_info.claimed_l2_block_number,
            config_hash: config_hash(&boot_info.rollup_config).unwrap(),
        }
    }
}

impl ProofJournal {
    pub fn encode_packed(&self) -> Vec<u8> {
        [
            self.precondition_output.as_slice(),
            self.l1_head.as_slice(),
            self.agreed_l2_output_root.as_slice(),
            self.claimed_l2_output_root.as_slice(),
            self.claimed_l2_block_number.to_be_bytes().as_slice(),
            self.config_hash.as_slice(),
        ]
        .concat()
    }

    pub fn decode_packed(encoded: &[u8]) -> Result<Self, anyhow::Error> {
        Ok(ProofJournal {
            precondition_output: encoded[..32].try_into().context("precondition_output")?,
            l1_head: encoded[32..64].try_into().context("l1_head")?,
            agreed_l2_output_root: encoded[64..96]
                .try_into()
                .context("agreed_l2_output_root")?,
            claimed_l2_output_root: encoded[96..128]
                .try_into()
                .context("claimed_l2_output_root")?,
            claimed_l2_block_number: u64::from_be_bytes(
                encoded[128..136]
                    .try_into()
                    .context("claimed_l2_block_number")?,
            ),
            config_hash: encoded[136..168].try_into().context("config_hash")?,
        })
    }
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

pub fn intermediate_outputs(blob_data: &BlobData, blocks: usize) -> anyhow::Result<Vec<B256>> {
    let mut outputs = vec![];
    for i in 0..blocks {
        let index = 32 * i;
        let bytes: [u8; 32] = blob_data.blob.0[index..index + 32].try_into()?;
        outputs.push(B256::from(bytes));
    }
    Ok(outputs)
}

pub async fn validate_precondition<
    O: CommsClient + FlushableCache + Send + Sync + Debug,
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
            let settings = blobs::kzg_settings();
            let blob = c_kzg::Blob::new(blob.0);
            let commitment = c_kzg::KzgCommitment::blob_to_kzg_commitment(&blob, settings)?;
            let hash = alloy_eips::eip4844::kzg_to_versioned_hash(commitment.as_slice());
            assert_eq!(hash, expected_hash);
        }

        blobs.push(blob);
    }
    // Check equivalence until divergence point
    for i in 0..FIELD_ELEMENTS_PER_BLOB {
        let index = 32 * i as usize;
        if blobs[0][index..index + 32] != blobs[1][index..index + 32] {
            let agreed_l2_output_root_fe = hash_to_fe(boot.agreed_l2_output_root);
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

pub fn hash_to_fe(mut hash: B256) -> B256 {
    hash.0[0] &= u8::MAX >> 2;
    hash
}
