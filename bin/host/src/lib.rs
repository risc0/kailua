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

use alloy::consensus::Transaction;
use alloy::network::primitives::BlockTransactionsKind;
use alloy::primitives::{keccak256, B256};
use alloy::providers::{Provider, ProviderBuilder, ReqwestProvider};
use alloy_chains::NamedChain;
use anyhow::bail;
use clap::Parser;
use kailua_client::parse_b256;
use kailua_common::oracle::BlobFetchRequest;
use kailua_common::precondition::PreconditionValidationData;
use kona_derive::prelude::IndexedBlobHash;
use kona_host::kv::SharedKeyValueStore;
use kona_preimage::{PreimageKey, PreimageKeyType};
use op_alloy_genesis::RollupConfig;
use op_alloy_protocol::BlockInfo;
use serde::Serialize;
use serde_json::{json, Value};
use std::env::set_var;
use std::path::PathBuf;
use tempfile::TempDir;
use tokio::fs;
use tracing::{debug, info, warn};
use zeth_core::driver::CoreDriver;
use zeth_core::mpt::{MptNode, MptNodeData};
use zeth_core::stateless::data::StatelessClientData;
use zeth_core_optimism::OpRethCoreDriver;
use zeth_preflight::client::PreflightClient;
use zeth_preflight_optimism::OpRethPreflightClient;

/// The host binary CLI application arguments.
#[derive(Parser, Serialize, Clone, Debug)]
pub struct KailuaHostCli {
    #[clap(flatten)]
    pub kona: kona_host::HostCli,

    /// Address of OP-NODE endpoint to use
    #[clap(long, env)]
    pub op_node_address: Option<String>,

    #[clap(long, default_value_t = 1, env)]
    /// Number of blocks to build in a single proof
    pub block_count: u64,

    /// Address of OP-NODE endpoint to use
    #[clap(long, default_value_t = false, env)]
    pub skip_zeth_preflight: bool,

    #[clap(long, value_parser = parse_b256, env)]
    pub u_block_hash: Option<B256>,

    #[clap(long, value_parser = parse_b256, env)]
    pub u_blob_kzg_hash: Option<B256>,

    #[clap(long, value_parser = parse_b256, env)]
    pub v_block_hash: Option<B256>,

    #[clap(long, value_parser = parse_b256, env)]
    pub v_blob_kzg_hash: Option<B256>,
}

pub async fn generate_rollup_config(
    cfg: &mut KailuaHostCli,
    tmp_dir: &TempDir,
) -> anyhow::Result<RollupConfig> {
    // generate a RollupConfig for the target network
    match cfg
        .kona
        .l2_chain_id
        .and_then(RollupConfig::from_l2_chain_id)
    {
        Some(rollup_config) => Ok(rollup_config),
        None => match cfg.kona.read_rollup_config().ok() {
            Some(rollup_config) => Ok(rollup_config),
            None => {
                info!("Fetching rollup config from nodes.");
                let tmp_cfg_file = tmp_dir.path().join("rollup-config.json");
                fetch_rollup_config(
                    cfg.op_node_address
                        .clone()
                        .expect("Missing op-node-address")
                        .as_str(),
                    cfg.kona
                        .l2_node_address
                        .clone()
                        .expect("Missing l2-node-address")
                        .as_str(),
                    Some(&tmp_cfg_file),
                )
                .await?;
                cfg.kona.rollup_config_path = Some(tmp_cfg_file);
                cfg.kona.read_rollup_config()
            }
        },
    }
}

