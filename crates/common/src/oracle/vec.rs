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

use crate::oracle::{needs_validation, validate_preimage};
use crate::rkyv::PreimageVecStoreRkyv;
use crate::witness::WitnessOracle;
use alloy_primitives::map::HashMap;
use anyhow::bail;
use async_trait::async_trait;
use kona_preimage::errors::PreimageOracleResult;
use kona_preimage::{HintWriterClient, PreimageKey, PreimageOracleClient};
use kona_proof::FlushableCache;
use lazy_static::lazy_static;
use std::collections::VecDeque;
use std::ops::DerefMut;
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

pub type IndexedPreimage = (PreimageKey, Vec<u8>, Option<(usize, usize)>);
pub type PreimageVecEntry = Vec<IndexedPreimage>;
pub type PreimageVecStore = Arc<Mutex<Vec<PreimageVecEntry>>>;

#[derive(Clone, Debug, Default, rkyv::Serialize, rkyv::Archive, rkyv::Deserialize)]
pub struct VecOracle {
    #[rkyv(with = PreimageVecStoreRkyv)]
    pub preimages: PreimageVecStore,
}

impl VecOracle {
    pub fn deep_clone(&self) -> Self {
        let mut cloned_with_arc = self.clone();
        cloned_with_arc.preimages = Arc::new(Mutex::new(self.preimages.lock().unwrap().clone()));
        cloned_with_arc
    }
}

impl WitnessOracle for VecOracle {
    fn preimage_count(&self) -> usize {
        self.preimages.lock().unwrap().iter().map(Vec::len).sum()
    }

    fn validate_preimages(&self) -> anyhow::Result<()> {
        let preimages = self.preimages.lock().unwrap();
        for (e, entry) in preimages.iter().enumerate() {
            for (p, (key, value, prev)) in entry.iter().enumerate() {
                if !needs_validation(&key.key_type()) {
                    continue;
                } else if let Some((i, j)) = prev {
                    if e < *i {
                        bail!("Attempted to validate preimage against future vec entry.");
                    } else if e == *i && p <= *j {
                        bail!(
                            "Attempted to validate preimage against future preimage in vec entry."
                        );
                    } else if key != &preimages[*i][*j].0 {
                        bail!("Cached preimage key comparison failed");
                    } else if value != &preimages[*i][*j].1 {
                        bail!("Cached preimage value comparison failed");
                    } else {
                        continue;
                    }
                }
                validate_preimage(key, value)?;
            }
        }
        Ok(())
    }

    fn insert_preimage(&mut self, key: PreimageKey, value: Vec<u8>) {
        validate_preimage(&key, &value).expect("Attempted to save invalid preimage");
        let mut preimages = self.preimages.lock().unwrap();
        if preimages.is_empty() {
            preimages.push(Vec::new());
        }
        preimages.last_mut().unwrap().push((key, value, None));
    }

    fn finalize_preimages(&mut self, shard_size: usize) {
        info!("Finalizing preimages with shard size: {shard_size}");
        self.validate_preimages()
            .expect("Failed to validate preimages during finalization");
        let mut preimages = self.preimages.lock().unwrap();
        // flatten and sort
        let mut flat_vec = core::mem::take(preimages.deref_mut())
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        // sort by expected access
        flat_vec.reverse();
        // shard vectors by size limit
        let mut sharded_vec = vec![vec![]];
        let mut last_shard_size = 0;
        for value in flat_vec {
            if value.1.len() + last_shard_size > shard_size {
                sharded_vec.push(vec![]);
                last_shard_size = 0;
            }
            last_shard_size += value.1.len();
            sharded_vec.last_mut().unwrap().push(value);
        }
        let _ = core::mem::replace(preimages.deref_mut(), sharded_vec);
        // add validation pointers
        let mut cache: HashMap<PreimageKey, (usize, usize)> =
            HashMap::with_capacity(preimages.len());
        for (i, entry) in preimages.iter_mut().enumerate() {
            for (j, (key, _, pointer)) in entry.iter_mut().enumerate() {
                if !needs_validation(&key.key_type()) {
                    continue;
                } else if let Some(prev) = cache.insert(*key, (i, j)) {
                    pointer.replace(prev);
                }
            }
        }
    }
}

impl FlushableCache for VecOracle {
    fn flush(&self) {}
}

pub type PreimageQueue = VecDeque<IndexedPreimage>;

lazy_static! {
    static ref QUEUE: Arc<Mutex<PreimageQueue>> = Default::default();
}

#[async_trait]
impl PreimageOracleClient for VecOracle {
    async fn get(&self, key: PreimageKey) -> PreimageOracleResult<Vec<u8>> {
        let mut preimages = self.preimages.lock().unwrap();
        let mut queue = QUEUE.lock().unwrap();
        // handle variations in memory access operations due to hashmap usages
        loop {
            let entry = preimages.last_mut().unwrap_or_else(|| {
                panic!(
                    "Exhausted VecOracle seeking {key} ({} queued preimages)",
                    queue.len()
                )
            });
            loop {
                let Some((last_key, value, _)) = entry.pop() else {
                    break;
                };

                if key == last_key {
                    if !queue.is_empty() {
                        warn!("VecOracle temp queue has {} elements", queue.len());
                        entry.extend(core::mem::take(queue.deref_mut()));
                    }
                    return Ok(value);
                }
                // keep entry in queue for later use, pointer is no longer necessary
                queue.push_front((last_key, value, None));
            }
            preimages.pop();
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
