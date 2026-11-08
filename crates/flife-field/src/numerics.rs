//! Host-independent elementary functions for the field's reference profile.
//!
//! The decay factor `exp(-λΔt)` and the Gaussian source weights must not
//! depend on the platform's `libm`: two hosts that disagree in the last bit
//! of `exp` would diverge. These functions use only IEEE-754 addition,
//! multiplication and division in a fixed order, and no fused
//! multiply-add, so every conforming host computes the same bits.

/// Arguments are halved until they are at most this, then the series runs.
const REDUCED: f64 = 1.0 / 16.0;

/// Taylor terms after the constant. For `y ≤ 1/16` the first omitted term
/// is below `(1/16)^13 / 13! ≈ 5e-26`, far under an F64 ulp of the result.
const TERMS: u32 = 12;

/// `exp(-x)` for finite `x ≥ 0`, computed identically on every IEEE-754 host.
///
/// The argument is reduced exactly to `y = x / 2^k ≤ 1/16`. The series
/// `Σ (−y)^i / i!` is then evaluated by Horner's rule from the highest term,
/// and the result is squared `k` times. Each squaring can double the
/// relative error, so the error grows with `k`. Measured against the
/// correctly rounded value: 0 ulp at Appendix A's decay factor
/// (`x = 7.8125e-5`, `k = 0`), 1 ulp at `x = 0.1`, and 36 ulp at a source
/// kernel's cutoff (`x = 8`, `k = 7`). Kernel weights are normalized, so
/// this costs no energy. Returns `None` for a negative or nonfinite
/// argument. Underflows to `0.0` for large `x`, as `exp` does.
#[must_use]
pub fn exp_neg(x: f64) -> Option<f64> {
    if !x.is_finite() || x < 0.0 {
        return None;
    }
    let mut y = x;
    let mut squarings = 0_u32;
    while y > REDUCED {
        y *= 0.5;
        squarings += 1;
    }
    let mut series = 1.0_f64;
    for term in (1..=TERMS).rev() {
        series = 1.0 - y / f64::from(term) * series;
    }
    for _ in 0..squarings {
        series *= series;
    }
    Some(series)
}
