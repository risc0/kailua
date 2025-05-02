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

use crate::eigenda::cert::EigenDAVersionedCert;
use crate::eigenda::commitment::AltDACommitment;
use crate::eigenda::EigenDABlobProvider;
use alloy_primitives::{FixedBytes, U256};
use ark_bn254::{Fq, G1Affine};
use ark_ff::PrimeField;
use async_trait::async_trait;
use eigenda_v2_struct::EigenDAV2Cert;
use kona_preimage::errors::PreimageOracleError;
use kona_proof::errors::OracleProviderError;
use rust_kzg_bn254_primitives::blob::Blob;
use rust_kzg_bn254_verifier::batch::verify_blob_kzg_proof_batch;
use serde::{Deserialize, Serialize};
use tracing::info;

/// One EigenDABlobWitnessData corresponds to one EigenDA cert
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct EigenDABlobWitnessData {
    /// eigenda v2 cert
    pub eigenda_certs: Vec<EigenDAV2Cert>,
    /// blob empty if cert is invalid
    /// ToDo make Blob Serializable
    pub eigenda_blobs: Vec<Vec<u8>>,
    /// kzg proof on Fiat Shamir points
    pub kzg_proofs: Vec<FixedBytes<64>>,
    /// indicates the validity of a cert is either true or false
    /// validity contains a zk proof attesting claimed
    /// validity
    // todo: STEEL
    pub validity: Vec<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct PreloadedEigenDABlobProvider {
    /// The tuple contains EigenDAV2Cert, Blob, isValid cert.
    pub entries: Vec<(EigenDAV2Cert, Blob)>,
}

impl From<EigenDABlobWitnessData> for PreloadedEigenDABlobProvider {
    fn from(value: EigenDABlobWitnessData) -> Self {
        let mut blobs = vec![];
        let mut proofs = vec![];
        let mut commitments = vec![];

        let mut entries = vec![];

        for i in 0..value.eigenda_blobs.len() {
            // todo verify validity of the cert
            // #[cfg(feature = "eigenda-view-proof")]
            // value.validity[i].validate_cert_receipt(
            //     &value.eigenda_certs[i],
            //     // TODO figure out a way to pass down validity_call_verifier_id
            //     B256::default(),
            // );

            // if valid, check blob kzg integrity
            if value.validity[i] {
                blobs.push(Blob::new(&value.eigenda_blobs[i]));
                proofs.push(value.kzg_proofs[i]);
                let commitment = value.eigenda_certs[i]
                    .blob_inclusion_info
                    .blob_certificate
                    .blob_header
                    .commitment
                    .commitment;
                commitments.push((commitment.x, commitment.y));
            } else {
                // check (P2) if cert is not valid, the blob is only allowed to be empty
                assert!(value.eigenda_blobs[i].is_empty());
            }
            entries.push((
                value.eigenda_certs[i].clone(),
                Blob::new(&value.eigenda_blobs[i]),
            ));
        }

        // for ease of when using
        entries.reverse();

        // check (P1) if cert is not valie, the blob must be empty, assert that commitments in the cert and blobs are consistent
        assert!(batch_verify(blobs, commitments, proofs));

        PreloadedEigenDABlobProvider { entries }
    }
}

/// Eventually, rust-kzg-bn254 would provide a nice interface that takes
/// bytes input, so that we can remove this wrapper. For now, just include it here
pub fn batch_verify(
    eigenda_blobs: Vec<Blob>,
    commitments: Vec<(U256, U256)>,
    proofs: Vec<FixedBytes<64>>,
) -> bool {
    info!("lib_blobs len {:?}", eigenda_blobs.len());
    // transform to rust-kzg-bn254 inputs types
    // TODO should make library do the parsing the return result
    let lib_blobs: Vec<Blob> = eigenda_blobs;
    let lib_commitments: Vec<G1Affine> = commitments
        .iter()
        .map(|c| {
            let a: [u8; 32] = c.0.to_be_bytes();
            let b: [u8; 32] = c.1.to_be_bytes();
            let x = Fq::from_be_bytes_mod_order(&a);
            let y = Fq::from_be_bytes_mod_order(&b);
            G1Affine::new(x, y)
        })
        .collect();
    let lib_proofs: Vec<G1Affine> = proofs
        .iter()
        .map(|p| {
            let x = Fq::from_be_bytes_mod_order(&p[..32]);
            let y = Fq::from_be_bytes_mod_order(&p[32..64]);

            G1Affine::new(x, y)
        })
        .collect();

    // convert all the error to false
    verify_blob_kzg_proof_batch(&lib_blobs, &lib_commitments, &lib_proofs).unwrap_or(false)
}

#[async_trait]
impl EigenDABlobProvider for PreloadedEigenDABlobProvider {
    // TODO investigate if create a special error type EigenDABlobProviderError
    type Error = OracleProviderError;

    /// Fetches a blob for V2 using preloaded data
    /// Return an error if cert does not match the immediate next item
    async fn get_blob(&mut self, altda_commitment: &AltDACommitment) -> Result<Blob, Self::Error> {
        let (eigenda_cert, eigenda_blob) = self.entries.pop().unwrap();
        let is_match = match &altda_commitment.versioned_cert {
            // secure integration is not implemented for v1, but feel free to contribute
            EigenDAVersionedCert::V1(_c) => unimplemented!(),
            EigenDAVersionedCert::V2(c) => {
                info!("request cert is {:?}", c.digest());
                info!("stored  cert is {:?}", eigenda_cert.digest());
                c == &eigenda_cert
            }
        };

        if is_match {
            Ok(eigenda_blob)
        } else {
            Err(OracleProviderError::Preimage(PreimageOracleError::Other(
                "does not contain header".into(),
            )))
        }
    }
}
