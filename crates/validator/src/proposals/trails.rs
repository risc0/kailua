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

use crate::args::ValidateArgs;
use crate::proposals::dispatch::current_time;
use alloy::primitives::Address;
use alloy::providers::Provider;
use anyhow::Context;
use kailua_contracts::*;
use kailua_sync::agent::SyncAgent;
use kailua_sync::stall::Stall;
use kailua_sync::transact::Transact;
use opentelemetry::global::tracer;
use opentelemetry::metrics::Counter;
use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::KeyValue;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::Duration;
use tracing::{error, info, warn};

#[allow(clippy::too_many_arguments)]
pub async fn publish_trail_proofs<P: Provider>(
    args: &ValidateArgs,
    agent: &mut SyncAgent,
    trail_fault_buffer: &mut BinaryHeap<(Reverse<u64>, u64)>,
    meter_proofs_discarded: &Counter<u64>,
    meter_proofs_published: &Counter<u64>,
    meter_proofs_fail: &Counter<u64>,
    validator_address: Address,
    validator_provider: &P,
) {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("publish_trail_proofs"));

    // publish trail fault proofs
    let current_timestamp = current_time();
    let trail_fault_proof_count = trail_fault_buffer.len();
    for _ in 0..trail_fault_proof_count {
        let Some((next_time, proposal_index)) = trail_fault_buffer.peek() else {
            break;
        };
        if current_timestamp < next_time.0 {
            info!(
                "Waiting {} more seconds before publishing trail fault proof for proposal {proposal_index}.",
                next_time.0 - current_timestamp
            );
            break;
        }

        let (next_time, proposal_index) = trail_fault_buffer.pop().unwrap();
        let retry_time = Reverse(next_time.0 + 10);
        // Fetch proposal from db
        let Some(proposal) = agent.proposals.get(&proposal_index) else {
            if agent.cursor.last_resolved_game < proposal_index {
                error!("Proposal {proposal_index} missing from database.");
                trail_fault_buffer.push((retry_time, proposal_index));
            } else {
                warn!("Skipping trail fault proof submission for freed proposal {proposal_index}.");
            }
            continue;
        };
        let proposal_contract =
            KailuaTournament::new(proposal.contract, &agent.provider.l1_provider);
        // Fetch proposal parent from db
        let Some(parent) = agent.proposals.get(&proposal.parent) else {
            if agent.cursor.last_resolved_game < proposal_index {
                error!("Parent proposal {} missing from database.", proposal.parent);
                trail_fault_buffer.push((retry_time, proposal_index));
            } else {
                warn!(
                    "Skipping trail fault proof submission for proposal {} with freed parent {}.",
                    proposal.index, proposal.parent
                );
            }
            continue;
        };
        let parent_contract = KailuaTournament::new(parent.contract, validator_provider);

        let Some(fault) = proposal.fault() else {
            error!("Attempted trail proof for correct proposal!");
            meter_proofs_discarded.add(
                1,
                &[
                    KeyValue::new("proposal", proposal.contract.to_string()),
                    KeyValue::new("reason", "unfalsifiable"),
                ],
            );
            continue;
        };
        if !fault.is_trail() {
            error!("Attempting trail fault proof for output fault!");
        }
        let divergence_point = fault.divergence_point() as u64;
        let output_fe = proposal.output_fe_at(divergence_point);
        let fe_position = divergence_point - 1;

        if output_fe.is_zero() {
            error!("Proposal fe {output_fe} zeroness as expected.");
        } else {
            warn!("Proposal fe {output_fe} zeroness divergent.");
        }

        // Skip proof submission if already proven
        let fault_proof_status = parent_contract
            .proofStatus(proposal.signature)
            .stall_with_context(context.clone(), "KailuaTournament::proofStatus")
            .await;
        if fault_proof_status != 0 {
            warn!("Skipping proof submission for already proven game at local index {proposal_index}.");
            meter_proofs_discarded.add(
                1,
                &[
                    KeyValue::new("proposal", proposal.contract.to_string()),
                    KeyValue::new("reason", "proven"),
                ],
            );
            continue;
        } else {
            info!("Fault proof status: {fault_proof_status}");
        }

        let blob_commitment = proposal.io_commitment_for(fe_position);
        let kzg_proof = match proposal.io_proof_for(fe_position) {
            Ok(proof) => proof,
            Err(err) => {
                error!("Failed to compute io proof for proposal {proposal_index} at {fe_position}: {err:?}");
                continue;
            }
        };

        // sanity check kzg proof
        {
            // check trail data
            if !proposal_contract
                .verifyIntermediateOutput(
                    fe_position,
                    output_fe,
                    blob_commitment.clone(),
                    kzg_proof.clone(),
                )
                .stall_with_context(context.clone(), "KailuaGame::verifyIntermediateOutput")
                .await
            {
                warn!("Could not verify divergent trail output for proposal");
            } else {
                info!("Proposal divergent trail output confirmed.");
            }
        }

        let child_index = parent
            .child_index(proposal.index)
            .expect("Could not look up proposal's index in parent tournament");

        info!(
            "Submitting trail fault proof to tournament at index {} for child {child_index} with \
                divergence position {divergence_point}.",
            parent.index
        );

        let transaction_dispatch = parent_contract
            .proveTrailFault(
                validator_address,
                [child_index, divergence_point],
                output_fe,
                blob_commitment,
                kzg_proof,
            )
            .timed_transact_with_context(
                context.clone(),
                "KailuaTournament::proveTrailFault",
                Some(Duration::from_secs(args.txn_args.txn_timeout)),
            )
            .await
            .context("KailuaTournament::proveTrailFault");

        match transaction_dispatch {
            Ok(receipt) => {
                info!("Trail fault proof submitted: {receipt:?}");
                let proof_status = parent_contract
                    .proofStatus(proposal.signature)
                    .stall_with_context(context.clone(), "KailuaTournament::proofStatus")
                    .await;
                info!("Proposal {} proven: {proof_status}", proposal.index);
                info!(
                    "KailuaTournament::proveTrailFault: {} gas",
                    receipt.gas_used
                );

                meter_proofs_published.add(
                    1,
                    &[
                        KeyValue::new("type", "fault_trail"),
                        KeyValue::new("proposal", proposal.contract.to_string()),
                        KeyValue::new("l2_height", proposal.output_block_number.to_string()),
                        KeyValue::new("txn_hash", receipt.transaction_hash.to_string()),
                        KeyValue::new("txn_from", receipt.from.to_string()),
                        KeyValue::new("txn_to", receipt.to.unwrap_or_default().to_string()),
                        KeyValue::new("txn_gas_used", receipt.gas_used.to_string()),
                        KeyValue::new("txn_gas_price", receipt.effective_gas_price.to_string()),
                        KeyValue::new(
                            "txn_blob_gas_used",
                            receipt.blob_gas_used.unwrap_or_default().to_string(),
                        ),
                        KeyValue::new(
                            "txn_blob_gas_price",
                            receipt.blob_gas_price.unwrap_or_default().to_string(),
                        ),
                    ],
                );
            }
            Err(e) => {
                error!("Failed to confirm fault proof txn: {e:?}");
                meter_proofs_fail.add(
                    1,
                    &[
                        KeyValue::new("type", "fault_trail"),
                        KeyValue::new("proposal", proposal.contract.to_string()),
                        KeyValue::new("l2_height", proposal.output_block_number.to_string()),
                        KeyValue::new("msg", e.to_string()),
                    ],
                );
                trail_fault_buffer.push((retry_time, proposal_index));
            }
        }
    }
}
