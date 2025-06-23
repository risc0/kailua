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

use opentelemetry::global::{meter, set_meter_provider, set_tracer_provider};
use opentelemetry::metrics::{Gauge, Meter};
use opentelemetry::KeyValue;
use opentelemetry_otlp::{MetricExporter, SpanExporter, WithExportConfig};
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider, Temporality};
use opentelemetry_sdk::{runtime::Tokio, trace::TracerProvider, Resource};

#[derive(clap::Args, Debug, Clone)]
pub struct TelemetryArgs {
    /// OTLP Collector endpoint address
    #[clap(long, env, num_args = 0..=1, default_missing_value = "http://localhost:4317")]
    pub otlp_collector: Option<String>,
}

pub fn init_tracer_provider(args: &TelemetryArgs) -> anyhow::Result<()> {
    if let Some(otlp_collector) = &args.otlp_collector {
        println!("OTLP Collector endpoint: {otlp_collector}");
        // Build and set default global tracer provider
        set_tracer_provider(
            TracerProvider::builder()
                .with_batch_exporter(
                    SpanExporter::builder()
                        .with_tonic()
                        .with_endpoint(otlp_collector)
                        .build()?,
                    Tokio,
                )
                .with_resource(Resource::new(vec![KeyValue::new("service.name", "kailua")]))
                .build(),
        );
        // Build and set default global meter provider
        set_meter_provider(
            SdkMeterProvider::builder()
                .with_reader(
                    PeriodicReader::builder(
                        MetricExporter::builder()
                            .with_temporality(Temporality::Delta)
                            .with_tonic()
                            .with_endpoint(otlp_collector)
                            .build()?,
                        Tokio,
                    )
                    .build(),
                )
                .with_resource(Resource::new(vec![KeyValue::new("service.name", "kailua")]))
                .build(),
        )
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
    ($c:ident, $e:expr, $l:literal) => {
        $crate::await_tel!($c, $e).context($l)
    };
    ($c:ident, $t:ident, $l:literal, $e:expr) => {
        $crate::await_tel!($c, $t, $l, $e).context($l)
    };
}

/// An collection of objects for reporting telemetry information
pub struct SyncTelemetry {
    /// Global meter object
    pub meter: Meter,
    /// Gauge for reporting the latest canonical block height
    pub sync_canonical: Gauge<u64>,
    /// Gauge for reporting the next proposal index to query
    pub sync_next: Gauge<u64>,
}

impl Default for SyncTelemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncTelemetry {
    pub fn new() -> Self {
        let meter = meter("kailua");
        let sync_canonical = meter.u64_gauge("sync.canonical").build();
        let sync_next = meter.u64_gauge("sync.next").build();

        Self {
            meter,
            sync_canonical,
            sync_next,
        }
    }
}
