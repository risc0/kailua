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

use alloy::contract::SolCallBuilder;
use alloy::network::{Network, TransactionBuilder};
use alloy::primitives::{Address, FixedBytes, Uint, B256, U256};
use alloy::providers::{Provider, ReqwestProvider};
use alloy::transports::Transport;
use anyhow::{bail, Context};
use std::ops::{Div, Sub};
// use kailua_contracts::KailuaGame::KailuaGameInstance;
use alloy::consensus::{Blob, BlobTransactionSidecar};
use alloy::eips::eip4844::{BLS_MODULUS, FIELD_ELEMENTS_PER_BLOB};
use kailua_contracts::Safe::SafeInstance;
use std::str::FromStr;
use tracing::debug;

// pub mod bench;
pub mod blob_provider;
pub mod channel;
pub mod deploy;
// pub mod fault;
// pub mod proposal;
// pub mod propose;
// pub mod validate;

pub const KAILUA_GAME_TYPE: u32 = 1337;

#[derive(clap::Parser, Debug, Clone)]
#[command(name = "kailua-cli")]
#[command(bin_name = "kailua-cli")]
#[command(author, version, about, long_about = None)]
pub enum Cli {
    Deploy(deploy::DeployArgs),
    // Propose(propose::ProposeArgs),
    // Validate(validate::ValidateArgs),
    // TestFault(fault::FaultArgs),
    // Benchmark(bench::BenchArgs),
}

impl Cli {
    pub fn verbosity(&self) -> u8 {
        match self {
            Cli::Deploy(args) => args.v,
            // Cli::Propose(args) => args.v,
            // Cli::Validate(args) => args.v,
            // Cli::TestFault(args) => args.propose_args.v,
            // Cli::Benchmark(args) => args.v,
        }
    }
}

pub async fn exec_safe_txn<
    T: Transport + Clone,
    P1: Provider<T, N>,
    P2: Provider<T, N>,
    C,
    N: Network,
>(
    txn: SolCallBuilder<T, P1, C, N>,
    safe: &SafeInstance<T, P2, N>,
    from: Address,
) -> anyhow::Result<()> {
    let req = txn.into_transaction_request();
    let value = req.value().unwrap_or_default();
    safe.execTransaction(
        req.to().unwrap(),
        value,
        req.input().cloned().unwrap_or_default(),
        0,
        Uint::from(req.gas_limit().unwrap_or_default()),
        U256::ZERO,
        U256::ZERO,
        Address::ZERO,
        Address::ZERO,
        [
            [0u8; 12].as_slice(),
            from.as_slice(),
            [0u8; 32].as_slice(),
            [1u8].as_slice(),
        ]
        .concat()
        .into(),
    )
    .send()
    .await?
    .get_receipt()
    .await?;
    Ok(())
}

pub async fn output_at_block(
    op_node_provider: &ReqwestProvider,
    output_block_number: u64,
) -> anyhow::Result<FixedBytes<32>> {
    let output_at_block: serde_json::Value = op_node_provider
        .client()
        .request(
            "optimism_outputAtBlock",
            (format!("0x{:x}", output_block_number),),
        )
        .await
        .context(format!("optimism_outputAtBlock {output_block_number}"))?;
    debug!("optimism_outputAtBlock {:?}", &output_at_block);
    Ok(FixedBytes::<32>::from_str(
        output_at_block["outputRoot"].as_str().unwrap(),
    )?)
}

pub async fn block_hash(
    l2_node_provider: &ReqwestProvider,
    block_number: u64,
) -> anyhow::Result<FixedBytes<32>> {
    let block: serde_json::Value = l2_node_provider
        .client()
        .request(
            "eth_getBlockByNumber",
            (format!("0x{:x}", block_number), false),
        )
        .await
        .context(format!("eth_getBlockByNumber {block_number}"))?;
    debug!("block_hash {:?}", &block);
    Ok(FixedBytes::<32>::from_str(
        block["hash"].as_str().expect("Failed to parse block hash"),
    )?)
}

