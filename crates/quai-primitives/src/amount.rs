use ruint::aliases::{U256, U512};
use thiserror::Error;

/// Errors converting human decimal input to exact integer units.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub enum AmountError {
    /// Syntax is ASCII decimal only, with no whitespace, plus sign or exponent.
    #[error("invalid decimal amount syntax")]
    InvalidSyntax,
    /// The selected unit supports at most 80 decimal places.
    #[error("unsupported decimal unit")]
    InvalidUnit,
    /// Nonzero digits would be lost by scaling to the chosen unit.
    #[error("amount has excess decimal precision")]
    PrecisionLoss,
    /// The amount is outside the documented integer range.
    #[error("amount is outside the supported range")]
    Overflow,
    /// An unsigned chain amount cannot be negative.
    #[error("chain amount cannot be negative")]
    Negative,
    /// Untrusted input is limited to 512 bytes before parsing.
    #[error("decimal input exceeds 512 bytes")]
    InputTooLong,
}

/// A validated decimal scale. Quai uses 18 decimals; Qi uses 3 decimals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Unit(u8);
impl Unit {
    /// The smallest indivisible unit.
    pub const BASE: Self = Self(0);
    /// Quai (10^18 base units).
    pub const QUAI: Self = Self(18);
    /// Qi (1,000 Qits).
    pub const QI: Self = Self(3);
    /// Validate an explicit scale, including token decimals.
    pub const fn new(decimals: u8) -> Result<Self, AmountError> {
        if decimals > 80 {
            Err(AmountError::InvalidUnit)
        } else {
            Ok(Self(decimals))
        }
    }
    /// The number of decimal places.
    pub const fn decimals(self) -> u8 {
        self.0
    }
    /// Resolve the exact legacy quais.js unit names, or the Quai/Qi names.
    pub fn named(name: &str) -> Result<Self, AmountError> {
        Ok(match name {
            "wei" => Self(0),
            "kwei" => Self(3),
            "mwei" => Self(6),
            "gwei" => Self(9),
            "szabo" => Self(12),
            "finney" => Self(15),
            "ether" | "quai" => Self::QUAI,
            "qi" => Self::QI,
            _ => return Err(AmountError::InvalidUnit),
        })
    }
}

/// Signed 512-bit integer units for display arithmetic and legacy JS conversion.
///
/// Range is -2^511 through 2^511-1, matching the reference unit helpers. Chain
/// transaction amounts require the narrower unsigned [`Self::try_u256`] conversion.
/// Zero is always normalized to positive; this type never uses floating point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignedUnits {
    negative: bool,
    magnitude: U512,
}
impl SignedUnits {
    /// Construct a signed integer with checked reference range.
    pub fn new(negative: bool, magnitude: U512) -> Result<Self, AmountError> {
        let boundary = U512::from(1) << 511;
        if magnitude > boundary || (!negative && magnitude == boundary) {
            return Err(AmountError::Overflow);
        }
        Ok(Self {
            negative: negative && magnitude != U512::ZERO,
            magnitude,
        })
    }
    /// Exact unsigned magnitude.
    pub const fn magnitude(self) -> U512 {
        self.magnitude
    }
    /// Whether the value is strictly negative.
    pub const fn is_negative(self) -> bool {
        self.negative
    }
    /// Convert to a nonnegative 256-bit transaction amount.
    pub fn try_u256(self) -> Result<U256, AmountError> {
        if self.negative {
            return Err(AmountError::Negative);
        }
        U256::checked_from_limbs_slice(self.magnitude.as_limbs()).ok_or(AmountError::Overflow)
    }
    /// Produce a minimal decimal string, retaining `.0` for fractional units.
    pub fn format(self, unit: Unit) -> String {
        let digits = self.magnitude.to_string();
        let scale = usize::from(unit.0);
        let mut result = String::with_capacity(1 + digits.len().max(scale + 1) + 1);
        if self.negative {
            result.push('-');
        }
        if scale == 0 {
            result.push_str(&digits);
            return result;
        }
        if digits.len() <= scale {
            result.push_str("0.");
            for _ in digits.len()..scale {
                result.push('0');
            }
            result.push_str(&digits);
        } else {
            let split = digits.len() - scale;
            result.push_str(&digits[..split]);
            result.push('.');
            result.push_str(&digits[split..]);
        }
        while result.ends_with('0') {
            result.pop();
        }
        if result.ends_with('.') {
            result.push('0');
        }
        result
    }
}

/// Parse an exact signed decimal with reference 512-bit range and no rounding.
///
/// Leading/trailing decimal points and redundant zeros match quais.js. Excess
/// fractional zero digits are accepted. Inputs above 512 bytes fail explicitly,
/// even if they contain only redundant zeros, to bound work on untrusted input.
pub fn parse_signed_units(value: &str, unit: Unit) -> Result<SignedUnits, AmountError> {
    if value.len() > 512 {
        return Err(AmountError::InputTooLong);
    }
    let negative = value.starts_with('-');
    let unsigned = if negative { &value[1..] } else { value };
    let mut whole = unsigned;
    let mut fractional = "";
    if let Some((left, right)) = unsigned.split_once('.') {
        whole = left;
        fractional = right;
    }
    if whole.is_empty() && fractional.is_empty() {
        return Err(AmountError::InvalidSyntax);
    }
    if !whole
        .bytes()
        .chain(fractional.bytes())
        .all(|b| b.is_ascii_digit())
    {
        return Err(AmountError::InvalidSyntax);
    }
    let scale = usize::from(unit.0);
    if fractional
        .as_bytes()
        .get(scale..)
        .is_some_and(|tail| tail.iter().any(|&b| b != b'0'))
    {
        return Err(AmountError::PrecisionLoss);
    }
    let mut magnitude = U512::ZERO;
    let decimal_digits = fractional.bytes().take(scale).chain(std::iter::repeat_n(
        b'0',
        scale.saturating_sub(fractional.len()),
    ));
    for digit in whole.bytes().chain(decimal_digits) {
        magnitude = magnitude
            .checked_mul(U512::from(10))
            .and_then(|v| v.checked_add(U512::from(digit - b'0')))
            .ok_or(AmountError::Overflow)?;
    }
    SignedUnits::new(negative, magnitude)
}

/// Parse a nonnegative 256-bit chain amount without rounding or floating point.
pub fn parse_units(value: &str, unit: Unit) -> Result<U256, AmountError> {
    parse_signed_units(value, unit)?.try_u256()
}
/// Format a 256-bit chain amount in the selected decimal unit.
pub fn format_units(value: U256, unit: Unit) -> String {
    SignedUnits {
        negative: false,
        magnitude: U512::from(value),
    }
    .format(unit)
}
/// Parse a human Quai amount (18 decimals) to base units.
pub fn parse_quai(value: &str) -> Result<U256, AmountError> {
    parse_units(value, Unit::QUAI)
}
/// Parse a human Qi amount (3 decimals) to Qits.
pub fn parse_qi(value: &str) -> Result<U256, AmountError> {
    parse_units(value, Unit::QI)
}
/// Format base units as a human Quai amount.
pub fn format_quai(value: U256) -> String {
    format_units(value, Unit::QUAI)
}
/// Format Qits as a human Qi amount.
pub fn format_qi(value: U256) -> String {
    format_units(value, Unit::QI)
}
