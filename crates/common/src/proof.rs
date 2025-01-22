// Copyright 2024, 2025 RISC Zero, Inc.
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

use crate::journal::ProofJournal;
use crate::witness::StitchedBootInfo;
use risc0_aggregation::{merkle_path_root, GuestOutput};
use risc0_zkvm::sha::Digest;
use risc0_zkvm::{Groth16Receipt, Journal, Receipt, ReceiptClaim};
use serde::{Deserialize, Serialize};
use std::borrow::Borrow;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Proof {
    ZKVMReceipt(Box<Receipt>),
    BoundlessSeal(Vec<u8>, Journal),
    SetBuilderReceipt(Box<Groth16Receipt<ReceiptClaim>>, Vec<Digest>, Journal),
}

impl Proof {
    pub fn journal(&self) -> &Journal {
        match self {
            Proof::ZKVMReceipt(receipt) => &receipt.journal,
            Proof::BoundlessSeal(_, journal) => journal,
            Proof::SetBuilderReceipt(_, _, journal) => journal,
        }
    }

    pub fn as_zkvm_receipt(&self) -> Option<&Receipt> {
        match self {
            Proof::ZKVMReceipt(receipt) => Some(receipt),
            _ => None,
        }
    }
}

impl From<&Proof> for ProofJournal {
    fn from(value: &Proof) -> Self {
        Self::decode_packed(value.journal().as_ref()).unwrap()
    }
}

impl From<&Proof> for StitchedBootInfo {
    fn from(value: &Proof) -> Self {
        ProofJournal::from(value).into()
    }
}

pub fn encoded_set_builder_journal(
    fpvm_claim_digest: &Digest,
    set_builder_siblings: impl IntoIterator<Item = impl Borrow<Digest>>,
    set_builder_id: Digest,
) -> Vec<u8> {
    // derive the root of the set of aggregated claims
    let set_builder_root = merkle_path_root(fpvm_claim_digest, set_builder_siblings);
    // construct set builder root from merkle proof
    GuestOutput::new(set_builder_id, set_builder_root).abi_encode()
}
