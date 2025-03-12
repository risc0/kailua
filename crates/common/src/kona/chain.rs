// Copyright 2025 RISC Zero, Inc.
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

// This file is a modified copy of kona_proof::l1::chain_provider

use alloy_consensus::{Header, Receipt, ReceiptEnvelope, TxEnvelope};
use alloy_eips::Decodable2718;
use alloy_primitives::map::B256Map;
use alloy_primitives::{Sealed, B256};
use alloy_rlp::Decodable;
use async_trait::async_trait;
use kona_derive::prelude::ChainProvider;
use kona_mpt::{OrderedListWalker, TrieNode, TrieProvider};
use kona_preimage::{CommsClient, PreimageKey, PreimageKeyType};
use kona_proof::errors::OracleProviderError;
use kona_proof::HintType;
use kona_protocol::BlockInfo;
use std::sync::Arc;

/// The oracle-backed L1 chain provider for the client program.
/// Forked from [kona_proof::l1::OracleL1ChainProvider]
#[derive(Debug, Clone)]
pub struct OracleL1ChainProvider<T: CommsClient> {
    /// The preimage oracle client.
    pub oracle: Arc<T>,
    /// The chain of block headers traversed
    pub headers: Vec<Sealed<Header>>,
    /// The index of each
    pub headers_map: B256Map<usize>,
}

impl<T: CommsClient> OracleL1ChainProvider<T> {
    /// Creates a new [OracleL1ChainProvider] with the given boot information and oracle client.
    pub async fn new(l1_head: B256, oracle: Arc<T>) -> Result<Self, OracleProviderError> {
        // Fetch the header RLP from the oracle.
        HintType::L1BlockHeader
            .with_data(&[l1_head.as_ref()])
            .send(oracle.as_ref())
            .await?;
        let header_rlp = oracle.get(PreimageKey::new_keccak256(*l1_head)).await?;

        // Decode the header RLP into a Header.
        let l1_header = Header::decode(&mut header_rlp.as_slice())
            .map_err(OracleProviderError::Rlp)?
            .seal(l1_head);

        Ok(Self {
            oracle,
            headers: vec![l1_header],
            headers_map: B256Map::from_iter(vec![(l1_head, 0usize)]),
        })
    }
}

#[async_trait]
impl<T: CommsClient + Sync + Send> ChainProvider for OracleL1ChainProvider<T> {
    type Error = OracleProviderError;

    async fn header_by_hash(&mut self, hash: B256) -> Result<Header, Self::Error> {
        // Use cached headers
        if let Some(index) = self.headers_map.get(&hash) {
            return Ok(self.headers[*index].clone().unseal());
        }

        // Fetch the header RLP from the oracle.
        HintType::L1BlockHeader
            .with_data(&[hash.as_ref()])
            .send(self.oracle.as_ref())
            .await?;
        let header_rlp = self.oracle.get(PreimageKey::new_keccak256(*hash)).await?;

        // Decode the header RLP into a Header.
        Header::decode(&mut header_rlp.as_slice()).map_err(OracleProviderError::Rlp)
    }

    async fn block_info_by_number(&mut self, block_number: u64) -> Result<BlockInfo, Self::Error> {
        // Check if the block number is in range. If not, we can fail early.
        if block_number > self.headers[0].number {
            return Err(OracleProviderError::BlockNumberPastHead(
                block_number,
                self.headers[0].number,
            ));
        }

        let header_index = (self.headers[0].number - block_number) as usize;

        // Walk back the block headers to the desired block number.
        while self.headers_map.len() <= header_index {
            let header_hash = self.headers[self.headers_map.len() - 1].parent_hash;
            let header = self
                .header_by_hash(self.headers[self.headers_map.len() - 1].parent_hash)
                .await?;
            self.headers_map.insert(header_hash, self.headers.len());
            self.headers.push(header.seal(header_hash));
        }

        let header = &self.headers[header_index];

        Ok(BlockInfo {
            hash: header.hash(),
            number: header.number,
            parent_hash: header.parent_hash,
            timestamp: header.timestamp,
        })
    }

    async fn receipts_by_hash(&mut self, hash: B256) -> Result<Vec<Receipt>, Self::Error> {
        // Fetch the block header to find the receipts root.
        let header = self.header_by_hash(hash).await?;

        // Send a hint for the block's receipts, and walk through the receipts trie in the header to
        // verify them.
        HintType::L1Receipts
            .with_data(&[hash.as_ref()])
            .send(self.oracle.as_ref())
            .await?;
        let trie_walker = OrderedListWalker::try_new_hydrated(header.receipts_root, self)
            .map_err(OracleProviderError::TrieWalker)?;

        // Decode the receipts within the receipts trie.
        let receipts = trie_walker
            .into_iter()
            .map(|(_, rlp)| {
                let envelope = ReceiptEnvelope::decode_2718(&mut rlp.as_ref())?;
                Ok(envelope.as_receipt().expect("Infallible").clone())
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(OracleProviderError::Rlp)?;

        Ok(receipts)
    }

    async fn block_info_and_transactions_by_hash(
        &mut self,
        hash: B256,
    ) -> Result<(BlockInfo, Vec<TxEnvelope>), Self::Error> {
        // Fetch the block header to construct the block info.
        let header = self.header_by_hash(hash).await?;
        let block_info = BlockInfo {
            hash,
            number: header.number,
            parent_hash: header.parent_hash,
            timestamp: header.timestamp,
        };

        // Send a hint for the block's transactions, and walk through the transactions trie in the
        // header to verify them.
        HintType::L1Transactions
            .with_data(&[hash.as_ref()])
            .send(self.oracle.as_ref())
            .await?;
        let trie_walker = OrderedListWalker::try_new_hydrated(header.transactions_root, self)
            .map_err(OracleProviderError::TrieWalker)?;

        // Decode the transactions within the transactions trie.
        let transactions = trie_walker
            .into_iter()
            .map(|(_, rlp)| {
                // note: not short-handed for error type coersion w/ `?`.
                let rlp = TxEnvelope::decode_2718(&mut rlp.as_ref())?;
                Ok(rlp)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(OracleProviderError::Rlp)?;

        Ok((block_info, transactions))
    }
}

impl<T: CommsClient> TrieProvider for OracleL1ChainProvider<T> {
    type Error = OracleProviderError;

    fn trie_node_by_hash(&self, key: B256) -> Result<TrieNode, Self::Error> {
        // On L1, trie node preimages are stored as keccak preimage types in the oracle. We assume
        // that a hint for these preimages has already been sent, prior to this call.
        kona_proof::block_on(async move {
            TrieNode::decode(
                &mut self
                    .oracle
                    .get(PreimageKey::new(*key, PreimageKeyType::Keccak256))
                    .await
                    .map_err(OracleProviderError::Preimage)?
                    .as_ref(),
            )
            .map_err(OracleProviderError::Rlp)
        })
    }
}
