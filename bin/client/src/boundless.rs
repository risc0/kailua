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

use crate::proof::Proof;
use alloy::signers::k256::ecdsa::signature::digest::Digest;
use alloy::sol_types::SolValue;
use alloy::transports::http::reqwest::Url;
use alloy_primitives::utils::parse_ether;
use alloy_primitives::{Address, B256, U160, U256};
use anyhow::{ensure, Context};
use boundless_market::alloy::providers::Provider;
use boundless_market::alloy::signers::local::PrivateKeySigner;
use boundless_market::client::ClientBuilder;
use boundless_market::contracts::{Input, Offer, Predicate, ProofRequest, Requirements};
use boundless_market::input::InputBuilder;
use boundless_market::storage::{StorageProviderConfig, StorageProviderType};
use clap::Parser;
use kailua_build::{KAILUA_FPVM_ELF, KAILUA_FPVM_ID};
use kailua_common::journal::ProofJournal;
use kailua_common::oracle::vec::VecOracle;
use kailua_common::witness::Witness;
use risc0_zkvm::sha::Digestible;
use risc0_zkvm::{default_executor, is_dev_mode, ExecutorEnv, Journal};
use std::borrow::Borrow;
use std::time::Duration;
use tracing::{info, warn};

#[derive(Parser, Clone, Debug)]
pub struct BoundlessArgs {
    /// Market provider for proof requests
    #[clap(flatten)]
    pub market: Option<MarketProviderConfig>,
    /// Storage provider for elf and input
    #[clap(flatten)]
    pub storage: Option<StorageProviderConfig>,
}

#[derive(Parser, Debug, Clone)]
#[group(requires_all = ["boundless_rpc_url", "boundless_wallet_key", "boundless_set_verifier_address", "boundless_market_address"])]
pub struct MarketProviderConfig {
    /// URL of the Ethereum RPC endpoint.
    #[clap(long, env)]
    #[arg(required = false)]
    pub boundless_rpc_url: Url,
    /// Private key used to interact with the EvenNumber contract.
    #[clap(long, env)]
    #[arg(required = false)]
    pub boundless_wallet_key: PrivateKeySigner,
    /// Submit the request offchain via the provided order stream service url.
    #[clap(long, requires = "boundless_order_stream_url", default_value_t = false)]
    pub boundless_offchain: bool,
    /// Offchain order stream service URL to submit offchain requests to.
    #[clap(long, env)]
    pub boundless_order_stream_url: Option<Url>,
    /// Address of the RiscZeroSetVerifier contract.
    #[clap(long, env)]
    #[arg(required = false)]
    pub boundless_set_verifier_address: Address,
    /// Address of the BoundlessMarket contract.
    #[clap(long, env)]
    #[arg(required = false)]
    pub boundless_market_address: Address,
    /// Number of transactions to lookback at
    #[clap(long, env)]
    #[arg(required = false, default_value_t = 5)]
    pub boundless_lookback: u64,
}

