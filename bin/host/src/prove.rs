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
use crate::config::generate_rollup_config;
use crate::preflight::{fetch_precondition_data, zeth_execution_preflight};
use alloy::network::primitives::BlockTransactionsKind;
use alloy::providers::Provider;
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::B256;
use anyhow::{anyhow, Context};
use kailua_build::KAILUA_FPVM_ID;
use kailua_client::proof::{proof_file_name, read_proof_file};
use kailua_client::proving::ProvingError;
use kailua_common::proof::Proof;
use kailua_common::stitching::stitch_boot_info;
use kailua_common::witness::StitchedBootInfo;
use kona_proof::BootInfo;
use std::env::set_var;
use std::path::Path;
use tempfile::tempdir;
use tracing::info;

/// Computes a receipt if it is not cached
pub async fn compute_fpvm_proof(
    mut args: KailuaHostArgs,
    stitched_boot_info: Vec<StitchedBootInfo>,
    stitched_proofs: Vec<Proof>,
    prove_snark: bool,
    force_attempt: bool,
) -> Result<Proof, ProvingError> {
    // preload precondition data into KV store
    let (precondition_hash, precondition_validation_data_hash) =
        match fetch_precondition_data(&args)
            .await
            .map_err(|e| ProvingError::OtherError(anyhow!(e)))?
        {
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
    // fetch rollup config
    let tmp_dir = tempdir().map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    let rollup_config = generate_rollup_config(&mut args, &tmp_dir)
        .await
        .context("generate_rollup_config")
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    // extract single chain kona config
    let boot = BootInfo {
        l1_head: args.kona.l1_head,
        agreed_l2_output_root: args.kona.agreed_l2_output_root,
        claimed_l2_output_root: args.kona.claimed_l2_output_root,
        claimed_l2_block_number: args.kona.claimed_l2_block_number,
        chain_id: rollup_config.l2_chain_id,
        rollup_config,
    };
    let proof_journal = stitch_boot_info(
        &boot,
        bytemuck::cast::<[u32; 8], [u8; 32]>(KAILUA_FPVM_ID).into(),
        args.payout_recipient_address.unwrap_or_default(),
        precondition_hash,
        stitched_boot_info.clone(),
    );
    // count transactions
    let stitched_count = stitched_boot_info.len();
    if !args.kona.is_offline() {
        let providers = args
            .kona
            .create_providers()
            .await
            .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
        let mut transactions = 0;
        let starting_block = providers
            .l2
            .get_block_by_hash(args.kona.agreed_l2_head_hash, BlockTransactionsKind::Hashes)
            .await
            .map_err(|e| ProvingError::OtherError(anyhow!(e)))?
            .unwrap()
            .header
            .number;
        let block_count = args.kona.claimed_l2_block_number - starting_block;
        for i in 0..block_count {
            transactions += providers
                .l2
                .get_block_transaction_count_by_number(BlockNumberOrTag::Number(
                    starting_block + i + 1,
                ))
                .await
                .map_err(|e| ProvingError::OtherError(anyhow!(e)))?
                .expect("Failed to get transaction count for block {i}");
        }
        info!(
            "Proving {} transactions over {} blocks.",
            transactions, block_count
        );
    }
    // generate proof
    let proof_file_name = proof_file_name(&proof_journal);
    if let Ok(true) = Path::new(&proof_file_name).try_exists() {
        info!("Proving skipped. Proof file {proof_file_name} already exists.");
    } else {
        info!("Computing uncached proof.");
        // run zeth preflight to fetch the necessary preimages
        if !args.skip_zeth_preflight {
            zeth_execution_preflight(&args, boot.rollup_config)
                .await
                .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
        }

        // generate a proof using the kailua client and kona server
        crate::server::start_server_and_native_client(
            args,
            precondition_validation_data_hash,
            stitched_boot_info,
            stitched_proofs,
            prove_snark,
            force_attempt,
        )
        .await?;
    }

    info!("{} sub-proofs composed", stitched_count);

    read_proof_file(&proof_file_name)
        .await
        .context(format!(
            "Failed to read proof file {proof_file_name} contents."
        ))
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))
}
