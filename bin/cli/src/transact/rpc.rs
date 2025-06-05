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

use crate::retry_res_ctx_timeout;
use alloy::consensus::BlockHeader;
use alloy::eips::{BlockId, BlockNumberOrTag};
use alloy::network::{BlockResponse, Network};
use alloy::primitives::{BlockNumber, B256};
use alloy_provider::Provider;
use anyhow::{anyhow, Context};
use kailua_client::await_tel;
use opentelemetry::global::tracer;
use opentelemetry::trace::FutureExt;
use opentelemetry::trace::{TraceContextExt, Tracer};

pub async fn get_next_block<P: Provider<N>, N: Network>(
    provider: P,
    parent_hash: B256,
) -> anyhow::Result<N::BlockResponse> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("get_next_block"));

    let block_parent = await_tel!(
        context,
        tracer,
        "Provider::get_block_by_hash",
        retry_res_ctx_timeout!(provider
            .get_block_by_hash(parent_hash)
            .await
            .context("get_block_by_hash")?
            .ok_or_else(|| anyhow!("Failed to fetch parent block")))
    );
    let parent_number = block_parent.header().number();
    let block = await_tel!(context, get_block_by_number(&provider, parent_number + 1))?;

    Ok(block)
}

pub async fn get_block_by_number<P: Provider<N>, N: Network>(
    provider: P,
    block_number: BlockNumber,
) -> anyhow::Result<N::BlockResponse> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("get_block_by_number"));

    let block = await_tel!(
        context,
        tracer,
        "Provider::get_block_by_number",
        retry_res_ctx_timeout!(provider
            .get_block_by_number(BlockNumberOrTag::Number(block_number))
            .await
            .context("get_block_by_number")?
            .ok_or_else(|| anyhow!("Failed to fetch block")))
    );

    Ok(block)
}

pub async fn get_block<P: Provider<N>, N: Network>(
    provider: P,
    block_id: BlockNumberOrTag,
) -> anyhow::Result<N::BlockResponse> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("get_block"));

    let block = await_tel!(
        context,
        tracer,
        "Provider::get_block",
        retry_res_ctx_timeout!(provider
            .get_block(BlockId::Number(block_id))
            .await
            .context("get_block")?
            .ok_or_else(|| anyhow!("Failed to fetch block")))
    );

    Ok(block)
}
