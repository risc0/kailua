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

use crate::providers::optimism::OpNodeProvider;
use crate::stall::Stall;
use crate::{BN254_CONTROL_ID, CONTROL_ROOT, KAILUA_GAME_TYPE, SET_BUILDER_ID};
use alloy::network::{EthereumWallet, Network, TxSigner};
use alloy::primitives::{Address, Bytes, Uint, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::signers::local::LocalSigner;
use alloy::sol_types::SolValue;
use alloy::transports::Transport;
use anyhow::{bail, Context};
use kailua_build::KAILUA_FPVM_ID;
use kailua_common::client::config_hash;
use kailua_contracts::*;
use kailua_host::fetch_rollup_config;
use std::process::exit;
use std::str::FromStr;
use tracing::{error, info};

#[derive(clap::Args, Debug, Clone)]
pub struct FastTrackArgs {
    #[arg(long, short, help = "Verbosity level (0-4)", action = clap::ArgAction::Count)]
    pub v: u8,

    /// Address of the OP-NODE endpoint to use
    #[clap(long, env)]
    pub op_node_url: String,
    /// Address of the OP-GETH endpoint to use (eth and debug namespace required).
    #[clap(long, env)]
    pub op_geth_url: String,
    /// Address of the ethereum rpc endpoint to use (eth namespace required)
    #[clap(long, env)]
    pub eth_rpc_url: String,

    /// The l2 block number to start sequencing since
    #[clap(long, env)]
    pub starting_block_number: u64,
    /// The number of blocks that a proposal must cover
    #[clap(long, env)]
    pub proposal_block_span: u64,
    /// The time gap before a proposal can be made
    #[clap(long, env)]
    pub proposal_time_gap: u64,

    /// The collateral (wei) that must be locked up by a sequencer to propose
    #[clap(long, env)]
    pub collateral_amount: u128,
    /// Address of the existing L1 `RiscZeroVerifier` contract to use
    #[clap(long, env)]
    pub verifier_contract: Option<String>,
    /// The timeout after which a counter-proposal can not be made
    #[clap(long, env)]
    pub challenge_timeout: u64,

    /// Secret key of L1 wallet to use for deploying contracts
    #[clap(long, env)]
    pub deployer_key: String,
    /// Secret key of L1 wallet that (indirectly) owns `DisputeGameFactory`
    #[clap(long, env)]
    pub owner_key: String,
    /// Secret key of L1 guardian wallet
    #[clap(long, env, required_if_eq("respect_kailua_proposals", "true"))]
    pub guardian_key: Option<String>,

    /// Whether to set Kailua as the OptimismPortal's respected game type
    #[clap(long, env)]
    pub respect_kailua_proposals: bool,
}

pub async fn fast_track(args: FastTrackArgs) -> anyhow::Result<()> {
    let op_node_provider =
        OpNodeProvider(ProviderBuilder::new().on_http(args.op_node_url.as_str().try_into()?));
    let eth_rpc_provider = ProviderBuilder::new().on_http(args.eth_rpc_url.as_str().try_into()?);

    info!("Fetching rollup configuration from rpc endpoints.");
    // fetch rollup config
    let config = fetch_rollup_config(&args.op_node_url, &args.op_geth_url, None)
        .await
        .context("fetch_rollup_config")?;
    let rollup_config_hash = config_hash(&config).expect("Configuration hash derivation error");
    info!("RollupConfigHash({})", hex::encode(rollup_config_hash));

    // load system config
    let system_config = SystemConfig::new(config.l1_system_config_address, &eth_rpc_provider);
    let portal_address = system_config.optimismPortal().stall().await.addr_;
    let dgf_address = system_config.disputeGameFactory().stall().await.addr_;

    // initialize owner wallet
    info!("Initializing owner wallet.");
    let owner_signer = LocalSigner::from_str(&args.owner_key)?;
    let owner_wallet = EthereumWallet::from(owner_signer);
    let owner_provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(&owner_wallet)
        .on_http(args.eth_rpc_url.as_str().try_into()?);

    // Init factory contract
    let dispute_game_factory = IDisputeGameFactory::new(dgf_address, &owner_provider);
    info!("DisputeGameFactory({:?})", dispute_game_factory.address());
    let game_count = dispute_game_factory.gameCount().stall().await.gameCount_;
    info!("There have been {game_count} games created using DisputeGameFactory");
    let dispute_game_factory_ownable = OwnableUpgradeable::new(dgf_address, &owner_provider);
    let factory_owner_address = dispute_game_factory_ownable.owner().stall().await._0;
    let factory_owner_safe = Safe::new(factory_owner_address, &owner_provider);
    info!("Safe({:?})", factory_owner_safe.address());
    let safe_owners = factory_owner_safe.getOwners().stall().await._0;
    info!("Safe::owners({:?})", &safe_owners);
    let owner_address = owner_wallet.default_signer().address();
    if safe_owners.first().unwrap() != &owner_address {
        error!("Incorrect owner key.");
        exit(2);
    } else if safe_owners.len() != 1 {
        error!("Expected exactly one owner of safe account.");
        exit(1);
    }

    // initialize deployment wallet
    info!("Initializing deployer wallet.");
    let deployer_signer = LocalSigner::from_str(&args.deployer_key)?;
    let deployer_wallet = EthereumWallet::from(deployer_signer);
    let deployer_provider = ProviderBuilder::new()
        .with_recommended_fillers()
        .wallet(&deployer_wallet)
        .on_http(args.eth_rpc_url.as_str().try_into()?);

    // Deploy or reuse existing RISCZeroVerifier contracts
    let verifier_contract_address = match &args.verifier_contract {
        None => deploy_verifier(&deployer_provider, &owner_provider, owner_address)
            .await
            .context("deploy_verifier")?,
        Some(address) => Address::from_str(address)?,
    };

    // Deploy KailuaTreasury contract
    info!("Deploying KailuaTreasury contract to L1 rpc.");
    let kailua_treasury_implementation = KailuaTreasury::deploy(
        &deployer_provider,
        verifier_contract_address,
        bytemuck::cast::<[u32; 8], [u8; 32]>(KAILUA_FPVM_ID).into(),
        rollup_config_hash.into(),
        Uint::from(args.proposal_block_span),
        KAILUA_GAME_TYPE,
        dgf_address,
    )
    .await
    .context("KailuaTreasury implementation contract deployment error")?;
    info!("{:?}", &kailua_treasury_implementation);

    // Update dispute factory implementation to KailuaTreasury
    info!("Setting KailuaTreasury initialization bond value in DisputeGameFactory to zero.");
    crate::exec_safe_txn(
        dispute_game_factory.setInitBond(KAILUA_GAME_TYPE, U256::ZERO),
        &factory_owner_safe,
        owner_address,
    )
    .await
    .context("setInitBond 0 wei")?;
    assert_eq!(
        dispute_game_factory
            .initBonds(KAILUA_GAME_TYPE)
            .stall()
            .await
            .bond_,
        U256::ZERO
    );
    info!("Setting KailuaTreasury participation bond value to 1 wei.");
    let bond_value = U256::from(1);
    crate::exec_safe_txn(
        kailua_treasury_implementation.setParticipationBond(bond_value),
        &factory_owner_safe,
        owner_address,
    )
    .await
    .context("setParticipationBond 1 wei")?;
    assert_eq!(
        kailua_treasury_implementation
            .participationBond()
            .stall()
            .await
            ._0,
        bond_value
    );

    info!("Setting KailuaTreasury implementation address in DisputeGameFactory.");
    crate::exec_safe_txn(
        dispute_game_factory
            .setImplementation(KAILUA_GAME_TYPE, *kailua_treasury_implementation.address()),
        &factory_owner_safe,
        owner_address,
    )
    .await
    .context("setImplementation KailuaTreasury")?;
    assert_eq!(
        dispute_game_factory
            .gameImpls(KAILUA_GAME_TYPE)
            .stall()
            .await
            .impl_,
        *kailua_treasury_implementation.address()
    );

    // Create new treasury instance from target block number
    let root_claim = op_node_provider
        .output_at_block(args.starting_block_number)
        .await?;
    let extra_data = Bytes::from(args.starting_block_number.abi_encode_packed());
    info!(
        "Creating new KailuaTreasury game instance from {} ({}).",
        args.starting_block_number, root_claim
    );
    crate::exec_safe_txn(
        dispute_game_factory.create(KAILUA_GAME_TYPE, root_claim, extra_data.clone()),
        &factory_owner_safe,
        owner_address,
    )
    .await
    .context("create KailuaTreasury")?;
    let kailua_treasury_instance_address = dispute_game_factory
        .games(KAILUA_GAME_TYPE, root_claim, extra_data)
        .stall()
        .await
        .proxy_;
    let kailua_treasury_instance =
        KailuaTreasury::new(kailua_treasury_instance_address, &owner_provider);
    info!("{:?}", &kailua_treasury_instance);
    let status = kailua_treasury_instance.status().stall().await._0;
    if status == 0 {
        info!("Resolving KailuaTreasury instance");
        crate::exec_safe_txn(
            kailua_treasury_instance.resolve(),
            &factory_owner_safe,
            owner_address,
        )
        .await
        .context("resolve KailuaTreasury")?;
    } else {
        info!("Game instance is not ongoing ({status})");
    }

    // Deploy KailuaGame contract
    info!("Deploying KailuaGame contract to L1 rpc.");
    let kailua_game_contract = KailuaGame::deploy(
        &deployer_provider,
        *kailua_treasury_implementation.address(),
        verifier_contract_address,
        bytemuck::cast::<[u32; 8], [u8; 32]>(KAILUA_FPVM_ID).into(),
        rollup_config_hash.into(),
        Uint::from(args.proposal_block_span),
        KAILUA_GAME_TYPE,
        dgf_address,
        U256::from(config.genesis.l2_time),
        U256::from(config.block_time),
        U256::from(args.proposal_time_gap),
        args.challenge_timeout,
    )
    .await
    .context("KailuaGame contract deployment error")?;
    info!("{:?}", &kailua_game_contract);

    // Update implementation to KailuaGame
    info!("Setting KailuaGame implementation address in DisputeGameFactory.");
    crate::exec_safe_txn(
        dispute_game_factory.setImplementation(KAILUA_GAME_TYPE, *kailua_game_contract.address()),
        &factory_owner_safe,
        owner_address,
    )
    .await
    .context("setImplementation KailuaGame")?;

    // Update the respectedGameType as the guardian
    if args.respect_kailua_proposals {
        // initialize guardian wallet
        info!("Initializing guardian wallet.");
        let guardian_signer = LocalSigner::from_str(&args.guardian_key.unwrap())?;
        let guardian_address = guardian_signer.address();
        let guardian_wallet = EthereumWallet::from(guardian_signer);
        let guardian_provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .wallet(&guardian_wallet)
            .on_http(args.eth_rpc_url.as_str().try_into()?);
        let optimism_portal = OptimismPortal2::new(portal_address, &guardian_provider);
        let portal_guardian_address = optimism_portal.guardian().stall().await._0;
        if portal_guardian_address != guardian_address {
            bail!("OptimismPortal2 Guardian is {portal_guardian_address}. Provided private key has account address {guardian_address}.");
        }

        info!("Setting respectedGameType in OptimismPortal2.");
        optimism_portal
            .setRespectedGameType(KAILUA_GAME_TYPE)
            .send()
            .await
            .context("setImplementation KailuaGame")?
            .get_receipt()
            .await?;
    }

    info!("Kailua upgrade complete.");
    Ok(())
}

pub async fn deploy_verifier<
    T: Transport + Clone,
    P1: Provider<T, N>,
    P2: Provider<T, N>,
    N: Network,
>(
    deployer_provider: P1,
    owner_provider: P2,
    owner_address: Address,
) -> anyhow::Result<Address> {
    // Deploy verifier router contract
    info!("Deploying RiscZeroVerifierRouter contract to L1 under ownership of {owner_address}.");
    let verifier_contract = RiscZeroVerifierRouter::deploy(&deployer_provider, owner_address)
        .await
        .context("RiscZeroVerifierRouter contract deployment error")?;
    let verifier_contract_address = *verifier_contract.address();
    let verifier_contract = RiscZeroVerifierRouter::new(verifier_contract_address, &owner_provider);

    // Deploy RiscZeroGroth16Verifier contract
    info!("Deploying RiscZeroGroth16Verifier contract to L1.");
    let groth16_verifier_contract =
        RiscZeroGroth16Verifier::deploy(&deployer_provider, CONTROL_ROOT, BN254_CONTROL_ID)
            .await
            .context("RiscZeroGroth16Verifier contract deployment error")?;
    info!("{:?}", &groth16_verifier_contract);
    let selector = groth16_verifier_contract.SELECTOR().stall().await._0;
    info!("Adding RiscZeroGroth16Verifier contract to RiscZeroVerifierRouter.");
    verifier_contract
        .addVerifier(selector, *groth16_verifier_contract.address())
        .send()
        .await
        .context("addVerifier RiscZeroGroth16Verifier (send)")?
        .get_receipt()
        .await
        .context("addVerifier RiscZeroGroth16Verifier (get_receipt)")?;

    // Deploy RiscZeroSetVerifier contract
    info!("Deploying RiscZeroSetVerifier contract to L1.");
    let set_verifier_contract = RiscZeroSetVerifier::deploy(
        &deployer_provider,
        verifier_contract_address,
        SET_BUILDER_ID,
        String::default(),
    )
    .await
    .context("RiscZeroSetVerifier contract deployment error")?;
    info!("{:?}", &set_verifier_contract);
    let selector = set_verifier_contract.SELECTOR().stall().await._0;
    info!("Adding RiscZeroSetVerifier contract to RiscZeroVerifierRouter.");
    verifier_contract
        .addVerifier(selector, *set_verifier_contract.address())
        .send()
        .await
        .context("addVerifier RiscZeroSetVerifier (send)")?
        .get_receipt()
        .await
        .context("addVerifier RiscZeroSetVerifier (get_receipt)")?;

    // Deploy mock verifier
    #[cfg(feature = "devnet")]
    if risc0_zkvm::is_dev_mode() {
        // Deploy MockVerifier contract
        tracing::warn!("Deploying RiscZeroMockVerifier contract to L1. This will accept fake proofs which are not cryptographically secure!");
        let mock_verifier_contract =
            RiscZeroMockVerifier::deploy(&deployer_provider, [0u8; 4].into())
                .await
                .context("RiscZeroMockVerifier contract deployment error")?;
        tracing::warn!("{:?}", &mock_verifier_contract);
        tracing::warn!("Adding RiscZeroMockVerifier contract to RiscZeroVerifierRouter.");
        verifier_contract
            .addVerifier([0u8; 4].into(), *mock_verifier_contract.address())
            .send()
            .await
            .context("addVerifier RiscZeroMockVerifier (send)")?
            .get_receipt()
            .await
            .context("addVerifier RiscZeroMockVerifier (get_receipt)")?;
    }

    Ok(verifier_contract_address)
}
