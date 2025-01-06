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

use crate::blobs::BlobWitnessData;
use crate::oracle::OracleWitnessData;
use alloy_primitives::{Address, B256};
use serde::{Deserialize, Serialize};

#[derive(
    Clone, Debug, Default, Serialize, Deserialize, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct Witness {
    pub oracle_witness: OracleWitnessData,
    pub blobs_witness: BlobWitnessData,
    #[rkyv(with = AddressDef)]
    pub payout_recipient_address: Address,
    #[rkyv(with = B256Def)]
    pub precondition_validation_data_hash: B256,
}

#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = B256)]
#[rkyv(archived = ArchivedB256)]
pub struct B256Def(pub [u8; 32]);

impl From<B256Def> for B256 {
    fn from(value: B256Def) -> Self {
        B256::new(value.0)
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = Address)]
#[rkyv(archived = ArchivedAddress)]
#[rkyv(derive(Hash, Eq, PartialEq))]
pub struct AddressDef(pub [u8; 20]);

impl From<AddressDef> for Address {
    fn from(value: AddressDef) -> Self {
        Address::new(value.0)
    }
}
