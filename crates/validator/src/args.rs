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

use alloy::primitives::{Address, B256};
use kailua_common::precondition::PreconditionValidationData;
use kailua_prover::backends::boundless::BoundlessArgs;
use kailua_sync::args::{parse_address, SyncArgs};
use kailua_sync::telemetry::TelemetryArgs;
use kailua_sync::transact::signer::ValidatorSignerArgs;
use kailua_sync::transact::TransactArgs;
use std::path::PathBuf;

#[derive(clap::Args, Debug, Clone)]
pub struct ValidateArgs {
    #[clap(flatten)]
    pub sync: SyncArgs,

    /// Path to the prover binary to use for proving
    #[clap(long, env)]
    pub kailua_cli: Option<PathBuf>,
    /// Fast-forward block height
    #[clap(long, env, required = false, default_value_t = 0)]
    pub fast_forward_target: u64,
    /// How many proofs to compute simultaneously
    #[clap(long, env, default_value_t = 1)]
    pub num_concurrent_provers: u64,
    /// The number of l1 heads to jump back when initially proving
    #[cfg(feature = "devnet")]
    #[clap(long, env, default_value_t = 0)]
    pub l1_head_jump_back: u64,

    /// Secret key of L1 wallet to use for challenging and proving outputs
    #[clap(flatten)]
    pub validator_signer: ValidatorSignerArgs,
    /// Transaction publication configuration
    #[clap(flatten)]
    pub txn_args: TransactArgs,
    /// Address of the recipient account to use for bond payouts
    #[clap(long, env, value_parser = parse_address)]
    pub payout_recipient_address: Option<Address>,
    /// Address of the KailuaGame implementation to use
    #[clap(long, env, value_parser = parse_address)]
    pub kailua_game_implementation: Option<Address>,
    /// Address of the anchor proposal to start synchronization from
    #[clap(long, env, value_parser = parse_address)]
    pub kailua_anchor_address: Option<Address>,

    #[clap(flatten)]
    pub boundless: BoundlessArgs,

    #[clap(flatten)]
    pub telemetry: TelemetryArgs,
}

#[allow(clippy::too_many_arguments)]
pub fn create_proving_args(
    args: &ValidateArgs,
    verbosity: u8,
    data_dir: PathBuf,
    l2_chain_id: String,
    payout_recipient: Address,
    precondition_validation_data: Option<PreconditionValidationData>,
    l1_head: B256,
    agreed_l2_head_hash: B256,
    agreed_l2_output_root: B256,
    claimed_l2_block_number: u64,
    claimed_l2_output_root: B256,
) -> Vec<String> {
    // Prepare prover parameters
    let mut proving_args = vec![
        // Invoke the CLI prove command
        String::from("prove"),
        // wallet address for payouts
        String::from("--payout-recipient-address"),
        payout_recipient.to_string(),
        // l2 el node
        String::from("--op-node-address"),
        args.sync.provider.op_node_url.clone(),
    ];
    // precondition data
    if let Some(precondition_data) = precondition_validation_data {
        let (block_hashes, blob_hashes): (Vec<_>, Vec<_>) = precondition_data
            .blob_fetch_requests()
            .iter()
            .map(|r| (r.block_ref.hash.to_string(), r.blob_hash.hash.to_string()))
            .unzip();
        let params = match precondition_data {
            PreconditionValidationData::Validity {
                proposal_l2_head_number,
                proposal_output_count,
                output_block_span,
                blob_hashes: _,
            } => vec![
                proposal_l2_head_number,
                proposal_output_count,
                output_block_span,
            ],
        }
        .into_iter()
        .map(|p| p.to_string())
        .collect::<Vec<_>>();

        proving_args.extend(vec![
            String::from("--precondition-params"),
            params.join(","),
            String::from("--precondition-block-hashes"),
            block_hashes.join(","),
            String::from("--precondition-blob-hashes"),
            blob_hashes.join(","),
        ]);
    }
    // boundless args
    if let Some(market) = &args.boundless.market {
        proving_args.extend(market.to_arg_vec(&args.boundless.storage));
    }
    // data directory
    let data_dir = data_dir.join(format!(
        "{}-{}",
        &agreed_l2_output_root.to_string()[..10].to_string(),
        &claimed_l2_output_root.to_string()[..10].to_string()
    ));
    // kona args
    proving_args.extend(vec![
        // l1 head from on-chain proposal
        String::from("--l1-head"),
        l1_head.to_string(),
        // l2 starting block hash from on-chain proposal
        String::from("--agreed-l2-head-hash"),
        agreed_l2_head_hash.to_string(),
        // l2 starting output root
        String::from("--agreed-l2-output-root"),
        agreed_l2_output_root.to_string(),
        // proposed output root
        String::from("--claimed-l2-output-root"),
        claimed_l2_output_root.to_string(),
        // proposed block number
        String::from("--claimed-l2-block-number"),
        claimed_l2_block_number.to_string(),
        // rollup chain id
        String::from("--l2-chain-id"),
        l2_chain_id.clone(),
        // l1 el node
        String::from("--l1-node-address"),
        args.sync.provider.eth_rpc_url.clone(),
        // l1 cl node
        String::from("--l1-beacon-address"),
        args.sync.provider.beacon_rpc_url.clone(),
        // l2 el node
        String::from("--l2-node-address"),
        args.sync.provider.op_geth_url.clone(),
        // path to cache
        String::from("--data-dir"),
        data_dir.to_str().unwrap().to_string(),
        // run the client natively
        String::from("--native"),
    ]);
    // verbosity level
    if verbosity > 0 {
        proving_args.push(
            [
                String::from("-"),
                (0..verbosity).map(|_| 'v').collect::<String>(),
            ]
            .concat(),
        );
    }
    proving_args
}
