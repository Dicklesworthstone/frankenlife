//! Paid emission (plan §5.5, §8.1 phase 7, §8.2, A.1, Appendix C.4).
//!
//! The action decoder proposes an emission (`flife_neural::Emission`) only
//! when the gate is open and the direction valid. Otherwise, under
//! `no-emission-no-charge`, the organism emits nothing and pays nothing. A
//! dead organism, one whose budget is exhausted, also emits nothing.
//!
//! An open emission spends `w = f E`. Here `E` is what the organism holds
//! at the emission stage, which comes after motion (§8.1), and `f` is the
//! decoded fraction `1 − e^(−r_max σ(o) Δt)`. A held controller output is
//! decoded against the current tick's budget, never a cached one. The
//! organism pays `w` from its budget (INV-05) and offers `ρ w` to the field
//! at its post-motion position (`post-motion`). The offer is staged in a
//! `flife_field::Deposits` and reaches the next field in phase 9
//! (`next-tick`), so no organism senses or absorbs it within the tick it
//! was made, the emitter included.
//!
//! The ledger records the transfer when the deposits are applied. It debits
//! `Σ w`, credits what the cells received, and charges the rest to
//! `emission-loss`. Because `ρ ≤ 1` and the field never receives more than
//! was offered, no emission loop can amplify spendable energy.
//!
//! An emission that spends the last unit of energy exhausts the emitter,
//! which then dies. Its deposit stands, because already-paid deposits keep
//! their stated effect (§8.2).

use flife_core::clock::{Tick, TickSpan};
use flife_core::ids::OrganismId;
use flife_core::ledger::{EnergyBudget, Payment};
use flife_core::numerics::{expm1_neg, sigmoid, tanh};
use flife_core::{CoreError, TimeStep, V2Feature, ValidatedConfig};
use flife_field::{Deposits, ExchangeKernel, FieldGrid, split_dual_carrier_emission};
use flife_neural::Emission;

/// The emission position this module implements.
pub const POST_MOTION: &str = "post-motion";

/// The deposit visibility this module implements: a deposit is first
/// sensed on the next tick.
pub const NEXT_TICK_VISIBILITY: &str = "next-tick";

/// Initial dimensionless secretory readiness `r_i ∈ [0, 1]` assigned identically
/// to founders, newborns, and introduced organisms (`V2-04`, `INV-07`).
/// Holds zero ecological energy (`INV-03`).
pub const INITIAL_SECRETORY_READINESS: f32 = 0.2;

/// Minimum speed magnitude for `wake-velocity-v1` unit-heading normalization;
/// slower organisms deposit at `x_post` with zero NaN risk (`V2-04`).
const WAKE_SPEED_THRESHOLD: f64 = 1e-12;

/// Surplus-gated emission floor (`V2-04`, `surplus-emission`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurplusFloorRule {
    /// Energy reserve `E_emit_floor` below which emission spends zero.
    pub energy_floor: f64,
}

/// Excitable refractory secretion relay mode (`V2-04`, `excitable-relay`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExcitableMode {
    /// Dimensionless readiness valve `r_i ∈ [0, 1]` (`vesicle-readiness-v1`).
    VesicleReadiness {
        /// Minimum readiness required to fire a burst (`r_fire`).
        fire_readiness_threshold: f64,
        /// Fraction of readiness depleted on a fired burst (`α_dep`).
        depletion_fraction: f64,
        /// Spend-rate multiplier during a fired burst (`>= 1.0`).
        burst_rate_multiplier: f64,
        /// Exponential recovery time constant `τ_rec > 0` in simulation seconds.
        recovery_time_seconds: f64,
    },
    /// Deterministic integer-tick refractory cooldown (`tick-cooldown-v1`).
    TickCooldown {
        /// Spend-rate multiplier during a fired burst (`>= 1.0`).
        burst_rate_multiplier: f64,
        /// Cooldown ticks after a fired burst.
        refractory_cooldown_ticks: u32,
    },
}

