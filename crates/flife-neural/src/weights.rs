//! Canonical weight dtypes (plan §10.3): the reference F32 profile, and the
//! compact-F16 profile whose heritable values are binary16.
//!
//! The reference evaluator reads every weight as F32. An F32 weight is read
//! as itself; an F16 weight is widened, which is exact
//! ([`F16::to_f32`]). The arithmetic after that is the F32 profile's,
//! unchanged, so an F16 controller's outputs are those of the F32
//! controller whose weights are the widened values, bit for bit.
//!
//! A value computed wider (initialization and mutation work in F64)
//! becomes canonical by one rounding, [`Weight::round_from_f64`]: to
//! nearest, ties to even, in the weight's own dtype.

use flife_core::CoreError;
use flife_core::binary16::F16;

/// The registry name of the compact-F16 weight dtype.
pub const F16_NAME: &str = "f16";

/// A canonical weight dtype.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WeightDtype {
    /// IEEE-754 binary32, the reference profile.
    F32,
    /// IEEE-754 binary16, the compact-F16 profile.
    F16,
}

impl WeightDtype {
    /// The dtype a configuration names in `controller.canonical_weight_dtype`.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for any other name.
    pub fn named(name: &str) -> Result<Self, CoreError> {
        match name {
            crate::controller::F32 => Ok(Self::F32),
            F16_NAME => Ok(Self::F16),
            other => Err(CoreError::UnknownAlgorithm {
                role: "controller.canonical_weight_dtype",
                name: other.to_owned(),
            }),
        }
    }

    /// The registry name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::F32 => crate::controller::F32,
            Self::F16 => F16_NAME,
        }
    }

    /// Bytes per parameter.
    #[must_use]
    pub const fn bytes(self) -> u64 {
        match self {
            Self::F32 => 4,
            Self::F16 => 2,
        }
    }
}

