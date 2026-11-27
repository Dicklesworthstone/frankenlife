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

use crate::error::CoreError;

/// Arguments are halved until they are at most this, then a series runs.
const REDUCED: f64 = 1.0 / 16.0;

/// Beyond this, `1 − e^(−x)` rounds to `1`: `e^(−38) < 2^(−54)`.
const EXPM1_SATURATES: f64 = 40.0;

/// Beyond this, `tanh(x)` rounds to `±1`: `2 e^(−40) < 2^(−54)`.
const TANH_SATURATES: f64 = 20.0;

/// `ln 2` split Cody–Waite style: the high part has 32 significant bits, so
/// `n × LN2_HI` is exact for `|n| < 2^21`.
const LN2_HI: f64 = f64::from_bits(0x3fe6_2e42_fee0_0000);
/// `ln 2 − LN2_HI`.
const LN2_LO: f64 = f64::from_bits(0x3dea_39ef_3579_3c76);

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
#[inline]
#[must_use]
pub fn exp_neg(x: f64) -> Option<f64> {
    if !x.is_finite() || x < 0.0 {
        return None;
    }
    if x == 0.0 {
        return Some(1.0);
    }
    if x <= REDUCED {
        return Some(exp_series_12(x));
    }
    if x >= EXP_UNDERFLOWS {
        return Some(0.0);
    }
    // `x < 746`, so `n ≤ 1077` and `n × LN2_HI` is exact; `x − n LN2_HI` is
    // exact by Sterbenz's lemma.
    let n = (x / core::f64::consts::LN_2).round();
    let r = (x - n * LN2_HI) - n * LN2_LO;
    let scaled = exp_series_15(r);
    // `n ≥ 0` here, and below 1078: scale in two exact power-of-two steps
    // so that only a subnormal result rounds.
    let n = n as i32;
    let first = n.min(1000);
    Some(scaled * power_of_two(-first) * power_of_two(first - n))
}

/// Four-lane batch [`exp_neg`], bit-identical to
/// `[exp_neg(xs[0])?, exp_neg(xs[1])?, exp_neg(xs[2])?, exp_neg(xs[3])?]`.
///
/// Uses the exact same IEEE-754 operations and parenthesization per lane in
/// lockstep fixed-size arrays so LLVM auto-vectorizes across lanes (`fdiv.2d` /
/// `fmul.2d` / `fsub.2d`) without FMA contraction or numerical drift. Returns
/// `None` if any lane is negative or nonfinite.
#[inline]
#[must_use]
pub fn exp_neg_4(xs: [f64; 4]) -> Option<[f64; 4]> {
    if !xs[0].is_finite()
        || xs[0] < 0.0
        || !xs[1].is_finite()
        || xs[1] < 0.0
        || !xs[2].is_finite()
        || xs[2] < 0.0
        || !xs[3].is_finite()
        || xs[3] < 0.0
    {
        return None;
    }
    let is_small = [
        xs[0] > 0.0 && xs[0] <= REDUCED,
        xs[1] > 0.0 && xs[1] <= REDUCED,
        xs[2] > 0.0 && xs[2] <= REDUCED,
        xs[3] > 0.0 && xs[3] <= REDUCED,
    ];
    let is_mid = [
        xs[0] > REDUCED && xs[0] < EXP_UNDERFLOWS,
        xs[1] > REDUCED && xs[1] < EXP_UNDERFLOWS,
        xs[2] > REDUCED && xs[2] < EXP_UNDERFLOWS,
        xs[3] > REDUCED && xs[3] < EXP_UNDERFLOWS,
    ];
    if is_mid[0] && is_mid[1] && is_mid[2] && is_mid[3] {
        let n = [
            (xs[0] / core::f64::consts::LN_2).round(),
            (xs[1] / core::f64::consts::LN_2).round(),
            (xs[2] / core::f64::consts::LN_2).round(),
            (xs[3] / core::f64::consts::LN_2).round(),
        ];
        let r = [
            (xs[0] - n[0] * LN2_HI) - n[0] * LN2_LO,
            (xs[1] - n[1] * LN2_HI) - n[1] * LN2_LO,
            (xs[2] - n[2] * LN2_HI) - n[2] * LN2_LO,
            (xs[3] - n[3] * LN2_HI) - n[3] * LN2_LO,
        ];
        let scaled = exp_series_15_4(r);
        let scale_lane = |s: f64, nf: f64| {
            let ni = nf as i32;
            let first = ni.min(1000);
            s * power_of_two(-first) * power_of_two(first - ni)
        };
        return Some([
            scale_lane(scaled[0], n[0]),
            scale_lane(scaled[1], n[1]),
            scale_lane(scaled[2], n[2]),
            scale_lane(scaled[3], n[3]),
        ]);
    }
    if !(is_mid[0] | is_mid[1] | is_mid[2] | is_mid[3]) {
        if !(is_small[0] | is_small[1] | is_small[2] | is_small[3]) {
            return Some([
                if xs[0] == 0.0 { 1.0 } else { 0.0 },
                if xs[1] == 0.0 { 1.0 } else { 0.0 },
                if xs[2] == 0.0 { 1.0 } else { 0.0 },
                if xs[3] == 0.0 { 1.0 } else { 0.0 },
            ]);
        }
        let y = [
            if is_small[0] { xs[0] } else { 0.0 },
            if is_small[1] { xs[1] } else { 0.0 },
            if is_small[2] { xs[2] } else { 0.0 },
            if is_small[3] { xs[3] } else { 0.0 },
        ];
        let s = exp_series_12_4(y);
        return Some([
            if is_small[0] {
                s[0]
            } else if xs[0] == 0.0 {
                1.0
            } else {
                0.0
            },
            if is_small[1] {
                s[1]
            } else if xs[1] == 0.0 {
                1.0
            } else {
                0.0
            },
            if is_small[2] {
                s[2]
            } else if xs[2] == 0.0 {
                1.0
            } else {
                0.0
            },
            if is_small[3] {
                s[3]
            } else if xs[3] == 0.0 {
                1.0
            } else {
                0.0
            },
        ]);
    }
    if !(is_small[0] | is_small[1] | is_small[2] | is_small[3]) {
        // Only `mid`, `0.0`, and underflowing lanes: vectorize the mid lanes
        // without scalar fallback.
        let xm = [
            if is_mid[0] { xs[0] } else { 1.0 },
            if is_mid[1] { xs[1] } else { 1.0 },
            if is_mid[2] { xs[2] } else { 1.0 },
            if is_mid[3] { xs[3] } else { 1.0 },
        ];
        let n = [
            (xm[0] / core::f64::consts::LN_2).round(),
            (xm[1] / core::f64::consts::LN_2).round(),
            (xm[2] / core::f64::consts::LN_2).round(),
            (xm[3] / core::f64::consts::LN_2).round(),
        ];
        let r = [
            (xm[0] - n[0] * LN2_HI) - n[0] * LN2_LO,
            (xm[1] - n[1] * LN2_HI) - n[1] * LN2_LO,
            (xm[2] - n[2] * LN2_HI) - n[2] * LN2_LO,
            (xm[3] - n[3] * LN2_HI) - n[3] * LN2_LO,
        ];
        let scaled = exp_series_15_4(r);
        let scale_lane = |s: f64, nf: f64, mid: bool, x: f64| {
            if mid {
                let ni = nf as i32;
                let first = ni.min(1000);
                s * power_of_two(-first) * power_of_two(first - ni)
            } else if x == 0.0 {
                1.0
            } else {
                0.0
            }
        };
        return Some([
            scale_lane(scaled[0], n[0], is_mid[0], xs[0]),
            scale_lane(scaled[1], n[1], is_mid[1], xs[1]),
            scale_lane(scaled[2], n[2], is_mid[2], xs[2]),
            scale_lane(scaled[3], n[3], is_mid[3], xs[3]),
        ]);
    }
    Some([
        exp_neg(xs[0])?,
        exp_neg(xs[1])?,
        exp_neg(xs[2])?,
        exp_neg(xs[3])?,
    ])
}

/// Four-lane lockstep [`exp_series_12`], bit-identical per lane.
#[inline]
fn exp_series_12_4(y: [f64; 4]) -> [f64; 4] {
    let mut s = [
        1.0 - y[0] / 12.0,
        1.0 - y[1] / 12.0,
        1.0 - y[2] / 12.0,
        1.0 - y[3] / 12.0,
    ];
    for div in [11.0, 10.0, 9.0] {
        for (si, &yi) in s.iter_mut().zip(&y) {
            *si = 1.0 - (yi / div) * *si;
        }
    }
    for (si, &yi) in s.iter_mut().zip(&y) {
        *si = 1.0 - (yi * 0.125) * *si;
    }
    for div in [7.0, 6.0, 5.0] {
        for (si, &yi) in s.iter_mut().zip(&y) {
            *si = 1.0 - (yi / div) * *si;
        }
    }
    for (si, &yi) in s.iter_mut().zip(&y) {
        *si = 1.0 - (yi * 0.25) * *si;
    }
    for (si, &yi) in s.iter_mut().zip(&y) {
        *si = 1.0 - (yi / 3.0) * *si;
    }
    for (si, &yi) in s.iter_mut().zip(&y) {
        *si = 1.0 - (yi * 0.5) * *si;
    }
    for (si, &yi) in s.iter_mut().zip(&y) {
        *si = 1.0 - yi * *si;
    }
    s
}

