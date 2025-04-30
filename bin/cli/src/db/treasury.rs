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
use alloy::network::Network;
use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use kailua_contracts::{KailuaTreasury::KailuaTreasuryInstance, *};
use opentelemetry::global::tracer;
use opentelemetry::trace::{TraceContextExt, Tracer};
use std::collections::hash_map::Entry;
use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct Treasury {
    pub address: Address,
    pub claim_proposer: HashMap<Address, Address>,
    pub participation_bond: U256,
    pub paid_bond: HashMap<Address, U256>,
}

impl Treasury {
    pub async fn init<P: Provider<N>, N: Network>(
        treasury_implementation: &KailuaTreasuryInstance<P, N>,
    ) -> anyhow::Result<Self> {
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(tracer.start("Treasury::init"));

        // Load participation bond
        let participation_bond = treasury_implementation
            .participationBond()
            .stall_with_context(context.clone(), "KailuaTreasury::participationBond")
            .await;
        Ok(Self {
            address: *treasury_implementation.address(),
            claim_proposer: Default::default(),
            participation_bond,
            paid_bond: Default::default(),
        })
    }

    pub fn treasury_contract_instance<P: Provider<N>, N: Network>(
        &self,
        provider: P,
    ) -> KailuaTreasuryInstance<P, N> {
        KailuaTreasury::new(self.address, provider)
    }

    pub async fn fetch_bond<P: Provider<N>, N: Network>(&mut self, provider: P) -> U256 {
        let tracer = tracer("kailua");
        let context =
            opentelemetry::Context::current_with_span(tracer.start("Treasury::fetch_bond"));
        self.participation_bond = self
            .treasury_contract_instance(provider)
            .participationBond()
            .stall_with_context(context.clone(), "KailuaTreasury::participationBond")
            .await;
        self.participation_bond
    }

    pub async fn fetch_vanguard<P: Provider<N>, N: Network>(&mut self, provider: P) -> Address {
        let tracer = tracer("kailua");
        let context =
            opentelemetry::Context::current_with_span(tracer.start("Treasury::fetch_vanguard"));
        self.treasury_contract_instance(provider)
            .vanguard()
            .stall_with_context(context.clone(), "KailuaTreasury::vanguard")
            .await
    }

    pub async fn fetch_vanguard_advantage<P: Provider<N>, N: Network>(
        &mut self,
        provider: P,
    ) -> u64 {
        let tracer = tracer("kailua");
        let context = opentelemetry::Context::current_with_span(
            tracer.start("Treasury::fetch_vanguard_advantage"),
        );
        self.treasury_contract_instance(provider)
            .vanguardAdvantage()
            .stall_with_context(context.clone(), "KailuaTreasury::vanguardAdvantage")
            .await
    }

    pub async fn fetch_balance<P: Provider<N>, N: Network>(
        &mut self,
        provider: P,
        address: Address,
    ) -> U256 {
        let tracer = tracer("kailua");
        let context =
            opentelemetry::Context::current_with_span(tracer.start("Treasury::fetch_balance"));
        let paid_bond = self
            .treasury_contract_instance(provider)
            .paidBonds(address)
            .stall_with_context(context.clone(), "KailuaTreasury::paidBonds")
            .await;
        self.paid_bond.insert(address, paid_bond);
        paid_bond
    }

    pub async fn fetch_proposer<P: Provider<N>, N: Network>(
        &mut self,
        provider: P,
        address: Address,
    ) -> Address {
        let tracer = tracer("kailua");
        let context =
            opentelemetry::Context::current_with_span(tracer.start("Treasury::fetch_proposer"));
        let instance = self.treasury_contract_instance(provider);
        match self.claim_proposer.entry(address) {
            Entry::Vacant(entry) => {
                let proposer = instance
                    .proposerOf(address)
                    .stall_with_context(context.clone(), "KailuaTreasury::proposerOf")
                    .await;
                *entry.insert(proposer)
            }
            Entry::Occupied(entry) => *entry.get(),
        }
    }
}
