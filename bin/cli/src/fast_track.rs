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

use crate::stall::Stall;
use crate::transact::signer::{DeployerSignerArgs, GuardianSignerArgs, OwnerSignerArgs};
use crate::transact::{Transact, TransactArgs};
use crate::{retry_with_context, KAILUA_GAME_TYPE};
use alloy::network::{Ethereum, Network, ReceiptResponse, TxSigner};
use alloy::primitives::{Address, Bytes, Uint, U256};
use alloy::providers::{Provider, RootProvider};
use alloy::sol_types::SolValue;
use anyhow::{anyhow, bail, Context};
use kailua_build::KAILUA_FPVM_ID;
use kailua_client::provider::OpNodeProvider;
use kailua_client::telemetry::TelemetryArgs;
use kailua_client::{await_tel, await_tel_res};
use kailua_common::config::{config_hash, BN254_CONTROL_ID, CONTROL_ROOT};
use kailua_contracts::*;
use kailua_host::config::fetch_rollup_config;
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, Status, TraceContextExt, Tracer};
use std::str::FromStr;
use tracing::info;

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

    /// Transaction publication configuration
    #[clap(flatten)]
    pub txn_args: TransactArgs,

    /// The l2 block number to start sequencing since
    #[clap(long, env)]
    pub starting_block_number: u64,
    /// The number of outputs that a proposal must publish
    #[clap(long, env)]
    pub proposal_output_count: u64,
    /// The number of blocks each output must cover
    #[clap(long, env)]
    pub output_block_span: u64,
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
    #[clap(flatten)]
    pub deployer_signer: DeployerSignerArgs,
    /// Secret key of L1 wallet that (indirectly) owns `DisputeGameFactory`
    #[clap(flatten)]
    pub owner_signer: OwnerSignerArgs,
    /// Secret key of L1 guardian wallet
    #[clap(flatten)]
    pub guardian_signer: Option<GuardianSignerArgs>,

    /// Address of the vanguard to set
    #[clap(long, env)]
    pub vanguard_address: Option<String>,
    /// Duration of the advantage given to the vanguard
    #[clap(long, env, requires = "vanguard_address")]
    pub vanguard_advantage: Option<u64>,

    /// Whether to set Kailua as the OptimismPortal's respected game type
    #[clap(long, env)]
    pub respect_kailua_proposals: bool,

    #[clap(flatten)]
    pub telemetry: TelemetryArgs,
}

