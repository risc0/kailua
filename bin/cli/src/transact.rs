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
use alloy::providers::fillers::{FillerControlFlow, TxFiller};
use alloy::providers::{Provider, SendableTx};
use alloy::transports::TransportResult;
use alloy_provider::fillers::{GasFillable, GasFiller};
use anyhow::Context;
use async_trait::async_trait;
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use std::future::IntoFuture;
use std::time::Duration;

#[derive(clap::Args, Debug, Clone)]
pub struct TransactArgs {
    /// Transaction Confirmation Timeout
    #[clap(long, env, value_parser = parse_duration)]
    pub txn_timeout: Option<Duration>,
}

fn parse_duration(arg: &str) -> Result<Duration, std::num::ParseIntError> {
    Ok(Duration::from_secs(arg.parse()?))
}

#[async_trait]
pub trait Transact<N: Network> {
    async fn transact(
        &self,
        span: &'static str,
        timeout: Option<Duration>,
    ) -> anyhow::Result<N::ReceiptResponse>;

    async fn timed_transact_with_context(
        &self,
        context: opentelemetry::Context,
        span: &'static str,
        timeout: Option<Duration>,
    ) -> anyhow::Result<N::ReceiptResponse> {
        self.transact(span, timeout).with_context(context).await
    }

    async fn transact_with_context(
        &self,
        context: opentelemetry::Context,
        span: &'static str,
    ) -> anyhow::Result<N::ReceiptResponse> {
        self.timed_transact_with_context(context, span, None).await
    }
}

#[async_trait]
impl<
        'coder,
        T: Sync + Send + 'static,
        P: Provider<N>,
        D: CallDecoder + Send + Sync + 'static,
        N: Network,
    > Transact<N> for CallBuilder<T, P, D, N>
where
    EthCall<'coder, D, N>: IntoFuture,
{
    async fn transact(
        &self,
        span: &'static str,
        timeout: Option<Duration>,
    ) -> anyhow::Result<N::ReceiptResponse> {
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(tracer.start(span));

        self.send()
            .with_context(context.with_span(tracer.start_with_context("send", &context)))
            .await
            .context("send")?
            .with_timeout(timeout)
            .get_receipt()
            .with_context(context.with_span(tracer.start_with_context("get_receipt", &context)))
            .await
            .context("get_receipt")
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PriorityGasFiller {
    pub inner: GasFiller,
}

impl<N: Network> TxFiller<N> for PriorityGasFiller {
    type Fillable = GasFillable;

    fn status(&self, tx: &N::TransactionRequest) -> FillerControlFlow {
        self.inner.status(tx)
    }

    fn fill_sync(&self, tx: &mut SendableTx<N>) {
        self.inner.fill_sync(tx)
    }

    async fn prepare<P: Provider<N>>(
        &self,
        provider: &P,
        tx: &N::TransactionRequest,
    ) -> TransportResult<Self::Fillable> {
        self.inner.prepare(provider, tx).await
    }

    async fn fill(
        &self,
        fillable: Self::Fillable,
        tx: SendableTx<N>,
    ) -> TransportResult<SendableTx<N>> {
        self.inner.fill(fillable, tx).await
    }
}
