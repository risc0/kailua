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

fn main() {
    if cfg!(feature = "rebuild-fpvm") {
        let build_opts = {
            #[cfg(not(any(feature = "debug-guest-build", debug_assertions)))]
            let root_dir = {
                let cwd = std::env::current_dir().unwrap();
                cwd.parent().unwrap().parent().map(|d| d.to_path_buf())
            };
            std::collections::HashMap::from([(
                "kailua-fpvm",
                risc0_build::GuestOptions {
                    // Build a reproducible ELF file using docker under the release profile
                    #[cfg(not(any(feature = "debug-guest-build", debug_assertions)))]
                    use_docker: Some(risc0_build::DockerOptions { root_dir }),
                    // Disable dev-mode receipts from being validated inside the guest
                    features: vec![
                        #[cfg(any(
                            feature = "disable-dev-mode",
                            not(any(feature = "debug-guest-build", debug_assertions))
                        ))]
                        String::from("disable-dev-mode"),
                    ],
                    ..Default::default()
                },
            )])
        };

        risc0_build::embed_methods_with_options(build_opts);
    }

    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=fpvm/src");
}
