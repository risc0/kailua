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

#[cfg(feature = "export-fpvm")]
fn main() {
    use kailua_build::{KAILUA_DA_HOKULEA_ELF, KAILUA_FPVM_HOKULEA_ELF, KAILUA_FPVM_KONA_ELF};
    use std::fs::File;
    use std::io::Write;
    use std::path::Path;

    let package_root = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");

    let programs = [
        (KAILUA_FPVM_KONA_ELF, "kailua-fpvm-kona.bin"),
        (KAILUA_FPVM_HOKULEA_ELF, "kailua-fpvm-hokulea.bin"),
        (KAILUA_DA_HOKULEA_ELF, "kailua-da-hokulea.bin"),
    ];

    for (elf, file_name) in programs {
        let file_path = Path::new(&package_root).join(file_name);
        match File::create(file_path) {
            Ok(mut file) => {
                if let Err(err) = file.write_all(&elf) {
                    println!("cargo:error={err:?}");
                }
                if let Err(err) = file.flush() {
                    println!("cargo:error={err:?}");
                }
                match risc0_zkvm::compute_image_id(elf) {
                    Ok(id) => {
                        let raw_id = id
                            .as_words()
                            .iter()
                            .map(|x| format!("0x{x:X}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        println!("cargo:warning={file_name}: [{raw_id}]");
                    }
                    Err(err) => {
                        println!("cargo:error={err:?}");
                    }
                }
            }
            Err(err) => {
                println!("cargo:error={err:?}");
            }
        }
    }
}

#[cfg(not(feature = "export-fpvm"))]
fn main() {}
