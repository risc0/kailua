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

use crate::eigenda::{commitment::AltDACommitment, data::EigenDABlobData, EigenDABlobProvider};
use alloy_primitives::Bytes;
use kona_derive::{
    errors::{BlobProviderError, PipelineError},
    types::PipelineResult,
};
use std::collections::VecDeque;
use tracing::{error, info, warn};

/// A data iterator that reads from a blob.
#[derive(Debug, Clone)]
pub struct EigenDABlobSource<B>
where
    B: EigenDABlobProvider + Send,
{
    /// Fetches blobs.
    pub eigenda_fetcher: B,
    /// EigenDA blobs.
    pub data: VecDeque<EigenDABlobData>,
    /// Whether the source is open.
    pub open: bool,
}

impl<B> EigenDABlobSource<B>
where
    B: EigenDABlobProvider + Send,
{
    /// Creates a new blob source.
    pub const fn new(eigenda_fetcher: B) -> Self {
        Self {
            eigenda_fetcher,
            data: VecDeque::new(),
            open: false,
        }
    }

    /// Fetches the next blob from the source.
    pub async fn next(&mut self, eigenda_commitment: &AltDACommitment) -> PipelineResult<Bytes> {
        // Use the fetcher
        self.load_blobs(eigenda_commitment).await?;
        // Load fetched blobs
        let next_data = match self.next_data() {
            Ok(d) => d,
            Err(e) => return e,
        };
        // Decode the blob data to raw bytes.
        // Otherwise, ignore blob and recurse next.
        match next_data.decode() {
            Ok(d) => Ok(d),
            Err(e) => {
                warn!(target: "blob-source", "Failed to decode blob data, skipping {}", e);
                panic!()
            }
        }
    }

    /// Loads blob data into the source if it is not open.
    async fn load_blobs(
        &mut self,
        eigenda_commitment: &AltDACommitment,
    ) -> Result<(), BlobProviderError> {
        if self.open {
            return Ok(());
        }

        match self.eigenda_fetcher.get_blob(eigenda_commitment).await {
            Ok(data) => {
                self.open = true;
                let new_blob: Vec<u8> = data.into();
                let eigenda_blob = EigenDABlobData {
                    blob: new_blob.into(),
                };
                self.data.push_back(eigenda_blob);

                info!(target: "eigenda-blobsource", "load_blobs {:?}", self.data);

                Ok(())
            }
            Err(e) => {
                error!("EigenDA blob source fetching error {}", e);
                self.open = true;
                Ok(())
            }
        }
    }

    fn next_data(&mut self) -> Result<EigenDABlobData, PipelineResult<Bytes>> {
        info!(target: "eigenda-blobsource", "self.data.is_empty() {:?}", self.data.is_empty());

        self.data
            .pop_front()
            .ok_or_else(|| Err(PipelineError::Eof.temp()))
    }

    /// Clears the source.
    pub fn clear(&mut self) {
        self.data.clear();
        self.open = false;
    }
}