/// Direction control for `polarized-pump-v1` (`V2-04`, `wake-offset`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PumpDirection {
    /// Constant sign `+1.0` or `-1.0` (`fixed-sign-v1`).
    FixedSign(f64),
    /// Controller action `tanh(o_pump)` (`neural-output-v1`).
    NeuralOutput,
}

/// Deposit offset mode (`V2-04`, `wake-offset`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OffsetMode {
    /// Deposit behind post-motion velocity (`wake-velocity-v1`).
    WakeVelocity,
    /// Deposit along polarity or regularized energy gradient (`polarized-pump-v1`).
    PolarizedPump {
        /// Sign or neural pump control.
        direction: PumpDirection,
        /// Sensor `L_gradient` used to form `g_0 = 0.05 / L_gradient` when
        /// `V2-08` polarity is not active.
        gradient_length_scale: f64,
    },
}

/// Deposit offset rule (`V2-04`, `wake-offset`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmissionOffsetRule {
    /// Offset direction mode.
    pub mode: OffsetMode,
    /// Offset distance `δ` in `[0, min(h_x, h_y)]` world units.
    pub distance_world_units: f64,
}

/// Resolved `field-ecology-v2` emission rules (`V2-04`). All three features
/// default to `None` on `field-ecology-v1*` or when disabled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmissionV2Rules {
    /// Optional `surplus-emission` floor.
    pub surplus_floor: Option<SurplusFloorRule>,
    /// Optional `excitable-relay` valve or cooldown.
    pub excitable: Option<ExcitableMode>,
    /// Optional `wake-offset` / `polarized-pump-v1` offset.
    pub offset: Option<EmissionOffsetRule>,
    /// Base `[emission] maximum_fractional_rate_per_second` (`r_max`), used
    /// when computing burst spend fractions under `excitable-relay`.
    pub max_spend_rate: f64,
}

impl EmissionV2Rules {
    /// Baseline `field-ecology-v1` rules (all three v2 emission features off).
    #[must_use]
    pub const fn disabled(max_spend_rate: f64) -> Self {
        Self {
            surplus_floor: None,
            excitable: None,
            offset: None,
            max_spend_rate,
        }
    }

