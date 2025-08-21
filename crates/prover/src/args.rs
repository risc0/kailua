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

use crate::hana::args::HanaArgs;
use crate::hokulea::args::HokuleaArgs;
use crate::risczero::boundless::BoundlessArgs;
use alloy_primitives::{Address, B256};
use clap::Parser;
use kailua_sync::args::{parse_address, parse_b256};
use kailua_sync::telemetry::TelemetryArgs;
use std::cmp::Ordering;

#[derive(Parser, Clone, Debug)]
pub struct ProvingArgs {
    /// Address of the recipient account to use for bond payouts
    #[clap(long, env, value_parser = parse_address)]
    pub payout_recipient_address: Option<Address>,
    /// ZKVM Proving Segment Limit
    #[clap(long, env, required = false, default_value_t = 21)]
    pub segment_limit: u32,
    /// Maximum input data size per proof
    #[clap(long, env, required = false, default_value_t = 2_684_354_560)]
    pub max_witness_size: usize,
    /// How many threads to use for fetching preflight data
    #[clap(long, env, default_value_t = 4)]
    pub num_concurrent_preflights: u64,
    /// How many threads to use for computing proofs
    #[clap(long, env, default_value_t = 1)]
    pub num_concurrent_proofs: u64,
    /// Whether to bypass loading rollup chain configurations from the kona registry
    #[clap(long, env, default_value_t = false)]
    pub bypass_chain_registry: bool,
    /// Whether to only prove L2 block execution without referring to the L1
    #[clap(long, env, default_value_t = false)]
    pub skip_derivation_proof: bool,
    /// Whether to skip waiting for the proof generation process to complete
    #[clap(long, env, default_value_t = false)]
    pub skip_await_proof: bool,
    /// Whether to keep cache data after successful completion
    #[clap(long, env, default_value_t = false)]
    pub clear_cache_data: bool,

    #[clap(flatten)]
    pub hokulea: HokuleaArgs,
    #[clap(flatten)]
    pub hana: HanaArgs,
}

impl ProvingArgs {
    pub fn to_arg_vec(&self) -> Vec<String> {
        // Core args
        let mut proving_args = vec![
            String::from("--segment-limit"),
            self.segment_limit.to_string(),
            String::from("--max-witness-size"),
            self.max_witness_size.to_string(),
            String::from("--num-concurrent-preflights"),
            self.num_concurrent_preflights.to_string(),
            String::from("--num-concurrent-proofs"),
            self.num_concurrent_proofs.to_string(),
        ];
        // Core flags
        proving_args.extend(
            [
                self.bypass_chain_registry
                    .then(|| String::from("--bypass-chain-registry")),
                self.skip_derivation_proof
                    .then(|| String::from("--skip-derivation-proof")),
                self.skip_await_proof
                    .then(|| String::from("--skip-await-proof")),
            ]
            .into_iter()
            .flatten(),
        );
        if let Some(payout_recipient_address) = &self.payout_recipient_address {
            proving_args.extend(vec![
                // wallet address for payouts
                String::from("--payout-recipient-address"),
                payout_recipient_address.to_string(),
            ]);
        }
        // Hokulea
        proving_args.extend(self.hokulea.to_arg_vec());
        // Hana
        proving_args.extend(self.hana.to_arg_vec());
        // Return
        proving_args
    }

    pub fn skip_stitching(&self) -> bool {
        self.skip_derivation_proof || self.skip_await_proof
    }

    pub fn use_hokulea(&self) -> bool {
        self.hokulea.is_set()
    }

    pub fn use_hana(&self) -> bool {
        !self.hokulea.is_set() && self.hana.is_set()
    }

    pub fn image_id(&self) -> [u32; 8] {
        if self.use_hokulea() {
            kailua_build::KAILUA_FPVM_HOKULEA_ID
        } else if self.use_hana() {
            kailua_build::KAILUA_FPVM_HANA_ID
        } else {
            kailua_build::KAILUA_FPVM_KONA_ID
        }
    }

    pub fn elf(&self) -> &'static [u8] {
        if self.use_hokulea() {
            kailua_build::KAILUA_FPVM_HOKULEA_ELF
        } else if self.use_hana() {
            kailua_build::KAILUA_FPVM_HANA_ELF
        } else {
            kailua_build::KAILUA_FPVM_KONA_ELF
        }
    }

    pub fn image(&self) -> ([u32; 8], &'static [u8]) {
        (self.image_id(), self.elf())
    }
}

/// Run the prover to generate an execution/fault/validity proof
#[derive(Parser, Clone, Debug)]
pub struct ProveArgs {
    #[clap(flatten)]
    pub kona: kona_host::single::SingleChainHost,

