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

use opentelemetry::global::meter;
use opentelemetry::metrics::{Gauge, Meter};

pub struct SyncTelemetry {
    pub meter: Meter,
    pub sync_canonical: Gauge<u64>,
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