/// A canonical weight: F32, or binary16 widened exactly.
pub trait Weight: Copy + Send + Sync + std::fmt::Debug + 'static {
    /// The dtype.
    const DTYPE: WeightDtype;

    /// Stack-resident tile scratch reused across all layers and strips of one
    /// controller evaluation (`()` for F32; an L1D-resident `[f32; 1024]` strip
    /// buffer for F16 so widening happens in a single contiguous vector pass).
    type TileScratch;

    /// Allocate zero-initialized tile scratch once per controller evaluation.
    fn new_tile_scratch() -> Self::TileScratch;

    /// The weight as the F32 the reference arithmetic reads. Exact.
    fn widen(self) -> f32;

    /// Four weights widened to F32 in lockstep. Exact.
    #[inline(always)]
    fn widen_4(values: [Self; 4]) -> [f32; 4] {
        [
            values[0].widen(),
            values[1].widen(),
            values[2].widen(),
            values[3].widen(),
        ]
    }

    /// Accumulate 8 output rows of a dense affine layer into `out_block`
    /// starting from `biases`, summing columns `0..input.len()` in strict
    /// ascending order in F32 (separate multiply then add, no FMA).
    ///
    /// `weights` must hold `8 * input.len()` row-major weights.
    #[inline]
    fn affine_block8(
        weights: &[Self],
        biases: &[Self; 8],
        input: &[f32],
        out_block: &mut [f32; 8],
        _scratch: &mut Self::TileScratch,
    ) {
        let n = input.len();
        let (in_q, in_r) = input.as_chunks::<4>();
        let mut z0 = [
            biases[0].widen(),
            biases[1].widen(),
            biases[2].widen(),
            biases[3].widen(),
        ];
        let mut z1 = [
            biases[4].widen(),
            biases[5].widen(),
            biases[6].widen(),
            biases[7].widen(),
        ];
        let (r0_q, r0_r) = weights[0..n].as_chunks::<4>();
        let (r1_q, r1_r) = weights[n..2 * n].as_chunks::<4>();
        let (r2_q, r2_r) = weights[2 * n..3 * n].as_chunks::<4>();
        let (r3_q, r3_r) = weights[3 * n..4 * n].as_chunks::<4>();
        let (r4_q, r4_r) = weights[4 * n..5 * n].as_chunks::<4>();
        let (r5_q, r5_r) = weights[5 * n..6 * n].as_chunks::<4>();
        let (r6_q, r6_r) = weights[6 * n..7 * n].as_chunks::<4>();
        let (r7_q, r7_r) = weights[7 * n..8 * n].as_chunks::<4>();

        for ((((((((&x, w0), w1), w2), w3), w4), w5), w6), w7) in in_q
            .iter()
            .zip(r0_q)
            .zip(r1_q)
            .zip(r2_q)
            .zip(r3_q)
            .zip(r4_q)
            .zip(r5_q)
            .zip(r6_q)
            .zip(r7_q)
        {
            let a0 = Self::widen_4(*w0);
            let a1 = Self::widen_4(*w1);
            let a2 = Self::widen_4(*w2);
            let a3 = Self::widen_4(*w3);
            let p0 = [a0[0] * x[0], a0[1] * x[1], a0[2] * x[2], a0[3] * x[3]];
            let p1 = [a1[0] * x[0], a1[1] * x[1], a1[2] * x[2], a1[3] * x[3]];
            let p2 = [a2[0] * x[0], a2[1] * x[1], a2[2] * x[2], a2[3] * x[3]];
            let p3 = [a3[0] * x[0], a3[1] * x[1], a3[2] * x[2], a3[3] * x[3]];
            for k in 0..4 {
                z0 = [z0[0] + p0[k], z0[1] + p1[k], z0[2] + p2[k], z0[3] + p3[k]];
            }
            let a4 = Self::widen_4(*w4);
            let a5 = Self::widen_4(*w5);
            let a6 = Self::widen_4(*w6);
            let a7 = Self::widen_4(*w7);
            let p4 = [a4[0] * x[0], a4[1] * x[1], a4[2] * x[2], a4[3] * x[3]];
            let p5 = [a5[0] * x[0], a5[1] * x[1], a5[2] * x[2], a5[3] * x[3]];
            let p6 = [a6[0] * x[0], a6[1] * x[1], a6[2] * x[2], a6[3] * x[3]];
            let p7 = [a7[0] * x[0], a7[1] * x[1], a7[2] * x[2], a7[3] * x[3]];
            for k in 0..4 {
                z1 = [z1[0] + p4[k], z1[1] + p5[k], z1[2] + p6[k], z1[3] + p7[k]];
            }
        }
        for ((((((((&x, &w0), &w1), &w2), &w3), &w4), &w5), &w6), &w7) in in_r
            .iter()
            .zip(r0_r)
            .zip(r1_r)
            .zip(r2_r)
            .zip(r3_r)
            .zip(r4_r)
            .zip(r5_r)
            .zip(r6_r)
            .zip(r7_r)
        {
            z0 = [
                z0[0] + w0.widen() * x,
                z0[1] + w1.widen() * x,
                z0[2] + w2.widen() * x,
                z0[3] + w3.widen() * x,
            ];
            z1 = [
                z1[0] + w4.widen() * x,
                z1[1] + w5.widen() * x,
                z1[2] + w6.widen() * x,
                z1[3] + w7.widen() * x,
            ];
        }
        out_block[0..4].copy_from_slice(&z0);
        out_block[4..8].copy_from_slice(&z1);
    }

    /// Accumulate 4 output rows of a dense affine layer into `out_block`
    /// starting from `biases`, summing columns `0..input.len()` in strict
    /// ascending order in F32 (separate multiply then add, no FMA).
    ///
    /// `weights` must hold `4 * input.len()` row-major weights.
    #[inline]
    fn affine_block4(
        weights: &[Self],
        biases: &[Self; 4],
        input: &[f32],
        out_block: &mut [f32; 4],
        _scratch: &mut Self::TileScratch,
    ) {
        let n = input.len();
        let (in_q, in_r) = input.as_chunks::<4>();
        let mut z0 = [
            biases[0].widen(),
            biases[1].widen(),
            biases[2].widen(),
            biases[3].widen(),
        ];
        let (r0_q, r0_r) = weights[0..n].as_chunks::<4>();
        let (r1_q, r1_r) = weights[n..2 * n].as_chunks::<4>();
        let (r2_q, r2_r) = weights[2 * n..3 * n].as_chunks::<4>();
        let (r3_q, r3_r) = weights[3 * n..4 * n].as_chunks::<4>();
        for ((((&x, w0), w1), w2), w3) in in_q.iter().zip(r0_q).zip(r1_q).zip(r2_q).zip(r3_q) {
            let a0 = Self::widen_4(*w0);
            let a1 = Self::widen_4(*w1);
            let a2 = Self::widen_4(*w2);
            let a3 = Self::widen_4(*w3);
            let p0 = [a0[0] * x[0], a0[1] * x[1], a0[2] * x[2], a0[3] * x[3]];
            let p1 = [a1[0] * x[0], a1[1] * x[1], a1[2] * x[2], a1[3] * x[3]];
            let p2 = [a2[0] * x[0], a2[1] * x[1], a2[2] * x[2], a2[3] * x[3]];
            let p3 = [a3[0] * x[0], a3[1] * x[1], a3[2] * x[2], a3[3] * x[3]];
            for k in 0..4 {
                z0 = [z0[0] + p0[k], z0[1] + p1[k], z0[2] + p2[k], z0[3] + p3[k]];
            }
        }
        for ((((&x, &w0), &w1), &w2), &w3) in in_r.iter().zip(r0_r).zip(r1_r).zip(r2_r).zip(r3_r) {
            z0 = [
                z0[0] + w0.widen() * x,
                z0[1] + w1.widen() * x,
                z0[2] + w2.widen() * x,
                z0[3] + w3.widen() * x,
            ];
        }
        *out_block = z0;
    }

    /// `value` rounded once to this dtype, to nearest with ties to even.
    fn round_from_f64(value: f64) -> Self;

    /// The weight's bits, widened to 32 (equality of bits, not of values:
    /// `-0` and `+0` differ).
    fn bits(self) -> u32;
}

