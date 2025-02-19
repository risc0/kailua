use crate::oracle::vec::{PreimageVecEntry, PreimageVecStore};
use alloy_consensus::Header;
use alloy_eips::eip4844::{Blob, BYTES_PER_BLOB};
use alloy_primitives::{Address, Sealable, B256, B64};
use alloy_rpc_types_engine::PayloadAttributes;
use c_kzg::Bytes48;
use kona_executor::ExecutionArtifacts;
use op_alloy_rpc_types_engine::OpPayloadAttributes;
use rkyv::rancor::Fallible;
use rkyv::with::{ArchiveWith, DeserializeWith, SerializeWith};
use rkyv::{Archive, Archived, Place, Resolver, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = B256)]
#[rkyv(archived = ArchivedB256)]
pub struct B256Def(pub [u8; 32]);

impl From<B256Def> for B256 {
    fn from(value: B256Def) -> Self {
        B256::new(value.0)
    }
}

#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = B64)]
#[rkyv(archived = ArchivedB64)]
pub struct B64Def(pub [u8; 8]);

impl From<B64Def> for B64 {
    fn from(value: B64Def) -> Self {
        B64::new(value.0)
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = Address)]
#[rkyv(archived = ArchivedAddress)]
#[rkyv(derive(Hash, Eq, PartialEq))]
pub struct AddressDef(pub [u8; 20]);

impl From<AddressDef> for Address {
    fn from(value: AddressDef) -> Self {
        Address::new(value.0)
    }
}

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

pub struct ExecutionArtifactsRkyv;

impl ArchiveWith<ExecutionArtifacts> for ExecutionArtifactsRkyv {
    type Archived = Archived<(Vec<u8>, Vec<u8>)>;
    type Resolver = Resolver<(Vec<u8>, Vec<u8>)>;

    fn resolve_with(
        field: &ExecutionArtifacts,
        resolver: Self::Resolver,
        out: Place<Self::Archived>,
    ) {
        let block_header = alloy_rlp::encode(field.block_header.clone().unseal());
        let receipts = alloy_rlp::encode(field.receipts.clone());
        let field = (block_header, receipts);
        <(Vec<u8>, Vec<u8>) as Archive>::resolve(&field, resolver, out);
    }
}

impl<S> SerializeWith<ExecutionArtifacts, S> for ExecutionArtifactsRkyv
where
    S: Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    <S as Fallible>::Error: rkyv::rancor::Source,
{
    fn serialize_with(
        field: &ExecutionArtifacts,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        let block_header = alloy_rlp::encode(field.block_header.clone().unseal());
        let receipts = alloy_rlp::encode(field.receipts.clone());
        let field = (block_header, receipts);
        <(Vec<u8>, Vec<u8>) as rkyv::Serialize<S>>::serialize(&field, serializer)
    }
}

impl<D: Fallible> DeserializeWith<Archived<(Vec<u8>, Vec<u8>)>, ExecutionArtifacts, D>
    for ExecutionArtifactsRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: rkyv::rancor::Source,
{
    fn deserialize_with(
        field: &Archived<(Vec<u8>, Vec<u8>)>,
        deserializer: &mut D,
    ) -> Result<ExecutionArtifacts, D::Error> {
        let field: (Vec<u8>, Vec<u8>) = rkyv::Deserialize::deserialize(field, deserializer)?;
        let block_header = alloy_rlp::decode_exact::<Header>(field.0.as_slice())
            .unwrap()
            .seal_slow();
        let receipts = alloy_rlp::decode_exact(field.0.as_slice()).unwrap();
        Ok(ExecutionArtifacts {
            block_header,
            receipts,
        })
    }
}

pub struct PreimageVecStoreRkyv;

impl ArchiveWith<PreimageVecStore> for PreimageVecStoreRkyv {
    type Archived = Archived<Vec<PreimageVecEntry>>;
    type Resolver = Resolver<Vec<PreimageVecEntry>>;

    fn resolve_with(
        field: &PreimageVecStore,
        resolver: Self::Resolver,
        out: Place<Self::Archived>,
    ) {
        let locked_vec = field.lock().unwrap();
        <Vec<PreimageVecEntry> as Archive>::resolve(&locked_vec, resolver, out);
    }
}

impl<S> SerializeWith<PreimageVecStore, S> for PreimageVecStoreRkyv
where
    S: Fallible + rkyv::ser::Allocator + rkyv::ser::Writer + ?Sized,
    <S as Fallible>::Error: rkyv::rancor::Source,
{
    fn serialize_with(
        field: &PreimageVecStore,
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        let locked_vec = field.lock().unwrap();
        <Vec<PreimageVecEntry> as Serialize<S>>::serialize(&locked_vec, serializer)
    }
}

impl<D: Fallible> DeserializeWith<Archived<Vec<PreimageVecEntry>>, PreimageVecStore, D>
    for PreimageVecStoreRkyv
where
    D: Fallible + ?Sized,
    <D as Fallible>::Error: rkyv::rancor::Source,
{
    fn deserialize_with(
        field: &Archived<Vec<PreimageVecEntry>>,
        deserializer: &mut D,
    ) -> Result<PreimageVecStore, D::Error> {
        let raw_vec = rkyv::Deserialize::deserialize(field, deserializer)?;
        Ok(Arc::new(Mutex::new(raw_vec)))
    }
}

#[derive(Clone, Debug, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(remote = Blob)]
#[rkyv(archived = ArchivedBlob)]
pub struct BlobDef(pub [u8; BYTES_PER_BLOB]);

impl From<BlobDef> for Blob {
    fn from(value: BlobDef) -> Self {
        Self(value.0)
    }
}

#[derive(
    rkyv::Archive, rkyv::Serialize, rkyv::Deserialize, Debug, Copy, Clone, Hash, PartialEq, Eq,
)]
#[rkyv(remote = Bytes48)]
#[rkyv(archived = ArchivedBytes48)]
pub struct Bytes48Def {
    #[rkyv(getter = get_48_bytes)]
    bytes: [u8; 48usize],
}

fn get_48_bytes(value: &Bytes48) -> [u8; 48] {
    value.into_inner()
}

impl From<Bytes48Def> for Bytes48 {
    fn from(value: Bytes48Def) -> Self {
        Self::from(value.bytes)
    }
}
