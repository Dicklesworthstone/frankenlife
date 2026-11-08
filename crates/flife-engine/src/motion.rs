//! Field-mediated, affordable motion (plan §5.4, §8.2, A.1, Appendix C.11,
//! C.12).
//!
//! Motion follows the local chemical response, not pairwise forces:
//! `f = bound[g_u (χ_e ∇ẽ + Σ_k χ_k ∇z̃_k)]`. Here `g_u = U/(Δt U_max)` is
//! the actual granted uptake as a fraction of capacity, and the 33
//! coefficients `χ` share a fixed Euclidean norm budget. `bound` clamps the
//! acceleration radially to `maximum_acceleration`. The normalized sensor
//! gradients come from sensing (FL-007).
//!
//! Linear drag is integrated analytically while the chemical acceleration is
//! held over the tick:
//! `v* = e^(−γΔt) v + (1 − e^(−γΔt))/γ · f`, with force coefficient `Δt`
//! when `γ = 0`. The coefficient uses [`expm1_neg`], because a literal
//! `1 − exp(−γΔt)` cancels to zero for small `γ` (C.11). The speed is then
//! clamped radially to `maximum_speed`.
//!
//! Motion must be affordable. The cost is `c_v ||v||² Δt`. When it exceeds
//! the budget, the whole proposed velocity is scaled by
//! `sqrt(E_available / cost)` and all available energy is paid, so the
//! organism is exhausted: scaling only the active force would not do,
//! because an opposing force can shrink an already unaffordable residual
//! velocity. The position step is then
//! `x_{n+1} = wrap(x_n + Δt v_{n+1})`. An exhausted organism still moves by
//! what it paid for and then dies (C.12).
//!
//! Every radial bound is checked on the computed F64 squares. A vector that
//! rounding leaves just outside the bound is shrunk by one ulp at a time
//! until it is inside, so the bound holds exactly as computed.

use flife_core::ledger::{EnergyBudget, Payment};
use flife_core::{CoreError, TimeStep, ValidatedConfig};
use flife_field::FieldGrid;
use flife_field::numerics::{exp_neg, expm1_neg};
use flife_field::state::CHANNELS;

/// The number of motion-response coefficients: energy plus 32 channels.
pub const RESPONSE_TERMS: usize = 1 + CHANNELS;

/// The per-run motion constants (§5.4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    step_seconds: f64,
    decay: f64,
    force_coefficient: f64,
    maximum_speed: f64,
    maximum_acceleration: f64,
    cost: f64,
    response_norm_limit: f64,
}

impl Motion {
    /// Motion with drag `γ ≥ 0` (per simulation-second), positive speed and
    /// acceleration limits, cost `c_v ≥ 0` (ecological-energy per
    /// speed²·second) and a positive response norm limit.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a value outside its range.
    pub fn new(
        step: TimeStep,
        drag: f64,
        maximum_speed: f64,
        maximum_acceleration: f64,
        cost: f64,
        response_norm_limit: f64,
    ) -> Result<Self, CoreError> {
        for (quantity, value, admissible) in [
            ("chemistry.drag_per_second", drag, drag >= 0.0),
            (
                "chemistry.maximum_speed",
                maximum_speed,
                maximum_speed > 0.0,
            ),
            (
                "chemistry.maximum_acceleration",
                maximum_acceleration,
                maximum_acceleration > 0.0,
            ),
            (
                "costs.motion_energy_per_speed_squared_second",
                cost,
                cost >= 0.0,
            ),
            (
                "chemistry.motion_response_norm_limit",
                response_norm_limit,
                response_norm_limit > 0.0,
            ),
        ] {
            if !value.is_finite() || !admissible {
                return Err(CoreError::NumericalInvalid {
                    quantity,
                    detail: format!("{value} is out of range"),
                });
            }
        }
        let step_seconds = step.seconds_per_tick();
        let exponent = drag * step_seconds;
        let invalid = || CoreError::NumericalInvalid {
            quantity: "drag over one tick",
            detail: format!("γΔt = {exponent} is out of range"),
        };
        let decay = exp_neg(exponent).ok_or_else(invalid)?;
        let force_coefficient = if drag == 0.0 {
            step_seconds
        } else {
            expm1_neg(exponent).ok_or_else(invalid)? / drag
        };
        Ok(Self {
            step_seconds,
            decay,
            force_coefficient,
            maximum_speed,
            maximum_acceleration,
            cost,
            response_norm_limit,
        })
    }

