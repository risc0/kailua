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

use crate::args::KailuaHostArgs;
use crate::kv::RWLKeyValueStore;
use alloy::consensus::Transaction;
use alloy::network::primitives::BlockTransactionsKind;
use alloy::providers::{Provider, RootProvider};
use alloy_chains::NamedChain;
use alloy_eips::eip4844::IndexedBlobHash;
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::{keccak256, B256};
use anyhow::bail;
use kailua_client::provider::OpNodeProvider;
use kailua_client::proving::ProvingError;
use kailua_common::blobs::BlobFetchRequest;
use kailua_common::precondition::PreconditionValidationData;
use kona_genesis::RollupConfig;
use kona_host::SharedKeyValueStore;
use kona_preimage::{PreimageKey, PreimageKeyType};
use kona_protocol::BlockInfo;
use std::env::set_var;
use std::iter::zip;
use tracing::{error, info, warn};
use zeth_core::driver::CoreDriver;
use zeth_core::mpt::{MptNode, MptNodeData};
use zeth_core::stateless::data::StatelessClientData;
use zeth_core_optimism::OpRethCoreDriver;
use zeth_preflight::client::PreflightClient;
use zeth_preflight_optimism::OpRethPreflightClient;

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
    cfg: &KailuaHostArgs,
    rollup_config: RollupConfig,
) -> anyhow::Result<()> {
    if let Ok(named_chain) = NamedChain::try_from(rollup_config.l2_chain_id) {
        // Limitation: Only works when disk caching is enabled under a known "NamedChain"
        if !cfg.kona.is_offline()
            && cfg.kona.data_dir.is_some()
            && OpRethCoreDriver::chain_spec(&named_chain).is_some()
        {
            // Fetch all the initial data
            let preflight_data: StatelessClientData<
                <OpRethCoreDriver as CoreDriver>::Block,
                <OpRethCoreDriver as CoreDriver>::Header,
            > = {
                info!("Performing zeth-optimism preflight.");
                let providers = cfg.kona.create_providers().await?;
                let preflight_start = providers
                    .l2
                    .get_block_by_hash(cfg.kona.agreed_l2_head_hash, BlockTransactionsKind::Hashes)
                    .await?
                    .unwrap()
                    .header
                    .number;
                let block_count = cfg.kona.claimed_l2_block_number - preflight_start;

                let data_dir = cfg.kona.data_dir.clone();
                let l2_node_address = cfg.kona.l2_node_address.clone();
                tokio::task::spawn_blocking(move || {
                    // Prepare the cache directory
                    let cache_dir = data_dir.clone().map(|dir| dir.join("optimism"));
                    if let Some(dir) = cache_dir.as_ref() {
                        std::fs::create_dir_all(dir).expect("Could not create directory");
                    };
                    OpRethPreflightClient::preflight(
                        Some(rollup_config.l2_chain_id),
                        cache_dir,
                        l2_node_address.clone(),
                        preflight_start,
                        block_count,
                    )
                })
                .await??
            };
            // Write data to the cached Kona kv-store
            let mut kv_store = cfg.kona.create_key_value_store()?;
            dump_data_to_kv_store(&mut kv_store, &preflight_data).await;
        }
    } else {
        warn!(
            "Unknown chain-id {}. Skipping zeth-preflight.",
            rollup_config.l2_chain_id
        );
    }
    Ok(())
}

