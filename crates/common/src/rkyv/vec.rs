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

use crate::oracle::vec::{PreimageVecEntry, PreimageVecStore};
use rkyv::rancor::Fallible;
use rkyv::with::{ArchiveWith, DeserializeWith, SerializeWith};
use rkyv::{Archive, Archived, Place, Resolver, Serialize};
use std::sync::{Arc, Mutex};

pub struct PreimageVecStoreRkyv;

impl ArchiveWith<PreimageVecStore> for PreimageVecStoreRkyv {
    type Archived = Archived<Vec<PreimageVecEntry>>;
    type Resolver = Resolver<Vec<PreimageVecEntry>>;

    fn resolve_with(
        field: &PreimageVecStore,
        resolver: Self::Resolver,
        out: Place<Self::Archived>,
    ) {
        let locked_vec = field.lock().unwrap();
        <Vec<PreimageVecEntry> as Archive>::resolve(&locked_vec, resolver, out);
    }
}

impl<S> SerializeWith<PreimageVecStore, S> for PreimageVecStoreRkyv
where
    S: Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    <S as Fallible>::Error: rkyv::rancor::Source,
{
    fn serialize_with(
        field: &PreimageVecStore,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        let locked_vec = field.lock().unwrap();
        <Vec<PreimageVecEntry> as Serialize<S>>::serialize(&locked_vec, serializer)
    }
}

impl<D: Fallible> DeserializeWith<Archived<Vec<PreimageVecEntry>>, PreimageVecStore, D>
    for PreimageVecStoreRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: rkyv::rancor::Source,
{
    fn deserialize_with(
        field: &Archived<Vec<PreimageVecEntry>>,
        deserializer: &mut D,
    ) -> Result<PreimageVecStore, D::Error> {
        let raw_vec = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(Arc::new(Mutex::new(raw_vec)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{from_bytes_with, to_bytes_with};
    use kona_preimage::{PreimageKey, PreimageKeyType};

    #[test]
    fn test_serialize_empty_store() {
        let store = PreimageVecStore::default();
        let bytes = to_bytes_with!(PreimageVecStoreRkyv, &store);
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_serialize_with_entries() {
        let store = Arc::new(Mutex::new(vec![
            vec![(
                PreimageKey::new([0x11; 32], PreimageKeyType::Keccak256),
                vec![0x22u8; 32],
                Some((0usize, 1usize)),
            )],
            vec![(
                PreimageKey::new([0x33; 32], PreimageKeyType::Keccak256),
                vec![0x44u8; 32],
                Some((2usize, 3usize)),
            )],
        ]));
        let bytes = to_bytes_with!(PreimageVecStoreRkyv, &store);
        assert!(!bytes.is_empty());
    }

    #[test]
    fn test_round_trip() {
        let original = Arc::new(Mutex::new(vec![
            vec![(
                PreimageKey::new([0x11; 32], PreimageKeyType::Keccak256),
                vec![0x22u8; 32],
                Some((0usize, 1usize)),
            )],
            vec![(
                PreimageKey::new([0x33; 32], PreimageKeyType::Keccak256),
                vec![0x44u8; 32],
                Some((2usize, 3usize)),
            )],
            vec![(
                PreimageKey::new([0x11; 32], PreimageKeyType::Keccak256),
                vec![0x22u8; 32],
                Some((0usize, 1usize)),
            )],
            vec![(
                PreimageKey::new([0x33; 32], PreimageKeyType::Keccak256),
                vec![0x44u8; 32],
                Some((2usize, 3usize)),
            )],
        ]));

        let bytes = to_bytes_with!(PreimageVecStoreRkyv, &original);
        let deserialized = from_bytes_with!(PreimageVecStoreRkyv, PreimageVecStore, &bytes);

        let original_vec = original.lock().unwrap();
        let deserialized_vec = deserialized.lock().unwrap();
        assert_eq!(*original_vec, *deserialized_vec);
    }
}
