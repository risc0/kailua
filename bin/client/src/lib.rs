// Copyright 2024 RISC Zero, Inc.
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

pub mod oracle;
pub mod proof;
pub mod witness;

use crate::proof::Proof;
use crate::witness::{BlobWitnessProvider, OracleWitnessProvider};
use alloy::transports::http::reqwest::Url;
use alloy_primitives::utils::parse_ether;
use alloy_primitives::{Address, B256};
use anyhow::{ensure, Context};
use boundless_market::alloy::signers::local::PrivateKeySigner;
use boundless_market::client::ClientBuilder;
use boundless_market::contracts::{Input, Offer, Predicate, ProofRequest, Requirements};
use boundless_market::storage::{StorageProviderConfig, StorageProviderType};
use clap::Parser;
use kailua_build::{KAILUA_FPVM_ELF, KAILUA_FPVM_ID};
use kailua_common::blobs::BlobWitnessData;
use kailua_common::journal::ProofJournal;
use kailua_common::oracle::OracleWitnessData;
use kailua_common::witness::Witness;
use kona_preimage::{HintWriterClient, PreimageOracleClient};
use kona_proof::l1::OracleBlobProvider;
use kona_proof::{BootInfo, CachingOracle};
use risc0_zkvm::sha::Digestible;
use risc0_zkvm::{default_executor, default_prover, ExecutorEnv, ProverOpts};
use std::fmt::Debug;
use std::ops::DerefMut;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::task::spawn_blocking;
use tracing::info;

/// The size of the LRU cache in the oracle.
pub const ORACLE_LRU_SIZE: usize = 1024;

/// The client binary CLI application arguments.
#[derive(Parser, Clone, Debug)]
pub struct KailuaClientCli {
    #[arg(long, action = clap::ArgAction::Count, env)]
    pub kailua_verbosity: u8,

    #[clap(long, value_parser = parse_b256, env)]
    pub precondition_validation_data_hash: Option<B256>,

    #[clap(flatten)]
    pub boundless_args: Option<BoundlessArgs>,
}

#[derive(Parser, Debug, Clone)]
#[group(requires_all = ["boundless_rpc_url", "boundless_wallet_key", "boundless_set_verifier_address", "boundless_market_address"])]
pub struct BoundlessArgs {
    /// URL of the Ethereum RPC endpoint.
    #[clap(long, env)]
    #[arg(required = false)]
    pub boundless_rpc_url: Url,
    /// Private key used to interact with the EvenNumber contract.
    #[clap(long, env)]
    #[arg(required = false)]
    pub boundless_wallet_key: PrivateKeySigner,
    /// Submit the request offchain via the provided order stream service url.
    #[clap(long, requires = "boundless_order_stream_url")]
    pub boundless_offchain: bool,
    /// Offchain order stream service URL to submit offchain requests to.
    #[clap(long, env)]
    pub boundless_order_stream_url: Option<Url>,
    /// Storage provider to use
    #[clap(flatten)]
    pub boundless_storage_config: Option<StorageProviderConfig>,
    /// Address of the RiscZeroSetVerifier contract.
    #[clap(long, env)]
    #[arg(required = false)]
    pub boundless_set_verifier_address: Address,
    /// Address of the BoundlessMarket contract.
    #[clap(long, env)]
    #[arg(required = false)]
    pub boundless_market_address: Address,
}

