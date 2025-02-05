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

use alloy::contract::{CallBuilder, CallDecoder, EthCall};
use alloy::network::Network;
use alloy::providers::Provider;
use alloy::transports::Transport;
use anyhow::Context;
use async_trait::async_trait;
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use std::future::IntoFuture;

#[async_trait]
pub trait Transact<N: Network> {
    async fn transact(&self) -> anyhow::Result<N::ReceiptResponse>;
}

#[async_trait]
impl<
        'req,
        'coder,
        T: Transport + Clone,
        P: Provider<T, N>,
        D: CallDecoder + Send + Sync + 'static,
        N: Network,
    > Transact<N> for CallBuilder<T, P, D, N>
where
    EthCall<'req, 'coder, D, T, N>: IntoFuture,
{
    async fn transact(&self) -> anyhow::Result<N::ReceiptResponse> {
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current();
        self.send()
            .with_context(context.with_span(tracer.start_with_context("send", &context)))
            .await
            .context("send")?
            .get_receipt()
            .with_context(context.with_span(tracer.start_with_context("get_receipt", &context)))
            .await
            .context("get_receipt")
    }
}
