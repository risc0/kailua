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

use risc0_zkvm::Receipt;

pub mod bonsai;
pub mod boundless;
pub mod zkvm;

/// Use our own version of SessionStats to avoid non-exhaustive issues (risc0_zkvm::SessionStats)
#[derive(Debug, Clone)]
pub struct KailuaSessionStats {
    pub segments: usize,
    pub total_cycles: u64,
    pub user_cycles: u64,
    pub paging_cycles: u64,
    pub reserved_cycles: u64,
}

/// Our own version of ProveInfo to avoid non-exhaustive issues (risc0_zkvm::ProveInfo)
#[derive(Debug)]
pub struct KailuaProveInfo {
    pub receipt: Receipt,
    pub stats: KailuaSessionStats,
}
