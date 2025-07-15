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

use crate::api::KailuaServerCache;
use crate::args::RpcArgs;
use crate::{api, sync};
use anyhow::Context;
use opentelemetry::global::tracer;
use opentelemetry::trace::{FutureExt, TraceContextExt, Tracer};
use std::path::PathBuf;
use tokio::{spawn, try_join};

pub async fn rpc(args: RpcArgs, data_dir: PathBuf) -> anyhow::Result<()> {
    let tracer = tracer("kailua");
    let context = opentelemetry::Context::current_with_span(tracer.start("rpc"));

    let server_cache: KailuaServerCache = Default::default();

    let handle_sync = spawn(
        sync::handle_sync(args.clone(), data_dir.clone(), server_cache.clone())
            .with_context(context.clone()),
    );
    let handle_requests = spawn(
        api::handle_requests(args.clone(), server_cache.clone()).with_context(context.clone()),
    );

    let (sync_task, requests_task) = try_join!(handle_sync, handle_requests)?;
    sync_task.context("handle_sync")?;
    requests_task.context("handle_requests")?;

    Ok(())
}
