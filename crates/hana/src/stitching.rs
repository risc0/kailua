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

use crate::da::CelestiaDataSourceProvider;
use alloy_primitives::{Address, B256};
use hana_oracle::provider::OracleCelestiaProvider;
use kailua_kona::boot::StitchedBootInfo;
use kailua_kona::client::stitching::{KonaStitchingClient, StitchingClient};
use kailua_kona::executor::Execution;
use kailua_kona::journal::ProofJournal;
use kona_derive::prelude::BlobProvider;
use kona_preimage::CommsClient;
use kona_proof::{BootInfo, FlushableCache};
use std::fmt::Debug;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct HanaStitchingClient<T: CommsClient + Clone>(pub Arc<T>);

impl<
        O: CommsClient + FlushableCache + Send + Sync + Debug,
        B: BlobProvider + Send + Sync + Debug + Clone,
        T: CommsClient + Send + Sync + Debug + Clone,
    > StitchingClient<O, B> for HanaStitchingClient<T>
{
    fn run_stitching_client(
        self,
        precondition_validation_data_hash: B256,
        oracle: Arc<O>,
        stream: Arc<O>,
        beacon: B,
        fpvm_image_id: B256,
        payout_recipient_address: Address,
        stitched_executions: Vec<Vec<Execution>>,
        stitched_boot_info: Vec<StitchedBootInfo>,
    ) -> (BootInfo, ProofJournal)
    where
        <B as BlobProvider>::Error: Debug,
    {
        KonaStitchingClient(CelestiaDataSourceProvider(OracleCelestiaProvider::new(
            self.0,
        )))
        .run_stitching_client(
            precondition_validation_data_hash,
            oracle,
            stream,
            beacon,
            fpvm_image_id,
            payout_recipient_address,
            stitched_executions,
            stitched_boot_info,
        )
    }
}
