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
use kona_client::executor::KonaExecutorConstructor;
use kona_client::l1::{OracleL1ChainProvider, OraclePipeline};
use kona_client::l2::OracleL2ChainProvider;
use kona_client::sync::new_pipeline_cursor;
use kona_client::{BootInfo, FlushableCache};
use kona_derive::traits::BlobProvider;
use kona_driver::Driver;
use kona_preimage::CommsClient;
use std::fmt::Debug;
use std::sync::Arc;

pub fn run_client<
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
    // T: TrieProvider + TrieHinter + Send + Sync + Debug + Clone + 'static,
>(
    oracle: Arc<O>,
    boot: Arc<BootInfo>,
    beacon: B,
    // execution_provider: T, // todo: skip oracle using provider
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
        let cursor = new_pipeline_cursor(
            oracle.clone(),
            &boot,
            &mut l1_provider.clone(),
            &mut l2_provider.clone(),
        )
        .await?;
        let safe_head = cursor.l2_safe_head_header().seal();

        let cfg = Arc::new(boot.rollup_config.clone());
        let pipeline = OraclePipeline::new(
            cfg.clone(),
            cursor.clone(),
            oracle.clone(),
            beacon,
            l1_provider.clone(),
            l2_provider.clone(),
        );

        let executor = KonaExecutorConstructor::new(&cfg, l2_provider.clone(), l2_provider, |_| {
            log("EXECUTE")
        });
        let mut driver = Driver::new(cursor, executor, pipeline);

        log(&format!(
            "PAYLOAD: safe_head({}|{})",
            safe_head, boot.agreed_l2_output_root
        ));

        log("ADVANCE");
        let (output_number, output_root) = driver
            .advance_to_target(&boot.rollup_config, boot.claimed_l2_block_number)
            .await?;

        // None indicates that there is insufficient L1 data available to produce an L2
        // output root at the claimed block number
        log(&format!(
            "OUTPUT: {output_number}|{}",
            boot.claimed_l2_block_number
        ));
        if output_number < boot.claimed_l2_block_number {
            Ok(None)
        } else {
            Ok(Some(output_root))
        }
    })
}

pub fn log(msg: &str) {
    #[cfg(target_os = "zkvm")]
    risc0_zkvm::guest::env::log(msg);
    #[cfg(not(target_os = "zkvm"))]
    tracing::info!("{msg}");
}
