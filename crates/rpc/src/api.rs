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

use crate::args::RpcArgs;
use alloy::primitives::Address;
use jsonrpsee::core::{async_trait, RpcResult};
use jsonrpsee::proc_macros::rpc;
use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::trace;

#[rpc(client, server, namespace = "kailua")]
pub trait KailuaApi {
    #[method(name = "gameAddressForBlockByNumber")]
    async fn game_address_for_block_by_number(&self, number: u64) -> RpcResult<Option<Address>>;
}

pub type KailuaServerCache = Arc<RwLock<BTreeMap<u64, Address>>>;

#[derive(Clone, Default, Debug)]
pub struct KailuaApiHandler {
    pub cache: KailuaServerCache,
}

#[async_trait]
impl KailuaApiServer for KailuaApiHandler {
    async fn game_address_for_block_by_number(&self, number: u64) -> RpcResult<Option<Address>> {
        trace!(target: "rpc::kailua", "Serving kailua_gameAddressForBlockByNumber");
        Ok(self
            .cache
            .read()
            .await
            .range(number..)
            .next()
            .map(|(_, addr)| *addr))
    }
}

pub async fn handle_requests(args: RpcArgs, cache: KailuaServerCache) -> anyhow::Result<()> {
    let kailua_api_handler = KailuaApiHandler { cache }.into_rpc();

    let rpc_server = jsonrpsee::server::Server::builder()
        .build(
            args.rpc_socket
                .unwrap_or(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 1337))),
        )
        .await
        .expect("RPC Server creation failed");

    // run until stopped
    rpc_server.start(kailua_api_handler).stopped().await;

    Ok(())
}