impl MarketProviderConfig {
    pub fn to_arg_vec(
        &self,
        storage_provider_config: &Option<StorageProviderConfig>,
    ) -> Vec<String> {
        let mut proving_args = Vec::new();
        proving_args.extend(vec![
            String::from("--boundless-rpc-url"),
            self.boundless_rpc_url.to_string(),
            String::from("--boundless-wallet-key"),
            self.boundless_wallet_key.to_bytes().to_string(),
            String::from("--boundless-set-verifier-address"),
            self.boundless_set_verifier_address.to_string(),
            String::from("--boundless-market-address"),
            self.boundless_market_address.to_string(),
        ]);
        if self.boundless_offchain {
            proving_args.push(String::from("--boundless-offchain"));
        }
        if let Some(url) = &self.boundless_order_stream_url {
            proving_args.extend(vec![
                String::from("--boundless-order-stream-url"),
                url.to_string(),
            ]);
        }
        if let Some(storage_cfg) = storage_provider_config {
            match &storage_cfg.storage_provider {
                StorageProviderType::S3 => {
                    proving_args.extend(vec![
                        String::from("--storage-provider"),
                        String::from("s3"),
                        String::from("--s3-access-key"),
                        storage_cfg.s3_access_key.clone().unwrap(),
                        String::from("--s3-secret-key"),
                        storage_cfg.s3_secret_key.clone().unwrap(),
                        String::from("--s3-bucket"),
                        storage_cfg.s3_bucket.clone().unwrap(),
                        String::from("--s3-url"),
                        storage_cfg.s3_url.clone().unwrap(),
                        String::from("--aws-region"),
                        storage_cfg.aws_region.clone().unwrap(),
                    ]);
                }
                StorageProviderType::Pinata => {
                    proving_args.extend(vec![
                        String::from("--storage-provider"),
                        String::from("pinata"),
                        String::from("--pinata-jwt"),
                        storage_cfg.pinata_jwt.clone().unwrap(),
                    ]);
                    if let Some(pinata_api_url) = &storage_cfg.pinata_api_url {
                        proving_args.extend(vec![
                            String::from("--pinata-api-url"),
                            pinata_api_url.to_string(),
                        ]);
                    }
                    if let Some(ipfs_gateway_url) = &storage_cfg.ipfs_gateway_url {
                        proving_args.extend(vec![
                            String::from("--ipfs-gateway-url"),
                            ipfs_gateway_url.to_string(),
                        ]);
                    }
                }
                StorageProviderType::File => {
                    proving_args.extend(vec![
                        String::from("--storage-provider"),
                        String::from("file"),
                    ]);
                    if let Some(file_path) = &storage_cfg.file_path {
                        proving_args.extend(vec![
                            String::from("--file-path"),
                            file_path.to_str().unwrap().to_string(),
                        ]);
                    }
                }
                _ => unimplemented!("Unknown storage provider."),
            }
        }
        proving_args
    }
}

