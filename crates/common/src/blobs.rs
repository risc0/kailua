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

extern crate alloc;

use crate::oracle::BlobFetchRequest;
use alloy_eips::eip4844::{
    kzg_to_versioned_hash, Blob, BYTES_PER_BLOB, BYTES_PER_COMMITMENT, BYTES_PER_PROOF,
};
use async_trait::async_trait;
use c_kzg::{Bytes48, KzgSettings};
use kona_derive::errors::BlobProviderError;
use kona_derive::prelude::IndexedBlobHash;
use kona_derive::traits::BlobProvider;
use lazy_static::lazy_static;
use op_alloy_protocol::BlockInfo;
use risc0_zkvm::guest::env::{FdReader, FdWriter};
use std::io::{Read, Write};
use std::sync::Mutex;

#[cfg(target_os = "zkvm")]
#[c_kzg::risc0_c_kzg_alloc_mod]
pub mod c_kzg_alloc {
    // proc macro inserts calloc/malloc/free definitions here

    #[no_mangle]
    pub extern "C" fn __assert_func(
        _file: *const i8,
        _line: i32,
        _func: *const i8,
        _expr: *const i8,
    ) {
        panic!("c_kzg assertion failure.");
    }
}

// todo: hardcode without serde in guest image
#[cfg(target_os = "zkvm")]
lazy_static! {
    /// KZG Ceremony data
    pub static ref KZG: (Vec<u8>, KzgSettings) = {
        let mut data = Vec::from(include_bytes!("../kzg_settings_raw.bin"));
        let settings = KzgSettings::from_u8_slice(&mut data);
        (data, settings)
    };
}

#[cfg(not(target_os = "zkvm"))]
lazy_static! {
    pub static ref KZG: alloy_eips::eip4844::env_settings::EnvKzgSettings = Default::default();
}

pub fn kzg_settings() -> &'static KzgSettings {
    #[cfg(target_os = "zkvm")]
    return &KZG.1;

    #[cfg(not(target_os = "zkvm"))]
    return KZG.get();
}

#[derive(Debug, Clone, Default, Copy)]
pub struct RISCZeroPOSIXBlobProvider;

pub static RISCZERO_POSIX_BLOB_PROVIDER: RISCZeroPOSIXBlobProvider = RISCZeroPOSIXBlobProvider;

lazy_static! {
    pub static ref RISCZERO_POSIX_BLOB_PROVIDER_READER: Mutex<FdReader> =
        Mutex::new(FdReader::new(104));
    pub static ref RISCZERO_POSIX_BLOB_PROVIDER_WRITER: Mutex<FdWriter<fn(&[u8])>> =
        Mutex::new(FdWriter::new(105, |_| {}));
}

#[async_trait]
impl BlobProvider for RISCZeroPOSIXBlobProvider {
    type Error = BlobProviderError;

    async fn get_blobs(
        &mut self,
        block_ref: &BlockInfo,
        blob_hashes: &[IndexedBlobHash],
    ) -> Result<Vec<Box<Blob>>, BlobProviderError> {
        risc0_zkvm::guest::env::log(&format!(
            "Fetching {} blob hashes for L1 block {} ({}).",
            blob_hashes.len(),
            block_ref.number,
            block_ref.hash
        ));
        let mut blobs = Vec::with_capacity(blob_hashes.len());
        let mut commitments = Vec::with_capacity(blob_hashes.len());
        let mut proofs = Vec::with_capacity(blob_hashes.len());
        for blob_hash in blob_hashes {
            let request = bincode::serialize(&BlobFetchRequest {
                block_ref: block_ref.clone(),
                blob_hash: blob_hash.clone(),
            })
            .expect("Failed to serialize blob request.");
            // Write the request to the host
            RISCZERO_POSIX_BLOB_PROVIDER_WRITER
                .lock()
                .unwrap()
                .write(&request)
                .expect("Unexpected failure writing blob request.");
            // Acquire reader
            let mut reader = RISCZERO_POSIX_BLOB_PROVIDER_READER.lock().unwrap();
            // Read the blob
            let mut blob = [0u8; BYTES_PER_BLOB];
            reader
                .read_exact(&mut blob)
                .expect("Unexpected failure reading blob data");
            blobs.push(c_kzg::Blob::new(blob));
            // Read the blob commitment
            let mut commitment = [0u8; BYTES_PER_COMMITMENT];
            reader
                .read_exact(&mut commitment)
                .expect("Unexpected failure reading blob commitment");
            commitments.push(Bytes48::new(commitment));
            // Read the blob proof
            let mut proof = [0u8; BYTES_PER_PROOF];
            reader
                .read_exact(&mut proof)
                .expect("Unexpected failure reading blob proof");
            proofs.push(Bytes48::new(proof));
        }
        risc0_zkvm::guest::env::log(&format!("Loaded {} blobs from oracle.", blobs.len()));
        if blob_hashes.len() != blobs.len() {
            return Err(BlobProviderError::SidecarLengthMismatch(
                blob_hashes.len(),
                blobs.len(),
            ));
        }
        // verify commitments
        // todo: amortize over entire client session
        c_kzg::KzgProof::verify_blob_kzg_proof_batch(
            blobs.as_slice(),
            commitments.as_slice(),
            proofs.as_slice(),
            kzg_settings(),
        )
        .expect("Failed to batch validate kzg proofs");
        risc0_zkvm::guest::env::log("Blob commitments validated.");
        // Validate commitment hashes
        for (commitment, blob_hash) in core::iter::zip(&commitments, blob_hashes) {
            let versioned_hash = kzg_to_versioned_hash(commitment.as_slice());
            assert_eq!(versioned_hash, blob_hash.hash);
        }
        risc0_zkvm::guest::env::log("Blob hashes validated.");
        Ok(blobs
            .into_iter()
            .map(|b| Box::new(Blob::from(*b)))
            .collect())
    }
}
