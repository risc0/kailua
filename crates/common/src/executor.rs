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

use alloy_consensus::Header;
use alloy_primitives::{Sealed, B256};
use async_trait::async_trait;
use kona_driver::Executor;
use kona_executor::ExecutionArtifacts;
use op_alloy_rpc_types_engine::OpPayloadAttributes;
use std::fmt::Debug;
use std::sync::Arc;

// #[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Execution {
    pub agreed_output: B256,
    pub attributes: OpPayloadAttributes,
    pub artifacts: ExecutionArtifacts,
    pub claimed_output: B256,
}

// #[derive(Debug)]
pub struct PrecomputedExecutor<E: Executor + Send + Sync + Debug> {
    pub executions: Vec<Arc<Execution>>,
    pub executor: E,
}

#[async_trait]
impl<E: Executor + Send + Sync + Debug> Executor for PrecomputedExecutor<E> {
    type Error = <E as Executor>::Error;

    async fn wait_until_ready(&mut self) {
        self.executor.wait_until_ready().await;
    }

    fn update_safe_head(&mut self, header: Sealed<Header>) {
        self.executor.update_safe_head(header);
    }

    async fn execute_payload(
        &mut self,
        attributes: OpPayloadAttributes,
    ) -> Result<ExecutionArtifacts, Self::Error> {
        let agreed_output = self.compute_output_root()?;
        if self
            .executions
            .last()
            .map(|e| Ok(agreed_output == e.agreed_output && attributes == e.attributes))
            .unwrap_or(Ok(false))?
        {
            let artifacts = self.executions.pop().unwrap().artifacts.clone();
            self.update_safe_head(artifacts.block_header.clone());
            return Ok(artifacts);
        }
        self.executor.execute_payload(attributes).await
    }

    fn compute_output_root(&mut self) -> Result<B256, Self::Error> {
        self.executor.compute_output_root()
    }
}