pub async fn fetch_rollup_config(
    op_node_address: &str,
    l2_node_address: &str,
    json_file_path: Option<&PathBuf>,
) -> anyhow::Result<RollupConfig> {
    let op_node_provider = ProviderBuilder::new().on_http(op_node_address.try_into()?);
    let l2_node_provider = ProviderBuilder::new().on_http(l2_node_address.try_into()?);

    let mut rollup_config: Value = op_node_provider
        .client()
        .request_noparams("optimism_rollupConfig")
        .await?;

    debug!("Rollup config: {:?}", rollup_config);

    let chain_config: Value = l2_node_provider
        .client()
        .request_noparams("debug_chainConfig")
        .await?;

    debug!("ChainConfig: {:?}", chain_config);

    // base_fee_params
    rollup_config["base_fee_params"] = json!({
        "elasticity_multiplier": chain_config["optimism"]["eip1559Elasticity"]
        .as_u64()
        .unwrap(),
        "max_change_denominator": chain_config["optimism"]["eip1559Denominator"]
        .as_u64()
        .unwrap()
    });
    // canyon_base_fee_params
    if let Some(canyon_denominator) = chain_config["optimism"]["eip1559DenominatorCanyon"].as_u64()
    {
        rollup_config["canyon_base_fee_params"] = json!({
            "elasticity_multiplier": chain_config["optimism"]["eip1559Elasticity"]
        .as_u64()
        .unwrap(),
            "max_change_denominator": canyon_denominator
        });
    }
    // fork times
    for fork in &[
        "regolithTime",
        "canyonTime",
        "deltaTime",
        "ecotoneTime",
        "fjordTime",
        "graniteTime",
        "holoceneTime",
    ] {
        if let Some(value) = chain_config[fork].as_str() {
            rollup_config[fork] = json!(value);
        }
    }
    // export
    // dbg!(&rollup_config);
    let ser_config = serde_json::to_string(&rollup_config)?;
    if let Some(json_file_path) = json_file_path {
        fs::write(json_file_path, &ser_config).await?;
    }

    Ok(serde_json::from_str(&ser_config)?)
}

pub fn mpt_to_vec(node: &MptNode) -> Vec<(B256, Vec<u8>)> {
    if node.is_digest() {
        return vec![];
    }
    let mut res = vec![(node.hash(), alloy::rlp::encode(node))];
    match node.as_data() {
        MptNodeData::Branch(children) => {
            children
                .iter()
                .flatten()
                .for_each(|n| res.append(&mut mpt_to_vec(n)));
        }
        MptNodeData::Extension(_, target) => {
            res.append(&mut mpt_to_vec(target));
        }
        _ => {}
    };
    res
}

pub async fn dump_mpt_to_kv_store(kv_store: &mut SharedKeyValueStore, mpt: &MptNode) {
    let mut store = kv_store.write().await;
    mpt_to_vec(mpt).into_iter().for_each(|(hash, data)| {
        store
            .set(
                PreimageKey::new(*hash, PreimageKeyType::Keccak256).into(),
                data,
            )
            .expect("Failed to dump node to kv_store");
    });
}

pub async fn dump_data_to_kv_store<B, H>(
    kv_store: &mut SharedKeyValueStore,
    data: &StatelessClientData<B, H>,
) {
    // State trie
    dump_mpt_to_kv_store(kv_store, &data.state_trie).await;
    // Storage tries
    for (mpt, _) in data.storage_tries.values() {
        dump_mpt_to_kv_store(kv_store, mpt).await;
    }
    // Contracts
    let mut store = kv_store.write().await;
    for contract in data.contracts.values() {
        let hash = keccak256(contract);
        store
            .set(
                PreimageKey::new(*hash, PreimageKeyType::Keccak256).into(),
                contract.to_vec(),
            )
            .expect("Failed to dump contract to kv_store");
    }
}