/// Four-lane lockstep [`exp_series_15`], bit-identical per lane.
#[inline]
fn exp_series_15_4(y: [f64; 4]) -> [f64; 4] {
    let mut s = [
        1.0 - y[0] / 15.0,
        1.0 - y[1] / 15.0,
        1.0 - y[2] / 15.0,
        1.0 - y[3] / 15.0,
    ];
    for div in [14.0, 13.0, 12.0, 11.0, 10.0, 9.0] {
        for (si, &yi) in s.iter_mut().zip(&y) {
            *si = 1.0 - (yi / div) * *si;
        }
    }
    for (si, &yi) in s.iter_mut().zip(&y) {
        *si = 1.0 - (yi * 0.125) * *si;
    }
    for div in [7.0, 6.0, 5.0] {
        for (si, &yi) in s.iter_mut().zip(&y) {
            *si = 1.0 - (yi / div) * *si;
        }
    }
    for (si, &yi) in s.iter_mut().zip(&y) {
        *si = 1.0 - (yi * 0.25) * *si;
    }
    for (si, &yi) in s.iter_mut().zip(&y) {
        *si = 1.0 - (yi / 3.0) * *si;
    }
    for (si, &yi) in s.iter_mut().zip(&y) {
        *si = 1.0 - (yi * 0.5) * *si;
    }
    for (si, &yi) in s.iter_mut().zip(&y) {
        *si = 1.0 - yi * *si;
    }
    s
}

/// `Σ_{i ≤ 12} (−y)^i / i!` by Horner's rule from the highest term.
/// Independent quotients `y / k` are evaluated up front (with exact
/// power-of-two multiplies for `2, 4, 8` and `y / 1 = y`) so out-of-order
/// FP dividers pipeline concurrently while preserving every IEEE-754 bit.
#[inline]
fn exp_series_12(y: f64) -> f64 {
    let q12 = y / 12.0;
    let q11 = y / 11.0;
    let q10 = y / 10.0;
    let q9 = y / 9.0;
    let q8 = y * 0.125;
    let q7 = y / 7.0;
    let q6 = y / 6.0;
    let q5 = y / 5.0;
    let q4 = y * 0.25;
    let q3 = y / 3.0;
    let q2 = y * 0.5;
    let s = 1.0 - q12;
    let s = 1.0 - q11 * s;
    let s = 1.0 - q10 * s;
    let s = 1.0 - q9 * s;
    let s = 1.0 - q8 * s;
    let s = 1.0 - q7 * s;
    let s = 1.0 - q6 * s;
    let s = 1.0 - q5 * s;
    let s = 1.0 - q4 * s;
    let s = 1.0 - q3 * s;
    let s = 1.0 - q2 * s;
    1.0 - y * s
}

