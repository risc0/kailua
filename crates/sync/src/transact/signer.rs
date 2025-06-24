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

use alloy::network::EthereumWallet;
use alloy::primitives::ChainId;
use alloy::signers::{
    aws::AwsSigner,
    gcp::{GcpKeyRingRef, GcpSigner, KeySpecifier},
    local::LocalSigner,
};
use anyhow::Context;
use aws_config::BehaviorVersion;
use gcloud_sdk::{
    google::cloud::kms::v1::key_management_service_client::KeyManagementServiceClient, GoogleApi,
};
use std::str::FromStr;

#[macro_export]
macro_rules! define_signer_args {
    ($vis: vis $struct_name: ident, $prefix: ident) => {
        paste::paste! {
            #[derive(clap::Args, Debug, Clone)]
            $vis struct $struct_name {
                /// Wallet private key
                #[clap(long, env, required_unless_present_any = [stringify!([<$prefix aws_key_id>]), stringify!([<$prefix google_keyring>])])]
                pub [<$prefix key>]: Option<String>,

                /// AWS KMS Key ID
                #[clap(long, env, required_unless_present_any = [stringify!([<$prefix key>]), stringify!([<$prefix google_keyring>])])]
                pub [<$prefix aws_key_id>]: Option<String>,

                /// GCP KMS Project ID
                #[clap(long, env, requires = stringify!([<$prefix google_location>]))]
                #[clap(required_unless_present_any = [stringify!([<$prefix key>]), stringify!([<$prefix aws_key_id>])])]
                pub [<$prefix google_project_id>]: Option<String>,
                /// GCP KMS Location
                #[clap(long, env, requires = stringify!([<$prefix google_keyring>]))]
                pub [<$prefix google_location>]: Option<String>,
                /// GCP KMS Keyring Name
                #[clap(long, env, requires = stringify!([<$prefix google_key_name>]))]
                pub [<$prefix google_keyring>]: Option<String>,
                /// GCP KMS Key name
                #[clap(long, env, requires = stringify!([<$prefix google_project_id>]))]
                pub [<$prefix google_key_name>]: Option<String>,
            }

            impl $struct_name {
                pub async fn wallet(&self, chain_id: Option<ChainId>) -> anyhow::Result<EthereumWallet> {
                    args_to_wallet(
                        &self.[<$prefix key>],
                        &self.[<$prefix aws_key_id>],
                        &self.[<$prefix google_project_id>],
                        &self.[<$prefix google_location>],
                        &self.[<$prefix google_keyring>],
                        &self.[<$prefix google_key_name>],
                        chain_id
                    ).await
                }
            }
        }
    }
}

define_signer_args!(pub DeployerSignerArgs, deployer_);
define_signer_args!(pub OwnerSignerArgs, owner_);
define_signer_args!(pub GuardianSignerArgs, guardian_);
define_signer_args!(pub ProposerSignerArgs, proposer_);
define_signer_args!(pub ValidatorSignerArgs, validator_);

pub async fn args_to_wallet(
    key: &Option<String>,
    aws_key_id: &Option<String>,
    google_project_id: &Option<String>,
    google_location: &Option<String>,
    google_keyring: &Option<String>,
    google_key_name: &Option<String>,
    chain_id: Option<ChainId>,
) -> anyhow::Result<EthereumWallet> {
    if let Some(key) = key {
        let local_signer = LocalSigner::from_str(key)?;
        return Ok(EthereumWallet::from(local_signer));
    } else if let Some(key_id) = aws_key_id {
        let config = aws_config::load_defaults(BehaviorVersion::latest()).await;
        let client = aws_sdk_kms::Client::new(&config);
        let signer = AwsSigner::new(client, key_id.clone(), chain_id)
            .await
            .context("AwsSigner::new")?;
        return Ok(EthereumWallet::from(signer));
    }

    let project_id = google_project_id.clone().unwrap();
    let location = google_location.clone().unwrap();
    let keyring_name = google_keyring.clone().unwrap();
    let keyring = GcpKeyRingRef::new(&project_id, &location, &keyring_name);
    let client = GoogleApi::from_function(
        KeyManagementServiceClient::new,
        "https://cloudkms.googleapis.com",
        None,
    )
    .await
    .context("Failed to create GCP KMS Client")?;

    let key_name = google_key_name.clone().unwrap();
    let key_specifier = KeySpecifier::new(keyring, &key_name, 1);
    let signer = GcpSigner::new(client, key_specifier, chain_id)
        .await
        .context("GcpSigner::new")?;

    Ok(EthereumWallet::from(signer))
}
