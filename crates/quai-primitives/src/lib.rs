//! Validated, allocation-free representations of Quai addresses and shards.
//!
//! [`Address`] accepts any 20-byte address, matching `quais.js`'s `getAddress`.
//! [`QuaiAddress`] and [`QiAddress`] additionally require a known zone and the
//! appropriate ledger bit. A known zone does not imply that a chain has activated
//! that zone; providers must check network capabilities separately.
//!
//! Address strings use the same mixed-case Keccak checksum as `quais.js`.
//! Lowercase and uppercase inputs are accepted; mixed-case inputs are checked.

mod fixed;
pub use fixed::{FixedError, FixedFormat, FixedPoint, Rounding};
mod encoding;
pub use encoding::{
    EncodingError, MAX_BASE58_BYTES, MAX_ENCODING_BYTES, concat_bytes, data_slice, decode_base58,
    decode_base58_bytes, decode_base64, decode_bytes32, encode_base58, encode_base64,
    encode_bytes32, from_twos, get_bytes, hexlify, mask, strip_zeros_left, to_twos, zero_pad_bytes,
    zero_pad_value,
};
mod address;
mod amount;
mod contract;
mod hash;
mod shard;

pub use address::{Address, AddressError, Ledger, QiAddress, QuaiAddress};
pub use amount::{
    AmountError, SignedUnits, Unit, format_qi, format_quai, format_units, parse_qi, parse_quai,
    parse_signed_units, parse_units,
};
pub use contract::{contract_address, create2_address};
pub use hash::{Hash32, Hash32Error};
pub use shard::{Region, Shard, ShardError, Zone};