/// `Σ_{i ≤ 15} (−y)^i / i!` by Horner's rule from the highest term,
/// bit-identical to the unrolled loop.
#[inline]
fn exp_series_15(y: f64) -> f64 {
    let q15 = y / 15.0;
    let q14 = y / 14.0;
    let q13 = y / 13.0;
    let q12 = y / 12.0;
    let q11 = y / 11.0;
    let q10 = y / 10.0;
    let q9 = y / 9.0;
    let q8 = y * 0.125;
    let q7 = y / 7.0;
    let q6 = y / 6.0;
    let q5 = y / 5.0;
    let q4 = y * 0.25;
    let q3 = y / 3.0;
    let q2 = y * 0.5;
    let s = 1.0 - q15;
    let s = 1.0 - q14 * s;
    let s = 1.0 - q13 * s;
    let s = 1.0 - q12 * s;
    let s = 1.0 - q11 * s;
    let s = 1.0 - q10 * s;
    let s = 1.0 - q9 * s;
    let s = 1.0 - q8 * s;
    let s = 1.0 - q7 * s;
    let s = 1.0 - q6 * s;
    let s = 1.0 - q5 * s;
    let s = 1.0 - q4 * s;
    let s = 1.0 - q3 * s;
    let s = 1.0 - q2 * s;
    1.0 - y * s
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
#[inline]
#[must_use]
pub fn expm1_neg(x: f64) -> Option<f64> {
    if !x.is_finite() || x < 0.0 {
        return None;
    }
    if x == 0.0 {
        return Some(x);
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
    let q14 = y / 14.0;
    let q13 = y / 13.0;
    let q12 = y / 12.0;
    let q11 = y / 11.0;
    let q10 = y / 10.0;
    let q9 = y / 9.0;
    let q8 = y * 0.125;
    let q7 = y / 7.0;
    let q6 = y / 6.0;
    let q5 = y / 5.0;
    let q4 = y * 0.25;
    let q3 = y / 3.0;
    let q2 = y * 0.5;
    let s = 1.0 - q14;
    let s = 1.0 - q13 * s;
    let s = 1.0 - q12 * s;
    let s = 1.0 - q11 * s;
    let s = 1.0 - q10 * s;
    let s = 1.0 - q9 * s;
    let s = 1.0 - q8 * s;
    let s = 1.0 - q7 * s;
    let s = 1.0 - q6 * s;
    let s = 1.0 - q5 * s;
    let s = 1.0 - q4 * s;
    let s = 1.0 - q3 * s;
    let series = 1.0 - q2 * s;
    let mut m = y * series;
    for _ in 0..halvings {
        m *= 2.0 - m;
    }
    Some(m)
}

/// Four-lane batch [`expm1_neg`], bit-identical to
/// `[expm1_neg(xs[0])?, expm1_neg(xs[1])?, expm1_neg(xs[2])?, expm1_neg(xs[3])?]`.
///
/// Executes the exact same IEEE-754 operations per lane in lockstep 4-lane
/// arrays so LLVM emits 2-wide F64 NEON divisions and multiplies without FMA
/// contraction or numerical drift. Returns `None` if any lane is negative or
/// nonfinite.
#[inline]
#[must_use]
pub fn expm1_neg_4(xs: [f64; 4]) -> Option<[f64; 4]> {
    if !xs[0].is_finite()
        || xs[0] < 0.0
        || !xs[1].is_finite()
        || xs[1] < 0.0
        || !xs[2].is_finite()
        || xs[2] < 0.0
        || !xs[3].is_finite()
        || xs[3] < 0.0
    {
        return None;
    }
    let ln2 = core::f64::consts::LN_2;
    if xs[0] < ln2 && xs[1] < ln2 && xs[2] < ln2 && xs[3] < ln2 {
        let (y0, h0) = reduce(xs[0]);
        let (y1, h1) = reduce(xs[1]);
        let (y2, h2) = reduce(xs[2]);
        let (y3, h3) = reduce(xs[3]);
        let y = [y0, y1, y2, y3];
        let mut s = [
            1.0 - y[0] / 14.0,
            1.0 - y[1] / 14.0,
            1.0 - y[2] / 14.0,
            1.0 - y[3] / 14.0,
        ];
        for div in [13.0, 12.0, 11.0, 10.0, 9.0] {
            for (si, &yi) in s.iter_mut().zip(&y) {
                *si = 1.0 - (yi / div) * *si;
            }
        }
        for (si, &yi) in s.iter_mut().zip(&y) {
            *si = 1.0 - (yi * 0.125) * *si;
        }
        for div in [7.0, 6.0, 5.0] {
            for (si, &yi) in s.iter_mut().zip(&y) {
                *si = 1.0 - (yi / div) * *si;
            }
        }
        for (si, &yi) in s.iter_mut().zip(&y) {
            *si = 1.0 - (yi * 0.25) * *si;
        }
        for (si, &yi) in s.iter_mut().zip(&y) {
            *si = 1.0 - (yi / 3.0) * *si;
        }
        for (si, &yi) in s.iter_mut().zip(&y) {
            *si = 1.0 - (yi * 0.5) * *si;
        }
        let mut m = [y[0] * s[0], y[1] * s[1], y[2] * s[2], y[3] * s[3]];
        let hs = [h0, h1, h2, h3];
        for (mi, (&x, &h)) in m.iter_mut().zip(xs.iter().zip(&hs)) {
            if x == 0.0 {
                *mi = x;
            } else {
                for _ in 0..h {
                    *mi *= 2.0 - *mi;
                }
            }
        }
        return Some(m);
    }
    if xs[0] >= ln2 && xs[1] >= ln2 && xs[2] >= ln2 && xs[3] >= ln2 {
        // Every lane is `>= ln 2` (including saturating `>= 40`): vectorize
        // active `[ln 2, 40)` lanes in one `exp_neg_4` pass.
        let xm = [
            if xs[0] < EXPM1_SATURATES { xs[0] } else { 0.0 },
            if xs[1] < EXPM1_SATURATES { xs[1] } else { 0.0 },
            if xs[2] < EXPM1_SATURATES { xs[2] } else { 0.0 },
            if xs[3] < EXPM1_SATURATES { xs[3] } else { 0.0 },
        ];
        let e = exp_neg_4(xm)?;
        return Some([
            if xs[0] >= EXPM1_SATURATES {
                1.0
            } else {
                1.0 - e[0]
            },
            if xs[1] >= EXPM1_SATURATES {
                1.0
            } else {
                1.0 - e[1]
            },
            if xs[2] >= EXPM1_SATURATES {
                1.0
            } else {
                1.0 - e[2]
            },
            if xs[3] >= EXPM1_SATURATES {
                1.0
            } else {
                1.0 - e[3]
            },
        ]);
    }
    Some([
        expm1_neg(xs[0])?,
        expm1_neg(xs[1])?,
        expm1_neg(xs[2])?,
        expm1_neg(xs[3])?,
    ])
}

/// `tanh(x)` for finite `x`, computed identically on every IEEE-754 host.
///
/// For `0 ≤ x < 1`, `tanh x = m/(2 − m)` with `m = 1 − e^(−2x)` from
/// [`expm1_neg`], which does not cancel near zero. For `x ≥ 1`,
/// `tanh x = 1 − 2d/(1 + d)` with `d = e^(−2x)`, which keeps the last bits
/// below `1`. Odd symmetry gives negative arguments, and `tanh(±0) = ±0`.
/// Within 2 ulp of the correctly rounded result (measured). Returns `None`
/// for a nonfinite argument.
#[inline]
#[must_use]
pub fn tanh(x: f64) -> Option<f64> {
    if !x.is_finite() {
        return None;
    }
    if x == 0.0 {
        return Some(x);
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

/// Four-lane `exp(-x)` for `x ∈ (REDUCED, 40.0)` (`0 ≤ n ≤ 58`), bit-identical
/// to `[exp_neg(xs[0]), ...]` using single-step branchless `2^(-n)` scaling
/// (since `n ≤ 58 < 1000`, `first = n` and `power_of_two(first - n) = 1.0`,
/// and `scaled * 2^(-n)` stays strictly normal).
#[inline]
fn exp_neg_small_4(xs: [f64; 4]) -> [f64; 4] {
    let n = [
        (xs[0] / core::f64::consts::LN_2).round(),
        (xs[1] / core::f64::consts::LN_2).round(),
        (xs[2] / core::f64::consts::LN_2).round(),
        (xs[3] / core::f64::consts::LN_2).round(),
    ];
    let r = [
        (xs[0] - n[0] * LN2_HI) - n[0] * LN2_LO,
        (xs[1] - n[1] * LN2_HI) - n[1] * LN2_LO,
        (xs[2] - n[2] * LN2_HI) - n[2] * LN2_LO,
        (xs[3] - n[3] * LN2_HI) - n[3] * LN2_LO,
    ];
    let scaled = exp_series_15_4(r);
    [
        scaled[0] * power_of_two_neg_small(n[0]),
        scaled[1] * power_of_two_neg_small(n[1]),
        scaled[2] * power_of_two_neg_small(n[2]),
        scaled[3] * power_of_two_neg_small(n[3]),
    ]
}

/// Four-lane batch [`tanh`], bit-identical to
/// `[tanh(xs[0])?, tanh(xs[1])?, tanh(xs[2])?, tanh(xs[3])?]`.
///
/// Fast-paths the two dominant smooth-field Chemistry regimes (`1 ≤ |x| < 20`
/// and `ln(2)/2 ≤ |x| < 1`) through branchless 2-wide NEON [`exp_neg_small_4`]
/// and `copysign`, handles general `|x| < 1` via [`expm1_neg_4`], and
/// delegates mixed/saturated lanes to [`tanh_4_large`]. Returns `None` if any
/// lane is nonfinite.
#[inline]
#[must_use]
pub fn tanh_4(xs: [f64; 4]) -> Option<[f64; 4]> {
    if !xs[0].is_finite() || !xs[1].is_finite() || !xs[2].is_finite() || !xs[3].is_finite() {
        return None;
    }
    let mags = [xs[0].abs(), xs[1].abs(), xs[2].abs(), xs[3].abs()];
    let two_mags = [2.0 * mags[0], 2.0 * mags[1], 2.0 * mags[2], 2.0 * mags[3]];
    if mags[0] >= 1.0
        && mags[0] < TANH_SATURATES
        && mags[1] >= 1.0
        && mags[1] < TANH_SATURATES
        && mags[2] >= 1.0
        && mags[2] < TANH_SATURATES
        && mags[3] >= 1.0
        && mags[3] < TANH_SATURATES
    {
        let d = exp_neg_small_4(two_mags);
        return Some([
            (1.0 - 2.0 * d[0] / (1.0 + d[0])).copysign(xs[0]),
            (1.0 - 2.0 * d[1] / (1.0 + d[1])).copysign(xs[1]),
            (1.0 - 2.0 * d[2] / (1.0 + d[2])).copysign(xs[2]),
            (1.0 - 2.0 * d[3] / (1.0 + d[3])).copysign(xs[3]),
        ]);
    }
    if mags[0] < 1.0 && mags[1] < 1.0 && mags[2] < 1.0 && mags[3] < 1.0 {
        let ln2 = core::f64::consts::LN_2;
        if two_mags[0] >= ln2 && two_mags[1] >= ln2 && two_mags[2] >= ln2 && two_mags[3] >= ln2 {
            let d = exp_neg_small_4(two_mags);
            let m = [1.0 - d[0], 1.0 - d[1], 1.0 - d[2], 1.0 - d[3]];
            return Some([
                (m[0] / (2.0 - m[0])).copysign(xs[0]),
                (m[1] / (2.0 - m[1])).copysign(xs[1]),
                (m[2] / (2.0 - m[2])).copysign(xs[2]),
                (m[3] / (2.0 - m[3])).copysign(xs[3]),
            ]);
        }
        let m = expm1_neg_4(two_mags)?;
        let mut out = [0.0_f64; 4];
        for ((dst, &x), &mi) in out.iter_mut().zip(&xs).zip(&m) {
            *dst = if x == 0.0 {
                x
            } else {
                (mi / (2.0 - mi)).copysign(x)
            };
        }
        return Some(out);
    }
    tanh_4_large(xs, mags, two_mags)
}

#[inline(never)]
fn tanh_4_large(xs: [f64; 4], mags: [f64; 4], two_mags: [f64; 4]) -> Option<[f64; 4]> {
    let ln2 = core::f64::consts::LN_2;
    // Both `ln(2)/2 ≤ |x| < 1` (`m = 1 - exp_neg(2|x|); m/(2 - m)`) and
    // `1 ≤ |x| < 20` (`d = exp_neg(2|x|); 1 - 2d/(1 + d)`) evaluate the exact
    // same `exp_neg(2|x|)` with `2|x| ∈ [ln 2, 40)`; handle them together with
    // `0.0` and `>= 20` in one vectorized `exp_neg_small_4` pass.
    let exp_ok = (two_mags[0] >= ln2 || mags[0] == 0.0)
        && (two_mags[1] >= ln2 || mags[1] == 0.0)
        && (two_mags[2] >= ln2 || mags[2] == 0.0)
        && (two_mags[3] >= ln2 || mags[3] == 0.0);
    if exp_ok {
        let active = [
            mags[0] > 0.0 && mags[0] < TANH_SATURATES,
            mags[1] > 0.0 && mags[1] < TANH_SATURATES,
            mags[2] > 0.0 && mags[2] < TANH_SATURATES,
            mags[3] > 0.0 && mags[3] < TANH_SATURATES,
        ];
        if !(active[0] | active[1] | active[2] | active[3]) {
            return Some([
                if mags[0] == 0.0 {
                    xs[0]
                } else {
                    1.0_f64.copysign(xs[0])
                },
                if mags[1] == 0.0 {
                    xs[1]
                } else {
                    1.0_f64.copysign(xs[1])
                },
                if mags[2] == 0.0 {
                    xs[2]
                } else {
                    1.0_f64.copysign(xs[2])
                },
                if mags[3] == 0.0 {
                    xs[3]
                } else {
                    1.0_f64.copysign(xs[3])
                },
            ]);
        }
        let d = exp_neg_small_4([
            if active[0] { two_mags[0] } else { 1.0 },
            if active[1] { two_mags[1] } else { 1.0 },
            if active[2] { two_mags[2] } else { 1.0 },
            if active[3] { two_mags[3] } else { 1.0 },
        ]);
        let mut out = [0.0_f64; 4];
        for ((dst, (&x, &mag)), &di) in out.iter_mut().zip(xs.iter().zip(&mags)).zip(&d) {
            *dst = if mag == 0.0 {
                x
            } else {
                let v = if mag >= TANH_SATURATES {
                    1.0
                } else if mag >= 1.0 {
                    1.0 - 2.0 * di / (1.0 + di)
                } else {
                    let m = 1.0 - di;
                    m / (2.0 - m)
                };
                v.copysign(x)
            };
        }
        return Some(out);
    }
    Some([tanh(xs[0])?, tanh(xs[1])?, tanh(xs[2])?, tanh(xs[3])?])
}

/// [`tanh`] of an F32 argument, rounded once to F32: for every `f32` `z`,
/// `tanh_f32(z)` is `tanh(f64::from(z)).map(|t| t as f32)`, bit for bit.
/// This is the controllers' hidden activation (plan §6.2, C.16).
///
/// Evaluates the exact same branchless Level-10 Padé `[9/10]` / Cody–Waite
/// Estrin path with double-rounding tie guard as [`tanh_f64_as_f32`].
/// `crates/flife-core/tests/numerics.rs` checks all 2^32 bit patterns.
/// Returns `None` for a nonfinite argument.
#[inline]
#[must_use]
pub fn tanh_f32(z: f32) -> Option<f32> {
    tanh_f64_as_f32(f64::from(z))
}

/// Four-lane batch [`tanh_f32`], bit-identical to
/// `[tanh_f32(xs[0])?, tanh_f32(xs[1])?, tanh_f32(xs[2])?, tanh_f32(xs[3])?]`.
///
/// Uses [`tanh_f64_as_f32_4`]'s branchless 4-lane Padé `[9/10]` and Cody–Waite
/// Estrin paths with exact tie and subnormal fallback so every finite `f32`
/// bit pattern matches [`tanh_f32`] bit for bit. Returns `None` if any lane is
/// nonfinite.
#[inline]
#[must_use]
pub fn tanh_f32_4(xs: [f32; 4]) -> Option<[f32; 4]> {
    tanh_f64_as_f32_4([
        f64::from(xs[0]),
        f64::from(xs[1]),
        f64::from(xs[2]),
        f64::from(xs[3]),
    ])
}

/// Branchless `2^(−n)` for integer `n ∈ [0, 58]` (exact bits of `power_of_two(-(n as i32))`).
#[inline]
fn power_of_two_neg_small(nf: f64) -> f64 {
    let ni = nf as u32;
    f64::from_bits(u64::from(1023_u32.wrapping_sub(ni)) << 52)
}

/// Upper magnitude bound for the branchless Level-10 Padé `[9/10]`
/// approximation in [`tanh_f64_as_f32`] / [`tanh_f64_as_f32_4`].
/// Over `[0, 2.25]`, Padé `[9/10]` stays within `1,380` F64 ulps of
/// correctly rounded `tanh` (`< 2.2%` of the `65,536`-ulp double-rounding
/// tie margin) while handling `0.0`, tiny, small, and medium arguments in a
/// single branchless polynomial ratio with zero range reduction.
const TANH_PADE_MAX: f64 = 2.25;

/// Exact integer coefficients of the Level-10 continued-fraction Padé `[9/10]`
/// approximant `tanh(x) ≈ x * P(x²) / Q(x²)`.
const TANH_PADE_P: [f64; 5] = [654_729_075.0, 91_891_800.0, 2_837_835.0, 25_740.0, 55.0];
const TANH_PADE_Q: [f64; 6] = [
    654_729_075.0,
    310_134_825.0,
    18_918_900.0,
    315_315.0,
    1_485.0,
    1.0,
];

/// Branchless Level-10 Padé `[9/10]` approximation of `tanh(a)` for
/// `0 ≤ a ≤ 2.25` (exact `0.0` at `a = 0.0`, within `1,380` F64 ulps over
/// `[0, 2.25]`).
#[inline]
fn tanh_pade10(a: f64) -> f64 {
    let t = a * a;
    let t2 = t * t;
    let t4 = t2 * t2;
    let p = ((TANH_PADE_P[0] + TANH_PADE_P[1] * t) + (TANH_PADE_P[2] + TANH_PADE_P[3] * t) * t2)
        + TANH_PADE_P[4] * t4;
    let q = ((TANH_PADE_Q[0] + TANH_PADE_Q[1] * t) + (TANH_PADE_Q[2] + TANH_PADE_Q[3] * t) * t2)
        + (TANH_PADE_Q[4] + t) * t4;
    (a * p) / q
}

/// Minimax `[7/8]` rational coefficients (`p0 = q0 = 1.0`, exact at `0.0`) for
/// `tanh(a) ≈ a * (1 + p1 t + p2 t² + p3 t³) / (1 + q1 t + q2 t² + q3 t³ + q4 t⁴)`
/// over `[0, 2.25]` (`max error = 1,387` F64 ulps, `< 2.2%` of the `65,536`-ulp
/// F32 tie margin, with fewer operations and constants than Padé `[9/10]`).
const TANH_RAT78_P: [f64; 3] = [
    0.132_965_569_342_744_26,
    0.003_374_417_911_542_324,
    1.703_499_079_728_788e-5,
];
const TANH_RAT78_Q: [f64; 4] = [
    0.466_298_902_675_348_07,
    0.025_474_052_142_157_415,
    0.000_303_452_635_627_566_7,
    4.596_795_534_754_085e-7,
];

/// Upper magnitude bound for the zero-division odd minimax polynomial fast path
/// in [`tanh_f64_as_f32`] / [`tanh_f64_as_f32_4`]. Over `[0, 0.75]`, the
/// 8-term degree-17 Estrin polynomial `a + (a * t) * P_7(t)` stays within
/// `44,989` F64 ulps of correctly rounded `tanh` (`< 35%` of the `131,072`-ulp
/// double-rounding tie margin, `< 0.05%` tie fallback rate) with zero FP
/// divisions — eliminating divider-port stalls in sensor saturation loops.
const TANH_POLY_MAX: f64 = 0.75;

/// Relative-error minimax odd polynomial coefficients `c_0..c_7` for
/// `tanh(a) ≈ a + (a * t) * Σ_{k=0..7} c_k t^k` (`t = a²`) over `[0, 0.75]`.
const TANH_POLY17: [f64; 8] = [
    -0.333_333_333_176_135_95,
    0.133_333_311_259_047_2,
    -0.053_967_658_464_065_17,
    0.021_862_689_531_537_548,
    -0.008_822_279_102_907_811,
    0.003_449_859_491_963_311_2,
    -0.001_162_799_197_095_419_6,
    0.000_237_083_439_569_873_8,
];

/// Double-rounding tie margin in F64 ulps for [`tanh_poly17_4`] (`~2.9×` the
/// `44,989`-ulp worst-case polynomial error, `< 0.05%` tie fallback rate).
const TANH_POLY_TIE_MARGIN: u64 = 131_072;

/// Branchless zero-division degree-17 Estrin approximation of `tanh(a)` for
/// `0 ≤ a ≤ 0.75` (exact `0.0` at `a = 0.0`, within `44,989` F64 ulps).
#[inline]
fn tanh_poly17(a: f64) -> f64 {
    let t = a * a;
    let t2 = t * t;
    let t4 = t2 * t2;
    let lo = (TANH_POLY17[0] + TANH_POLY17[1] * t) + (TANH_POLY17[2] + TANH_POLY17[3] * t) * t2;
    let hi = (TANH_POLY17[4] + TANH_POLY17[5] * t) + (TANH_POLY17[6] + TANH_POLY17[7] * t) * t2;
    let p = lo + hi * t4;
    a + (a * t) * p
}

/// Four-lane lockstep zero-division degree-17 Estrin approximation of `tanh(a)`
/// for `0 ≤ a ≤ 0.75`, auto-vectorized into pure `fmul.2d` / `fadd.2d` with
/// zero `fdiv.2d` and zero FMA contraction.
#[inline(always)]
fn tanh_poly17_4(a: [f64; 4]) -> [f64; 4] {
    let mut out = [0.0_f64; 4];
    for (dst, &ai) in out.iter_mut().zip(&a) {
        let t = ai * ai;
        let t2 = t * t;
        let t4 = t2 * t2;
        let lo = (TANH_POLY17[0] + TANH_POLY17[1] * t) + (TANH_POLY17[2] + TANH_POLY17[3] * t) * t2;
        let hi = (TANH_POLY17[4] + TANH_POLY17[5] * t) + (TANH_POLY17[6] + TANH_POLY17[7] * t) * t2;
        let p = lo + hi * t4;
        *dst = ai + (ai * t) * p;
    }
    out
}

/// Four-lane lockstep minimax `[7/8]` approximation of `tanh(a)` for
/// `0 ≤ a ≤ 2.25`, auto-vectorized without FMA contraction or regime branches.
#[inline(always)]
fn tanh_rat78_4(a: [f64; 4]) -> [f64; 4] {
    let mut out = [0.0_f64; 4];
    for (dst, &ai) in out.iter_mut().zip(&a) {
        let t = ai * ai;
        let t2 = t * t;
        let p = (1.0 + TANH_RAT78_P[0] * t) + (TANH_RAT78_P[1] + TANH_RAT78_P[2] * t) * t2;
        let q = ((1.0 + TANH_RAT78_Q[0] * t) + (TANH_RAT78_Q[1] + TANH_RAT78_Q[2] * t) * t2)
            + TANH_RAT78_Q[3] * (t2 * t2);
        *dst = (ai * p) / q;
    }
    out
}

/// Out-of-line cold path for [`tanh_f64_as_f32_4`] when any lane lies in
/// `(2.25, 20)`. Keeping Cody–Waite Estrin out of line lets the common
/// `|x| ≤ 0.75` path inline into sensor encoding loops and hoist constants.
#[cold]
#[inline(never)]
fn tanh_f64_as_f32_4_large(xs: [f64; 4], mags: [f64; 4], any_sat: bool) -> Option<[f32; 4]> {
    let s = [2.0 * mags[0], 2.0 * mags[1], 2.0 * mags[2], 2.0 * mags[3]];
    let s_safe = [
        if mags[0] > TANH_PADE_MAX && mags[0] < TANH_SATURATES {
            s[0]
        } else {
            0.0
        },
        if mags[1] > TANH_PADE_MAX && mags[1] < TANH_SATURATES {
            s[1]
        } else {
            0.0
        },
        if mags[2] > TANH_PADE_MAX && mags[2] < TANH_SATURATES {
            s[2]
        } else {
            0.0
        },
        if mags[3] > TANH_PADE_MAX && mags[3] < TANH_SATURATES {
            s[3]
        } else {
            0.0
        },
    ];
    let n = [
        (s_safe[0] * core::f64::consts::LOG2_E).round(),
        (s_safe[1] * core::f64::consts::LOG2_E).round(),
        (s_safe[2] * core::f64::consts::LOG2_E).round(),
        (s_safe[3] * core::f64::consts::LOG2_E).round(),
    ];
    let neg_r = [
        -((s_safe[0] - n[0] * LN2_HI) - n[0] * LN2_LO),
        -((s_safe[1] - n[1] * LN2_HI) - n[1] * LN2_LO),
        -((s_safe[2] - n[2] * LN2_HI) - n[2] * LN2_LO),
        -((s_safe[3] - n[3] * LN2_HI) - n[3] * LN2_LO),
    ];
    let poly = estrin12_4(neg_r, &EXP_COEFFICIENTS);
    let d = [
        poly[0] * power_of_two_neg_small(n[0]),
        poly[1] * power_of_two_neg_small(n[1]),
        poly[2] * power_of_two_neg_small(n[2]),
        poly[3] * power_of_two_neg_small(n[3]),
    ];
    let mut approx = [
        (1.0 - d[0]) / (1.0 + d[0]),
        (1.0 - d[1]) / (1.0 + d[1]),
        (1.0 - d[2]) / (1.0 + d[2]),
        (1.0 - d[3]) / (1.0 + d[3]),
    ];
    for (dst, &mag) in approx.iter_mut().zip(&mags) {
        if mag > 0.0 && mag <= TANH_PADE_MAX {
            *dst = tanh_pade10(mag);
        }
    }
    const TIE: u64 = 1 << 28;
    const LOW_MASK: u64 = (1 << 29) - 1;
    const MARGIN: u64 = 65_536;
    let min_normal = 2.0 * f64::from(f32::MIN_POSITIVE);
    if !any_sat
        && ((approx[0] == 0.0 || approx[0] >= min_normal)
            & (approx[1] == 0.0 || approx[1] >= min_normal)
            & (approx[2] == 0.0 || approx[2] >= min_normal)
            & (approx[3] == 0.0 || approx[3] >= min_normal)
            & ((approx[0].to_bits() & LOW_MASK).abs_diff(TIE) > MARGIN)
            & ((approx[1].to_bits() & LOW_MASK).abs_diff(TIE) > MARGIN)
            & ((approx[2].to_bits() & LOW_MASK).abs_diff(TIE) > MARGIN)
            & ((approx[3].to_bits() & LOW_MASK).abs_diff(TIE) > MARGIN))
    {
        return Some([
            approx[0].copysign(xs[0]) as f32,
            approx[1].copysign(xs[1]) as f32,
            approx[2].copysign(xs[2]) as f32,
            approx[3].copysign(xs[3]) as f32,
        ]);
    }
    finalize_tanh_f64_as_f32_4_slow(
        xs[0], xs[1], xs[2], xs[3], approx[0], approx[1], approx[2], approx[3],
    )
}

/// [`tanh`] of an F64 argument, rounded once to F32: for every `f64` `x`,
/// `tanh_f64_as_f32(x)` is `tanh(x).map(|t| t as f32)`, bit for bit.
///
/// Evaluates a zero-division degree-17 Estrin odd polynomial for `|x| ≤ 0.75`
/// (error ≤ 44,989 F64 ulps), Padé `[9/10]` for `0.75 < |x| ≤ 2.25` (error
/// ≤ 1,380 F64 ulps), and Cody–Waite Estrin for `2.25 < |x| < 20`, guarded by
/// double-rounding tie margins around `0x1000_0000`. Near a tie or in the F32
/// subnormal range (< 0.05% of inputs) it falls back to [`tanh`].
#[inline]
#[must_use]
pub fn tanh_f64_as_f32(x: f64) -> Option<f32> {
    if !x.is_finite() {
        return None;
    }
    if x == 0.0 {
        return Some(x as f32);
    }
    let magnitude = x.abs();
    if magnitude >= TANH_SATURATES {
        return Some(if x.is_sign_negative() { -1.0 } else { 1.0 });
    }
    let (approx, margin) = if magnitude <= TANH_POLY_MAX {
        (tanh_poly17(magnitude), TANH_POLY_TIE_MARGIN)
    } else if magnitude <= TANH_PADE_MAX {
        (tanh_pade10(magnitude), 65_536)
    } else {
        return tanh_f64_as_f32_large_or_slow(x, magnitude);
    };
    const TIE: u64 = 1 << 28;
    const LOW_MASK: u64 = (1 << 29) - 1;
    if approx >= 2.0 * f64::from(f32::MIN_POSITIVE) {
        let low = approx.to_bits() & LOW_MASK;
        if low.abs_diff(TIE) > margin {
            let value = approx as f32;
            return Some(if x.is_sign_negative() { -value } else { value });
        }
    }
    tanh_f64_as_f32_fallback(x)
}

#[cold]
#[inline(never)]
fn tanh_f64_as_f32_large_or_slow(x: f64, magnitude: f64) -> Option<f32> {
    let approx = tanh_estrin_large(magnitude);
    const TIE: u64 = 1 << 28;
    const LOW_MASK: u64 = (1 << 29) - 1;
    const MARGIN: u64 = 65_536;
    if approx >= 2.0 * f64::from(f32::MIN_POSITIVE) {
        let low = approx.to_bits() & LOW_MASK;
        if low.abs_diff(TIE) > MARGIN {
            let value = approx as f32;
            return Some(if x.is_sign_negative() { -value } else { value });
        }
    }
    tanh(x).map(|value| value as f32)
}

#[cold]
#[inline(never)]
fn tanh_f64_as_f32_fallback(x: f64) -> Option<f32> {
    tanh(x).map(|value| value as f32)
}

/// Four-lane batch [`tanh_f64_as_f32`], bit-identical to
/// `[tanh_f64_as_f32(xs[0])?, tanh_f64_as_f32(xs[1])?, tanh_f64_as_f32(xs[2])?, tanh_f64_as_f32(xs[3])?]`.
///
/// Uses an inlined branchless zero-division 4-lane degree-17 Estrin odd
/// polynomial covering every lane in `[0, 0.75]` (`max error ≤ 44,989` F64
/// ulps, `< 35%` of the `131,072`-ulp tie margin) and delegates any lane
/// outside `[-0.75, 0.75]` to an out-of-line scalar-register helper running
/// `[7/8]` rational (`≤ 2.25`) and Cody–Waite Estrin (`< 20`). Passing scalar
/// `f64`s (not `[f64; 4]`) to out-of-line helpers keeps all arguments in
/// `d0..d7` on AArch64 so the hot loop emits zero stack spills. Every lane
/// passes the exact double-rounding tie guard and falls back to scalar
/// [`tanh`] on ties or F32 subnormals. Returns `None` if any lane is nonfinite.
#[inline(always)]
#[must_use]
pub fn tanh_f64_as_f32_4(xs: [f64; 4]) -> Option<[f32; 4]> {
    let mags = [xs[0].abs(), xs[1].abs(), xs[2].abs(), xs[3].abs()];
    // IEEE-754 `<= TANH_POLY_MAX` is false for NaN, ±Inf, and > 0.75, keeping
    // the zero-division hot path to a single branch.
    if !((mags[0] <= TANH_POLY_MAX)
        & (mags[1] <= TANH_POLY_MAX)
        & (mags[2] <= TANH_POLY_MAX)
        & (mags[3] <= TANH_POLY_MAX))
    {
        return tanh_f64_as_f32_4_mid_or_large(xs[0], xs[1], xs[2], xs[3]);
    }
    let approx = tanh_poly17_4(mags);

    const TIE: u64 = 1 << 28;
    const LOW_MASK: u64 = (1 << 29) - 1;
    const MARGIN: u64 = TANH_POLY_TIE_MARGIN;
    const MIN_NORMAL_BITS: u64 = (2.0 * (f32::MIN_POSITIVE as f64)).to_bits();
    let b0 = approx[0].to_bits();
    let b1 = approx[1].to_bits();
    let b2 = approx[2].to_bits();
    let b3 = approx[3].to_bits();
    if (b0.wrapping_sub(1) >= MIN_NORMAL_BITS - 1)
        & (b1.wrapping_sub(1) >= MIN_NORMAL_BITS - 1)
        & (b2.wrapping_sub(1) >= MIN_NORMAL_BITS - 1)
        & (b3.wrapping_sub(1) >= MIN_NORMAL_BITS - 1)
        & ((b0 & LOW_MASK).wrapping_sub(TIE - MARGIN) > 2 * MARGIN)
        & ((b1 & LOW_MASK).wrapping_sub(TIE - MARGIN) > 2 * MARGIN)
        & ((b2 & LOW_MASK).wrapping_sub(TIE - MARGIN) > 2 * MARGIN)
        & ((b3 & LOW_MASK).wrapping_sub(TIE - MARGIN) > 2 * MARGIN)
    {
        return Some([
            approx[0].copysign(xs[0]) as f32,
            approx[1].copysign(xs[1]) as f32,
            approx[2].copysign(xs[2]) as f32,
            approx[3].copysign(xs[3]) as f32,
        ]);
    }
    finalize_tanh_f64_as_f32_4_slow(
        xs[0], xs[1], xs[2], xs[3], approx[0], approx[1], approx[2], approx[3],
    )
}

#[inline(never)]
fn tanh_f64_as_f32_4_mid_or_large(x0: f64, x1: f64, x2: f64, x3: f64) -> Option<[f32; 4]> {
    let xs = [x0, x1, x2, x3];
    let mags = [x0.abs(), x1.abs(), x2.abs(), x3.abs()];
    if (mags[0] <= TANH_PADE_MAX)
        & (mags[1] <= TANH_PADE_MAX)
        & (mags[2] <= TANH_PADE_MAX)
        & (mags[3] <= TANH_PADE_MAX)
    {
        let approx = tanh_rat78_4(mags);
        const TIE: u64 = 1 << 28;
        const LOW_MASK: u64 = (1 << 29) - 1;
        const MARGIN: u64 = 65_536;
        const MIN_NORMAL_BITS: u64 = (2.0 * (f32::MIN_POSITIVE as f64)).to_bits();
        let b0 = approx[0].to_bits();
        let b1 = approx[1].to_bits();
        let b2 = approx[2].to_bits();
        let b3 = approx[3].to_bits();
        if (b0.wrapping_sub(1) >= MIN_NORMAL_BITS - 1)
            & (b1.wrapping_sub(1) >= MIN_NORMAL_BITS - 1)
            & (b2.wrapping_sub(1) >= MIN_NORMAL_BITS - 1)
            & (b3.wrapping_sub(1) >= MIN_NORMAL_BITS - 1)
            & ((b0 & LOW_MASK).wrapping_sub(TIE - MARGIN) > 2 * MARGIN)
            & ((b1 & LOW_MASK).wrapping_sub(TIE - MARGIN) > 2 * MARGIN)
            & ((b2 & LOW_MASK).wrapping_sub(TIE - MARGIN) > 2 * MARGIN)
            & ((b3 & LOW_MASK).wrapping_sub(TIE - MARGIN) > 2 * MARGIN)
        {
            return Some([
                approx[0].copysign(x0) as f32,
                approx[1].copysign(x1) as f32,
                approx[2].copysign(x2) as f32,
                approx[3].copysign(x3) as f32,
            ]);
        }
        return finalize_tanh_f64_as_f32_4_slow(
            x0, x1, x2, x3, approx[0], approx[1], approx[2], approx[3],
        );
    }
    tanh_f64_as_f32_4_outside_rat78(xs, mags)
}

#[cold]
#[inline(never)]
fn tanh_f64_as_f32_4_outside_rat78(xs: [f64; 4], mags: [f64; 4]) -> Option<[f32; 4]> {
    let all_finite = (mags[0] < f64::INFINITY)
        & (mags[1] < f64::INFINITY)
        & (mags[2] < f64::INFINITY)
        & (mags[3] < f64::INFINITY);
    if !all_finite {
        return None;
    }
    let any_large = (mags[0] > TANH_PADE_MAX && mags[0] < TANH_SATURATES)
        | (mags[1] > TANH_PADE_MAX && mags[1] < TANH_SATURATES)
        | (mags[2] > TANH_PADE_MAX && mags[2] < TANH_SATURATES)
        | (mags[3] > TANH_PADE_MAX && mags[3] < TANH_SATURATES);
    if any_large {
        let any_sat = (mags[0] >= TANH_SATURATES)
            | (mags[1] >= TANH_SATURATES)
            | (mags[2] >= TANH_SATURATES)
            | (mags[3] >= TANH_SATURATES);
        return tanh_f64_as_f32_4_large(xs, mags, any_sat);
    }
    let mut out = [0.0_f32; 4];
    for ((dst, &x), &mag) in out.iter_mut().zip(&xs).zip(&mags) {
        *dst = if mag >= TANH_SATURATES {
            if x.is_sign_negative() { -1.0 } else { 1.0 }
        } else {
            tanh_f64_as_f32(x)?
        };
    }
    Some(out)
}

#[cold]
#[inline(never)]
#[expect(
    clippy::too_many_arguments,
    reason = "8 scalar f64s pass in AArch64 d0..d7 with zero stack spills in caller hot loops"
)]
fn finalize_tanh_f64_as_f32_4_slow(
    x0: f64,
    x1: f64,
    x2: f64,
    x3: f64,
    a0: f64,
    a1: f64,
    a2: f64,
    a3: f64,
) -> Option<[f32; 4]> {
    let xs = [x0, x1, x2, x3];
    let approx = [a0, a1, a2, a3];
    const TIE: u64 = 1 << 28;
    const LOW_MASK: u64 = (1 << 29) - 1;
    let min_normal = 2.0 * f64::from(f32::MIN_POSITIVE);
    let mut out = [0.0_f32; 4];
    for ((dst, &x), &ap) in out.iter_mut().zip(&xs).zip(&approx) {
        let mag = x.abs();
        let margin = if mag <= TANH_POLY_MAX {
            TANH_POLY_TIE_MARGIN
        } else {
            65_536
        };
        *dst = if x == 0.0 {
            x as f32
        } else if mag >= TANH_SATURATES {
            if x.is_sign_negative() { -1.0 } else { 1.0 }
        } else if ap >= min_normal && (ap.to_bits() & LOW_MASK).abs_diff(TIE) > margin {
            let value = ap as f32;
            if x.is_sign_negative() { -value } else { value }
        } else {
            tanh(x)? as f32
        };
    }
    Some(out)
}

/// Four-lane lockstep [`estrin12`], bit-identical per lane.
#[inline]
fn estrin12_4(u: [f64; 4], c: &[f64; 12]) -> [f64; 4] {
    let [c0, c1, c2, c3, c4, c5, c6, c7, c8, c9, c10, c11] = *c;
    let mut out = [0.0; 4];
    for (dst, &ui) in out.iter_mut().zip(&u) {
        let u2 = ui * ui;
        let u4 = u2 * u2;
        let u8 = u4 * u4;
        let low = (c0 + c1 * ui) + (c2 + c3 * ui) * u2;
        let middle = (c4 + c5 * ui) + (c6 + c7 * ui) * u2;
        let high = (c8 + c9 * ui) + (c10 + c11 * ui) * u2;
        *dst = (low + middle * u4) + high * u8;
    }
    out
}

/// `1/k!` for `k = 0..=11`, each a correctly rounded quotient: Taylor's
/// coefficients of `e^u`.
const EXP_COEFFICIENTS: [f64; 12] = [
    1.0,
    1.0,
    1.0 / 2.0,
    1.0 / 6.0,
    1.0 / 24.0,
    1.0 / 120.0,
    1.0 / 720.0,
    1.0 / 5_040.0,
    1.0 / 40_320.0,
    1.0 / 362_880.0,
    1.0 / 3_628_800.0,
    1.0 / 39_916_800.0,
];

/// `tanh(a)` for `2.25 < a < 20` via Cody–Waite range reduction and Estrin
/// degree-11 `exp(-r)`, using branchless `2^(-n)`.
#[inline]
fn tanh_estrin_large(a: f64) -> f64 {
    let s = 2.0 * a;
    let n = (s * core::f64::consts::LOG2_E).round();
    let r = (s - n * LN2_HI) - n * LN2_LO;
    let d = estrin12(-r, &EXP_COEFFICIENTS) * power_of_two_neg_small(n);
    (1.0 - d) / (1.0 + d)
}

/// `Σ_{k<12} c_k u^k` by Estrin's scheme.
#[inline]
fn estrin12(u: f64, c: &[f64; 12]) -> f64 {
    let [c0, c1, c2, c3, c4, c5, c6, c7, c8, c9, c10, c11] = *c;
    let u2 = u * u;
    let u4 = u2 * u2;
    let u8 = u4 * u4;
    let low = (c0 + c1 * u) + (c2 + c3 * u) * u2;
    let middle = (c4 + c5 * u) + (c6 + c7 * u) * u2;
    let high = (c8 + c9 * u) + (c10 + c11 * u) * u2;
    (low + middle * u4) + high * u8
}

/// The logistic `σ(x) = 1/(1 + e^(−x))` for finite `x`, with a sign-stable
/// branch (plan A.1): `e^(−|x|)` never overflows, and a large negative
/// logit decays smoothly to zero instead of dividing by infinity. Within
/// 2 ulp of the correctly rounded result (measured). Returns `None` for a
/// nonfinite argument.
#[inline]
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

/// Four-lane batch [`sigmoid`], bit-identical to
/// `[sigmoid(xs[0])?, sigmoid(xs[1])?, sigmoid(xs[2])?, sigmoid(xs[3])?]`.
#[inline]
#[must_use]
pub fn sigmoid_4(xs: [f64; 4]) -> Option<[f64; 4]> {
    if !xs[0].is_finite() || !xs[1].is_finite() || !xs[2].is_finite() || !xs[3].is_finite() {
        return None;
    }
    let decay = exp_neg_4([xs[0].abs(), xs[1].abs(), xs[2].abs(), xs[3].abs()])?;
    Some([
        if xs[0] >= 0.0 {
            1.0 / (1.0 + decay[0])
        } else {
            decay[0] / (1.0 + decay[0])
        },
        if xs[1] >= 0.0 {
            1.0 / (1.0 + decay[1])
        } else {
            decay[1] / (1.0 + decay[1])
        },
        if xs[2] >= 0.0 {
            1.0 / (1.0 + decay[2])
        } else {
            decay[2] / (1.0 + decay[2])
        },
        if xs[3] >= 0.0 {
            1.0 / (1.0 + decay[3])
        } else {
            decay[3] / (1.0 + decay[3])
        },
    ])
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
        return Some(exp_series_12(-x));
    }
    // `x < 710`, so `n ≤ 1024`; the reduction is exact as in `exp_neg`.
    let n = (x / core::f64::consts::LN_2).round();
    let r = (x - n * LN2_HI) - n * LN2_LO;
    let scaled = exp_series_15(-r);
    let n = n as i32;
    let first = n.min(1000);
    let value = scaled * power_of_two(first) * power_of_two(n - first);
    value.is_finite().then_some(value)
}

/// `base^exponent` for finite `base > 0` and finite `exponent`, computed
/// identically on every IEEE-754 host (plan §5.6, A.1).
///
/// Handles the exact identities `x^0 = 1`, `x^1 = x` and `1^a = 1` without
/// rounding, and otherwise evaluates [`exp`]`(exponent * `[`ln`]`(base))` using
/// only host-independent operations. Returns `None` for a nonpositive or
/// nonfinite base, a nonfinite exponent, or an overflowing result.
#[must_use]
pub fn pow_positive(base: f64, exponent: f64) -> Option<f64> {
    if !base.is_finite() || base <= 0.0 || !exponent.is_finite() {
        return None;
    }
    if exponent == 0.0 || base.to_bits() == 1.0_f64.to_bits() {
        return Some(1.0);
    }
    if exponent.to_bits() == 1.0_f64.to_bits() {
        return Some(base);
    }
    let scaled = exponent * ln(base)?;
    exp(scaled)
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

/// Wrap any finite turn angle `u` into `[0.0, 1.0)` for [`sin_cos_turns`]
/// (`V2-01` Trap D guard).
///
/// Eliminates both negative-angle `None` refusals and the IEEE-754 edge case
/// `(-1e-17_f64).rem_euclid(1.0) == 1.0` by folding `rem >= 1.0` to `0.0`.
/// Returns `None` for nonfinite `u`.
#[must_use]
pub fn normalize_turns(u: f64) -> Option<f64> {
    if !u.is_finite() {
        return None;
    }
    let rem = u.rem_euclid(1.0);
    Some(if rem >= 1.0 || rem == 0.0 { 0.0 } else { rem })
}

/// Shrink `v` so its exact left-fold squared norm
/// `v.iter().fold(0.0, |t, &c| t + c * c)` satisfies `<= limit * limit`
/// (`V2-01` Trap D guard).
///
/// Leaves `v` bit-for-bit untouched when it is already within the bound.
/// Otherwise scales radially if far outside and multiplies by
/// `1.0 - f64::EPSILON / 2.0` in a bounded loop (`<= 64` steps) until the
/// exact left-fold sum passes.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for a nonfinite or nonpositive `limit`,
/// a nonfinite component, overflowing norm, or a vector that cannot be
/// brought within `limit` in 64 steps.
pub fn shrink_to_left_fold_norm_limit<const N: usize>(
    v: &mut [f64; N],
    limit: f64,
) -> Result<(), CoreError> {
    if !limit.is_finite() || limit <= 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "norm limit",
            detail: format!("{limit} is not a positive finite norm limit"),
        });
    }
    if let Some((index, &value)) = v.iter().enumerate().find(|(_, value)| !value.is_finite()) {
        return Err(CoreError::NumericalInvalid {
            quantity: "vector component",
            detail: format!("component {index} is {value}"),
        });
    }
    let limit_sq = limit * limit;
    if !limit_sq.is_finite() || limit_sq == 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "norm limit",
            detail: format!("{limit}² is not a positive finite squared norm limit"),
        });
    }
    let left_fold_sq = |values: &[f64; N]| {
        values
            .iter()
            .fold(0.0_f64, |total, &component| total + component * component)
    };
    let initial_sq = left_fold_sq(v);
    if !initial_sq.is_finite() {
        return Err(CoreError::NumericalInvalid {
            quantity: "vector squared norm",
            detail: "squared norm overflows".into(),
        });
    }
    if initial_sq <= limit_sq {
        return Ok(());
    }
    let norm = initial_sq.sqrt();
    if norm > limit * (1.0 + 16.0 * f64::EPSILON) {
        let scale = limit / norm;
        for component in v.iter_mut() {
            *component *= scale;
        }
    }
    let shrink = 1.0 - f64::EPSILON / 2.0;
    for _ in 0..64 {
        if left_fold_sq(v) <= limit_sq {
            return Ok(());
        }
        for component in v.iter_mut() {
            *component *= shrink;
        }
    }
    Err(CoreError::NumericalInvalid {
        quantity: "vector norm",
        detail: format!("could not bring left-fold squared norm within {limit}²"),
    })
}

