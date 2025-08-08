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
use hana_host::celestia::CelestiaCfg;
use serde::Serialize;

/// The host binary CLI application arguments.
#[derive(Default, Parser, Serialize, Clone, Debug)]
pub struct HanaArgs {
    /// Connection to celestia network
    #[clap(long, alias = "celestia-conn", env)]
    pub celestia_connection: Option<String>,
    /// Token for the Celestia node connection
    #[clap(long, alias = "celestia-auth", env)]
    pub celestia_auth_token: Option<String>,
    /// Celestia Namespace to fetch data from
    #[clap(long, env)]
    pub celestia_namespace: Option<String>,
}

impl HanaArgs {
    pub fn to_arg_vec(&self) -> Vec<String> {
        [
            self.celestia_connection
                .as_ref()
                .map(|v| vec![String::from("--celestia-connection"), v.to_string()]),
            self.celestia_auth_token
                .as_ref()
                .map(|v| vec![String::from("--celestia-auth-token"), v.to_string()]),
            self.celestia_namespace
                .as_ref()
                .map(|v| vec![String::from("--celestia-namespace"), v.to_string()]),
        ]
        .into_iter()
        .flatten()
        .flatten()
        .collect()
    }

    pub fn is_set(&self) -> bool {
        self.celestia_connection.is_some()
            && self.celestia_auth_token.is_some()
            && self.celestia_namespace.is_some()
    }
}

impl From<CelestiaCfg> for HanaArgs {
    fn from(value: CelestiaCfg) -> Self {
        Self {
            celestia_connection: value.celestia_connection,
            celestia_auth_token: value.auth_token,
            celestia_namespace: value.namespace,
        }
    }
}

impl From<HanaArgs> for CelestiaCfg {
    fn from(value: HanaArgs) -> Self {
        Self {
            celestia_connection: value.celestia_connection,
            auth_token: value.celestia_auth_token,
            namespace: value.celestia_namespace,
        }
    }
}
