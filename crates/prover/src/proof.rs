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

use alloy_primitives::{keccak256, B256};
use anyhow::{bail, Context};
use bytemuck::NoUninit;
use risc0_zkvm::Journal;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub fn proof_file_name<A: NoUninit>(image_id: A, journal: impl Into<Journal>) -> String {
    let version = risc0_zkvm::get_version().unwrap();
    let suffix = if risc0_zkvm::is_dev_mode() {
        "fake"
    } else {
        "zkp"
    };
    format!("risc0-{version}-{}.{suffix}", proof_id(image_id, journal))
}

pub fn proof_id<A: NoUninit>(image_id: A, journal: impl Into<Journal>) -> B256 {
    let image_id = bytemuck::cast::<A, [u8; 32]>(image_id);
    let data = [image_id.as_slice(), journal.into().bytes.as_slice()].concat();
    keccak256(&data)
}

pub async fn read_bincoded_file<T: DeserializeOwned>(file_name: &str) -> anyhow::Result<T> {
    // Read receipt file
    if !Path::new(file_name).exists() {
        bail!("File {file_name} not found.");
    }
    let mut file = File::open(file_name)
        .await
        .context(format!("Failed to open file {file_name}."))?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .await
        .context(format!("Failed to read file {file_name} data until end."))?;
    bincode::deserialize::<T>(&data).context(format!(
        "Failed to deserialize file {file_name} data with bincode."
    ))
}

pub async fn save_to_bincoded_file<T: Serialize>(value: &T, file_name: &str) -> anyhow::Result<()> {
    let mut file = File::create(file_name)
        .await
        .context("Failed to create output file.")?;
    let data = bincode::serialize(value).context("Could not serialize proving data.")?;
    file.write_all(data.as_slice())
        .await
        .context("Failed to write proof to file")?;
    file.flush()
        .await
        .context("Failed to flush proof output file data.")?;
    Ok(())
}
