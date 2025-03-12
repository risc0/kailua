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

// This file is a modified copy of kona_proof::l1::pipeline

use crate::kona::chain::OracleL1ChainProvider;
use async_trait::async_trait;
use kona_derive::attributes::StatefulAttributesBuilder;
use kona_derive::errors::PipelineErrorKind;
use kona_derive::pipeline::{DerivationPipeline, PipelineBuilder};
use kona_derive::prelude::{
    AttributesQueue, BatchProvider, BatchStream, BlobProvider, ChannelProvider, ChannelReader,
    EthereumDataSource, FrameQueue, L1Retrieval, L1Traversal, OriginProvider, Pipeline,
    PipelineResult, Signal, SignalReceiver, StepResult,
};
use kona_driver::{DriverPipeline, PipelineCursor};
use kona_genesis::{RollupConfig, SystemConfig};
use kona_preimage::CommsClient;
use kona_proof::l2::OracleL2ChainProvider;
use kona_proof::FlushableCache;
use kona_protocol::{BlockInfo, L2BlockInfo};
use kona_rpc::OpAttributesWithParent;
use spin::RwLock;
use std::fmt::Debug;
use std::sync::Arc;

/// An oracle-backed derivation pipeline.
pub type OracleDerivationPipeline<O, B> = DerivationPipeline<
    OracleAttributesQueue<OracleDataProvider<O, B>, O>,
    OracleL2ChainProvider<O>,
>;

/// An oracle-backed Ethereum data source.
pub type OracleDataProvider<O, B> = EthereumDataSource<OracleL1ChainProvider<O>, B>;

/// An oracle-backed payload attributes builder for the `AttributesQueue` stage of the derivation
/// pipeline.
pub type OracleAttributesBuilder<O> =
    StatefulAttributesBuilder<OracleL1ChainProvider<O>, OracleL2ChainProvider<O>>;

/// An oracle-backed attributes queue for the derivation pipeline.
pub type OracleAttributesQueue<DAP, O> = AttributesQueue<
    BatchProvider<
        BatchStream<
            ChannelReader<
                ChannelProvider<
                    FrameQueue<L1Retrieval<DAP, L1Traversal<OracleL1ChainProvider<O>>>>,
                >,
            >,
            OracleL2ChainProvider<O>,
        >,
        OracleL2ChainProvider<O>,
    >,
    OracleAttributesBuilder<O>,
>;

/// An oracle-backed derivation pipeline.
#[derive(Debug)]
pub struct OraclePipeline<O, B>
where
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
{
    /// The internal derivation pipeline.
    pub pipeline: OracleDerivationPipeline<O, B>,
    /// The caching oracle.
    pub caching_oracle: Arc<O>,
}

impl<O, B> OraclePipeline<O, B>
where
    O: CommsClient + FlushableCache + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
{
    /// Constructs a new oracle-backed derivation pipeline.
    pub fn new(
        cfg: Arc<RollupConfig>,
        sync_start: Arc<RwLock<PipelineCursor>>,
        caching_oracle: Arc<O>,
        blob_provider: B,
        chain_provider: OracleL1ChainProvider<O>,
        l2_chain_provider: OracleL2ChainProvider<O>,
    ) -> Self {
        let attributes = StatefulAttributesBuilder::new(
            cfg.clone(),
            l2_chain_provider.clone(),
            chain_provider.clone(),
        );
        let dap = EthereumDataSource::new_from_parts(chain_provider.clone(), blob_provider, &cfg);

        let pipeline = PipelineBuilder::new()
            .rollup_config(cfg)
            .dap_source(dap)
            .l2_chain_provider(l2_chain_provider)
            .chain_provider(chain_provider)
            .builder(attributes)
            .origin(sync_start.read().origin())
            .build();
        Self {
            pipeline,
            caching_oracle,
        }
    }
}

impl<O, B> DriverPipeline<OracleDerivationPipeline<O, B>> for OraclePipeline<O, B>
where
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
{
    /// Flushes the cache on re-org.
    fn flush(&mut self) {
        self.caching_oracle.flush();
    }
}

#[async_trait]
impl<O, B> SignalReceiver for OraclePipeline<O, B>
where
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
{
    /// Receives a signal from the driver.
    async fn signal(&mut self, signal: Signal) -> PipelineResult<()> {
        self.pipeline.signal(signal).await
    }
}

impl<O, B> OriginProvider for OraclePipeline<O, B>
where
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
{
    /// Returns the optional L1 [BlockInfo] origin.
    fn origin(&self) -> Option<BlockInfo> {
        self.pipeline.origin()
    }
}

impl<O, B> Iterator for OraclePipeline<O, B>
where
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
{
    type Item = OpAttributesWithParent;

    fn next(&mut self) -> Option<Self::Item> {
        self.pipeline.next()
    }
}

#[async_trait]
impl<O, B> Pipeline for OraclePipeline<O, B>
where
    O: CommsClient + FlushableCache + Send + Sync + Debug,
    B: BlobProvider + Send + Sync + Debug + Clone,
{
    /// Peeks at the next [OpAttributesWithParent] from the pipeline.
    fn peek(&self) -> Option<&OpAttributesWithParent> {
        self.pipeline.peek()
    }

    /// Attempts to progress the pipeline.
    async fn step(&mut self, cursor: L2BlockInfo) -> StepResult {
        self.pipeline.step(cursor).await
    }

    /// Returns the rollup config.
    fn rollup_config(&self) -> &RollupConfig {
        self.pipeline.rollup_config()
    }

    /// Returns the [SystemConfig] by L2 number.
    async fn system_config_by_number(
        &mut self,
        number: u64,
    ) -> Result<SystemConfig, PipelineErrorKind> {
        self.pipeline.system_config_by_number(number).await
    }
}
