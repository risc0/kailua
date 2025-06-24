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

use crate::args::KailuaHostArgs;
use crate::kv::RWLKeyValueStore;
use crate::prove;
use alloy_primitives::B256;
use anyhow::Context;
use async_channel::{Receiver, Sender};
use kailua_common::boot::StitchedBootInfo;
use kailua_common::executor::Execution;
use kailua_prover::ProvingError;
use kona_genesis::RollupConfig;
use risc0_zkvm::Receipt;
use std::cmp::Ordering;
use tracing::error;

#[derive(Clone, Debug)]
pub struct Cached {
    pub args: KailuaHostArgs,
    pub rollup_config: RollupConfig,
    pub disk_kv_store: Option<RWLKeyValueStore>,
    pub precondition_hash: B256,
    pub precondition_validation_data_hash: B256,
    pub stitched_executions: Vec<Vec<Execution>>,
    pub stitched_boot_info: Vec<StitchedBootInfo>,
    pub stitched_proofs: Vec<Receipt>,
    pub prove_snark: bool,
    pub force_attempt: bool,
    pub seek_proof: bool,
}

impl Cached {
    pub async fn compute_cached(self) -> Result<Receipt, ProvingError> {
        prove::compute_cached_proof(
            self.args,
            self.rollup_config,
            self.disk_kv_store,
            self.precondition_hash,
            self.precondition_validation_data_hash,
            self.stitched_executions,
            self.stitched_boot_info,
            self.stitched_proofs,
            self.prove_snark,
            self.force_attempt,
            self.seek_proof,
        )
        .await
    }

    pub async fn compute_fpvm(
        self,
        task_sender: Sender<Oneshot>,
    ) -> Result<Option<Receipt>, ProvingError> {
        prove::compute_fpvm_proof(
            self.args,
            self.rollup_config,
            self.disk_kv_store,
            self.precondition_hash,
            self.precondition_validation_data_hash,
            self.stitched_boot_info,
            self.stitched_proofs,
            self.prove_snark,
            task_sender,
        )
        .await
    }
}

impl PartialEq for Cached {
    fn eq(&self, other: &Self) -> bool {
        self.args.eq(&other.args)
    }
}

impl Eq for Cached {}

impl PartialOrd for Cached {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Cached {
    fn cmp(&self, other: &Self) -> Ordering {
        self.args.cmp(&other.args)
    }
}

#[derive(Debug)]
pub struct OneshotResult {
    pub cached: Cached,
    pub result: Result<Receipt, ProvingError>,
}

impl PartialEq for OneshotResult {
    fn eq(&self, other: &Self) -> bool {
        self.cached.eq(&other.cached)
    }
}

impl Eq for OneshotResult {}

impl PartialOrd for OneshotResult {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OneshotResult {
    fn cmp(&self, other: &Self) -> Ordering {
        self.cached.cmp(&other.cached)
    }
}

#[derive(Debug)]
pub struct Oneshot {
    pub cached_task: Cached,
    pub result_sender: Sender<OneshotResult>,
}

pub async fn handle_oneshot_tasks(task_receiver: Receiver<Oneshot>) -> anyhow::Result<()> {
    loop {
        let Oneshot {
            cached_task,
            result_sender,
        } = task_receiver
            .recv()
            .await
            .context("task receiver channel closed")?;

        if let Err(res) = result_sender
            .send(OneshotResult {
                cached: cached_task.clone(),
                result: cached_task.compute_cached().await,
            })
            .await
        {
            error!("failed to send task result: {res:?}");
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn compute_oneshot_task(
    args: KailuaHostArgs,
    rollup_config: RollupConfig,
    disk_kv_store: Option<RWLKeyValueStore>,
    precondition_hash: B256,
    precondition_validation_data_hash: B256,
    stitched_executions: Vec<Vec<Execution>>,
    stitched_boot_info: Vec<StitchedBootInfo>,
    stitched_proofs: Vec<Receipt>,
    prove_snark: bool,
    force_attempt: bool,
    seek_proof: bool,
    task_sender: Sender<Oneshot>,
) -> Result<Receipt, ProvingError> {
    // create proving task
    let cached_task = Cached {
        args,
        rollup_config,
        disk_kv_store,
        precondition_hash,
        precondition_validation_data_hash,
        stitched_executions,
        stitched_boot_info,
        stitched_proofs,
        prove_snark,
        force_attempt,
        seek_proof,
    };
    // create onshot channel
    let oneshot_channel = async_channel::bounded(1);
    // dispatch task to pool
    task_sender
        .send(Oneshot {
            cached_task,
            result_sender: oneshot_channel.0,
        })
        .await
        .expect("Oneshot channel closed");
    // wait for result
    oneshot_channel
        .1
        .recv()
        .await
        .expect("oneshot_channel should never panic")
        .result
}
