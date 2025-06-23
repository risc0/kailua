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

use alloy::consensus::Blob;
use alloy::eips::eip4844::{BLS_MODULUS, FIELD_ELEMENTS_PER_BLOB};
use alloy::primitives::U256;
use anyhow::bail;
use std::ops::{Div, Sub};

pub const PRIMITIVE_ROOT_OF_UNITY: U256 = U256::from_limbs([7, 0, 0, 0]);
// primitive_root = 7
// bls_mod = 52435875175126190479447740508185965837690552500527637822603658699938581184513
// pow(primitive_root, (bls_mod - 1) // (2 ** 12), bls_mod)
// 39033254847818212395286706435128746857159659164139250548781411570340225835782
pub const FE_ORDER_PO2: u32 = 12;

pub fn reverse_bits(index: u128, order_po2: u32) -> u128 {
    index.reverse_bits() >> (u128::BITS - order_po2)
}

pub fn root_of_unity(index: usize) -> U256 {
    let primitive_root_exponent = BLS_MODULUS
        .sub(U256::from(1))
        .div(U256::from(FIELD_ELEMENTS_PER_BLOB));
    let root = PRIMITIVE_ROOT_OF_UNITY.pow_mod(primitive_root_exponent, BLS_MODULUS);
    let root_exponent = reverse_bits(index as u128, FE_ORDER_PO2);
    root.pow_mod(U256::from(root_exponent), BLS_MODULUS)
}

pub fn blob_fe_proof(
    blob: &Blob,
    index: usize,
) -> anyhow::Result<(c_kzg::Bytes48, c_kzg::Bytes32)> {
    let bytes = root_of_unity(index).to_be_bytes();
    let z = c_kzg::Bytes32::new(bytes);
    let c_kzg_blob = c_kzg::Blob::from_bytes(blob.as_slice())?;
    let settings = alloy::consensus::EnvKzgSettings::default();
    let (proof, value) = settings.get().compute_kzg_proof(&c_kzg_blob, &z)?;

    let commitment = settings.get().blob_to_kzg_commitment(&c_kzg_blob)?;

    let proof_bytes = proof.to_bytes();
    if settings
        .get()
        .verify_kzg_proof(&commitment.to_bytes(), &z, &value, &proof_bytes)?
    {
        Ok((proof_bytes, value))
    } else {
        bail!("Generated invalid kzg proof.")
    }
}
