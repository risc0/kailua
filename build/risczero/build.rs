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

use std::vec;

fn main() {
    // Build a reproducible ELF file using docker under the release profile
    #[cfg(not(any(feature = "debug-guest-build", debug_assertions)))]
    let build_opts = {
        let cwd = std::env::current_dir().unwrap();
        let root_dir = cwd.parent().unwrap().parent().map(|d| d.to_path_buf());
        std::collections::HashMap::from([(
            "kailua-fpvm",
            risc0_build::GuestOptions {
                use_docker: Some(risc0_build::DockerOptions { root_dir }),
                ..Default::default()
            },
        )])
    };

    // Build ELFs natively under debug
    #[cfg(any(feature = "debug-guest-build", debug_assertions))]
    let features = if cfg!(feature = "eigenda") {
        vec!["eigenda".to_string()]
    } else {
        vec![]
    };
    let build_opts = {
        std::collections::HashMap::from([(
            "kailua-fpvm",
            risc0_build::GuestOptions {
                features,
                ..Default::default()
            },
        )])
    };

    risc0_build::embed_methods_with_options(build_opts);
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=fpvm/src");
}
