//! Raw prost representation of the pinned transaction wire field numbers.
//! Schema reference: quais.js 94e32c7 / src/encoding/protoc/proto_block.proto.
//! Explicit optional scalar/byte presence is consensus-relevant, including zero.
//! These DTOs are unvalidated. Prefer the bounded encode_proto_transaction and
//! decode_proto_transaction entry points over prost's unchecked allocation paths.
//! Typed transaction decoding additionally verifies semantics and signatures.
use prost::Message;

#[derive(Clone, PartialEq, Message)]
/// Raw hash bytes.
pub struct Hash {
    #[prost(bytes = "vec", tag = "1")]
    /// Raw wire field `value`; no semantic validation.
    pub value: Vec<u8>,
}
#[derive(Clone, PartialEq, Message)]
/// Raw address and ordered storage keys.
pub struct AccessTuple {
    #[prost(bytes = "vec", tag = "1")]
    /// Raw wire field `address`; no semantic validation.
    pub address: Vec<u8>,
    #[prost(message, repeated, tag = "2")]
    /// Raw wire field `storage_key`; no semantic validation.
    pub storage_key: Vec<Hash>,
}
#[derive(Clone, PartialEq, Message)]
/// Ordered access tuples.
pub struct AccessList {
    #[prost(message, repeated, tag = "1")]
    /// Raw wire field `access_tuples`; no semantic validation.
    pub access_tuples: Vec<AccessTuple>,
}
#[derive(Clone, PartialEq, Message)]
/// Raw output reference.
pub struct OutPoint {
    #[prost(message, optional, tag = "1")]
    /// Raw wire field `hash`; no semantic validation.
    pub hash: Option<Hash>,
    #[prost(uint32, optional, tag = "2")]
    /// Raw wire field `index`; no semantic validation.
    pub index: Option<u32>,
}
#[derive(Clone, PartialEq, Message)]
/// Raw Qi input.
pub struct Input {
    #[prost(message, optional, tag = "1")]
    /// Raw wire field `previous_out_point`; no semantic validation.
    pub previous_out_point: Option<OutPoint>,
    #[prost(bytes = "vec", optional, tag = "2")]
    /// Raw wire field `pub_key`; no semantic validation.
    pub pub_key: Option<Vec<u8>>,
}
#[derive(Clone, PartialEq, Message)]
/// Ordered raw Qi inputs.
pub struct Inputs {
    #[prost(message, repeated, tag = "1")]
    /// Raw wire field `tx_ins`; no semantic validation.
    pub tx_ins: Vec<Input>,
}
#[derive(Clone, PartialEq, Message)]
/// Raw Qi output, including wire lock presence.
pub struct Output {
    #[prost(uint32, optional, tag = "1")]
    /// Raw wire field `denomination`; no semantic validation.
    pub denomination: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "2")]
    /// Raw wire field `address`; no semantic validation.
    pub address: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "3")]
    /// Raw wire field `lock`; no semantic validation.
    pub lock: Option<Vec<u8>>,
}
#[derive(Clone, PartialEq, Message)]
/// Ordered raw Qi outputs.
pub struct Outputs {
    #[prost(message, repeated, tag = "1")]
    /// Raw wire field `tx_outs`; no semantic validation.
    pub tx_outs: Vec<Output>,
}
#[derive(Clone, PartialEq, Message)]
/// Raw transaction fields; presence and transaction kind require validation.
pub struct Transaction {
    #[prost(uint64, optional, tag = "1")]
    /// Raw wire field `r#type`; no semantic validation.
    pub r#type: Option<u64>,
    #[prost(bytes = "vec", optional, tag = "2")]
    /// Raw wire field `to`; no semantic validation.
    pub to: Option<Vec<u8>>,
    #[prost(uint64, optional, tag = "3")]
    /// Raw wire field `nonce`; no semantic validation.
    pub nonce: Option<u64>,
    #[prost(bytes = "vec", optional, tag = "4")]
    /// Raw wire field `value`; no semantic validation.
    pub value: Option<Vec<u8>>,
    #[prost(uint64, optional, tag = "5")]
    /// Raw wire field `gas`; no semantic validation.
    pub gas: Option<u64>,
    #[prost(bytes = "vec", optional, tag = "6")]
    /// Raw wire field `data`; no semantic validation.
    pub data: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "7")]
    /// Raw wire field `chain_id`; no semantic validation.
    pub chain_id: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "8")]
    /// Raw wire field `gas_price`; no semantic validation.
    pub gas_price: Option<Vec<u8>>,
    #[prost(message, optional, tag = "9")]
    /// Raw wire field `access_list`; no semantic validation.
    pub access_list: Option<AccessList>,
    #[prost(bytes = "vec", optional, tag = "10")]
    /// Raw wire field `v`; no semantic validation.
    pub v: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "11")]
    /// Raw wire field `r`; no semantic validation.
    pub r: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "12")]
    /// Raw wire field `s`; no semantic validation.
    pub s: Option<Vec<u8>>,
    #[prost(message, optional, tag = "13")]
    /// Raw wire field `originating_tx_hash`; no semantic validation.
    pub originating_tx_hash: Option<Hash>,
    #[prost(uint32, optional, tag = "14")]
    /// Raw wire field `etx_index`; no semantic validation.
    pub etx_index: Option<u32>,
    #[prost(message, optional, tag = "15")]
    /// Raw wire field `tx_ins`; no semantic validation.
    pub tx_ins: Option<Inputs>,
    #[prost(message, optional, tag = "16")]
    /// Raw wire field `tx_outs`; no semantic validation.
    pub tx_outs: Option<Outputs>,
    #[prost(bytes = "vec", optional, tag = "17")]
    /// Raw wire field `signature`; no semantic validation.
    pub signature: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "18")]
    /// Raw wire field `etx_sender`; no semantic validation.
    pub etx_sender: Option<Vec<u8>>,
    #[prost(message, optional, tag = "19")]
    /// Raw wire field `parent_hash`; no semantic validation.
    pub parent_hash: Option<Hash>,
    #[prost(message, optional, tag = "20")]
    /// Raw wire field `mix_hash`; no semantic validation.
    pub mix_hash: Option<Hash>,
    #[prost(uint64, optional, tag = "21")]
    /// Raw wire field `work_nonce`; no semantic validation.
    pub work_nonce: Option<u64>,
    #[prost(uint64, optional, tag = "22")]
    /// Raw wire field `etx_type`; no semantic validation.
    pub etx_type: Option<u64>,
}
