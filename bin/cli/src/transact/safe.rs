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

use crate::transact::Transact;
use alloy::contract::SolCallBuilder;
use alloy::network::{Network, TransactionBuilder};
use alloy::primitives::{Address, Uint, U256};
use alloy_provider::Provider;
use anyhow::Context;

pub async fn exec_safe_txn<T, P1: Provider<N>, P2: Provider<N>, C, N: Network>(
    txn: SolCallBuilder<T, P1, C, N>,
    safe: &kailua_contracts::Safe::SafeInstance<(), P2, N>,
    from: Address,
) -> anyhow::Result<()> {
    let req = txn.into_transaction_request();
    safe.execTransaction(
        req.to().unwrap(),
        req.value().unwrap_or_default(),
        req.input().cloned().unwrap_or_default(),
        0,
        Uint::from(req.gas_limit().unwrap_or_default()),
        U256::ZERO,
        U256::ZERO,
        Address::ZERO,
        Address::ZERO,
        [
            [0u8; 12].as_slice(),
            from.as_slice(),
            [0u8; 32].as_slice(),
            [1u8].as_slice(),
        ]
        .concat()
        .into(),
    )
    .transact("Safe::execTransaction", None)
    .await
    .context("Safe::execTransaction")?;
    Ok(())
}
