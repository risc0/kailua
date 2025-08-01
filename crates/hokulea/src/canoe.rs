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
            kailua_kona::client::log(&format!("ASSUME {} (EIGEN)", journal_bytes.digest()));
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
