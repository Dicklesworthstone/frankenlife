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
