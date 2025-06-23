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

use crate::{await_tel, retry_res_ctx_timeout};
use alloy::contract::{EthCall, SolCallBuilder};
use alloy::network::Network;
use alloy::providers::Provider;
use alloy::sol_types::SolCall;
use async_trait::async_trait;
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use opentelemetry::Context;
use std::future::IntoFuture;
use std::marker::PhantomData;

#[async_trait]
pub trait Stall<R> {
    async fn stall(&self, span: &'static str) -> R;

    async fn stall_with_context(&self, context: Context, span: &'static str) -> R {
        self.stall(span).with_context(context).await
    }
}

#[async_trait]
impl<'coder, P: Provider<N>, C: SolCall + 'static + Sync, N: Network> Stall<C::Return>
    for SolCallBuilder<P, C, N>
where
    EthCall<'coder, PhantomData<C>, N>: IntoFuture,
    C::Return: Send,
{
    async fn stall(&self, span: &'static str) -> C::Return {
        let tracer = tracer("kailua");
        let context = Context::current_with_span(tracer.start(span));

        await_tel!(
            context,
            tracer,
            "call_raw",
            retry_res_ctx_timeout!(self
                .call_raw()
                .await
                .and_then(|response| self.decode_output(response)))
        )
    }
}
