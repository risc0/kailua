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
//! This module contains code vendored in from https://github.com/Layr-Labs/hokulea

pub mod cert;
pub mod commitment;
pub mod data;
pub mod provider;
pub mod source;

use crate::eigenda::commitment::AltDACommitment;
use crate::eigenda::source::EigenDABlobSource;
use alloy_primitives::{Address, Bytes};
use async_trait::async_trait;
use kona_derive::prelude::{
    BlobProvider, ChainProvider, DataAvailabilityProvider, EthereumDataSource, PipelineError,
    PipelineErrorKind, PipelineResult,
};
use kona_protocol::{BlockInfo, DERIVATION_VERSION_0};
use std::fmt::{Debug, Display};
use tracing::{error, info};

/// This minimal blob encoding contains a 32 byte header = [0x00, version byte, uint32 len of data, 0x00, 0x00,...]
/// followed by the encoded data [0x00, 31 bytes of data, 0x00, 31 bytes of data,...]
pub const PAYLOAD_ENCODING_VERSION_0: u8 = 0x0;

/// A trait for providing EigenDA blobs.
#[async_trait]
pub trait EigenDABlobProvider {
    /// The error type for the [EigenDABlobProvider].
    type Error: Display + ToString + Into<PipelineErrorKind>;

    /// Fetches eigenda blob
    async fn get_blob(
        &mut self,
        altda_commitment: &AltDACommitment,
    ) -> Result<rust_kzg_bn254_primitives::blob::Blob, Self::Error>;
}

/// A factory for creating an Ethereum data source provider.
#[derive(Debug, Clone)]
pub struct EigenDADataSource<C, B, A>
where
    C: ChainProvider + Send + Clone,
    B: BlobProvider + Send + Clone,
    A: EigenDABlobProvider + Send + Clone,
{
    /// The blob source.
    pub ethereum_source: EthereumDataSource<C, B>,
    /// The eigenda source.
    pub eigenda_source: EigenDABlobSource<A>,
}

impl<C, B, A> EigenDADataSource<C, B, A>
where
    C: ChainProvider + Send + Clone + Debug,
    B: BlobProvider + Send + Clone + Debug,
    A: EigenDABlobProvider + Send + Clone + Debug,
{
    /// Instantiates a new [EigenDADataSource].
    pub const fn new(
        ethereum_source: EthereumDataSource<C, B>,
        eigenda_source: EigenDABlobSource<A>,
    ) -> Self {
        Self {
            ethereum_source,
            eigenda_source,
        }
    }
}

#[async_trait]
impl<C, B, A> DataAvailabilityProvider for EigenDADataSource<C, B, A>
where
    C: ChainProvider + Send + Sync + Clone + Debug,
    B: BlobProvider + Send + Sync + Clone + Debug,
    A: EigenDABlobProvider + Send + Sync + Clone + Debug,
{
    type Item = Bytes;

    async fn next(
        &mut self,
        block_ref: &BlockInfo,
        batcher_addr: Address,
    ) -> PipelineResult<Self::Item> {
        info!("EigenDADataSource next {} {}", block_ref, batcher_addr);

        // data is either an op channel frame or an eigenda cert
        let data = self.ethereum_source.next(block_ref, batcher_addr).await?;

        // if data is op channel framce
        if data[0] == DERIVATION_VERSION_0 {
            // see https://github.com/op-rs/kona/blob/ace7c8918be672c1761eba3bd7480cdc1f4fa115/crates/protocol/protocol/src/frame.rs#L140
            return Ok(data);
        }
        if data.len() <= 2 {
            return Err(PipelineError::NotEnoughData.temp());
        }

        let altda_commitment: AltDACommitment = match data[1..].try_into() {
            Ok(a) => a,
            Err(e) => {
                // same handling procedure as in kona
                // https://github.com/op-rs/kona/blob/ace7c8918be672c1761eba3bd7480cdc1f4fa115/crates/protocol/derive/src/stages/frame_queue.rs#L130
                // https://github.com/op-rs/kona/blob/ace7c8918be672c1761eba3bd7480cdc1f4fa115/crates/protocol/derive/src/stages/frame_queue.rs#L165
                error!("failed to parse altda commitment {}", e);
                return Err(PipelineError::NotEnoughData.temp());
            }
        };

        // see https://github.com/ethereum-optimism/optimism/blob/0bb2ff57c8133f1e3983820c0bf238001eca119b/op-alt-da/damgr.go#L211
        // TODO check rbn + STALE_GAP < l1_block_number {
        //info!(
        //    "altda_commitment 0x{}",
        //    hex::encode(altda_commitment.digest_template())
        //);
        let eigenda_blob = self.eigenda_source.next(&altda_commitment).await?;
        Ok(eigenda_blob)
    }

    fn clear(&mut self) {
        self.eigenda_source.clear();
        self.ethereum_source.clear();
    }
}
