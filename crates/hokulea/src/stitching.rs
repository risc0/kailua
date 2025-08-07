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

use crate::canoe::KailuaCanoeVerifier;
use crate::da::EigenDADataSourceProvider;
use crate::witness::{da_witness_postcondition, da_witness_precondition};
use alloy_primitives::aliases::B256;
use alloy_primitives::Address;
use hokulea_proof::eigenda_blob_witness::EigenDABlobWitnessData;
use hokulea_proof::preloaded_eigenda_provider::PreloadedEigenDABlobProvider;
use kailua_kona::boot::StitchedBootInfo;
use kailua_kona::client::stitching::{KonaStitchingClient, StitchingClient};
use kailua_kona::executor::Execution;
use kailua_kona::journal::ProofJournal;
use kona_derive::prelude::BlobProvider;
use kona_preimage::CommsClient;
use kona_proof::boot::BootInfo;
use kona_proof::FlushableCache;
use std::fmt::Debug;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct HokuleaStitchingClient {
    pub eigen_da_witness: EigenDABlobWitnessData,
    pub canoe_image_id: B256,
}

impl HokuleaStitchingClient {
    pub fn new(eigen_da_witness: EigenDABlobWitnessData, canoe_image_id: B256) -> Self {
        Self {
            eigen_da_witness,
            canoe_image_id,
        }
    }
}

impl<
        O: CommsClient + FlushableCache + Send + Sync + Debug,
        B: BlobProvider + Send + Sync + Debug + Clone,
    > StitchingClient<O, B> for HokuleaStitchingClient
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
        let eigen_da_precondition = da_witness_precondition(&self.eigen_da_witness);

        let eigen_da = PreloadedEigenDABlobProvider::from_witness(
            self.eigen_da_witness,
            KailuaCanoeVerifier(self.canoe_image_id.0),
        );

        let (boot, proof_journal) = KonaStitchingClient(EigenDADataSourceProvider(eigen_da))
            .run_stitching_client(
                precondition_validation_data_hash,
                oracle,
                stream,
                beacon,
                fpvm_image_id,
                payout_recipient_address,
                stitched_executions,
                stitched_boot_info,
            );

        da_witness_postcondition(eigen_da_precondition, &boot);

        (boot, proof_journal)
    }
}
