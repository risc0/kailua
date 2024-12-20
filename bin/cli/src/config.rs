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

use crate::stall::Stall;
use crate::{BN254_CONTROL_ID, CONTROL_ROOT, KAILUA_GAME_TYPE, SET_BUILDER_ID};
use alloy::primitives::address;
use alloy::providers::ProviderBuilder;
use anyhow::Context;
use kailua_build::KAILUA_FPVM_ID;
use kailua_common::client::config_hash;
use kailua_contracts::SystemConfig;
use kailua_host::fetch_rollup_config;
use risc0_zkvm::sha::Digest;

#[derive(clap::Args, Debug, Clone)]
pub struct ConfigArgs {
    #[arg(long, short, help = "Verbosity level (0-4)", action = clap::ArgAction::Count)]
    pub v: u8,

    /// URL of OP-NODE endpoint to use
    #[clap(long, env)]
    pub op_node_url: String,
    /// URL of OP-GETH endpoint to use (eth and debug namespace required).
    #[clap(long, env)]
    pub op_geth_url: String,
    /// Address of the ethereum rpc endpoint to use (eth namespace required)
    #[clap(long, env)]
    pub eth_rpc_url: String,
}

pub async fn config(args: ConfigArgs) -> anyhow::Result<()> {
    let config = fetch_rollup_config(&args.op_node_url, &args.op_geth_url, None)
        .await
        .context("fetch_rollup_config")?;
    let eth_rpc_provider = ProviderBuilder::new().on_http(args.eth_rpc_url.as_str().try_into()?);
    // load system config
    let system_config = SystemConfig::new(config.l1_system_config_address, &eth_rpc_provider);
    let portal_address = system_config.optimismPortal().stall().await.addr_;
    let dgf_address = system_config.disputeGameFactory().stall().await.addr_;

    // report risc0 version
    println!("RISC0_VERSION: {}", risc0_zkvm::get_version()?);
    // report fpvm image id
    println!(
        "FPVM_IMAGE_ID: 0x{}",
        hex::encode_upper(Digest::new(KAILUA_FPVM_ID).as_bytes())
    );
    // Report expected Groth16 verifier parameters
    println!(
        "CONTROL_ROOT: 0x{}",
        hex::encode_upper(CONTROL_ROOT.as_slice()),
    );
    println!(
        "CONTROL_ID: 0x{}",
        hex::encode_upper(BN254_CONTROL_ID.as_slice()),
    );
    // Report expected Boundless verifier parameters
    println!(
        "SET_BUILDER_ID: 0x{}",
        hex::encode_upper(SET_BUILDER_ID.as_slice())
    );
    // report verifier address
    let verifier_address = match config.l1_chain_id {
        // eth
        1 => Some(address!("8EaB2D97Dfce405A1692a21b3ff3A172d593D319")),
        11155111 => Some(address!("925d8331ddc0a1F0d96E68CF073DFE1d92b69187")),
        17000 => Some(address!("f70aBAb028Eb6F4100A24B203E113D94E87DE93C")),
        // arb
        42161 => Some(address!("0b144e07a0826182b6b59788c34b32bfa86fb711")),
        421614 => Some(address!("0b144e07a0826182b6b59788c34b32bfa86fb711")),
        // ava
        43114 => Some(address!("0b144e07a0826182b6b59788c34b32bfa86fb711")),
        43113 => Some(address!("0b144e07a0826182b6b59788c34b32bfa86fb711")),
        // base
        8453 => Some(address!("0b144e07a0826182b6b59788c34b32bfa86fb711")),
        84532 => Some(address!("0b144e07a0826182b6b59788c34b32bfa86fb711")),
        // op
        10 => Some(address!("0b144e07a0826182b6b59788c34b32bfa86fb711")),
        11155420 => Some(address!("B369b4dd27FBfb59921d3A4a3D23AC2fc32FB908")),
        // linea
        59144 => Some(address!("0b144e07a0826182b6b59788c34b32bfa86fb711")),
        // ploygon
        1101 => Some(address!("0b144e07a0826182b6b59788c34b32bfa86fb711")),
        _ => None,
    };
    println!(
        "RISC_ZERO_VERIFIER: 0x{}",
        verifier_address
            .map(|a| hex::encode_upper(a.as_slice()))
            .unwrap_or_default()
    );

    // report genesis time
    println!("GENESIS_TIMESTAMP: {}", config.genesis.l2_time);
    // report inter-block time
    println!("BLOCK_TIME: {}", config.block_time);
    // report rollup config hash
    let rollup_config_hash = config_hash(&config).expect("Configuration hash derivation error");
    println!(
        "ROLLUP_CONFIG_HASH: 0x{}",
        hex::encode_upper(rollup_config_hash)
    );
    // report factory address
    println!(
        "DISPUTE_GAME_FACTORY: 0x{}",
        hex::encode_upper(dgf_address.as_slice())
    );
    // report portal address
    println!(
        "OPTIMISM_PORTAL: 0x{}",
        hex::encode_upper(portal_address.as_slice())
    );
    // report game type
    println!("KAILUA_GAME_TYPE: {}", KAILUA_GAME_TYPE);

    Ok(())
}
