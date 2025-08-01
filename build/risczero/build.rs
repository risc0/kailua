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

fn main() {
    #[cfg(feature = "rebuild-fpvm")]
    {
        risc0_build::embed_methods_with_options({
            let guest_options = {
                // set CANOE_IMAGE_ID if not set
                let canoe_image_id = std::env::var("CANOE_IMAGE_ID").unwrap_or_else(|_| {
                    // Warn about unstable build
                    if std::env::var("RISC0_USE_DOCKER").is_err() {
                        println!("cargo:warning=Building without RISC0_USE_DOCKER=1 will yield an irreproducible build for kailua-fpvm-hokulea.");
                    }
                    let canoe_image_id = alloy_primitives::B256::from(bytemuck::cast::<_, [u8; 32]>(
                        canoe_steel_methods::CERT_VERIFICATION_ID,
                    ))
                        .to_string();
                    canoe_image_id
                });
                println!("cargo:rustc-env=CANOE_IMAGE_ID={canoe_image_id}");
                std::env::set_var("CANOE_IMAGE_ID", &canoe_image_id);
                // Start with default build options
                let opts = risc0_build::GuestOptions::default();
                // Build a reproducible ELF file using docker under the release profile
                #[cfg(not(any(feature = "debug-guest-build", debug_assertions)))]
                let opts = {
                    let mut opts = opts;
                    opts.use_docker = Some(
                        risc0_build::DockerOptionsBuilder::default()
                            .docker_container_tag("r0.1.88.0")
                            .root_dir({
                                let cwd = std::env::current_dir().unwrap();
                                cwd.parent()
                                    .unwrap()
                                    .parent()
                                    .map(|d| d.to_path_buf())
                                    .unwrap()
                            })
                            .env(vec![(
                                String::from("CANOE_IMAGE_ID"),
                                canoe_image_id.to_string(),
                            )])
                            .build()
                            .unwrap(),
                    );
                    opts
                };
                // Disable dev-mode receipts from being validated inside the guest
                #[cfg(any(
                    feature = "disable-dev-mode",
                    not(any(feature = "debug-guest-build", debug_assertions))
                ))]
                let opts = {
                    let mut opts = opts;
                    opts.features.push(String::from("disable-dev-mode"));
                    opts
                };
                opts
            };
            std::collections::HashMap::from([
                ("kailua-fpvm-kona", guest_options.clone()),
                ("kailua-fpvm-hokulea", guest_options.clone()),
            ])
        });
    }

    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=fpvm/src");
}
