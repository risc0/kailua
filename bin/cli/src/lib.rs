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
use alloy::primitives::{Address, FixedBytes, Uint, U256};
use alloy::providers::{Provider, ReqwestProvider};
use alloy::transports::Transport;
use anyhow::Context;
// use kailua_contracts::KailuaGame::KailuaGameInstance;
use kailua_contracts::Safe::SafeInstance;
use std::str::FromStr;
use tracing::debug;

// pub mod bench;
pub mod channel;
pub mod db;
pub mod deploy;
// pub mod fault;
pub mod propose;
pub mod providers;
// pub mod validate;

pub const KAILUA_GAME_TYPE: u32 = 1337;

#[derive(clap::Parser, Debug, Clone)]
#[command(name = "kailua-cli")]
#[command(bin_name = "kailua-cli")]
#[command(author, version, about, long_about = None)]
pub enum Cli {
    Deploy(deploy::DeployArgs),
    Propose(propose::ProposeArgs),
    // Validate(validate::ValidateArgs),
    // TestFault(fault::FaultArgs),
    // Benchmark(bench::BenchArgs),
}

impl Cli {
    pub fn verbosity(&self) -> u8 {
        match self {
            Cli::Deploy(args) => args.v,
            Cli::Propose(args) => args.v,
            // Cli::Validate(args) => args.v,
            // Cli::TestFault(args) => args.propose_args.v,
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

pub async fn block_hash(
    l2_node_provider: &ReqwestProvider,
    block_number: u64,
) -> anyhow::Result<FixedBytes<32>> {
    let block: serde_json::Value = l2_node_provider
        .client()
        .request(
            "eth_getBlockByNumber",
            (format!("0x{:x}", block_number), false),
        )
        .await
        .context(format!("eth_getBlockByNumber {block_number}"))?;
    debug!("block_hash {:?}", &block);
    Ok(FixedBytes::<32>::from_str(
        block["hash"].as_str().expect("Failed to parse block hash"),
    )?)
}

// pub async fn derive_expected_journal<T: Transport + Clone, P: Provider<T, N>, N: Network>(
//     game_contract: &KailuaGameInstance<T, P, N>,
//     output_number: u32,
//     safe_output: B256,
//     proposed_output: B256,
//     computed_output: B256
// ) -> anyhow::Result<Vec<u8>> {
//     let l1_head = game_contract.l1Head().call().await?.l1Head_.0;
//     let parent_contract_address = game_contract.parentGame().call().await?.parentGame_;
//     let parent_contract =
//         KailuaGameInstance::new(parent_contract_address, game_contract.provider());
//     let l2_output_root = parent_contract.rootClaim().call().await?.rootClaim_.0;
//     let l2_claim = game_contract.rootClaim().call().await?.rootClaim_.0;
//     let l2_claim_block = game_contract
//         .l2BlockNumber()
//         .call()
//         .await?
//         .l2BlockNumber_
//         .to::<u64>()
//         .to_be_bytes();
//     let config_hash = game_contract.configHash().call().await?.configHash_.0;
//     Ok([
//         l1_head.as_slice(),
//         l2_output_root.as_slice(),
//         l2_claim.as_slice(),
//         l2_claim_block.as_slice(),
//         config_hash.as_slice(),
//     ]
//     .concat())
// }
