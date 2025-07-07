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
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

//! This crate contains the cryptographic primitives and procedures for securely generating
//! Kona block derivation and execution traces usable in the Kailua hybrid dispute engine.

/// Enables the provision of authenticated KZG Blob to the proving pipeline.
pub mod blobs;
/// Structures for representing divided proving workloads.
pub mod boot;
/// Procedures for securely generating and combining stateless Kona client execution traces.
pub mod client;
/// Procedures for generating secure cryptographic commitments to rollup configuration settings.
pub mod config;
/// A module for eigen-da support
#[cfg(feature = "eigen-da")]
pub mod eigen;
/// Implementation for an execution engine with caching support.
pub mod executor;
/// A tightly packed representation for extended execution trace results.
pub mod journal;
/// A modified `kona_proof::l1::chain_provider` with caching support.
pub mod kona;
/// Definitions and implementations of secure stateless oracles for hash preimages.
pub mod oracle;
/// Structures and logic for defining preconditions for Kailua proofs.
pub mod precondition;
/// Utility methods for zero-copy (de)serialization using the `rkyv` crate.
pub mod rkyv;
/// A module for representing oracle-backed stateless client witness data.
pub mod witness;
