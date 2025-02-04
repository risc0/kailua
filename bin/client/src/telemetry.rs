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
use opentelemetry_otlp::SpanExporter;
use opentelemetry_sdk::{runtime::Tokio, trace::TracerProvider, Resource};

pub fn init_tracer_provider() -> Result<TracerProvider, TraceError> {
    // Instantiate OTLP exporter
    let exporter = SpanExporter::builder().with_tonic().build()?;
    // Build tracer provider with exporter
    let provider = TracerProvider::builder()
        .with_batch_exporter(exporter, Tokio)
        .with_resource(Resource::new(vec![KeyValue::new("service.name", "kailua")]))
        .build();
    // Set as default global provider
    set_tracer_provider(provider.clone());
    // Return provider
    Ok(provider)
}
