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

use alloy_consensus::Header;
use alloy_primitives::{Address, B256, U256};
use hashbrown::HashMap;
use kona_mpt::{TrieHinter, TrieNode, TrieProvider};
use std::fmt::Debug;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct ExecutionProvider<F: TrieProvider + TrieHinter + Debug + Clone + 'static> {
    pub tries: Arc<Mutex<HashMap<B256, TrieNode>>>,
    pub contracts: Arc<Mutex<HashMap<B256, Vec<u8>>>>,
    pub headers: Arc<Mutex<HashMap<B256, Header>>>,
    pub fallback: F,
}

impl<F: TrieProvider + TrieHinter + Debug + Clone + 'static> TrieProvider for ExecutionProvider<F>
where
    <F as TrieProvider>::Error: Send + Sync + std::error::Error,
{
    type Error = anyhow::Error;

    fn trie_node_by_hash(&self, key: B256) -> Result<TrieNode, Self::Error> {
        let mut tries = self.tries.lock().unwrap();
        if let Some(trie_node) = tries.remove(&key) {
            return Ok(trie_node);
        }
        drop(tries);
        // the fallback takes care of repeated lookups
        self.fallback.trie_node_by_hash(key).map_err(Into::into)
    }
}

impl<F: TrieProvider + TrieHinter + Debug + Clone + 'static> TrieHinter for ExecutionProvider<F> {
    type Error = <F as TrieHinter>::Error;

    fn hint_trie_node(&self, hash: B256) -> Result<(), Self::Error> {
        self.fallback.hint_trie_node(hash)
    }

    fn hint_account_proof(&self, address: Address, block_number: u64) -> Result<(), Self::Error> {
        self.fallback.hint_account_proof(address, block_number)
    }

    fn hint_storage_proof(
        &self,
        address: Address,
        slot: U256,
        block_number: u64,
    ) -> Result<(), Self::Error> {
        self.fallback
            .hint_storage_proof(address, slot, block_number)
    }
}
