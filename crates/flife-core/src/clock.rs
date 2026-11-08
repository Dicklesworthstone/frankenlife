//! Integer clocks (plan §4.1, §12.2, A.3, C.15).
//!
//! The scientific clock is an integer [`Tick`] and a rational [`TimeStep`].
//! Ages, cooldowns and deadlines are integer tick arithmetic, and seconds
//! are derived from it on demand. Nothing repeatedly adds `Δt` to a float:
//! at `Δt = 1/128` an F32 accumulator stops advancing at 131,072
//! simulation-seconds. Every conversion from configured seconds to ticks is
//! exact rational arithmetic on the f64's binary value. Overflow is refused.

use crate::error::CoreError;
use crate::ids::OrganismId;

/// The fixed scientific timestep `Δt = numerator / denominator` simulation
/// seconds, stored in lowest terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimeStep {
    numerator: u32,
    denominator: u32,
}

const fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let rest = a % b;
        a = b;
        b = rest;
    }
    a
}

impl TimeStep {
    /// `numerator / denominator` seconds, reduced to lowest terms.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless both parts are positive.
    pub fn new(numerator: u32, denominator: u32) -> Result<Self, CoreError> {
        if numerator == 0 || denominator == 0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "timestep",
                detail: format!("{numerator}/{denominator} is not a positive rational"),
            });
        }
        let divisor = gcd(numerator, denominator);
        Ok(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    /// Numerator in lowest terms.
    #[must_use]
    pub const fn numerator(self) -> u32 {
        self.numerator
    }

    /// Denominator in lowest terms.
    #[must_use]
    pub const fn denominator(self) -> u32 {
        self.denominator
    }

    /// `Δt` in seconds, correctly rounded to f64.
    #[must_use]
    pub fn seconds_per_tick(self) -> f64 {
        f64::from(self.numerator) / f64::from(self.denominator)
    }

    /// The simulation seconds spanned by `span` ticks.
    ///
    /// Computed as `(span × numerator) / denominator`. The product is exact
    /// in 128 bits; it converts to f64 exactly below 2^53 and the division
    /// is then correctly rounded. For a power-of-two denominator, such as
    /// the reference 1/128, the result is exact whenever the product is
    /// below 2^53.
    #[must_use]
    pub fn seconds(self, span: TickSpan) -> f64 {
        let scaled = u128::from(span.get()) * u128::from(self.numerator);
        scaled as f64 / f64::from(self.denominator)
    }

    /// Ticks in exactly `seconds`, which must be a whole number of ticks
    /// (plan A.3 item 8: `T/Δt` is an integer).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a negative, nonfinite or
    /// non-tick-aligned value; [`CoreError::Overflow`] above `u64::MAX`
    /// ticks.
    pub fn ticks_exact(self, seconds: f64, quantity: &'static str) -> Result<TickSpan, CoreError> {
        let (floor, exact) = self.seconds_to_ticks(seconds, quantity)?;
        if !exact {
            return Err(CoreError::NumericalInvalid {
                quantity,
                detail: format!(
                    "{seconds:?} s is not a whole number of {}/{} s ticks",
                    self.numerator, self.denominator
                ),
            });
        }
        tick_span(floor, quantity)
    }

    /// `ceil(seconds / Δt)` ticks, computed exactly (plan §7.4 cooldowns).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a negative or nonfinite value;
    /// [`CoreError::Overflow`] above `u64::MAX` ticks.
    pub fn ticks_ceil(self, seconds: f64, quantity: &'static str) -> Result<TickSpan, CoreError> {
        let (floor, exact) = self.seconds_to_ticks(seconds, quantity)?;
        let ceil = if exact {
            Some(floor)
        } else {
            floor.checked_add(1)
        };
        tick_span(ceil.ok_or(CoreError::Overflow { quantity })?, quantity)
    }

    /// `(floor(seconds / Δt), exact)`, using `seconds = m × 2^e` exactly.
    fn seconds_to_ticks(
        self,
        seconds: f64,
        quantity: &'static str,
    ) -> Result<(u128, bool), CoreError> {
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity,
                detail: format!("{seconds:?} s is not a finite nonnegative duration"),
            });
        }
        let (mantissa, exponent) = decompose(seconds);
        // ticks = mantissa × 2^exponent × denominator / numerator
        let scaled = u128::from(mantissa) * u128::from(self.denominator);
        let divisor = u128::from(self.numerator);
        if exponent >= 0 {
            let shift = exponent.unsigned_abs();
            if shift > scaled.leading_zeros() {
                return Err(CoreError::Overflow { quantity });
            }
            let value = scaled << shift;
            Ok((value / divisor, value % divisor == 0))
        } else {
            // floor(a / (n 2^k)) = floor(floor(a / n) / 2^k) for positive integers.
            let shift = exponent.unsigned_abs();
            let quotient = scaled / divisor;
            let remainder = scaled % divisor;
            if shift >= 128 {
                return Ok((0, remainder == 0 && quotient == 0));
            }
            let low_bits = quotient & ((1_u128 << shift) - 1);
            Ok((quotient >> shift, remainder == 0 && low_bits == 0))
        }
    }
}

