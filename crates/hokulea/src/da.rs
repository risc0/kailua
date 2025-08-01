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

use kailua_kona::client::core::{DASourceProvider, EthereumDataSourceProvider};
use kona_derive::prelude::{BlobProvider, ChainProvider};
use kona_genesis::RollupConfig;
use std::fmt::Debug;

#[derive(Clone, Debug)]
pub struct EigenDADataSourceProvider<
    E: hokulea_eigenda::EigenDABlobProvider + Send + Sync + Clone + Debug,
>(pub E);

impl<
        C: ChainProvider + Send + Sync + Clone + Debug,
        B: BlobProvider + Send + Sync + Clone + Debug,
        E: hokulea_eigenda::EigenDABlobProvider + Send + Sync + Clone + Debug,
    > DASourceProvider<C, B> for EigenDADataSourceProvider<E>
{
    type DAS = hokulea_eigenda::EigenDADataSource<C, B, E>;

    fn new_from_parts(self, l1_provider: C, blobs: B, cfg: &RollupConfig) -> Self::DAS {
        hokulea_eigenda::EigenDADataSource::new(
            EthereumDataSourceProvider.new_from_parts(l1_provider, blobs, cfg),
            hokulea_eigenda::EigenDABlobSource::new(self.0),
        )
    }
}