    /// The motion constants of a validated configuration.
    ///
    /// # Errors
    /// As [`Motion::new`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let chemistry = &config.config().chemistry;
        Self::new(
            config.report().timestep,
            chemistry.drag_per_second,
            chemistry.maximum_speed,
            chemistry.maximum_acceleration,
            config.config().costs.motion_energy_per_speed_squared_second,
            chemistry.motion_response_norm_limit,
        )
    }

    /// `e^(−γΔt)`, the velocity kept over one tick.
    #[must_use]
    pub const fn decay(&self) -> f64 {
        self.decay
    }

    /// `(1 − e^(−γΔt))/γ`, or `Δt` when `γ = 0`.
    #[must_use]
    pub const fn force_coefficient(&self) -> f64 {
        self.force_coefficient
    }

    /// The response norm limit.
    #[must_use]
    pub const fn response_norm_limit(&self) -> f64 {
        self.response_norm_limit
    }

    /// `f = bound[g_u Σ_j χ_j ∇_j]` for uptake ratio `g_u ∈ [0, 1]`, summed
    /// in coefficient order: energy first, then channels.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a ratio outside `[0, 1]` or a
    /// nonfinite gradient.
    pub fn acceleration(
        &self,
        uptake_ratio: f64,
        response: &MotionResponse,
        gradients: &[[f64; 2]; RESPONSE_TERMS],
    ) -> Result<[f64; 2], CoreError> {
        if !(0.0..=1.0).contains(&uptake_ratio) {
            return Err(CoreError::NumericalInvalid {
                quantity: "uptake ratio",
                detail: format!("{uptake_ratio} is outside [0, 1]"),
            });
        }
        let mut sum = [0.0, 0.0];
        for (coefficient, gradient) in response.0.iter().zip(gradients) {
            if !gradient[0].is_finite() || !gradient[1].is_finite() {
                return Err(CoreError::NumericalInvalid {
                    quantity: "sensor gradient",
                    detail: format!("{gradient:?} is not finite"),
                });
            }
            sum[0] += coefficient * gradient[0];
            sum[1] += coefficient * gradient[1];
        }
        Ok(clamp_radius(
            [uptake_ratio * sum[0], uptake_ratio * sum[1]],
            self.maximum_acceleration,
        ))
    }

    /// The proposed velocity `v* = e^(−γΔt) v + coefficient · f`, clamped to
    /// the speed limit.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite velocity or
    /// acceleration.
    pub fn propose(
        &self,
        velocity: [f64; 2],
        acceleration: [f64; 2],
    ) -> Result<[f64; 2], CoreError> {
        if velocity.iter().chain(&acceleration).any(|v| !v.is_finite()) {
            return Err(CoreError::NumericalInvalid {
                quantity: "velocity proposal",
                detail: format!("{velocity:?} + {acceleration:?} is not finite"),
            });
        }
        let proposed = [
            self.decay * velocity[0] + self.force_coefficient * acceleration[0],
            self.decay * velocity[1] + self.force_coefficient * acceleration[1],
        ];
        Ok(clamp_radius(proposed, self.maximum_speed))
    }

    /// Pay for `proposed` from `budget`, scaling the whole velocity when the
    /// cost `c_v ||v||² Δt` is unaffordable.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite velocity.
    pub fn settle(
        &self,
        proposed: [f64; 2],
        budget: &mut EnergyBudget,
    ) -> Result<MotionSettlement, CoreError> {
        if !proposed[0].is_finite() || !proposed[1].is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "proposed velocity",
                detail: format!("{proposed:?} is not finite"),
            });
        }
        let requested =
            self.cost * (proposed[0] * proposed[0] + proposed[1] * proposed[1]) * self.step_seconds;
        let available = budget.remaining();
        if requested <= available {
            let cost = budget.pay(requested)?;
            return Ok(MotionSettlement {
                proposed,
                velocity: proposed,
                cost,
                scaled: false,
            });
        }
        let factor = (available / requested).sqrt();
        let paid = budget.pay(available)?;
        Ok(MotionSettlement {
            proposed,
            velocity: [proposed[0] * factor, proposed[1] * factor],
            cost: Payment {
                requested,
                paid: paid.paid,
            },
            scaled: true,
        })
    }

    /// `wrap(x + Δt v)` on `grid`'s torus.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite result.
    pub fn displace(
        &self,
        grid: &FieldGrid,
        position: [f64; 2],
        velocity: [f64; 2],
    ) -> Result<[f64; 2], CoreError> {
        grid.wrap_position([
            position[0] + self.step_seconds * velocity[0],
            position[1] + self.step_seconds * velocity[1],
        ])
    }
}

