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

use crate::client::log;
use crate::config::safe_default;
use crate::rkyv::{B256Def, ExecutionArtifactsRkyv, OpPayloadAttributesRkyv};
use alloy_consensus::Header;
use alloy_eips::eip4895::Withdrawal;
use alloy_eips::Encodable2718;
use alloy_primitives::{Bytes, Sealed, B256, B64};
use anyhow::Context;
use async_trait::async_trait;
use kona_driver::{Executor, PipelineCursor, TipCursor};
use kona_executor::ExecutionArtifacts;
use kona_mpt::ordered_trie_with_encoder;
use kona_preimage::CommsClient;
use kona_proof::errors::OracleProviderError;
use kona_proof::l2::OracleL2ChainProvider;
use kona_proof::FlushableCache;
use maili_genesis::RollupConfig;
use maili_protocol::{BatchValidationProvider, BlockInfo};
use op_alloy_consensus::OpReceiptEnvelope;
use op_alloy_rpc_types_engine::OpPayloadAttributes;
use risc0_zkvm::sha::{Impl as SHA2, Sha256};
use spin::RwLock;
use std::fmt::Debug;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Execution {
    /// Output root prior to execution
    #[rkyv(with = B256Def)]
    pub agreed_output: B256,
    /// Derived attributes to be executed
    #[rkyv(with = OpPayloadAttributesRkyv)]
    pub attributes: OpPayloadAttributes,
    /// Output block from execution
    #[rkyv(with = ExecutionArtifactsRkyv)]
    pub artifacts: ExecutionArtifacts,
    /// Output root after execution
    #[rkyv(with = B256Def)]
    pub claimed_output: B256,
}

#[derive(Debug)]
pub struct CachedExecutor<E: Executor + Send + Sync + Debug> {
    pub cache: Vec<Arc<Execution>>,
    pub executor: E,
    pub collection_target: Option<Arc<Mutex<Vec<Execution>>>>,
}

#[async_trait]
impl<E: Executor + Send + Sync + Debug> Executor for CachedExecutor<E> {
    type Error = <E as Executor>::Error;

    async fn wait_until_ready(&mut self) {
        self.executor.wait_until_ready().await;
    }

    fn update_safe_head(&mut self, header: Sealed<Header>) {
        self.executor.update_safe_head(header);
    }

    async fn execute_payload(
        &mut self,
        attributes: OpPayloadAttributes,
    ) -> Result<ExecutionArtifacts, Self::Error> {
        let agreed_output = self.compute_output_root()?;
        if self
            .cache
            .last()
            .map(|e| Ok(agreed_output == e.agreed_output && attributes == e.attributes))
            .unwrap_or(Ok(false))?
        {
            let artifacts = self.cache.pop().unwrap().artifacts.clone();
            log(&format!("CACHE {}", artifacts.block_header.number));
            self.update_safe_head(artifacts.block_header.clone());
            return Ok(artifacts);
        }
        if let Some(collection_target) = &self.collection_target {
            let artifacts = self.executor.execute_payload(attributes.clone()).await?;
            let mut collection_target = collection_target.lock().unwrap();
            collection_target.push(Execution {
                agreed_output,
                attributes,
                artifacts: artifacts.clone(),
                claimed_output: Default::default(),
            });
            return Ok(artifacts);
        }
        self.executor.execute_payload(attributes).await
    }

    fn compute_output_root(&mut self) -> Result<B256, Self::Error> {
        self.executor.compute_output_root()
    }
}

pub async fn new_execution_cursor<O>(
    rollup_config: &RollupConfig,
    safe_header: Sealed<Header>,
    l2_chain_provider: &mut OracleL2ChainProvider<O>,
) -> Result<Arc<RwLock<PipelineCursor>>, OracleProviderError>
where
    O: CommsClient + FlushableCache + FlushableCache + Send + Sync + Debug,
{
    let safe_head_info = l2_chain_provider
        .l2_block_info_by_number(safe_header.number)
        .await?;

    // Walk back the starting L1 block by `channel_timeout` to ensure that the full channel is
    // captured.
    let channel_timeout = rollup_config.channel_timeout(safe_head_info.block_info.timestamp);

    // Construct the cursor.
    let mut cursor = PipelineCursor::new(channel_timeout, BlockInfo::default());
    let tip = TipCursor::new(safe_head_info, safe_header, B256::ZERO);
    cursor.advance(BlockInfo::default(), tip);

    // Wrap the cursor in a shared read-write lock
    Ok(Arc::new(RwLock::new(cursor)))
}

