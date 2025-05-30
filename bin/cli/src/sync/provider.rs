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

use crate::transact::blob::BlobProvider;
use crate::CoreArgs;
use alloy_provider::RootProvider;
use anyhow::Context;
use kailua_client::await_tel;
use kailua_client::provider::OpNodeProvider;
use opentelemetry::trace::FutureExt;
use opentelemetry::trace::{TraceContextExt, Tracer};

pub struct SyncProvider {
    pub da_provider: BlobProvider,
    pub l1_provider: RootProvider,
    pub op_provider: OpNodeProvider,
    pub l2_provider: RootProvider,
}

impl SyncProvider {
    pub async fn new(core_args: &CoreArgs) -> anyhow::Result<Self> {
        let tracer = opentelemetry::global::tracer("kailua");
        let context = opentelemetry::Context::current_with_span(tracer.start("SyncProvider::new"));

        let da_provider = await_tel!(context, BlobProvider::new(core_args.beacon_rpc_url.clone()))
            .context("BlobProvider::new")?;
        let l1_provider = RootProvider::new_http(core_args.eth_rpc_url.as_str().try_into()?);
        let op_provider = OpNodeProvider(RootProvider::new_http(
            core_args.op_node_url.as_str().try_into()?,
        ));
        let l2_provider = RootProvider::new_http(core_args.op_geth_url.as_str().try_into()?);

        Ok(Self {
            da_provider,
            l1_provider,
            op_provider,
            l2_provider,
        })
    }
}