/// Heritable motion-response coefficients `(χ_e, χ_1, …, χ_32)` within the
/// run's norm budget (§5.4), checked in F64.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionResponse([f64; RESPONSE_TERMS]);

impl MotionResponse {
    /// The coefficients, which must lie within `motion`'s norm limit.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite coefficient or a norm
    /// above the limit.
    pub fn new(coefficients: [f64; RESPONSE_TERMS], motion: &Motion) -> Result<Self, CoreError> {
        let mut squared = 0.0;
        for coefficient in coefficients {
            if !coefficient.is_finite() {
                return Err(CoreError::NumericalInvalid {
                    quantity: "motion response",
                    detail: format!("coefficient {coefficient} is not finite"),
                });
            }
            squared += coefficient * coefficient;
        }
        let limit = motion.response_norm_limit;
        if squared > limit * limit {
            return Err(CoreError::NumericalInvalid {
                quantity: "motion response",
                detail: format!("||χ||² = {squared} exceeds the limit {limit}²"),
            });
        }
        Ok(Self(coefficients))
    }

    /// The coefficients.
    #[must_use]
    pub const fn coefficients(&self) -> &[f64; RESPONSE_TERMS] {
        &self.0
    }
}

/// One organism's motion for this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionSettlement {
    /// The proposed velocity after the speed limit.
    pub proposed: [f64; 2],
    /// The velocity actually applied.
    pub velocity: [f64; 2],
    /// The cost of the proposal and the cost actually paid (§8.2).
    pub cost: Payment,
    /// Whether the proposal was unaffordable and scaled down.
    pub scaled: bool,
}

/// `g_u = U / (Δt U_max)`: actual granted uptake as a fraction of capacity,
/// zero at zero capacity. Grants are whole field quanta, so a ratio can
/// exceed one by rounding; it is clamped to `[0, 1]`.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for a negative or nonfinite uptake or
/// capacity.
pub fn uptake_ratio(
    absorbed: f64,
    step: TimeStep,
    capacity_per_second: f64,
) -> Result<f64, CoreError> {
    for (quantity, value) in [
        ("absorbed uptake", absorbed),
        ("chemistry.uptake_capacity_per_second", capacity_per_second),
    ] {
        if !value.is_finite() || value < 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity,
                detail: format!("{value} is not a finite nonnegative amount"),
            });
        }
    }
    if capacity_per_second == 0.0 {
        return Ok(0.0);
    }
    Ok((absorbed / (step.seconds_per_tick() * capacity_per_second)).min(1.0))
}

/// `v` clamped radially so that `v·v ≤ limit²` as computed in F64.
fn clamp_radius(v: [f64; 2], limit: f64) -> [f64; 2] {
    let bound = limit * limit;
    let squared = v[0] * v[0] + v[1] * v[1];
    if squared <= bound {
        return v;
    }
    let scale = limit / squared.sqrt();
    let mut clamped = [v[0] * scale, v[1] * scale];
    let shrink = 1.0 - f64::EPSILON / 2.0;
    while clamped[0] * clamped[0] + clamped[1] * clamped[1] > bound {
        clamped = [clamped[0] * shrink, clamped[1] * shrink];
    }
    clamped
}
