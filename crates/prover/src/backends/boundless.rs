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
use crate::client::proving::save_to_bincoded_file;
use crate::proof::read_bincoded_file;
use crate::ProvingError;
use alloy::transports::http::reqwest::Url;
use alloy_primitives::{keccak256, Address, U256};
use anyhow::{anyhow, bail, Context};
use boundless_market::alloy::providers::Provider;
use boundless_market::alloy::signers::local::PrivateKeySigner;
use boundless_market::client::Client;
use boundless_market::contracts::{Predicate, RequestId, RequestStatus, Requirements};
use boundless_market::request_builder::OfferParams;
use boundless_market::storage::{StorageProviderConfig, StorageProviderType};
use boundless_market::{Deployment, GuestEnv, StandardStorageProvider};
use clap::Parser;
use kailua_build::{KAILUA_FPVM_ELF, KAILUA_FPVM_ID};
use kailua_common::journal::ProofJournal;
use risc0_zkvm::sha::Digestible;
use risc0_zkvm::{default_executor, ExecutorEnv, Journal, Receipt};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::time::Duration;
use tokio::time::sleep;
use tracing::log::warn;
use tracing::{debug, info};

#[derive(Parser, Clone, Debug, Default)]
pub struct BoundlessArgs {
    /// Market provider for proof requests
    #[clap(flatten)]
    pub market: Option<MarketProviderConfig>,
    /// Storage provider for elf and input
    #[clap(flatten)]
    pub storage: Option<StorageProviderConfig>,
}

#[derive(Parser, Debug, Clone)]
#[group(requires_all = ["boundless_rpc_url", "boundless_wallet_key"])]
pub struct MarketProviderConfig {
    /// URL of the Ethereum RPC endpoint.
    #[clap(long, env, required = false)]
    pub boundless_rpc_url: Url,
    /// Private key used to interact with the EvenNumber contract.
    #[clap(long, env, required = false)]
    pub boundless_wallet_key: PrivateKeySigner,

    /// EIP-155 chain ID of the network hosting Boundless.
    ///
    /// This parameter takes precedent over all other deployment arguments if set to a known value
    #[clap(long, env, required = false)]
    pub boundless_chain_id: Option<u64>,
    /// Address of the [BoundlessMarket] contract.
    ///
    /// [BoundlessMarket]: crate::contracts::IBoundlessMarket
    #[clap(long, env, required = false)]
    pub boundless_market_address: Option<Address>,
    /// Address of the [RiscZeroVerifierRouter] contract.
    ///
    /// The verifier router implements [IRiscZeroVerifier]. Each network has a canonical router,
    /// that is deployed by the core team. You can additionally deploy and manage your own verifier
    /// instead. See the [Boundless docs for more details].
    ///
    /// [RiscZeroVerifierRouter]: https://github.com/risc0/risc0-ethereum/blob/main/contracts/src/RiscZeroVerifierRouter.sol
    /// [IRiscZeroVerifier]: https://github.com/risc0/risc0-ethereum/blob/main/contracts/src/IRiscZeroVerifier.sol
    /// [Boundless docs for more details]: https://docs.beboundless.xyz/developers/smart-contracts/verifier-contracts
    #[clap(
        long,
        env = "VERIFIER_ADDRESS",
        required = false,
        long_help = "Address of the RiscZeroVerifierRouter contract"
    )]
    pub boundless_verifier_router_address: Option<Address>,
    /// Address of the [RiscZeroSetVerifier] contract.
    ///
    /// [RiscZeroSetVerifier]: https://github.com/risc0/risc0-ethereum/blob/main/contracts/src/RiscZeroSetVerifier.sol
    #[clap(long, env, required = false)]
    pub boundless_set_verifier_address: Option<Address>,
    /// Address of the stake token contract. The staking token is an ERC-20.
    #[clap(long, env, required = false)]
    pub boundless_stake_token_address: Option<Address>,
    /// URL for the offchain [order stream service].
    ///
    /// [order stream service]: crate::order_stream_client
    #[clap(
        long,
        env,
        required = false,
        long_help = "URL for the offchain order stream service"
    )]
    pub boundless_order_stream_url: Option<Cow<'static, str>>,

    /// Number of transactions to lookback at
    #[clap(long, env, required = false, default_value_t = 5)]
    pub boundless_lookback: u32,

    /// Starting price (wei) per cycle of the proving order
    #[clap(long, env, required = false, default_value = "100000000")]
    pub boundless_cycle_min_wei: U256,
    /// Maximum price (wei) per cycle of the proving order
    #[clap(long, env, required = false, default_value = "200000000")]
    pub boundless_cycle_max_wei: U256,
    /// Duration in seconds for the price to ramp up from min to max.
    #[clap(long, env, required = false, default_value_t = 60)]
    pub boundless_order_ramp_up_period: u32,
    /// Multiplier for order fulfillment timeout (seconds/segment) after locking
    #[clap(long, env, required = false, default_value_t = 3.0)]
    pub boundless_order_lock_timeout_factor: f64,
    /// Multiplier for order expiry timeout (seconds/segment) after creation
    #[clap(long, env, required = false, default_value_t = 10.0)]
    pub boundless_order_timeout_factor: f64,
    /// Time in seconds between attempts to check order status
    #[clap(long, env, required = false, default_value_t = 12)]
    pub boundless_order_check_interval: u64,
}

