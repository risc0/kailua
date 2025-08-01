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

use kailua_kona::executor::Execution;

pub mod args;
pub mod backends;
pub mod channel;
pub mod client;
pub mod config;
pub mod kv;
pub mod preflight;
pub mod proof;
pub mod prove;
pub mod tasks;

#[derive(Debug, thiserror::Error)]
pub enum ProvingError {
    #[error("DerivationProofError error: execution proofs {0}")]
    DerivationProofError(usize),

    #[error("NotSeekingProof error: witness {0}")]
    NotSeekingProof(usize, Vec<Vec<Execution>>),

    #[error("NotAwaitingProof error")]
    NotAwaitingProof,

    #[error("WitnessSizeError error: size {0} limit {0}")]
    WitnessSizeError(usize, usize, Vec<Vec<Execution>>),

    #[error("ExecutionError error: ZKVM failed {0:?}")]
    ExecutionError(anyhow::Error),

    #[error("OtherError error: {0:?}")]
    OtherError(anyhow::Error),
}
