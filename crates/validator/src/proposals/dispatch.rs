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

use crate::channel::{DuplexChannel, Message};
use crate::proposals::get_next_l1_head;
use crate::requests::{request_fault_proof, request_validity_proof};
use kailua_contracts::*;
use kailua_sync::agent::SyncAgent;
use kailua_sync::await_tel;
use kailua_sync::stall::Stall;
use opentelemetry::global::tracer;
use opentelemetry::metrics::Counter;
use opentelemetry::trace::FutureExt;
use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::KeyValue;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use std::time::SystemTime;
use tracing::{error, info, warn};

pub async fn dispatch_output_fault_proofs(
    #[cfg(feature = "devnet")] args: &crate::args::ValidateArgs,
    agent: &mut SyncAgent,
    output_fault_buffer: &mut BinaryHeap<(Reverse<u64>, u64)>,
    meter_proofs_requested: &Counter<u64>,
    last_proof_l1_head: &mut BTreeMap<u64, u64>,
    channel: &mut DuplexChannel<Message>,
) {
    let tracer = tracer("kailua");
    let context =
        opentelemetry::Context::current_with_span(tracer.start("dispatch_output_fault_proofs"));

    // dispatch buffered output fault proof requests
    let current_timestamp = current_time();
    let output_fault_proof_requests = output_fault_buffer.len();
    for _ in 0..output_fault_proof_requests {
        let Some((next_time, proposal_index)) = output_fault_buffer.peek() else {
            break;
        };
        if current_timestamp < next_time.0 {
            info!(
                "Waiting {} more seconds before dispatching next fault proving task for proposal {proposal_index}.",
                next_time.0 - current_timestamp
            );
            break;
        }

        let (next_time, proposal_index) = output_fault_buffer.pop().unwrap();
        let retry_time = Reverse(next_time.0 + 10);
        let Some(proposal) = agent.proposals.get(&proposal_index) else {
            if agent.cursor.last_resolved_game < proposal_index {
                error!("Proposal {proposal_index} missing from database.");
                // retry later
                output_fault_buffer.push((retry_time, proposal_index));
            } else {
                warn!("Skipping fault proof request for freed proposal {proposal_index}.");
            };
            continue;
        };
        // Look up parent proposal
        let Some(parent) = agent.proposals.get(&proposal.parent) else {
            if agent.cursor.last_resolved_game < proposal.parent {
                error!(
                    "Proposal {} parent {} missing from database.",
                    proposal.index, proposal.parent
                );
                // retry later
                output_fault_buffer.push((retry_time, proposal_index));
            } else {
                warn!(
                    "Skipping fault proof request for proposal {} with freed parent {}.",
                    proposal.index, proposal.parent
                );
            };
            continue;
        };

        let parent_contract = KailuaTournament::new(parent.contract, &agent.provider.l1_provider);
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
            continue;
        }

        let Some(l1_head) = get_next_l1_head(
            agent,
            last_proof_l1_head,
            proposal,
            #[cfg(feature = "devnet")]
            args.l1_head_jump_back,
        ) else {
            error!("Could not choose an L1 head to fault prove proposal {proposal_index}");
            // retry later
            output_fault_buffer.push((retry_time, proposal_index));
            continue;
        };

        if let Err(err) = await_tel!(
            context,
            request_fault_proof(agent, channel, parent, proposal, l1_head)
        ) {
            error!("Could not request fault proof for {proposal_index}: {err:?}");
            // retry later
            output_fault_buffer.push((retry_time, proposal_index));
        } else {
            meter_proofs_requested.add(
                1,
                &[
                    KeyValue::new("type", "fault"),
                    KeyValue::new("proposal", proposal.contract.to_string()),
                ],
            );
        }
    }
}

pub async fn dispatch_proposal_validity_requests(
    #[cfg(feature = "devnet")] args: &crate::args::ValidateArgs,
    agent: &mut SyncAgent,
    proposal_validity_buffer: &mut BinaryHeap<(Reverse<u64>, u64)>,
    meter_proofs_requested: &Counter<u64>,
    last_proof_l1_head: &mut BTreeMap<u64, u64>,
    channel: &mut DuplexChannel<Message>,
) {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(
        tracer.start("dispatch_proposal_validity_requests"),
    );

    // dispatch buffered validity proof requests
    let current_timestamp = current_time();
    let validity_proof_requests = proposal_validity_buffer.len();
    for _ in 0..validity_proof_requests {
        let Some((next_time, proposal_index)) = proposal_validity_buffer.peek() else {
            break;
        };
        if current_timestamp < next_time.0 {
            info!(
                "Waiting {} more seconds before dispatching next validity proving task for proposal {proposal_index}.",
                next_time.0 - current_timestamp
            );
            break;
        }

        let (next_time, proposal_index) = proposal_validity_buffer.pop().unwrap();
        let retry_time = Reverse(next_time.0 + 10);
        let Some(proposal) = agent.proposals.get(&proposal_index) else {
            if agent.cursor.last_resolved_game < proposal_index {
                error!("Proposal {proposal_index} missing from database.");
                proposal_validity_buffer.push((retry_time, proposal_index));
            } else {
                warn!("Skipping validity proof request for freed proposal {proposal_index}");
            }
            continue;
        };
        // Look up parent proposal
        let Some(parent) = agent.proposals.get(&proposal.parent) else {
            if agent.cursor.last_resolved_game < proposal.parent {
                error!(
                    "Proposal {} parent {} missing from database.",
                    proposal.index, proposal.parent
                );
                proposal_validity_buffer.push((retry_time, proposal_index));
            } else {
                warn!(
                    "Skipping validity proof request for proposal {} with freed parent {}",
                    proposal.index, proposal.parent
                );
            }
            continue;
        };

        let parent_contract = KailuaTournament::new(parent.contract, &agent.provider.l1_provider);
        // Check that a validity proof had not already been posted
        let proof_status = parent_contract
            .proofStatus(proposal.signature)
            .stall_with_context(context.clone(), "KailuaTournament::proofStatus")
            .await;
        if proof_status != 0 {
            info!(
                "Proposal {} signature {} already proven {proof_status}",
                proposal.index, proposal.signature
            );
            continue;
        }

        let Some(l1_head) = get_next_l1_head(
            agent,
            last_proof_l1_head,
            proposal,
            #[cfg(feature = "devnet")]
            args.l1_head_jump_back,
        ) else {
            error!("Could not choose an L1 head to validity prove proposal {proposal_index}");
            proposal_validity_buffer.push((retry_time, proposal_index));
            continue;
        };

        if let Err(err) = await_tel!(
            context,
            request_validity_proof(agent, channel, parent, proposal, l1_head)
        ) {
            error!("Could not request validity proof for {proposal_index}: {err:?}");
            proposal_validity_buffer.push((retry_time, proposal_index));
        } else {
            meter_proofs_requested.add(
                1,
                &[
                    KeyValue::new("type", "validity"),
                    KeyValue::new("proposal", proposal.contract.to_string()),
                ],
            );
        }
    }
}

pub fn current_time() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
