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

use alloy::eips::BlockId;
use alloy::providers::{Provider, RootProvider};
use alloy_primitives::{keccak256, Address, B256};
use anyhow::{bail, Context};
use async_trait::async_trait;
use hana_blobstream::blobstream::SP1Blobstream::SP1BlobstreamInstance;
use hana_blobstream::blobstream::{blobstream_address, SP1Blobstream};
use hana_host::celestia::{CelestiaChainHintHandler, CelestiaChainHost};
use hana_oracle::hint::HintWrapper;
use kailua_sync::stall::Stall;
use kona_host::{HintHandler, OnlineHostBackendCfg, SharedKeyValueStore};
use kona_preimage::{PreimageKey, PreimageKeyType};
use kona_proof::Hint;
use risc0_steel::ethereum::{
    EthChainSpec, EthEvmEnv, EthEvmFactory, ETH_HOLESKY_CHAIN_SPEC, ETH_MAINNET_CHAIN_SPEC,
    ETH_SEPOLIA_CHAIN_SPEC,
};
use risc0_steel::{Contract, EvmInput};
use tracing::info;

/// The [HintHandler] for the [CelestiaChainHost].
#[derive(Debug, Clone, Copy)]
pub struct HanaHintHandler;

impl HanaHintHandler {
    pub async fn blobstream_height(
        l1_provider: &RootProvider,
        l1_head: B256,
    ) -> anyhow::Result<u64> {
        let chain_id = l1_provider.get_chain_id().await?;
        let blobstream_address = blobstream_address(chain_id)
            .expect("No canonical Blobstream address found for chain id");
        let blobstream_contract = SP1BlobstreamInstance::new(blobstream_address, l1_provider);
        Ok(blobstream_contract
            .latestBlock()
            .block(BlockId::Hash(l1_head.into()))
            .stall("SP1Blobstream::latestBlock")
            .await)
    }

    pub async fn blobstream_height_proof(
        l1_provider: RootProvider,
        l1_head: B256,
        blobstream_address: Address,
    ) -> anyhow::Result<Vec<u8>> {
        let chain_id = l1_provider.get_chain_id().await?;
        let chain_spec = match chain_id {
            1 => ETH_MAINNET_CHAIN_SPEC.clone(),
            11155111 => ETH_SEPOLIA_CHAIN_SPEC.clone(),
            17000 => ETH_HOLESKY_CHAIN_SPEC.clone(),
            _ => EthChainSpec::new_single(chain_id, Default::default()),
        };
        let mut env = EthEvmEnv::builder()
            .chain_spec(&chain_spec)
            .provider(l1_provider)
            .block_hash(l1_head)
            .build()
            .await?;
        // Preflight the call to prepare the input that is required to execute the function in
        // the guest without RPC access. It also returns the result of the call.
        let mut contract = Contract::preflight(blobstream_address, &mut env);
        let preflight_call = contract
            .call_builder(&SP1Blobstream::latestBlockCall)
            .call()
            .await
            .context("Failed to preflight STEEL call for celestia height")?;
        info!("Verified celestia height is {preflight_call} at block {l1_head}.");
        // Construct the input from the environment.
        let evm_input: EvmInput<EthEvmFactory> = env
            .into_input()
            .await
            .context("Failed to convert STEEL env for celestia height")?;
        let proof_response = bincode::serialize(&evm_input)
            .context("Failed to serialize STEEL proof for celestia height.")?;

        Ok(proof_response)
    }
}

#[async_trait]
impl HintHandler for HanaHintHandler {
    type Cfg = CelestiaChainHost;

    async fn fetch_hint(
        hint: Hint<<Self::Cfg as OnlineHostBackendCfg>::HintType>,
        cfg: &Self::Cfg,
        providers: &<Self::Cfg as OnlineHostBackendCfg>::Providers,
        kv: SharedKeyValueStore,
    ) -> anyhow::Result<()> {
        let HintWrapper::CelestiaDA = hint.ty else {
            return CelestiaChainHintHandler::fetch_hint(hint, cfg, providers, kv).await;
        };
        if hint.data.len() == 20 {
            let blobstream_address = Address::from_slice(&hint.data);
            info!("Fetching blobstream height proof for {blobstream_address}.");
            let blobstream_proof = Self::blobstream_height_proof(
                providers.l1().clone(),
                cfg.single_host.l1_head,
                blobstream_address,
            )
            .await
            .context("Failed to request blobstream height proof")?;
            let mut kv_lock = kv.write().await;
            let blobstream_address_hash = keccak256(blobstream_address.as_slice());
            // store the proof data as a the preimage behind the hash of the contract address (masked by l1 head)
            kv_lock.set(
                PreimageKey::new(*blobstream_address_hash, PreimageKeyType::GlobalGeneric).into(),
                blobstream_proof,
            )?;
            return Ok(());
        }

        anyhow::ensure!(hint.data.len() == 40, "Invalid hint data length");
        let height = u64::from_le_bytes(hint.data[0..8].try_into().unwrap());
        let latest_height =
            Self::blobstream_height(providers.l1(), cfg.single_host.l1_head).await?;
        // Early abort if l1 head is insufficient
        if latest_height < height {
            bail!(
                "SP1Blobstream::latestBlock={latest_height} < {height} at L1_HEAD={}",
                cfg.single_host.l1_head
            );
        }

        CelestiaChainHintHandler::fetch_hint(hint, cfg, providers, kv).await
    }
}
