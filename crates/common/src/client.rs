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

use crate::validate_precondition;
use alloy_consensus::Header;
use alloy_primitives::{Sealed, B256};
use anyhow::bail;
use kona_derive::traits::BlobProvider;
use kona_driver::Driver;
use kona_executor::TrieDBProvider;
use kona_preimage::{CommsClient, PreimageKey, PreimageKeyType};
use kona_proof::errors::OracleProviderError;
use kona_proof::executor::KonaExecutor;
use kona_proof::l1::{OracleL1ChainProvider, OraclePipeline};
use kona_proof::l2::OracleL2ChainProvider;
use kona_proof::sync::new_pipeline_cursor;
use kona_proof::{BootInfo, FlushableCache, HintType};
use std::fmt::Debug;
use std::sync::Arc;

pub fn run_client<
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
    // T: TrieProvider + TrieHinter + Send + Sync + Debug + Clone + 'static,
>(
    precondition_validation_data_hash: B256,
    oracle: Arc<O>,
    boot: Arc<BootInfo>,
    mut beacon: B,
    // execution_provider: T, // todo: skip oracle using provider
) -> anyhow::Result<(B256, Option<B256>)>
where
    <B as BlobProvider>::Error: Debug,
{
    kona_proof::block_on(async move {
        ////////////////////////////////////////////////////////////////
        //                        PRECONDITION                        //
        ////////////////////////////////////////////////////////////////

        log("PRECONDITION");
        let precondition_hash = validate_precondition(
            precondition_validation_data_hash,
            oracle.clone(),
            boot.clone(),
            &mut beacon,
        )
        .await?;

        ////////////////////////////////////////////////////////////////
        //                          PROLOGUE                          //
        ////////////////////////////////////////////////////////////////
        log("PROLOGUE");

        let mut l1_provider = OracleL1ChainProvider::new(boot.clone(), oracle.clone());
        let mut l2_provider = OracleL2ChainProvider::new(boot.clone(), oracle.clone());

        // If the claimed L2 block number is less than the safe head of the L2 chain, the claim is
        // invalid.
        let safe_head = fetch_safe_head(oracle.as_ref(), boot.as_ref(), &mut l2_provider).await?;
        if boot.claimed_l2_block_number < safe_head.number {
            bail!("Invalid Claim");
        }

        // In the case where the agreed upon L2 output root is the same as the claimed L2 output root,
        // trace extension is detected and we can skip the derivation and execution steps.
        if boot.agreed_l2_output_root == boot.claimed_l2_output_root {
            return Ok((precondition_hash, Some(boot.claimed_l2_output_root)));
        }

        ////////////////////////////////////////////////////////////////
        //                   DERIVATION & EXECUTION                   //
        ////////////////////////////////////////////////////////////////
        log("DERIVATION");
        // Create a new derivation driver with the given boot information and oracle.
        let cursor =
            new_pipeline_cursor(&boot, safe_head, &mut l1_provider, &mut l2_provider).await?;
        let cfg = Arc::new(boot.rollup_config.clone());
        let pipeline = OraclePipeline::new(
            cfg.clone(),
            cursor.clone(),
            oracle.clone(),
            beacon,
            l1_provider.clone(),
            l2_provider.clone(),
        );
        let executor = KonaExecutor::new(&cfg, l2_provider.clone(), l2_provider, None, None);
        let mut driver = Driver::new(cursor, executor, pipeline);

        // Run the derivation pipeline until we are able to produce the output root of the claimed
        // L2 block.
        log("ADVANCE");
        let (number, output_root) = driver
            .advance_to_target(&boot.rollup_config, Some(boot.claimed_l2_block_number))
            .await?;

        // None indicates that there is insufficient L1 data available to produce an L2
        // output root at the claimed block number
        log(&format!(
            "OUTPUT: {number}|{}",
            boot.claimed_l2_block_number
        ));

        if number < boot.claimed_l2_block_number {
            Ok((precondition_hash, None))
        } else {
            Ok((precondition_hash, Some(output_root)))
        }
    })
}

/// Fetches the safe head of the L2 chain based on the agreed upon L2 output root in the
/// [BootInfo].
async fn fetch_safe_head<O>(
    caching_oracle: &O,
    boot_info: &BootInfo,
    l2_chain_provider: &mut OracleL2ChainProvider<O>,
) -> Result<Sealed<Header>, OracleProviderError>
where
    O: CommsClient,
{
    caching_oracle
        .write(&HintType::StartingL2Output.encode_with(&[boot_info.agreed_l2_output_root.as_ref()]))
        .await
        .map_err(OracleProviderError::Preimage)?;
    let mut output_preimage = [0u8; 128];
    caching_oracle
        .get_exact(
            PreimageKey::new(*boot_info.agreed_l2_output_root, PreimageKeyType::Keccak256),
            &mut output_preimage,
        )
        .await
        .map_err(OracleProviderError::Preimage)?;

    let safe_hash = output_preimage[96..128]
        .try_into()
        .map_err(OracleProviderError::SliceConversion)?;
    l2_chain_provider
        .header_by_hash(safe_hash)
        .map(|header| Sealed::new_unchecked(header, safe_hash))
}

pub fn log(msg: &str) {
    #[cfg(target_os = "zkvm")]
    risc0_zkvm::guest::env::log(msg);
    #[cfg(not(target_os = "zkvm"))]
    tracing::info!("{msg}");
}
