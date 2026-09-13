use core::{fmt, str::FromStr};
use tiny_keccak::{Hasher, Keccak};

use crate::{ShardError, Zone};

/// The ledger selected by the high bit of an address's second byte.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum Ledger {
    /// Account-based Quai ledger.
    Quai = 0,
    /// UTXO-based Qi ledger.
    Qi = 1,
}

/// Failure to parse an address or validate its ledger and zone.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressError {
    /// Address strings must contain exactly 40 hex digits after an optional `0x`.
    InvalidLength,
    /// Only ASCII hexadecimal characters are accepted.
    InvalidHex,
    /// A mixed-case input fails the Keccak capitalization checksum.
    InvalidChecksum,
    /// The address does not encode a published zone.
    InvalidZone(ShardError),
    /// The typed address requires a different ledger.
    WrongLedger {
        /// Ledger required by the requested address type.
        expected: Ledger,
        /// Ledger encoded in the address.
        actual: Ledger,
    },
}
impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength => f.write_str("address must contain exactly 20 bytes"),
            Self::InvalidHex => f.write_str("address contains non-hexadecimal characters"),
            Self::InvalidChecksum => f.write_str("invalid mixed-case address checksum"),
            Self::InvalidZone(error) => error.fmt(f),
            Self::WrongLedger { expected, actual } => {
                write!(f, "expected {expected:?} address, found {actual:?}")
            }
        }
    }
}
impl std::error::Error for AddressError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidZone(error) => Some(error),
            _ => None,
        }
    }
}

/// An arbitrary 20-byte address with validated string parsing.
///
/// Parsing validates syntax and checksums, matching `quais.js` `getAddress`.
/// It does not require a known zone. Use [`QuaiAddress`] or [`QiAddress`] when
/// accepting a ledger-specific destination, or call [`Address::zone`] explicitly.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Address([u8; 20]);

impl Address {
    /// The all-zero address. Applications must decide whether it is a valid destination.
    pub const ZERO: Self = Self([0; 20]);

    /// Construct from an exact-size byte array. No zone or ledger is implied.
    pub const fn from_bytes(bytes: [u8; 20]) -> Self {
        Self(bytes)
    }

    /// Borrow the underlying bytes.
    pub const fn bytes(&self) -> &[u8; 20] {
        &self.0
    }

    /// Return the ledger encoded by bit 7 of byte 1.
    pub const fn ledger(self) -> Ledger {
        if self.0[1] & 0x80 == 0 {
            Ledger::Quai
        } else {
            Ledger::Qi
        }
    }

    /// Decode the first byte as a published zone, rejecting unknown values.
    pub const fn zone(self) -> Result<Zone, ShardError> {
        Zone::from_byte(self.0[0])
    }

    /// Format as a `0x`-prefixed mixed-case Keccak checksum address.
    pub fn to_checksum(self) -> String {
        self.to_string()
    }

    /// Parse only the exact `0x`-prefixed checksum spelling, matching the pinned
    /// `validateAddress` policy. Ordinary `FromStr` also accepts uniform case and
    /// an omitted prefix; this explicit strict import rejects those spellings
    /// whenever they differ from the canonical checksum string.
    pub fn from_checksummed_str(text: &str) -> Result<Self, AddressError> {
        let address: Self = text.parse()?;
        if address.to_checksum() != text {
            return Err(AddressError::InvalidChecksum);
        }
        Ok(address)
    }

    fn checksum_bytes(self) -> [u8; 42] {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = [0; 42];
        output[0] = b'0';
        output[1] = b'x';
        for (i, byte) in self.0.iter().enumerate() {
            output[2 + i * 2] = HEX[(byte >> 4) as usize];
            output[3 + i * 2] = HEX[(byte & 0x0f) as usize];
        }
        let mut hash = [0; 32];
        let mut keccak = Keccak::v256();
        keccak.update(&output[2..]);
        keccak.finalize(&mut hash);
        for i in 0..40 {
            let nibble = if i % 2 == 0 {
                hash[i / 2] >> 4
            } else {
                hash[i / 2] & 0x0f
            };
            if nibble >= 8 {
                output[i + 2] = output[i + 2].to_ascii_uppercase();
            }
        }
        output
    }
}

