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

use clap::Parser;
use kailua_client::fpvm_proof_file_name;
use kailua_host::{generate_rollup_config, KailuaHostCli};
use kona_host::{init_tracing_subscriber, start_server_and_native_client};
use std::path::Path;
use tempfile::tempdir;
use tracing::info;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let mut cfg = KailuaHostCli::parse();
    init_tracing_subscriber(cfg.kona.v)?;

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
        generate_rollup_config(&mut cfg, &tmp_dir).await?;
        // generate a proof using the kailua client and kona server
        start_server_and_native_client(cfg.kona.clone())
            .await
            .expect("Proving failure");
    }

    info!("Exiting host program.");
    Ok(())
}
