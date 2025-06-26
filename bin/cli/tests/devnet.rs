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
use kailua_proposer::args::ProposeArgs;
use kailua_proposer::propose::propose;
use kailua_sync::args::SyncArgs;
use kailua_sync::provider::ProviderArgs;
use kailua_sync::transact::signer::{
    DeployerSignerArgs, GuardianSignerArgs, OwnerSignerArgs, ProposerSignerArgs,
    ValidatorSignerArgs,
};
use kailua_sync::transact::TransactArgs;
use kailua_validator::args::ValidateArgs;
use kailua_validator::validate::validate;
use std::env::set_var;
use std::process::ExitStatus;
use tempfile::tempdir;
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
    // Start the upgraded optimism devnet
    start_devnet_or_clean().await;

    // Instantiate sync arguments
    let tmp_dir = tempdir().unwrap();
    let data_dir = tmp_dir.path().to_path_buf();
    let sync = SyncArgs {
        provider: ProviderArgs {
            eth_rpc_url: "http://127.0.0.1:8545".to_string(),
            op_geth_url: "http://127.0.0.1:9545".to_string(),
            op_node_url: "http://127.0.0.1:7545".to_string(),
            beacon_rpc_url: "http://127.0.0.1:5052".to_string(),
        },
        kailua_game_implementation: None,
        kailua_anchor_address: None,
        delay_l2_blocks: 0,
        final_l2_block: Some(60),
        data_dir: Some(data_dir.clone()),
        telemetry: Default::default(),
    };

    // Instantiate transacting arguments
    let txn_args = TransactArgs {
        txn_timeout: 30,
        exec_gas_premium: 25,
        blob_gas_premium: 25,
    };

    // Run the proposer until block 60
    propose(
        ProposeArgs {
            sync: sync.clone(),
            proposer_signer: ProposerSignerArgs::from(
                "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba".to_string(),
            ),
            txn_args: txn_args.clone(),
        },
        data_dir.clone(),
    )
    .await
    .unwrap();

    // Run the validator until block 60
    validate(
        ValidateArgs {
            sync: sync.clone(),
            kailua_cli: None,
            fast_forward_target: 0,
            num_concurrent_provers: 1,
            l1_head_jump_back: 0,
            validator_signer: ValidatorSignerArgs::from(
                "0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e".to_string(),
            ),
            txn_args: txn_args.clone(),
            payout_recipient_address: None,
            boundless: Default::default(),
        },
        3,
        data_dir.clone(),
    )
    .await
    .unwrap();

    // Stop and discard the devnet
    stop_devnet().await;
}
