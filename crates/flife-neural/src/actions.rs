//! The action ABI: decoding the 36 raw outputs of a linear final layer
//! (plan §6.2, §6.3, §7.4, A.1, Appendix C.11).
//!
//! The controller's final affine layer is linear. Sigmoid is applied
//! exactly once, here, with the sign-stable branch of
//! `flife_core::numerics::sigmoid`. Applying `tanh` first would confine
//! every gate to about `[0.269, 0.731]`.
//!
//! Output order follows §6.3's table:
//! - `0`: reproduction. The attempt rate is `λ = λ_max σ(o)` and the
//!   probability of an attempt this tick is `p = 1 − e^(−λΔt)`, through the
//!   stable `expm1` (§7.4, C.11).
//! - `1`: child energy share. `β = β_min + (β_max − β_min) σ(o)`, clamped to
//!   `[β_min, β_max]` as computed.
//! - `2`: emission gate, `σ(o)`. Emission needs `σ(o) > θ_emit`.
//! - `3..35`: emission direction, the 32 raw outputs normalized to a unit
//!   vector. Under `no-emission-no-charge`, a norm at or below the zero
//!   threshold means no emission and no charge.
//! - `35`: emission spending. `f = 1 − e^(−r_max σ(o) Δt)` of the energy
//!   available at the emission stage, through the stable `expm1` (A.1).
//!
//! Everything is decoded before any state changes (§6.3). A held output is
//! decoded again every tick against that tick's budget; this module returns
//! fractions, never amounts.

use flife_core::numerics::{expm1_neg, sigmoid};
use flife_core::{CoreError, TimeStep, ValidatedConfig};
use flife_field::settle::EmissionAction;
use flife_field::sources::unit_direction;
use flife_field::state::CHANNELS;

/// Controller outputs.
pub const ACTION_WIDTH: usize = 4 + CHANNELS;

/// Index of the reproduction output.
pub const REPRODUCTION: usize = 0;
/// Index of the child-share output.
pub const CHILD_SHARE: usize = 1;
/// Index of the emission-gate output.
pub const EMISSION_GATE: usize = 2;
/// Index of the first emission-direction output; channel `k` is at
/// `EMISSION_DIRECTION + k`.
pub const EMISSION_DIRECTION: usize = 3;
/// Index of the emission-spending output.
pub const EMISSION_SPEND: usize = EMISSION_DIRECTION + CHANNELS;

/// The zero-direction policy this module implements.
pub const NO_EMISSION_NO_CHARGE: &str = "no-emission-no-charge";

/// The decoding constants of `[reproduction]` and `[emission]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActionDecoding {
    maximum_attempt_rate: f64,
    minimum_child_fraction: f64,
    maximum_child_fraction: f64,
    gate_threshold: f64,
    maximum_spend_rate: f64,
    direction_zero_threshold: f64,
}

impl ActionDecoding {
    /// Constants: `λ_max ≥ 0` (per second), child fractions
    /// `0 < β_min ≤ β_max < 1`, gate threshold `θ ∈ [0, 1)`, `r_max ≥ 0` (per
    /// second) and a zero-direction threshold `≥ 0`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a value outside its range.
    pub fn new(
        maximum_attempt_rate: f64,
        child_fractions: [f64; 2],
        gate_threshold: f64,
        maximum_spend_rate: f64,
        direction_zero_threshold: f64,
    ) -> Result<Self, CoreError> {
        let [minimum_child_fraction, maximum_child_fraction] = child_fractions;
        for (quantity, value, admissible) in [
            (
                "reproduction.maximum_attempt_rate_per_second",
                maximum_attempt_rate,
                maximum_attempt_rate >= 0.0,
            ),
            (
                "reproduction.minimum_child_fraction",
                minimum_child_fraction,
                minimum_child_fraction > 0.0 && minimum_child_fraction <= maximum_child_fraction,
            ),
            (
                "reproduction.maximum_child_fraction",
                maximum_child_fraction,
                maximum_child_fraction < 1.0,
            ),
            (
                "emission.gate_threshold",
                gate_threshold,
                (0.0..1.0).contains(&gate_threshold),
            ),
            (
                "emission.maximum_fractional_rate_per_second",
                maximum_spend_rate,
                maximum_spend_rate >= 0.0,
            ),
            (
                "emission.direction_zero_threshold",
                direction_zero_threshold,
                direction_zero_threshold >= 0.0,
            ),
        ] {
            if !value.is_finite() || !admissible {
                return Err(CoreError::NumericalInvalid {
                    quantity,
                    detail: format!("{value} is out of range"),
                });
            }
        }
        Ok(Self {
            maximum_attempt_rate,
            minimum_child_fraction,
            maximum_child_fraction,
            gate_threshold,
            maximum_spend_rate,
            direction_zero_threshold,
        })
    }