pub async fn run_boundless_client(
    args: MarketProviderConfig,
    storage: Option<StorageProviderConfig>,
    journal: ProofJournal,
    witness: Witness<VecOracle>,
) -> anyhow::Result<Proof> {
    info!("Running boundless client.");
    let proof_journal = Journal::new(journal.encode_packed());

    // Instantiate client
    let boundless_client = ClientBuilder::default()
        .with_rpc_url(args.boundless_rpc_url)
        .with_boundless_market_address(args.boundless_market_address)
        .with_set_verifier_address(args.boundless_set_verifier_address)
        .with_order_stream_url(
            args.boundless_offchain
                .then_some(args.boundless_order_stream_url)
                .flatten(),
        )
        .with_storage_provider_config(storage)
        .with_private_key(args.boundless_wallet_key)
        .build()
        .await?;

    // ad-hoc boundless dev mode
    if is_dev_mode() {
        warn!("DEV MODE: Generating fake boundless network proof.");
        let seal = kailua_contracts::SetVerifierSeal {
            path: vec![],
            rootSeal: Default::default(),
        }
        .abi_encode();
        let image_id = boundless_client
            .set_verifier
            .image_info()
            .await
            .context("Failed to get image info")?
            .0;
        let selector = set_verifier_selector(image_id);
        let encoded_seal = [selector.as_slice(), seal.as_slice()].concat();
        return Ok(Proof::BoundlessSeal(encoded_seal, proof_journal));
    }

    // Set the proof request requirements
    let requirements = Requirements::new(
        KAILUA_FPVM_ID,
        Predicate::digest_match(proof_journal.digest()),
    );

    // Check if an unexpired request had already been made recently
    let boundless_wallet_address = boundless_client.signer.address();
    let boundless_wallet_nonce = boundless_client
        .provider()
        .get_transaction_count(boundless_wallet_address)
        .await
        .context("get_transaction_count boundless_wallet_address")?;

    // Look back at prior transactions to avoid repeated requests
    for i in 0..args.boundless_lookback {
        if i > boundless_wallet_nonce {
            break;
        }
        let nonce = boundless_wallet_nonce.saturating_sub(i + 1) as u32;

        let request_id = request_id(&boundless_wallet_address, nonce);
        info!("Looking back at txn w/ nonce {nonce} | request: {request_id:x}");

        let Ok((request, _)) = boundless_client
            .boundless_market
            .get_submitted_request(request_id, None)
            .await
            .context("get_submitted_request")
        else {
            // No request for that nonce
            continue;
        };

        // Skip unrelated request
        if request.requirements != requirements {
            continue;
        }

        info!("Waiting for 0x{request_id:x} to be fulfilled");
        let (_journal, seal) = boundless_client
            .wait_for_request_fulfillment(request_id, Duration::from_secs(5), request.expires_at())
            .await?;
        info!("Request 0x{request_id:x} fulfilled");

        return Ok(Proof::BoundlessSeal(seal.to_vec(), proof_journal));
    }

    // Preflight execution to get cycle count
    info!("Preflighting execution.");
    let input_frame = rkyv::to_bytes::<rkyv::rancor::Error>(&witness)?.to_vec();
    info!("Witness size: {}", input_frame.len());
    let env = ExecutorEnv::builder()
        // Pass in witness data
        .write_frame(&input_frame)
        .build()?;
    let session_info = default_executor().execute(env, KAILUA_FPVM_ELF)?;
    let mcycles_count = session_info
        .segments
        .iter()
        .map(|segment| 1 << segment.po2)
        .sum::<u64>()
        .div_ceil(1_000_000);

    // todo: remember this storage location to avoid duplicate uploads
    // Upload the ELF to the storage provider so that it can be fetched by the market.
    ensure!(
        boundless_client.storage_provider.is_some(),
        "A storage provider is required to host the FPVM program and input."
    );
    let image_url = boundless_client.upload_image(KAILUA_FPVM_ELF).await?;
    info!("Uploaded image to {}", image_url);
    // Upload input
    let input = InputBuilder::new().write_frame(&input_frame).build();
    let input_url = boundless_client.upload_input(&input).await?;
    info!("Uploaded input to {input_url}");
    let request_input = Input::url(input_url);
    let request = {
        let mut req = ProofRequest::default()
            .with_image_url(&image_url)
            .with_input(request_input)
            .with_requirements(requirements)
            .with_offer(
                Offer::default()
                    .with_min_price_per_mcycle(parse_ether("0.001")?, mcycles_count)
                    .with_max_price_per_mcycle(parse_ether("0.002")?, mcycles_count)
                    .with_ramp_up_period(10)
                    .with_timeout(1500),
            );
        req.id = boundless_client
            .boundless_market
            .request_id_from_nonce()
            .await
            .context("request_id_from_nonce")?;
        req
    };

    // Send the request and wait for it to be completed.
    let (request_id, expires_at) = boundless_client.submit_request(&request).await?;
    info!("Boundless request 0x{request_id:x} submitted");

    // Wait for the request to be fulfilled by the market, returning the journal and seal.
    info!("Waiting for 0x{request_id:x} to be fulfilled");
    let (_journal, seal) = boundless_client
        .wait_for_request_fulfillment(request_id, Duration::from_secs(5), expires_at)
        .await?;
    info!("Request 0x{request_id:x} fulfilled");

    Ok(Proof::BoundlessSeal(seal.to_vec(), proof_journal))
}

pub fn request_id(addr: &Address, id: u32) -> U256 {
    let addr = U160::from_be_bytes(addr.0 .0);
    (U256::from(addr) << 32) | U256::from(id)
}

pub fn set_verifier_selector(image_id: B256) -> [u8; 4] {
    let tag = sha2::Sha256::digest("risc0.SetInclusionReceiptVerifierParameters");
    let len = (1u16 << 8).to_be_bytes();
    let input = [tag.as_slice(), image_id.as_slice(), len.as_slice()].concat();
    let digest = sha2::Sha256::digest(&input);
    digest.as_slice()[..4].try_into().unwrap()
}

pub fn encoded_set_builder_journal(
    fpvm_claim_digest: &risc0_zkvm::sha::Digest,
    set_builder_siblings: impl IntoIterator<Item = impl Borrow<risc0_zkvm::sha::Digest>>,
    set_builder_id: risc0_zkvm::sha::Digest,
) -> Vec<u8> {
    // derive the root
    let set_builder_root =
        risc0_aggregation::merkle_path_root(fpvm_claim_digest, set_builder_siblings);
    // construct set builder root from merkle proof
    risc0_aggregation::GuestOutput::new(set_builder_id, set_builder_root).abi_encode()
}
