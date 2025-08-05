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

use risc0_zkvm::{Digest, InnerReceipt, MaybePruned, Receipt};
use tracing::warn;

#[allow(deprecated)]
pub fn maybe_patch_proof(
    mut receipt: Receipt,
    expected_fpvm_image_id: [u8; 32],
) -> anyhow::Result<Receipt> {
    // Return the proof if we can't patch it
    if !risc0_zkvm::is_dev_mode() {
        return Ok(receipt);
    }

    let expected_fpvm_image_id = Digest::from(expected_fpvm_image_id);

    // Patch the image id of the receipt to match the expected one
    if let InnerReceipt::Fake(fake_inner_receipt) = &mut receipt.inner {
        if let MaybePruned::Value(claim) = &mut fake_inner_receipt.claim {
            warn!("DEV-MODE ONLY: Patching fake receipt image id to match game contract.");
            claim.pre = MaybePruned::Pruned(expected_fpvm_image_id);
            if let MaybePruned::Value(Some(output)) = &mut claim.output {
                if let MaybePruned::Value(journal) = &mut output.journal {
                    let n = journal.len();
                    journal[n - 32..n].copy_from_slice(expected_fpvm_image_id.as_bytes());
                    receipt.journal.bytes[n - 32..n]
                        .copy_from_slice(expected_fpvm_image_id.as_bytes());
                }
            }
        }
    }
    Ok(receipt)
}
