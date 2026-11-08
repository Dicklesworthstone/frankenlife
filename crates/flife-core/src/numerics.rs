//! Host-independent elementary functions for the reference numerical profile
//! (plan A.1: activations, the emission exponential and decay factors are
//! numerical-profile definitions).
//!
//! Model arithmetic must not depend on the platform's `libm`: two hosts that
//! disagree in the last bit of `exp` or `tanh` would diverge. These
//! functions use only IEEE-754 addition, subtraction, multiplication and
//! division in a fixed order, with no fused multiply-add, so every
//! conforming host computes the same bits. Square root is correctly rounded
//! by IEEE-754 and may be used directly.

/// Arguments are halved until they are at most this, then a series runs.
const REDUCED: f64 = 1.0 / 16.0;

/// Taylor terms after the constant for [`exp_neg`]. For `y ≤ 1/16` the
/// first omitted term is below `(1/16)^13 / 13! ≈ 5e-26`.
const EXP_TERMS: u32 = 12;

/// Terms for [`expm1_neg`]'s series `y(1 − y/2(1 − y/3(…)))`. The first
/// omitted term is below `(1/16)^14 / 14!`, relative to `y`.
const EXPM1_TERMS: u32 = 13;

/// Beyond this, `1 − e^(−x)` rounds to `1`: `e^(−38) < 2^(−54)`.
const EXPM1_SATURATES: f64 = 40.0;

/// Beyond this, `tanh(x)` rounds to `±1`: `2 e^(−40) < 2^(−54)`.
const TANH_SATURATES: f64 = 20.0;

/// `ln 2` split Cody–Waite style: the high part has 32 significant bits, so
/// `n × LN2_HI` is exact for `|n| < 2^21`.
const LN2_HI: f64 = f64::from_bits(0x3fe6_2e42_fee0_0000);
/// `ln 2 − LN2_HI`.
const LN2_LO: f64 = f64::from_bits(0x3dea_39ef_3579_3c76);

/// Taylor terms for `e^(−r)` with `|r| ≤ ln 2 / 2`: the first omitted term
/// is below `0.35^16 / 16! ≈ 2e-21`.
const REDUCED_EXP_TERMS: u32 = 15;

/// Beyond this, `exp(−x)` is below half the smallest subnormal.
const EXP_UNDERFLOWS: f64 = 746.0;

/// At and beyond this, `exp(x)` overflows: `ln(f64::MAX) ≈ 709.78`.
const EXP_OVERFLOWS: f64 = 710.0;

/// Nested Taylor factors for the quarter-turn kernel. With `x ≤ π/4` the
/// first omitted sine term is below `x^20/21! ≈ 2e-22` relative to `x`, and
/// the first omitted cosine term below `x^20/20! ≈ 3e-21`.
const TRIG_TERMS: u32 = 9;

/// `exp(−x)` for finite `x ≥ 0`, computed identically on every IEEE-754 host.
///
/// For `x ≤ 1/16` the series `Σ (−x)^i / i!` is evaluated directly by
/// Horner's rule from the highest term. Larger arguments are reduced
/// Cody–Waite style to `x = n ln 2 + r` with `|r| ≤ ln 2 / 2`. The series
/// then gives `e^(−r)`, and scaling by `2^(−n)` is exact unless the result
/// is subnormal. The reduction does not amplify error, unlike repeated
/// squaring. Measured against correctly rounded values over 1,296 arguments
/// from `1e-9` to `745`: at most 1 ulp, and exact at Appendix A's decay
/// factor (`x = 7.8125e-5`) and a source kernel's cutoff (`x = 8`). Returns
/// `None` for a negative or nonfinite argument. Underflows to `0.0` for
/// large `x`, as `exp` does.
#[must_use]
pub fn exp_neg(x: f64) -> Option<f64> {
    if !x.is_finite() || x < 0.0 {
        return None;
    }
    if x <= REDUCED {
        return Some(exp_series(x, EXP_TERMS));
    }
    if x >= EXP_UNDERFLOWS {
        return Some(0.0);
    }
    // `x < 746`, so `n ≤ 1077` and `n × LN2_HI` is exact; `x − n LN2_HI` is
    // exact by Sterbenz's lemma.
    let n = (x / core::f64::consts::LN_2).round();
    let r = (x - n * LN2_HI) - n * LN2_LO;
    let scaled = exp_series(r, REDUCED_EXP_TERMS);
    // `n ≥ 0` here, and below 1078: scale in two exact power-of-two steps
    // so that only a subnormal result rounds.
    let n = n as i32;
    let first = n.min(1000);
    Some(scaled * power_of_two(-first) * power_of_two(first - n))
}

