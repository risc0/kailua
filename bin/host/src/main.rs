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

use alloy::network::primitives::BlockTransactionsKind;
use alloy::providers::Provider;
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::B256;
use anyhow::Context;
use clap::Parser;
use kailua_client::proof::fpvm_proof_file_name;
use kailua_host::args::KailuaHostArgs;
use kailua_host::config::generate_rollup_config;
use kailua_host::preflight::{fetch_precondition_data, zeth_execution_preflight};
use kona_host::cli::HostMode;
use kona_host::init_tracing_subscriber;
use std::env::set_var;
use std::path::Path;
use tempfile::tempdir;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = KailuaHostArgs::parse();
    init_tracing_subscriber(args.kona.v)?;
    set_var("KAILUA_VERBOSITY", args.kona.v.to_string());

    // compute receipt if uncached
    let (precondition_hash, precondition_validation_data_hash) =
        match fetch_precondition_data(&args).await? {
            Some(data) => {
                let precondition_validation_data_hash = data.hash();
                set_var(
                    "PRECONDITION_VALIDATION_DATA_HASH",
                    precondition_validation_data_hash.to_string(),
                );
                (data.precondition_hash(), precondition_validation_data_hash)
            }
            None => (B256::ZERO, B256::ZERO),
        };
    let HostMode::Single(kona_cfg) = &args.kona.mode;
    // count transactions
    let (.., l2_provider) = kona_cfg.create_providers().await?;
    let mut transactions = 0;
    let starting_block = l2_provider
        .get_block_by_hash(kona_cfg.agreed_l2_head_hash, BlockTransactionsKind::Hashes)
        .await?
        .unwrap()
        .header
        .number;
    let block_count = kona_cfg.claimed_l2_block_number - starting_block;
    for i in 0..block_count {
        transactions += l2_provider
            .get_block_transaction_count_by_number(BlockNumberOrTag::Number(starting_block + i))
            .await?
            .expect("Failed to get transaction count for block {i}");
    }
    info!(
        "Proving {} transactions over {} blocks.",
        transactions, block_count
    );

    let file_name = fpvm_proof_file_name(
        precondition_hash,
        kona_cfg.l1_head,
        kona_cfg.claimed_l2_output_root,
        kona_cfg.claimed_l2_block_number,
        kona_cfg.agreed_l2_output_root,
    );
    if let Ok(true) = Path::new(&file_name).try_exists() {
        info!("Proving skipped. Proof file {file_name} already exists.");
    } else {
        info!("Computing uncached proof.");
        let tmp_dir = tempdir()?;
        let rollup_config = generate_rollup_config(&mut args, &tmp_dir)
            .await
            .context("generate_rollup_config")?;
        // run zeth preflight to fetch the necessary preimages
        if !args.skip_zeth_preflight {
            zeth_execution_preflight(&args, rollup_config).await?;
        }

        // generate a proof using the kailua client and kona server
        kailua_host::server::start_server_and_native_client(
            args,
            precondition_validation_data_hash,
        )
        .await
        .expect("Proving failure");
    }

    info!(
        "Proved {} transactions over {} blocks.",
        transactions, block_count
    );
    info!("Exiting host program.");
    Ok(())
}
