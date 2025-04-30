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

use alloy_eips::eip4844::{Blob, BYTES_PER_BLOB};
use c_kzg::Bytes48;

#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = Blob)]
#[rkyv(archived = ArchivedBlob)]
pub struct BlobDef(pub [u8; BYTES_PER_BLOB]);

impl From<BlobDef> for Blob {
    fn from(value: BlobDef) -> Self {
        Self(value.0)
    }
}

#[derive(
    rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Copy, Clone, Hash, PartialEq, Eq,
)]
#[rkyv(remote = Bytes48)]
#[rkyv(archived = ArchivedBytes48)]
pub struct Bytes48Def {
    #[rkyv(getter = get_48_bytes)]
    bytes: [u8; 48usize],
}

fn get_48_bytes(value: &Bytes48) -> [u8; 48] {
    value.into_inner()
}

impl From<Bytes48Def> for Bytes48 {
    fn from(value: Bytes48Def) -> Self {
        Self::from(value.bytes)
    }
}