/// `Σ_{i ≤ terms} (−y)^i / i!` by Horner's rule from the highest term.
fn exp_series(y: f64, terms: u32) -> f64 {
    let mut series = 1.0_f64;
    for term in (1..=terms).rev() {
        series = 1.0 - y / f64::from(term) * series;
    }
    series
}

/// `2^k` for `−1022 ≤ k ≤ 1023`, built from its bits.
fn power_of_two(k: i32) -> f64 {
    // The biased exponent `k + 1023` lies in `[1, 2046]`.
    let biased = u64::try_from(k + 1023).unwrap_or(1);
    f64::from_bits(biased << 52)
}

/// `1 − exp(−x) = −expm1(−x)` for finite `x ≥ 0`, within 2 ulp of the
/// correctly rounded result (measured) even when `x` is tiny, where
/// `1 − exp_neg(x)` would cancel to nothing (plan C.11).
///
/// For `x ≥ ln 2`, `e^(−x) ≤ 1/2` and `1 − exp_neg(x)` cannot cancel.
/// Below that, the argument is reduced to `y = x / 2^k ≤ 1/16` with `k ≤ 4`.
/// The series `m = y(1 − y/2(1 − y/3(…)))` gives `1 − e^(−y)`, and the
/// doubling identity `1 − e^(−2y) = m(2 − m)` is applied `k` times; unlike
/// squaring, doubling does not amplify relative error. Returns `None` for a
/// negative or nonfinite argument.
#[must_use]
pub fn expm1_neg(x: f64) -> Option<f64> {
    if !x.is_finite() || x < 0.0 {
        return None;
    }
    if x >= EXPM1_SATURATES {
        return Some(1.0);
    }
    if x >= core::f64::consts::LN_2 {
        // `e^(−x) ≤ 1/2`, so `1 − e^(−x)` cannot cancel: its half-ulp
        // rounding plus a quarter ulp from `exp_neg` stays within 1 ulp.
        return Some(1.0 - exp_neg(x)?);
    }
    let (y, halvings) = reduce(x);
    let mut series = 1.0_f64;
    for term in (2..=EXPM1_TERMS + 1).rev() {
        series = 1.0 - y / f64::from(term) * series;
    }
    let mut m = y * series;
    for _ in 0..halvings {
        m *= 2.0 - m;
    }
    Some(m)
}

/// `tanh(x)` for finite `x`, computed identically on every IEEE-754 host.
///
/// For `0 ≤ x < 1`, `tanh x = m/(2 − m)` with `m = 1 − e^(−2x)` from
/// [`expm1_neg`], which does not cancel near zero. For `x ≥ 1`,
/// `tanh x = 1 − 2d/(1 + d)` with `d = e^(−2x)`, which keeps the last bits
/// below `1`. Odd symmetry gives negative arguments, and `tanh(±0) = ±0`.
/// Within 2 ulp of the correctly rounded result (measured). Returns `None`
/// for a nonfinite argument.
#[must_use]
pub fn tanh(x: f64) -> Option<f64> {
    if !x.is_finite() {
        return None;
    }
    let magnitude = x.abs();
    let value = if magnitude >= TANH_SATURATES {
        1.0
    } else if magnitude >= 1.0 {
        // `d = e^(−2x) ≤ e^(−2)`: `tanh x = 1 − 2d/(1 + d)`, where the
        // subtrahend is below a quarter and its error below half an ulp of
        // the result. This keeps `tanh(19) = 1 − 2^(−53)`, which
        // `(1 − d)/(1 + d)` would round to `1`.
        let d = exp_neg(2.0 * magnitude)?;
        1.0 - 2.0 * d / (1.0 + d)
    } else {
        let m = expm1_neg(2.0 * magnitude)?;
        m / (2.0 - m)
    };
    Some(if x.is_sign_negative() { -value } else { value })
}

