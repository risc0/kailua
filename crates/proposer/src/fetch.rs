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

use alloy::consensus::BlockHeader;
use alloy::eips::BlockNumberOrTag;
use alloy::network::BlockResponse;
use alloy::primitives::{Address, U256};
use kailua_contracts::*;
use kailua_sync::agent::SyncAgent;
use kailua_sync::await_tel;
use kailua_sync::proposal::Proposal;
use kailua_sync::stall::Stall;
use kailua_sync::transact::rpc::get_block;
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};

pub async fn fetch_vanguard(agent: &SyncAgent) -> Address {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("fetch_vanguard"));
    KailuaTreasury::new(agent.deployment.treasury, &agent.provider.l1_provider)
        .vanguard()
        .stall_with_context(context.clone(), "KailuaTreasury::vanguard")
        .await
}

pub async fn fetch_vanguard_advantage(agent: &SyncAgent) -> u64 {
    let tracer = tracer("kailua");
    let context =
        opentelemetry::Context::current_with_span(tracer.start("fetch_vanguard_advantage"));
    KailuaTreasury::new(agent.deployment.treasury, &agent.provider.l1_provider)
        .vanguardAdvantage()
        .stall_with_context(context.clone(), "KailuaTreasury::vanguardAdvantage")
        .await
}

pub async fn fetch_participation_bond(agent: &SyncAgent) -> U256 {
    let tracer = tracer("kailua");
    let context =
        opentelemetry::Context::current_with_span(tracer.start("fetch_participation_bond"));
    KailuaTreasury::new(agent.deployment.treasury, &agent.provider.l1_provider)
        .participationBond()
        .stall_with_context(context.clone(), "KailuaTreasury::participationBond")
        .await
}

pub async fn fetch_paid_bond(agent: &SyncAgent, address: Address) -> U256 {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("fetch_paid_bond"));
    KailuaTreasury::new(agent.deployment.treasury, &agent.provider.l1_provider)
        .paidBonds(address)
        .stall_with_context(context.clone(), "KailuaTreasury::paidBonds")
        .await
}

pub async fn fetch_current_challenger_duration(agent: &SyncAgent, proposal: &Proposal) -> u64 {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(
        tracer.start("Proposal::fetch_current_challenger_duration"),
    );

    let chain_time = await_tel!(
        context,
        get_block(&agent.provider.l1_provider, BlockNumberOrTag::Latest)
    )
    .header()
    .timestamp();

    proposal
        .tournament_contract_instance(&agent.provider.l1_provider)
        .getChallengerDuration(U256::from(chain_time))
        .stall_with_context(context.clone(), "KailuaTournament::getChallengerDuration")
        .await
}
