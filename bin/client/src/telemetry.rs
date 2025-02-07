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

use opentelemetry::global::set_tracer_provider;
use opentelemetry::trace::TraceError;
use opentelemetry::KeyValue;
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::{runtime::Tokio, trace::TracerProvider, Resource};

#[derive(clap::Args, Debug, Clone)]
pub struct TelemetryArgs {
    /// OTLP Collector endpoint address
    #[clap(long, env, num_args = 0..=1, default_missing_value = "http://localhost:4317")]
    pub otlp_collector: Option<String>,
}

pub fn init_tracer_provider<T: Into<String>>(endpoint: Option<T>) -> Result<(), TraceError> {
    if let Some(endpoint) = endpoint {
        let endpoint: String = endpoint.into();
        println!("telemetry export endpoint: {endpoint}");
        // Build and set default global provider
        set_tracer_provider(
            TracerProvider::builder()
                .with_batch_exporter(
                    SpanExporter::builder()
                        .with_tonic()
                        .with_endpoint(endpoint)
                        .build()?,
                    Tokio,
                )
                .with_resource(Resource::new(vec![KeyValue::new("service.name", "kailua")]))
                .build(),
        );
    }
    Ok(())
}

#[macro_export]
macro_rules! await_tel {
    ($c:ident, $e:expr) => {
        $e.with_context($c.clone()).await
    };
    ($c:ident, $t:ident, $l:literal, $e:expr) => {
        $e.with_context($c.with_span($t.start_with_context($l, &$c)))
            .await
    };
}

#[macro_export]
macro_rules! await_tel_res {
    ($c:ident, $t:ident, $l:literal, $e:expr) => {
        $crate::await_tel!($c, $t, $l, $e).context($l)
    };
}
