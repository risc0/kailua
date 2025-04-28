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
use alloy::eips::eip4844::BLOB_TX_MIN_BLOB_GASPRICE;
use alloy::eips::{BlockId, BlockNumberOrTag};
use alloy::network::{Network, TransactionBuilder4844};
use alloy::primitives::Address;
use alloy::providers::fillers::{FillerControlFlow, TxFiller};
use alloy::providers::{Provider, SendableTx};
use alloy::transports::{RpcError, TransportResult};
use alloy_provider::fillers::{
    BlobGasFiller, ChainIdFiller, GasFillable, GasFiller, JoinFill, NonceFiller, NonceManager,
};
use alloy_provider::network::TransactionBuilder;
use alloy_provider::{Identity, ProviderBuilder};
use anyhow::Context;
use async_trait::async_trait;
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

        // Require call to succeed against pending block
        self.call_raw()
            .block(BlockId::Number(BlockNumberOrTag::Pending))
            .into_future()
            .with_context(context.with_span(tracer.start_with_context("call_raw", &context)))
            .await
            .context("call_raw")?;

        // Publish transaction
        let pending_txn = self
            .send()
            .with_context(context.with_span(tracer.start_with_context("send", &context)))
            .await
            .context("send")?;
        info!("Published transaction: {:?}", pending_txn.tx_hash());

        // Wait for receipt with timeout
        pending_txn
            .with_timeout(timeout)
            .get_receipt()
            .with_context(context.with_span(tracer.start_with_context("get_receipt", &context)))
            .await
            .context("get_receipt")
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PremiumExecGasFiller {
    pub inner: GasFiller,
    pub premium: u128,
}

impl PremiumExecGasFiller {
    pub fn with_premium(premium: u128) -> Self {
        Self {
            inner: Default::default(),
            premium,
        }
    }

    pub fn make_premium(&self, price: u128) -> u128 {
        let price = price.max(1);
        price + price * self.premium.max(1) / 100
    }
}

impl<N: Network> TxFiller<N> for PremiumExecGasFiller {
    type Fillable = GasFillable;

    fn status(&self, tx: &N::TransactionRequest) -> FillerControlFlow {
        <GasFiller as TxFiller<N>>::status(&self.inner, tx)
    }

    fn fill_sync(&self, tx: &mut SendableTx<N>) {
        self.inner.fill_sync(tx);
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
        let mut tx = self.inner.fill(fillable, tx).await?;
        if let Some(builder) = tx.as_mut_builder() {
            if let Some(gas_price) = builder.gas_price() {
                builder.set_gas_price(self.make_premium(gas_price));
            }
            if let Some(base_fee) = builder.max_fee_per_gas() {
                builder.set_max_fee_per_gas(self.make_premium(base_fee));
            }
            if let Some(priority_fee) = builder.max_priority_fee_per_gas() {
                builder.set_max_priority_fee_per_gas(self.make_premium(priority_fee));
            }
        }
        Ok(tx)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PremiumBlobGasFiller {
    pub inner: BlobGasFiller,
    pub premium: u128,
}

impl PremiumBlobGasFiller {
    pub fn with_premium(premium: u128) -> Self {
        Self {
            inner: Default::default(),
            premium,
        }
    }

    pub fn make_premium(&self, price: u128) -> u128 {
        let price = price.max(1);
        price + price * self.premium.max(1) / 100
    }
}

impl<N: Network> TxFiller<N> for PremiumBlobGasFiller
where
    N::TransactionRequest: TransactionBuilder4844,
{
    type Fillable = u128;

    fn status(&self, tx: &N::TransactionRequest) -> FillerControlFlow {
        <BlobGasFiller as TxFiller<N>>::status(&self.inner, tx)
    }

    fn fill_sync(&self, tx: &mut SendableTx<N>) {
        self.inner.fill_sync(tx);
    }

    async fn prepare<P: Provider<N>>(
        &self,
        provider: &P,
        tx: &N::TransactionRequest,
    ) -> TransportResult<Self::Fillable> {
        let tx = tx
            .max_fee_per_blob_gas()
            .unwrap_or(BLOB_TX_MIN_BLOB_GASPRICE);

        let rpc = provider
            .get_fee_history(5, BlockNumberOrTag::Latest, &[])
            .await?
            .base_fee_per_blob_gas
            .iter()
            .max()
            .ok_or(RpcError::NullResp)
            .copied()?;

        Ok(tx.max(rpc))
    }

    async fn fill(
        &self,
        fillable: Self::Fillable,
        tx: SendableTx<N>,
    ) -> TransportResult<SendableTx<N>> {
        let mut tx = self.inner.fill(fillable, tx).await?;
        if let Some(builder) = tx.as_mut_builder() {
            if let Some(blob_base_fee) = builder.max_fee_per_blob_gas() {
                builder.set_max_fee_per_blob_gas(self.make_premium(blob_base_fee));
            }
        }
        Ok(tx)
    }
}

#[derive(Clone, Debug, Default)]
pub struct LatestNonceManager;

#[async_trait]
impl NonceManager for LatestNonceManager {
    async fn get_next_nonce<P, N>(&self, provider: &P, address: Address) -> TransportResult<u64>
    where
        P: Provider<N>,
        N: Network,
    {
        provider
            .get_transaction_count(address)
            .block_id(BlockId::Number(BlockNumberOrTag::Latest))
            .await
    }
}

pub type PremiumFiller = JoinFill<
    PremiumExecGasFiller,
    JoinFill<PremiumBlobGasFiller, JoinFill<NonceFiller<LatestNonceManager>, ChainIdFiller>>,
>;

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
