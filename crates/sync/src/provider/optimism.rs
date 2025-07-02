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

use crate::await_tel;
use alloy::primitives::B256;
use alloy::providers::{Provider, ProviderBuilder, RootProvider};
use anyhow::Context;
use kona_genesis::RollupConfig;
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::str::FromStr;
use tokio::fs;
use tracing::debug;

#[derive(Clone)]
pub struct OpNodeProvider(pub RootProvider);

impl OpNodeProvider {
    pub async fn output_at_block(&self, output_block_number: u64) -> anyhow::Result<B256> {
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(
            tracer.start("OpNodeProvider::output_at_block"),
        );

        let output_at_block: Value = await_tel!(
            context,
            tracer,
            "optimism_outputAtBlock",
            self.0.client().request(
                "optimism_outputAtBlock",
                (format!("0x{output_block_number:x}"),),
            )
        )
        .context(format!("optimism_outputAtBlock {output_block_number}"))?;

        Ok(B256::from_str(
            output_at_block["outputRoot"].as_str().unwrap(),
        )?)
    }

    pub async fn sync_status(&self) -> anyhow::Result<Value> {
        let tracer = tracer("kailua");
        let context =
            opentelemetry::Context::current_with_span(tracer.start("OpNodeProvider::sync_status"));

        Ok(await_tel!(
            context,
            tracer,
            "optimism_syncStatus",
            self.0.client().request_noparams("optimism_syncStatus")
        )?)
    }

    pub async fn rollup_config(&self) -> anyhow::Result<Value> {
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(
            tracer.start("OpNodeProvider::rollup_config"),
        );

        Ok(await_tel!(
            context,
            tracer,
            "optimism_rollupConfig",
            self.0.client().request_noparams("optimism_rollupConfig")
        )?)
    }
}

pub async fn fetch_rollup_config(
    op_node_address: &str,
    l2_node_address: &str,
    json_file_path: Option<&PathBuf>,
) -> anyhow::Result<RollupConfig> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("fetch_rollup_config"));

    let op_node_provider = OpNodeProvider(RootProvider::new_http(op_node_address.try_into()?));
    let l2_node_provider = ProviderBuilder::new().connect_http(l2_node_address.try_into()?);

    let mut rollup_config: Value = op_node_provider
        .rollup_config()
        .with_context(context.clone())
        .await
        .context("rollup_config")?;

    debug!("Rollup config: {:?}", rollup_config);

    let chain_config: Value =
        l2_node_provider
            .client()
            .request_noparams("debug_chainConfig")
            .with_context(context.with_span(
                tracer.start_with_context("ReqwestProvider::debug_chainConfig", &context),
            ))
            .await
            .context("debug_chainConfig")?;

    debug!("ChainConfig: {:?}", chain_config);

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
