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

pub mod core;
pub mod stateless;
pub mod stitching;

/// Logs a given message under different logging mechanisms based on the target operating system.
///
/// # Parameters
/// - `msg`: A string slice representing the message to be logged.
///
/// # Platform-specific Behavior
/// - On a `zkvm` target operating system:
///   - Logs the message using the RISC Zero zkVM environment's logging mechanism (`risc0_zkvm::guest::env::log`).
/// - On other target operating systems:
///   - Logs the message using the `tracing` crate's `info!` macro.
pub fn log(msg: &str) {
    #[cfg(target_os = "zkvm")]
    risc0_zkvm::guest::env::log(msg);
    #[cfg(not(target_os = "zkvm"))]
    tracing::info!("{msg}");
}

#[cfg(test)]
pub mod tests {
    use crate::precondition::PreconditionValidationData;
    use alloy_primitives::B256;
    use async_trait::async_trait;
    use copy_dir::copy_dir;
    use kona_host::single::{SingleChainHost, SingleChainLocalInputs};
    use kona_host::{DiskKeyValueStore, KeyValueStore, OfflineHostBackend, SplitKeyValueStore};
    use kona_preimage::errors::PreimageOracleResult;
    use kona_preimage::{
        HintWriterClient, PreimageFetcher, PreimageKey, PreimageKeyType, PreimageOracleClient,
    };
    use kona_proof::{BootInfo, FlushableCache};
    use std::sync::Arc;
    use tempfile::{tempdir, TempDir};
    use tokio::sync::RwLock;
    use tokio::task::block_in_place;

    #[derive(Debug)]
    pub struct TestOracle<T: KeyValueStore + Send + Sync> {
        pub kv: Arc<RwLock<T>>,
        pub backend: OfflineHostBackend<T>,
        pub temp_dir: Option<TempDir>,
    }

    impl Clone for TestOracle<TestKeyValueStore> {
        fn clone(&self) -> Self {
            Self {
                kv: self.kv.clone(),
                backend: OfflineHostBackend::new(self.kv.clone()),
                temp_dir: None,
            }
        }
    }

    pub type TestKeyValueStore = SplitKeyValueStore<SingleChainLocalInputs, DiskKeyValueStore>;

    impl TestOracle<TestKeyValueStore> {
        pub fn new(boot_info: BootInfo) -> Self {
            // Create memory store
            let scli = SingleChainLocalInputs::new(SingleChainHost {
                l1_head: boot_info.l1_head,
                agreed_l2_output_root: boot_info.agreed_l2_output_root,
                claimed_l2_output_root: boot_info.claimed_l2_output_root,
                claimed_l2_block_number: boot_info.claimed_l2_block_number,
                l2_chain_id: Some(boot_info.chain_id),
                // rollup_config_path: None, // no support for custom chains
                ..Default::default()
            });
            // Create a cloned disk store in a temp dir
            let temp_dir = tempdir().unwrap();
            let dest = temp_dir.path().join("testdata");
            copy_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata"), &dest).unwrap();
            let disk = DiskKeyValueStore::new(dest);
            let kv = Arc::new(RwLock::new(SplitKeyValueStore::new(scli, disk)));

            Self {
                kv: kv.clone(),
                backend: OfflineHostBackend::new(kv.clone()),
                temp_dir: Some(temp_dir),
            }
        }

        pub fn add_precondition_data(&self, data: PreconditionValidationData) -> B256 {
            block_in_place(move || {
                let mut kv = self.kv.blocking_write();
                let precondition_data_hash = data.hash();
                let preimage_key =
                    PreimageKey::new(precondition_data_hash.0, PreimageKeyType::Sha256);
                kv.set(B256::from(preimage_key), data.to_vec()).unwrap();
                // sanity check
                assert_eq!(kv.get(B256::from(preimage_key)).unwrap(), data.to_vec());
                precondition_data_hash
            })
        }
    }

    impl<T: KeyValueStore + Send + Sync> FlushableCache for TestOracle<T> {
        fn flush(&self) {
            // noop
        }
    }

    #[async_trait]
    impl<T: KeyValueStore + Send + Sync> PreimageOracleClient for TestOracle<T> {
        async fn get(&self, key: PreimageKey) -> PreimageOracleResult<Vec<u8>> {
            self.backend.get_preimage(key).await
        }

        async fn get_exact(&self, key: PreimageKey, buf: &mut [u8]) -> PreimageOracleResult<()> {
            let value = self.get(key).await?;
            buf.copy_from_slice(value.as_ref());
            Ok(())
        }
    }

    #[async_trait]
    impl<T: KeyValueStore + Send + Sync> HintWriterClient for TestOracle<T> {
        async fn write(&self, _hint: &str) -> PreimageOracleResult<()> {
            // just hit the noop
            self.flush();
            Ok(())
        }
    }
}