/// The logistic `σ(x) = 1/(1 + e^(−x))` for finite `x`, with a sign-stable
/// branch (plan A.1): `e^(−|x|)` never overflows, and a large negative
/// logit decays smoothly to zero instead of dividing by infinity. Within
/// 2 ulp of the correctly rounded result (measured). Returns `None` for a
/// nonfinite argument.
#[must_use]
pub fn sigmoid(x: f64) -> Option<f64> {
    if !x.is_finite() {
        return None;
    }
    let decay = exp_neg(x.abs())?;
    Some(if x >= 0.0 {
        1.0 / (1.0 + decay)
    } else {
        decay / (1.0 + decay)
    })
}

/// Terms after the first of `atanh(z)/z = Σ z^(2k)/(2k + 1)`. For
/// `|z| ≤ (√2 − 1)/(√2 + 1) < 0.172` the first omitted term is below
/// `0.172^26 / 27 < 1e-21`.
const LOG_TERMS: u32 = 12;

/// `2^54`, which brings a subnormal into the normal range exactly.
const SUBNORMAL_SCALE: f64 = 18_014_398_509_481_984.0;

/// `log₂(x)` for finite `x > 0`, computed identically on every IEEE-754
/// host.
///
/// The exponent `n` and mantissa `m` come exactly from the bits, with `m`
/// moved into `(√2/2, √2]`. Then `ln m = 2 atanh(z)` with
/// `z = (m − 1)/(m + 1)`, where `m − 1` is exact by Sterbenz's lemma, and
/// `log₂ x = n + ln m · log₂ e`. A power of two gives its exponent exactly,
/// so a pyramid level boundary blends with weight exactly zero (§6.1).
/// Returns `None` for a nonpositive or nonfinite argument.
#[must_use]
pub fn log2(x: f64) -> Option<f64> {
    let (exponent, natural) = log_parts(x)?;
    Some(f64::from(exponent) + natural * core::f64::consts::LOG2_E)
}

/// `ln(x)` for finite `x > 0`, computed identically on every IEEE-754 host.
///
/// With [`log2`]'s exponent `n` and `ln m`, `ln x = n LN2_HI + (n LN2_LO +
/// ln m)`. The first product is exact, and when `n ≠ 0` it dominates
/// `|ln m| ≤ ln 2 / 2`, so the final sum rounds once. Measured against
/// correctly rounded values over 12,387 arguments from `1e-300` to `1e300`,
/// including open uniforms near both ends of `(0, 1)`: at most 2 ulp.
/// `ln 1 = 0` exactly. Returns `None` for a nonpositive or nonfinite
/// argument.
#[must_use]
pub fn ln(x: f64) -> Option<f64> {
    let (exponent, natural) = log_parts(x)?;
    let n = f64::from(exponent);
    Some(n * LN2_HI + (n * LN2_LO + natural))
}

/// `(n, ln m)` with `x = m 2^n` and `m ∈ (√2/2, √2]`.
fn log_parts(x: f64) -> Option<(i32, f64)> {
    if !x.is_finite() || x <= 0.0 {
        return None;
    }
    let (normal, adjust) = if x < f64::MIN_POSITIVE {
        (x * SUBNORMAL_SCALE, -54)
    } else {
        (x, 0)
    };
    let bits = normal.to_bits();
    let biased = i32::try_from((bits >> 52) & 0x7ff).unwrap_or(0);
    let mut exponent = biased - 1023 + adjust;
    let mut mantissa = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000);
    if mantissa > core::f64::consts::SQRT_2 {
        mantissa *= 0.5;
        exponent += 1;
    }
    let z = (mantissa - 1.0) / (mantissa + 1.0);
    let z2 = z * z;
    // Horner from the highest term: `P_k = 1/(2k + 1) + z² P_(k+1)`, and
    // `atanh(z) = z + z · z² P_1`.
    let mut series = 0.0_f64;
    for k in (1..=LOG_TERMS).rev() {
        series = 1.0 / f64::from(2 * k + 1) + z2 * series;
    }
    Some((exponent, 2.0 * (z + z * (z2 * series))))
}

