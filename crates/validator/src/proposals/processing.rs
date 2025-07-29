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
use kailua_contracts::*;
use kailua_sync::agent::SyncAgent;
use kailua_sync::await_tel;
use kailua_sync::stall::Stall;
use opentelemetry::global::tracer;
use opentelemetry::metrics::{Counter, Gauge};
use opentelemetry::trace::FutureExt;
use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::KeyValue;
use rand::Rng;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{error, info, warn};

#[allow(clippy::too_many_arguments)]
pub async fn process_proposals(
    args: &ValidateArgs,
    agent: &mut SyncAgent,
    loaded_proposals: &[u64],
    meter_correct_count: &Counter<u64>,
    meter_correct_latest: &Gauge<u64>,
    meter_fault_count: &Counter<u64>,
    meter_fault_latest: &Gauge<u64>,
    meter_skipped_count: &Counter<u64>,
    meter_skipped_latest: &Gauge<u64>,
    proposal_validity_buffer: &mut BinaryHeap<(Reverse<u64>, u64)>,
    output_fault_buffer: &mut BinaryHeap<(Reverse<u64>, u64)>,
    trail_fault_buffer: &mut BinaryHeap<(Reverse<u64>, u64)>,
) {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("process_proposals"));

    // check new proposals for fault and queue potential responses
    for proposal_index in loaded_proposals {
        let Some(proposal) = agent.proposals.get(proposal_index) else {
            error!("Proposal {proposal_index} missing from database.");
            continue;
        };
        // Skip Treasury instance
        if !proposal.has_parent() {
            info!("Skipping proving for treasury instance.");
            continue;
        }
        // Skip resolved games
        if proposal.resolved_at != 0 {
            info!("Skipping proving for resolved game.");
            continue;
        }
        // Telemetry
        if proposal.is_correct().unwrap_or_default() {
            meter_correct_count.add(
                1,
                &[
                    KeyValue::new("proposal", proposal.contract.to_string()),
                    KeyValue::new("l2_height", proposal.output_block_number.to_string()),
                ],
            );
            meter_correct_latest.record(
                proposal.index,
                &[
                    KeyValue::new("proposal", proposal.contract.to_string()),
                    KeyValue::new("l2_height", proposal.output_block_number.to_string()),
                ],
            );
        } else {
            meter_fault_count.add(
                1,
                &[
                    KeyValue::new("proposal", proposal.contract.to_string()),
                    KeyValue::new("l2_height", proposal.output_block_number.to_string()),
                ],
            );
            meter_fault_latest.record(
                proposal.index,
                &[
                    KeyValue::new("proposal", proposal.contract.to_string()),
                    KeyValue::new("l2_height", proposal.output_block_number.to_string()),
                ],
            );
        }
        // Look up parent proposal
        let Some(parent) = agent.proposals.get(&proposal.parent) else {
            error!(
                "Proposal {} parent {} missing from database.",
                proposal.index, proposal.parent
            );
            continue;
        };
        let parent_contract = KailuaTournament::new(parent.contract, &agent.provider.l1_provider);
        // Check termination condition
        if let Some(final_l2_block) = args.sync.final_l2_block {
            if parent.output_block_number >= final_l2_block {
                warn!(
                    "Dropping proposal {} with parent output height {} past final l2 block {}.",
                    proposal.index, parent.output_block_number, final_l2_block
                );
                continue;
            }
        }
        // Check that a validity proof has not already been posted
        let is_validity_proven = await_tel!(
            context,
            parent.fetch_is_successor_validity_proven(&agent.provider.l1_provider)
        );
        if is_validity_proven {
            info!(
                "Validity proof settling all disputes in tournament {} already submitted",
                parent.index
            );
            meter_skipped_count.add(
                1,
                &[
                    KeyValue::new("proposal", proposal.contract.to_string()),
                    KeyValue::new("tournament", parent.contract.to_string()),
                    KeyValue::new("reason", "parent_successor_proven"),
                ],
            );
            meter_skipped_latest.record(
                proposal.index,
                &[
                    KeyValue::new("proposal", proposal.contract.to_string()),
                    KeyValue::new("tournament", parent.contract.to_string()),
                    KeyValue::new("reason", "parent_successor_proven"),
                ],
            );
            continue;
        }
        // fetch canonical status of proposal
        let Some(is_proposal_canonical) = proposal.canonical else {
            error!("Canonical status of proposal {proposal_index} unknown");
            continue;
        };
        // utilize validity proofs for proposals of height within the ff range
        if (args.fast_forward_start..=args.fast_forward_target)
            .contains(&proposal.output_block_number)
        {
            // prove the validity of this proposal if it is canon
            if is_proposal_canonical {
                // Randomize proving wait
                let random_wait = random_processing_time(args.max_validity_proving_delay);
                // Prove full validity
                proposal_validity_buffer.push((random_wait, *proposal_index));
                continue;
            }
            // skip fault proving if a validity proof is en-route
            if let Some(successor) = parent.successor {
                info!(
                    "Skipping proving for proposal {proposal_index} assuming ongoing \
                        validity proof generation for proposal {successor}."
                );
                continue;
            }
        }

        // Switch to validity proving if only one output is admissible
        if agent.deployment.proposal_output_count == 1 {
            // Check if there is a faulty predecessor
            let is_prior_fault = parent
                .children
                .iter()
                .filter(|p| *p < proposal_index)
                .any(|p| {
                    // Fetch predecessor from db
                    let Some(predecessor) = agent.proposals.get(p) else {
                        error!("Proposal {p} missing from database.");
                        return false;
                    };
                    if agent.was_proposer_eliminated_before(predecessor) {
                        return false;
                    }
                    if predecessor.is_correct().unwrap_or_default() {
                        return false;
                    }
                    info!("Found invalid predecessor proposal {p}");
                    true
                });
            // Check canonical proposal status
            match parent.successor {
                Some(p) if p == proposal.index && is_prior_fault => {
                    // Compute validity proof on arrival of correct proposal after faulty proposal
                    info!(
                            "Computing validity proof for {proposal_index} to discard invalid predecessors."
                        );
                    let random_wait = random_processing_time(args.max_fault_proving_delay);
                    proposal_validity_buffer.push((random_wait, p));
                }
                Some(p) if p == proposal.index => {
                    // Skip proving as no conflicts exist
                    info!("Skipping proving for proposal {proposal_index} with no invalid predecessors.");
                }
                Some(p) if proposal.is_correct() == Some(false) && !is_prior_fault => {
                    // Compute validity proof on arrival of faulty proposal after correct proposal
                    info!("Computing validity proof for {p} to discard invalid successor.");
                    let random_wait = random_processing_time(args.max_fault_proving_delay);
                    proposal_validity_buffer.push((random_wait, p));
                }
                Some(p) if proposal.is_correct() == Some(false) => {
                    // is_prior_fault is true and a successor exists, so some proof must be queued
                    info!(
                            "Skipping proving for proposal {proposal_index} assuming ongoing validity proof for proposal {p}."
                        );
                }
                Some(p) => {
                    info!(
                        "Skipping proving for correct proposal {proposal_index} replicating {p}."
                    );
                }
                None => {
                    info!(
                            "Skipping fault proving for proposal {proposal_index} with no valid sibling."
                        );
                }
            }
            continue;
        }

        // Skip proving on repeat signature
        let is_repeat_signature = parent
            .children
            .iter()
            .filter(|p| *p < proposal_index)
            .any(|p| {
                // Fetch predecessor from db
                let Some(predecessor) = agent.proposals.get(p) else {
                    error!("Proposal {p} missing from database.");
                    return false;
                };
                if agent.was_proposer_eliminated_before(predecessor) {
                    return false;
                }
                if predecessor.signature != proposal.signature {
                    return false;
                }
                info!("Found duplicate predecessor proposal {p}");
                true
            });
        if is_repeat_signature {
            info!(
                "Skipping fault proving for proposal {proposal_index} with repeat signature {}",
                proposal.signature
            );
            continue;
        }

        // Skip attempting to fault prove correct proposals
        if let Some(true) = proposal.is_correct() {
            info!(
                "Skipping fault proving for proposal {proposal_index} with valid signature {}",
                proposal.signature
            );
            continue;
        }

        // Check that a fault proof had not already been posted
        let proof_status = parent_contract
            .proofStatus(proposal.signature)
            .stall_with_context(context.clone(), "KailuaTournament::proofStatus")
            .await;
        if proof_status != 0 {
            info!(
                "Proposal {} signature {} already proven {proof_status}",
                proposal.index, proposal.signature
            );
            meter_skipped_count.add(
                1,
                &[
                    KeyValue::new("proposal", proposal.contract.to_string()),
                    KeyValue::new("tournament", parent.contract.to_string()),
                    KeyValue::new("reason", "proof_status"),
                ],
            );
            meter_skipped_latest.record(
                proposal.index,
                &[
                    KeyValue::new("proposal", proposal.contract.to_string()),
                    KeyValue::new("tournament", parent.contract.to_string()),
                    KeyValue::new("reason", "proof_status"),
                ],
            );
            continue;
        }

        // Get divergence point
        let Some(fault) = proposal.fault() else {
            error!("Attempted to request fault proof for correct proposal {proposal_index}");
            continue;
        };
        // Randomize proving wait
        let random_wait = random_processing_time(args.max_fault_proving_delay);
        // Queue fault proof
        if fault.is_output() {
            // Queue output fault proof request
            output_fault_buffer.push((random_wait, *proposal_index));
        } else {
            // Queue trail fault proof submission
            trail_fault_buffer.push((random_wait, *proposal_index));
        }
    }
}

pub fn random_processing_time(max_seconds: u64) -> Reverse<u64> {
    let random_wait = rand::rng().random_range(0..=max_seconds);
    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    Reverse(current_time + random_wait)
}
