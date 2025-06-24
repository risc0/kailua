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

#[derive(Copy, Clone, Debug)]
pub enum Fault {
    /// Denotes a faulty intermediate output commitment
    Output(usize),
    /// Denotes a faulty trailing data field element
    Trail(usize),
}

impl Fault {
    /// Returns true iff this fault denotes a trail data fault
    pub fn is_trail(&self) -> bool {
        matches!(self, Self::Trail(_))
    }

    /// Returns true iff this fault denotes an output commitment fault
    pub fn is_output(&self) -> bool {
        matches!(self, Self::Output(_))
    }

    /// Returns the faulty output index
    pub fn divergence_point(&self) -> usize {
        match self {
            Fault::Output(index) | Fault::Trail(index) => *index,
        }
    }
}
