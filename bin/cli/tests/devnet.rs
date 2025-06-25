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

#![cfg(feature = "devnet")]

use kailua_cli::fast_track::{fast_track, FastTrackArgs};
use kailua_sync::transact::signer::{DeployerSignerArgs, GuardianSignerArgs, OwnerSignerArgs};
use kailua_sync::transact::TransactArgs;
use std::env::set_var;
use std::process::ExitStatus;
use tokio::io;
use tokio::process::Command;
use tracing_subscriber::EnvFilter;

async fn make(recipe: &str) -> io::Result<ExitStatus> {
    let mut cmd = Command::new("make");
    cmd.args(vec!["-C", "../../optimism", recipe]);
    cmd.kill_on_drop(true)
        .spawn()
        .expect("Failed to spawn devnet up")
        .wait()
        .await
}

async fn start_devnet() -> anyhow::Result<()> {
    kona_cli::init_tracing_subscriber(3, None::<EnvFilter>)?;
    // start optimism devnet
    make("devnet-up").await?;
    println!("Optimism devnet deployed.");
    // fast-track upgrade w/ devmode proof support
    set_var("RISC0_DEV_MODE", "1");
    fast_track(FastTrackArgs {
        eth_rpc_url: "http://127.0.0.1:8545".to_string(),
        op_geth_url: "http://127.0.0.1:9545".to_string(),
        op_node_url: "http://127.0.0.1:7545".to_string(),
        txn_args: TransactArgs {
            txn_timeout: 12,
            exec_gas_premium: 0,
            blob_gas_premium: 0,
        },
        starting_block_number: 0,
        proposal_output_count: 5,
        output_block_span: 3,
        collateral_amount: 1,
        verifier_contract: None,
        challenge_timeout: 60,
        deployer_signer: DeployerSignerArgs::from(
            "0x4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356".to_string(),
        ),
        owner_signer: OwnerSignerArgs::from(
            "0x7c852118294e51e653712a81e05800f419141751be58f605c371e15141b007a6".to_string(),
        ),
        guardian_signer: Some(GuardianSignerArgs::from(
            "0x2a871d0798f97d79848a013d4936a73bf4cc922c825d33c1cf7073dff6d409c6".to_string(),
        )),
        vanguard_address: Some("0x9965507D1a55bcC2695C58ba16FB37d819B0A4dc".to_string()),
        vanguard_advantage: Some(60),
        respect_kailua_proposals: true,
        telemetry: Default::default(),
    })
    .await?;
    println!("Kailua contracts installed");
    Ok(())
}

async fn stop_devnet() {
    match make("devnet-down").await {
        Ok(exit_code) => {
            println!("1/2 Complete: {exit_code:?}")
        }
        Err(err) => {
            println!("1/2 Error: {err:?}")
        }
    }
    match make("devnet-clean").await {
        Ok(exit_code) => {
            println!("2/2 Complete: {exit_code:?}")
        }
        Err(err) => {
            println!("2/2 Error: {err:?}")
        }
    }
}

async fn start_devnet_or_clean() {
    if let Err(err) = start_devnet().await {
        eprintln!("Error: {err}");
        stop_devnet().await;
    }
}

#[tokio::test]
async fn devnet_happy_path() {
    // Start the optimism devnet
    start_devnet_or_clean().await;

    // Stop and discard the devnet
    stop_devnet().await;
}
