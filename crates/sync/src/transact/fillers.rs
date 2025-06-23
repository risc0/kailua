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

use alloy::eips::eip4844::BLOB_TX_MIN_BLOB_GASPRICE;
use alloy::eips::{BlockId, BlockNumberOrTag};
use alloy::network::{Network, TransactionBuilder, TransactionBuilder4844};
use alloy::primitives::Address;
use alloy::providers::fillers::{
    BlobGasFiller, ChainIdFiller, FillerControlFlow, GasFillable, GasFiller, JoinFill, NonceFiller,
    NonceManager, TxFiller,
};
use alloy::providers::{Provider, SendableTx};
use alloy::transports::{RpcError, TransportResult};
use async_trait::async_trait;

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
        price + price * self.premium / 100
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

        Ok(tx.max(rpc * 2))
    }

    async fn fill(
        &self,
        fillable: Self::Fillable,
        tx: SendableTx<N>,
    ) -> TransportResult<SendableTx<N>> {
        self.inner.fill(self.make_premium(fillable), tx).await
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
