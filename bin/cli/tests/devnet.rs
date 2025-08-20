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

use alloy::eips::{BlockId, BlockNumberOrTag};
use alloy::network::BlockResponse;
use alloy::providers::Provider;
use kailua_cli::fast_track::{fast_track, FastTrackArgs};
use kailua_cli::fault::{fault, FaultArgs};
use kailua_proposer::args::ProposeArgs;
use kailua_proposer::propose::propose;
use kailua_prover::args::{ProveArgs, ProvingArgs};
use kailua_prover::prove::prove;
use kailua_sync::agent::SyncAgent;
use kailua_sync::args::SyncArgs;
use kailua_sync::provider::ProviderArgs;
use kailua_sync::transact::signer::{
    DeployerSignerArgs, GuardianSignerArgs, OwnerSignerArgs, ProposerSignerArgs,
    ValidatorSignerArgs,
};
use kailua_sync::transact::TransactArgs;
use kailua_validator::args::ValidateArgs;
use kailua_validator::validate::validate;
use lazy_static::lazy_static;
use std::env::set_var;
use std::process::ExitStatus;
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tokio::{io, try_join};
use tracing_subscriber::EnvFilter;

lazy_static! {
    static ref DEVNET: Arc<Mutex<()>> = Default::default();
}

async fn make(recipe: &str) -> io::Result<ExitStatus> {
    let mut cmd = Command::new("make");
    cmd.args(vec!["-C", "../../optimism", recipe]);
    cmd.kill_on_drop(true)
        .spawn()
        .expect("Failed to spawn devnet up")
        .wait()
        .await
}

async fn deploy_kailua_contracts(challenge_timeout: u64) -> anyhow::Result<()> {
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
        challenge_timeout,
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
        bypass_chain_registry: false,
    })
    .await?;
    println!("Kailua contracts installed");
    Ok(())
}

