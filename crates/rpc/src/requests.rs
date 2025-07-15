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

use crate::api::{KailuaApiHandler, KailuaApiServer, KailuaServerCache};
use crate::args::RpcArgs;
use jsonrpsee::server::ServerConfig;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

pub async fn handle_requests(args: RpcArgs, cache: KailuaServerCache) -> anyhow::Result<()> {
    // Actual handler for requests
    let kailua_api_handler = KailuaApiHandler { cache }.into_rpc();

    // Bind address
    let socket_addr = args
        .socket_addr
        .unwrap_or(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 1337)));

    // Configure which protocols to support
    let server_config = match (args.disable_http, args.disable_ws) {
        (false, false) => ServerConfig::default(),
        (false, true) => ServerConfig::builder().http_only().build(),
        (true, false) => ServerConfig::builder().ws_only().build(),
        (true, true) => return Ok(()),
    };

    // Run server until termination
    jsonrpsee::server::Server::builder()
        .set_config(server_config)
        .build(socket_addr)
        .await
        .expect("HTTP/WS RPC Server creation failed")
        .start(kailua_api_handler.clone())
        .stopped()
        .await;

    Ok(())
}