pub async fn zeth_execution_preflight(
    cfg: &KailuaHostCli,
    rollup_config: RollupConfig,
) -> anyhow::Result<()> {
    if let Ok(named_chain) = NamedChain::try_from(rollup_config.l2_chain_id) {
        // Limitation: Only works when disk caching is enabled under a known "NamedChain"
        if !cfg.kona.is_offline()
            && cfg.kona.data_dir.is_some()
            && OpRethCoreDriver::chain_spec(&named_chain).is_some()
        {
            info!("Performing zeth-optimism preflight.");
            let kona_cfg = cfg.kona.clone();
            let preflight_start = kona_cfg.claimed_l2_block_number - cfg.block_count + 1;
            let block_count = cfg.block_count;
            // Fetch all the initial data
            let preflight_data: StatelessClientData<
                <OpRethCoreDriver as CoreDriver>::Block,
                <OpRethCoreDriver as CoreDriver>::Header,
            > = tokio::task::spawn_blocking(move || {
                // Prepare the cache directory
                let cache_dir = kona_cfg.data_dir.map(|dir| dir.join("optimism"));
                if let Some(dir) = cache_dir.as_ref() {
                    std::fs::create_dir_all(dir).expect("Could not create directory");
                };
                OpRethPreflightClient::preflight(
                    Some(rollup_config.l2_chain_id),
                    cache_dir,
                    kona_cfg.l2_node_address,
                    preflight_start,
                    block_count,
                )
            })
            .await??;
            // Write data to the cached Kona kv-store
            let mut kv_store = cfg.kona.construct_kv_store();
            dump_data_to_kv_store(&mut kv_store, &preflight_data).await;
        }
    }
    Ok(())
}

pub async fn get_blob_fetch_request(
    l1_provider: &ReqwestProvider,
    block_hash: B256,
    blob_hash: B256,
) -> anyhow::Result<BlobFetchRequest> {
    let block = l1_provider
        .get_block_by_hash(block_hash, BlockTransactionsKind::Full)
        .await?
        .expect("Failed to fetch block {block_hash}.");
    let mut blob_index = 0usize;
    for txn in block.transactions.into_transactions() {
        if let Some(blobs) = txn.blob_versioned_hashes() {
            for blob in blobs {
                if blob == &blob_hash {
                    break;
                }
                blob_index += 1;
            }
        }
    }

    Ok(BlobFetchRequest {
        block_ref: BlockInfo {
            hash: block.header.hash,
            number: block.header.number,
            parent_hash: block.header.parent_hash,
            timestamp: block.header.timestamp,
        },
        blob_hash: IndexedBlobHash {
            index: blob_index,
            hash: blob_hash,
        },
    })
}

pub async fn fetch_precondition_data(
    cfg: &KailuaHostCli,
) -> anyhow::Result<Option<PreconditionValidationData>> {
    // Determine precondition hash
    let hash_arguments = [
        cfg.u_block_hash,
        cfg.u_blob_kzg_hash,
        cfg.v_block_hash,
        cfg.v_blob_kzg_hash,
    ];

    // fetch necessary data to validate blob equivalence precondition
    if hash_arguments.iter().all(|arg| arg.is_some()) {
        let (l1_provider, _, _) = cfg.kona.create_providers().await?;
        // todo fetch & write data
        let precondition_validation_data = PreconditionValidationData {
            validated_blobs: [
                get_blob_fetch_request(
                    &l1_provider,
                    cfg.u_block_hash.unwrap(),
                    cfg.u_blob_kzg_hash.unwrap(),
                )
                .await?,
                get_blob_fetch_request(
                    &l1_provider,
                    cfg.v_block_hash.unwrap(),
                    cfg.v_blob_kzg_hash.unwrap(),
                )
                .await?,
            ],
        };
        let kv_store = cfg.kona.construct_kv_store();
        let mut store = kv_store.write().await;
        let hash = precondition_validation_data.hash();
        store.set(
            PreimageKey::new(*hash, PreimageKeyType::Sha256).into(),
            precondition_validation_data.to_vec(),
        )?;
        set_var("PRECONDITION_VALIDATION_DATA_HASH", hash.to_string());
        Ok(Some(precondition_validation_data))
    } else if hash_arguments.iter().any(|arg| arg.is_some()) {
        bail!("Insufficient number of arguments provided for precondition hash.")
    } else {
        warn!("Proving without a precondition hash.");
        Ok(None)
    }
}
