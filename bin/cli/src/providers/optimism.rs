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

use alloy::primitives::B256;
use alloy::providers::{Provider, ReqwestProvider};
use anyhow::Context;
use serde_json::Value;
use std::str::FromStr;
use tracing::debug;

pub struct OpNodeProvider(pub ReqwestProvider);

impl OpNodeProvider {
    pub async fn output_at_block(&self, output_block_number: u64) -> anyhow::Result<B256> {
        let output_at_block: serde_json::Value = self
            .0
            .client()
            .request(
                "optimism_outputAtBlock",
                (format!("0x{:x}", output_block_number),),
            )
            .await
            .context(format!("optimism_outputAtBlock {output_block_number}"))?;
        debug!("optimism_outputAtBlock {:?}", &output_at_block);
        Ok(B256::from_str(
            output_at_block["outputRoot"].as_str().unwrap(),
        )?)
    }

    pub async fn sync_status(&self) -> anyhow::Result<Value> {
        Ok(self
            .0
            .client()
            .request_noparams("optimism_syncStatus")
            .await?)
    }

    pub async fn rollup_config(&self) -> anyhow::Result<Value> {
        Ok(self
            .0
            .client()
            .request_noparams("optimism_rollupConfig")
            .await?)
    }
}
