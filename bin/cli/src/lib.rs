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
use alloy::primitives::{b256, Address, Uint, B256, U256};
use alloy::providers::Provider;
use alloy::transports::Transport;
use kailua_contracts::Safe::SafeInstance;
use sha2::Digest;
use std::path::PathBuf;

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

pub const CONTROL_ROOT: B256 =
    b256!("8cdad9242664be3112aba377c5425a4df735eb1c6966472b561d2855932c0469");
pub const BN254_CONTROL_ID: B256 =
    b256!("04446e66d300eb7fb45c9726bb53c793dda407a62e9601618bb43c5c14657ac0");
pub const SET_BUILDER_ID: B256 =
    b256!("744cca56cde6933dea72752c78b4a6ca894ed620e8af6437ab05fad53bcec40a");

#[derive(clap::Parser, Debug, Clone)]
#[command(name = "kailua-cli")]
#[command(bin_name = "kailua-cli")]
#[command(author, version, about, long_about = None)]
#[allow(clippy::large_enum_variant)]
pub enum Cli {
    Deploy(deploy::DeployArgs),
    Propose(propose::ProposeArgs),
    Validate(validate::ValidateArgs),
    TestFault(fault::FaultArgs),
    // Benchmark(bench::BenchArgs),
}

#[derive(clap::Args, Debug, Clone)]
pub struct CoreArgs {
    #[arg(long, short, help = "Verbosity level (0-4)", action = clap::ArgAction::Count)]
    pub v: u8,

    /// Address of OP-NODE endpoint to use
    #[clap(long, env)]
    pub op_node_address: String,
    /// Address of L1 JSON-RPC endpoint to use (eth namespace required)
    #[clap(long, env)]
    pub l1_node_address: String,
    /// Address of the L1 Beacon API endpoint to use.
    #[clap(long, env)]
    pub l1_beacon_address: String,

    /// Address of the L1 `AnchorStateRegistry` contract
    #[clap(long, env)]
    pub registry_contract: String,

    /// Directory to use for caching data
    #[clap(long, env)]
    pub data_dir: Option<PathBuf>,
}

impl Cli {
    pub fn verbosity(&self) -> u8 {
        match self {
            Cli::Deploy(args) => args.v,
            Cli::Propose(args) => args.core.v,
            Cli::Validate(args) => args.core.v,
            Cli::TestFault(args) => args.propose_args.core.v,
            // Cli::Benchmark(args) => args.v,
        }
    }

    pub fn data_dir(&self) -> Option<PathBuf> {
        match self {
            Cli::Propose(args) => args.core.data_dir.clone(),
            Cli::Validate(args) => args.core.data_dir.clone(),
            _ => None,
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

pub fn set_verifier_selector(image_id: B256) -> [u8; 4] {
    let tag = sha2::Sha256::digest("risc0.SetInclusionReceiptVerifierParameters");
    let len = (1u16 << 8).to_be_bytes();
    let input = [tag.as_slice(), image_id.as_slice(), len.as_slice()].concat();
    let digest = sha2::Sha256::digest(&input);
    digest.as_slice()[..4].try_into().unwrap()
}