/// Apply the canonical 2D-block symplectic rotation `J` over
/// `0..active_channels` of a 32-channel chemical direction (`V2-01` Trap D):
/// `(J a)_{2k} = -a_{2k+1}`, `(J a)_{2k+1} = a_{2k}` for `2k + 1 < active_channels`,
/// zeroing channels `active_channels..32` (serving both 32-channel single-carrier
/// `V2-11`/`V2-12` and 24-channel Carrier-0 `V2-15`), then enforce the exact
/// left-fold norm bound `limit` via [`shrink_to_left_fold_norm_limit`].
///
/// # Errors
/// [`CoreError::NumericalInvalid`] if `active_channels > 32`, `a` holds a
/// nonfinite component, or [`shrink_to_left_fold_norm_limit`] refuses the result.
pub fn symplectic_rotate_channels(
    a: &[f64; 32],
    active_channels: usize,
    limit: f64,
) -> Result<[f64; 32], CoreError> {
    if active_channels > 32 {
        return Err(CoreError::NumericalInvalid {
            quantity: "active chemical channels",
            detail: format!("{active_channels} exceeds 32"),
        });
    }
    if let Some((index, &value)) = a.iter().enumerate().find(|(_, value)| !value.is_finite()) {
        return Err(CoreError::NumericalInvalid {
            quantity: "chemical direction",
            detail: format!("component {index} is {value}"),
        });
    }
    let mut rotated = [0.0_f64; 32];
    let pairs = active_channels / 2;
    for pair in 0..pairs {
        let even = 2 * pair;
        let odd = even + 1;
        rotated[even] = -a[odd];
        rotated[odd] = a[even];
    }
    shrink_to_left_fold_norm_limit(&mut rotated, limit)?;
    Ok(rotated)
}

