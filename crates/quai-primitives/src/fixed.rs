//! Checked fixed-point decimal arithmetic with explicit rounding policy.
use crate::{AmountError, SignedUnits, Unit, from_twos, parse_signed_units, to_twos};
use ruint::{
    Uint,
    aliases::{U256, U512},
};
use std::{cmp::Ordering, fmt, str::FromStr};
use thiserror::Error;
type Wide = Uint<1024, 16>;
/// An explicit signed/unsigned byte-aligned field of 8..=256 bits and 0..=80 decimals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FixedFormat {
    signed: bool,
    width: u16,
    unit: Unit,
}
impl FixedFormat {
    /// Validate the integer field width and decimal scale.
    pub fn new(signed: bool, width: u16, decimals: u8) -> Result<Self, FixedError> {
        if !(8..=256).contains(&width) || !width.is_multiple_of(8) {
            return Err(FixedError::Format);
        }
        Ok(Self {
            signed,
            width,
            unit: Unit::new(decimals).map_err(|_| FixedError::Format)?,
        })
    }
    /// Whether negative values are permitted.
    pub fn signed(self) -> bool {
        self.signed
    }
    /// Integer storage width in bits.
    pub fn width(self) -> u16 {
        self.width
    }
    /// Decimal scale applied to the integer units.
    pub fn unit(self) -> Unit {
        self.unit
    }
}
impl Default for FixedFormat {
    fn default() -> Self {
        Self {
            signed: true,
            width: 128,
            unit: Unit::QUAI,
        }
    }
}
impl fmt::Display for FixedFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}fixed{}x{}",
            if self.signed { "" } else { "u" },
            self.width,
            self.unit.decimals()
        )
    }
}
impl FromStr for FixedFormat {
    type Err = FixedError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.len() > 32 {
            return Err(FixedError::Format);
        }
        let (signed, rest) = if let Some(rest) = text.strip_prefix("ufixed") {
            (false, rest)
        } else {
            (true, text.strip_prefix("fixed").ok_or(FixedError::Format)?)
        };
        if rest.is_empty() {
            return Self::new(signed, 128, 18);
        }
        let (width, decimals) = rest.split_once('x').ok_or(FixedError::Format)?;
        if width.is_empty()
            || decimals.is_empty()
            || !width
                .bytes()
                .chain(decimals.bytes())
                .all(|b| b.is_ascii_digit())
        {
            return Err(FixedError::Format);
        }
        Self::new(
            signed,
            width.parse().map_err(|_| FixedError::Format)?,
            decimals.parse().map_err(|_| FixedError::Format)?,
        )
    }
}
/// How to handle discarded fractional units. Overflow is always an error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rounding {
    /// Reject any nonzero discarded fraction.
    Exact,
    /// Discard the fractional magnitude, toward zero.
    TowardZero,
    /// Round toward negative infinity.
    Floor,
    /// Round toward positive infinity.
    Ceiling,
    /// Round to nearest, choosing an even retained integer on exact ties.
    NearestTiesEven,
    /// Round to nearest, choosing the value toward positive infinity on exact ties.
    NearestTiesPositive,
}
/// Fixed-point validation and arithmetic errors never retain input strings.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
#[non_exhaustive]
pub enum FixedError {
    /// Unsupported format, bit width or decimal scale.
    #[error("invalid fixed-point format")]
    Format,
    /// Arithmetic operands must have the same explicit field; comparison may cross fields.
    #[error("fixed-point operand formats differ")]
    FormatMismatch,
    /// A result does not fit its signed/unsigned field.
    #[error("fixed-point overflow")]
    Overflow,
    /// Exact rounding was requested but fractional units would be lost.
    #[error("fixed-point precision loss")]
    PrecisionLoss,
    /// Division by zero.
    #[error("fixed-point division by zero")]
    DivisionByZero,
    /// Invalid bounded decimal input.
    #[error(transparent)]
    Amount(#[from] AmountError),
}
/// An exact decimal value with a checked integer field. Arithmetic never uses
/// floating point or silently wraps overflow. Explicit wrapping methods opt into
/// modular arithmetic, and to_f64_lossy is a separate presentation conversion.
/// Comparisons compare numeric value
/// across formats; arithmetic requires equal formats unless explicitly rescaled.
#[derive(Clone, Copy, Debug)]
pub struct FixedPoint {
    units: SignedUnits,
    format: FixedFormat,
}
fn tens(decimals: u8) -> Wide {
    (0..decimals).fold(Wide::from(1), |v, _| v * Wide::from(10))
}
fn rounded(
    value: Wide,
    denominator: Wide,
    negative: bool,
    rounding: Rounding,
) -> Result<Wide, FixedError> {
    if denominator == Wide::ZERO {
        return Err(FixedError::DivisionByZero);
    }
    let quotient = value / denominator;
    let remainder = value % denominator;
    if remainder == Wide::ZERO {
        return Ok(quotient);
    }
    let increment = match rounding {
        Rounding::Exact => return Err(FixedError::PrecisionLoss),
        Rounding::TowardZero => false,
        Rounding::Floor => negative,
        Rounding::Ceiling => !negative,
        Rounding::NearestTiesEven | Rounding::NearestTiesPositive => {
            // Compare r with d-r, avoiding an overflowing doubled remainder.
            match remainder.cmp(&(denominator - remainder)) {
                Ordering::Greater => true,
                Ordering::Less => false,
                Ordering::Equal => {
                    if rounding == Rounding::NearestTiesEven {
                        quotient.bit(0)
                    } else {
                        !negative
                    }
                }
            }
        }
    };
    quotient
        .checked_add(Wide::from(u8::from(increment)))
        .ok_or(FixedError::Overflow)
}
impl FixedPoint {
    fn parts(negative: bool, magnitude: Wide, format: FixedFormat) -> Result<Self, FixedError> {
        let limit = Wide::from(1) << usize::from(format.width - u16::from(format.signed));
        if (!format.signed && negative && magnitude != Wide::ZERO)
            || magnitude > limit
            || (magnitude == limit && !(format.signed && negative))
        {
            return Err(FixedError::Overflow);
        }
        let magnitude =
            U512::checked_from_limbs_slice(magnitude.as_limbs()).ok_or(FixedError::Overflow)?;
        Ok(Self {
            units: SignedUnits::new(negative, magnitude).map_err(|_| FixedError::Overflow)?,
            format,
        })
    }
    /// Parse a bounded exact decimal string; excess nonzero precision is rejected.
    pub fn parse(text: &str, format: FixedFormat) -> Result<Self, FixedError> {
        let units = parse_signed_units(text, format.unit)?;
        Self::parts(units.is_negative(), Wide::from(units.magnitude()), format)
    }
    /// Interpret signed integer units at an explicit source scale, rescaling exactly.
    pub fn from_scaled_units(
        units: SignedUnits,
        source: Unit,
        format: FixedFormat,
    ) -> Result<Self, FixedError> {
        let numerator = Wide::from(units.magnitude())
            .checked_mul(tens(format.unit.decimals()))
            .ok_or(FixedError::Overflow)?;
        let magnitude = rounded(
            numerator,
            tens(source.decimals()),
            units.is_negative(),
            Rounding::Exact,
        )?;
        Self::parts(units.is_negative(), magnitude, format)
    }
    /// Interpret at most 32 bytes as a big-endian pattern of the complete
    /// field width. Redundant leading zeros are accepted; short signed patterns
    /// are zero-extended, matching the reference.
    pub fn from_bytes(bytes: &[u8], format: FixedFormat) -> Result<Self, FixedError> {
        if bytes.len() > 32 {
            return Err(FixedError::Overflow);
        }
        let bytes = crate::strip_zeros_left(bytes);
        if bytes.len() > usize::from(format.width / 8) {
            return Err(FixedError::Overflow);
        }
        let value = U256::from_be_slice(bytes);
        let units = if format.signed {
            from_twos(value, format.width).map_err(|_| FixedError::Overflow)?
        } else {
            SignedUnits::new(false, U512::from(value)).map_err(|_| FixedError::Overflow)?
        };
        Self::parts(units.is_negative(), Wide::from(units.magnitude()), format)
    }
    /// Exact signed integer units behind the decimal representation.
    pub fn units(self) -> SignedUnits {
        self.units
    }
    /// Explicit field format.
    pub fn format(self) -> FixedFormat {
        self.format
    }
    /// Whether the exact numeric value is zero.
    pub fn is_zero(self) -> bool {
        self.units.magnitude() == U512::ZERO
    }
    /// Whether the exact numeric value is negative; zero is always normalized.
    pub fn is_negative(self) -> bool {
        self.units.is_negative()
    }
    /// Encode exactly width/8 bytes, using two's complement for signed fields.
    pub fn to_bytes(self) -> Vec<u8> {
        let value = if self.format.signed {
            to_twos(self.units, self.format.width).expect("validated fixed field")
        } else {
            self.units
                .try_u256()
                .expect("validated unsigned fixed field")
        };
        value.to_be_bytes::<32>()[32 - usize::from(self.format.width / 8)..].to_vec()
    }
    fn same(self, other: Self) -> Result<(), FixedError> {
        if self.format != other.format {
            Err(FixedError::FormatMismatch)
        } else {
            Ok(())
        }
    }
    fn sum_parts(self, negative: bool, magnitude: Wide) -> Result<(bool, Wide), FixedError> {
        let own = Wide::from(self.units.magnitude());
        let (sign, result) = if self.is_negative() == negative {
            (
                negative,
                own.checked_add(magnitude).ok_or(FixedError::Overflow)?,
            )
        } else if own >= magnitude {
            (self.is_negative(), own - magnitude)
        } else {
            (negative, magnitude - own)
        };
        Ok((sign, result))
    }
    fn add_parts(self, negative: bool, magnitude: Wide) -> Result<Self, FixedError> {
        let (sign, result) = self.sum_parts(negative, magnitude)?;
        Self::parts(sign, result, self.format)
    }
    fn wrapping_parts(
        negative: bool,
        magnitude: Wide,
        format: FixedFormat,
    ) -> Result<Self, FixedError> {
        let modulus = Wide::from(1) << usize::from(format.width);
        let residue = magnitude & (modulus - Wide::from(1));
        let residue = if negative && residue != Wide::ZERO {
            modulus - residue
        } else {
            residue
        };
        if format.signed && residue.bit(usize::from(format.width - 1)) {
            Self::parts(true, modulus - residue, format)
        } else {
            Self::parts(false, residue, format)
        }
    }
    /// Add equal-format values modulo 2^width, interpreting signed results in
    /// two's complement. Overflow wraps only through this explicitly named API.
    pub fn wrapping_add(self, other: Self) -> Result<Self, FixedError> {
        self.same(other)?;
        let (negative, value) =
            self.sum_parts(other.is_negative(), Wide::from(other.units.magnitude()))?;
        Self::wrapping_parts(negative, value, self.format)
    }
    /// Subtract equal-format values modulo 2^width, including signed minimums.
    pub fn wrapping_sub(self, other: Self) -> Result<Self, FixedError> {
        self.same(other)?;
        let (negative, value) =
            self.sum_parts(!other.is_negative(), Wide::from(other.units.magnitude()))?;
        Self::wrapping_parts(negative, value, self.format)
    }
    /// Multiply equal-format values, truncate fractional scaled units toward zero,
    /// then wrap modulo 2^width. Use checked_mul for explicit rounding/overflow checks.
    pub fn wrapping_mul(self, other: Self) -> Result<Self, FixedError> {
        self.same(other)?;
        let value = Wide::from(self.units.magnitude())
            .checked_mul(Wide::from(other.units.magnitude()))
            .ok_or(FixedError::Overflow)?
            / tens(self.format.unit.decimals());
        Self::wrapping_parts(
            self.is_negative() != other.is_negative(),
            value,
            self.format,
        )
    }
    /// Divide equal-format values, truncate toward zero, then wrap modulo
    /// 2^width. Division by zero and mismatched formats always return errors.
    pub fn wrapping_div(self, other: Self) -> Result<Self, FixedError> {
        self.same(other)?;
        if other.is_zero() {
            return Err(FixedError::DivisionByZero);
        }
        let value = Wide::from(self.units.magnitude())
            .checked_mul(tens(self.format.unit.decimals()))
            .ok_or(FixedError::Overflow)?
            / Wide::from(other.units.magnitude());
        Self::wrapping_parts(
            self.is_negative() != other.is_negative(),
            value,
            self.format,
        )
    }
    /// Explicit approximate f64 display conversion. Rounding may discard integer
    /// or fractional precision; never use this result to authorize chain amounts.
    /// The supported fixed formats have finite values within the f64 exponent range.
    pub fn to_f64_lossy(self) -> f64 {
        self.to_string()
            .parse()
            .expect("validated bounded decimal is a finite f64 input")
    }
    /// Add equal-format values with overflow checks.
    pub fn checked_add(self, other: Self) -> Result<Self, FixedError> {
        self.same(other)?;
        self.add_parts(other.is_negative(), Wide::from(other.units.magnitude()))
    }
    /// Subtract equal-format values with overflow checks, including minimum signed values.
    pub fn checked_sub(self, other: Self) -> Result<Self, FixedError> {
        self.same(other)?;
        self.add_parts(!other.is_negative(), Wide::from(other.units.magnitude()))
    }
    /// Multiply using wide intermediate units and an explicit precision policy.
    pub fn checked_mul(self, other: Self, rounding: Rounding) -> Result<Self, FixedError> {
        self.same(other)?;
        let negative = self.is_negative() != other.is_negative();
        let product = Wide::from(self.units.magnitude())
            .checked_mul(Wide::from(other.units.magnitude()))
            .ok_or(FixedError::Overflow)?;
        Self::parts(
            negative,
            rounded(
                product,
                tens(self.format.unit.decimals()),
                negative,
                rounding,
            )?,
            self.format,
        )
    }
    /// Divide using wide intermediate units and an explicit precision policy.
    pub fn checked_div(self, other: Self, rounding: Rounding) -> Result<Self, FixedError> {
        self.same(other)?;
        let negative = self.is_negative() != other.is_negative();
        let numerator = Wide::from(self.units.magnitude())
            .checked_mul(tens(self.format.unit.decimals()))
            .ok_or(FixedError::Overflow)?;
        Self::parts(
            negative,
            rounded(
                numerator,
                Wide::from(other.units.magnitude()),
                negative,
                rounding,
            )?,
            self.format,
        )
    }
    /// Change field width/scale with explicit rounding and overflow checks.
    pub fn with_format(self, format: FixedFormat, rounding: Rounding) -> Result<Self, FixedError> {
        let numerator = Wide::from(self.units.magnitude())
            .checked_mul(tens(format.unit.decimals()))
            .ok_or(FixedError::Overflow)?;
        let magnitude = rounded(
            numerator,
            tens(self.format.unit.decimals()),
            self.is_negative(),
            rounding,
        )?;
        Self::parts(self.is_negative(), magnitude, format)
    }
    /// Round to at most 80 decimal places, retaining the original field format.
    pub fn round(self, decimals: u8, rounding: Rounding) -> Result<Self, FixedError> {
        if decimals > 80 {
            return Err(FixedError::Format);
        }
        if decimals >= self.format.unit.decimals() {
            return Ok(self);
        }
        let factor = tens(self.format.unit.decimals() - decimals);
        let magnitude = rounded(
            Wide::from(self.units.magnitude()),
            factor,
            self.is_negative(),
            rounding,
        )?
        .checked_mul(factor)
        .ok_or(FixedError::Overflow)?;
        Self::parts(self.is_negative(), magnitude, self.format)
    }
    /// Mathematical floor, including negative fractions. Overflow still fails.
    pub fn floor(self) -> Result<Self, FixedError> {
        self.round(0, Rounding::Floor)
    }
    /// Mathematical ceiling, including positive fractions. Overflow still fails.
    pub fn ceiling(self) -> Result<Self, FixedError> {
        self.round(0, Rounding::Ceiling)
    }
    /// Exact numeric comparison across different field widths and scales.
    pub fn compare(self, other: Self) -> Ordering {
        if self.is_negative() != other.is_negative() {
            return if self.is_negative() {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        let a = Wide::from(self.units.magnitude()) * tens(other.format.unit.decimals());
        let b = Wide::from(other.units.magnitude()) * tens(self.format.unit.decimals());
        if self.is_negative() {
            b.cmp(&a)
        } else {
            a.cmp(&b)
        }
    }
    /// Convert to nonnegative exact chain units at the requested scale. No rounding.
    pub fn to_chain_units(self, unit: Unit) -> Result<U256, FixedError> {
        self.with_format(
            FixedFormat::new(false, 256, unit.decimals())?,
            Rounding::Exact,
        )?
        .units
        .try_u256()
        .map_err(FixedError::Amount)
    }
}
impl fmt::Display for FixedPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.units.format(self.format.unit))
    }
}
impl PartialEq for FixedPoint {
    fn eq(&self, other: &Self) -> bool {
        self.compare(*other) == Ordering::Equal
    }
}
impl Eq for FixedPoint {}
impl PartialOrd for FixedPoint {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for FixedPoint {
    fn cmp(&self, other: &Self) -> Ordering {
        self.compare(*other)
    }
}
