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

use alloy_primitives::B256;
use eigenda_cert::AltDACommitment;
use hokulea_proof::canoe_verifier::errors::HokuleaCanoeVerificationError;
use hokulea_proof::canoe_verifier::{to_journal_bytes, CanoeVerifier};
use hokulea_proof::cert_validity::CertValidity;
use risc0_zkvm::sha::Digestible;
use risc0_zkvm::Receipt;

#[derive(Copy, Clone, Debug)]
pub struct KailuaCanoeVerifier(pub [u8; 32]);

impl CanoeVerifier for KailuaCanoeVerifier {
    fn validate_cert_receipt(
        &self,
        cert_validity: CertValidity,
        eigenda_cert: AltDACommitment,
    ) -> Result<(), HokuleaCanoeVerificationError> {
        let journal_bytes = to_journal_bytes(&cert_validity, &eigenda_cert);

        let Some(proof) = cert_validity.canoe_proof else {
            crate::client::log(&format!("ASSUME {} (EIGEN)", journal_bytes.digest()));
            #[cfg(target_os = "zkvm")]
            return risc0_zkvm::guest::env::verify(self.0, &journal_bytes)
                .map_err(|e| HokuleaCanoeVerificationError::InvalidProofAndJournal(e.to_string()));
            #[cfg(not(target_os = "zkvm"))]
            return Err(HokuleaCanoeVerificationError::MissingProof);
        };

        // todo: avoid serde_json
        // todo: load seal only
        let receipt: Receipt = serde_json::from_slice(proof.as_ref()).map_err(|e| {
            HokuleaCanoeVerificationError::UnableToDeserializeReceipt(e.to_string())
        })?;

        receipt
            .verify(self.0)
            .map_err(|e| HokuleaCanoeVerificationError::InvalidProofAndJournal(e.to_string()))?;

        if receipt.journal.bytes != journal_bytes {
            return Err(HokuleaCanoeVerificationError::InconsistentPublicJournal);
        }

        Ok(())
    }
}

pub fn da_witness_precondition(
    eigen_da: &hokulea_proof::eigenda_blob_witness::EigenDABlobWitnessData,
) -> Option<(B256, u64, u64)> {
    // Enforce sufficient data requirements
    assert!(eigen_da.recency.len() >= eigen_da.validity.len());
    assert!(eigen_da.validity.len() >= eigen_da.blob.len());
    // Enforce L1 chain consistency
    let (l1_head, l1_chain_id) = eigen_da
        .validity
        .first()
        .map(|(_, cert)| (cert.l1_head_block_hash, cert.l1_chain_id))?;
    for (_, cert) in &eigen_da.validity {
        assert_eq!(l1_head, cert.l1_head_block_hash);
        assert_eq!(l1_chain_id, cert.l1_chain_id);
    }
    // Enforce L2 configuration consistency
    let recency = eigen_da.recency.first().map(|(_, r)| *r)?;
    for (_, r) in &eigen_da.recency {
        assert_eq!(recency, *r);
    }
    Some((l1_head, l1_chain_id, recency))
}

pub fn da_witness_postcondition(
    precondition: Option<(B256, u64, u64)>,
    boot_info: &kona_proof::BootInfo,
) {
    if let Some((l1_head, l1_chain_id, recency)) = precondition {
        assert_eq!(l1_head, boot_info.l1_head);
        assert_eq!(l1_chain_id, boot_info.rollup_config.l1_chain_id);
        // ToDo (bx) fix the hack at eigenda-proxy. For now + 100_000_000 to avoid recency failure
        assert_eq!(
            recency,
            boot_info.rollup_config.seq_window_size + 100_000_000
        )
    }
}