impl MarketProviderConfig {
    pub fn to_arg_vec(
        &self,
        storage_provider_config: &Option<StorageProviderConfig>,
    ) -> Vec<String> {
        // RPC/Wallet args
        let mut proving_args = vec![
            String::from("--boundless-rpc-url"),
            self.boundless_rpc_url.to_string(),
            String::from("--boundless-wallet-key"),
            self.boundless_wallet_key.to_bytes().to_string(),
        ];
        // Boundless Deployment args
        if let Some(boundless_chain_id) = self.boundless_chain_id {
            proving_args.extend(vec![
                String::from("--boundless-chain-id"),
                boundless_chain_id.to_string(),
            ]);
        };
        if let Some(boundless_market_address) = &self.boundless_market_address {
            proving_args.extend(vec![
                String::from("--boundless-market-address"),
                boundless_market_address.to_string(),
            ]);
        };
        if let Some(boundless_verifier_router_address) = &self.boundless_verifier_router_address {
            proving_args.extend(vec![
                String::from("--boundless-verifier-router-address"),
                boundless_verifier_router_address.to_string(),
            ]);
        };
        if let Some(boundless_set_verifier_address) = &self.boundless_set_verifier_address {
            proving_args.extend(vec![
                String::from("--boundless-set-verifier-address"),
                boundless_set_verifier_address.to_string(),
            ]);
        };
        if let Some(boundless_stake_token_address) = &self.boundless_stake_token_address {
            proving_args.extend(vec![
                String::from("--boundless-stake-token-address"),
                boundless_stake_token_address.to_string(),
            ]);
        };
        if let Some(boundless_order_stream_url) = &self.boundless_order_stream_url {
            proving_args.extend(vec![
                String::from("--boundless-order-stream-url"),
                boundless_order_stream_url.to_string(),
            ]);
        };
        // Proving fee args
        proving_args.extend(vec![
            String::from("--boundless-lookback"),
            self.boundless_lookback.to_string(),
            String::from("--boundless-cycle-min wei"),
            self.boundless_cycle_min_wei.to_string(),
            String::from("--boundless-cycle-max-wei"),
            self.boundless_cycle_max_wei.to_string(),
            String::from("--boundless-order-ramp-up-period"),
            self.boundless_order_ramp_up_period.to_string(),
            String::from("--boundless-order-lock-timeout-factor"),
            self.boundless_order_lock_timeout_factor.to_string(),
            String::from("--boundless-order-timeout-factor"),
            self.boundless_order_timeout_factor.to_string(),
            String::from("--boundless-order-check-interval"),
            self.boundless_order_check_interval.to_string(),
        ]);
        // Storage provider args
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
    market: MarketProviderConfig,
    storage: StorageProviderConfig,
    proof_journal: ProofJournal,
    witness_frames: Vec<Vec<u8>>,
    stitched_proofs: Vec<Receipt>,
    proving_args: &ProvingArgs,
    skip_await_proof: bool,
) -> Result<Receipt, ProvingError> {
    info!("Running boundless client.");
    let journal = Journal::new(proof_journal.encode_packed());

    // Instantiate storage provider
    let storage_provider = StandardStorageProvider::from_config(&storage)
        .context("StandardStorageProvider::from_config")
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;

    // Override deployment configuration if set
    let market_deployment = market
        .boundless_chain_id
        .and_then(Deployment::from_chain_id)
        .or_else(|| {
            let mut builder = Deployment::builder();
            if let Some(boundless_market_address) = market.boundless_market_address {
                builder.boundless_market_address(boundless_market_address);
            };
            if let Some(boundless_verifier_router_address) =
                market.boundless_verifier_router_address
            {
                builder.verifier_router_address(boundless_verifier_router_address);
            };
            if let Some(boundless_set_verifier_address) = market.boundless_set_verifier_address {
                builder.set_verifier_address(boundless_set_verifier_address);
            };
            if let Some(boundless_stake_token_address) = market.boundless_stake_token_address {
                builder.stake_token_address(boundless_stake_token_address);
            };
            if let Some(boundless_order_stream_url) = market.boundless_order_stream_url.clone() {
                builder.order_stream_url(boundless_order_stream_url);
            };
            builder.build().ok()
        });

    // Instantiate client
    let boundless_client = Client::builder()
        .with_private_key(market.boundless_wallet_key)
        .with_rpc_url(market.boundless_rpc_url)
        .with_deployment(market_deployment)
        .with_storage_provider(Some(storage_provider))
        .build()
        .await
        .context("ClientBuilder::build()")
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;

    // Report boundless deployment info
    info!(
        "Using BoundlessMarket at {}",
        boundless_client.deployment.boundless_market_address,
    );
    debug!("Deployment: {:?}", boundless_client.deployment);

    // Set the proof request requirements
    let requirements = Requirements::new(KAILUA_FPVM_ID, Predicate::digest_match(journal.digest()))
        .with_groth16_proof();

    // Check if an unexpired request had already been made recently
    let boundless_wallet_address = boundless_client.signer.as_ref().unwrap().address();
    let boundless_wallet_nonce = boundless_client
        .provider()
        .get_transaction_count(boundless_wallet_address)
        .await
        .context("get_transaction_count boundless_wallet_address")
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))? as u32;

    // Look back at prior transactions to avoid repeated requests
    for i in 0..market.boundless_lookback {
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
            .context("get_status")
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

        if skip_await_proof {
            warn!("Skipping awaiting proof on Boundless and exiting process.");
            std::process::exit(0);
        }

        return retrieve_proof(
            boundless_client,
            request_id,
            market.boundless_order_check_interval,
            request.expires_at(),
        )
        .await
        .context("retrieve_proof")
        .map_err(|e| ProvingError::OtherError(anyhow!(e)));
    }

    // Upload program
    info!("Uploading Kailua binary.");
    let program_url = boundless_client
        .upload_program(KAILUA_FPVM_ELF)
        .await
        .context("Client::upload_program")
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;

    // Preflight execution to get cycle count
    let req_file_name = request_file_name(&proof_journal);
    let cycle_count = match read_bincoded_file::<BoundlessRequest>(&req_file_name).await {
        Ok(request) => request.cycle_count,
        Err(err) => {
            warn!("Preflighting execution: {err:?}");
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
            .context("spawn_blocking")
            .map_err(|e| ProvingError::OtherError(anyhow!(e)))?
            .map_err(|e| ProvingError::ExecutionError(anyhow!(e)))?;
            let cycle_count = session_info
                .segments
                .iter()
                .map(|segment| 1 << segment.po2)
                .sum::<u64>();
            let cached_data = BoundlessRequest { cycle_count };
            if let Err(err) = save_to_bincoded_file(&cached_data, &req_file_name).await {
                warn!("Failed to cache cycle count data: {err:?}");
            }
            cycle_count
        }
    };

    // Pass in input frames
    let mut guest_env_builder = GuestEnv::builder();
    for frame in &witness_frames {
        guest_env_builder = guest_env_builder.write_frame(frame);
    }
    // Pass in proofs
    for proof in &stitched_proofs {
        guest_env_builder = guest_env_builder
            .write(proof)
            .context("GuestEnvBuilder::write")
            .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    }
    // Build input vector
    let input = guest_env_builder
        .build_vec()
        .context("GuestEnvBuilder::build_vec")
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;

    // Upload input
    info!("Uploading input data.");
    let input_url = boundless_client
        .upload_input(&input)
        .await
        .context("Client::upload_input")
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?;
    sleep(Duration::from_secs(2)).await;

    // Build final request
    let segment_count = cycle_count.div_ceil(1 << 20) as f64;
    let cycles = U256::from(cycle_count);
    let min_price = market.boundless_cycle_min_wei * cycles;
    let max_price = market.boundless_cycle_max_wei * cycles;
    let request = boundless_client
        .new_request()
        .with_journal(journal)
        .with_cycles(cycle_count)
        .with_program_url(program_url)
        .context("RequestParams::with_program_url")
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?
        .with_input_url(input_url)
        .context("RequestParams::with_input_url")
        .map_err(|e| ProvingError::OtherError(anyhow!(e)))?
        .with_requirements(requirements)
        .with_offer(
            OfferParams::builder()
                .min_price(min_price)
                .max_price(max_price)
                .ramp_up_period(market.boundless_order_ramp_up_period)
                .lock_stake(max_price)
                .lock_timeout((market.boundless_order_lock_timeout_factor * segment_count) as u32)
                .timeout((market.boundless_order_timeout_factor * segment_count) as u32)
                .build()
                .context("OfferParamsBuilder::build()")
                .map_err(|e| ProvingError::OtherError(anyhow!(e)))?,
        )
        .with_request_id(RequestId::new(
            boundless_wallet_address,
            boundless_wallet_nonce,
        ));

    // Send the request and wait for it to be completed.
    let (request_id, expires_at) = if market.boundless_order_stream_url.is_some() {
        info!("Submitting offchain request.");
        boundless_client
            .submit_offchain(request.clone())
            .await
            .context("Client::submit_offchain()")
            .map_err(|e| ProvingError::OtherError(anyhow!(e)))?
    } else {
        info!("Submitting onchain request.");
        boundless_client
            .submit_onchain(request.clone())
            .await
            .context("Client::submit_onchain()")
            .map_err(|e| ProvingError::OtherError(anyhow!(e)))?
    };
    info!("Boundless request 0x{request_id:x} submitted");

    if skip_await_proof {
        warn!("Skipping awaiting proof on Boundless and exiting process.");
        std::process::exit(0);
    }

    retrieve_proof(
        boundless_client,
        request_id,
        market.boundless_order_check_interval,
        expires_at,
    )
    .await
    .context("retrieve_proof")
    .map_err(|e| ProvingError::OtherError(anyhow!(e)))
}

pub async fn retrieve_proof(
    boundless_client: Client,
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

pub fn request_file_name(
    proof_journal: &ProofJournal,
    // boundless_chain_id: u64,
    // boundless_market_address: Address,
    // set_verifier_address: Address,
    // verifier_router_address: Option<Address>,
) -> String {
    let journal_hash = keccak256(proof_journal.encode_packed());
    // todo: integrate market config
    // let verifier_router_address = verifier_router_address.unwrap_or_default();
    // let data = [
    //     journal_hash.0.as_slice(),
    //     boundless_chain_id.to_be_bytes().as_slice(),
    //     boundless_market_address.0.as_slice(),
    //     set_verifier_address.0.as_slice(),
    //     verifier_router_address.as_slice()
    // ].concat();
    format!("boundless-{}.req", journal_hash)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BoundlessRequest {
    /// Number of cycles that require proving
    pub cycle_count: u64,
}
