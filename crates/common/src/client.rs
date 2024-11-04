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

use alloy_primitives::B256;
use anyhow::bail;
use kona_client::l1::{DerivationDriver, OracleL1ChainProvider};
use kona_client::l2::OracleL2ChainProvider;
use kona_client::{BootInfo, FlushableCache};
use kona_derive::traits::BlobProvider;
use kona_preimage::CommsClient;
use std::fmt::Debug;
use std::sync::Arc;

pub fn run_client<
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
>(
    oracle: Arc<O>,
    boot: Arc<BootInfo>,
    beacon: B,
) -> anyhow::Result<Option<B256>> {
    kona_common::block_on(async move {
        ////////////////////////////////////////////////////////////////
        //                          PROLOGUE                          //
        ////////////////////////////////////////////////////////////////
        log("PROLOGUE");

        let l1_provider = OracleL1ChainProvider::new(boot.clone(), oracle.clone());
        let l2_provider = OracleL2ChainProvider::new(boot.clone(), oracle.clone());

        ////////////////////////////////////////////////////////////////
        //                   DERIVATION & EXECUTION                   //
        ////////////////////////////////////////////////////////////////
        log("DERIVATION");
        let mut driver = DerivationDriver::new(
            boot.as_ref(),
            &oracle,
            beacon,
            l1_provider,
            l2_provider.clone(),
        )
        .await?;

        log(&format!(
            "PAYLOAD: safe_head({}|{})",
            driver.l2_safe_head_header().seal(),
            boot.agreed_l2_output_root
        ));

        log("STEP");
        let (output_number, output_root) = driver
            .produce_output(&boot.rollup_config, &l2_provider, &l2_provider, |_| {})
            .await?;

        if output_number != boot.claimed_l2_block_number {
            bail!("Only single-block proofs are currently supported.");
        }

        return Ok(Some(output_root));

        // vestigial code below for when kona can handle multi-block proofs

        // loop {
        //
        //     if header.number == boot.l2_claim_block {
        //         log("OUTPUT");
        //         // return the output at the claim block number
        //         break Ok(Some(output_root));
        //     } else {
        //         log("STEP");
        //         // Derive block info
        //         let body = payload
        //             .attributes
        //             .transactions
        //             .iter()
        //             .map(|raw_tx| {
        //                 op_alloy_consensus::OpTxEnvelope::decode_2718(&mut raw_tx.as_ref()).unwrap()
        //             })
        //             .collect();
        //         let l2_payload_envelope = L2ExecutionPayloadEnvelope::from(OpBlock {
        //             header: header.clone(),
        //             body,
        //             withdrawals: boot
        //                 .rollup_config
        //                 .is_canyon_active(header.timestamp)
        //                 .then(Vec::new),
        //             ..Default::default()
        //         });
        //         // Update safe head
        //         driver.l2_safe_head = l2_payload_envelope.to_l2_block_ref(&boot.rollup_config)?;
        //         driver.l2_safe_head_header = header.seal_slow();
        //         let l2_head = driver.l2_safe_head_header.seal();
        //         oracle
        //             .write(&HintType::SafeL2Head.encode_with(&[l2_head.as_ref()]))
        //             .await?;
        //
        //         // Update l2_output_root in l2 provider boot info
        //         let new_boot_info = Arc::new(BootInfo {
        //             l2_output_root: output_root,
        //             ..boot.as_ref().clone()
        //         });
        //         l2_provider.boot_info = new_boot_info.clone();
        //         driver.pipeline.l2_chain_provider.boot_info = new_boot_info.clone();
        //         driver.pipeline.attributes.builder.config_fetcher.boot_info = new_boot_info.clone();
        //         // boot = Arc::new(BootInfo {
        //         //     l2_output_root: output_root,
        //         //     ..boot.as_ref().clone()
        //         // });
        //     }
        // }
    })
}

pub fn log(msg: &str) {
    #[cfg(target_os = "zkvm")]
    risc0_zkvm::guest::env::log(msg);
    #[cfg(not(target_os = "zkvm"))]
    tracing::info!("{msg}");
}
