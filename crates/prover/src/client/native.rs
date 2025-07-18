// Copyright 2024, 2025 RISC Zero, Inc.
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
use crate::kv::{create_disk_kv_store, create_split_kv_store, RWLKeyValueStore};
use crate::ProvingError;
use alloy_primitives::B256;
use anyhow::anyhow;
use kailua_common::boot::StitchedBootInfo;
use kailua_common::executor::Execution;
use kona_host::single::{SingleChainHintHandler, SingleChainHost};
use kona_host::{
    OfflineHostBackend, OnlineHostBackend, PreimageServer, PreimageServerError, SharedKeyValueStore,
};
use kona_preimage::{
    BidirectionalChannel, Channel, HintReader, HintWriter, OracleReader, OracleServer,
};
use kona_proof::HintType;
use risc0_zkvm::Receipt;
use std::sync::Arc;
use tokio::task;
use tokio::task::JoinHandle;
use tracing::info;

/// Starts the [PreimageServer] and the client program in separate threads. The client program is
/// ran natively in this mode.
///
/// ## Takes
/// - `cfg`: The host configuration.
///
/// ## Returns
/// - `Ok(exit_code)` if the client program exits successfully.
/// - `Err(_)` if the client program failed to execute, was killed by a signal, or the host program
///   exited first.
#[allow(clippy::too_many_arguments)]
pub async fn run_native_client(
    args: ProveArgs,
    disk_kv_store: Option<RWLKeyValueStore>,
    precondition_validation_data_hash: B256,
    stitched_executions: Vec<Vec<Execution>>,
    stitched_boot_info: Vec<StitchedBootInfo>,
    stitched_proofs: Vec<Receipt>,
    prove_snark: bool,
    force_attempt: bool,
    seek_proof: bool,
) -> Result<(), ProvingError> {
    // Instantiate data channels
    let hint = BidirectionalChannel::new().map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    let preimage = BidirectionalChannel::new().map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    // Create the server and start it.
    let disk_kv_store = match disk_kv_store {
        None => create_disk_kv_store(&args.kona),
        v => v,
    };
    let kv_store = create_split_kv_store(&args.kona, disk_kv_store)
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    let server_task = start_server(&args.kona, kv_store, hint.host, preimage.host)
        .await
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    // Start the client program in a separate thread
    let client_task = tokio::spawn(crate::client::proving::run_proving_client(
        args.proving,
        args.boundless,
        OracleReader::new(preimage.client),
        HintWriter::new(hint.client),
        precondition_validation_data_hash,
        stitched_executions,
        stitched_boot_info,
        stitched_proofs,
        prove_snark,
        force_attempt,
        seek_proof,
    ));
    // Wait for both tasks to complete.
    info!("Starting preimage server and client program.");
    let (_, client_result) = tokio::try_join!(server_task, client_task,)
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    info!(target: "kona_host", "Preimage server and client program have joined.");
    // Return execution result
    client_result
}

pub async fn start_server<C>(
    kona: &SingleChainHost,
    kv_store: SharedKeyValueStore,
    hint: C,
    preimage: C,
) -> anyhow::Result<JoinHandle<Result<(), PreimageServerError>>>
where
    C: Channel + Send + Sync + 'static,
{
    let task_handle = if kona.is_offline() {
        task::spawn(
            PreimageServer::new(
                OracleServer::new(preimage),
                HintReader::new(hint),
                Arc::new(OfflineHostBackend::new(kv_store)),
            )
            .start(),
        )
    } else {
        let providers = kona.create_providers().await?;
        let backend = OnlineHostBackend::new(
            kona.clone(),
            kv_store.clone(),
            providers,
            SingleChainHintHandler,
        )
        .with_proactive_hint(HintType::L2PayloadWitness);

        task::spawn(
            PreimageServer::new(
                OracleServer::new(preimage),
                HintReader::new(hint),
                Arc::new(backend),
            )
            .start(),
        )
    };

    Ok(task_handle)
}
