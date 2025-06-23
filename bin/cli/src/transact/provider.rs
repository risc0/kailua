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

use crate::transact::rpc::get_block;
use alloy::consensus::{BlockHeader, Transaction};
use alloy::eips::{BlockId, BlockNumberOrTag};
use alloy::network::{BlockResponse, Ethereum, Network, TransactionBuilder4844};
use alloy::transports::TransportResult;
use alloy_provider::fillers::{FillProvider, TxFiller};
use alloy_provider::network::TransactionBuilder;
use alloy_provider::{PendingTransactionBuilder, Provider, RootProvider};
use async_trait::async_trait;
use kailua_game::await_tel;
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use tracing::info;

#[derive(Debug, Clone)]
pub struct SafeProvider<P> {
    /// Inner provider.
    inner: P,
}

impl<P> SafeProvider<P> {
    pub fn new(inner: P) -> Self {
        Self { inner }
    }

    pub fn provider(&self) -> &P {
        &self.inner
    }
}

#[async_trait]
impl<F: TxFiller<Ethereum>, P: Provider<Ethereum>> Provider<Ethereum>
    for SafeProvider<FillProvider<F, P, Ethereum>>
where
    P: Provider<Ethereum>,
{
    fn root(&self) -> &RootProvider<Ethereum> {
        self.inner.root()
    }

    async fn send_transaction(
        &self,
        tx: <Ethereum as Network>::TransactionRequest,
    ) -> TransportResult<PendingTransactionBuilder<Ethereum>> {
        let mut fee_factor = 1.0;
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(
            tracer.start("Proposal::fetch_current_challenger_duration"),
        );

        loop {
            let mut tx = tx.clone();
            // Get latest block
            let latest_block = await_tel!(
                context,
                get_block(self.provider(), BlockNumberOrTag::Latest)
            )
            .header()
            .number();
            info!("Testing transaction viability under block {latest_block}");

            // Recover signer
            let envelope = self
                .inner
                .fill(tx.clone())
                .await?
                .as_envelope()
                .cloned()
                .unwrap();
            let sender = envelope.recover_signer().unwrap();

            // Ensure call success
            self.call(tx.clone().with_from(sender))
                .block(BlockId::Number(BlockNumberOrTag::Number(latest_block)))
                .await?;

            // Set nonce to that as of successful call block
            tx.set_nonce(
                self.inner
                    .get_transaction_count(sender)
                    .block_id(BlockId::Number(BlockNumberOrTag::Number(latest_block)))
                    .await?,
            );

            info!(
                "Broadcasting transaction with nonce {} and fee factor {fee_factor}",
                tx.nonce.unwrap_or_default()
            );

            // scale fees
            if let Some(fee) = envelope.max_priority_fee_per_gas() {
                tx.set_max_priority_fee_per_gas((fee as f64 * fee_factor) as u128);
            }
            if let Some(fee) = envelope.max_fee_per_blob_gas() {
                tx.set_max_fee_per_blob_gas((fee as f64 * fee_factor) as u128);
            }
            tx.set_max_fee_per_gas((envelope.max_fee_per_gas() as f64 * fee_factor) as u128);

            // attempt broadcast
            match self.inner.send_transaction(tx).await {
                Ok(res) => break Ok(res),
                Err(err) => {
                    if !err.to_string().contains("underpriced") {
                        break Err(err);
                    }
                    // increase fees
                    fee_factor *= 1.1;
                }
            }
        }
    }
}
