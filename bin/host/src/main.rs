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

use alloy_primitives::B256;
use anyhow::Context;
use clap::Parser;
use kailua_client::fpvm_proof_file_name;
use kailua_host::{
    fetch_precondition_data, generate_rollup_config, zeth_execution_preflight, KailuaHostCli,
};
use kona_host::init_tracing_subscriber;
use std::env::set_var;
use std::path::Path;
use tempfile::tempdir;
use tracing::info;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let mut cfg = KailuaHostCli::parse();
    init_tracing_subscriber(cfg.kona.v)?;
    set_var("KAILUA_VERBOSITY", cfg.kona.v.to_string());

    // compute receipt if uncached
    let (precondition_hash, precondition_validation_data_hash) =
        match fetch_precondition_data(&cfg).await? {
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
    let file_name = fpvm_proof_file_name(
        precondition_hash,
        cfg.kona.l1_head,
        cfg.kona.claimed_l2_output_root,
        cfg.kona.claimed_l2_block_number,
        cfg.kona.agreed_l2_output_root,
    );
    if let Ok(true) = Path::new(&file_name).try_exists() {
        info!("Proving skipped. Receipt file {file_name} already exists.");
    } else {
        info!("Computing uncached receipt.");
        let tmp_dir = tempdir()?;
        let rollup_config = generate_rollup_config(&mut cfg, &tmp_dir)
            .await
            .context("generate_rollup_config")?;
        // run zeth preflight to fetch the necessary preimages
        if !cfg.skip_zeth_preflight {
            zeth_execution_preflight(&cfg, rollup_config).await?;
        }

        // generate a proof using the kailua client and kona server
        kailua_host::start_server_and_native_client(cfg.kona, precondition_validation_data_hash)
            .await
            .expect("Proving failure");
    }

    info!("Exiting host program.");
    Ok(())
}
