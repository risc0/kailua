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

use alloy_consensus::Header;
use alloy_eips::eip7685::Requests;
use alloy_evm::block::BlockExecutionResult;
use alloy_primitives::Sealable;
use kona_executor::BlockBuildingOutcome;
use op_alloy_consensus::OpReceiptEnvelope;
use rkyv::rancor::Fallible;
use rkyv::with::{ArchiveWith, DeserializeWith, SerializeWith, With};
use rkyv::{Archive, Archived, Place, Resolver};

pub struct BlockBuildingOutcomeRkyv;

impl ArchiveWith<BlockBuildingOutcome> for BlockBuildingOutcomeRkyv {
    type Archived = Archived<(Vec<u8>, Vec<u8>)>;
    type Resolver = Resolver<(Vec<u8>, Vec<u8>)>;

    fn resolve_with(
        field: &BlockBuildingOutcome,
        resolver: Self::Resolver,
        out: Place<Self::Archived>,
    ) {
        let block_header = alloy_rlp::encode(field.header.clone().unseal());
        let execution_result =
            rkyv::to_bytes::<rkyv::rancor::Error>(With::<_, BlockExecutionResultRkyv>::cast(
                &field.execution_result,
            ))
            .unwrap()
            .to_vec();
        let field = (block_header, execution_result);
        <(Vec<u8>, Vec<u8>) as Archive>::resolve(&field, resolver, out);
    }
}

impl<S> SerializeWith<BlockBuildingOutcome, S> for BlockBuildingOutcomeRkyv
where
    S: Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    <S as Fallible>::Error: rkyv::rancor::Source,
{
    fn serialize_with(
        field: &BlockBuildingOutcome,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        let header = alloy_rlp::encode(field.header.clone().unseal());
        let execution_result =
            rkyv::to_bytes::<rkyv::rancor::Error>(With::<_, BlockExecutionResultRkyv>::cast(
                &field.execution_result,
            ))
            .unwrap()
            .to_vec();
        let field = (header, execution_result);
        <(Vec<u8>, Vec<u8>) as rkyv::Serialize<S>>::serialize(&field, serializer)
    }
}

impl<D: Fallible> DeserializeWith<Archived<(Vec<u8>, Vec<u8>)>, BlockBuildingOutcome, D>
    for BlockBuildingOutcomeRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: rkyv::rancor::Source,
{
    fn deserialize_with(
        field: &Archived<(Vec<u8>, Vec<u8>)>,
        deserializer: &mut D,
    ) -> Result<BlockBuildingOutcome, D::Error> {
        let field: (Vec<u8>, Vec<u8>) = rkyv::Deserialize::deserialize(field, deserializer)?;
        let header = alloy_rlp::decode_exact::<Header>(field.0.as_slice())
            .unwrap()
            .seal_slow();
        let execution_result = {
            let access = rkyv::access(&field.1)?;
            BlockExecutionResultRkyv::deserialize_with(access, deserializer)?
        };
        Ok(BlockBuildingOutcome {
            header,
            execution_result,
        })
    }
}

pub struct BlockExecutionResultRkyv;

impl ArchiveWith<BlockExecutionResult<OpReceiptEnvelope>> for BlockExecutionResultRkyv {
    type Archived = Archived<(Vec<u8>, Vec<u8>, u64)>;
    type Resolver = Resolver<(Vec<u8>, Vec<u8>, u64)>;

    fn resolve_with(
        field: &BlockExecutionResult<OpReceiptEnvelope>,
        resolver: Self::Resolver,
        out: Place<Self::Archived>,
    ) {
        let receipts = alloy_rlp::encode(field.receipts.clone());
        let requests = alloy_rlp::encode(field.requests.clone().take());
        let field = (receipts, requests, field.gas_used);
        <(Vec<u8>, Vec<u8>, u64) as Archive>::resolve(&field, resolver, out);
    }
}

impl<S> SerializeWith<BlockExecutionResult<OpReceiptEnvelope>, S> for BlockExecutionResultRkyv
where
    S: Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    <S as Fallible>::Error: rkyv::rancor::Source,
{
    fn serialize_with(
        field: &BlockExecutionResult<OpReceiptEnvelope>,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        let receipts = alloy_rlp::encode(field.receipts.clone());
        let requests = alloy_rlp::encode(field.requests.clone().take());
        let field = (receipts, requests, field.gas_used);
        <(Vec<u8>, Vec<u8>, u64) as rkyv::Serialize<S>>::serialize(&field, serializer)
    }
}