pub async fn get_blob_fetch_request(
    l1_provider: &RootProvider,
    block_hash: B256,
    blob_hash: B256,
) -> anyhow::Result<BlobFetchRequest> {
    let block = l1_provider
        .get_block_by_hash(block_hash, BlockTransactionsKind::Full)
        .await?
        .expect("Failed to fetch block {block_hash}.");
    let mut blob_index = 0;
    let mut blob_found = false;
    for blob in block.transactions.into_transactions().flat_map(|tx| {
        tx.blob_versioned_hashes()
            .map(|h| h.to_vec())
            .unwrap_or_default()
    }) {
        if blob == blob_hash {
            blob_found = true;
            break;
        }
        blob_index += 1;
    }

    if !blob_found {
        bail!("Could not find blob with hash {blob_hash} in block {block_hash}");
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
    cfg: &KailuaHostArgs,
) -> anyhow::Result<Option<PreconditionValidationData>> {
    // Determine precondition hash
    let hash_arguments = [
        cfg.precondition_params.is_empty(),
        cfg.precondition_block_hashes.is_empty(),
        cfg.precondition_blob_hashes.is_empty(),
    ];

    // fetch necessary data to validate blob equivalence precondition
    if hash_arguments.iter().all(|arg| !arg) {
        let providers = cfg.kona.create_providers().await?;
        if cfg.precondition_block_hashes.len() != cfg.precondition_blob_hashes.len() {
            bail!(
                "Blob reference mismatch. Found {} block hashes and {} blob hashes",
                cfg.precondition_block_hashes.len(),
                cfg.precondition_blob_hashes.len()
            );
        }

        let precondition_validation_data = if cfg.precondition_params.len() == 3 {
            let mut fetch_requests = Vec::with_capacity(cfg.precondition_block_hashes.len());
            for (block_hash, blob_hash) in zip(
                cfg.precondition_block_hashes.iter(),
                cfg.precondition_blob_hashes.iter(),
            ) {
                fetch_requests
                    .push(get_blob_fetch_request(&providers.l1, *block_hash, *blob_hash).await?);
            }
            PreconditionValidationData::Validity(
                cfg.precondition_params[0],
                cfg.precondition_params[1],
                cfg.precondition_params[2],
                fetch_requests,
            )
        } else {
            bail!("Too many precondition_params values provided");
        };

        let kv_store = cfg.kona.create_key_value_store()?;
        let mut store = kv_store.write().await;
        let hash = precondition_validation_data.hash();
        store.set(
            PreimageKey::new(*hash, PreimageKeyType::Sha256).into(),
            precondition_validation_data.to_vec(),
        )?;
        set_var("PRECONDITION_VALIDATION_DATA_HASH", hash.to_string());
        info!("Precondition data hash: {hash}");
        Ok(Some(precondition_validation_data))
    } else if hash_arguments.iter().any(|arg| !arg) {
        bail!("Insufficient number of arguments provided for precondition hash.")
    } else {
        warn!("Proving without a precondition hash.");
        Ok(None)
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn concurrent_execution_preflight(
    args: &KailuaHostArgs,
    rollup_config: RollupConfig,
    op_node_provider: &OpNodeProvider,
    disk_kv_store: Option<RWLKeyValueStore>,
) -> anyhow::Result<()> {
    let l2_provider = args.kona.create_providers().await?.l2;
    let starting_block = l2_provider
        .get_block_by_hash(args.kona.agreed_l2_head_hash, BlockTransactionsKind::Hashes)
        .await?
        .unwrap()
        .header
        .number;
    let mut num_blocks = args.kona.claimed_l2_block_number - starting_block;
    if num_blocks == 0 {
        return Ok(());
    }
    let blocks_per_thread = num_blocks / args.num_concurrent_preflights;
    let mut extra_blocks = num_blocks % args.num_concurrent_preflights;
    let mut jobs = vec![];
    let mut args = args.clone();
    while num_blocks > 0 {
        let processed_blocks = if extra_blocks > 0 {
            extra_blocks -= 1;
            blocks_per_thread + 1
        } else {
            blocks_per_thread
        };
        num_blocks = num_blocks.saturating_sub(processed_blocks);

        // update ending block
        args.kona.claimed_l2_block_number = l2_provider
            .get_block_by_hash(args.kona.agreed_l2_head_hash, BlockTransactionsKind::Hashes)
            .await?
            .unwrap()
            .header
            .number
            + processed_blocks;
        args.kona.claimed_l2_output_root = op_node_provider
            .output_at_block(args.kona.claimed_l2_block_number)
            .await?;
        // queue new job
        jobs.push(tokio::spawn(crate::prove::compute_cached_proof(
            args.clone(),
            rollup_config.clone(),
            disk_kv_store.clone(),
            B256::ZERO,
            B256::ZERO,
            vec![],
            vec![],
            vec![],
            false,
            true,
            false,
        )));
        // jobs.push(args.clone());
        // update starting block for next job
        if num_blocks > 0 {
            args.kona.agreed_l2_head_hash = l2_provider
                .get_block_by_number(
                    BlockNumberOrTag::Number(args.kona.claimed_l2_block_number),
                    BlockTransactionsKind::Hashes,
                )
                .await?
                .unwrap()
                .header
                .hash;
            args.kona.agreed_l2_output_root = args.kona.claimed_l2_output_root;
        }
    }
    // Await all tasks
    for job in jobs {
        let result = job.await?;
        if let Err(e) = result {
            if !matches!(e, ProvingError::SeekProofError(..)) {
                error!("Error during preflight execution: {e:?}");
            }
        }
    }

    Ok(())
}
