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

use alloy_chains::NamedChain;
use anyhow::Context;
use clap::Parser;
use kailua_client::fpvm_proof_file_name;
use kailua_host::{dump_data_to_kv_store, generate_rollup_config, KailuaHostCli};
use kona_host::{init_tracing_subscriber, start_server_and_native_client};
use std::env::set_var;
use std::path::Path;
use tempfile::tempdir;
use tracing::info;
use zeth_core::driver::CoreDriver;
use zeth_core::stateless::data::StatelessClientData;
use zeth_core_optimism::OpRethCoreDriver;
use zeth_preflight::client::PreflightClient;
use zeth_preflight_optimism::OpRethPreflightClient;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let mut cfg = KailuaHostCli::parse();
    init_tracing_subscriber(cfg.kona.v)?;
    set_var("KAILUA_VERBOSITY", cfg.kona.v.to_string());

    // compute receipt if uncached
    let file_name = fpvm_proof_file_name(
        cfg.kona.l1_head,
        cfg.kona.claimed_l2_output_root,
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
        if let Ok(named_chain) = NamedChain::try_from(rollup_config.l2_chain_id) {
            // Limitation: Only works when disk caching is enabled under a known "NamedChain"
            if !cfg.kona.is_offline()
                && cfg.kona.data_dir.is_some()
                && OpRethCoreDriver::chain_spec(&named_chain).is_some()
            {
                let kona_cfg = cfg.kona.clone();
                // Fetch all the initial data
                let preflight_data: StatelessClientData<
                    <OpRethCoreDriver as CoreDriver>::Block,
                    <OpRethCoreDriver as CoreDriver>::Header,
                > = tokio::task::spawn_blocking(move || {
                    // Prepare the cache directory
                    let cache_dir = kona_cfg.data_dir.map(|dir| dir.join("optimism"));
                    if let Some(dir) = cache_dir.as_ref() {
                        std::fs::create_dir_all(&dir).expect("Could not create directory");
                    };
                    OpRethPreflightClient::preflight(
                        Some(rollup_config.l2_chain_id),
                        cache_dir,
                        kona_cfg.l2_node_address,
                        kona_cfg.claimed_l2_block_number,
                        1,
                    )
                })
                .await??;
                // Write data to the cached Kona kv-store
                let mut kv_store = cfg.kona.construct_kv_store();
                dump_data_to_kv_store(&mut kv_store, &preflight_data).await;
            }
        }
        // generate a proof using the kailua client and kona server
        start_server_and_native_client(cfg.kona.clone())
            .await
            .expect("Proving failure");
    }

    info!("Exiting host program.");
    Ok(())
}
