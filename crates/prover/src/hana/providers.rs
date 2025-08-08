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

use celestia_types::nmt::Namespace;
use hana_host::celestia::{CelestiaChainHost, CelestiaChainProviders, OnlineCelestiaProvider};
use kona_host::eth::http_provider;
use kona_host::single::{SingleChainHostError, SingleChainProviders};
use kona_providers_alloy::{OnlineBeaconClient, OnlineBlobProvider};
use op_alloy_network::Optimism;

/// This code is copied from [CelestiaChainHost]
/// Creates the providers required for the host backend.
pub async fn create_providers(
    cfg: &CelestiaChainHost,
) -> anyhow::Result<CelestiaChainProviders, SingleChainHostError> {
    let l1_provider = http_provider(
        cfg.single_host
            .l1_node_address
            .as_ref()
            .ok_or(SingleChainHostError::Other("Provider must be set"))?,
    );
    let blob_provider = OnlineBlobProvider::init(OnlineBeaconClient::new_http(
        cfg.single_host
            .l1_beacon_address
            .clone()
            .ok_or(SingleChainHostError::Other("Beacon API URL must be set"))?,
    ))
    .await;
    let l2_provider = http_provider::<Optimism>(
        cfg.single_host
            .l2_node_address
            .as_ref()
            .ok_or(SingleChainHostError::Other("L2 node address must be set"))?,
    );

    let celestia_client = celestia_rpc::Client::new(
        cfg.celestia_args
            .celestia_connection
            .as_ref()
            .ok_or(SingleChainHostError::Other(
                "Celestia connection must be set",
            ))?,
        cfg.celestia_args.auth_token.as_deref(),
    )
    .await
    .expect("Failed creating rpc client");

    let namespace_bytes = alloy::hex::decode(cfg.celestia_args.namespace.as_ref().ok_or(
        SingleChainHostError::Other("Celestia Namespace must be set"),
    )?)
    .expect("Invalid hex");
    let namespace = Namespace::new_v0(&namespace_bytes).expect("Invalid namespace");

    let celestia_provider = OnlineCelestiaProvider::new(celestia_client, namespace);

    Ok(CelestiaChainProviders {
        inner_providers: SingleChainProviders {
            l1: l1_provider,
            blobs: blob_provider,
            l2: l2_provider,
        },
        celestia: celestia_provider,
    })
}
