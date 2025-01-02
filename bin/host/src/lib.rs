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
use alloy_eips::eip4844::IndexedBlobHash;
use anyhow::bail;
use boundless_market::storage::StorageProviderConfig;
use clap::Parser;
use kailua_client::{parse_b256, BoundlessArgs};
use kailua_common::blobs::BlobFetchRequest;
use kailua_common::precondition::PreconditionValidationData;
use kona_host::kv::SharedKeyValueStore;
use kona_preimage::{BidirectionalChannel, HintWriter, OracleReader, PreimageKey, PreimageKeyType};
use op_alloy_genesis::RollupConfig;
use op_alloy_protocol::BlockInfo;
use op_alloy_registry::Registry;
use serde_json::{json, Value};
use std::env::set_var;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::RwLock;
use tokio::{fs, task};
use tracing::{debug, info, warn};
use zeth_core::driver::CoreDriver;
use zeth_core::mpt::{MptNode, MptNodeData};
use zeth_core::stateless::data::StatelessClientData;
use zeth_core_optimism::OpRethCoreDriver;
use zeth_preflight::client::PreflightClient;
use zeth_preflight_optimism::OpRethPreflightClient;

#[cfg(feature = "eigenda")]
use {
    anyhow::anyhow,
    hokulea_host::{
        eigenda_blobs::OnlineEigenDABlobProvider, eigenda_fetcher::FetcherWithEigenDASupport,
        start_native_preimage_server,
    },
};

#[cfg(not(feature = "eigenda"))]
use kona_host::{fetcher::Fetcher, start_native_preimage_server};

/// The host binary CLI application arguments.
#[derive(Parser, Clone, Debug)]
pub struct KailuaHostCli {
    #[clap(flatten)]
    pub kona: kona_host::HostCli,

    /// Address of OP-NODE endpoint to use
    #[clap(long, env)]
    pub op_node_address: Option<String>,
    /// Whether to skip running the zeth preflight engine
    #[clap(long, default_value_t = false, env)]
    pub skip_zeth_preflight: bool,

    #[clap(long, default_value_t = 1, env)]
    /// Number of blocks to build in a single proof
    pub block_count: u64,
    #[clap(long, value_parser = parse_b256, env)]
    pub u_block_hash: Option<B256>,
    #[clap(long, value_parser = parse_b256, env)]
    pub u_blob_kzg_hash: Option<B256>,
    #[clap(long, value_parser = parse_b256, env)]
    pub v_block_hash: Option<B256>,
    #[clap(long, value_parser = parse_b256, env)]
    pub v_blob_kzg_hash: Option<B256>,

    #[clap(flatten)]
    pub boundless_args: Option<BoundlessArgs>,
    /// Storage provider to use for elf and input
    #[clap(flatten)]
    pub boundless_storage_config: Option<StorageProviderConfig>,

    #[clap(flatten)]
    pub eigenda_args: EigenDAArgs,
}

#[derive(Parser, Debug, Clone)]
pub struct EigenDAArgs {
    /// URL of the Ethereum RPC endpoint.
    #[clap(long, env)]
    #[arg(required = false)]
    pub eigenda_proxy_address: Option<String>,
}

/// Starts the [PreimageServer] and the client program in separate threads. The client program is
/// ran natively in this mode.
///
/// ## Takes
/// - `cfg`: The host configuration.
///
/// ## Returns
/// - `Ok(exit_code)` if the client program exits successfully.
/// - `Err(_)` if the client program failed to execute, was killed by a signal, or the host program
///   exited first.
pub async fn start_server_and_native_client(
    args: KailuaHostCli,
    precondition_validation_data_hash: B256,
) -> anyhow::Result<i32> {
    let hint_chan = BidirectionalChannel::new()?;
    let preimage_chan = BidirectionalChannel::new()?;
    let kv_store = args.kona.construct_kv_store();

    #[cfg(feature = "eigenda")]
    let fetcher = if !args.kona.is_offline() {
        let (l1_provider, blob_provider, l2_provider) = args.kona.create_providers().await?;
        let eigenda_blob_provider = OnlineEigenDABlobProvider::new_http(
            args.eigenda_args
                .eigenda_proxy_address
                .expect("missing eigenda_proxy_address")
                .clone(),
        )
        .await
        .map_err(|e| anyhow!("Failed to load eigenda blob provider configuration: {e}"))?;
        Some(Arc::new(RwLock::new(
            FetcherWithEigenDASupport::new_from_parts(
                kv_store.clone(),
                l1_provider,
                blob_provider,
                eigenda_blob_provider,
                l2_provider,
                args.kona.agreed_l2_head_hash,
            ),
        )))
    } else {
        None
    };

    #[cfg(not(feature = "eigenda"))]
    let fetcher = if !args.kona.is_offline() {
        let (l1_provider, blob_provider, l2_provider) = args.kona.create_providers().await?;
        Some(Arc::new(RwLock::new(Fetcher::new(
            kv_store.clone(),
            l1_provider,
            blob_provider,
            l2_provider,
            args.kona.agreed_l2_head_hash,
        ))))
    } else {
        None
    };

    // Create the server and start it.
    let server_task = task::spawn(start_native_preimage_server(
        kv_store,
        fetcher,
        hint_chan.host,
        preimage_chan.host,
    ));

    // Start the client program in a separate child process.
    let program_task = task::spawn(kailua_client::run_client(
        args.boundless_args,
        args.boundless_storage_config,
        OracleReader::new(preimage_chan.client),
        HintWriter::new(hint_chan.client),
        precondition_validation_data_hash,
    ));

    // Execute both tasks and wait for them to complete.
    info!("Starting preimage server and client program.");
    let (_, client_result) = tokio::try_join!(server_task, program_task,)?;
    info!(target: "kona_host", "Preimage server and client program have joined.");

    Ok(client_result.is_err() as i32)
}

pub async fn generate_rollup_config(
    cfg: &mut KailuaHostCli,
    tmp_dir: &TempDir,
) -> anyhow::Result<RollupConfig> {
    // generate a RollupConfig for the target network
    match cfg.kona.read_rollup_config().ok() {
        Some(rollup_config) => Ok(rollup_config),
        None => {
            let registry = Registry::from_chain_list();
            let tmp_cfg_file = tmp_dir.path().join("rollup-config.json");
            if let Some(rollup_config) = cfg
                .kona
                .l2_chain_id
                .and_then(|chain_id| registry.rollup_configs.get(&chain_id))
            {
                info!(
                    "Loading config for rollup with chain id {} from registry",
                    cfg.kona.l2_chain_id.unwrap()
                );
                let ser_config = serde_json::to_string(rollup_config)?;
                fs::write(&tmp_cfg_file, &ser_config).await?;
            } else {
                info!("Fetching rollup config from nodes.");
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
            }
            cfg.kona.rollup_config_path = Some(tmp_cfg_file);
            cfg.kona.read_rollup_config()
        }
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
    let mut blob_index = 0;
    for blob in block.transactions.into_transactions().flat_map(|tx| {
        tx.blob_versioned_hashes()
            .map(|h| h.to_vec())
            .unwrap_or_default()
    }) {
        if blob == blob_hash {
            break;
        }
        blob_index += 1;
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
