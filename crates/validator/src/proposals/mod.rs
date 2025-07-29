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

#[cfg(feature = "devnet")]
pub mod devnet;
pub mod dispatch;
pub mod processing;
pub mod receipts;
pub mod trails;

use crate::args::ValidateArgs;
use crate::channel::DuplexChannel;
use crate::channel::Message;
use alloy::network::{Ethereum, TxSigner};
use alloy::primitives::B256;
use anyhow::{bail, Context};
use kailua_sync::agent::{SyncAgent, FINAL_L2_BLOCK_RESOLVED};
use kailua_sync::proposal::Proposal;
use kailua_sync::transact::provider::SafeProvider;
use kailua_sync::{await_tel, await_tel_res};
use opentelemetry::global::{meter, tracer};
use opentelemetry::trace::FutureExt;
use opentelemetry::trace::{TraceContextExt, Tracer};
use risc0_zkvm::sha::Digestible;
use risc0_zkvm::InnerReceipt;
use std::collections::{BTreeMap, BinaryHeap, VecDeque};
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{error, info, warn};

pub async fn handle_proposals(
    mut channel: DuplexChannel<Message>,
    args: ValidateArgs,
    data_dir: PathBuf,
) -> anyhow::Result<()> {
    // Telemetry
    let meter = meter("kailua");
    let meter_fault_count = meter.u64_counter("validator.fault.count").build();
    let meter_fault_latest = meter.u64_gauge("validator.fault.latest").build();
    let meter_correct_count = meter.u64_counter("validator.correct.count").build();
    let meter_correct_latest = meter.u64_gauge("validator.correct.latest").build();
    let meter_skipped_count = meter.u64_counter("validator.skipped.count").build();
    let meter_skipped_latest = meter.u64_gauge("validator.skipped.latest").build();
    let meter_proofs_requested = meter.u64_counter("validator.proofs.requested").build();
    let meter_proofs_completed = meter.u64_counter("validator.proofs.complete").build();
    let meter_proofs_published = meter.u64_counter("validator.proofs.published").build();
    let meter_proofs_fail = meter.u64_counter("validator.proofs.errs").build();
    let meter_proofs_discarded = meter.u64_counter("validator.proofs.discarded").build();
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("handle_proposals"));

    // initialize sync agent
    let mut agent = SyncAgent::new(
        &args.sync.provider,
        data_dir,
        args.sync.kailua_game_implementation,
        args.sync.kailua_anchor_address,
        args.proving.bypass_chain_registry,
    )
    .await?;
    info!("KailuaTreasury({:?})", agent.deployment.treasury);

    // initialize validator wallet
    info!("Initializing validator wallet.");
    let validator_wallet = await_tel_res!(
        context,
        tracer,
        "ValidatorSigner::wallet",
        args.validator_signer.wallet(Some(agent.config.l1_chain_id))
    )?;
    let validator_address = validator_wallet.default_signer().address();
    let validator_provider = SafeProvider::new(
        args.txn_args
            .premium_provider::<Ethereum>()
            .wallet(validator_wallet)
            .connect_http(args.sync.provider.eth_rpc_url.as_str().try_into()?),
    );
    info!("Validator address: {validator_address}");

    // Run the validator loop
    info!(
        "Starting from proposal at factory index {}",
        agent.cursor.next_factory_index
    );
    // init channel buffers
    let mut computed_proof_buffer = VecDeque::new();
    let mut output_fault_buffer = BinaryHeap::new();
    let mut trail_fault_buffer = BinaryHeap::new();
    let mut valid_buffer = BinaryHeap::new();
    let mut last_proof_l1_head = BTreeMap::new();
    loop {
        // Wait for new data on every iteration
        sleep(Duration::from_secs(1)).await;
        // fetch latest games
        let loaded_proposals = match await_tel!(
            context,
            agent.sync(
                #[cfg(feature = "devnet")]
                args.sync.delay_l2_blocks,
                args.sync.final_l2_block
            )
        )
        .context("SyncAgent::sync")
        {
            Ok(result) => result,
            Err(err) => {
                if err
                    .root_cause()
                    .to_string()
                    .contains(FINAL_L2_BLOCK_RESOLVED)
                {
                    warn!("handle_proposals terminated");
                    return Ok(());
                }
                error!("Synchronization error: {err:?}");
                vec![]
            }
        };

        // scan newly added proposals
        processing::process_proposals(
            &args,
            &mut agent,
            &loaded_proposals,
            &meter_correct_count,
            &meter_correct_latest,
            &meter_fault_count,
            &meter_fault_latest,
            &meter_skipped_count,
            &meter_skipped_latest,
            &mut valid_buffer,
            &mut output_fault_buffer,
            &mut trail_fault_buffer,
        )
        .await;

        // dispatch buffered output fault proof requests
        dispatch::dispatch_output_fault_proofs(
            #[cfg(feature = "devnet")]
            &args,
            &mut agent,
            &mut output_fault_buffer,
            &meter_proofs_requested,
            &mut last_proof_l1_head,
            &mut channel,
        )
        .await;

        // dispatch buffered validity proof requests
        dispatch::dispatch_proposal_validity_requests(
            #[cfg(feature = "devnet")]
            &args,
            &mut agent,
            &mut valid_buffer,
            &meter_proofs_requested,
            &mut last_proof_l1_head,
            &mut channel,
        )
        .await;

        // publish proofs with receipts on chain
        receipts::publish_receipt_proofs(
            &args,
            &mut agent,
            &mut computed_proof_buffer,
            &mut valid_buffer,
            &mut output_fault_buffer,
            &meter_proofs_completed,
            &meter_proofs_discarded,
            &meter_proofs_published,
            &meter_proofs_fail,
            &mut channel,
            &validator_provider,
        )
        .await;

        // publish trail fault proofs
        trails::publish_trail_proofs(
            &args,
            &mut agent,
            &mut trail_fault_buffer,
            &meter_proofs_discarded,
            &meter_proofs_published,
            &meter_proofs_fail,
            validator_address,
            &validator_provider,
        )
        .await;
    }
}

