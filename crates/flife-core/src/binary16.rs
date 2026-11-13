//! IEEE-754 binary16 weights for the compact-F16 numerical profile (plan
//! §10.3, "Compact F16": "F16 with declared rounding", a distinct
//! quantized evolutionary model).
//!
//! A compact-F16 genome keeps each parameter as the 16 bits of a binary16
//! value ([`F16`]). Its value is defined by widening, which is exact: every
//! binary16 value, subnormals included, is also an F32 and an F64 value. So
//! evaluation reads a weight as [`F16::to_f32`] and then runs the F32
//! reference arithmetic unchanged.
//!
//! The declared rounding: a value computed wider (initialization and
//! mutation work in F64) becomes canonical by one rounding,
//! [`F16::from_f64`], to nearest with ties to even, directly from F64.
//! Rounding to F32 first and then to binary16 could round twice and differ
//! in the last bit. Magnitudes from 65,520 up round to infinity; NaN stays
//! NaN (a canonical quiet NaN with the input's sign).
//!
//! Only IEEE-754 bit manipulation and integer arithmetic are used, so every
//! host computes the same bits.

/// A binary16 value, as its 16 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct F16(u16);

/// The sign bit.
const SIGN: u16 = 0x8000;
/// Every exponent bit set: infinity or NaN.
const EXPONENT_MASK: u16 = 0x7C00;
/// The fraction bits.
const FRACTION_MASK: u16 = 0x03FF;
/// The canonical quiet NaN's magnitude bits.
const QUIET_NAN: u16 = 0x7E00;
/// 2^-24, the smallest subnormal binary16 value, as F32 bits.
const SMALLEST_SUBNORMAL_F32: u32 = 0x3380_0000;

impl F16 {
    /// Positive zero.
    pub const ZERO: Self = Self(0);

    /// The value with these bits.
    #[must_use]
    pub const fn from_bits(bits: u16) -> Self {
        Self(bits)
    }

    /// The value's bits.
    #[must_use]
    pub const fn to_bits(self) -> u16 {
        self.0
    }

    /// Whether the value is NaN.
    #[must_use]
    pub const fn is_nan(self) -> bool {
        self.0 & EXPONENT_MASK == EXPONENT_MASK && self.0 & FRACTION_MASK != 0
    }

    /// The value as F32, exactly. A NaN keeps its sign and payload.
    #[must_use]
    pub fn to_f32(self) -> f32 {
        let sign = u32::from(self.0 & SIGN) << 16;
        let exponent = (self.0 & EXPONENT_MASK) >> 10;
        let fraction = self.0 & FRACTION_MASK;
        let magnitude = match exponent {
            // Zero or subnormal: fraction × 2^-24, exact in F32 (a 10-bit
            // integer scaled by a power of two well inside F32's normal
            // range).
            0 => (f32::from(fraction) * f32::from_bits(SMALLEST_SUBNORMAL_F32)).to_bits(),
            // Infinity, or NaN with the payload moved to F32's high
            // fraction bits.
            0x1F => 0x7F80_0000 | (u32::from(fraction) << 13),
            // Normal: rebias the exponent from 15 to 127 and widen the
            // fraction from 10 to 23 bits.
            _ => ((u32::from(exponent) + 112) << 23) | (u32::from(fraction) << 13),
        };
        f32::from_bits(sign | magnitude)
    }

    /// The value as F64, exactly.
    #[must_use]
    pub fn to_f64(self) -> f64 {
        f64::from(self.to_f32())
    }

    /// `x` rounded once to the nearest binary16 value, ties to even
    /// (the declared rounding; see the [module documentation](self)).
    #[must_use]
    pub const fn from_f64(x: f64) -> Self {
        let bits = x.to_bits();
        let sign = ((bits >> 48) & 0x8000) as u16;
        let biased = ((bits >> 52) & 0x7FF) as i64;
        let fraction = bits & 0x000F_FFFF_FFFF_FFFF;
        if biased == 0x7FF {
            return Self(
                sign | if fraction == 0 {
                    EXPONENT_MASK
                } else {
                    QUIET_NAN
                },
            );
        }
        if biased == 0 {
            // Zero or an F64 subnormal: below 2^-1022, far under half the
            // smallest binary16 subnormal, so it rounds to zero.
            return Self(sign);
        }
        let exponent = biased - 1023;
        if exponent >= 16 {
            return Self(sign | EXPONENT_MASK);
        }
        // The 53-bit significand: |x| = significand × 2^(exponent − 52).
        let significand = fraction | (1 << 52);
        if exponent >= -14 {
            // A normal binary16 candidate: keep 11 significant bits.
            let kept = round_shift(significand, 42);
            // A carry out of the significand moves to the next binade.
            let (kept, exponent) = if kept == 1 << 11 {
                (1 << 10, exponent + 1)
            } else {
                (kept, exponent)
            };
            if exponent >= 16 {
                return Self(sign | EXPONENT_MASK);
            }
            let field = ((exponent + 15) as u16) << 10;
            return Self(sign | field | ((kept as u16) & FRACTION_MASK));
        }
        // Subnormal: |x| = units × 2^-24 with units below 1,024, rounded;
        // a carry to 1,024 is the smallest normal value, which these bits
        // also encode.
        let shift = 28 - exponent;
        if shift >= 54 {
            // significand < 2^53 ≤ half a unit: rounds to zero.
            return Self(sign);
        }
        Self(sign | round_shift(significand, shift as u32) as u16)
    }
}

/// `value >> shift` rounded to nearest, ties to even (`1 ≤ shift ≤ 53`).
const fn round_shift(value: u64, shift: u32) -> u64 {
    let kept = value >> shift;
    let rest = value & ((1 << shift) - 1);
    let half = 1 << (shift - 1);
    if rest > half || (rest == half && kept & 1 == 1) {
        kept + 1
    } else {
        kept
    }
}
