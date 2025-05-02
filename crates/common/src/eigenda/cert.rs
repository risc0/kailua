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
//! This module contains code vendored in from https://github.com/Layr-Labs/hokulea

use crate::eigenda::commitment::AltDACommitmentParseError;
use alloy_primitives::Bytes;
use alloy_rlp::{RlpDecodable, RlpEncodable};
use eigenda_v2_struct::EigenDAV2Cert;

#[derive(Debug, PartialEq, Copy, Clone)]
/// Represents the cert version derived from rollup inbox
/// The version is needed to decode the Cert from serialiezd bytes
/// Once a valid blob is retrieved, both versions use the identical
/// logic to derive the rollup channel frame from eigenda blobs
pub enum CertVersion {
    /// eigenda cert v1 version
    Version1 = 0,
    /// eigenda cert v2 version
    Version2,
}

impl TryFrom<u8> for CertVersion {
    type Error = AltDACommitmentParseError;
    fn try_from(value: u8) -> Result<CertVersion, Self::Error> {
        match value {
            0 => Ok(Self::Version1),
            1 => Ok(Self::Version2),
            _ => Err(AltDACommitmentParseError::UnsupportedCertVersionType),
        }
    }
}

impl From<CertVersion> for u8 {
    fn from(version: CertVersion) -> Self {
        version as u8
    }
}

/// EigenDACert can be either v1 or v2
/// TODO consider boxing them, since the variant has large size
#[allow(clippy::large_enum_variant)]
#[derive(Debug, PartialEq, Clone)]
pub enum EigenDAVersionedCert {
    /// V1
    V1(BlobInfo),
    /// V2
    V2(EigenDAV2Cert),
}

/// eigenda v1 certificate
#[derive(Debug, PartialEq, Clone, RlpEncodable, RlpDecodable)]
pub struct BlobInfo {
    /// v1 blob header
    pub blob_header: BlobHeader,
    /// v1 blob verification proof with merkle tree
    pub blob_verification_proof: BlobVerificationProof,
}

/// eigenda v1 blob header
#[derive(Debug, PartialEq, Clone, RlpEncodable, RlpDecodable)]
pub struct BlobHeader {
    pub commitment: G1Commitment,
    pub data_length: u32,
    pub blob_quorum_params: Vec<BlobQuorumParam>,
}

/// eigenda v1 blob verification proof
#[derive(Debug, PartialEq, Clone, RlpEncodable, RlpDecodable)]
pub struct BlobVerificationProof {
    pub batch_id: u32,
    pub blob_index: u32,
    pub batch_medatada: BatchMetadata,
    pub inclusion_proof: Bytes,
    pub quorum_indexes: Bytes,
}

#[derive(Debug, PartialEq, Clone, RlpEncodable, RlpDecodable)]
pub struct G1Commitment {
    pub x: [u8; 32],
    pub y: [u8; 32],
}

#[derive(Debug, PartialEq, Clone, RlpEncodable, RlpDecodable)]
pub struct BlobQuorumParam {
    pub quorum_number: u32,
    pub adversary_threshold_percentage: u32,
    pub confirmation_threshold_percentage: u32,
    pub chunk_length: u32,
}

#[derive(Debug, PartialEq, Clone, RlpEncodable, RlpDecodable)]
pub struct BatchMetadata {
    pub batch_header: BatchHeader,
    pub signatory_record_hash: Bytes,
    pub fee: Bytes,
    pub confirmation_block_number: u32,
    pub batch_header_hash: Bytes,
}

#[derive(Debug, PartialEq, Clone, RlpEncodable, RlpDecodable)]
pub struct BatchHeader {
    pub batch_root: Bytes,
    pub quorum_numbers: Bytes,
    pub quorum_signed_percentages: Bytes,
    pub reference_block_number: u32,
}