pub fn get_next_l1_head(
    agent: &SyncAgent,
    last_proof_l1_head: &mut BTreeMap<u64, u64>,
    proposal: &Proposal,
    #[cfg(feature = "devnet")] jump_back: u64,
) -> Option<B256> {
    // fetch next l1 head to use
    let l1_head = match last_proof_l1_head.get(&proposal.index) {
        None => Some(proposal.l1_head),
        Some(last_block_no) => agent
            .l1_heads
            .range((last_block_no + 1)..)
            .next()
            .map(|(_, (_, l1_head))| *l1_head),
    }?;
    // delay if necessary
    #[cfg(feature = "devnet")]
    let l1_head = if last_proof_l1_head.contains_key(&proposal.index) {
        l1_head
    } else {
        let (_, block_no) = *agent.l1_heads_inv.get(&l1_head).unwrap();
        let delayed_l1_head = agent
            .l1_heads
            .range(..block_no)
            .rev()
            .take(jump_back as usize)
            .last()
            .map(|(_, (_, delayed_head))| *delayed_head)
            .unwrap_or(l1_head);
        if delayed_l1_head != l1_head {
            warn!("(DEVNET ONLY) Forced l1 head rollback from {l1_head} to {delayed_l1_head}. Expect a proving error.");
        }
        delayed_l1_head
    };
    // update last head used
    let block_no = agent
        .l1_heads_inv
        .get(&l1_head)
        .expect("Missing l1 head from db")
        .1;
    last_proof_l1_head.insert(proposal.index, block_no);

    Some(l1_head)
}

/// Encode the seal of the given receipt for use with EVM smart contract verifiers.
///
/// Appends the verifier selector, determined from the first 4 bytes of the verifier parameters
/// including the Groth16 verification key and the control IDs that commit to the RISC Zero
/// circuits.
///
/// Copied from crate risc0-ethereum-contracts v2.0.2
pub fn encode_seal(receipt: &risc0_zkvm::Receipt) -> anyhow::Result<Vec<u8>> {
    let seal = match receipt.inner.clone() {
        InnerReceipt::Fake(receipt) => {
            let seal = receipt.claim.digest().as_bytes().to_vec();
            let selector = &[0xFFu8; 4];
            // Create a new vector with the capacity to hold both selector and seal
            let mut selector_seal = Vec::with_capacity(selector.len() + seal.len());
            selector_seal.extend_from_slice(selector);
            selector_seal.extend_from_slice(&seal);
            selector_seal
        }
        InnerReceipt::Groth16(receipt) => {
            let selector = &receipt.verifier_parameters.as_bytes()[..4];
            // Create a new vector with the capacity to hold both selector and seal
            let mut selector_seal = Vec::with_capacity(selector.len() + receipt.seal.len());
            selector_seal.extend_from_slice(selector);
            selector_seal.extend_from_slice(receipt.seal.as_ref());
            selector_seal
        }
        _ => bail!("Unsupported receipt type"),
        // TODO(victor): Add set verifier seal here.
    };
    Ok(seal)
}
