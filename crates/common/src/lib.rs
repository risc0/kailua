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

use alloy_primitives::{Address, B256};
use alloy_rpc_types_beacon::sidecar::BlobData;
use anyhow::Context;
use kona_client::BootInfo;
use op_alloy_genesis::RollupConfig;
use risc0_zkvm::sha::{Impl as SHA2, Sha256};
use serde::{Deserialize, Serialize};

pub mod blobs;
pub mod client;
pub mod oracle;

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct ProofJournal {
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

impl From<BootInfo> for ProofJournal {
    fn from(value: BootInfo) -> Self {
        Self {
            l1_head: value.l1_head,
            agreed_l2_output_root: value.agreed_l2_output_root,
            claimed_l2_output_root: value.claimed_l2_output_root,
            claimed_l2_block_number: value.claimed_l2_block_number,
            config_hash: config_hash(&value.rollup_config).unwrap(),
        }
    }
}

impl ProofJournal {
    pub fn encode_packed(&self) -> Vec<u8> {
        [
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
            l1_head: encoded[..32].try_into()?,
            agreed_l2_output_root: encoded[32..64].try_into()?,
            claimed_l2_output_root: encoded[64..96].try_into()?,
            claimed_l2_block_number: u64::from_be_bytes(encoded[96..104].try_into()?),
            config_hash: encoded[104..136].try_into()?,
        })
    }
}

fn safe_default<V: core::fmt::Debug + Eq>(opt: Option<V>, default: V) -> anyhow::Result<V> {
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