impl BoundlessArgs {
    pub fn to_arg_vec(&self) -> Vec<String> {
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
        if let Some(storage_cfg) = &self.boundless_storage_config {
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

pub fn parse_b256(s: &str) -> Result<B256, String> {
    B256::from_str(s).map_err(|_| format!("Invalid B256 value: {}", s))
}

pub async fn run_client<P, H>(
    boundless_args: Option<BoundlessArgs>,
    oracle_client: P,
    hint_client: H,
    precondition_validation_data_hash: B256,
) -> anyhow::Result<()>
where
    P: PreimageOracleClient + Send + Sync + Debug + Clone + 'static,
    H: HintWriterClient + Send + Sync + Debug + Clone + 'static,
{
    // preload all data natively
    info!("Running native client.");
    let (_, witness) = run_native_client(
        oracle_client.clone(),
        hint_client.clone(),
        precondition_validation_data_hash,
    )
    .await
    .expect("Failed to run native client.");
    // compute the receipt in the zkvm
    info!("Running zk client.");
    let proof = match boundless_args {
        None => run_zkvm_client(witness)
            .await
            .context("Failed to run zkvm client.")?,
        Some(args) => run_boundless_client(args, witness)
            .await
            .context("Failed to run boundless client.")?,
    };
    // Prepare proof file
    let proof_journal = ProofJournal::decode_packed(proof.journal().as_ref())
        .expect("Failed to decode proof output");
    let mut output_file = File::create(proof::fpvm_proof_file_name(
        proof_journal.precondition_output,
        proof_journal.l1_head,
        proof_journal.claimed_l2_output_root,
        proof_journal.claimed_l2_block_number,
        proof_journal.agreed_l2_output_root,
    ))
    .await
    .expect("Failed to create proof output file");
    // Write proof data to file
    let proof_bytes = bincode::serialize(&proof).expect("Could not serialize proof.");
    output_file
        .write_all(proof_bytes.as_slice())
        .await
        .expect("Failed to write proof to file");
    output_file
        .flush()
        .await
        .expect("Failed to flush proof output file data.");

    Ok(())
}

pub async fn run_native_client<P, H>(
    oracle_client: P,
    hint_client: H,
    precondition_validation_data_hash: B256,
) -> anyhow::Result<(B256, Witness)>
where
    P: PreimageOracleClient + Send + Sync + Debug + Clone,
    H: HintWriterClient + Send + Sync + Debug + Clone,
{
    let oracle_witness = Arc::new(Mutex::new(OracleWitnessData::default()));
    let blobs_witness = Arc::new(Mutex::new(BlobWitnessData::default()));
    info!("Preamble");
    let oracle = Arc::new(OracleWitnessProvider {
        oracle: CachingOracle::new(ORACLE_LRU_SIZE, oracle_client, hint_client),
        witness: oracle_witness.clone(),
    });
    let boot = Arc::new(
        BootInfo::load(oracle.as_ref())
            .await
            .context("BootInfo::load")?,
    );
    let beacon = BlobWitnessProvider {
        provider: OracleBlobProvider::new(oracle.clone()),
        witness: blobs_witness.clone(),
    };
    // Run client
    let (precondition_hash, real_output_hash) = kailua_common::client::run_client(
        precondition_validation_data_hash,
        oracle,
        boot.clone(),
        beacon,
    )?;
    // Check output
    if let Some(computed_output) = real_output_hash {
        // With sufficient data, the input l2_claim must be true
        assert_eq!(boot.claimed_l2_output_root, computed_output);
    } else {
        // We use the zero claim hash to denote that the data as of l1 head is insufficient
        assert_eq!(boot.claimed_l2_output_root, B256::ZERO);
    }
    let witness = Witness {
        oracle_witness: core::mem::take(oracle_witness.lock().unwrap().deref_mut()),
        blobs_witness: core::mem::take(blobs_witness.lock().unwrap().deref_mut()),
        precondition_validation_data_hash,
    };
    Ok((precondition_hash, witness))
}

pub async fn run_zkvm_client(witness: Witness) -> anyhow::Result<Proof> {
    let prove_info = spawn_blocking(move || {
        let data = rkyv::to_bytes::<rkyv::rancor::Error>(&witness)?.to_vec();
        // Execution environment
        let env = ExecutorEnv::builder()
            // Pass in witness data
            .write_frame(&data)
            .build()?;
        let prover = default_prover();
        let prove_info = prover
            .prove_with_opts(env, KAILUA_FPVM_ELF, &ProverOpts::groth16())
            .context("prove_with_opts")?;
        Ok::<_, anyhow::Error>(prove_info)
    })
    .await??;

    info!(
        "Proof of {} total cycles ({} user cycles) computed.",
        prove_info.stats.total_cycles, prove_info.stats.user_cycles
    );
    prove_info
        .receipt
        .verify(KAILUA_FPVM_ID)
        .context("receipt verification")?;
    info!("Receipt verified.");

    Ok(Proof::ZKVMReceipt(Box::new(prove_info.receipt)))
}

pub async fn run_boundless_client(args: BoundlessArgs, witness: Witness) -> anyhow::Result<Proof> {
    let input = rkyv::to_bytes::<rkyv::rancor::Error>(&witness)?.to_vec();
    // Preflight execution to get journal
    let env = ExecutorEnv::builder()
        // Pass in witness data
        .write_frame(&input)
        .build()?;
    let session_info = default_executor().execute(env, KAILUA_FPVM_ELF)?;
    let mcycles_count = session_info
        .segments
        .iter()
        .map(|segment| 1 << segment.po2)
        .sum::<u64>()
        .div_ceil(1_000_000);
    let journal = session_info.journal;
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
        .with_storage_provider_config(args.boundless_storage_config)
        .with_private_key(args.boundless_wallet_key)
        .build()
        .await?;
    // Upload the ELF to the storage provider so that it can be fetched by the market.
    ensure!(
        boundless_client.storage_provider.is_some(),
        "a storage provider is required to upload the zkVM guest ELF"
    );
    let image_url = boundless_client.upload_image(KAILUA_FPVM_ELF).await?;
    info!("Uploaded image to {}", image_url);
    // Upload input
    let input_url = boundless_client.upload_input(&input).await?;
    tracing::info!("Uploaded input to {input_url}");
    let request_input = Input::url(input_url);
    let request = ProofRequest::default()
        .with_image_url(&image_url)
        .with_input(request_input)
        .with_requirements(Requirements::new(
            KAILUA_FPVM_ID,
            Predicate::digest_match(journal.digest()),
        ))
        .with_offer(
            Offer::default()
                .with_min_price_per_mcycle(parse_ether("0.001")?, mcycles_count)
                .with_max_price_per_mcycle(parse_ether("0.002")?, mcycles_count)
                .with_timeout(1000),
        );

    // Send the request and wait for it to be completed.
    let (request_id, expires_at) = boundless_client.submit_request(&request).await?;
    info!("Boundless request 0x{request_id:x} submitted");
    // Wait for the request to be fulfilled by the market, returning the journal and seal.
    info!("Waiting for 0x{request_id:x} to be fulfilled");
    let (_journal, seal) = boundless_client
        .wait_for_request_fulfillment(request_id, Duration::from_secs(5), expires_at)
        .await?;
    info!("Request 0x{request_id:x} fulfilled");

    Ok(Proof::BoundlessSeal(seal.to_vec(), journal))
}
