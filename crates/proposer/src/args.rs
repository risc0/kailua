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

use kailua_sync::args::SyncArgs;
use kailua_sync::transact::signer::ProposerSignerArgs;
use kailua_sync::transact::TransactArgs;

#[derive(clap::Args, Debug, Clone)]
pub struct ProposeArgs {
    #[clap(flatten)]
    pub sync: SyncArgs,

    /// L1 wallet to use for proposing outputs
    #[clap(flatten)]
    pub proposer_signer: ProposerSignerArgs,
    /// Transaction publication configuration
    #[clap(flatten)]
    pub txn_args: TransactArgs,
}
