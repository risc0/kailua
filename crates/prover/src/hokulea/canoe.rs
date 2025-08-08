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

use crate::args::ProvingArgs;
use crate::proof::{proof_file_name, read_bincoded_file};
use crate::risczero::boundless::BoundlessArgs;
use crate::risczero::seek_proof;
use alloy::transports::http::reqwest::Url;
use anyhow::{anyhow, bail, Context};
use async_trait::async_trait;
use canoe_bindings::StatusCode;
use canoe_provider::{CanoeInput, CanoeProvider, CertVerifierCall};
use hokulea_proof::canoe_verifier::{cert_verifier_address, to_journal_bytes};
use hokulea_proof::cert_validity::CertValidity;
use risc0_steel::alloy::providers::ProviderBuilder;
use risc0_steel::ethereum::{
    EthChainSpec, EthEvmEnv, ETH_HOLESKY_CHAIN_SPEC, ETH_MAINNET_CHAIN_SPEC, ETH_SEPOLIA_CHAIN_SPEC,
};
use risc0_steel::host::BlockNumberOrTag;
use risc0_steel::Contract;
use risc0_zkvm::serde::to_vec;
use risc0_zkvm::Journal;
use std::str::FromStr;
use tracing::info;

/// A canoe provider implementation with steel
#[derive(Debug, Clone)]
pub struct KailuaCanoeSteelProvider {
    /// rpc to l1 geth node
    pub eth_rpc_url: String,
    /// proving arguments
    pub proving_args: ProvingArgs,
    /// Boundless arguments
    pub boundless_args: BoundlessArgs,
}

#[async_trait]
impl CanoeProvider for KailuaCanoeSteelProvider {
    type Receipt = risc0_zkvm::Receipt;

    async fn create_cert_validity_proof(&self, input: CanoeInput) -> anyhow::Result<Self::Receipt> {
        info!(
            "Begin to generate a Canoe proof using l1 block number {}",
            input.l1_head_block_number
        );

        let eth_rpc_url =
            Url::from_str(&self.eth_rpc_url).context("Failed to parse Ethereum RPC URL")?;

        // Create an alloy provider for that private key and URL.
        let l1_provider = ProviderBuilder::new().connect_http(eth_rpc_url);

        let chain_spec = match input.l1_chain_id {
            1 => ETH_MAINNET_CHAIN_SPEC.clone(),
            11155111 => ETH_SEPOLIA_CHAIN_SPEC.clone(),
            17000 => ETH_HOLESKY_CHAIN_SPEC.clone(),
            _ => EthChainSpec::new_single(input.l1_chain_id, Default::default()),
        };

        let mut env = EthEvmEnv::builder()
            .chain_spec(&chain_spec)
            .provider(l1_provider)
            .block_number_or_tag(BlockNumberOrTag::Number(input.l1_head_block_number))
            .build()
            .await?;

        let verifier_address = cert_verifier_address(input.l1_chain_id, &input.altda_commitment);
        info!("Using Cert Verifier address: {verifier_address}");

        // Preflight the call to prepare the input that is required to execute the function in
        // the guest without RPC access. It also returns the result of the call.
        let mut contract = Contract::preflight(verifier_address, &mut env);

        // Prepare the function call
        let preflight_validity = match CertVerifierCall::build(&input.altda_commitment) {
            CertVerifierCall::V2(call) => contract.call_builder(&call).call().await?,
            CertVerifierCall::Router(call) => {
                let status = contract.call_builder(&call).call().await?;
                status == StatusCode::SUCCESS as u8
            }
        };

        // Verify same outcome
        if input.claimed_validity != preflight_validity {
            bail!(
                "claimed_validity={} != preflight_validity={}",
                input.claimed_validity,
                preflight_validity
            );
        }

        // Construct the input from the environment.
        let evm_input: risc0_steel::EvmInput<risc0_steel::ethereum::EthEvmFactory> =
            env.into_input().await?;

        // Construct output
        let journal = Journal::new(to_journal_bytes(
            &CertValidity {
                claimed_validity: input.claimed_validity,
                canoe_proof: None,
                l1_head_block_hash: input.l1_head_block_hash,
                l1_chain_id: input.l1_chain_id,
            },
            &input.altda_commitment,
        ));

        // todo: dynamic lookup of KAILUA_DA_HOKULEA_ID corresponding to KAILUA_FPVM_HOKULEA_ID
        let file_name = proof_file_name(kailua_build::KAILUA_DA_HOKULEA_ID, journal.clone());

        seek_proof(
            &self.proving_args,
            self.boundless_args.clone(),
            journal,
            vec![
                to_vec(&evm_input)?,
                to_vec(&verifier_address)?,
                to_vec(&input)?,
            ],
            vec![],
            vec![],
            false,
        )
        .await
        .map_err(|err| anyhow!(err))?;

        read_bincoded_file(&file_name)
            .await
            .context(format!("Failed to read proof file {file_name} contents."))
    }

    fn get_eth_rpc_url(&self) -> String {
        self.eth_rpc_url.clone()
    }
}
