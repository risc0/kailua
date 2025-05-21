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

use alloy_primitives::{Address, B256, B64};

#[derive(Clone, Debug, Copy, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = B256)]
#[rkyv(archived = ArchivedB256)]
pub struct B256Def(pub [u8; 32]);

impl From<B256Def> for B256 {
    fn from(value: B256Def) -> Self {
        B256::new(value.0)
    }
}

#[derive(Clone, Debug, Copy, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = B64)]
#[rkyv(archived = ArchivedB64)]
pub struct B64Def(pub [u8; 8]);

impl From<B64Def> for B64 {
    fn from(value: B64Def) -> Self {
        B64::new(value.0)
    }
}

#[derive(
    Clone, Debug, Copy, Hash, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
#[rkyv(remote = Address)]
#[rkyv(archived = ArchivedAddress)]
#[rkyv(derive(Hash, Eq, PartialEq))]
pub struct AddressDef(pub [u8; 20]);

impl From<AddressDef> for Address {
    fn from(value: AddressDef) -> Self {
        Address::new(value.0)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::{from_bytes_with, to_bytes_with};

    #[test]
    fn test_b256() {
        let bytes = [42u8; 32];
        let def = B256Def(bytes);
        let b256 = B256::from(def);
        assert_eq!(b256.as_slice(), &bytes);

        let serialized = to_bytes_with!(B256Def, &b256);
        let deserialized = from_bytes_with!(B256Def, B256, &serialized);
        assert_eq!(def.0, deserialized.0);
    }

    #[test]
    fn test_b64() {
        let bytes = [42u8; 8];
        let def = B64Def(bytes);
        let b64 = B64::from(def);
        assert_eq!(b64.as_slice(), &bytes);

        let serialized = to_bytes_with!(B64Def, &b64);
        let deserialized = from_bytes_with!(B64Def, B64, &serialized);
        assert_eq!(def.0, deserialized.0);
    }

    #[test]
    fn test_address() {
        let bytes = [42u8; 20];
        let def = AddressDef(bytes);
        let addr = Address::from(def);
        assert_eq!(addr.as_slice(), &bytes);

        let serialized = to_bytes_with!(AddressDef, &addr);
        let deserialized = from_bytes_with!(AddressDef, Address, &serialized);
        assert_eq!(def.0, deserialized.0);
    }
}
