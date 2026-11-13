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

    /// `value` rounded once to this dtype, to nearest with ties to even.
    fn round_from_f64(value: f64) -> Self;

    /// The weight's bits, widened to 32 (equality of bits, not of values:
    /// `-0` and `+0` differ).
    fn bits(self) -> u32;
}

impl Weight for f32 {
    const DTYPE: WeightDtype = WeightDtype::F32;

    #[inline]
    fn widen(self) -> f32 {
        self
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

    #[inline]
    fn widen(self) -> f32 {
        self.to_f32()
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
