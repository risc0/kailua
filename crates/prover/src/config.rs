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

use crate::args::ProveArgs;
use kailua_sync::provider::optimism::fetch_rollup_config;
use kona_genesis::RollupConfig;
use tempfile::TempDir;
use tokio::fs;
use tracing::debug;

pub async fn generate_rollup_config_file(
    args: &mut ProveArgs,
    tmp_dir: &TempDir,
) -> anyhow::Result<RollupConfig> {
    // generate a RollupConfig for the target network
    Ok(match args.kona.read_rollup_config().ok() {
        Some(rollup_config) => rollup_config,
        None => {
            let tmp_cfg_file = tmp_dir.path().join("rollup-config.json");
            // read/fetch config
            let rollup_config = fetch_rollup_config(
                args.op_node_address.as_ref().unwrap().as_str(),
                args.kona
                    .l2_node_address
                    .clone()
                    .expect("Missing l2-node-address")
                    .as_str(),
                args.kona.l2_chain_id,
                args.proving.bypass_chain_registry,
            )
            .await?;
            // export
            let ser_config = serde_json::to_string(&rollup_config)?;
            fs::write(&tmp_cfg_file, &ser_config).await?;

            args.kona.rollup_config_path = Some(tmp_cfg_file);
            debug!("{:?}", args.kona.rollup_config_path);
            args.kona.read_rollup_config()?
        }
    })
}
