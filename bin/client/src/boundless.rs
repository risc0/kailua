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

use crate::proving::{ProvingArgs, ProvingError};
use alloy::network::Ethereum;
use alloy::transports::http::reqwest::Url;
use alloy_primitives::utils::parse_ether;
use alloy_primitives::{Address, U256};
use anyhow::{anyhow, bail, Context};
use boundless_market::alloy::providers::Provider;
use boundless_market::alloy::signers::local::PrivateKeySigner;
use boundless_market::client::{Client, ClientBuilder};
use boundless_market::contracts::{
    Input, Offer, Predicate, ProofRequest, RequestId, RequestStatus, Requirements,
};
use boundless_market::input::InputBuilder;
use boundless_market::storage::{StorageProvider, StorageProviderConfig, StorageProviderType};
use clap::Parser;
use kailua_build::{KAILUA_FPVM_ELF, KAILUA_FPVM_ID};
use kailua_common::journal::ProofJournal;
use risc0_zkvm::sha::Digestible;
use risc0_zkvm::{default_executor, ExecutorEnv, Journal, Receipt};
use std::time::Duration;
use tracing::info;
use tracing::log::warn;

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
    #[clap(
        long,
        env,
        requires = "boundless_order_stream_url",
        default_value_t = false
    )]
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
    pub boundless_lookback: u32,
    /// Starting price per megacycle of the proving order
    #[clap(long, env)]
    #[arg(required = false, default_value = "0.0001")]
    pub boundless_order_min_price_eth: String,
    /// Maximum price per megacycle of the proving order
    #[clap(long, env)]
    #[arg(required = false, default_value = "0.0002")]
    pub boundless_order_max_price_eth: String,
    /// Time in seconds before order pricing increases
    #[clap(long, env)]
    #[arg(required = false, default_value_t = 60)]
    pub boundless_order_ramp_up_period: u32,
    /// Multiplier for order fulfillment timeout after locking
    #[clap(long, env)]
    #[arg(required = false, default_value_t = 3.0)]
    pub boundless_order_lock_timeout_factor: f64,
    /// Multiplier for order expiry timeout after creation
    #[clap(long, env)]
    #[arg(required = false, default_value_t = 10.0)]
    pub boundless_order_timeout_factor: f64,
    /// Time in seconds between attempts to check order status
    #[clap(long, env)]
    #[arg(required = false, default_value_t = 12)]
    pub boundless_order_check_interval: u64,
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
            String::from("--boundless-lookback"),
            self.boundless_lookback.to_string(),
            String::from("--boundless-order-min-price-eth"),
            self.boundless_order_min_price_eth.to_string(),
            String::from("--boundless-order-max-price-eth"),
            self.boundless_order_max_price_eth.to_string(),
            String::from("--boundless-order-ramp-up-period"),
            self.boundless_order_ramp_up_period.to_string(),
            String::from("--boundless-order-lock-timeout-factor"),
            self.boundless_order_lock_timeout_factor.to_string(),
            String::from("--boundless-order-timeout-factor"),
            self.boundless_order_timeout_factor.to_string(),
            String::from("--boundless-order-check-interval"),
            self.boundless_order_check_interval.to_string(),
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
    witness_frames: Vec<Vec<u8>>,
    stitched_proofs: Vec<Receipt>,
    proving_args: &ProvingArgs,
) -> Result<Receipt, ProvingError> {
    info!("Running boundless client.");
    let proof_journal = Journal::new(journal.encode_packed());

    // Instantiate client
    let boundless_client = ClientBuilder::default()
        .with_private_key(args.boundless_wallet_key)
        .with_rpc_url(args.boundless_rpc_url)
        .with_boundless_market_address(args.boundless_market_address)
        .with_set_verifier_address(args.boundless_set_verifier_address)
        .with_storage_provider_config(storage)
        .await
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?
        .with_order_stream_url(
            args.boundless_offchain
                .then_some(args.boundless_order_stream_url)
                .flatten(),
        )
        .build()
        .await
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;

    // Set the proof request requirements
    let requirements = Requirements::new(
        KAILUA_FPVM_ID,
        Predicate::digest_match(proof_journal.digest()),
    )
    .with_groth16_proof();

    // Check if an unexpired request had already been made recently
    let boundless_wallet_address = boundless_client.local_signer.as_ref().unwrap().address();
    let boundless_wallet_nonce = boundless_client
        .provider()
        .get_transaction_count(boundless_wallet_address)
        .await
        .context("get_transaction_count boundless_wallet_address")
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))? as u32;

    // Look back at prior transactions to avoid repeated requests
    for i in 0..args.boundless_lookback {
        if i > boundless_wallet_nonce {
            break;
        }
        let nonce = boundless_wallet_nonce.saturating_sub(i + 1);

        let request_id = RequestId::u256(boundless_wallet_address, nonce);
        info!("Looking back at txn w/ nonce {nonce} | request: {request_id:x}");

        let Ok((request, _)) = boundless_client
            .boundless_market
            .get_submitted_request(request_id, None)
            .await
            .context("get_submitted_request")
            .map_err(|e| ProvingError::OtherError(anyhow!(e)))
        else {
            // No request for that nonce
            continue;
        };

        let request_status = boundless_client
            .boundless_market
            .get_status(request_id, Some(request.expires_at()))
            .await
            .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;

        if matches!(request_status, RequestStatus::Expired) {
            // We found a duplicate but it was expired
            continue;
        }

        // Skip unrelated request
        if request.requirements != requirements {
            continue;
        }

        info!("Found matching request already submitted!");

        if proving_args.skip_await_proof {
            warn!("Skipping awaiting proof on Boundless and exiting process.");
            std::process::exit(0);
        }

        return retrieve_proof(
            boundless_client,
            request_id,
            args.boundless_order_check_interval,
            request.expires_at(),
        )
        .await
        .map_err(|e| ProvingError::OtherError(anyhow!(e)));
    }

    // Preflight execution to get cycle count
    info!("Preflighting execution.");
    let preflight_witness_frames = witness_frames.clone();
    let preflight_stitched_proofs = stitched_proofs.clone();
    let segment_limit = proving_args.segment_limit;
    let session_info = tokio::task::spawn_blocking(move || {
        let mut builder = ExecutorEnv::builder();
        // Set segment po2
        builder.segment_limit_po2(segment_limit);
        // Pass in witness data
        for frame in &preflight_witness_frames {
            builder.write_frame(frame);
        }
        // Pass in proofs
        for proof in &preflight_stitched_proofs {
            builder.write(proof)?;
        }
        let env = builder.build()?;
        let session_info = default_executor().execute(env, KAILUA_FPVM_ELF)?;
        Ok::<_, anyhow::Error>(session_info)
    })
    .await
    .map_err(|e| ProvingError::OtherError(anyhow!(e)))?
    .map_err(|e| ProvingError::ExecutionError(anyhow!(e)))?;
    let mcycles_count = session_info
        .segments
        .iter()
        .map(|segment| 1 << segment.po2)
        .sum::<u64>()
        .div_ceil(1_000_000);

    // todo: remember this storage location to avoid duplicate uploads
    // Upload the ELF to the storage provider so that it can be fetched by the market.
    if boundless_client.storage_provider.is_none() {
        return Err(ProvingError::OtherError(anyhow!(
            "A storage provider is required to host the FPVM program and input."
        )));
    }
    let image_url = boundless_client
        .upload_program(KAILUA_FPVM_ELF)
        .await
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    info!("Uploaded image to {}", image_url);
    // Upload input
    let mut builder = InputBuilder::new();
    for frame in &witness_frames {
        builder = builder.write_frame(frame);
    }
    // Pass in proofs
    for proof in &stitched_proofs {
        builder = builder
            .write(proof)
            .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    }
    // Build final input
    let input = builder
        .build_vec()
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    let input_url = boundless_client
        .upload_input(&input)
        .await
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    info!("Uploaded input to {input_url}");
    let request_input = Input::url(input_url);
    let request = ProofRequest::builder()
        .with_image_url(image_url.as_str())
        .with_input(request_input)
        .with_requirements(requirements)
        .with_offer(
            Offer::default()
                .with_min_price_per_mcycle(
                    parse_ether(&args.boundless_order_min_price_eth)
                        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?,
                    mcycles_count,
                )
                .with_max_price_per_mcycle(
                    parse_ether(&args.boundless_order_max_price_eth)
                        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?,
                    mcycles_count,
                )
                .with_ramp_up_period(args.boundless_order_ramp_up_period)
                .with_lock_stake_per_mcycle(
                    parse_ether(&args.boundless_order_max_price_eth)
                        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?,
                    mcycles_count,
                )
                .with_lock_timeout(
                    (args.boundless_order_lock_timeout_factor * mcycles_count as f64) as u32,
                )
                .with_timeout((args.boundless_order_timeout_factor * mcycles_count as f64) as u32),
        )
        .with_request_id(RequestId::new(
            boundless_wallet_address,
            boundless_wallet_nonce,
        ))
        .build()
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;

    // Send the request and wait for it to be completed.
    let (request_id, expires_at) = boundless_client
        .submit_request(&request)
        .await
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    info!("Boundless request 0x{request_id:x} submitted");

    if proving_args.skip_await_proof {
        warn!("Skipping awaiting proof on Boundless and exiting process.");
        std::process::exit(0);
    }

    retrieve_proof(
        boundless_client,
        request_id,
        args.boundless_order_check_interval,
        expires_at,
    )
    .await
    .map_err(|e| ProvingError::OtherError(anyhow!(e)))
}

pub async fn retrieve_proof<P: Provider<Ethereum> + 'static + Clone, S: StorageProvider>(
    boundless_client: Client<P, S>,
    request_id: U256,
    interval: u64,
    expires_at: u64,
) -> anyhow::Result<Receipt> {
    // Wait for the request to be fulfilled by the market, returning the journal and seal.
    info!("Waiting for 0x{request_id:x} to be fulfilled");
    let (journal, seal) = boundless_client
        .wait_for_request_fulfillment(request_id, Duration::from_secs(interval), expires_at)
        .await?;

    let risc0_ethereum_contracts::receipt::Receipt::Base(receipt) =
        risc0_ethereum_contracts::receipt::decode_seal(seal, KAILUA_FPVM_ID, journal)?
    else {
        bail!("Did not receive an unaggregated receipt.");
    };

    Ok(*receipt)
}
