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

use crate::args::ProveArgs;
use crate::kv::RWLKeyValueStore;
use crate::ProvingError;
use alloy::consensus::Transaction;
use alloy::eips::eip4844::IndexedBlobHash;
use alloy::eips::BlockNumberOrTag;
use alloy::providers::{Provider, RootProvider};
use alloy_primitives::B256;
use anyhow::bail;
use kailua_common::blobs::BlobFetchRequest;
use kailua_common::precondition::PreconditionValidationData;
use kailua_sync::provider::optimism::OpNodeProvider;
use kona_genesis::RollupConfig;
use kona_preimage::{PreimageKey, PreimageKeyType};
use kona_protocol::BlockInfo;
use std::env::set_var;
use std::iter::zip;
use tracing::{error, info, warn};

pub async fn get_blob_fetch_request(
    l1_provider: &RootProvider,
    block_hash: B256,
    blob_hash: B256,
) -> anyhow::Result<BlobFetchRequest> {
    let block = l1_provider
        .get_block_by_hash(block_hash)
        .full()
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
    cfg: &ProveArgs,
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
                info!("Fetching blob hash {blob_hash} from block {block_hash}");
                fetch_requests
                    .push(get_blob_fetch_request(&providers.l1, *block_hash, *blob_hash).await?);
            }
            PreconditionValidationData::Validity {
                proposal_l2_head_number: cfg.precondition_params[0],
                proposal_output_count: cfg.precondition_params[1],
                output_block_span: cfg.precondition_params[2],
                blob_hashes: fetch_requests,
            }
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
    args: &ProveArgs,
    rollup_config: RollupConfig,
    op_node_provider: &OpNodeProvider,
    disk_kv_store: Option<RWLKeyValueStore>,
) -> anyhow::Result<()> {
    let l2_provider = args.kona.create_providers().await?.l2;
    let starting_block = l2_provider
        .get_block_by_hash(args.kona.agreed_l2_head_hash)
        .await?
        .unwrap()
        .header
        .number;
    let mut num_blocks = args.kona.claimed_l2_block_number - starting_block;
    if num_blocks == 0 {
        return Ok(());
    }
    let blocks_per_thread = num_blocks / args.proving.num_concurrent_preflights;
    let mut extra_blocks = num_blocks % args.proving.num_concurrent_preflights;
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
            .get_block_by_hash(args.kona.agreed_l2_head_hash)
            .await?
            .unwrap()
            .header
            .number
            + processed_blocks;
        args.kona.claimed_l2_output_root = op_node_provider
            .output_at_block(args.kona.claimed_l2_block_number)
            .await?;
        // queue and start new job
        jobs.push(tokio::spawn(crate::tasks::compute_cached_proof(
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
                .get_block_by_number(BlockNumberOrTag::Number(args.kona.claimed_l2_block_number))
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
            if !matches!(e, ProvingError::NotSeekingProof(..)) {
                error!("Error during preflight execution: {e:?}");
            }
        }
    }

    Ok(())
}
