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

/// Every exponent bit set: infinity or NaN.
const EXPONENT_MASK: u16 = 0x7C00;
/// The fraction bits.
const FRACTION_MASK: u16 = 0x03FF;
/// The canonical quiet NaN's magnitude bits.
const QUIET_NAN: u16 = 0x7E00;
/// `2^-14` in F32 bits (`biased_exp = 113 = 127 - 14`), used for exact
/// branchless subnormal widening via Sterbenz subtraction:
/// `f32::from_bits(0x3880_0000 | (frac << 13)) - 2^-14 == frac * 2^-24`.
const SUBNORMAL_BIAS_F32: u32 = 0x3880_0000;
/// Exponent rebias delta `(127 - 15) << 23 = 112 << 23` for normal F16 -> F32.
const NORMAL_BIAS_DELTA_F32: u32 = 0x3800_0000;

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

    /// The value as F32, exactly, via branchless IEEE-754 bit arithmetic across
    /// all 65,536 bit patterns (normals, zero, subnormals, infinities, and NaN
    /// payloads with sign preserved).
    #[inline(always)]
    #[must_use]
    pub fn to_f32(self) -> f32 {
        let b = u32::from(self.0);
        let sign = (b & 0x8000) << 16;
        let mag = b & 0x7FFF;
        let shifted = mag << 13;
        let extra_inf_bias = if mag >= 0x7C00 {
            NORMAL_BIAS_DELTA_F32
        } else {
            0
        };
        let norm_or_inf = shifted
            .wrapping_add(NORMAL_BIAS_DELTA_F32)
            .wrapping_add(extra_inf_bias);
        // Zero and subnormals (`mag < 0x0400`): `(2^-14 + frac * 2^-24) - 2^-14`
        // is exact in F32 for every 10-bit fraction (`0..=1023`), returning +0.0
        // for `mag == 0` so `sign | sub` preserves `-0.0` bit-for-bit.
        let sub = (f32::from_bits(shifted | SUBNORMAL_BIAS_F32)
            - f32::from_bits(SUBNORMAL_BIAS_F32))
        .to_bits();
        let magnitude = if mag < 0x0400 { sub } else { norm_or_inf };
        f32::from_bits(sign | magnitude)
    }

    /// Four-lane lockstep [`F16::to_f32`], bit-identical per lane across all
    /// 65,536 binary16 patterns and auto-vectorizing to branchless SIMD.
    #[inline(always)]
    #[must_use]
    pub fn to_f32_4(xs: [Self; 4]) -> [f32; 4] {
        [
            xs[0].to_f32(),
            xs[1].to_f32(),
            xs[2].to_f32(),
            xs[3].to_f32(),
        ]
    }

    /// Whether every element of a 4×4 binary16 tile is normal finite
    /// (`1 <= exponent <= 30`, i.e. `0x0400 <= (bits & 0x7FFF) <= 0x7BFF`),
    /// so widening is an exact exponent-rebias shift with no zero, subnormal,
    /// infinity, or NaN special cases.
    #[inline(always)]
    #[must_use]
    pub fn all_normal_4x4(rows: [[Self; 4]; 4]) -> bool {
        let mut worst = [0_u16; 4];
        for row in rows {
            for (slot, h) in worst.iter_mut().zip(row) {
                let u = (h.0 & 0x7FFF).wrapping_sub(0x0400);
                if u > *slot {
                    *slot = u;
                }
            }
        }
        worst[0].max(worst[1]).max(worst[2]).max(worst[3]) < 0x7800
    }

    /// Exact F32 widening for four binary16 values already proven normal finite
    /// (`0x0400 <= (bits & 0x7FFF) <= 0x7BFF`). Private so callers cannot violate
    /// the precondition.
    #[inline(always)]
    fn to_f32_4_assuming_normal(xs: [Self; 4]) -> [f32; 4] {
        let mut out = [0.0_f32; 4];
        for (dst, h) in out.iter_mut().zip(xs) {
            let b = u32::from(h.0);
            let sign = (b & 0x8000) << 16;
            let mag = b & 0x7FFF;
            *dst = f32::from_bits(sign | ((mag << 13).wrapping_add(NORMAL_BIAS_DELTA_F32)));
        }
        out
    }

    /// Cold exact fallback for [`F16::to_f32_4x4`] when at least one lane in
    /// the 4×4 tile is zero, subnormal, infinity, or NaN.
    #[cold]
    #[inline(never)]
    fn to_f32_4x4_slow(rows: [[Self; 4]; 4]) -> [[f32; 4]; 4] {
        [
            Self::to_f32_4(rows[0]),
            Self::to_f32_4(rows[1]),
            Self::to_f32_4(rows[2]),
            Self::to_f32_4(rows[3]),
        ]
    }

    /// Widen a 4×4 tile of binary16 values to F32 if and only if every element
    /// is normal finite (`1 <= exponent <= 30`). Returns `None` if any lane is
    /// zero, subnormal, infinity, or NaN so callers can dispatch to an
    /// out-of-line exact fallback without repeating the tile scan.
    #[inline(always)]
    #[must_use]
    pub fn to_f32_4x4_if_normal(rows: [[Self; 4]; 4]) -> Option<[[f32; 4]; 4]> {
        if Self::all_normal_4x4(rows) {
            Some([
                Self::to_f32_4_assuming_normal(rows[0]),
                Self::to_f32_4_assuming_normal(rows[1]),
                Self::to_f32_4_assuming_normal(rows[2]),
                Self::to_f32_4_assuming_normal(rows[3]),
            ])
        } else {
            None
        }
    }

    /// Widen a 4×4 tile of binary16 values to F32, bit-identical per element
    /// to [`F16::to_f32`] across all 65,536 binary16 bit patterns.
    ///
    /// When all 16 values are normal finite binary16 values, widens via pure
    /// exponent-rebias arithmetic with no Sterbenz subnormal FADD or Inf/NaN
    /// masks; otherwise delegates to [`F16::to_f32_4`].
    #[inline(always)]
    #[must_use]
    pub fn to_f32_4x4(rows: [[Self; 4]; 4]) -> [[f32; 4]; 4] {
        Self::to_f32_4x4_if_normal(rows).unwrap_or_else(|| Self::to_f32_4x4_slow(rows))
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
