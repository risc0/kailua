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

use crate::args::KailuaHostArgs;
use alloy_primitives::B256;
use kailua_common::witness::StitchedBootInfo;
use kona_host::cli::HostMode;
use kona_host::single::{start_native_preimage_server, SingleChainFetcher};
use kona_preimage::{BidirectionalChannel, HintWriter, OracleReader};
use std::sync::Arc;
use tokio::sync::RwLock;
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
pub async fn start_server_and_native_client(
    args: KailuaHostArgs,
    precondition_validation_data_hash: B256,
    stitched_boot_info: Vec<StitchedBootInfo>,
) -> anyhow::Result<i32> {
    // Instantiate data channels
    let hint_chan = BidirectionalChannel::new()?;
    let preimage_chan = BidirectionalChannel::new()?;
    // Create chain client
    let HostMode::Single(kona_cfg) = args.kona.mode;
    let kv_store = kona_cfg.construct_kv_store();
    let fetcher = if !kona_cfg.is_offline() {
        let (l1_provider, blob_provider, l2_provider) = kona_cfg.create_providers().await?;
        Some(Arc::new(RwLock::new(SingleChainFetcher::new(
            kv_store.clone(),
            l1_provider,
            blob_provider,
            l2_provider,
            kona_cfg.agreed_l2_head_hash,
        ))))
    } else {
        None
    };
    // Create the server and start it.
    let server_task = tokio::spawn(start_native_preimage_server(
        kv_store,
        fetcher,
        hint_chan.host,
        preimage_chan.host,
    ));
    // Start the client program in a separate child process.
    let program_task = tokio::spawn(kailua_client::proving::run_proving_client(
        args.boundless,
        OracleReader::new(preimage_chan.client),
        HintWriter::new(hint_chan.client),
        args.payout_recipient_address.unwrap_or_default(),
        precondition_validation_data_hash,
        stitched_boot_info,
    ));
    // Execute both tasks and wait for them to complete.
    info!("Starting preimage server and client program.");
    let (_, client_result) = tokio::try_join!(server_task, program_task,)?;
    info!(target: "kona_host", "Preimage server and client program have joined.");
    // Return execution result
    Ok(client_result.is_err() as i32)
}
