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

use kona_preimage::{HintWriter, OracleReader};
use kona_std_fpvm::{FileChannel, FileDescriptor};

pub mod proving;
pub mod witgen;

/// The global preimage oracle reader pipe.
static ORACLE_READER_PIPE: FileChannel =
    FileChannel::new(FileDescriptor::PreimageRead, FileDescriptor::PreimageWrite);
/// The global hint writer pipe.
static HINT_WRITER_PIPE: FileChannel =
    FileChannel::new(FileDescriptor::HintRead, FileDescriptor::HintWrite);
/// The global preimage oracle reader.
pub static ORACLE_READER: OracleReader<FileChannel> = OracleReader::new(ORACLE_READER_PIPE);
/// The global hint writer.
pub static HINT_WRITER: HintWriter<FileChannel> = HintWriter::new(HINT_WRITER_PIPE);
