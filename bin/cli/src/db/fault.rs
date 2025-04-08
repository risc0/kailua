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

use crate::db::proposal::Proposal;
use std::cmp::Ordering;

#[derive(Copy, Clone, Debug)]
pub enum Fault {
    Output(usize),
    Null(usize),
}

impl Fault {
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null(_))
    }

    pub fn is_output(&self) -> bool {
        matches!(self, Self::Output(_))
    }

    pub fn divergence_point(&self) -> usize {
        match self {
            Fault::Output(index) | Fault::Null(index) => *index,
        }
    }

    pub fn expect_zero(&self, proposal: &Proposal) -> bool {
        let position = self.divergence_point();
        match position.cmp(&proposal.io_field_elements.len()) {
            Ordering::Less | Ordering::Equal => false,
            Ordering::Greater => true,
        }
    }
}
