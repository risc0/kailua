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

use crate::propose::ProposeArgs;
use crate::transact::Transact;
use alloy::eips::eip4844::FIELD_ELEMENTS_PER_BLOB;
use alloy::network::Ethereum;
use alloy::primitives::{Bytes, B256, U256};
use alloy::providers::RootProvider;
use alloy::sol_types::SolValue;
use anyhow::Context;
use kailua_common::blobs::hash_to_fe;
use kailua_common::config::config_hash;
use kailua_contracts::*;
use kailua_game::proposal::Proposal;
use kailua_game::provider::optimism::fetch_rollup_config;
use kailua_game::provider::optimism::OpNodeProvider;
use kailua_game::stall::Stall;
use kailua_game::{await_tel, await_tel_res, retry_res_ctx_timeout, KAILUA_GAME_TYPE};
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use tracing::{error, info};

#[derive(clap::Args, Debug, Clone)]
pub struct FaultArgs {
    #[clap(flatten)]
    pub propose_args: ProposeArgs,

    /// Offset of the faulty block within the proposal
    #[clap(long, env)]
    pub fault_offset: u64,

    /// Index of the parent of the faulty proposal
    #[clap(long, env)]
    pub fault_parent: u64,
}

pub async fn fault(args: FaultArgs) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("fault"));

    let op_node_provider = OpNodeProvider(RootProvider::new_http(
        args.propose_args
            .core
            .provider
            .op_node_url
            .as_str()
            .try_into()?,
    ));
    let eth_rpc_provider = RootProvider::<Ethereum>::new_http(
        args.propose_args
            .core
            .provider
            .eth_rpc_url
            .as_str()
            .try_into()?,
    );

    info!("Fetching rollup configuration from rpc endpoints.");
    // fetch rollup config
    let config = await_tel!(
        context,
        fetch_rollup_config(
            &args.propose_args.core.provider.op_node_url,
            &args.propose_args.core.provider.op_geth_url,
            None,
        )
    )
    .context("fetch_rollup_config")?;
    let rollup_config_hash = config_hash(&config).expect("Configuration hash derivation error");
    info!("RollupConfigHash({})", hex::encode(rollup_config_hash));

    // load system config
    let system_config = SystemConfig::new(config.l1_system_config_address, &eth_rpc_provider);
    let dgf_address = system_config
        .disputeGameFactory()
        .stall_with_context(context.clone(), "SystemConfig::disputeGameFactory")
        .await;

    // init l1 stuff
    let tester_wallet = await_tel_res!(
        context,
        tracer,
        "ProposerSignerArgs::wallet",
        args.propose_args
            .proposer_signer
            .wallet(Some(config.l1_chain_id))
    )?;
    let tester_address = tester_wallet.default_signer().address();
    let tester_provider = args
        .propose_args
        .txn_args
        .premium_provider::<Ethereum>()
        .wallet(tester_wallet)
        .connect_http(
            args.propose_args
                .core
                .provider
                .eth_rpc_url
                .as_str()
                .try_into()?,
        );

    let dispute_game_factory = IDisputeGameFactory::new(dgf_address, &tester_provider);
    let kailua_game_implementation = KailuaGame::new(
        dispute_game_factory
            .gameImpls(KAILUA_GAME_TYPE)
            .stall_with_context(context.clone(), "DisputeGameFactory::gameImpls")
            .await,
        &tester_provider,
    );
    let kailua_treasury_address = kailua_game_implementation
        .KAILUA_TREASURY()
        .stall_with_context(context.clone(), "KailuaGame::KAILUA_TREASURY")
        .await;
    let kailua_treasury_instance = KailuaTreasury::new(kailua_treasury_address, &tester_provider);

    // load constants
    let proposal_output_count = kailua_game_implementation
        .PROPOSAL_OUTPUT_COUNT()
        .stall_with_context(context.clone(), "KailuaGame::PROPOSAL_OUTPUT_COUNT")
        .await;
    let output_block_span = kailua_game_implementation
        .OUTPUT_BLOCK_SPAN()
        .stall_with_context(context.clone(), "KailuaGame::OUTPUT_BLOCK_SPAN")
        .await;
    let proposal_block_count = proposal_output_count * output_block_span;

    // get proposal parent
    let games_count = dispute_game_factory
        .gameCount()
        .stall_with_context(context.clone(), "DisputeGameFactory::gameCount")
        .await;
    let parent_game_address = dispute_game_factory
        .gameAtIndex(U256::from(args.fault_parent))
        .stall_with_context(context.clone(), "DisputeGameFactory::gameAtIndex")
        .await
        .proxy_;
    let parent_game_contract = KailuaGame::new(parent_game_address, &tester_provider);
    let parent_block_number: u64 = parent_game_contract
        .l2BlockNumber()
        .stall_with_context(context.clone(), "KailuaTournament::l2BlockNumber")
        .await
        .to();
    // Prepare faulty proposal
    let faulty_block_number = parent_block_number + args.fault_offset * output_block_span;
    let faulty_root_claim = B256::from(games_count.to_be_bytes());
    // Prepare remainder of proposal
    let proposed_block_number = parent_block_number + proposal_block_count;
    let proposed_output_root = if proposed_block_number == faulty_block_number {
        faulty_root_claim
    } else {
        await_tel!(
            context,
            tracer,
            "proposed_output_root",
            retry_res_ctx_timeout!(
                op_node_provider
                    .output_at_block(proposed_block_number)
                    .await
            )
        )
    };

    // Prepare intermediate outputs
    let mut io_field_elements = vec![];
    let is_output_fault = faulty_block_number <= proposed_block_number;
    let normalized_fault_block_number =
        faulty_block_number - (!is_output_fault as u64) * output_block_span;
    for i in 1..FIELD_ELEMENTS_PER_BLOB {
        let io_block_number = parent_block_number + i * output_block_span;

        let output_hash = if io_block_number == normalized_fault_block_number {
            faulty_root_claim
        } else if io_block_number < proposed_block_number {
            await_tel!(
                context,
                tracer,
                "output_hash",
                retry_res_ctx_timeout!(op_node_provider.output_at_block(io_block_number).await)
            )
        } else {
            B256::ZERO
        };
        io_field_elements.push(hash_to_fe(output_hash));
    }
    let sidecar = Proposal::create_sidecar(&io_field_elements)?;

    // Calculate required duplication counter
    let mut dupe_counter = 0u64;
    let extra_data = loop {
        // compute extra data with block number, parent factory index, and blob hash
        let extra_data = [
            proposed_block_number.abi_encode_packed(),
            args.fault_parent.abi_encode_packed(),
            dupe_counter.abi_encode_packed(),
        ]
        .concat();
        // check if proposal exists
        let dupe_game_address = dispute_game_factory
            .games(
                KAILUA_GAME_TYPE,
                proposed_output_root,
                Bytes::from(extra_data.clone()),
            )
            .stall_with_context(context.clone(), "DisputeGameFactory::games")
            .await
            .proxy_;
        if dupe_game_address.is_zero() {
            // proposal was not made before using this dupe counter
            break extra_data;
        }
        // increment counter
        dupe_counter += 1;
    };

    let bond_value = kailua_treasury_instance
        .participationBond()
        .stall_with_context(context.clone(), "KailuaTreasury::participationBond")
        .await;
    let paid_in = kailua_treasury_instance
        .paidBonds(tester_address)
        .stall_with_context(context.clone(), "KailuaTreasury::paidBonds")
        .await;
    let owed_collateral = bond_value.saturating_sub(paid_in);

    let mut transaction =
        kailua_treasury_instance.propose(proposed_output_root, Bytes::from(extra_data));
    if !owed_collateral.is_zero() {
        transaction = transaction.value(owed_collateral);
    }
    if !sidecar.blobs.is_empty() {
        transaction = transaction.sidecar(sidecar);
    }
    match transaction
        .transact_with_context(context.clone(), "KailuaTreasury::propose")
        .await
        .context("KailuaTreasury::propose")
    {
        Ok(receipt) => {
            info!("Faulty proposal submitted at index {games_count}: {receipt:?}")
        }
        Err(e) => {
            error!("Failed to confirm faulty proposal txn: {e:?}");
        }
    }
    Ok(())
}