pub fn blob_sidecar(blob: Blob) -> anyhow::Result<BlobTransactionSidecar> {
    let c_kzg_blob = c_kzg::Blob::from_bytes(blob.as_slice())?;
    let settings = alloy::consensus::EnvKzgSettings::default();
    let commitment = c_kzg::KzgCommitment::blob_to_kzg_commitment(&c_kzg_blob, settings.get())
        .expect("Failed to convert blob to commitment");
    let proof = c_kzg::KzgProof::compute_blob_kzg_proof(
        &c_kzg_blob,
        &commitment.to_bytes(),
        settings.get(),
    )?;
    Ok(BlobTransactionSidecar::new(
        vec![blob],
        vec![commitment.to_bytes().into_inner().into()],
        vec![proof.to_bytes().into_inner().into()],
    ))
}

pub fn reverse_bits(index: u128, order_po2: u32) -> u128 {
    index.reverse_bits() >> (u128::BITS - order_po2)
}

pub const PRIMITIVE_ROOT_OF_UNITY: U256 = U256::from_limbs([7, 0, 0, 0]);
// primitive_root = 7
// bls_mod = 52435875175126190479447740508185965837690552500527637822603658699938581184513
// pow(primitive_root, (bls_mod - 1) // (2 ** 12), bls_mod)
// 39033254847818212395286706435128746857159659164139250548781411570340225835782
pub const FE_ORDER_PO2: u32 = 12;

pub fn root_of_unity(index: usize) -> U256 {
    let primitive_root_exponent = BLS_MODULUS
        .sub(U256::from(1))
        .div(U256::from(FIELD_ELEMENTS_PER_BLOB));
    let root = PRIMITIVE_ROOT_OF_UNITY.pow_mod(primitive_root_exponent, BLS_MODULUS);
    let root_exponent = reverse_bits(index as u128, FE_ORDER_PO2);
    root.pow_mod(U256::from(root_exponent), BLS_MODULUS)
}

pub fn blob_fe_proof(
    blob: &Blob,
    index: usize,
) -> anyhow::Result<(c_kzg::Bytes48, c_kzg::Bytes32)> {
    let bytes = root_of_unity(index).to_be_bytes();
    let z = c_kzg::Bytes32::new(bytes);
    let c_kzg_blob = c_kzg::Blob::from_bytes(blob.as_slice())?;
    let settings = alloy::consensus::EnvKzgSettings::default();
    let (proof, value) = c_kzg::KzgProof::compute_kzg_proof(&c_kzg_blob, &z, settings.get())?;

    let commitment = c_kzg::KzgCommitment::blob_to_kzg_commitment(&c_kzg_blob, settings.get())?;

    let proof_bytes = proof.to_bytes();
    if c_kzg::KzgProof::verify_kzg_proof(
        &commitment.to_bytes(),
        &z,
        &value,
        &proof_bytes,
        settings.get(),
    )? {
        Ok((proof_bytes, value))
    } else {
        bail!("Generated invalid kzg proof.")
    }
}

// pub async fn derive_expected_journal<T: Transport + Clone, P: Provider<T, N>, N: Network>(
//     game_contract: &KailuaGameInstance<T, P, N>,
//     output_number: u32,
//     safe_output: B256,
//     proposed_output: B256,
//     computed_output: B256
// ) -> anyhow::Result<Vec<u8>> {
//     let l1_head = game_contract.l1Head().call().await?.l1Head_.0;
//     let parent_contract_address = game_contract.parentGame().call().await?.parentGame_;
//     let parent_contract =
//         KailuaGameInstance::new(parent_contract_address, game_contract.provider());
//     let l2_output_root = parent_contract.rootClaim().call().await?.rootClaim_.0;
//     let l2_claim = game_contract.rootClaim().call().await?.rootClaim_.0;
//     let l2_claim_block = game_contract
//         .l2BlockNumber()
//         .call()
//         .await?
//         .l2BlockNumber_
//         .to::<u64>()
//         .to_be_bytes();
//     let config_hash = game_contract.configHash().call().await?.configHash_.0;
//     Ok([
//         l1_head.as_slice(),
//         l2_output_root.as_slice(),
//         l2_claim.as_slice(),
//         l2_claim_block.as_slice(),
//         config_hash.as_slice(),
//     ]
//     .concat())
// }

pub fn hash_to_fe(mut hash: B256) -> B256 {
    hash.0[0] &= u8::MAX >> 2;
    hash
}
