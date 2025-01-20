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

use crate::oracle::validate_preimage;
use crate::witness::WitnessOracle;
use async_trait::async_trait;
use kona_preimage::errors::PreimageOracleResult;
use kona_preimage::{HintWriterClient, PreimageKey, PreimageOracleClient};
use kona_proof::FlushableCache;
use lazy_static::lazy_static;
use rkyv::rancor::Fallible;
use rkyv::with::{ArchiveWith, DeserializeWith, SerializeWith};
use rkyv::{Archive, Archived, Place, Resolver, Serialize};
use std::collections::VecDeque;
use std::ops::DerefMut;
use std::sync::{Arc, Mutex};
use tracing::warn;

pub type VecPreimageStore = Arc<Mutex<Vec<(PreimageKey, Vec<u8>)>>>;

pub struct VecPreimageStoreRkyv;

impl ArchiveWith<VecPreimageStore> for VecPreimageStoreRkyv {
    type Archived = Archived<Vec<(PreimageKey, Vec<u8>)>>;
    type Resolver = Resolver<Vec<(PreimageKey, Vec<u8>)>>;

    fn resolve_with(
        field: &VecPreimageStore,
        resolver: Self::Resolver,
        out: Place<Self::Archived>,
    ) {
        let locked_vec = field.lock().unwrap();
        <Vec<(PreimageKey, Vec<u8>)> as Archive>::resolve(&locked_vec, resolver, out);
    }
}

impl<S> SerializeWith<VecPreimageStore, S> for VecPreimageStoreRkyv
where
    S: Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    <S as Fallible>::Error: rkyv::rancor::Source,
{
    fn serialize_with(
        field: &VecPreimageStore,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        let locked_vec = field.lock().unwrap();
        <Vec<(PreimageKey, Vec<u8>)> as Serialize<S>>::serialize(&locked_vec, serializer)
    }
}

impl<D: Fallible> DeserializeWith<Archived<Vec<(PreimageKey, Vec<u8>)>>, VecPreimageStore, D>
    for VecPreimageStoreRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: rkyv::rancor::Source,
{
    fn deserialize_with(
        field: &Archived<Vec<(PreimageKey, Vec<u8>)>>,
        deserializer: &mut D,
    ) -> Result<VecPreimageStore, D::Error> {
        let raw_vec = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(Arc::new(Mutex::new(raw_vec)))
    }
}

#[derive(Clone, Debug, Default, rkyv::Serialize, rkyv::Archive, rkyv::Deserialize)]
pub struct VecOracle {
    #[rkyv(with = VecPreimageStoreRkyv)]
    pub preimages: VecPreimageStore,
}

impl WitnessOracle for VecOracle {
    fn preimage_count(&self) -> usize {
        self.preimages.lock().unwrap().len()
    }

    fn validate_preimages(&self) -> anyhow::Result<()> {
        for (key, value) in self.preimages.lock().unwrap().iter() {
            validate_preimage(key, value)?;
        }
        Ok(())
    }

    fn insert_preimage(&mut self, key: PreimageKey, value: Vec<u8>) {
        validate_preimage(&key, &value).expect("Attempted to save invalid preimage");
        self.preimages.lock().unwrap().push((key, value));
    }

    fn finalize_preimages(&mut self) {
        self.validate_preimages()
            .expect("Failed to validate preimages during finalization");
        self.preimages.lock().unwrap().reverse();
    }
}

impl FlushableCache for VecOracle {
    fn flush(&self) {}
}

pub type PreimageQueue = VecDeque<(PreimageKey, Vec<u8>)>;

lazy_static! {
    static ref QUEUE: Arc<Mutex<PreimageQueue>> = Default::default();
}

#[async_trait]
impl PreimageOracleClient for VecOracle {
    async fn get(&self, key: PreimageKey) -> PreimageOracleResult<Vec<u8>> {
        let mut preimages = self.preimages.lock().unwrap();
        let mut queue = QUEUE.lock().unwrap();
        // address variations in memory access operations due to hashmap usages
        loop {
            let (last_key, value) = preimages.pop().expect("VecOracle Exhausted");

            if key == last_key {
                if !queue.is_empty() {
                    warn!("VecOracle temp queue has {} elements", queue.len());
                    preimages.extend(core::mem::take(queue.deref_mut()));
                }
                return Ok(value);
            }
            queue.push_front((last_key, value));
        }
    }

    async fn get_exact(&self, key: PreimageKey, buf: &mut [u8]) -> PreimageOracleResult<()> {
        let value = self.get(key).await?;
        buf.copy_from_slice(value.as_ref());
        Ok(())
    }
}

#[async_trait]
impl HintWriterClient for VecOracle {
    async fn write(&self, _hint: &str) -> PreimageOracleResult<()> {
        Ok(())
    }
}
