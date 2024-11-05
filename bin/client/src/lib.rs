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

pub mod oracle;
pub mod oracle_posix;

use crate::oracle::{HINT_WRITER, ORACLE_READER};
use crate::oracle_posix::{POSIXBlobProvider, POSIXHintWriterClient};
use alloy_primitives::{keccak256, B256};
use anyhow::Context;
use kailua_build::{KAILUA_FPVM_ELF, KAILUA_FPVM_ID};
use kona_client::l1::OracleBlobProvider;
use kona_client::{BootInfo, CachingOracle};
use oracle_posix::{POSIXCallbackHandle, POSIXPreimageOracleClient};
use risc0_zkvm::{default_prover, ExecutorEnv, ProveInfo, ProverOpts};
use std::collections::VecDeque;
use std::io::BufReader;
use std::sync::Arc;
use tokio::join;
use tokio::task::spawn_blocking;
use tracing::info;

/// The size of the LRU cache in the oracle.
pub const ORACLE_LRU_SIZE: usize = 1024;

pub async fn run_native_client() -> anyhow::Result<Option<B256>> {
    info!("Preamble");
    let oracle = Arc::new(CachingOracle::new(
        ORACLE_LRU_SIZE,
        ORACLE_READER,
        HINT_WRITER,
    ));

    let boot = Arc::new(
        BootInfo::load(oracle.as_ref())
            .await
            .context("BootInfo::load")?,
    );
    let beacon = OracleBlobProvider::new(oracle.clone());
    kailua_common::client::run_client(oracle, boot, beacon)
}

pub async fn prove_zkvm_client() -> anyhow::Result<ProveInfo> {
    let client_task = spawn_blocking(|| {
        // Kona preimage oracle
        let oracle = Arc::new(CachingOracle::new(
            ORACLE_LRU_SIZE,
            ORACLE_READER,
            HINT_WRITER,
        ));
        // Kailua preimage oracle reader
        let oracle_posix = POSIXCallbackHandle::from(POSIXPreimageOracleClient {
            oracle: oracle.clone(),
            key: VecDeque::new(),
            preimage: VecDeque::new(),
        });
        let oracle_posix_reader = BufReader::new(oracle_posix.clone());
        // Kailua hint writer
        let writer_posix = POSIXHintWriterClient {
            writer: oracle.clone(),
        };
        // Kona blob provider
        let provider = OracleBlobProvider::new(oracle.clone());
        // Kailua blob posix
        let provider_posix = POSIXCallbackHandle::from(POSIXBlobProvider {
            provider,
            request: Default::default(),
            blob: Default::default(),
        });
        let provider_posix_reader = BufReader::new(provider_posix.clone());
        // Execution environment
        let env = ExecutorEnv::builder()
            .env_var("RUST_BACKTRACE", "full")
            // Handle preimage reads via posix
            .read_fd(100, oracle_posix_reader)
            .write_fd(101, oracle_posix)
            // Handle hint writes via posix
            .write_fd(102, writer_posix)
            // Handle blob reads via posix
            .read_fd(104, provider_posix_reader)
            .write_fd(105, provider_posix)
            .build()?;
        let prover = default_prover();
        let prove_info = prover.prove_with_opts(env, KAILUA_FPVM_ELF, &ProverOpts::groth16())?;
        println!(
            "Proof of {} total cycles ({} user cycles) computed.",
            prove_info.stats.total_cycles, prove_info.stats.user_cycles
        );
        prove_info
            .receipt
            .verify(KAILUA_FPVM_ID)
            .context("receipt verification")?;
        println!("Receipt verified.");
        Ok::<_, anyhow::Error>(prove_info)
    });
    join!(client_task).0?
}

pub fn fpvm_proof_file_name(l1_head: B256, l2_claim: B256, l2_output_root: B256) -> String {
    let version = risc0_zkvm::get_version().unwrap();
    let suffix = if risc0_zkvm::is_dev_mode() {
        "fake"
    } else {
        "zkp"
    };
    let data = [
        bytemuck::cast::<_, [u8; 32]>(KAILUA_FPVM_ID).as_slice(),
        l1_head.as_slice(),
        l2_output_root.as_slice(),
        l2_claim.as_slice(),
    ]
    .concat();
    let file_name = keccak256(data);
    format!("risc0-{}-{file_name}.{suffix}", version.to_string())
}
