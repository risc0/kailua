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

use crate::kv::{create_split_kv_store, RWLKeyValueStore};
use crate::ProvingError;
use alloy::eips::BlockNumberOrTag;
use alloy::providers::{Provider, RootProvider};
use alloy_primitives::hex::FromHex;
use alloy_primitives::keccak256;
use anyhow::{anyhow, Context};
use kailua_sync::provider::optimism::OpNodeProvider;
use kailua_sync::{await_tel, retry_res_ctx_timeout};
use kona_host::KeyValueStore;
use kona_preimage::{PreimageKey, PreimageKeyType};
use kona_proof::BootInfo;
use opentelemetry::global::tracer;
use opentelemetry::trace::FutureExt;
use opentelemetry::trace::{TraceContextExt, Tracer};
use std::error::Error;
use std::ops::DerefMut;
use tracing::{error, info};

pub async fn run_payload_client(
    mut boot_info: BootInfo,
    l2_provider: RootProvider,
    op_node_provider: OpNodeProvider,
    disk_kv_store: Option<RWLKeyValueStore>,
) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("run_payload_client"));

    let kv = create_split_kv_store(&Default::default(), disk_kv_store)
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;

    while boot_info.claimed_l2_output_root != boot_info.agreed_l2_output_root {
        // Read block hash
        let block_hash = await_tel!(
            context,
            tracer,
            "l2_provider get_block_by_number claimed_l2_block_number",
            retry_res_ctx_timeout!(l2_provider
                .get_block_by_number(BlockNumberOrTag::Number(boot_info.claimed_l2_block_number))
                .await
                .context("l2_provider get_block_by_number claimed_l2_block_number")?
                .ok_or_else(|| anyhow!("Failed to claimed l2 block")))
        )
        .header
        .hash;
        // Go back one block
        boot_info.claimed_l2_block_number -= 1;
        boot_info.claimed_l2_output_root = op_node_provider
            .output_at_block(boot_info.claimed_l2_block_number)
            .await?;

        // Check if block payload had already been processed
        let kv_lock = kv.read().await;
        let exec_wit_key = PreimageKey::new(block_hash.0, PreimageKeyType::GlobalGeneric);
        if kv_lock.get(exec_wit_key.into()).is_some() {
            info!(
                "Payload for {} already processed.",
                boot_info.claimed_l2_block_number + 1
            );
            continue;
        }
        drop(kv_lock);

        let mut retries = 5;
        let Ok(execution_witness) = (loop {
            let attempt = l2_provider
                .client()
                .request::<(BlockNumberOrTag,), serde_json::Value>(
                    "debug_executionWitness",
                    (BlockNumberOrTag::Number(
                        boot_info.claimed_l2_block_number + 1,
                    ),),
                )
                .await;
            retries -= 1;
            if attempt.is_ok() || retries == 0 {
                break attempt;
            }
            let attempt = attempt.unwrap_err();
            error!(
                "Failed to fetch payload for {} (Retry)\n{:?}.",
                boot_info.claimed_l2_block_number + 1,
                attempt.source()
            );
        }) else {
            // Allow this hint to fail silently, as not all execution clients support
            // the `debug_executePayload` method.
            error!(
                "Failed to fetch payload for {} (Skip).",
                boot_info.claimed_l2_block_number + 1
            );
            continue;
        };

        // dump preimages into kv store
        let mut kv_lock = kv.write().await;
        dump_payload_to_kv_store(&execution_witness, kv_lock.deref_mut());
        // Mark block payload as processed in kv store
        kv_lock.set(exec_wit_key.into(), vec![])?;
        info!(
            "Fetched payload for {}.",
            boot_info.claimed_l2_block_number + 1
        );
        drop(kv_lock);
    }

    Ok(())
}

fn dump_payload_to_kv_store(payload: &serde_json::Value, kv: &mut dyn KeyValueStore) {
    if let Some(obj) = payload.as_object() {
        for (k, v) in obj {
            save_hex_preimage_to_kv(k, kv);
            dump_payload_to_kv_store(v, kv);
        }
    }
    if let Some(seq) = payload.as_array() {
        for v in seq {
            dump_payload_to_kv_store(v, kv);
        }
    }
    if let Some(v) = payload.as_str() {
        save_hex_preimage_to_kv(v, kv);
    }
}

fn save_hex_preimage_to_kv(preimage: &str, kv: &mut dyn KeyValueStore) {
    if let Ok(preimage) = alloy_primitives::Bytes::from_hex(preimage) {
        let computed_hash = keccak256(preimage.as_ref());
        let key = PreimageKey::new_keccak256(*computed_hash);
        kv.set(key.into(), preimage.into()).unwrap();
    }
}
