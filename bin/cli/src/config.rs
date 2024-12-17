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

use alloy::primitives::address;
use anyhow::Context;
use kailua_build::KAILUA_FPVM_ID;
use kailua_common::client::config_hash;
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
}

pub async fn config(args: ConfigArgs) -> anyhow::Result<()> {
    // report rollup config hash
    let config = fetch_rollup_config(&args.op_node_url, &args.op_geth_url, None)
        .await
        .context("fetch_rollup_config")?;
    let rollup_config_hash = config_hash(&config).expect("Configuration hash derivation error");
    println!(
        "ROLLUP_CONFIG_HASH: 0x{}",
        hex::encode_upper(rollup_config_hash)
    );
    // report fpvm image id
    println!(
        "FPVM_IMAGE_ID: 0x{}",
        hex::encode_upper(Digest::new(KAILUA_FPVM_ID).as_bytes())
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

    Ok(())
}