/// `exp(x)` for finite `x`, computed identically on every IEEE-754 host.
///
/// A nonpositive argument is [`exp_neg`]`(−x)`. Otherwise `x ≤ 1/16` runs
/// the series directly, and larger arguments reduce Cody–Waite style to
/// `x = n ln 2 + r` with `|r| ≤ ln 2 / 2`, scaling `e^r` by `2^n` exactly.
/// Measured against correctly rounded values over 3,292 arguments of both
/// signs up to `709`: at most 1 ulp. Returns `None` for a nonfinite argument
/// or an overflowing result.
#[must_use]
pub fn exp(x: f64) -> Option<f64> {
    if !x.is_finite() {
        return None;
    }
    if x <= 0.0 {
        return exp_neg(-x);
    }
    if x >= EXP_OVERFLOWS {
        return None;
    }
    if x <= REDUCED {
        return Some(exp_series(-x, EXP_TERMS));
    }
    // `x < 710`, so `n ≤ 1024`; the reduction is exact as in `exp_neg`.
    let n = (x / core::f64::consts::LN_2).round();
    let r = (x - n * LN2_HI) - n * LN2_LO;
    let scaled = exp_series(-r, REDUCED_EXP_TERMS);
    let n = n as i32;
    let first = n.min(1000);
    let value = scaled * power_of_two(first) * power_of_two(n - first);
    value.is_finite().then_some(value)
}

/// `(sin 2πu, cos 2πu)` for `0 ≤ u < 1`, computed identically on every
/// IEEE-754 host: an angle measured in turns, as a uniform draw gives it.
///
/// The quadrant `q = ⌊4u⌋` and `r = 4u − q` are exact. When `r > 1/2` the
/// complement `1 − r` is taken, also exactly, and sine and cosine swap.
/// The kernel then evaluates `sin x` and `cos x` for `x = (π/2) r ≤ π/4` by
/// nested Taylor series, and the quadrant rotates the pair. Quarter turns
/// are exact. Measured against correctly rounded values over 4,096 turns: at
/// most 2 ulp. Returns `None` outside `[0, 1)`.
#[must_use]
pub fn sin_cos_turns(u: f64) -> Option<(f64, f64)> {
    if !(0.0..1.0).contains(&u) {
        return None;
    }
    let quarters = 4.0 * u;
    let quadrant = quarters.floor();
    let r = quarters - quadrant;
    let (sin, cos) = if r <= 0.5 {
        quarter_turn_kernel(r)
    } else {
        let (sin, cos) = quarter_turn_kernel(1.0 - r);
        (cos, sin)
    };
    Some(match quadrant as u8 {
        0 => (sin, cos),
        1 => (cos, -sin),
        2 => (-sin, -cos),
        _ => (-cos, sin),
    })
}

/// `(sin x, cos x)` for `x = (π/2) r`, `0 ≤ r ≤ 1/2`, by
/// `sin x = x(1 − x²/(2·3)(1 − x²/(4·5)(…)))` and
/// `cos x = 1 − x²/(1·2)(1 − x²/(3·4)(…))`.
fn quarter_turn_kernel(r: f64) -> (f64, f64) {
    let x = r * core::f64::consts::FRAC_PI_2;
    let x2 = x * x;
    let mut sin = 1.0_f64;
    let mut cos = 1.0_f64;
    for k in (1..=TRIG_TERMS).rev() {
        sin = 1.0 - x2 / f64::from((2 * k) * (2 * k + 1)) * sin;
        cos = 1.0 - x2 / f64::from((2 * k - 1) * (2 * k)) * cos;
    }
    (x * sin, cos)
}

/// `x = y 2^k` with `y ≤ 1/16`, halving exactly.
fn reduce(x: f64) -> (f64, u32) {
    let mut y = x;
    let mut halvings = 0_u32;
    while y > REDUCED {
        y *= 0.5;
        halvings += 1;
    }
    (y, halvings)
}
