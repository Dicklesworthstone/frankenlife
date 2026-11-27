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
use flife_field::settle::{MotorSteeringConstants, MotorSteeringInputs};
use flife_field::state::CHANNELS;

/// The number of motion-response coefficients: energy plus 32 channels.
pub const RESPONSE_TERMS: usize = 1 + CHANNELS;

/// Minimum speed or gradient magnitude for `V2-05` (`so2-motor`) guarded
/// heading normalization (`0x3D719799812DEA11` in `motion.metal`).
pub const HEADING_EPS: f64 = 1.0e-12;

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
        let force_coefficient = if drag == 0.0 || exponent == 0.0 {
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

    /// The speed limit every committed velocity is clamped to.
    #[must_use]
    pub const fn maximum_speed(&self) -> f64 {
        self.maximum_speed
    }

    /// `Δt`, the acceleration limit and the cost coefficient `c_v`, for a
    /// device's phase 6 (FL-032).
    pub(crate) const fn step_limit_and_cost(&self) -> (f64, f64, f64) {
        (self.step_seconds, self.maximum_acceleration, self.cost)
    }

    /// `f = bound[g_u Σ_j χ_j ∇_j]` for uptake ratio `g_u ∈ [0, 1]`, summed
    /// in coefficient order: energy first, then channels.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a ratio outside `[0, 1]` or a
    /// nonfinite gradient.
    #[inline]
    pub fn acceleration(
        &self,
        uptake_ratio: f64,
        response: &MotionResponse,
        gradients: &[[f64; 2]; RESPONSE_TERMS],
    ) -> Result<[f64; 2], CoreError> {
        self.acceleration_slice(uptake_ratio, &response.0, gradients)
    }

    /// As [`Motion::acceleration`], taking a pre-validated response array by
    /// reference without copying 264 bytes onto the stack.
    ///
    /// Uses 2-lane `[gx, gy]` accumulation in exact ascending term order
    /// `0..RESPONSE_TERMS` (0-ULP bit-identical) so LLVM emits direct `ldr q` +
    /// `fmul.2d` + `fadd.2d` without `ld2.2d` deinterleaving or scalar lane
    /// extractions.
    ///
    /// # Errors
    /// As [`Motion::acceleration`].
    #[inline]
    pub fn acceleration_slice(
        &self,
        uptake_ratio: f64,
        response: &[f64; RESPONSE_TERMS],
        gradients: &[[f64; 2]; RESPONSE_TERMS],
    ) -> Result<[f64; 2], CoreError> {
        #[cold]
        #[inline(never)]
        fn err_uptake_ratio(uptake_ratio: f64) -> CoreError {
            CoreError::NumericalInvalid {
                quantity: "uptake ratio",
                detail: format!("{uptake_ratio} is outside [0, 1]"),
            }
        }
        #[cold]
        #[inline(never)]
        fn err_nonfinite_gradient(gradient: [f64; 2]) -> CoreError {
            CoreError::NumericalInvalid {
                quantity: "sensor gradient",
                detail: format!("{gradient:?} is not finite"),
            }
        }
        if !(0.0..=1.0).contains(&uptake_ratio) {
            return Err(err_uptake_ratio(uptake_ratio));
        }
        let mut sum = [0.0_f64; 2];
        // Iterate via slices so LLVM keeps a tight register-only 2-wide loop
        // (`ldr q` + `fmul.2d` + `fadd.2d`). Every finite `c` multiplied by a
        // nonfinite gradient yields `NaN` or `±Inf` (including `0.0 * ±Inf = NaN`),
        // which poisons `sum` monotonically, so checking `sum.is_finite()` catches
        // every nonfinite gradient without a separate `fin` accumulator.
        for (&c, &g) in response.as_slice().iter().zip(gradients.as_slice()) {
            sum = [sum[0] + c * g[0], sum[1] + c * g[1]];
        }
        if !(sum[0].is_finite() && sum[1].is_finite()) {
            for &gradient in gradients {
                if !gradient[0].is_finite() || !gradient[1].is_finite() {
                    return Err(err_nonfinite_gradient(gradient));
                }
            }
        }
        clamp_radius(
            "chemical acceleration",
            [uptake_ratio * sum[0], uptake_ratio * sum[1]],
            self.maximum_acceleration,
        )
    }

    /// `V2-05` (`so2-motor`) SO(2)-equivariant neural acceleration: splits
    /// `g_E = χ_e ∇q` and `g_C = Σ_{k=1..33} χ_k ∇Z_k`, applies chiral contour
    /// `g_perp = [-g_sum.y, g_sum.x]` and energy-scaled guarded heading
    /// propulsion `prop_mag * h_hat`, then clamps radially to `a_max`.
    ///
    /// When `motor.is_zero()`, takes the exact baseline single-loop fast-path
    /// ([`Motion::acceleration`]) for 0-ulp bit-identity with `field-ecology-v1`
    /// and `chemistry-only`, matching `motion.metal` bit-for-bit.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for out-of-range inputs or nonfinite
    /// intermediates.
    #[expect(
        clippy::too_many_arguments,
        reason = "V2-05 motion acceleration requires post-maintenance budget, velocity, constants, and steering"
    )]
    pub fn acceleration_v2(
        &self,
        uptake_ratio: f64,
        response: &MotionResponse,
        gradients: &[[f64; 2]; RESPONSE_TERMS],
        velocity: [f64; 2],
        remaining: f64,
        constants: &MotorSteeringConstants,
        motor: MotorSteeringInputs,
    ) -> Result<[f64; 2], CoreError> {
        #[cold]
        #[inline(never)]
        fn err_motor_range(quantity: &'static str, value: f64) -> CoreError {
            CoreError::NumericalInvalid {
                quantity,
                detail: format!("{value} is outside [-1, 1]"),
            }
        }
        #[cold]
        #[inline(never)]
        fn err_v2_inputs(
            uptake_ratio: f64,
            velocity: [f64; 2],
            remaining: f64,
            internal_energy_scale: f64,
            g0: [f64; 2],
            motor: MotorSteeringInputs,
        ) -> CoreError {
            for (name, value) in [
                ("motor.energy_gradient", motor.energy_gradient),
                ("motor.chemical_gradient", motor.chemical_gradient),
                ("motor.chiral_contour", motor.chiral_contour),
                ("motor.heading_propulsion", motor.heading_propulsion),
            ] {
                if !value.is_finite() || value.abs() > 1.0 {
                    return err_motor_range(name, value);
                }
            }
            if !(0.0..=1.0).contains(&uptake_ratio) {
                return CoreError::NumericalInvalid {
                    quantity: "uptake ratio",
                    detail: format!("{uptake_ratio} is outside [0, 1]"),
                };
            }
            if !velocity[0].is_finite() || !velocity[1].is_finite() {
                return CoreError::NumericalInvalid {
                    quantity: "organism velocity",
                    detail: format!("{velocity:?} is not finite"),
                };
            }
            if !remaining.is_finite() || remaining < 0.0 {
                return CoreError::NumericalInvalid {
                    quantity: "post-maintenance energy",
                    detail: format!("{remaining} is not finite and nonnegative"),
                };
            }
            if !internal_energy_scale.is_finite() || internal_energy_scale <= 0.0 {
                return CoreError::NumericalInvalid {
                    quantity: "sensors.internal_energy_scale",
                    detail: format!("{internal_energy_scale} must be finite and positive"),
                };
            }
            CoreError::NumericalInvalid {
                quantity: "sensor gradient",
                detail: format!("{g0:?} is not finite"),
            }
        }
        #[cold]
        #[inline(never)]
        fn err_nonfinite_gradient(gradient: [f64; 2]) -> CoreError {
            CoreError::NumericalInvalid {
                quantity: "sensor gradient",
                detail: format!("{gradient:?} is not finite"),
            }
        }
        #[cold]
        #[inline(never)]
        fn err_v_sq_overflow(velocity: [f64; 2]) -> CoreError {
            CoreError::NumericalInvalid {
                quantity: "organism velocity",
                detail: format!("{velocity:?} norm squared overflowed"),
            }
        }
        #[cold]
        #[inline(never)]
        fn err_g_sq_overflow(gsum_x: f64, gsum_y: f64) -> CoreError {
            CoreError::NumericalInvalid {
                quantity: "chemical gradient sum",
                detail: format!("[{gsum_x}, {gsum_y}] norm squared overflowed"),
            }
        }
        #[cold]
        #[inline(never)]
        fn err_e_denom(e_denom: f64) -> CoreError {
            CoreError::NumericalInvalid {
                quantity: "energy gate denominator",
                detail: format!("{e_denom} is invalid"),
            }
        }

        if motor.is_zero() {
            return self.acceleration(uptake_ratio, response, gradients);
        }
        let g0 = gradients[0];
        let inputs_ok = (motor.energy_gradient.abs() <= 1.0)
            & (motor.chemical_gradient.abs() <= 1.0)
            & (motor.chiral_contour.abs() <= 1.0)
            & (motor.heading_propulsion.abs() <= 1.0)
            & (0.0..=1.0).contains(&uptake_ratio)
            & velocity[0].is_finite()
            & velocity[1].is_finite()
            & remaining.is_finite()
            & (remaining >= 0.0)
            & constants.internal_energy_scale.is_finite()
            & (constants.internal_energy_scale > 0.0)
            & g0[0].is_finite()
            & g0[1].is_finite();
        if !inputs_ok {
            return Err(err_v2_inputs(
                uptake_ratio,
                velocity,
                remaining,
                constants.internal_energy_scale,
                g0,
                motor,
            ));
        }

        let ge_x = response.0[0] * g0[0];
        let ge_y = response.0[0] * g0[1];

        // Accumulate all 32 chemical gradient terms in exact ascending order
        // without per-channel error branches so LLVM emits 2-wide NEON
        // (`ldr q` + `fmul.2d` + `fadd.2d`). Any nonfinite gradient produces
        // `NaN` or `±Inf` (including `0.0 * ±Inf = NaN`), caught monotonically.
        let mut gc = [0.0_f64; 2];
        for (&c, &g) in response.0[1..].iter().zip(&gradients[1..]) {
            gc = [gc[0] + c * g[0], gc[1] + c * g[1]];
        }
        if !(gc[0].is_finite() && gc[1].is_finite()) {
            for &gradient in &gradients[1..] {
                if !gradient[0].is_finite() || !gradient[1].is_finite() {
                    return Err(err_nonfinite_gradient(gradient));
                }
            }
        }
        let [gc_x, gc_y] = gc;

        let gsum_x = ge_x + gc_x;
        let gsum_y = ge_y + gc_y;
        let gperp_x = -gsum_y;
        let gperp_y = gsum_x;

        let v_sq = velocity[0] * velocity[0] + velocity[1] * velocity[1];
        if !v_sq.is_finite() {
            return Err(err_v_sq_overflow(velocity));
        }
        let v_norm = v_sq.sqrt();
        let (hx, hy) = if HEADING_EPS < v_norm {
            (velocity[0] / v_norm, velocity[1] / v_norm)
        } else {
            let g_sq = gsum_x * gsum_x + gsum_y * gsum_y;
            if !g_sq.is_finite() {
                return Err(err_g_sq_overflow(gsum_x, gsum_y));
            }
            let g_norm = g_sq.sqrt();
            if HEADING_EPS < g_norm {
                (gsum_x / g_norm, gsum_y / g_norm)
            } else {
                (0.0, 0.0)
            }
        };

        let w_e = 1.0 + constants.energy_gradient_gain * motor.energy_gradient;
        let w_c = 1.0 + constants.chemical_gradient_gain * motor.chemical_gradient;
        let w_perp = constants.chiral_contour_gain * motor.chiral_contour;

        let chem_x = (w_e * ge_x + w_c * gc_x) + w_perp * gperp_x;
        let chem_y = (w_e * ge_y + w_c * gc_y) + w_perp * gperp_y;
        let grad_x = uptake_ratio * chem_x;
        let grad_y = uptake_ratio * chem_y;

        let e_denom = remaining + constants.internal_energy_scale;
        if !e_denom.is_finite() || e_denom == 0.0 {
            return Err(err_e_denom(e_denom));
        }
        let energy_gate = remaining / e_denom;
        let prop_mag = ((constants.heading_propulsion_fraction * self.maximum_acceleration)
            * energy_gate)
            * motor.heading_propulsion;
        let ax = grad_x + prop_mag * hx;
        let ay = grad_y + prop_mag * hy;
        clamp_radius("chemical acceleration", [ax, ay], self.maximum_acceleration)
    }

    /// The proposed velocity `v* = e^(−γΔt) v + coefficient · f`, clamped to
    /// the speed limit.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite velocity, acceleration,
    /// or proposed velocity.
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
        clamp_radius("proposed velocity", proposed, self.maximum_speed)
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
        let requested = if self.cost == 0.0 {
            0.0
        } else {
            self.cost * (proposed[0] * proposed[0] + proposed[1] * proposed[1]) * self.step_seconds
        };
        if requested.is_nan() || requested < 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "motion cost",
                detail: format!("{requested} is not nonnegative"),
            });
        }
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
        let bound = limit * limit;
        let exceeds = if squared.is_finite() && bound.is_finite() && bound >= f64::MIN_POSITIVE {
            squared > bound
        } else if !squared.is_finite() || !bound.is_finite() {
            let s_limit = limit * CLAMP_DOWN_SCALE;
            let s_squared: f64 = coefficients
                .iter()
                .map(|&c| {
                    let sc = c * CLAMP_DOWN_SCALE;
                    sc * sc
                })
                .sum();
            s_squared > s_limit * s_limit
        } else if coefficients.iter().any(|&c| c.abs() > limit) {
            true
        } else {
            let up = if limit >= 1.0e-300 {
                CLAMP_UP_MID
            } else {
                CLAMP_UP_TINY
            };
            let s_limit = limit * up;
            let s_squared: f64 = coefficients
                .iter()
                .map(|&c| {
                    let sc = c * up;
                    sc * sc
                })
                .sum();
            s_squared > s_limit * s_limit
        };
        if exceeds {
            return Err(CoreError::NumericalInvalid {
                quantity: "motion response",
                detail: format!("||χ||² = {squared} exceeds the limit {limit}²"),
            });
        }
        Ok(Self(coefficients))
    }

    /// Construct from traits already verified against a bound at or below
    /// `motion.response_norm_limit`.
    #[must_use]
    pub(crate) const fn trusted(coefficients: [f64; RESPONSE_TERMS]) -> Self {
        Self(coefficients)
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

/// Exact IEEE-754 power-of-two factor (`2^-520`) that scales any finite `f64`
/// below `2^504` so sums of squares never overflow `f64::MAX`, with zero
/// mantissa rounding error.
const CLAMP_DOWN_SCALE: f64 = f64::from_bits((1023_u64 - 520) << 52);

/// Exact IEEE-754 powers of two (`2^530`, `2^1020`) that lift tiny limits and
/// vectors into normal range when `limit²` underflows below `f64::MIN_POSITIVE`.
const CLAMP_UP_MID: f64 = f64::from_bits((1023_u64 + 530) << 52);
const CLAMP_UP_TINY: f64 = f64::from_bits((1023_u64 + 1020) << 52);

/// Step `value` toward zero by `shrink`, guaranteeing progress by 1 ULP even
/// in the subnormal range where `value * shrink == value`.
fn step_toward_zero(value: f64, shrink: f64) -> f64 {
    if value == 0.0 {
        return value;
    }
    let next = value * shrink;
    if next == value {
        f64::from_bits(value.to_bits() - 1)
    } else {
        next
    }
}

/// `v` clamped radially so that `v·v ≤ limit²` as computed in F64.
///
/// The finite-square fast path executes the exact arithmetic of
/// `motion.metal`'s `clamp_radius` for 0-ulp CPU/Metal parity whenever
/// `limit²` stays normal. When `v·v` or `limit²` overflows `f64::MAX` or
/// `limit²` underflows below `f64::MIN_POSITIVE`, falls back to exact
/// power-of-two scaling rather than collapsing to `[0, 0]`, bypassing the
/// clamp, or leaking `NaN`.
fn clamp_radius(quantity: &'static str, v: [f64; 2], limit: f64) -> Result<[f64; 2], CoreError> {
    if !v[0].is_finite() || !v[1].is_finite() {
        return Err(CoreError::NumericalInvalid {
            quantity,
            detail: format!("{v:?} is not finite"),
        });
    }
    let bound = limit * limit;
    let squared = v[0] * v[0] + v[1] * v[1];
    let shrink = 1.0 - f64::EPSILON / 2.0;
    if squared.is_finite() && bound.is_finite() && bound >= f64::MIN_POSITIVE {
        if squared <= bound {
            return Ok(v);
        }
        let scale = limit / squared.sqrt();
        let mut clamped = [v[0] * scale, v[1] * scale];
        while clamped[0] * clamped[0] + clamped[1] * clamped[1] > bound {
            clamped = [clamped[0] * shrink, clamped[1] * shrink];
        }
        return Ok(clamped);
    }
    if bound < f64::MIN_POSITIVE {
        let max_v = v[0].abs().max(v[1].abs());
        if max_v == 0.0 {
            return Ok(v);
        }
        let up_limit = if limit >= 1.0e-300 {
            CLAMP_UP_MID
        } else {
            CLAMP_UP_TINY
        };
        let s_limit = limit * up_limit;
        let s_bound = s_limit * s_limit;
        if max_v <= limit {
            let sv = [v[0] * up_limit, v[1] * up_limit];
            if sv[0] * sv[0] + sv[1] * sv[1] <= s_bound {
                return Ok(v);
            }
        }
        let v_scale = if max_v > 1.0e150 {
            CLAMP_DOWN_SCALE
        } else if max_v < 1.0e-150 {
            if max_v >= 1.0e-300 {
                CLAMP_UP_MID
            } else {
                CLAMP_UP_TINY
            }
        } else {
            1.0
        };
        let sv = [v[0] * v_scale, v[1] * v_scale];
        let s_norm = (sv[0] * sv[0] + sv[1] * sv[1]).sqrt();
        let mut clamped = [(sv[0] / s_norm) * limit, (sv[1] / s_norm) * limit];
        while {
            let sc = [clamped[0] * up_limit, clamped[1] * up_limit];
            sc[0] * sc[0] + sc[1] * sc[1] > s_bound
        } {
            clamped = [
                step_toward_zero(clamped[0], shrink),
                step_toward_zero(clamped[1], shrink),
            ];
        }
        return Ok(clamped);
    }
    let sv = [v[0] * CLAMP_DOWN_SCALE, v[1] * CLAMP_DOWN_SCALE];
    let s_squared = sv[0] * sv[0] + sv[1] * sv[1];
    if bound.is_finite() {
        let s_norm = s_squared.sqrt();
        let mut clamped = [(sv[0] / s_norm) * limit, (sv[1] / s_norm) * limit];
        while clamped[0] * clamped[0] + clamped[1] * clamped[1] > bound {
            clamped = [clamped[0] * shrink, clamped[1] * shrink];
        }
        return Ok(clamped);
    }
    let s_limit = limit * CLAMP_DOWN_SCALE;
    let s_bound = s_limit * s_limit;
    if s_squared <= s_bound {
        return Ok(v);
    }
    let s_norm = s_squared.sqrt();
    let mut clamped = [(sv[0] / s_norm) * limit, (sv[1] / s_norm) * limit];
    while {
        let sc = [clamped[0] * CLAMP_DOWN_SCALE, clamped[1] * CLAMP_DOWN_SCALE];
        sc[0] * sc[0] + sc[1] * sc[1] > s_bound
    } {
        clamped = [clamped[0] * shrink, clamped[1] * shrink];
    }
    Ok(clamped)
}