#[cfg(test)]
mod batch_tests {
    use super::*;

    #[test]
    fn exp_neg_4_matches_scalar_exp_neg_bit_for_bit() {
        let mut samples = vec![
            0.0,
            -0.0,
            f64::MIN_POSITIVE,
            1e-12,
            7.8125e-5,
            1.0 / 32.0,
            REDUCED.next_down(),
            REDUCED,
            REDUCED.next_up(),
            0.1,
            core::f64::consts::LN_2,
            1.0,
            2.5,
            8.0,
            20.0,
            100.0,
            693.1471805599453,
            709.78,
            745.0,
            EXP_UNDERFLOWS.next_down(),
            EXP_UNDERFLOWS,
            EXP_UNDERFLOWS.next_up(),
            1000.0,
            f64::MAX,
        ];
        // Dense log/linear sweeps across small (<= REDUCED) and Cody-Waite ranges.
        for k in 1..=2000_u32 {
            samples.push(f64::from(k) * (REDUCED / 2000.0));
            samples.push(REDUCED + f64::from(k) * (20.0 / 2000.0));
            samples.push(20.0 + f64::from(k) * (730.0 / 2000.0));
        }
        // Homogeneous 4-tuples (same regime) and strided mixed 4-tuples.
        for &xs in samples.as_chunks::<4>().0 {
            let got = exp_neg_4(xs).expect("valid exp_neg_4");
            for (lane, &x) in xs.iter().enumerate() {
                let expected = exp_neg(x).expect("valid scalar exp_neg");
                assert_eq!(
                    got[lane].to_bits(),
                    expected.to_bits(),
                    "exp_neg_4 mismatch at lane {lane} for x={x:e}"
                );
            }
        }
        // Explicitly test homogeneous small, homogeneous Cody-Waite, and mixed tuples.
        let special = [
            [1e-6, 1e-4, 0.01, REDUCED],
            [0.07, 0.5, 5.0, 740.0],
            [0.0, 1e-5, 1.5, 800.0],
            [-0.0, REDUCED, REDUCED.next_up(), EXP_UNDERFLOWS],
        ];
        for xs in special {
            let got = exp_neg_4(xs).expect("special exp_neg_4");
            for (lane, &x) in xs.iter().enumerate() {
                assert_eq!(
                    got[lane].to_bits(),
                    exp_neg(x).expect("scalar").to_bits(),
                    "special exp_neg_4 mismatch for {xs:?} lane {lane}"
                );
            }
        }
        // Refusals: negative or nonfinite in any lane.
        assert_eq!(exp_neg_4([-1e-12, 0.5, 1.0, 2.0]), None);
        assert_eq!(exp_neg_4([0.5, f64::NAN, 1.0, 2.0]), None);
        assert_eq!(exp_neg_4([0.5, 1.0, f64::INFINITY, 2.0]), None);
        assert_eq!(exp_neg_4([0.5, 1.0, 2.0, f64::NEG_INFINITY]), None);
    }

