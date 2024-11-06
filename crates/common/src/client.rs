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
use kona_client::l1::{DerivationDriver, OracleL1ChainProvider};
use kona_client::l2::OracleL2ChainProvider;
use kona_client::{BootInfo, FlushableCache};
use kona_derive::traits::BlobProvider;
use kona_mpt::{TrieHinter, TrieProvider};
use kona_preimage::CommsClient;
use std::fmt::Debug;
use std::sync::Arc;

pub fn run_client<
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
    T: TrieProvider + TrieHinter + Send + Sync + Debug + Clone + 'static,
>(
    oracle: Arc<O>,
    boot: Arc<BootInfo>,
    beacon: B,
    trie: T,
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
            .advance_to_target(&boot.rollup_config, &trie, &trie, |_| log("EXECUTE"))
            .await?;

        assert_eq!(output_number, boot.claimed_l2_block_number);
        Ok(Some(output_root))
    })
}

pub fn log(msg: &str) {
    #[cfg(target_os = "zkvm")]
    risc0_zkvm::guest::env::log(msg);
    #[cfg(not(target_os = "zkvm"))]
    tracing::info!("{msg}");
}