    /// Address of OP-NODE endpoint to use
    #[clap(long, env)]
    pub op_node_address: Option<String>,

    #[clap(flatten)]
    pub proving: ProvingArgs,
    #[clap(flatten)]
    pub boundless: BoundlessArgs,

    #[clap(long, env, value_delimiter = ',')]
    pub precondition_params: Vec<u64>,
    #[clap(long, env, value_parser = parse_b256, value_delimiter = ',')]
    pub precondition_block_hashes: Vec<B256>,
    #[clap(long, env, value_parser = parse_b256, value_delimiter = ',')]
    pub precondition_blob_hashes: Vec<B256>,

    #[clap(flatten)]
    pub telemetry: TelemetryArgs,
}

impl ProveArgs {
    pub fn to_arg_vec(&self) -> Vec<String> {
        // Prepare prover parameters
        let mut prove_args = vec![
            // Invoke the CLI prove command
            String::from("prove"),
        ];

        // kona args
        prove_args.extend(vec![
            // l1 head from on-chain proposal
            String::from("--l1-head"),
            self.kona.l1_head.to_string(),
            // l2 starting block hash from on-chain proposal
            String::from("--agreed-l2-head-hash"),
            self.kona.agreed_l2_head_hash.to_string(),
            // l2 starting output root
            String::from("--agreed-l2-output-root"),
            self.kona.agreed_l2_output_root.to_string(),
            // proposed output root
            String::from("--claimed-l2-output-root"),
            self.kona.claimed_l2_output_root.to_string(),
            // proposed block number
            String::from("--claimed-l2-block-number"),
            self.kona.claimed_l2_block_number.to_string(),
        ]);
        if let Some(l2_chain_id) = self.kona.l2_chain_id {
            prove_args.extend(vec![
                // rollup chain id
                String::from("--l2-chain-id"),
                l2_chain_id.to_string(),
            ]);
        }
        if let Some(l1_node_address) = &self.kona.l1_node_address {
            prove_args.extend(vec![
                // l1 el node
                String::from("--l1-node-address"),
                l1_node_address.clone(),
            ]);
        }
        if let Some(l1_beacon_address) = &self.kona.l1_beacon_address {
            prove_args.extend(vec![
                // l1 cl node
                String::from("--l1-beacon-address"),
                l1_beacon_address.clone(),
            ]);
        }
        if let Some(l2_node_address) = &self.kona.l2_node_address {
            prove_args.extend(vec![
                // l2 el node
                String::from("--l2-node-address"),
                l2_node_address.clone(),
            ]);
        }
        if let Some(data_dir) = &self.kona.data_dir {
            prove_args.extend(vec![
                // path to cache
                String::from("--data-dir"),
                data_dir.to_str().unwrap().to_string(),
            ]);
        }

        // op-node
        if let Some(op_node_address) = &self.op_node_address {
            prove_args.extend(vec![
                // l2 el node
                String::from("--op-node-address"),
                op_node_address.to_string(),
            ])
        }

        // proving args
        prove_args.extend(self.proving.to_arg_vec());

        // boundless args
        if let Some(market) = &self.boundless.market {
            prove_args.extend(market.to_arg_vec(&self.boundless.storage));
        }

        // precondition data
        if !self.precondition_params.is_empty() {
            prove_args.extend(vec![
                String::from("--precondition-params"),
                self.precondition_params
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<String>>()
                    .join(","),
            ]);
        }
        if !self.precondition_block_hashes.is_empty() {
            prove_args.extend(vec![
                String::from("--precondition-block-hashes"),
                self.precondition_block_hashes
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<String>>()
                    .join(","),
            ]);
        }
        if !self.precondition_blob_hashes.is_empty() {
            prove_args.extend(vec![
                String::from("--precondition-blob-hashes"),
                self.precondition_blob_hashes
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<String>>()
                    .join(","),
            ]);
        }

        // telemetry
        prove_args.extend(self.telemetry.to_arg_vec());

        prove_args
    }
}

impl PartialEq<Self> for ProveArgs {
    fn eq(&self, other: &Self) -> bool {
        self.kona
            .claimed_l2_block_number
            .eq(&other.kona.claimed_l2_block_number)
    }
}

impl Eq for ProveArgs {}

impl PartialOrd<Self> for ProveArgs {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ProveArgs {
    fn cmp(&self, other: &Self) -> Ordering {
        self.kona
            .claimed_l2_block_number
            .cmp(&other.kona.claimed_l2_block_number)
    }
}
