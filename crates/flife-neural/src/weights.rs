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
        scratch: &mut Self::TileScratch,
    ) {
        let n = input.len();
        let b0 = [biases[0], biases[1], biases[2], biases[3]];
        let b1 = [biases[4], biases[5], biases[6], biases[7]];
        let mut o0 = [0.0_f32; 4];
        let mut o1 = [0.0_f32; 4];
        Self::affine_block4(&weights[0..4 * n], &b0, input, &mut o0, scratch);
        Self::affine_block4(&weights[4 * n..8 * n], &b1, input, &mut o1, scratch);
        out_block[0..4].copy_from_slice(&o0);
        out_block[4..8].copy_from_slice(&o1);
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
            z0 = accumulate_4x4(
                z0,
                Self::widen_4(*w0),
                Self::widen_4(*w1),
                Self::widen_4(*w2),
                Self::widen_4(*w3),
                x,
            );
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

/// Stack-resident 4 KiB L1D tile buffer reused across repeated `F16`
/// [`crate::ReferenceController`] evaluations in one worker loop so each
/// organism does not zero 4 KiB on entry.
pub type F16TileScratch = [f32; 8 * F16_TILE_COLS];

/// Create a zero-initialized [`F16TileScratch`] once per worker tile.
#[inline]
#[must_use]
pub const fn new_f16_tile_scratch() -> F16TileScratch {
    [0.0_f32; 8 * F16_TILE_COLS]
}

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

/// Accumulate 4 rows × 4 columns in strict ascending column order (`0..4`)
/// using an explicit in-register 4×4 transpose (`trn1`/`trn2` on AArch64 NEON,
/// `unpcklps`/`unpckhps` + `movlhps`/`movhlps` on x86_64) to avoid scalar lane
/// insert chains while preserving exact separate-mul-then-add F32 semantics.
#[inline(always)]
fn accumulate_4x4(
    mut z: [f32; 4],
    w0: [f32; 4],
    w1: [f32; 4],
    w2: [f32; 4],
    w3: [f32; 4],
    x: [f32; 4],
) -> [f32; 4] {
    let p0 = [w0[0] * x[0], w0[1] * x[1], w0[2] * x[2], w0[3] * x[3]];
    let p1 = [w1[0] * x[0], w1[1] * x[1], w1[2] * x[2], w1[3] * x[3]];
    let p2 = [w2[0] * x[0], w2[1] * x[1], w2[2] * x[2], w2[3] * x[3]];
    let p3 = [w3[0] * x[0], w3[1] * x[1], w3[2] * x[2], w3[3] * x[3]];
    let t0 = [p0[0], p1[0], p0[2], p1[2]];
    let t1 = [p0[1], p1[1], p0[3], p1[3]];
    let t2 = [p2[0], p3[0], p2[2], p3[2]];
    let t3 = [p2[1], p3[1], p2[3], p3[3]];
    let c0 = [t0[0], t0[1], t2[0], t2[1]];
    let c1 = [t1[0], t1[1], t3[0], t3[1]];
    let c2 = [t0[2], t0[3], t2[2], t2[3]];
    let c3 = [t1[2], t1[3], t3[2], t3[3]];
    z = [z[0] + c0[0], z[1] + c0[1], z[2] + c0[2], z[3] + c0[3]];
    z = [z[0] + c1[0], z[1] + c1[1], z[2] + c1[2], z[3] + c1[3]];
    z = [z[0] + c2[0], z[1] + c2[1], z[2] + c2[2], z[3] + c2[3]];
    z = [z[0] + c3[0], z[1] + c3[1], z[2] + c3[2], z[3] + c3[3]];
    z
}

/// Accumulate 8 rows from two column-interleaved L1D strips
/// (`cols_lo[col] = [w0..w3]`, `cols_hi[col] = [w4..w7]`) in strict ascending
/// column order (separate multiply then add, no FMA). Pure sequential NEON
/// loads (`ldp q`) + indexed `fmul.4s` + interleaved `fadd.4s` with zero
/// lane shuffles and zero strided row pointers.
#[inline(never)]
fn affine_block8_interleaved(
    cols_lo: &[[f32; 4]],
    cols_hi: &[[f32; 4]],
    z0_inout: &mut [f32; 4],
    z1_inout: &mut [f32; 4],
    input: &[f32],
) {
    let mut z0 = *z0_inout;
    let mut z1 = *z1_inout;
    let (in_q, in_r) = input.as_chunks::<4>();
    let (lo_q, lo_r) = cols_lo.as_chunks::<4>();
    let (hi_q, hi_r) = cols_hi.as_chunks::<4>();
    for ((&x, a), b) in in_q.iter().zip(lo_q).zip(hi_q) {
        for k in 0..4 {
            let xk = x[k];
            let ca = a[k];
            let cb = b[k];
            let pa = [ca[0] * xk, ca[1] * xk, ca[2] * xk, ca[3] * xk];
            let pb = [cb[0] * xk, cb[1] * xk, cb[2] * xk, cb[3] * xk];
            z0 = [z0[0] + pa[0], z0[1] + pa[1], z0[2] + pa[2], z0[3] + pa[3]];
            z1 = [z1[0] + pb[0], z1[1] + pb[1], z1[2] + pb[2], z1[3] + pb[3]];
        }
    }
    for ((&xk, &ca), &cb) in in_r.iter().zip(lo_r).zip(hi_r) {
        z0 = [
            z0[0] + ca[0] * xk,
            z0[1] + ca[1] * xk,
            z0[2] + ca[2] * xk,
            z0[3] + ca[3] * xk,
        ];
        z1 = [
            z1[0] + cb[0] * xk,
            z1[1] + cb[1] * xk,
            z1[2] + cb[2] * xk,
            z1[3] + cb[3] * xk,
        ];
    }
    *z0_inout = z0;
    *z1_inout = z1;
}

#[inline(never)]
fn affine_block4_interleaved(cols: &[[f32; 4]], z_inout: &mut [f32; 4], input: &[f32]) {
    let mut z = *z_inout;
    let (in_q, in_r) = input.as_chunks::<4>();
    let (c_q, c_r) = cols.as_chunks::<4>();
    for (&x, a) in in_q.iter().zip(c_q) {
        for k in 0..4 {
            let xk = x[k];
            let ca = a[k];
            let pa = [ca[0] * xk, ca[1] * xk, ca[2] * xk, ca[3] * xk];
            z = [z[0] + pa[0], z[1] + pa[1], z[2] + pa[2], z[3] + pa[3]];
        }
    }
    for (&xk, &ca) in in_r.iter().zip(c_r) {
        z = [
            z[0] + ca[0] * xk,
            z[1] + ca[1] * xk,
            z[2] + ca[2] * xk,
            z[3] + ca[3] * xk,
        ];
    }
    *z_inout = z;
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
        let mut z0 = Self::to_f32_4([biases[0], biases[1], biases[2], biases[3]]);
        let mut z1 = Self::to_f32_4([biases[4], biases[5], biases[6], biases[7]]);
        let (cols_buf, _) = buf.as_chunks_mut::<4>();
        let (lo_buf, hi_buf) = cols_buf.split_at_mut(F16_TILE_COLS);
        if n <= F16_TILE_COLS {
            Self::widen_4rows_contiguous_interleaved_into(&weights[0..4 * n], n, &mut lo_buf[..n]);
            Self::widen_4rows_contiguous_interleaved_into(
                &weights[4 * n..8 * n],
                n,
                &mut hi_buf[..n],
            );
            affine_block8_interleaved(&lo_buf[..n], &hi_buf[..n], &mut z0, &mut z1, input);
        } else {
            let mut col = 0_usize;
            while col < n {
                let len = (n - col).min(F16_TILE_COLS);
                Self::widen_4rows_interleaved_into(
                    &weights[col..col + len],
                    &weights[n + col..n + col + len],
                    &weights[2 * n + col..2 * n + col + len],
                    &weights[3 * n + col..3 * n + col + len],
                    &mut lo_buf[..len],
                );
                Self::widen_4rows_interleaved_into(
                    &weights[4 * n + col..4 * n + col + len],
                    &weights[5 * n + col..5 * n + col + len],
                    &weights[6 * n + col..6 * n + col + len],
                    &weights[7 * n + col..7 * n + col + len],
                    &mut hi_buf[..len],
                );
                affine_block8_interleaved(
                    &lo_buf[..len],
                    &hi_buf[..len],
                    &mut z0,
                    &mut z1,
                    &input[col..col + len],
                );
                col += len;
            }
        }
        out_block[0..4].copy_from_slice(&z0);
        out_block[4..8].copy_from_slice(&z1);
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
        let mut z = Self::to_f32_4(*biases);
        let (cols_buf, _) = buf.as_chunks_mut::<4>();
        if n <= F16_TILE_COLS {
            Self::widen_4rows_contiguous_interleaved_into(
                &weights[0..4 * n],
                n,
                &mut cols_buf[..n],
            );
            affine_block4_interleaved(&cols_buf[..n], &mut z, input);
        } else {
            let mut col = 0_usize;
            while col < n {
                let len = (n - col).min(F16_TILE_COLS);
                Self::widen_4rows_interleaved_into(
                    &weights[col..col + len],
                    &weights[n + col..n + col + len],
                    &weights[2 * n + col..2 * n + col + len],
                    &weights[3 * n + col..3 * n + col + len],
                    &mut cols_buf[..len],
                );
                affine_block4_interleaved(&cols_buf[..len], &mut z, &input[col..col + len]);
                col += len;
            }
        }
        *out_block = z;
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