    /// Resolve `EmissionV2Rules` from a validated configuration.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for an unrecognized excitable or offset
    /// mode string; [`CoreError::NumericalInvalid`] for invalid bounds.
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let model = config.config();
        let max_spend_rate = model.emission.maximum_fractional_rate_per_second;
        let Some(v2) = model.v2.as_ref() else {
            return Ok(Self::disabled(max_spend_rate));
        };
        let surplus_floor = if v2.is_enabled(V2Feature::SurplusEmission) {
            let floor = v2.emission.surplus_floor.energy_floor.unwrap_or(0.0);
            if !floor.is_finite() || floor < 0.0 {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.emission.surplus_floor.energy_floor",
                    detail: format!("{floor} must be finite and nonnegative"),
                });
            }
            Some(SurplusFloorRule {
                energy_floor: floor + 0.0,
            })
        } else {
            None
        };
        let excitable = if v2.is_enabled(V2Feature::ExcitableRelay) {
            let ex = &v2.emission.excitable;
            match ex.mode.as_str() {
                "vesicle-readiness-v1" => Some(ExcitableMode::VesicleReadiness {
                    fire_readiness_threshold: ex.fire_readiness_threshold,
                    depletion_fraction: ex.depletion_fraction,
                    burst_rate_multiplier: ex.burst_rate_multiplier,
                    recovery_time_seconds: ex.recovery_time_seconds,
                }),
                "tick-cooldown-v1" => Some(ExcitableMode::TickCooldown {
                    burst_rate_multiplier: ex.burst_rate_multiplier,
                    refractory_cooldown_ticks: ex.refractory_cooldown_ticks,
                }),
                other => {
                    return Err(CoreError::UnknownAlgorithm {
                        role: "v2.emission.excitable.mode",
                        name: other.to_owned(),
                    });
                }
            }
        } else {
            None
        };
        let offset = if v2.is_enabled(V2Feature::WakeOffset) {
            let off = &v2.emission.offset;
            let distance = off.distance_world_units.unwrap_or(0.0);
            if !distance.is_finite() || distance < 0.0 {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.emission.offset.distance_world_units",
                    detail: format!("{distance} must be finite and nonnegative"),
                });
            }
            let mode = match off.mode.as_str() {
                "wake-velocity-v1" => OffsetMode::WakeVelocity,
                "polarized-pump-v1" => {
                    let direction = match off.pump_direction_mode.as_str() {
                        "fixed-sign-v1" => PumpDirection::FixedSign(off.pump_sign),
                        "neural-output-v1" => PumpDirection::NeuralOutput,
                        other => {
                            return Err(CoreError::UnknownAlgorithm {
                                role: "v2.emission.offset.pump_direction_mode",
                                name: other.to_owned(),
                            });
                        }
                    };
                    OffsetMode::PolarizedPump {
                        direction,
                        gradient_length_scale: model.sensors.gradient_length_scale_world_units,
                    }
                }
                other => {
                    return Err(CoreError::UnknownAlgorithm {
                        role: "v2.emission.offset.mode",
                        name: other.to_owned(),
                    });
                }
            };
            Some(EmissionOffsetRule {
                mode,
                distance_world_units: distance + 0.0,
            })
        } else {
            None
        };
        Ok(Self {
            surplus_floor,
            excitable,
            offset,
            max_spend_rate,
        })
    }

    /// Whether any of the three `V2-04` toggles is active.
    #[must_use]
    pub const fn any_enabled(&self) -> bool {
        self.surplus_floor.is_some() || self.excitable.is_some() || self.offset.is_some()
    }

    /// Configured energy floor (`0.0` when `surplus-emission` is disabled).
    #[must_use]
    pub const fn energy_floor(&self) -> f64 {
        match self.surplus_floor {
            Some(rule) => rule.energy_floor,
            None => 0.0,
        }
    }

    /// Resolve deposit coordinate, effective emission action, surplus floor,
    /// and next-tick excitable state for one organism at Phase 7 emission.
    ///
    /// Shared identically by the CPU reference stage and the Metal settle host
    /// preparation/replay path so both routes execute the exact same math.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for invalid readiness, nonfinite logits,
    /// or unwrapped positions; [`CoreError::Overflow`] if a cooldown tick
    /// overflows `u64`.
    pub fn plan(
        &self,
        grid: &FieldGrid,
        step: TimeStep,
        available: f64,
        exhausted: bool,
        emission: Option<&Emission>,
        ctx: &EmissionContext,
    ) -> Result<EmissionPlan, CoreError> {
        let energy_floor = self.energy_floor();
        let surplus = if energy_floor == 0.0 {
            available
        } else {
            (available - energy_floor).max(0.0)
        };
        let dt = step.seconds_per_tick();

        let (effective_emission, next_readiness, next_cooldown_deadline) = match self.excitable {
            None => (
                emission.copied(),
                ctx.secretory_readiness,
                ctx.emission_cooldown_deadline,
            ),
            Some(ExcitableMode::VesicleReadiness {
                fire_readiness_threshold,
                depletion_fraction,
                burst_rate_multiplier,
                recovery_time_seconds,
            }) => {
                let r = f64::from(ctx.secretory_readiness);
                if !r.is_finite() || !(0.0..=1.0).contains(&r) {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "secretory readiness",
                        detail: format!("{r} is outside [0, 1]"),
                    });
                }
                let recover = || -> Result<f32, CoreError> {
                    let alpha = expm1_neg(dt / recovery_time_seconds).ok_or_else(|| {
                        CoreError::NumericalInvalid {
                            quantity: "excitable recovery rate",
                            detail: format!("dt={dt}, tau={recovery_time_seconds} out of range"),
                        }
                    })?;
                    let recovered = (r + (1.0 - r) * alpha).min(1.0);
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "readiness is bounded in [0, 1] and stored in f32"
                    )]
                    Ok((recovered as f32).clamp(0.0, 1.0))
                };
                if let Some(em) = emission
                    && !exhausted
                    && surplus > 0.0
                    && r >= fire_readiness_threshold
                {
                    let sigma = sigmoid(ctx.raw_spend_logit).ok_or_else(|| {
                        CoreError::NumericalInvalid {
                            quantity: "emission spend logit",
                            detail: format!("{} is nonfinite", ctx.raw_spend_logit),
                        }
                    })?;
                    let burst_rate = self.max_spend_rate * burst_rate_multiplier * sigma;
                    let burst_frac =
                        expm1_neg(burst_rate * dt).ok_or_else(|| CoreError::NumericalInvalid {
                            quantity: "excitable burst spend fraction",
                            detail: format!("rate={burst_rate}, dt={dt} out of range"),
                        })?;
                    let f_eff = (r * burst_frac).clamp(0.0, 1.0);
                    let requested = (f_eff * surplus).min(available);
                    if requested > 0.0 {
                        #[expect(
                            clippy::cast_possible_truncation,
                            reason = "depleted readiness is bounded in [0, 1] and stored in f32"
                        )]
                        let depleted = (((1.0 - depletion_fraction) * r).clamp(0.0, 1.0)) as f32;
                        (
                            Some(Emission {
                                direction: em.direction,
                                spend_fraction: f_eff,
                            }),
                            depleted,
                            ctx.emission_cooldown_deadline,
                        )
                    } else {
                        (None, recover()?, ctx.emission_cooldown_deadline)
                    }
                } else {
                    (None, recover()?, ctx.emission_cooldown_deadline)
                }
            }
            Some(ExcitableMode::TickCooldown {
                burst_rate_multiplier,
                refractory_cooldown_ticks,
            }) => {
                if let Some(em) = emission
                    && !exhausted
                    && surplus > 0.0
                    && ctx.tick >= ctx.emission_cooldown_deadline
                {
                    let sigma = sigmoid(ctx.raw_spend_logit).ok_or_else(|| {
                        CoreError::NumericalInvalid {
                            quantity: "emission spend logit",
                            detail: format!("{} is nonfinite", ctx.raw_spend_logit),
                        }
                    })?;
                    let burst_rate = self.max_spend_rate * burst_rate_multiplier * sigma;
                    let f_eff =
                        expm1_neg(burst_rate * dt).ok_or_else(|| CoreError::NumericalInvalid {
                            quantity: "excitable burst spend fraction",
                            detail: format!("rate={burst_rate}, dt={dt} out of range"),
                        })?;
                    let requested = (f_eff * surplus).min(available);
                    if requested > 0.0 {
                        let next_deadline = ctx
                            .tick
                            .checked_add(TickSpan::new(u64::from(refractory_cooldown_ticks)))?;
                        (
                            Some(Emission {
                                direction: em.direction,
                                spend_fraction: f_eff,
                            }),
                            ctx.secretory_readiness,
                            next_deadline,
                        )
                    } else {
                        (
                            None,
                            ctx.secretory_readiness,
                            ctx.emission_cooldown_deadline,
                        )
                    }
                } else {
                    (
                        None,
                        ctx.secretory_readiness,
                        ctx.emission_cooldown_deadline,
                    )
                }
            }
        };

        let deposit_position = match (effective_emission.is_some(), self.offset) {
            (true, Some(offset)) if offset.distance_world_units > 0.0 => {
                compute_offset_position(grid, ctx, offset)?
            }
            _ => ctx.position,
        };

        Ok(EmissionPlan {
            deposit_position,
            effective_emission,
            energy_floor,
            next_readiness,
            next_cooldown_deadline,
        })
    }
}

