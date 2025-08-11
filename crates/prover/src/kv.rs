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

use alloy_primitives::B256;
use kona_host::single::{SingleChainHost, SingleChainLocalInputs};
use kona_host::{
    DiskKeyValueStore, KeyValueStore, MemoryKeyValueStore, SharedKeyValueStore, SplitKeyValueStore,
};
use kona_preimage::PreimageKeyType;
use std::ops::Deref;
use std::sync::{Arc, RwLock};
use tokio::sync;

#[derive(Debug, Clone)]
pub struct RWLKeyValueStore {
    pub kv: Arc<RwLock<DiskKeyValueStore>>,
    pub global_mask: B256,
}

impl RWLKeyValueStore {
    pub fn new(kv: Arc<RwLock<DiskKeyValueStore>>, global_mask: B256) -> Self {
        Self { kv, global_mask }
    }

    pub fn with_global_mask(self, global_mask: B256) -> Self {
        Self {
            kv: self.kv,
            global_mask,
        }
    }

    /// Applies a mask to global generics that may be relevant only in certain contexts
    pub fn mask(&self, key: B256) -> B256 {
        if let Ok(PreimageKeyType::GlobalGeneric) = PreimageKeyType::try_from(key.0[0]) {
            key ^ self.global_mask
        } else {
            key
        }
    }
}

impl Deref for RWLKeyValueStore {
    type Target = Arc<RwLock<DiskKeyValueStore>>;

    fn deref(&self) -> &Self::Target {
        &self.kv
    }
}

impl From<DiskKeyValueStore> for RWLKeyValueStore {
    fn from(value: DiskKeyValueStore) -> Self {
        Self {
            kv: Arc::new(RwLock::new(value)),
            global_mask: B256::ZERO,
        }
    }
}

impl KeyValueStore for RWLKeyValueStore {
    fn get(&self, key: B256) -> Option<Vec<u8>> {
        self.read().unwrap().get(self.mask(key))
    }

    fn set(&mut self, key: B256, value: Vec<u8>) -> anyhow::Result<()> {
        self.write().unwrap().set(self.mask(key), value)
    }
}

pub fn create_disk_kv_store(kona: &SingleChainHost) -> Option<RWLKeyValueStore> {
    kona.data_dir
        .as_ref()
        .map(|data_dir| RWLKeyValueStore::from(DiskKeyValueStore::new(data_dir.clone())))
}

pub fn create_split_kv_store(
    kona: &SingleChainHost,
    disk_kv_store: Option<RWLKeyValueStore>,
) -> anyhow::Result<SharedKeyValueStore> {
    let local_kv_store = SingleChainLocalInputs::new(kona.clone());

    let kv_store: SharedKeyValueStore = if let Some(disk_kv_store) = disk_kv_store {
        let split_kv_store = SplitKeyValueStore::new(local_kv_store, disk_kv_store);
        Arc::new(sync::RwLock::new(split_kv_store))
    } else {
        let mem_kv_store = MemoryKeyValueStore::new();
        let split_kv_store = SplitKeyValueStore::new(local_kv_store, mem_kv_store);
        Arc::new(sync::RwLock::new(split_kv_store))
    };

    Ok(kv_store)
}