    #[test]
    fn tanh_f64_as_f32_4_matches_scalar_bit_for_bit() {
        let cutoff = 0.5 * REDUCED;
        let mut samples = vec![
            0.0,
            -0.0,
            f64::MIN_POSITIVE,
            -f64::MIN_POSITIVE,
            f64::from(f32::MIN_POSITIVE),
            -f64::from(f32::MIN_POSITIVE),
            1e-9,
            -1e-9,
            cutoff.next_down(),
            cutoff,
            cutoff.next_up(),
            -cutoff.next_down(),
            -cutoff,
            -cutoff.next_up(),
            0.5,
            -0.5,
            1.0,
            -1.0,
            TANH_SATURATES.next_down(),
            TANH_SATURATES,
            TANH_SATURATES.next_up(),
            -TANH_SATURATES.next_down(),
            -TANH_SATURATES,
            -TANH_SATURATES.next_up(),
            1e6,
            -1e6,
            f64::MAX,
            f64::MIN,
        ];
        for k in 1..=2500_u32 {
            let small = f64::from(k) * (cutoff / 2500.0);
            let mid = cutoff + f64::from(k) * ((TANH_SATURATES - cutoff) / 2500.0);
            samples.push(small);
            samples.push(-small);
            samples.push(mid);
            samples.push(-mid);
        }
        // Deterministic pseudo-random F64 values to hit near-tie rounding margins too.
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        for _ in 0..4000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let u = (state as f64) / (u64::MAX as f64);
            samples.push((u * 2.0 - 1.0) * 22.0);
        }

