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

    /// Exact 3-op F32 widening for a single binary16 value already proven
    /// normal finite (`0x0400 <= (bits & 0x7FFF) <= 0x7BFF`).
    ///
    /// Sign-extending `bits` to `i32` and shifting left by 13 yields
    /// `0x0000_0000 | (mag << 13)` for positive normals and
    /// `0xF000_0000 | (mag << 13)` for negative normals. Masking with
    /// `0x8FFF_FFFF` (ARM64 `bic #0x70, lsl #24`) leaves `(sign << 31) |
    /// (mag << 13)`, and adding `0x3800_0000` never carries into bit 31
    /// because `0x0080_0000 <= (mag << 13) <= 0x0F7F_E000`.
    #[inline(always)]
    fn to_f32_assuming_normal(h: Self) -> f32 {
        let shifted = (i32::from(h.0.cast_signed()) << 13).cast_unsigned();
        f32::from_bits((shifted & 0x8FFF_FFFF).wrapping_add(NORMAL_BIAS_DELTA_F32))
    }

    /// Cold exact fallback for a 32-element chunk containing at least one
    /// zero, subnormal, infinity, or NaN value.
    #[cold]
    #[inline(never)]
    fn widen_32_slow(src: &[Self; 32], dst: &mut [f32; 32]) {
        let (s4, _) = src.as_chunks::<4>();
        let (d4, _) = dst.as_chunks_mut::<4>();
        for (s, d) in s4.iter().zip(d4) {
            *d = Self::to_f32_4(*s);
        }
    }

    /// Widen `src` into `dst` (up to `src.len().min(dst.len())` elements),
    /// bit-identical per element to [`F16::to_f32`] across all 65,536 binary16
    /// patterns.
    ///
    /// Processes 32 contiguous elements at a time with an 8-lane `u16` normal
    /// check: when all 32 lanes are normal finite binary16 values, widens via
    /// pure contiguous 3-op exponent rebias (`sshll #13` + `bic #0x70<<24` +
    /// `add #0x3800_0000`), falling back to [`F16::to_f32_4`] otherwise.
    /// Kept out-of-line so downstream GEMV accumulators cannot deform the
    /// contiguous vector widening into strided GPR extracts.
    #[inline(never)]
    pub fn widen_slice_into(src: &[Self], dst: &mut [f32]) {
        let len = src.len().min(dst.len());
        let (src_chunks, src_rem) = src[..len].as_chunks::<32>();
        let (dst_chunks, dst_rem) = dst[..len].as_chunks_mut::<32>();
        for (s, d) in src_chunks.iter().zip(dst_chunks) {
            let mut worst = [0_u16; 8];
            for group in s.as_chunks::<8>().0 {
                for (slot, &h) in worst.iter_mut().zip(group) {
                    let u = (h.0 & 0x7FFF).wrapping_sub(0x0400);
                    *slot = (*slot).max(u);
                }
            }
            let max_u = worst[0]
                .max(worst[1])
                .max(worst[2].max(worst[3]))
                .max(worst[4].max(worst[5]).max(worst[6].max(worst[7])));
            if max_u < 0x7800 {
                for (dst_slot, &h) in d.iter_mut().zip(s) {
                    *dst_slot = Self::to_f32_assuming_normal(h);
                }
            } else {
                Self::widen_32_slow(s, d);
            }
        }
        let (s4, sr) = src_rem.as_chunks::<4>();
        let (d4, dr) = dst_rem.as_chunks_mut::<4>();
        for (s, d) in s4.iter().zip(d4) {
            *d = Self::to_f32_4(*s);
        }
        for (dst_slot, &h) in dr.iter_mut().zip(sr) {
            *dst_slot = h.to_f32();
        }
    }

    /// The value as F64, exactly.
    #[must_use]
    pub fn to_f64(self) -> f64 {
        f64::from(self.to_f32())
    }

    /// `x` rounded once to the nearest binary16 value, ties to even
    /// (the declared rounding; see the [module documentation](self)).
    #[inline]
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
#[inline(always)]
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