/// `x = mantissa × 2^exponent` exactly, for finite nonnegative `x`.
fn decompose(x: f64) -> (u64, i32) {
    let bits = x.to_bits();
    let biased = i32::try_from((bits >> 52) & 0x7ff).unwrap_or(0);
    let fraction = bits & ((1_u64 << 52) - 1);
    if biased == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1_u64 << 52), biased - 1075)
    }
}

fn tick_span(ticks: u128, quantity: &'static str) -> Result<TickSpan, CoreError> {
    u64::try_from(ticks)
        .map(TickSpan)
        .map_err(|_| CoreError::Overflow { quantity })
}

/// A committed scientific tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tick(u64);

impl Tick {
    /// The initial tick.
    pub const ZERO: Self = Self(0);

    /// Wrap a raw tick.
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw tick.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// The following tick.
    ///
    /// # Errors
    /// [`CoreError::Overflow`] at `u64::MAX`.
    pub fn next(self) -> Result<Self, CoreError> {
        self.checked_add(TickSpan(1))
    }

    /// This tick plus `span`, for example a cooldown deadline.
    ///
    /// # Errors
    /// [`CoreError::Overflow`] past `u64::MAX`.
    pub fn checked_add(self, span: TickSpan) -> Result<Self, CoreError> {
        self.0
            .checked_add(span.0)
            .map(Self)
            .ok_or(CoreError::Overflow { quantity: "tick" })
    }

    /// Ticks elapsed since `earlier`.
    ///
    /// # Errors
    /// [`CoreError::Overflow`] when `earlier` is after this tick.
    pub fn since(self, earlier: Self) -> Result<TickSpan, CoreError> {
        self.0
            .checked_sub(earlier.0)
            .map(TickSpan)
            .ok_or(CoreError::Overflow {
                quantity: "elapsed ticks (earlier tick is later)",
            })
    }
}

/// A whole number of ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TickSpan(u64);

impl TickSpan {
    /// Wrap a raw tick count.
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// The tick count.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Age in simulation seconds, derived from integer ticks
/// (`(now − birth) Δt`, plan §4.1).
///
/// # Errors
/// [`CoreError::Overflow`] when `birth` is after `now`.
pub fn age_seconds(birth: Tick, now: Tick, step: TimeStep) -> Result<f64, CoreError> {
    Ok(step.seconds(now.since(birth)?))
}

/// An organism's stable controller phase: its canonical unsigned identity
/// modulo the evaluation interval (plan A.3 item 10). It depends only on
/// identity, never on storage order or worker assignment.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for a zero interval.
pub fn stable_phase(id: OrganismId, interval_ticks: u32) -> Result<u32, CoreError> {
    if interval_ticks == 0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "controller evaluation interval",
            detail: "0 ticks; the interval must be at least 1".into(),
        });
    }
    u32::try_from(id.get() % u64::from(interval_ticks)).map_err(|_| CoreError::Overflow {
        quantity: "controller phase",
    })
}
