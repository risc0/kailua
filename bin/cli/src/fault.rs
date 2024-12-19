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

use crate::db::proposal::Proposal;
use crate::propose::ProposeArgs;
use crate::providers::optimism::OpNodeProvider;
use crate::stall::Stall;
use crate::KAILUA_GAME_TYPE;
use alloy::network::EthereumWallet;
use alloy::primitives::{Bytes, B256, U256};
use alloy::providers::ProviderBuilder;
use alloy::signers::local::LocalSigner;
use alloy::sol_types::SolValue;
use anyhow::Context;
use kailua_common::blobs::hash_to_fe;
use kailua_common::client::config_hash;
use kailua_contracts::*;
use kailua_host::fetch_rollup_config;
use std::str::FromStr;
use tracing::{error, info};

#[derive(clap::Args, Debug, Clone)]
pub struct FaultArgs {
    #[clap(flatten)]
    pub propose_args: ProposeArgs,

    /// Offset of the faulty block within the proposal
    #[clap(long)]
    pub fault_offset: u64,

    /// Index of the parent of the faulty proposal
    #[clap(long)]
    pub fault_parent: u64,
}

pub async fn fault(args: FaultArgs) -> anyhow::Result<()> {
    let op_node_provider = OpNodeProvider(
        ProviderBuilder::new().on_http(args.propose_args.core.op_node_url.as_str().try_into()?),
    );
    let eth_rpc_provider =
        ProviderBuilder::new().on_http(args.propose_args.core.eth_rpc_url.as_str().try_into()?);

    info!("Fetching rollup configuration from rpc endpoints.");
    // fetch rollup config
    let config = fetch_rollup_config(
        &args.propose_args.core.op_node_url,
        &args.propose_args.core.op_geth_url,
        None,
    )
    .await
    .context("fetch_rollup_config")?;
    let rollup_config_hash = config_hash(&config).expect("Configuration hash derivation error");
    info!("RollupConfigHash({})", hex::encode(rollup_config_hash));

    // load system config
    let system_config = SystemConfig::new(config.l1_system_config_address, &eth_rpc_provider);
    let dgf_address = system_config.disputeGameFactory().stall().await.addr_;

    // init l1 stuff
    let tester_signer = LocalSigner::from_str(&args.propose_args.proposer_key)?;
    let tester_address = tester_signer.address();
    let tester_wallet = EthereumWallet::from(tester_signer);
    let tester_provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(tester_wallet)
        .on_http(args.propose_args.core.eth_rpc_url.as_str().try_into()?);

    let dispute_game_factory = IDisputeGameFactory::new(dgf_address, &tester_provider);
    let kailua_game_implementation = kailua_contracts::KailuaGame::new(
        dispute_game_factory
            .gameImpls(KAILUA_GAME_TYPE)
            .stall()
            .await
            .impl_,
        &tester_provider,
    );
    let kailua_treasury_address = kailua_game_implementation
        .treasury()
        .stall()
        .await
        .treasury_;
    let kailua_treasury_instance = KailuaTreasury::new(kailua_treasury_address, &tester_provider);

    // load constants
    let proposal_block_count: u64 = kailua_game_implementation
        .proposalBlockCount()
        .stall()
        .await
        .proposalBlockCount_
        .to();

    // get proposal parent
    let games_count = dispute_game_factory.gameCount().stall().await.gameCount_;
    let parent_game_address = dispute_game_factory
        .gameAtIndex(U256::from(args.fault_parent))
        .stall()
        .await
        .proxy_;
    let parent_game_contract = KailuaGame::new(parent_game_address, &tester_provider);
    let parent_block_number: u64 = parent_game_contract
        .l2BlockNumber()
        .stall()
        .await
        .l2BlockNumber_
        .to();
    // Prepare faulty proposal
    let faulty_block_number = parent_block_number + args.fault_offset;
    let faulty_root_claim = B256::from(games_count.to_be_bytes());
    // Prepare remainder of proposal
    let proposed_block_number = parent_block_number + proposal_block_count;
    let proposed_output_root = if proposed_block_number == faulty_block_number {
        faulty_root_claim
    } else {
        op_node_provider
            .output_at_block(proposed_block_number)
            .await?
    };

    // Prepare intermediate outputs
    let mut io_field_elements = vec![];
    let first_io_number = parent_block_number + 1;
    for i in first_io_number..proposed_block_number {
        let output = if i == faulty_block_number {
            faulty_root_claim
        } else {
            op_node_provider.output_at_block(i).await?
        };
        io_field_elements.push(hash_to_fe(output));
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
            .stall()
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
        .stall()
        .await
        ._0;
    let paid_in = kailua_treasury_instance
        .paidBonds(tester_address)
        .stall()
        .await
        ._0;
    let owed_collateral = bond_value.saturating_sub(paid_in);

    match kailua_treasury_instance
        .propose(proposed_output_root, Bytes::from(extra_data))
        .value(owed_collateral)
        .sidecar(sidecar)
        .send()
        .await
        .context("propose (send)")
    {
        Ok(txn) => match txn.get_receipt().await.context("propose (get_receipt)") {
            Ok(receipt) => {
                info!("Faulty proposal submitted at index {games_count}: {receipt:?}")
            }
            Err(e) => {
                error!("Failed to confirm faulty proposal txn: {e:?}");
            }
        },
        Err(e) => {
            error!("Failed to send faulty proposal txn: {e:?}");
        }
    }
    Ok(())
}
