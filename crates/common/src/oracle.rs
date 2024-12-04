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

use alloy_eips::eip4844::IndexedBlobHash;
use alloy_primitives::keccak256;
use async_trait::async_trait;
use kona_preimage::errors::PreimageOracleResult;
use kona_preimage::{HintWriterClient, PreimageKey, PreimageKeyType, PreimageOracleClient};
use kona_proof::FlushableCache;
use lazy_static::lazy_static;
use op_alloy_protocol::BlockInfo;
use risc0_zkvm::guest::env::{FdReader, FdWriter};
use risc0_zkvm::sha::{Impl as SHA2, Sha256};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::sync::Mutex;

pub fn validate_preimage(key: &PreimageKey, value: &[u8]) {
    let key_type = key.key_type();
    let image = match key_type {
        PreimageKeyType::Keccak256 => keccak256(value).0,
        PreimageKeyType::Sha256 => {
            let x = SHA2::hash_bytes(value);
            x.as_bytes().try_into().unwrap()
        }
        // kzg validation done by blob provider
        _ => return,
    };
    assert_eq!(key, &PreimageKey::new(image, key_type));
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RISCZeroPOSIXOracle;

pub static RISCZERO_POSIX_ORACLE: RISCZeroPOSIXOracle = RISCZeroPOSIXOracle;

pub type OracleReader = Mutex<FdReader>;
pub type OracleWriter = Mutex<FdWriter<fn(&[u8])>>;

lazy_static! {
    pub static ref RISCZERO_POSIX_ORACLE_READER: OracleReader = Mutex::new(FdReader::new(100));
    pub static ref RISCZERO_POSIX_ORACLE_WRITER: OracleWriter =
        Mutex::new(FdWriter::new(101, |_| {}));
    pub static ref RISCZERO_POSIX_HINT_WRITER: OracleWriter =
        Mutex::new(FdWriter::new(102, |_| {}));
}

impl FlushableCache for RISCZeroPOSIXOracle {
    fn flush(&self) {
        /* noop */
    }
}

#[async_trait]
impl PreimageOracleClient for RISCZeroPOSIXOracle {
    async fn get(&self, key: PreimageKey) -> PreimageOracleResult<Vec<u8>> {
        // Provide key
        let key_bytes: [u8; 32] = key.into();
        let _ = RISCZERO_POSIX_ORACLE_WRITER
            .lock()
            .unwrap()
            .write(&key_bytes)
            .expect("Unexpected failure writing preimage key");
        // Acquire reader
        let mut reader = RISCZERO_POSIX_ORACLE_READER.lock().unwrap();
        // Read preimage length
        let mut len_bytes = [0u8; 8];
        reader
            .read_exact(&mut len_bytes)
            .expect("Unexpected failure reading preimage length");
        let len = u64::from_be_bytes(len_bytes) as usize;
        // Read preimage
        let mut response = vec![0u8; len];
        reader
            .read_exact(&mut response)
            .expect("Unexpected failure reading preimage");
        // Verify host response
        validate_preimage(&key, &response);

        Ok(response)
    }

    async fn get_exact(&self, key: PreimageKey, buf: &mut [u8]) -> PreimageOracleResult<()> {
        // Provide key
        let key_bytes: [u8; 32] = key.into();
        let _ = RISCZERO_POSIX_ORACLE_WRITER
            .lock()
            .unwrap()
            .write(&key_bytes)
            .expect("Unexpected failure writing exact preimage key");
        // Acquire reader
        let mut reader = RISCZERO_POSIX_ORACLE_READER.lock().unwrap();
        // Read preimage length
        let mut len_bytes = [0u8; 8];
        reader
            .read_exact(&mut len_bytes)
            .expect("Unexpected failure reading exact preimage length");
        // Read preimage
        reader
            .read_exact(buf)
            .expect("Unexpected failure reading exact preimage");
        // Verify host response
        validate_preimage(&key, buf);

        Ok(())
    }
}

#[async_trait]
impl HintWriterClient for RISCZeroPOSIXOracle {
    async fn write(&self, hint: &str) -> PreimageOracleResult<()> {
        let hint_bytes = hint.as_bytes();
        assert_eq!(
            RISCZERO_POSIX_HINT_WRITER
                .lock()
                .unwrap()
                .write(hint_bytes)
                .unwrap(),
            hint_bytes.len()
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlobFetchRequest {
    pub block_ref: BlockInfo,
    pub blob_hash: IndexedBlobHash,
}