        for &xs in samples.as_chunks::<4>().0 {
            let got = tanh_f64_as_f32_4(xs).expect("valid tanh_f64_as_f32_4");
            for (lane, &x) in xs.iter().enumerate() {
                let expected = tanh_f64_as_f32(x).expect("valid scalar tanh_f64_as_f32");
                assert_eq!(
                    got[lane].to_bits(),
                    expected.to_bits(),
                    "tanh_f64_as_f32_4 mismatch at lane {lane} for x={x:e}"
                );
            }
        }

        // Explicit regime combos: all-small, all-mid + zero + saturated, mixed small+mid.
        let special = [
            [0.0, -0.0, 1e-4, -0.02],
            [0.0, -0.5, 25.0, -0.0],
            [0.2, -3.5, 19.999, -100.0],
            [1e-4, 0.5, -1e-4, -0.5],
            [1e-40, -1e-40, 1e-38, -1e-38],
        ];
        for xs in special {
            let got = tanh_f64_as_f32_4(xs).expect("special tanh_f64_as_f32_4");
            for (lane, &x) in xs.iter().enumerate() {
                let expected = tanh_f64_as_f32(x).expect("scalar");
                assert_eq!(
                    got[lane].to_bits(),
                    expected.to_bits(),
                    "special tanh_f64_as_f32_4 mismatch for {xs:?} lane {lane}"
                );
            }
        }

