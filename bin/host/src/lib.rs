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

use alloy::primitives::{keccak256, B256};
use alloy::providers::{Provider, ProviderBuilder};
use clap::Parser;
use kona_host::kv::SharedKeyValueStore;
use kona_preimage::{PreimageKey, PreimageKeyType};
use op_alloy_genesis::RollupConfig;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use tempfile::TempDir;
use tokio::fs;
use tracing::info;
use zeth_core::mpt::{MptNode, MptNodeData};
use zeth_core::stateless::data::StatelessClientData;

/// The host binary CLI application arguments.
#[derive(Parser, Serialize, Clone, Debug)]
pub struct KailuaHostCli {
    #[clap(flatten)]
    pub kona: kona_host::HostCli,

    /// Address of OP-NODE endpoint to use
    #[clap(long)]
    pub op_node_address: Option<String>,
}

pub async fn generate_rollup_config(
    cfg: &mut KailuaHostCli,
    tmp_dir: &TempDir,
) -> anyhow::Result<RollupConfig> {
    // generate a RollupConfig for the target network
    match cfg
        .kona
        .l2_chain_id
        .map(RollupConfig::from_l2_chain_id)
        .flatten()
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
    let chain_config: Value = l2_node_provider
        .client()
        .request_noparams("debug_chainConfig")
        .await?;

    // genesis
    rollup_config["genesis"]["L1"] = rollup_config["genesis"]
        .as_object_mut()
        .unwrap()
        .remove("l1")
        .unwrap();
    rollup_config["genesis"]["L1"]["Hash"] = rollup_config["genesis"]["L1"]
        .as_object_mut()
        .unwrap()
        .remove("hash")
        .unwrap();
    rollup_config["genesis"]["L1"]["Number"] = rollup_config["genesis"]["L1"]
        .as_object_mut()
        .unwrap()
        .remove("number")
        .unwrap();
    rollup_config["genesis"]["L2"] = rollup_config["genesis"]
        .as_object_mut()
        .unwrap()
        .remove("l2")
        .unwrap();
    rollup_config["genesis"]["L2"]["Hash"] = rollup_config["genesis"]["L2"]
        .as_object_mut()
        .unwrap()
        .remove("hash")
        .unwrap();
    rollup_config["genesis"]["L2"]["Number"] = rollup_config["genesis"]["L2"]
        .as_object_mut()
        .unwrap()
        .remove("number")
        .unwrap();
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
    let mut res = vec![(node.hash(), alloy::rlp::encode(node))];
    match node.as_data() {
        MptNodeData::Branch(children) => {
            children
                .into_iter()
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
    mpt_to_vec(&mpt).into_iter().for_each(|(hash, data)| {
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
