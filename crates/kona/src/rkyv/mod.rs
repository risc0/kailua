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

pub mod execution;
pub mod kzg;
pub mod optimism;
pub mod primitives;
pub mod vec;

/// A macro for serializing a given value into a `Vec<u8>` (byte vector) while applying a
/// specified wrapper type for the serialization process.
///
/// This macro uses the `rkyv` library's serialization functionality and the `With` wrapper
/// to allow for custom serialization contexts. The macro will immediately `unwrap` the
/// result of the serialization, so it will panic if serialization fails. It is intended
/// for cases where you are confident that serialization will not error in normal usage.
///
/// # Parameters
///
/// * `$with:ty` - The type of the wrapper that will be applied during serialization.
/// * `$value:expr` - The value to be serialized using the specified wrapper.
///
/// # Returns
///
/// A `Vec<u8>` containing the serialized byte representation of the `$value`.
///
/// # Panics
///
/// This macro panics if the underlying serialization process returns an error or if
/// the `unwrap()` call fails. Ensure that serialization cannot fail for the provided value
/// and context.
#[macro_export]
macro_rules! to_bytes_with {
    ($with:ty, $value:expr) => {
        rkyv::to_bytes::<rkyv::rancor::Error>(rkyv::with::With::<_, $with>::cast($value))
            .unwrap()
            .to_vec()
    };
}

/// A macro to deserialize a byte slice into a specific type using a custom `rkyv::with` implementation.
///
/// This macro is particularly useful when you want to deserialize archived data that requires a
/// custom implementation of the `ArchiveWith` and `DeserializeWith` traits. It uses the `rkyv`
/// crate to access a byte slice, and applies the provided wrapper trait for deserialization to the
/// original type.
///
/// # Arguments
///
/// - `$with`: The custom type implementing the `rkyv::with::ArchiveWith` and
///   `rkyv::with::DeserializeWith` traits.
/// - `$orig`: The original type that the input will be deserialized into.
/// - `$bytes`: A reference to the byte slice which contains the archived data to deserialize.
///
/// # Returns
///
/// - The deserialized value of type `$orig`.
///
/// # Panics
///
/// - This macro will panic if:
///   - The byte slice does not contain valid archived data.
///   - Deserialization fails.
#[macro_export]
macro_rules! from_bytes_with {
    ($with:ty, $orig:ty, $bytes:expr) => {{
        let archived = rkyv::access::<
            <$with as rkyv::with::ArchiveWith<$orig>>::Archived,
            rkyv::rancor::Error,
        >($bytes)
        .unwrap();
        rkyv::deserialize::<$orig, rkyv::rancor::Error>(rkyv::with::With::<_, $with>::cast(
            archived,
        ))
        .unwrap()
    }};
}
