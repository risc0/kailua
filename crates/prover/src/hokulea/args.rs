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

use clap::Parser;

#[derive(Parser, Clone, Debug, Default)]
pub struct HokuleaArgs {
    /// URL of the EigenDA RPC endpoint.
    #[clap(long, env)]
    pub eigenda_proxy_address: Option<String>,
}

impl HokuleaArgs {
    pub fn to_arg_vec(&self) -> Vec<String> {
        self.eigenda_proxy_address
            .as_ref()
            .map(|address| vec![String::from("--eigenda-proxy-address"), address.to_string()])
            .unwrap_or_default()
    }

    pub fn is_set(&self) -> bool {
        self.eigenda_proxy_address.is_some()
    }
}