    /// The constants of a validated configuration.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for another zero-direction policy;
    /// otherwise as [`ActionDecoding::new`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let model = config.config();
        if model.emission.zero_direction_policy != NO_EMISSION_NO_CHARGE {
            return Err(CoreError::UnknownAlgorithm {
                role: "emission.zero_direction_policy",
                name: model.emission.zero_direction_policy.clone(),
            });
        }
        Self::new(
            model.reproduction.maximum_attempt_rate_per_second,
            [
                model.reproduction.minimum_child_fraction,
                model.reproduction.maximum_child_fraction,
            ],
            model.emission.gate_threshold,
            model.emission.maximum_fractional_rate_per_second,
            model.emission.direction_zero_threshold,
        )
    }
}

/// An emission proposal, before any budget is consulted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Emission {
    /// The unit direction, `||a||₂ ≤ 1` as computed.
    pub direction: [f64; CHANNELS],
    /// `f = 1 − e^(−r_max σ(o) Δt)`: the fraction of the energy available
    /// at the emission stage to spend.
    pub spend_fraction: f64,
}

impl From<Emission> for EmissionAction {
    /// The same emission as phase 6's port carries it (FL-032).
    fn from(emission: Emission) -> Self {
        Self {
            direction: emission.direction,
            spend_fraction: emission.spend_fraction,
        }
    }
}

/// One tick's decoded actions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecodedActions {
    /// `λ = λ_max σ(o_split)`, attempts per second.
    pub attempt_rate: f64,
    /// `p = 1 − e^(−λΔt)`, the probability of one attempt this tick.
    pub attempt_probability: f64,
    /// `β`, the child's share of the energy after the birth cost.
    pub child_fraction: f64,
    /// `σ(o_gate)`.
    pub emission_gate: f64,
    /// The emission proposal: present when the gate is open and the
    /// direction is valid.
    pub emission: Option<Emission>,
}

/// Decode `raw`, the linear outputs of one controller evaluation, for a
/// tick of `step`.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for a nonfinite output, or a direction
/// whose norm overflows.
pub fn decode(
    raw: &[f64; ACTION_WIDTH],
    step: TimeStep,
    decoding: &ActionDecoding,
) -> Result<DecodedActions, CoreError> {
    if let Some((index, value)) = raw.iter().enumerate().find(|(_, value)| !value.is_finite()) {
        return Err(CoreError::NumericalInvalid {
            quantity: "controller output",
            detail: format!("output {index} is {value}"),
        });
    }
    let output = |index: usize| raw.get(index).copied().unwrap_or(0.0);
    let squash = |index: usize| {
        sigmoid(output(index)).ok_or_else(|| CoreError::NumericalInvalid {
            quantity: "controller output",
            detail: format!("σ(output {index}) is undefined"),
        })
    };
    let interval = |rate: f64| {
        expm1_neg(rate * step.seconds_per_tick()).ok_or_else(|| CoreError::NumericalInvalid {
            quantity: "decoded rate",
            detail: format!("{rate} per second is out of range"),
        })
    };

    let attempt_rate = decoding.maximum_attempt_rate * squash(REPRODUCTION)?;
    let attempt_probability = interval(attempt_rate)?;
    let span = decoding.maximum_child_fraction - decoding.minimum_child_fraction;
    let child_fraction = (decoding.minimum_child_fraction + span * squash(CHILD_SHARE)?).clamp(
        decoding.minimum_child_fraction,
        decoding.maximum_child_fraction,
    );
    let emission_gate = squash(EMISSION_GATE)?;

    let mut direction = [0.0; CHANNELS];
    for (slot, index) in direction.iter_mut().zip(EMISSION_DIRECTION..) {
        *slot = output(index);
    }
    let mut squared = 0.0;
    for component in direction {
        squared += component * component;
    }
    let norm = squared.sqrt();
    if !norm.is_finite() {
        return Err(CoreError::NumericalInvalid {
            quantity: "emission direction",
            detail: "the raw direction's norm overflows".into(),
        });
    }
    let emission = if emission_gate > decoding.gate_threshold
        && norm > decoding.direction_zero_threshold
        && norm > 0.0
    {
        Some(Emission {
            direction: unit_direction(&direction)?,
            spend_fraction: interval(decoding.maximum_spend_rate * squash(EMISSION_SPEND)?)?,
        })
    } else {
        None
    };
    Ok(DecodedActions {
        attempt_rate,
        attempt_probability,
        child_fraction,
        emission_gate,
        emission,
    })
}
