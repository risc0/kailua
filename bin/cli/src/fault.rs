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
use crate::{hash_to_fe, output_at_block, KAILUA_GAME_TYPE};
use alloy::consensus::Blob;
use alloy::network::EthereumWallet;
use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::providers::ProviderBuilder;
use alloy::signers::local::LocalSigner;
use alloy::sol_types::SolValue;
use anyhow::Context;
use kailua_contracts::KailuaGame::KailuaGameInstance;
use kailua_contracts::{IAnchorStateRegistry, IDisputeGameFactory};
use std::str::FromStr;
use tracing::info;

#[derive(clap::Args, Debug, Clone)]
pub struct FaultArgs {
    #[clap(flatten)]
    pub propose_args: ProposeArgs,

    /// Number of blocks in the faulty proposal
    #[clap(long)]
    pub fault_block_count: u64,
}

pub async fn fault(args: FaultArgs) -> anyhow::Result<()> {
    let op_node_provider =
        ProviderBuilder::new().on_http(args.propose_args.op_node_address.as_str().try_into()?);
    // init l1 stuff
    let tester_signer = LocalSigner::from_str(&args.propose_args.proposer_key)?;
    let tester_wallet = EthereumWallet::from(tester_signer);
    let tester_provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(tester_wallet)
        .on_http(args.propose_args.l1_node_address.as_str().try_into()?);
    let anchor_state_registry = IAnchorStateRegistry::new(
        Address::from_str(&args.propose_args.registry_contract)?,
        &tester_provider,
    );
    let dispute_game_factory = IDisputeGameFactory::new(
        anchor_state_registry.disputeGameFactory().call().await?._0,
        &tester_provider,
    );
    let kailua_game_implementation = kailua_contracts::KailuaGame::new(
        dispute_game_factory
            .gameImpls(KAILUA_GAME_TYPE)
            .call()
            .await?
            .impl_,
        &tester_provider,
    );
    // load constants
    let max_proposal_span: u64 = kailua_game_implementation
        .proposalBlockCount()
        .call()
        .await?
        .proposalBlockCount_
        .to();
    let bond_value = dispute_game_factory
        .initBonds(KAILUA_GAME_TYPE)
        .call()
        .await?
        .bond_;
    // get proposal parent
    let games_count = dispute_game_factory.gameCount().call().await?.gameCount_;
    let first_game_data = dispute_game_factory
        .findLatestGames(
            KAILUA_GAME_TYPE,
            games_count - U256::from(1),
            games_count,
        )
        .call()
        .await?
        .games_
        .pop()
        .expect("No fault proof game proposals. Is it installed?");
    let first_game_address = dispute_game_factory
        .gameAtIndex(first_game_data.index)
        .call()
        .await?
        .proxy_;
    let first_game_contract = KailuaGameInstance::new(first_game_address, &tester_provider);
    let anchor_block_number: u64 = first_game_contract
        .l2BlockNumber()
        .call()
        .await?
        .l2BlockNumber_
        .to();
    let first_game_index: u64 = first_game_data.index.to();
    // Prepare faulty proposal
    let faulty_block_number = anchor_block_number + args.fault_block_count;
    let faulty_root_claim = B256::from(
        dispute_game_factory
            .gameCount()
            .call()
            .await?
            .gameCount_
            .to_be_bytes(),
    );
    // Prepare remainder of proposal
    let proposed_block_number = anchor_block_number + max_proposal_span;
    let proposed_output_root = if proposed_block_number == faulty_block_number {
        faulty_root_claim
    } else {
        output_at_block(&op_node_provider, proposed_block_number).await?
    };
    // Prepare intermediate outputs
    let mut intermediate_outputs = vec![];
    let first_io_number = anchor_block_number + 1;
    for i in first_io_number..proposed_block_number {
        let output = if i == faulty_block_number {
            faulty_root_claim
        } else {
            output_at_block(&op_node_provider, i).await?
        };
        intermediate_outputs.push(hash_to_fe(output));
    }
    let io_bytes = intermediate_outputs.concat();
    // Encode as blob sidecar
    let blob = Blob::right_padding_from(io_bytes.as_slice());
    let sidecar = crate::blob_sidecar(blob)?;

    let extra_data = [
        proposed_block_number.abi_encode_packed(),
        first_game_index.abi_encode_packed(),
        sidecar
            .versioned_hash_for_blob(0)
            .unwrap()
            .abi_encode_packed(),
    ]
    .concat();
    dispute_game_factory
        .create(
            KAILUA_GAME_TYPE,
            proposed_output_root,
            Bytes::from(extra_data),
        )
        .value(bond_value)
        .sidecar(sidecar)
        .send()
        .await
        .context("create KailuaGame (send)")?
        .get_receipt()
        .await
        .context("create KailuaGame (get_receipt)")?;
    info!(
        "Submitted faulty proposal at index {games_count} with parent at index {first_game_index}."
    );

    Ok(())
}
