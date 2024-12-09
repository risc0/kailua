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

extern crate alloc;

use c_kzg::KzgSettings;
use lazy_static::lazy_static;

#[cfg(target_os = "zkvm")]
#[c_kzg::risc0_c_kzg_alloc_mod]
pub mod c_kzg_alloc {
    // proc macro inserts calloc/malloc/free definitions here

    #[no_mangle]
    pub extern "C" fn __assert_func(
        _file: *const i8,
        _line: i32,
        _func: *const i8,
        _expr: *const i8,
    ) {
        panic!("c_kzg assertion failure.");
    }
}

// todo: hardcode without serde in guest image
#[cfg(target_os = "zkvm")]
lazy_static! {
    /// KZG Ceremony data
    pub static ref KZG: (Vec<u8>, KzgSettings) = {
        let mut data = Vec::from(include_bytes!("../kzg_settings_raw.bin"));
        let settings = KzgSettings::from_u8_slice(&mut data);
        (data, settings)
    };
}

#[cfg(not(target_os = "zkvm"))]
lazy_static! {
    pub static ref KZG: alloy_eips::eip4844::env_settings::EnvKzgSettings = Default::default();
}

pub fn kzg_settings() -> &'static KzgSettings {
    #[cfg(target_os = "zkvm")]
    return &KZG.1;

    #[cfg(not(target_os = "zkvm"))]
    return KZG.get();
}