pub async fn fast_track(args: FastTrackArgs) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("fast_track"));

    let op_node_provider = OpNodeProvider(RootProvider::new_http(
        args.op_node_url.as_str().try_into()?,
    ));
    let eth_rpc_provider =
        RootProvider::<Ethereum>::new_http(args.eth_rpc_url.as_str().try_into()?);

    info!("Fetching rollup configuration from rpc endpoints.");
    // fetch rollup config
    let config = await_tel!(
        context,
        fetch_rollup_config(&args.op_node_url, &args.op_geth_url, None)
    )
    .context("fetch_rollup_config")?;
    let rollup_config_hash = config_hash(&config).context("config_hash")?;
    info!("RollupConfigHash({})", hex::encode(rollup_config_hash));

    // load system config
    let system_config = SystemConfig::new(config.l1_system_config_address, &eth_rpc_provider);
    let portal_address = system_config
        .optimismPortal()
        .stall_with_context(context.clone(), "SystemConfig::optimismPortal")
        .await;
    let dgf_address = system_config
        .disputeGameFactory()
        .stall_with_context(context.clone(), "SystemConfig::disputeGameFactory")
        .await;

    // initialize owner wallet
    info!("Initializing owner wallet.");
    let owner_wallet = await_tel_res!(
        context,
        tracer,
        "OwnerSignerArgs::wallet",
        args.owner_signer.wallet(Some(config.l1_chain_id))
    )?;
    let owner_provider = args
        .txn_args
        .premium_provider::<Ethereum>()
        .wallet(&owner_wallet)
        .connect_http(args.eth_rpc_url.as_str().try_into()?);

    // Init factory contract
    let dispute_game_factory = IDisputeGameFactory::new(dgf_address, &owner_provider);
    info!("DisputeGameFactory({:?})", dispute_game_factory.address());
    let game_count = dispute_game_factory
        .gameCount()
        .stall_with_context(context.clone(), "DisputeGameFactory::gameCount")
        .await;
    info!("There have been {game_count} games created using DisputeGameFactory");
    let dispute_game_factory_ownable = OwnableUpgradeable::new(dgf_address, &owner_provider);
    let factory_owner_address = dispute_game_factory_ownable
        .owner()
        .stall_with_context(context.clone(), "DisputeGameFactory::owner")
        .await;
    let factory_owner_safe = Safe::new(factory_owner_address, &owner_provider);
    info!("Safe({:?})", factory_owner_safe.address());
    let safe_owners = factory_owner_safe
        .getOwners()
        .stall_with_context(context.clone(), "Safe::getOwners")
        .await;
    info!("Safe::owners({:?})", &safe_owners);
    let owner_address = owner_wallet.default_signer().address();
    if safe_owners.first().unwrap() != &owner_address {
        bail!("Incorrect owner key.");
    } else if safe_owners.len() != 1 {
        bail!("Expected exactly one owner of safe account.");
    }

    // initialize deployment wallet
    info!("Initializing deployer wallet.");
    let deployer_wallet = await_tel_res!(
        context,
        tracer,
        "DeployerSignerArgs::wallet",
        args.deployer_signer.wallet(Some(config.l1_chain_id))
    )?;
    let deployer_provider = args
        .txn_args
        .premium_provider::<Ethereum>()
        .wallet(&deployer_wallet)
        .connect_http(args.eth_rpc_url.as_str().try_into()?);

    // Deploy or reuse existing RISCZeroVerifier contracts
    let verifier_contract_address = match &args.verifier_contract {
        None => await_tel!(
            context,
            deploy_verifier(&deployer_provider, &owner_provider, owner_address)
        )
        .context("deploy_verifier")?,
        Some(address) => Address::from_str(address)?,
    };

    // Deploy KailuaTreasury contract
    let root_claim = await_tel_res!(
        context,
        tracer,
        "root_claim",
        retry_with_context!(op_node_provider.output_at_block(args.starting_block_number))
    )?;
    info!("Deploying KailuaTreasury contract to L1 rpc.");
    let receipt = KailuaTreasury::deploy_builder(
        &deployer_provider,
        verifier_contract_address,
        bytemuck::cast::<[u32; 8], [u8; 32]>(KAILUA_FPVM_ID).into(),
        rollup_config_hash.into(),
        Uint::from(args.proposal_output_count),
        Uint::from(args.output_block_span),
        KAILUA_GAME_TYPE,
        portal_address,
        root_claim,
        args.starting_block_number,
    )
    .transact_with_context(context.clone(), "KailuaTreasury::deploy")
    .await
    .context("KailuaTreasury::deploy")?;
    info!("KailuaTreasury::deploy: {} gas", receipt.gas_used);
    let kailua_treasury_impl_addr = receipt
        .contract_address
        .ok_or_else(|| anyhow!("KailuaTreasury not deployed"))?;
    let kailua_treasury_implementation =
        KailuaTreasury::new(kailua_treasury_impl_addr, &deployer_provider);
    info!("{:?}", &kailua_treasury_implementation);

    // Update dispute factory implementation to new KailuaTreasury deployment
    info!("Setting KailuaTreasury implementation address in DisputeGameFactory.");
    await_tel_res!(
        context,
        tracer,
        "DisputeGameFactory::setImplementation",
        crate::transact::safe::exec_safe_txn(
            dispute_game_factory.setImplementation(KAILUA_GAME_TYPE, kailua_treasury_impl_addr),
            &factory_owner_safe,
            owner_address,
        )
    )?;
    assert_eq!(
        dispute_game_factory
            .gameImpls(KAILUA_GAME_TYPE)
            .stall_with_context(context.clone(), "DisputeGameFactory::gameImpls")
            .await,
        kailua_treasury_impl_addr
    );

    if !dispute_game_factory
        .initBonds(KAILUA_GAME_TYPE)
        .stall_with_context(context.clone(), "DisputeGameFactory::initBonds")
        .await
        .is_zero()
    {
        info!("Setting KailuaTreasury initialization bond value in DisputeGameFactory to zero.");
        await_tel_res!(
            context,
            tracer,
            "DisputeGameFactory::setInitBond",
            crate::transact::safe::exec_safe_txn(
                dispute_game_factory.setInitBond(KAILUA_GAME_TYPE, U256::ZERO),
                &factory_owner_safe,
                owner_address,
            )
        )?;
        assert_eq!(
            dispute_game_factory
                .initBonds(KAILUA_GAME_TYPE)
                .stall_with_context(context.clone(), "DisputeGameFactory::initBonds")
                .await,
            U256::ZERO
        );
    }

    // Create new treasury instance from target block number
    let extra_data = Bytes::from(
        [
            args.starting_block_number.abi_encode_packed(),
            kailua_treasury_impl_addr.abi_encode_packed(),
        ]
        .concat(),
    );
    info!(
        "Creating new KailuaTreasury game instance from {} ({}).",
        args.starting_block_number, root_claim
    );

    kailua_treasury_implementation
        .propose(root_claim, extra_data.clone())
        .transact_with_context(context.clone(), "KailuaTreasury::propose")
        .await
        .context("KailuaTreasury::propose")?;
    let kailua_treasury_instance_address = dispute_game_factory
        .games(KAILUA_GAME_TYPE, root_claim, extra_data)
        .stall_with_context(context.clone(), "DisputeGameFactory::games")
        .await
        .proxy_;
    let kailua_treasury_instance =
        KailuaTreasury::new(kailua_treasury_instance_address, &owner_provider);
    info!("{:?}", &kailua_treasury_instance);
    let status = kailua_treasury_instance
        .status()
        .stall_with_context(context.clone(), "KailuaTreasury::status")
        .await;
    if status == 0 {
        info!("Resolving KailuaTreasury instance");
        await_tel_res!(
            context,
            tracer,
            "KailuaTreasury::resolve",
            crate::transact::safe::exec_safe_txn(
                kailua_treasury_instance.resolve(),
                &factory_owner_safe,
                owner_address,
            )
        )?;
    } else {
        info!("Game instance is not ongoing ({status})");
    }

    // Update participation bond value
    info!(
        "Setting KailuaTreasury participation bond value to {} wei.",
        args.collateral_amount
    );
    let bond_value = U256::from(args.collateral_amount);
    await_tel_res!(
        context,
        tracer,
        "KailuaTreasury::setParticipationBond",
        crate::transact::safe::exec_safe_txn(
            kailua_treasury_implementation.setParticipationBond(bond_value),
            &factory_owner_safe,
            owner_address,
        )
    )?;
    assert_eq!(
        kailua_treasury_implementation
            .participationBond()
            .stall_with_context(context.clone(), "KailuaTreasury::participationBond")
            .await,
        bond_value
    );

    // Deploy KailuaGame contract
    info!("Deploying KailuaGame contract to L1 rpc.");
    let receipt = KailuaGame::deploy_builder(
        &deployer_provider,
        *kailua_treasury_implementation.address(),
        U256::from(config.genesis.l2_time),
        U256::from(config.block_time),
        U256::from(args.proposal_time_gap),
        args.challenge_timeout,
    )
    .transact_with_context(context.clone(), "KailuaGame::deploy")
    .await
    .context("KailuaGame::deploy")?;
    info!("KailuaGame::deploy: {} gas", receipt.gas_used);
    let kailua_game_contract = KailuaGame::new(
        receipt
            .contract_address
            .ok_or_else(|| anyhow!("KailuaGame not deployed"))?,
        &deployer_provider,
    );
    info!("{:?}", &kailua_game_contract);

    // Update implementation to KailuaGame
    info!("Setting KailuaGame implementation address in DisputeGameFactory.");
    await_tel_res!(
        context,
        tracer,
        "DisputeGameFactory::setImplementation",
        crate::transact::safe::exec_safe_txn(
            dispute_game_factory
                .setImplementation(KAILUA_GAME_TYPE, *kailua_game_contract.address()),
            &factory_owner_safe,
            owner_address,
        )
    )?;

    // Set the vanguard parameters if provided
    if let Some(vanguard_address_string) = args.vanguard_address {
        let vanguard_address = Address::from_str(&vanguard_address_string)?;
        let vanguard_advantage = args.vanguard_advantage.unwrap_or(u64::MAX >> 4);
        info!("Assigning proposal advantage to vanguard in KailuaTreasury.");

        await_tel_res!(
            context,
            tracer,
            "KailuaTreasury::assignVanguard",
            crate::transact::safe::exec_safe_txn(
                kailua_treasury_implementation.assignVanguard(vanguard_address, vanguard_advantage),
                &factory_owner_safe,
                owner_address,
            )
        )?;
    }

    // Update the respectedGameType as the guardian
    if args.respect_kailua_proposals {
        // initialize guardian wallet
        info!("Initializing guardian wallet.");
        let guardian_wallet = await_tel_res!(
            context,
            tracer,
            "GuardianSignerArgs::wallet",
            args.guardian_signer
                .ok_or_else(|| anyhow!("Guardian signer not provided"))?
                .wallet(Some(config.l1_chain_id))
        )?;
        let guardian_address = guardian_wallet.default_signer().address();
        let guardian_provider = args
            .txn_args
            .premium_provider::<Ethereum>()
            .wallet(&guardian_wallet)
            .connect_http(args.eth_rpc_url.as_str().try_into()?);
        let optimism_portal = OptimismPortal2::new(portal_address, &guardian_provider);
        let portal_guardian_address = optimism_portal
            .guardian()
            .stall_with_context(context.clone(), "OptimismPortal2::guardian")
            .await;
        if portal_guardian_address != guardian_address {
            bail!("OptimismPortal2 Guardian is {portal_guardian_address}. Provided private key has account address {guardian_address}.");
        }

        info!("Setting respectedGameType in OptimismPortal2.");
        let receipt = optimism_portal
            .setRespectedGameType(KAILUA_GAME_TYPE)
            .transact_with_context(context.clone(), "OptimismPortal2::setRespectedGameType")
            .await
            .context("OptimismPortal2::setRespectedGameType")?;
        info!(
            "OptimismPortal2::setRespectedGameType: {} gas",
            receipt.gas_used
        );
    }

    info!("Kailua upgrade complete.");
    context.span().set_status(Status::Ok);
    Ok(())
}