pub fn attributes_hash(attributes: &OpPayloadAttributes) -> anyhow::Result<B256> {
    let hashed_bytes = [
        attributes
            .payload_attributes
            .timestamp
            .to_be_bytes()
            .as_slice(),
        attributes.payload_attributes.prev_randao.as_slice(),
        attributes
            .payload_attributes
            .suggested_fee_recipient
            .as_slice(),
        safe_default(
            attributes
                .payload_attributes
                .withdrawals
                .as_ref()
                .map(|wds| withdrawals_hash(wds.as_slice())),
            B256::ZERO,
        )
        .context("safe_default withdrawals")?
        .as_slice(),
        safe_default(
            attributes.payload_attributes.parent_beacon_block_root,
            B256::ZERO,
        )
        .context("safe_default parent_beacon_block_root")?
        .as_slice(),
        safe_default(
            attributes.transactions.as_ref().map(transactions_hash),
            B256::ZERO,
        )
        .context("safe_default transactions_hash")?
        .as_slice(),
        &[safe_default(attributes.no_tx_pool, false).context("safe_default no_tx_pool")? as u8],
        safe_default(attributes.gas_limit, u64::MAX)
            .context("safe_default gas_limit")?
            .to_be_bytes()
            .as_slice(),
        safe_default(attributes.eip_1559_params, B64::new([0xff; 8]))
            .context("safe_default eip_1559_params")?
            .as_slice(),
    ]
    .concat();
    let digest: [u8; 32] = SHA2::hash_bytes(hashed_bytes.as_slice())
        .as_bytes()
        .try_into()?;
    Ok(digest.into())
}

pub fn withdrawals_hash(withdrawals: &[Withdrawal]) -> B256 {
    let hashed_bytes = withdrawals
        .iter()
        .map(|w| {
            [
                w.index.to_be_bytes().as_slice(),
                w.validator_index.to_be_bytes().as_slice(),
                w.address.as_slice(),
                w.amount.to_be_bytes().as_slice(),
            ]
            .concat()
        })
        .collect::<Vec<_>>()
        .concat();
    let digest: [u8; 32] = SHA2::hash_bytes(hashed_bytes.as_slice())
        .as_bytes()
        .try_into()
        .unwrap();
    digest.into()
}

pub fn transactions_hash(transactions: &Vec<Bytes>) -> B256 {
    let hashed_bytes = alloy_rlp::encode(transactions);
    let digest: [u8; 32] = SHA2::hash_bytes(hashed_bytes.as_slice())
        .as_bytes()
        .try_into()
        .unwrap();
    digest.into()
}

pub fn exec_precondition_hash(executions: &[Arc<Execution>]) -> B256 {
    let hashed_bytes = executions
        .iter()
        .map(|e| {
            [
                e.agreed_output.0,
                attributes_hash(&e.attributes)
                    .expect("Unhashable attributes.")
                    .0,
                e.artifacts.block_header.hash().0,
                e.claimed_output.0,
            ]
            .concat()
        })
        .collect::<Vec<_>>()
        .concat();
    let digest: [u8; 32] = SHA2::hash_bytes(hashed_bytes.as_slice())
        .as_bytes()
        .try_into()
        .unwrap();
    digest.into()
}

/// Computes the receipts root from the given set of receipts.
///
/// ## Takes
/// - `receipts`: The receipts to compute the root for.
/// - `config`: The rollup config to use for the computation.
/// - `timestamp`: The timestamp to use for the computation.
///
/// ## Returns
/// The computed receipts root.
pub fn compute_receipts_root(
    receipts: &[OpReceiptEnvelope],
    config: &RollupConfig,
    timestamp: u64,
) -> B256 {
    // There is a minor bug in op-geth and op-erigon where in the Regolith hardfork,
    // the receipt root calculation does not inclide the deposit nonce in the
    // receipt encoding. In the Regolith hardfork, we must strip the deposit nonce
    // from the receipt encoding to match the receipt root calculation.
    if config.is_regolith_active(timestamp) && !config.is_canyon_active(timestamp) {
        let receipts = receipts
            .iter()
            .cloned()
            .map(|receipt| match receipt {
                OpReceiptEnvelope::Deposit(mut deposit_receipt) => {
                    deposit_receipt.receipt.deposit_nonce = None;
                    OpReceiptEnvelope::Deposit(deposit_receipt)
                }
                _ => receipt,
            })
            .collect::<Vec<_>>();

        ordered_trie_with_encoder(receipts.as_ref(), |receipt, mut buf| {
            receipt.encode_2718(&mut buf)
        })
        .root()
    } else {
        ordered_trie_with_encoder(receipts, |receipt, mut buf| receipt.encode_2718(&mut buf)).root()
    }
}