        // Nonfinite refusal in any lane.
        assert_eq!(tanh_f64_as_f32_4([f64::NAN, 0.0, 1.0, -1.0]), None);
        assert_eq!(tanh_f64_as_f32_4([0.0, f64::INFINITY, 1.0, -1.0]), None);
        assert_eq!(tanh_f64_as_f32_4([0.0, 1.0, f64::NEG_INFINITY, -1.0]), None);
        assert_eq!(tanh_f64_as_f32_4([0.0, 1.0, -1.0, f64::NAN]), None);
    }

    #[test]
    fn tanh_f32_4_matches_scalar_tanh_f32_bit_for_bit() {
        let cutoff = (0.5 * REDUCED) as f32;
        let sat = TANH_SATURATES as f32;
        let smallest_sub = f32::from_bits(1);
        let largest_sub = f32::from_bits(0x007f_ffff);
        let mut samples = vec![
            0.0_f32,
            -0.0_f32,
            smallest_sub,
            -smallest_sub,
            f32::from_bits(42),
            -f32::from_bits(42),
            largest_sub,
            -largest_sub,
            f32::MIN_POSITIVE,
            -f32::MIN_POSITIVE,
            1e-9_f32,
            -1e-9_f32,
            cutoff.next_down(),
            cutoff,
            cutoff.next_up(),
            -cutoff.next_down(),
            -cutoff,
            -cutoff.next_up(),
            0.25_f32,
            -0.25_f32,
            0.5_f32,
            -0.5_f32,
            1.0_f32,
            -1.0_f32,
            sat.next_down(),
            sat,
            sat.next_up(),
            -sat.next_down(),
            -sat,
            -sat.next_up(),
            1e6_f32,
            -1e6_f32,
            f32::MAX,
            f32::MIN,
        ];
        // Dense sweeps across small `(0, 1/32)`, mid `[1/32, 20)`, and saturated `[20, 40]`.
        for k in 1..=3000_u32 {
            let small = (f64::from(k) * (f64::from(cutoff) / 3000.0)) as f32;
            let mid =
                (f64::from(cutoff) + f64::from(k) * ((20.0 - f64::from(cutoff)) / 3000.0)) as f32;
            samples.push(small);
            samples.push(-small);
            samples.push(mid);
            samples.push(-mid);
        }
        // Dense ULP walk across the `1/32` branch boundary and `20.0` saturation boundary.
        let mut u = cutoff;
        for _ in 0..128 {
            samples.push(u);
            samples.push(-u);
            u = u.next_down();
        }
        let mut u = cutoff;
        for _ in 0..128 {
            samples.push(u);
            samples.push(-u);
            u = u.next_up();
        }
        let mut u = sat;
        for _ in 0..64 {
            samples.push(u);
            samples.push(-u);
            u = u.next_down();
        }
        // Deterministic pseudo-random F32 values spanning subnormals through saturation.
        let mut state: u64 = 0xd1b5_4a32_d192_ed03;
        for _ in 0..6000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let raw = (state as u32) & 0x7fff_ffff;
            let z = f32::from_bits(raw);
            if z.is_finite() {
                samples.push(z);
                samples.push(-z);
            }
        }

        for &xs in samples.as_chunks::<4>().0 {
            let got = tanh_f32_4(xs).expect("valid tanh_f32_4");
            for (lane, &z) in xs.iter().enumerate() {
                let expected = tanh_f32(z).expect("valid scalar tanh_f32");
                assert_eq!(
                    got[lane].to_bits(),
                    expected.to_bits(),
                    "tanh_f32_4 mismatch at lane {lane} for z={z:e} (bits=0x{:08x})",
                    z.to_bits()
                );
            }
        }

        // Explicit regime combos: all zero/saturated, all small, all mid, and every mixed pattern.
        let special = [
            [0.0_f32, -0.0, 20.0, -25.0],
            [0.0, -0.0, smallest_sub, -largest_sub],
            [1e-6, -1e-4, 0.01, -cutoff.next_down()],
            [cutoff, -0.5, 3.5, -19.999],
            [1e-4, 0.5, -1e-4, -0.5],
            [0.5, 1e-4, -0.0, -25.0],
            [-0.0, 25.0, 1e-5, -0.75],
            [0.75, -0.75, 1.5, smallest_sub],
        ];
        for xs in special {
            let got = tanh_f32_4(xs).expect("special tanh_f32_4");
            for (lane, &z) in xs.iter().enumerate() {
                let expected = tanh_f32(z).expect("scalar tanh_f32");
                assert_eq!(
                    got[lane].to_bits(),
                    expected.to_bits(),
                    "special tanh_f32_4 mismatch for {xs:?} lane {lane}"
                );
            }
        }

        // Nonfinite refusal in every lane position (matching tanh_f32 -> None).
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(tanh_f32_4([bad, 0.0, 1.0, -1.0]), None);
            assert_eq!(tanh_f32_4([0.0, bad, 1.0, -1.0]), None);
            assert_eq!(tanh_f32_4([0.0, 1.0, bad, -1.0]), None);
            assert_eq!(tanh_f32_4([0.0, 1.0, -1.0, bad]), None);
        }
    }
}
