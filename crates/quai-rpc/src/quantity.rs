//! JSON-RPC quantities are hexadecimal integers, not byte strings.

pub use ruint::aliases::U256;
use thiserror::Error;

/// A malformed or overflowing 256-bit JSON-RPC quantity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
#[error("expected a canonical 0x-prefixed quantity of at most 256 bits")]
pub struct QuantityError;

/// Parse an unsigned quantity, rejecting empty, padded and overflowing values.
pub fn parse_quantity(value: &str) -> Result<U256, QuantityError> {
    let digits = value.strip_prefix("0x").ok_or(QuantityError)?;
    if digits.is_empty()
        || digits.len() > 64
        || (digits.len() > 1 && digits.starts_with('0'))
        || !digits.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(QuantityError);
    }
    U256::from_str_radix(digits, 16).map_err(|_| QuantityError)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_quantities_and_full_width() {
        assert_eq!(parse_quantity("0x0"), Ok(U256::ZERO));
        assert_eq!(parse_quantity("0x3a98"), Ok(U256::from(15000)));
        assert_eq!(
            parse_quantity(&format!("0x{}", "f".repeat(64))),
            Ok(U256::MAX)
        );
        for bad in ["", "0x", "0X1", "0x00", "0x01", "0x-1", "0xg", " 0x1"] {
            assert!(parse_quantity(bad).is_err(), "{bad}");
        }
        assert!(parse_quantity(&format!("0x1{}", "0".repeat(64))).is_err());
    }
}