async fn start_devnet() -> anyhow::Result<()> {
    // print out INFO logs
    if let Err(err) = kona_cli::init_tracing_subscriber(3, None::<EnvFilter>) {
        eprintln!("Failed to set up tracing: {err:?}");
    }
    // start optimism devnet
    make("devnet-up").await?;
    println!("Optimism devnet deployed.");
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

#[tokio::test(flavor = "multi_thread")]
async fn proposer_validator() {
    // We can only run one of these dockerized devnets at a time
    let devnet_lock = DEVNET.lock().await;
    sleep(Duration::from_secs(5)).await;

    // Start the optimism devnet
    start_devnet_or_clean().await;
    // update dgf to use kailua
    deploy_kailua_contracts(60).await.unwrap();

    // Instantiate sync arguments
    let tmp_dir = tempdir().unwrap();
    let proposer_data_dir = tmp_dir.path().join("proposer").to_path_buf();
    let sync = SyncArgs {
        provider: ProviderArgs {
            eth_rpc_url: "http://127.0.0.1:8545".to_string(),
            op_geth_url: "http://127.0.0.1:9545".to_string(),
            op_node_url: "http://127.0.0.1:7545".to_string(),
            op_rpc_delay: 0,
            beacon_rpc_url: "http://127.0.0.1:5052".to_string(),
        },
        kailua_game_implementation: None,
        kailua_anchor_address: None,
        final_l2_block: Some(60),
        data_dir: Some(proposer_data_dir.clone()),
        telemetry: Default::default(),
    };

    // Instantiate transacting arguments
    let txn_args = TransactArgs {
        txn_timeout: 30,
        exec_gas_premium: 25,
        blob_gas_premium: 25,
    };

    // Instantiate proposer wallet
    let proposer_signer = ProposerSignerArgs::from(
        "0x8b3a350cf5c34c9194ca85829a2df0ec3153be0318b5e2d3348e872092edffba".to_string(),
    );

    // Run the proposer until block 60
    propose(
        ProposeArgs {
            sync: sync.clone(),
            bypass_chain_registry: false,
            proposer_signer: proposer_signer.clone(),
            txn_args: txn_args.clone(),
        },
        proposer_data_dir.clone(),
    )
    .await
    .unwrap();

    // wait until block 75 is available
    let mut agent = SyncAgent::new(&sync.provider, proposer_data_dir.clone(), None, None, false)
        .await
        .unwrap();
    loop {
        agent.sync(0, Some(75)).await.unwrap();
        if agent.cursor.last_output_index >= 75 {
            break;
        }
        // Wait for more blocks to be confirmed
        sleep(Duration::from_secs(2)).await;
    }
    // release proposer db
    let fault_parent = agent.cursor.last_resolved_game;
    drop(agent);

    // submit an output fault
    fault(FaultArgs {
        propose_args: ProposeArgs {
            sync: sync.clone(),
            bypass_chain_registry: false,
            proposer_signer: ProposerSignerArgs::from(
                "0x4bbbf85ce3377467afe5d46f804f221813b2bb87f24d81f60f1fcdbf7cbf4356".to_string(),
            ),
            txn_args: txn_args.clone(),
        },
        fault_offset: 1,
        fault_parent,
    })
    .await
    .unwrap();

    // submit a trail fault
    fault(FaultArgs {
        propose_args: ProposeArgs {
            sync: sync.clone(),
            bypass_chain_registry: false,
            proposer_signer: ProposerSignerArgs::from(
                "0xdbda1821b80551c9d65939329250298aa3472ba22feea921c0cf5d620ea67b97".to_string(),
            ),
            txn_args: txn_args.clone(),
        },
        fault_offset: 250,
        fault_parent,
    })
    .await
    .unwrap();

    // new sync target at block 90
    let sync = SyncArgs {
        final_l2_block: Some(90),
        ..sync
    };

    // Run the proposer and validator until block 90
    let validator_data_dir = tmp_dir.path().join("validator").to_path_buf();
    let validator_handle = tokio::task::spawn(validate(
        ValidateArgs {
            sync: SyncArgs {
                data_dir: Some(validator_data_dir.clone()),
                ..sync.clone()
            },
            kailua_cli: None,
            fast_forward_start: 0,
            fast_forward_target: 0,
            num_concurrent_provers: 1,
            enable_experimental_witness_endpoint: true,
            max_fault_proving_delay: 0,
            max_validity_proving_delay: 0,
            l1_head_jump_back: 0,
            validator_signer: ValidatorSignerArgs::from(
                "0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e".to_string(),
            ),
            txn_args: txn_args.clone(),
            proving: ProvingArgs {
                payout_recipient_address: None,
                segment_limit: 21,
                max_witness_size: 2_684_354_560,
                num_concurrent_preflights: 1,
                num_concurrent_proofs: 1,
                bypass_chain_registry: true,
                skip_derivation_proof: false,
                skip_await_proof: false,
                clear_cache_data: true,
                hokulea: Default::default(),
                hana: Default::default(),
            },
            boundless: Default::default(),
        },
        3,
        validator_data_dir.clone(),
    ));
    let proposer_handle = tokio::task::spawn(propose(
        ProposeArgs {
            sync: sync.clone(),
            bypass_chain_registry: false,
            proposer_signer: proposer_signer.clone(),
            txn_args: txn_args.clone(),
        },
        proposer_data_dir.clone(),
    ));
    println!("Waiting for proposer and validator to terminate.");
    // Wait for both agents to hit termination condition
    let (validator, proposer) = try_join!(validator_handle, proposer_handle).unwrap();
    validator.unwrap();
    proposer.unwrap();

    // Deploy new set of Kailua contracts for validity proving
    deploy_kailua_contracts(u64::MAX).await.unwrap();
    // Run the proposer and validator until block 90
    let validator_data_dir = tmp_dir.path().join("validator").to_path_buf();
    let validator_handle = tokio::task::spawn(validate(
        ValidateArgs {
            sync: SyncArgs {
                data_dir: Some(validator_data_dir.clone()),
                ..sync.clone()
            },
            kailua_cli: None,
            fast_forward_start: 0,
            fast_forward_target: 90, // run validity proofs until block 90 is finalized
            num_concurrent_provers: 5,
            enable_experimental_witness_endpoint: true,
            max_fault_proving_delay: 0,
            max_validity_proving_delay: 0,
            l1_head_jump_back: 0,
            validator_signer: ValidatorSignerArgs::from(
                "0x92db14e403b83dfe3df233f83dfa3a0d7096f21ca9b0d6d6b8d88b2b4ec1564e".to_string(),
            ),
            txn_args: txn_args.clone(),
            proving: ProvingArgs {
                payout_recipient_address: None,
                segment_limit: 21,
                max_witness_size: 2_684_354_560,
                num_concurrent_preflights: 1,
                num_concurrent_proofs: 1,
                bypass_chain_registry: true,
                skip_derivation_proof: false,
                skip_await_proof: false,
                clear_cache_data: true,
                hokulea: Default::default(),
                hana: Default::default(),
            },
            boundless: Default::default(),
        },
        3,
        validator_data_dir.clone(),
    ));
    let proposer_handle = tokio::task::spawn(propose(
        ProposeArgs {
            sync: sync.clone(),
            bypass_chain_registry: false,
            proposer_signer: proposer_signer.clone(),
            txn_args: txn_args.clone(),
        },
        proposer_data_dir.clone(),
    ));
    println!("Waiting for proposer and validator to terminate.");
    // Wait for both agents to hit termination condition
    let (validator, proposer) = try_join!(validator_handle, proposer_handle).unwrap();
    validator.unwrap();
    proposer.unwrap();

    // Stop and discard the devnet
    stop_devnet().await;
    drop(devnet_lock);
}

#[tokio::test(flavor = "multi_thread")]
async fn prover() {
    const PROOF_SIZE: u64 = 200;

    // We can only run one of these dockerized devnets at a time
    let devnet_lock = DEVNET.lock().await;
    sleep(Duration::from_secs(5)).await;

    // Start the optimism devnet
    start_devnet_or_clean().await;
    // update dgf to use kailua
    deploy_kailua_contracts(60).await.unwrap();

    // Instantiate sync arguments
    let tmp_dir = tempdir().unwrap();
    let data_dir = tmp_dir.path().join("agent").to_path_buf();
    let sync = SyncArgs {
        provider: ProviderArgs {
            eth_rpc_url: "http://127.0.0.1:8545".to_string(),
            op_geth_url: "http://127.0.0.1:9545".to_string(),
            op_node_url: "http://127.0.0.1:7545".to_string(),
            op_rpc_delay: 0,
            beacon_rpc_url: "http://127.0.0.1:5052".to_string(),
        },
        kailua_game_implementation: None,
        kailua_anchor_address: None,
        final_l2_block: Some(60),
        data_dir: Some(data_dir.clone()),
        telemetry: Default::default(),
    };

    // Wait for 200 blocks to be provable
    println!("Waiting for l2 block #{PROOF_SIZE} to be safe.");
    let mut agent = SyncAgent::new(&sync.provider, data_dir.clone(), None, None, false)
        .await
        .unwrap();
    loop {
        agent.sync(0, Some(PROOF_SIZE)).await.unwrap();
        if agent.cursor.last_output_index >= PROOF_SIZE {
            break;
        }
        // Wait for more blocks to be confirmed
        sleep(Duration::from_secs(2)).await;
    }
    println!("Proving l2 block #{PROOF_SIZE} since genesis");

    // Prove 200 blocks with very little memory
    let l1_head = agent
        .provider
        .l1_provider
        .get_block(BlockId::Number(BlockNumberOrTag::Latest))
        .await
        .unwrap()
        .unwrap()
        .header()
        .hash;
    let agreed_l2_head_hash = agent
        .provider
        .l2_provider
        .get_block(BlockId::Number(BlockNumberOrTag::Number(0)))
        .await
        .unwrap()
        .unwrap()
        .header()
        .hash;
    let agreed_l2_output_root = agent.provider.op_provider.output_at_block(0).await.unwrap();
    let claimed_l2_output_root = agent
        .provider
        .op_provider
        .output_at_block(PROOF_SIZE)
        .await
        .unwrap();
    prove(ProveArgs {
        kona: kona_host::single::SingleChainHost {
            l1_head,
            agreed_l2_head_hash,
            agreed_l2_output_root,
            claimed_l2_output_root,
            claimed_l2_block_number: PROOF_SIZE,
            l2_node_address: Some(sync.provider.op_geth_url),
            l1_node_address: Some(sync.provider.eth_rpc_url),
            l1_beacon_address: Some(sync.provider.beacon_rpc_url),
            data_dir: Some(tmp_dir.path().join("prover").to_path_buf()),
            native: true,
            server: false,
            l2_chain_id: Some(agent.config.l2_chain_id),
            rollup_config_path: None,
            enable_experimental_witness_endpoint: false,
        },
        op_node_address: Some(sync.provider.op_node_url),
        proving: ProvingArgs {
            payout_recipient_address: None,
            segment_limit: 21,
            max_witness_size: 5 * 1024 * 1024, // 5 MB witness maximum
            num_concurrent_preflights: 4,
            num_concurrent_proofs: 2,
            bypass_chain_registry: false,
            skip_derivation_proof: false,
            skip_await_proof: false,
            clear_cache_data: true,
            hokulea: Default::default(),
            hana: Default::default(),
        },
        boundless: Default::default(),
        precondition_params: vec![],
        precondition_block_hashes: vec![],
        precondition_blob_hashes: vec![],
        telemetry: Default::default(),
    })
    .await
    .unwrap();

    // Stop and discard the devnet
    stop_devnet().await;
    drop(devnet_lock);
}