fn compute_offset_position(
    grid: &FieldGrid,
    ctx: &EmissionContext,
    offset: EmissionOffsetRule,
) -> Result<[f64; 2], CoreError> {
    let d = offset.distance_world_units;
    match offset.mode {
        OffsetMode::WakeVelocity => {
            let [vx, vy] = ctx.velocity;
            if !vx.is_finite() || !vy.is_finite() {
                return Err(CoreError::NumericalInvalid {
                    quantity: "wake offset velocity",
                    detail: format!("[{vx}, {vy}] is not finite"),
                });
            }
            let speed = vx.hypot(vy);
            if speed > WAKE_SPEED_THRESHOLD {
                grid.wrap_position([
                    ctx.position[0] - d * (vx / speed),
                    ctx.position[1] - d * (vy / speed),
                ])
            } else {
                Ok(ctx.position)
            }
        }
        OffsetMode::PolarizedPump {
            direction,
            gradient_length_scale,
        } => {
            let axis = match ctx.polarity {
                Some(p) => {
                    if !p[0].is_finite() || !p[1].is_finite() {
                        return Err(CoreError::NumericalInvalid {
                            quantity: "pump polarity axis",
                            detail: format!("{p:?} is not finite"),
                        });
                    }
                    p
                }
                None => {
                    let [gx, gy] = ctx.energy_gradient;
                    if !gx.is_finite() || !gy.is_finite() {
                        return Err(CoreError::NumericalInvalid {
                            quantity: "pump energy gradient",
                            detail: format!("[{gx}, {gy}] is not finite"),
                        });
                    }
                    let g0 = 0.05 / gradient_length_scale;
                    let denom = (gx * gx + gy * gy + g0 * g0).sqrt();
                    [gx / denom, gy / denom]
                }
            };
            let sign = match direction {
                PumpDirection::FixedSign(s) => s,
                PumpDirection::NeuralOutput => {
                    let logit = ctx.raw_pump_logit.ok_or_else(|| CoreError::StateConflict {
                        operation: "polarized pump emission",
                        detail: "neural-output-v1 requires wake_pump controller output".to_owned(),
                    })?;
                    tanh(logit).ok_or_else(|| CoreError::NumericalInvalid {
                        quantity: "pump direction logit",
                        detail: format!("{logit} is not finite"),
                    })?
                }
            };
            grid.wrap_position([
                ctx.position[0] + d * sign * axis[0],
                ctx.position[1] + d * sign * axis[1],
            ])
        }
    }
}

