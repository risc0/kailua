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

use clap::Parser;
use kailua_client::args::KailuaClientArgs;
use kailua_client::oracle::{HINT_WRITER, ORACLE_READER};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = KailuaClientArgs::parse();
    kona_host::init_tracing_subscriber(args.kailua_verbosity)?;
    let precondition_validation_data_hash =
        args.precondition_validation_data_hash.unwrap_or_default();
    let payout_recipient_address = args.payout_recipient_address.unwrap_or_default();

    kailua_client::proving::run_proving_client(
        args.boundless,
        ORACLE_READER,
        HINT_WRITER,
        payout_recipient_address,
        precondition_validation_data_hash,
        vec![], // todo: read stitched boot info data
    )
    .await
}
