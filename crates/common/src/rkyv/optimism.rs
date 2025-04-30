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

use crate::rkyv::primitives::{AddressDef, B256Def, B64Def};
use alloy_primitives::{Address, B256, B64};
use alloy_rpc_types_engine::PayloadAttributes;
use op_alloy_rpc_types_engine::OpPayloadAttributes;
use rkyv::rancor::Fallible;
use rkyv::with::{ArchiveWith, DeserializeWith, SerializeWith};
use rkyv::{Archive, Archived, Place, Resolver, Serialize};

#[derive(Clone, Debug, Hash, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct OpPayloadAttributesRkyv {
    pub timestamp: u64,
    #[rkyv(with = B256Def)]
    pub prev_randao: B256,
    #[rkyv(with = AddressDef)]
    pub suggested_fee_recipient: Address,
    pub withdrawals: Option<Vec<u8>>,
    #[rkyv(with = rkyv::with::Map<B256Def>)]
    pub parent_beacon_block_root: Option<B256>,
    pub transactions: Option<Vec<u8>>,
    pub no_tx_pool: Option<bool>,
    pub gas_limit: Option<u64>,
    #[rkyv(with = rkyv::with::Map<B64Def>)]
    pub eip_1559_params: Option<B64>,
}

impl From<&OpPayloadAttributes> for OpPayloadAttributesRkyv {
    fn from(value: &OpPayloadAttributes) -> Self {
        OpPayloadAttributesRkyv {
            timestamp: value.payload_attributes.timestamp,
            prev_randao: value.payload_attributes.prev_randao,
            suggested_fee_recipient: value.payload_attributes.suggested_fee_recipient,
            withdrawals: value
                .payload_attributes
                .withdrawals
                .as_ref()
                .map(alloy_rlp::encode),
            parent_beacon_block_root: value.payload_attributes.parent_beacon_block_root,
            transactions: value.transactions.as_ref().map(alloy_rlp::encode),
            no_tx_pool: value.no_tx_pool,
            gas_limit: value.gas_limit,
            eip_1559_params: value.eip_1559_params,
        }
    }
}

impl From<OpPayloadAttributesRkyv> for OpPayloadAttributes {
    fn from(value: OpPayloadAttributesRkyv) -> Self {
        OpPayloadAttributes {
            payload_attributes: PayloadAttributes {
                timestamp: value.timestamp,
                prev_randao: value.prev_randao,
                suggested_fee_recipient: value.suggested_fee_recipient,
                withdrawals: value
                    .withdrawals
                    .as_ref()
                    .map(|wds| alloy_rlp::decode_exact(wds.as_slice()).unwrap()),
                parent_beacon_block_root: value.parent_beacon_block_root,
            },
            transactions: value
                .transactions
                .as_ref()
                .map(|txs| alloy_rlp::decode_exact(txs.as_slice()).unwrap()),
            no_tx_pool: value.no_tx_pool,
            gas_limit: value.gas_limit,
            eip_1559_params: value.eip_1559_params,
        }
    }
}

impl ArchiveWith<OpPayloadAttributes> for OpPayloadAttributesRkyv {
    type Archived = Archived<OpPayloadAttributesRkyv>;
    type Resolver = Resolver<OpPayloadAttributesRkyv>;

    fn resolve_with(
        field: &OpPayloadAttributes,
        resolver: Self::Resolver,
        out: Place<Self::Archived>,
    ) {
        let field = OpPayloadAttributesRkyv::from(field);
        <OpPayloadAttributesRkyv as Archive>::resolve(&field, resolver, out);
    }
}

impl<S> SerializeWith<OpPayloadAttributes, S> for OpPayloadAttributesRkyv
where
    S: Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    <S as Fallible>::Error: rkyv::rancor::Source,
{
    fn serialize_with(
        field: &OpPayloadAttributes,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        let field = OpPayloadAttributesRkyv::from(field);
        <OpPayloadAttributesRkyv as Serialize<S>>::serialize(&field, serializer)
    }
}

impl<D: Fallible> DeserializeWith<Archived<OpPayloadAttributesRkyv>, OpPayloadAttributes, D>
    for OpPayloadAttributesRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: rkyv::rancor::Source,
{
    fn deserialize_with(
        field: &Archived<OpPayloadAttributesRkyv>,
        deserializer: &mut D,
    ) -> Result<OpPayloadAttributes, D::Error> {
        let field: OpPayloadAttributesRkyv = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(OpPayloadAttributes::from(field))
    }
}