/// Per-organism post-motion state consumed by [`EmissionV2Rules::plan`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmissionContext {
    /// Post-motion wrapped world position `x_post`.
    pub position: [f64; 2],
    /// Post-motion velocity `v_post`.
    pub velocity: [f64; 2],
    /// Sensed unsaturated energy gradient `∇q` (fallback axis for
    /// `polarized-pump-v1` when `V2-08` polarity is not active).
    pub energy_gradient: [f64; 2],
    /// Optional apical polarity vector `p_i` populated by `V2-08` (`None`
    /// when `pcp-polarity` is disabled).
    pub polarity: Option<[f64; 2]>,
    /// Raw controller spend output `o_35` (`0.0` under chemistry-only).
    pub raw_spend_logit: f64,
    /// Optional raw controller pump output `o_pump` (`neural-output-v1`).
    pub raw_pump_logit: Option<f64>,
    /// Current tick `n` (for `tick-cooldown-v1`).
    pub tick: Tick,
    /// Committed secretory readiness `r_i ∈ [0, 1]` entering tick `n`.
    pub secretory_readiness: f32,
    /// Committed emission cooldown deadline entering tick `n`.
    pub emission_cooldown_deadline: Tick,
}

/// Host-resolved emission plan produced by [`EmissionV2Rules::plan`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmissionPlan {
    /// Wrapped world position where the deposit lands.
    pub deposit_position: [f64; 2],
    /// Effective emission action after excitable gating (`None` if silent or
    /// refractory).
    pub effective_emission: Option<Emission>,
    /// Surplus floor `E_emit_floor` (`0.0` when `surplus-emission` is off).
    pub energy_floor: f64,
    /// Updated secretory readiness `r'_i` to commit if the organism survives.
    pub next_readiness: f32,
    /// Updated emission cooldown deadline to commit if the organism survives.
    pub next_cooldown_deadline: Tick,
}