/// Maximum input fan-in widened in one L1D-resident stack tile for `F16`
/// 8-row / 4-row affine blocks (`8 * 128 * 4 = 4 KiB`, covering all standard
/// `104`, `64`, `32`, `128` catalog layers in a single pass; wider layers tile
/// across columns in exact ascending order).
const F16_TILE_COLS: usize = 128;

impl Weight for f32 {
    const DTYPE: WeightDtype = WeightDtype::F32;
    type TileScratch = ();

    #[inline(always)]
    fn new_tile_scratch() -> Self::TileScratch {}

    #[inline(always)]
    fn widen(self) -> f32 {
        self
    }

    #[inline(always)]
    fn widen_4(values: [Self; 4]) -> [f32; 4] {
        values
    }

    #[inline]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the declared rounding: F64 to F32, to nearest, ties to even"
    )]
    fn round_from_f64(value: f64) -> Self {
        value as f32
    }

    #[inline]
    fn bits(self) -> u32 {
        self.to_bits()
    }
}

impl Weight for F16 {
    const DTYPE: WeightDtype = WeightDtype::F16;
    type TileScratch = [f32; 8 * F16_TILE_COLS];

    #[inline]
    fn new_tile_scratch() -> Self::TileScratch {
        [0.0_f32; 8 * F16_TILE_COLS]
    }

    #[inline(always)]
    fn widen(self) -> f32 {
        self.to_f32()
    }

    #[inline(always)]
    fn widen_4(values: [Self; 4]) -> [f32; 4] {
        Self::to_f32_4(values)
    }

    #[inline]
    fn affine_block8(
        weights: &[Self],
        biases: &[Self; 8],
        input: &[f32],
        out_block: &mut [f32; 8],
        buf: &mut Self::TileScratch,
    ) {
        let n = input.len();
        let b0 = Self::to_f32_4([biases[0], biases[1], biases[2], biases[3]]);
        let b1 = Self::to_f32_4([biases[4], biases[5], biases[6], biases[7]]);
        let mut acc = [b0[0], b0[1], b0[2], b0[3], b1[0], b1[1], b1[2], b1[3]];
        if n <= F16_TILE_COLS {
            // Single contiguous linear widen of all 8*n weights into 4 KiB
            // L1D stack scratch, followed by pure L1D-resident F32 GEMV.
            Self::widen_slice_into(&weights[..8 * n], &mut buf[..8 * n]);
            f32::affine_block8(&buf[..8 * n], &acc, input, out_block, &mut ());
        } else {
            let mut col = 0_usize;
            while col < n {
                let len = (n - col).min(F16_TILE_COLS);
                for row in 0..8 {
                    let src = &weights[row * n + col..row * n + col + len];
                    let dst = &mut buf[row * len..(row + 1) * len];
                    Self::widen_slice_into(src, dst);
                }
                let next_in = &input[col..col + len];
                let prev = acc;
                f32::affine_block8(&buf[..8 * len], &prev, next_in, &mut acc, &mut ());
                col += len;
            }
            *out_block = acc;
        }
    }

    #[inline]
    fn affine_block4(
        weights: &[Self],
        biases: &[Self; 4],
        input: &[f32],
        out_block: &mut [f32; 4],
        buf: &mut Self::TileScratch,
    ) {
        let n = input.len();
        let mut acc = Self::to_f32_4(*biases);
        if n <= F16_TILE_COLS {
            Self::widen_slice_into(&weights[..4 * n], &mut buf[..4 * n]);
            f32::affine_block4(&buf[..4 * n], &acc, input, out_block, &mut ());
        } else {
            let mut col = 0_usize;
            while col < n {
                let len = (n - col).min(F16_TILE_COLS);
                for row in 0..4 {
                    let src = &weights[row * n + col..row * n + col + len];
                    let dst = &mut buf[row * len..(row + 1) * len];
                    Self::widen_slice_into(src, dst);
                }
                let next_in = &input[col..col + len];
                let prev = acc;
                f32::affine_block4(&buf[..4 * len], &prev, next_in, &mut acc, &mut ());
                col += len;
            }
            *out_block = acc;
        }
    }

