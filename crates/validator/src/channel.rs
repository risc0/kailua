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

use alloy::primitives::FixedBytes;
use kailua_common::precondition::PreconditionValidationData;
use risc0_zkvm::Receipt;
use tokio::sync::mpsc::{channel, Receiver, Sender};

/// A channel for two-way communication
#[derive(Debug)]
pub struct DuplexChannel<T> {
    /// Messages coming in
    pub receiver: Receiver<T>,
    /// Messages going out
    pub sender: Sender<T>,
}

impl<T> DuplexChannel<T> {
    /// Returns a pair of duplex channel instances, one for each endpoint
    pub fn new_pair(buffer: usize) -> (Self, Self) {
        let pair_0 = channel(buffer);
        let pair_1 = channel(buffer);
        let channel_0 = Self {
            receiver: pair_1.1,
            sender: pair_0.0,
        };
        let channel_1 = Self {
            receiver: pair_0.1,
            sender: pair_1.0,
        };
        (channel_0, channel_1)
    }
}

#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Message {
    // The proposal and its parent
    Proposal {
        index: u64,
        precondition_validation_data: Option<PreconditionValidationData>,
        l1_head: FixedBytes<32>,
        agreed_l2_head_hash: FixedBytes<32>,
        agreed_l2_output_root: FixedBytes<32>,
        claimed_l2_block_number: u64,
        claimed_l2_output_root: FixedBytes<32>,
    },
    Proof(u64, Option<Receipt>),
}