impl From<[u8; 20]> for Address {
    fn from(value: [u8; 20]) -> Self {
        Self::from_bytes(value)
    }
}
impl From<Address> for [u8; 20] {
    fn from(value: Address) -> Self {
        value.0
    }
}
impl AsRef<[u8]> for Address {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}
impl TryFrom<&[u8]> for Address {
    type Error = AddressError;
    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let bytes = value.try_into().map_err(|_| AddressError::InvalidLength)?;
        Ok(Self(bytes))
    }
}
impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.checksum_bytes();
        let address = core::str::from_utf8(&bytes).map_err(|_| fmt::Error)?;
        f.write_str(address)
    }
}
impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl FromStr for Address {
    type Err = AddressError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        // Deliberately do not accept `0X` or trim whitespace: pinned quais.js
        // requires an optional lowercase prefix and exactly 40 hex digits.
        let hex = value.strip_prefix("0x").unwrap_or(value).as_bytes();
        if hex.len() != 40 {
            return Err(AddressError::InvalidLength);
        }
        let mut bytes = [0; 20];
        let mut has_lower = false;
        let mut has_upper = false;
        for (i, digit) in hex.iter().copied().enumerate() {
            let nibble = match digit {
                b'0'..=b'9' => digit - b'0',
                b'a'..=b'f' => {
                    has_lower = true;
                    digit - b'a' + 10
                }
                b'A'..=b'F' => {
                    has_upper = true;
                    digit - b'A' + 10
                }
                _ => return Err(AddressError::InvalidHex),
            };
            bytes[i / 2] |= nibble << if i % 2 == 0 { 4 } else { 0 };
        }
        let address = Self(bytes);
        if has_lower && has_upper && address.checksum_bytes()[2..] != *hex {
            return Err(AddressError::InvalidChecksum);
        }
        Ok(address)
    }
}

macro_rules! ledger_address {
    ($name:ident, $ledger:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name {
            address: Address,
            zone: Zone,
        }

        impl $name {
            /// The checked ledger-independent address.
            pub const fn address(self) -> Address {
                self.address
            }
            /// Borrow the underlying bytes.
            pub const fn bytes(&self) -> &[u8; 20] {
                self.address.bytes()
            }
            /// The validated zone. This does not imply network activation.
            pub const fn zone(self) -> Zone {
                self.zone
            }
            /// The ledger enforced by this address type.
            pub const fn ledger(self) -> Ledger {
                Ledger::$ledger
            }
        }
        impl TryFrom<Address> for $name {
            type Error = AddressError;
            fn try_from(address: Address) -> Result<Self, Self::Error> {
                let zone = address.zone().map_err(AddressError::InvalidZone)?;
                let actual = address.ledger();
                if actual != Ledger::$ledger {
                    return Err(AddressError::WrongLedger {
                        expected: Ledger::$ledger,
                        actual,
                    });
                }
                Ok(Self { address, zone })
            }
        }
        impl TryFrom<[u8; 20]> for $name {
            type Error = AddressError;
            fn try_from(bytes: [u8; 20]) -> Result<Self, Self::Error> {
                Address::from_bytes(bytes).try_into()
            }
        }
        impl FromStr for $name {
            type Err = AddressError;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                value.parse::<Address>()?.try_into()
            }
        }
        impl From<$name> for Address {
            fn from(value: $name) -> Self {
                value.address
            }
        }
        impl AsRef<[u8]> for $name {
            fn as_ref(&self) -> &[u8] {
                self.address.as_ref()
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.address.fmt(f)
            }
        }
    };
}
ledger_address!(
    QuaiAddress,
    Quai,
    "An address with a published zone and the Quai ledger bit."
);
ledger_address!(
    QiAddress,
    Qi,
    "An address with a published zone and the Qi ledger bit."
);