    #[inline]
    fn round_from_f64(value: f64) -> Self {
        Self::from_f64(value)
    }

    #[inline]
    fn bits(self) -> u32 {
        u32::from(self.to_bits())
    }
}

/// A controller's parameters in their canonical dtype, borrowed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Weights<'a> {
    /// The reference profile's F32 values.
    F32(&'a [f32]),
    /// The compact-F16 profile's binary16 values.
    F16(&'a [F16]),
}

impl<'a> Weights<'a> {
    /// The dtype.
    #[must_use]
    pub const fn dtype(&self) -> WeightDtype {
        match self {
            Self::F32(_) => WeightDtype::F32,
            Self::F16(_) => WeightDtype::F16,
        }
    }

    /// The number of parameters.
    #[must_use]
    pub const fn len(&self) -> usize {
        match self {
            Self::F32(values) => values.len(),
            Self::F16(values) => values.len(),
        }
    }

    /// Whether there are none.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Parameter `index` as the F32 the arithmetic reads.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<f32> {
        match self {
            Self::F32(values) => values.get(index).copied(),
            Self::F16(values) => values.get(index).map(|value| value.to_f32()),
        }
    }

    /// Every parameter as the F32 the arithmetic reads, in layout order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = f32> + 'a {
        let (f32s, f16s): (&'a [f32], &'a [F16]) = match *self {
            Self::F32(values) => (values, &[]),
            Self::F16(values) => (&[], values),
        };
        let widened = f16s.iter().map(|value| value.to_f32());
        // One of the two is empty; chaining keeps a single iterator type.
        Chained {
            first: f32s.iter().copied(),
            second: widened,
        }
    }

    /// Every parameter's bits, widened to 32, in layout order.
    pub fn bits(&self) -> impl ExactSizeIterator<Item = u32> + 'a {
        let (f32s, f16s): (&'a [f32], &'a [F16]) = match *self {
            Self::F32(values) => (values, &[]),
            Self::F16(values) => (&[], values),
        };
        Chained {
            first: f32s.iter().map(|value| value.to_bits()),
            second: f16s.iter().map(|value| u32::from(value.to_bits())),
        }
    }

    /// The F32 values, or `None` for another dtype.
    #[must_use]
    pub const fn as_f32(&self) -> Option<&'a [f32]> {
        match *self {
            Self::F32(values) => Some(values),
            Self::F16(_) => None,
        }
    }

    /// The binary16 values, or `None` for another dtype.
    #[must_use]
    pub const fn as_f16(&self) -> Option<&'a [F16]> {
        match *self {
            Self::F16(values) => Some(values),
            Self::F32(_) => None,
        }
    }

    /// Whether `self` and `other` hold the same dtype and the same bits.
    #[must_use]
    pub fn same_bits(&self, other: &Weights<'_>) -> bool {
        self.dtype() == other.dtype() && self.len() == other.len() && self.bits().eq(other.bits())
    }
}

impl<'a> From<&'a [f32]> for Weights<'a> {
    fn from(values: &'a [f32]) -> Self {
        Self::F32(values)
    }
}

impl<'a> From<&'a Vec<f32>> for Weights<'a> {
    fn from(values: &'a Vec<f32>) -> Self {
        Self::F32(values)
    }
}

impl<'a, const N: usize> From<&'a [f32; N]> for Weights<'a> {
    fn from(values: &'a [f32; N]) -> Self {
        Self::F32(values)
    }
}

impl<'a> From<&'a [F16]> for Weights<'a> {
    fn from(values: &'a [F16]) -> Self {
        Self::F16(values)
    }
}

impl<'a> From<&'a Vec<F16>> for Weights<'a> {
    fn from(values: &'a Vec<F16>) -> Self {
        Self::F16(values)
    }
}

/// Two exact-size iterators, one after the other.
struct Chained<A, B> {
    first: A,
    second: B,
}

impl<T, A: Iterator<Item = T>, B: Iterator<Item = T>> Iterator for Chained<A, B> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.first.next().or_else(|| self.second.next())
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let (a, b) = (self.first.size_hint(), self.second.size_hint());
        let low = a.0.saturating_add(b.0);
        let high = a.1.zip(b.1).and_then(|(a, b)| a.checked_add(b));
        (low, high)
    }
}

impl<T, A: ExactSizeIterator<Item = T>, B: ExactSizeIterator<Item = T>> ExactSizeIterator
    for Chained<A, B>
{
}