/// The `[emission]` transfer rule.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmissionPolicy {
    efficiency: f64,
}

impl EmissionPolicy {
    /// A transfer efficiency `ρ ∈ [0, 1]`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] outside `[0, 1]`.
    pub fn new(efficiency: f64) -> Result<Self, CoreError> {
        if !(0.0..=1.0).contains(&efficiency) {
            return Err(CoreError::NumericalInvalid {
                quantity: "emission.transfer_efficiency",
                detail: format!("{efficiency} is outside [0, 1]"),
            });
        }
        Ok(Self {
            efficiency: efficiency + 0.0,
        })
    }

    /// The policy of a validated configuration.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for another position or visibility;
    /// otherwise as [`EmissionPolicy::new`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let emission = &config.config().emission;
        for (role, name, implemented) in [
            ("emission.position", &emission.position, POST_MOTION),
            (
                "emission.visibility",
                &emission.visibility,
                NEXT_TICK_VISIBILITY,
            ),
        ] {
            if name != implemented {
                return Err(CoreError::UnknownAlgorithm {
                    role,
                    name: name.clone(),
                });
            }
        }
        Self::new(emission.transfer_efficiency)
    }

    /// `ρ`.
    #[must_use]
    pub const fn efficiency(&self) -> f64 {
        self.efficiency
    }

    /// Settle `organism`'s `emission` at the emission stage under `grid-cic-v1`:
    /// pay `f E` from `budget` and stage `ρ f E` at `position`, its post-motion
    /// position, in `deposits`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a spending fraction outside
    /// `[0, 1]` or an invalid direction or position. A refused emission
    /// changes neither the budget nor the deposits.
    pub fn emit(
        &self,
        grid: &FieldGrid,
        organism: OrganismId,
        position: [f64; 2],
        emission: Option<&Emission>,
        budget: &mut EnergyBudget,
        deposits: &mut Deposits,
    ) -> Result<EmissionSettlement, CoreError> {
        self.emit_with(
            grid,
            ExchangeKernel::GridCicV1,
            organism,
            position,
            emission,
            budget,
            deposits,
        )
    }

    /// [`EmissionPolicy::emit`] under an explicit [`ExchangeKernel`] (FL-053).
    ///
    /// # Errors
    /// As [`EmissionPolicy::emit`].
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors EmissionPolicy::emit with the explicit exchange kernel"
    )]
    pub fn emit_with(
        &self,
        grid: &FieldGrid,
        exchange: ExchangeKernel,
        organism: OrganismId,
        position: [f64; 2],
        emission: Option<&Emission>,
        budget: &mut EnergyBudget,
        deposits: &mut Deposits,
    ) -> Result<EmissionSettlement, CoreError> {
        self.emit_with_floor(
            grid, exchange, organism, position, emission, 0.0, budget, deposits,
        )
    }

    /// [`EmissionPolicy::emit_with`] with an explicit surplus `energy_floor`
    /// (`0.0` on `field-ecology-v1` and when `surplus-emission` is disabled;
    /// `surplus = max(available - energy_floor, 0.0)` under `V2-04`).
    ///
    /// # Errors
    /// As [`EmissionPolicy::emit`], or [`CoreError::NumericalInvalid`] for a
    /// negative or nonfinite `energy_floor`.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors EmissionPolicy::emit_with plus the V2-04 surplus floor"
    )]
    pub fn emit_with_floor(
        &self,
        grid: &FieldGrid,
        exchange: ExchangeKernel,
        organism: OrganismId,
        position: [f64; 2],
        emission: Option<&Emission>,
        energy_floor: f64,
        budget: &mut EnergyBudget,
        deposits: &mut Deposits,
    ) -> Result<EmissionSettlement, CoreError> {
        if !energy_floor.is_finite() || energy_floor < 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "emission energy floor",
                detail: format!("{energy_floor} is not finite and nonnegative"),
            });
        }
        let available = budget.remaining();
        let nothing = EmissionSettlement {
            available,
            cost: Payment {
                requested: 0.0,
                paid: 0.0,
            },
            offered: 0.0,
        };
        let Some(emission) = emission else {
            return Ok(nothing);
        };
        let fraction = emission.spend_fraction;
        if !(0.0..=1.0).contains(&fraction) {
            return Err(CoreError::NumericalInvalid {
                quantity: "emission spending fraction",
                detail: format!("{fraction} is outside [0, 1]"),
            });
        }
        if budget.is_exhausted() {
            return Ok(nothing);
        }
        let surplus = if energy_floor == 0.0 {
            available
        } else {
            (available - energy_floor).max(0.0)
        };
        // `f ≤ 1` and `surplus ≤ available`, so the request never exceeds the
        // balance: emission is always affordable and pays in full.
        let requested = (fraction * surplus).min(available);
        if requested == 0.0 {
            return Ok(nothing);
        }
        let offered = self.efficiency * requested;
        deposits.push_with(
            grid,
            exchange,
            organism,
            position,
            requested,
            offered,
            &emission.direction,
        )?;
        let cost = budget.pay(requested)?;
        Ok(EmissionSettlement {
            available,
            cost,
            offered,
        })
    }

    /// Settle one organism's emission under `V2-04` (`surplus-emission`,
    /// `excitable-relay`, `wake-offset`), returning both the ledger settlement
    /// and the [`EmissionPlan`] (carrying the next-tick excitable state).
    ///
    /// # Errors
    /// As [`EmissionV2Rules::plan`] and [`EmissionPolicy::emit_with_floor`].
    #[expect(
        clippy::too_many_arguments,
        reason = "Phase 7 emission stage inputs plus V2-04 rules and context"
    )]
    pub fn emit_v2(
        &self,
        grid: &FieldGrid,
        exchange: ExchangeKernel,
        step: TimeStep,
        v2: &EmissionV2Rules,
        organism: OrganismId,
        emission: Option<&Emission>,
        ctx: &EmissionContext,
        budget: &mut EnergyBudget,
        deposits: &mut Deposits,
    ) -> Result<(EmissionSettlement, EmissionPlan), CoreError> {
        let plan = v2.plan(
            grid,
            step,
            budget.remaining(),
            budget.is_exhausted(),
            emission,
            ctx,
        )?;
        let settlement = self.emit_with_floor(
            grid,
            exchange,
            organism,
            plan.deposit_position,
            plan.effective_emission.as_ref(),
            plan.energy_floor,
            budget,
            deposits,
        )?;
        Ok((settlement, plan))
    }

    /// Dual-carrier (`V2-15`) variant of [`EmissionPolicy::emit_with_floor`]:
    /// splits the single paid emission `requested` between Carrier 0 (`24`
    /// metabolic channels, `deposits`) and Carrier 1 (`8` non-destructive trail
    /// channels, `trail_deposits`) via [`split_dual_carrier_emission`].
    ///
    /// # Errors
    /// As [`EmissionPolicy::emit_with_floor`] and [`split_dual_carrier_emission`].
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors emit_with_floor plus dual-carrier trail share and trail deposits"
    )]
    pub fn emit_dual_with_floor(
        &self,
        grid: &FieldGrid,
        exchange: ExchangeKernel,
        organism: OrganismId,
        position: [f64; 2],
        emission: Option<&Emission>,
        energy_floor: f64,
        trail_share: f64,
        budget: &mut EnergyBudget,
        deposits: &mut Deposits,
        trail_deposits: &mut Deposits,
    ) -> Result<EmissionSettlement, CoreError> {
        if !energy_floor.is_finite() || energy_floor < 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "emission energy floor",
                detail: format!("{energy_floor} is not finite and nonnegative"),
            });
        }
        let available = budget.remaining();
        let nothing = EmissionSettlement {
            available,
            cost: Payment {
                requested: 0.0,
                paid: 0.0,
            },
            offered: 0.0,
        };
        let Some(emission) = emission else {
            return Ok(nothing);
        };
        let fraction = emission.spend_fraction;
        if !(0.0..=1.0).contains(&fraction) {
            return Err(CoreError::NumericalInvalid {
                quantity: "emission spending fraction",
                detail: format!("{fraction} is outside [0, 1]"),
            });
        }
        if budget.is_exhausted() {
            return Ok(nothing);
        }
        let surplus = if energy_floor == 0.0 {
            available
        } else {
            (available - energy_floor).max(0.0)
        };
        let requested = (fraction * surplus).min(available);
        if requested == 0.0 {
            return Ok(nothing);
        }
        let split = split_dual_carrier_emission(
            requested,
            self.efficiency,
            trail_share,
            &emission.direction,
        )?;
        deposits.push_with(
            grid,
            exchange,
            organism,
            position,
            split.paid_0,
            split.offered_0,
            &split.direction_0,
        )?;
        trail_deposits.push_with(
            grid,
            exchange,
            organism,
            position,
            split.paid_1,
            split.offered_1,
            &split.direction_1,
        )?;
        let cost = budget.pay(requested)?;
        Ok(EmissionSettlement {
            available,
            cost,
            offered: split.offered_0 + split.offered_1,
        })
    }

    /// Dual-carrier (`V2-15`) variant of [`EmissionPolicy::emit_v2`].
    ///
    /// # Errors
    /// As [`EmissionV2Rules::plan`] and [`EmissionPolicy::emit_dual_with_floor`].
    #[expect(
        clippy::too_many_arguments,
        reason = "Phase 7 dual-carrier emission stage inputs plus V2-04 rules and context"
    )]
    pub fn emit_v2_dual(
        &self,
        grid: &FieldGrid,
        exchange: ExchangeKernel,
        step: TimeStep,
        v2: &EmissionV2Rules,
        organism: OrganismId,
        emission: Option<&Emission>,
        ctx: &EmissionContext,
        trail_share: f64,
        budget: &mut EnergyBudget,
        deposits: &mut Deposits,
        trail_deposits: &mut Deposits,
    ) -> Result<(EmissionSettlement, EmissionPlan), CoreError> {
        let plan = v2.plan(
            grid,
            step,
            budget.remaining(),
            budget.is_exhausted(),
            emission,
            ctx,
        )?;
        let settlement = self.emit_dual_with_floor(
            grid,
            exchange,
            organism,
            plan.deposit_position,
            plan.effective_emission.as_ref(),
            plan.energy_floor,
            trail_share,
            budget,
            deposits,
            trail_deposits,
        )?;
        Ok((settlement, plan))
    }
}

/// What one organism's emission stage paid and offered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EmissionSettlement {
    /// The budget at the emission stage, after motion.
    pub available: f64,
    /// `w = f E`, requested and paid; emission always pays in full.
    pub cost: Payment,
    /// `ρ w`, offered to the field.
    pub offered: f64,
}

impl EmissionSettlement {
    /// Whether anything was emitted.
    #[must_use]
    pub fn emitted(&self) -> bool {
        self.cost.paid > 0.0
    }
}