pub async fn deploy_verifier<P1: Provider<N>, P2: Provider<N>, N: Network>(
    deployer_provider: P1,
    owner_provider: P2,
    owner_address: Address,
) -> anyhow::Result<Address> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("deploy_verifier"));
    // Deploy verifier router contract
    info!("Deploying RiscZeroVerifierRouter contract to L1 under ownership of {owner_address}.");
    let receipt = RiscZeroVerifierRouter::deploy_builder(&deployer_provider, owner_address)
        .transact_with_context(context.clone(), "RiscZeroVerifierRouter::deploy")
        .await
        .context("RiscZeroVerifierRouter::deploy")?;
    info!("RiscZeroVerifierRouter::deploy: {} gas", receipt.gas_used());
    let verifier_contract_address = receipt
        .contract_address()
        .ok_or_else(|| anyhow!("RiscZeroVerifierRouter not deployed"))?;
    let verifier_contract = RiscZeroVerifierRouter::new(verifier_contract_address, &owner_provider);

    // Deploy RiscZeroGroth16Verifier contract
    info!("Deploying RiscZeroGroth16Verifier contract to L1.");
    let receipt =
        RiscZeroGroth16Verifier::deploy_builder(&deployer_provider, CONTROL_ROOT, BN254_CONTROL_ID)
            .transact_with_context(context.clone(), "RiscZeroGroth16Verifier::deploy")
            .await
            .context("RiscZeroGroth16Verifier::deploy")?;
    info!(
        "RiscZeroGroth16Verifier::deploy: {} gas",
        receipt.gas_used()
    );
    let groth16_verifier_contract = RiscZeroGroth16Verifier::new(
        receipt
            .contract_address()
            .ok_or_else(|| anyhow!("RiscZeroGroth16Verifier not deployed"))?,
        &deployer_provider,
    );
    info!("{:?}", &groth16_verifier_contract);
    let selector = groth16_verifier_contract
        .SELECTOR()
        .stall_with_context(context.clone(), "RiscZeroGroth16Verifier::SELECTOR")
        .await;
    info!("Adding RiscZeroGroth16Verifier contract to RiscZeroVerifierRouter.");
    let receipt = verifier_contract
        .addVerifier(selector, *groth16_verifier_contract.address())
        .transact_with_context(
            context.clone(),
            "RiscZeroVerifierRouter::addVerifier(RiscZeroGroth16Verifier)",
        )
        .await
        .context("RiscZeroVerifierRouter::addVerifier(RiscZeroGroth16Verifier)")?;
    info!(
        "RiscZeroVerifierRouter::addVerifier(RiscZeroGroth16Verifier): {} gas",
        receipt.gas_used()
    );

    // Deploy mock verifier
    #[cfg(feature = "devnet")]
    if risc0_zkvm::is_dev_mode() {
        // Deploy MockVerifier contract
        tracing::warn!("Deploying RiscZeroMockVerifier contract to L1. This will accept fake proofs which are not cryptographically secure!");
        let receipt = RiscZeroMockVerifier::deploy_builder(&deployer_provider, [0xFFu8; 4].into())
            .transact_with_context(context.clone(), "RiscZeroMockVerifier::deploy")
            .await
            .context("RiscZeroMockVerifier::deploy")?;
        info!("RiscZeroMockVerifier::deploy: {} gas", receipt.gas_used());

        let mock_verifier_contract = RiscZeroMockVerifier::new(
            receipt
                .contract_address()
                .ok_or_else(|| anyhow!("RiscZeroMockVerifier not deployed"))?,
            &deployer_provider,
        );
        tracing::warn!("{:?}", &mock_verifier_contract);
        tracing::warn!("Adding RiscZeroMockVerifier contract to RiscZeroVerifierRouter.");
        let receipt = verifier_contract
            .addVerifier([0xFFu8; 4].into(), *mock_verifier_contract.address())
            .transact_with_context(
                context.clone(),
                "RiscZeroVerifierRouter::addVerifier(RiscZeroMockVerifier)",
            )
            .await
            .context("RiscZeroVerifierRouter::addVerifier(RiscZeroMockVerifier)")?;
        info!(
            "RiscZeroVerifierRouter::addVerifier(RiscZeroMockVerifier): {} gas",
            receipt.gas_used()
        );
    }

    context.span().set_status(Status::Ok);
    Ok(verifier_contract_address)
}
