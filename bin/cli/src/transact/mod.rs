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

pub mod fillers;
pub mod provider;
pub mod rpc;
pub mod safe;
pub mod signer;

use alloy::contract::{CallBuilder, CallDecoder, EthCall};
use alloy::network::{Network, TransactionBuilder4844};
use alloy::providers::Provider;
use alloy_provider::fillers::JoinFill;
use alloy_provider::{Identity, ProviderBuilder};
use anyhow::Context;
use async_trait::async_trait;
use fillers::{PremiumBlobGasFiller, PremiumExecGasFiller, PremiumFiller};
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use std::future::IntoFuture;
use std::time::Duration;
use tracing::info;

#[derive(clap::Args, Debug, Clone)]
pub struct TransactArgs {
    /// Transaction Confirmation Timeout
    #[clap(long, env, required = false, default_value_t = 120)]
    pub txn_timeout: u64,
    /// Execution Gas Fee Premium
    #[clap(long, env, required = false, default_value_t = 25)]
    pub exec_gas_premium: u128,
    /// Blob Gas Fee Premium
    #[clap(long, env, required = false, default_value_t = 25)]
    pub blob_gas_premium: u128,
}

impl TransactArgs {
    pub fn premium_provider<N: Network>(
        &self,
    ) -> ProviderBuilder<Identity, JoinFill<Identity, PremiumFiller>>
    where
        N::TransactionRequest: TransactionBuilder4844,
    {
        premium_provider::<N>(self.exec_gas_premium, self.blob_gas_premium)
    }
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
impl<'coder, P: Provider<N>, D: CallDecoder + Send + Sync + 'static, N: Network> Transact<N>
    for CallBuilder<P, D, N>
where
    CallBuilder<P, D, N>: Clone,
    EthCall<'coder, D, N>: IntoFuture,
{
    async fn transact(
        &self,
        span: &'static str,
        timeout: Option<Duration>,
    ) -> anyhow::Result<N::ReceiptResponse> {
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(tracer.start(span));

        // Publish transaction
        let pending_txn = self
            .send()
            .with_context(context.with_span(tracer.start_with_context("send", &context)))
            .await
            .context("send")?;
        info!("Transaction published: {:?}", pending_txn.tx_hash());

        // Wait for receipt with timeout
        pending_txn
            .with_timeout(timeout)
            .get_receipt()
            .with_context(context.with_span(tracer.start_with_context("get_receipt", &context)))
            .await
            .context("get_receipt")
    }
}

pub fn premium_provider<N: Network>(
    premium_exec_gas: u128,
    premium_blob_gas: u128,
) -> ProviderBuilder<Identity, JoinFill<Identity, PremiumFiller>>
where
    N::TransactionRequest: TransactionBuilder4844,
{
    ProviderBuilder::default().filler(JoinFill::new(
        PremiumExecGasFiller::with_premium(premium_exec_gas),
        JoinFill::new(
            PremiumBlobGasFiller::with_premium(premium_blob_gas),
            Default::default(),
        ),
    ))
}
