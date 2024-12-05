// Copyright 2024 RISC Zero, Inc.
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

use alloy::contract::SolCallBuilder;
use alloy::network::{Network, TransactionBuilder};
use alloy::primitives::{Address, Uint, U256};
use alloy::providers::Provider;
use alloy::transports::Transport;
use kailua_contracts::Safe::SafeInstance;

// pub mod bench;
pub mod channel;
pub mod db;
pub mod deploy;
pub mod fault;
pub mod propose;
pub mod providers;
pub mod stall;
pub mod validate;

pub const KAILUA_GAME_TYPE: u32 = 1337;

#[derive(clap::Parser, Debug, Clone)]
#[command(name = "kailua-cli")]
#[command(bin_name = "kailua-cli")]
#[command(author, version, about, long_about = None)]
pub enum Cli {
    Deploy(deploy::DeployArgs),
    Propose(propose::ProposeArgs),
    Validate(validate::ValidateArgs),
    TestFault(fault::FaultArgs),
    // Benchmark(bench::BenchArgs),
}

impl Cli {
    pub fn verbosity(&self) -> u8 {
        match self {
            Cli::Deploy(args) => args.v,
            Cli::Propose(args) => args.v,
            Cli::Validate(args) => args.v,
            Cli::TestFault(args) => args.propose_args.v,
            // Cli::Benchmark(args) => args.v,
        }
    }
}

pub async fn exec_safe_txn<
    T: Transport + Clone,
    P1: Provider<T, N>,
    P2: Provider<T, N>,
    C,
    N: Network,
>(
    txn: SolCallBuilder<T, P1, C, N>,
    safe: &SafeInstance<T, P2, N>,
    from: Address,
) -> anyhow::Result<()> {
    let req = txn.into_transaction_request();
    let value = req.value().unwrap_or_default();
    safe.execTransaction(
        req.to().unwrap(),
        value,
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
    .send()
    .await?
    .get_receipt()
    .await?;
    Ok(())
}