impl<D: Fallible>
    DeserializeWith<Archived<(Vec<u8>, Vec<u8>, u64)>, BlockExecutionResult<OpReceiptEnvelope>, D>
    for BlockExecutionResultRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: rkyv::rancor::Source,
{
    fn deserialize_with(
        field: &Archived<(Vec<u8>, Vec<u8>, u64)>,
        deserializer: &mut D,
    ) -> Result<BlockExecutionResult<OpReceiptEnvelope>, D::Error> {
        let field: (Vec<u8>, Vec<u8>, u64) = rkyv::Deserialize::deserialize(field, deserializer)?;
        let receipts = alloy_rlp::decode_exact(field.0.as_slice()).unwrap();
        let requests = alloy_rlp::decode_exact(field.1.as_slice()).unwrap();
        Ok(BlockExecutionResult {
            receipts,
            requests: Requests::new(requests),
            gas_used: field.2,
        })
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod tests {
    use super::*;
    use crate::{from_bytes_with, to_bytes_with};
    use alloy_primitives::{keccak256, Address, Bloom, B64, U256};
    use op_alloy_consensus::OpTxType;

    pub fn gen_execution_results(count: usize) -> Vec<BlockExecutionResult<OpReceiptEnvelope>> {
        (0..count)
            .map(|i| BlockExecutionResult {
                receipts: (0..128)
                    .map(|j| {
                        OpReceiptEnvelope::from_parts(
                            j % 2 == 1,
                            21000 + 100 * j,
                            vec![],
                            OpTxType::Legacy,
                            None,
                            None,
                        )
                    })
                    .collect(),
                requests: Requests::new(
                    (0..32)
                        .map(|j| {
                            format!("gen_execution_results requests {i} {j}")
                                .into_bytes()
                                .into()
                        })
                        .collect(),
                ),
                gas_used: U256::from_be_bytes(
                    keccak256(format!("gen_execution_results gas_used {i}")).0,
                )
                .reduce_mod(U256::from(1u128 << 64))
                .to(),
            })
            .collect()
    }

    pub fn gen_execution_outcomes(count: usize) -> Vec<BlockBuildingOutcome> {
        gen_execution_results(count)
            .into_iter()
            .enumerate()
            .map(|(i, execution_result)| BlockBuildingOutcome {
                execution_result,
                header: Header {
                    parent_hash: keccak256(format!("parent_hash {i}")),
                    ommers_hash: keccak256(format!("ommers_hash {i}")),
                    beneficiary: Address::from([0x33; 20]),
                    state_root: keccak256(format!("sate_root {i}")),
                    transactions_root: keccak256(format!("transactions_root {i}")),
                    receipts_root: keccak256(format!("receipts_root {i}")),
                    logs_bloom: Bloom::from([0x77; 256]),
                    difficulty: U256::from_be_bytes(keccak256(format!("difficulty {i}")).0),
                    number: i as u64,
                    gas_limit: 0xaa,
                    gas_used: 0xbb,
                    timestamp: 0xcc,
                    extra_data: format!("extra_data {i}").into_bytes().into(),
                    mix_hash: keccak256(format!("mix_hash {i}")),
                    nonce: B64::from([0xff; 8]),
                    base_fee_per_gas: Some(u64::from_be_bytes([0x01; 8])),
                    withdrawals_root: Some(keccak256(format!("withdrawals_root {i}"))),
                    blob_gas_used: Some(u64::from_be_bytes([0x03; 8])),
                    excess_blob_gas: Some(u64::from_be_bytes([0x04; 8])),
                    parent_beacon_block_root: Some(keccak256(format!(
                        "parent_beacon_block_root {i}"
                    ))),
                    requests_hash: Some(keccak256(format!("requests_hash {i}"))),
                }
                .seal_slow(),
            })
            .collect()
    }

    #[test]
    fn test_block_building_outcome_rkyv() {
        for outcome in gen_execution_outcomes(128) {
            let serialized = to_bytes_with!(BlockBuildingOutcomeRkyv, &outcome);
            let deserialized =
                from_bytes_with!(BlockBuildingOutcomeRkyv, BlockBuildingOutcome, &serialized);

            assert_eq!(outcome.header, deserialized.header);
            assert_eq!(
                outcome.execution_result.gas_used,
                deserialized.execution_result.gas_used
            );
            assert_eq!(
                outcome.execution_result.receipts,
                deserialized.execution_result.receipts
            );
            assert_eq!(
                outcome.execution_result.requests.take(),
                deserialized.execution_result.requests.take()
            );
        }
    }
}
