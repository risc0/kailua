// Copyright 2025 RISC Zero, Inc.
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

use alloy_primitives::{keccak256, Bytes};
use async_trait::async_trait;
use celestia_types::Commitment;
use hana_blobstream::blobstream::{blobstream_address, SP1Blobstream};
use hana_celestia::CelestiaProvider;
use hana_oracle::hint::HintWrapper;
use hana_oracle::provider::OracleCelestiaProvider;
use kailua_kona::client::log;
use kona_preimage::{CommsClient, PreimageKey, PreimageKeyType};
use kona_proof::errors::OracleProviderError;
use kona_proof::{BootInfo, FlushableCache, Hint};
use risc0_steel::ethereum::{
    EthChainSpec, EthEvmInput, ETH_HOLESKY_CHAIN_SPEC, ETH_MAINNET_CHAIN_SPEC,
    ETH_SEPOLIA_CHAIN_SPEC,
};
use risc0_steel::Contract;
use std::fmt::Debug;
use std::sync::Arc;

/// A [CelestiaProvider] aware of the celestia height synchronized with a blobstream contract
#[derive(Debug, Clone)]
pub struct HanaProvider<T: CelestiaProvider + Send + Sync + Clone + Debug> {
    pub celestia_provider: T,
    pub blobstream_height: u64,
}

impl<T: CommsClient + FlushableCache + Send + Sync + Debug + Clone>
    HanaProvider<OracleCelestiaProvider<T>>
{
    pub fn new(celestia_oracle: Arc<T>) -> (Self, BootInfo) {
        // Boot up hana provider with validated max height
        let boot = kona_proof::block_on(BootInfo::load(celestia_oracle.as_ref()))
            .expect("Failed to load boot info");
        let blobstream_addr = blobstream_address(boot.rollup_config.l1_chain_id)
            .expect("No canonical Blobstream address found for chain id");
        let hint = Hint::new(HintWrapper::CelestiaDA, blobstream_addr.0);
        kona_proof::block_on(hint.send(celestia_oracle.as_ref()))
            .expect("Failed to send celestia height hint");
        let proof = kona_proof::block_on(celestia_oracle.get(PreimageKey::new(
            keccak256(blobstream_addr.as_slice()).0,
            PreimageKeyType::GlobalGeneric,
        )))
        .expect("Failed to get celestia height proof");
        let evm_input: EthEvmInput = bincode::deserialize(&proof)
            .expect("Failed to deserialize EthEvmInput for celestia height");
        let env = match boot.rollup_config.l1_chain_id {
            1 => evm_input.into_env(&ETH_MAINNET_CHAIN_SPEC),
            11155111 => evm_input.into_env(&ETH_SEPOLIA_CHAIN_SPEC),
            17000 => evm_input.into_env(&ETH_HOLESKY_CHAIN_SPEC),
            _ => evm_input.into_env(&EthChainSpec::new_single(
                boot.rollup_config.l1_chain_id,
                Default::default(),
            )),
        };
        let blobstream_height = Contract::new(blobstream_addr, &env)
            .call_builder(&SP1Blobstream::latestBlockCall)
            .call();
        assert_eq!(env.header().seal(), boot.l1_head);
        log(&format!("BLOBSTREAM HEIGHT {blobstream_height}"));
        (
            Self {
                celestia_provider: OracleCelestiaProvider::new(celestia_oracle),
                blobstream_height,
            },
            boot,
        )
    }
}

#[async_trait]
impl<T: CelestiaProvider<Error = OracleProviderError> + Send + Sync + Clone + Debug>
    CelestiaProvider for HanaProvider<T>
{
    type Error = OracleProviderError;

    async fn blob_get(&self, height: u64, commitment: Commitment) -> Result<Bytes, Self::Error> {
        if height > self.blobstream_height {
            return Err(OracleProviderError::BlockNumberPastHead(
                height,
                self.blobstream_height,
            ));
        }
        self.celestia_provider.blob_get(height, commitment).await
    }
}
