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

    /// Accumulate a 4-row × 4-column weight tile against 4 inputs `x` into
    /// 4 row accumulators `z`, adding columns in strictly ascending order
    /// `k = 0..4` (`z[r] = (((z[r] + w[r][0]*x[0]) + w[r][1]*x[1]) +
    /// w[r][2]*x[2]) + w[r][3]*x[3]`), 0-ULP bit-identical to scalar F32
    /// multiply-then-add (no FMA).
    #[inline(always)]
    fn mac_4x4(z: [f32; 4], rows: [[Self; 4]; 4], x: [f32; 4]) -> [f32; 4] {
        mac_4x4_f32(
            z,
            [
                Self::widen_4(rows[0]),
                Self::widen_4(rows[1]),
                Self::widen_4(rows[2]),
                Self::widen_4(rows[3]),
            ],
            x,
        )
    }

    /// `value` rounded once to this dtype, to nearest with ties to even.
    fn round_from_f64(value: f64) -> Self;

    /// The weight's bits, widened to 32 (equality of bits, not of values:
    /// `-0` and `+0` differ).
    fn bits(self) -> u32;
}

/// Accumulate four widened F32 weight rows against `x` into `z` in strict
/// ascending column order `k = 0..4`, using an explicit two-stage 4×4
/// butterfly transpose (`trn1`/`trn2` on NEON, `unpcklps`/`unpckhps` +
/// `movlhps`/`movhlps` on x86_64) so products are computed row-wise and summed
/// column-wise with zero scalar lane insertions and 0-ULP exactness.
#[inline(always)]
fn mac_4x4_f32(z: [f32; 4], rows: [[f32; 4]; 4], x: [f32; 4]) -> [f32; 4] {
    let [a0, a1, a2, a3] = rows;
    let p0 = [a0[0] * x[0], a0[1] * x[1], a0[2] * x[2], a0[3] * x[3]];
    let p1 = [a1[0] * x[0], a1[1] * x[1], a1[2] * x[2], a1[3] * x[3]];
    let p2 = [a2[0] * x[0], a2[1] * x[1], a2[2] * x[2], a2[3] * x[3]];
    let p3 = [a3[0] * x[0], a3[1] * x[1], a3[2] * x[2], a3[3] * x[3]];
    // Stage 1: interleave 32-bit pairs within (p0, p1) and (p2, p3).
    let t0 = [p0[0], p1[0], p0[2], p1[2]];
    let t1 = [p0[1], p1[1], p0[3], p1[3]];
    let t2 = [p2[0], p3[0], p2[2], p3[2]];
    let t3 = [p2[1], p3[1], p2[3], p3[3]];
    // Stage 2: interleave 64-bit halves to yield the 4 column vectors.
    let c0 = [t0[0], t0[1], t2[0], t2[1]];
    let c1 = [t1[0], t1[1], t3[0], t3[1]];
    let c2 = [t0[2], t0[3], t2[2], t2[3]];
    let c3 = [t1[2], t1[3], t3[2], t3[3]];
    let z = [z[0] + c0[0], z[1] + c0[1], z[2] + c0[2], z[3] + c0[3]];
    let z = [z[0] + c1[0], z[1] + c1[1], z[2] + c1[2], z[3] + c1[3]];
    let z = [z[0] + c2[0], z[1] + c2[1], z[2] + c2[2], z[3] + c2[3]];
    [z[0] + c3[0], z[1] + c3[1], z[2] + c3[2], z[3] + c3[3]]
}

/// Cold exact F16 4×4 MAC fallback invoked when a tile contains any zero,
/// subnormal, infinity, or NaN weight. Keeping this out-of-line keeps the hot
/// normal-only loop free of subnormal/Inf constants and merges only `[f32; 4]`.
#[cold]
#[inline(never)]
fn mac_4x4_f16_slow(z: [f32; 4], rows: [[F16; 4]; 4], x: [f32; 4]) -> [f32; 4] {
    mac_4x4_f32(
        z,
        [
            F16::to_f32_4(rows[0]),
            F16::to_f32_4(rows[1]),
            F16::to_f32_4(rows[2]),
            F16::to_f32_4(rows[3]),
        ],
        x,
    )
}

impl Weight for f32 {
    const DTYPE: WeightDtype = WeightDtype::F32;

    #[inline(always)]
    fn widen(self) -> f32 {
        self
    }

    #[inline(always)]
    fn widen_4(values: [Self; 4]) -> [f32; 4] {
        values
    }

    #[inline(always)]
    fn mac_4x4(z: [f32; 4], rows: [[Self; 4]; 4], x: [f32; 4]) -> [f32; 4] {
        mac_4x4_f32(z, rows, x)
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

    #[inline(always)]
    fn widen(self) -> f32 {
        self.to_f32()
    }

    #[inline(always)]
    fn widen_4(values: [Self; 4]) -> [f32; 4] {
        Self::to_f32_4(values)
    }

    #[inline(always)]
    fn mac_4x4(z: [f32; 4], rows: [[Self; 4]; 4], x: [f32; 4]) -> [f32; 4] {
        if let Some(widened) = Self::to_f32_4x4_if_normal(rows) {
            mac_4x4_f32(z, widened, x)
        } else {
            mac_4x4_f16_slow(z, rows, x)
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
