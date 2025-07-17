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

use kailua_sync::args::SyncArgs;
use std::net::SocketAddr;

#[derive(clap::Args, Debug, Clone)]
pub struct RpcArgs {
    #[clap(flatten)]
    pub sync: SyncArgs,
    /// Whether to bypass loading rollup chain configurations from the kona registry
    #[clap(long, env, default_value_t = false)]
    pub bypass_chain_registry: bool,
    /// Socket for http or ws connections. (default: 127.0.0.1:1337).
    #[clap(long, env)]
    pub socket_addr: Option<SocketAddr>,
    /// Disables listening for RPC requests over HTTP
    #[clap(long, env, default_value_t = false)]
    pub disable_http: bool,
    /// Disables listening for RPC requests over WS
    #[clap(long, env, default_value_t = false)]
    pub disable_ws: bool,
}
