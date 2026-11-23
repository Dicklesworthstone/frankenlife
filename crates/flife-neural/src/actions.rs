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

use std::ops::Range;

use flife_core::numerics::{expm1_neg, shrink_to_left_fold_norm_limit, sigmoid, tanh};
use flife_core::{CoreError, TimeStep, V2Abi, ValidatedConfig};
use flife_field::settle::{EmissionAction, MotorSteeringInputs};
use flife_field::sources::unit_direction;
use flife_field::state::CHANNELS;

use crate::controller::{
    INDEPENDENT_DENSE_MLP_V1, INDEPENDENT_RECURRENT_MLP_V1, MAXIMUM_RECURRENT_STATE_WIDTH,
};

/// Baseline `field-ecology-v1` controller outputs (`4 + 32 = 36`).
pub const ACTION_WIDTH: usize = 4 + CHANNELS;

/// Maximum action outputs stored in `HeldOutput.outputs` across all
/// `field-ecology-v2` feature combinations.
pub const MAX_ACTION_WIDTH: usize = 48;

/// Maximum total linear output width of a controller, including up to
/// `2 * MAXIMUM_RECURRENT_STATE_WIDTH` (`32`) trailing recurrent candidate and
/// gate logits (`48 + 32 = 80`).
pub const MAX_CONTROLLER_OUTPUT_WIDTH: usize = MAX_ACTION_WIDTH + 2 * MAXIMUM_RECURRENT_STATE_WIDTH;

/// Number of extra outputs appended by `so2-motor`: `(m_e, m_c, m_perp, m_prop)`.
pub const SO2_MOTOR_OUTPUTS: usize = 4;

/// Number of extra outputs appended by `so2-budding`: `(o_rad, o_tan)`.
pub const SO2_BUDDING_OUTPUTS: usize = 2;

/// Number of extra outputs appended by `emt-gate`: `o_adh`.
pub const EMT_GATE_OUTPUTS: usize = 1;

/// Number of extra outputs appended by `wake-offset` (`polarized-pump-v1` +
/// `neural-output-v1`): `o_pump`.
pub const WAKE_PUMP_OUTPUTS: usize = 1;

/// Pure scientific action and raw controller output layout (V2-01).
///
/// Fixed slot order:
/// - `0..36`: baseline `field-ecology-v1` action logits (immutable);
/// - `+4` if `so2-motor`: `(m_e, m_c, m_perp, m_prop)`;
/// - `+2` if `so2-budding`: `(o_rad, o_tan)`;
/// - `+1` if `emt-gate`: `o_adh`;
/// - `+1` if `wake-offset` (`polarized-pump-v1` + `neural-output-v1`): `o_pump`;
/// - trailing `+2*S` if recurrent / `epigenetic-recurrence` (consumed into
///   `HeldOutput.state`, not stored in `HeldOutput.outputs`):
///   `cand[0..S]`, `gate[0..S]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ActionLayout {
    so2_motor: Option<usize>,
    so2_budding: Option<usize>,
    emt_gate: Option<usize>,
    wake_pump: Option<usize>,
    action_width: usize,
    recurrent_state_width: usize,
}

impl ActionLayout {
    /// Baseline stateless `field-ecology-v1` layout (`36` actions, `36` raw outputs).
    #[must_use]
    pub const fn v1() -> Self {
        Self {
            so2_motor: None,
            so2_budding: None,
            emt_gate: None,
            wake_pump: None,
            action_width: ACTION_WIDTH,
            recurrent_state_width: 0,
        }
    }

    /// `field-ecology-v1` layout with FL-054 recurrent state width `state_width`
    /// (`36` actions, `36 + 2S` raw outputs).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] if `state_width` exceeds
    /// [`MAXIMUM_RECURRENT_STATE_WIDTH`].
    pub fn v1_recurrent(state_width: usize) -> Result<Self, CoreError> {
        Self::new(false, false, false, false, state_width)
    }

    /// Build a canonical [`ActionLayout`] from the active layout-affecting
    /// feature flags and recurrent state width `S`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] if `recurrent_state_width` exceeds
    /// [`MAXIMUM_RECURRENT_STATE_WIDTH`], `action_width` exceeds
    /// [`MAX_ACTION_WIDTH`], or `raw_output_width` exceeds
    /// [`MAX_CONTROLLER_OUTPUT_WIDTH`].
    pub fn new(
        so2_motor: bool,
        so2_budding: bool,
        emt_gate: bool,
        wake_pump: bool,
        recurrent_state_width: usize,
    ) -> Result<Self, CoreError> {
        if recurrent_state_width > MAXIMUM_RECURRENT_STATE_WIDTH {
            return Err(CoreError::NumericalInvalid {
                quantity: "controller.recurrent_state_width",
                detail: format!(
                    "recurrent state width {recurrent_state_width} exceeds maximum \
                     {MAXIMUM_RECURRENT_STATE_WIDTH}"
                ),
            });
        }
        let mut cursor = ACTION_WIDTH;
        let so2_motor = if so2_motor {
            let start = cursor;
            cursor += SO2_MOTOR_OUTPUTS;
            Some(start)
        } else {
            None
        };
        let so2_budding = if so2_budding {
            let start = cursor;
            cursor += SO2_BUDDING_OUTPUTS;
            Some(start)
        } else {
            None
        };
        let emt_gate = if emt_gate {
            let start = cursor;
            cursor += EMT_GATE_OUTPUTS;
            Some(start)
        } else {
            None
        };
        let wake_pump = if wake_pump {
            let start = cursor;
            cursor += WAKE_PUMP_OUTPUTS;
            Some(start)
        } else {
            None
        };
        let action_width = cursor;
        if action_width > MAX_ACTION_WIDTH {
            return Err(CoreError::NumericalInvalid {
                quantity: "controller.action_width",
                detail: format!("action width {action_width} exceeds maximum {MAX_ACTION_WIDTH}"),
            });
        }
        let raw_output_width = action_width + 2 * recurrent_state_width;
        if raw_output_width > MAX_CONTROLLER_OUTPUT_WIDTH {
            return Err(CoreError::NumericalInvalid {
                quantity: "controller.output_width",
                detail: format!(
                    "raw output width {raw_output_width} exceeds maximum \
                     {MAX_CONTROLLER_OUTPUT_WIDTH}"
                ),
            });
        }
        Ok(Self {
            so2_motor,
            so2_budding,
            emt_gate,
            wake_pump,
            action_width,
            recurrent_state_width,
        })
    }

    /// Build an [`ActionLayout`] directly from `flife-core`'s canonical
    /// [`V2Abi`].
    ///
    /// # Errors
    /// As [`ActionLayout::new`] if `abi` carries an out-of-range state width
    /// or output width.
    pub fn from_v2_abi(abi: V2Abi) -> Result<Self, CoreError> {
        let state_width =
            usize::try_from(abi.recurrent_state_width).map_err(|_| CoreError::Overflow {
                quantity: "controller.recurrent_state_width",
            })?;
        let layout = Self::new(
            abi.so2_motor_actions,
            abi.so2_budding_actions,
            abi.emt_gate_action,
            abi.wake_pump_action,
            state_width,
        )?;
        if layout.action_width() != abi.action_width as usize
            || layout.raw_output_width() != abi.raw_output_width as usize
        {
            return Err(CoreError::LayoutMismatch {
                buffer: "action layout",
                detail: format!(
                    "V2Abi ({}/{}) disagrees with ActionLayout ({}/{})",
                    abi.action_width,
                    abi.raw_output_width,
                    layout.action_width(),
                    layout.raw_output_width()
                ),
            });
        }
        Ok(layout)
    }

    /// Resolve the action layout of a validated configuration (`v1` or `v2`).
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for an unsupported controller family, or
    /// [`CoreError::NumericalInvalid`] for an out-of-range recurrent width.
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let controller = &config.config().controller;
        match controller.family.as_str() {
            INDEPENDENT_DENSE_MLP_V1 => {}
            INDEPENDENT_RECURRENT_MLP_V1 => {
                let width = controller.recurrent_state_width.ok_or_else(|| {
                    CoreError::NumericalInvalid {
                        quantity: "controller.recurrent_state_width",
                        detail: "independent-recurrent-mlp-v1 requires recurrent_state_width"
                            .to_owned(),
                    }
                })?;
                if width == 0 {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "controller.recurrent_state_width",
                        detail: format!(
                            "recurrent state width must lie in 1..={MAXIMUM_RECURRENT_STATE_WIDTH}, \
                             got 0"
                        ),
                    });
                }
            }
            other => {
                return Err(CoreError::UnknownAlgorithm {
                    role: "controller.family",
                    name: other.to_owned(),
                });
            }
        }
        Self::from_v2_abi(config.v2_abi())
    }

    /// Number of action logits stored in `HeldOutput.outputs` (`36..=48`).
    #[must_use]
    pub const fn action_width(&self) -> usize {
        self.action_width
    }

    /// Recurrent internal state width `S` (`0..=16`).
    #[must_use]
    pub const fn recurrent_state_width(&self) -> usize {
        self.recurrent_state_width
    }

    /// Total linear output width of the controller (`action_width + 2 * S`, `<= 80`).
    #[must_use]
    pub const fn raw_output_width(&self) -> usize {
        self.action_width + 2 * self.recurrent_state_width
    }

    /// Start index of the 4 `so2-motor` outputs `(m_e, m_c, m_perp, m_prop)`,
    /// if active.
    #[must_use]
    pub const fn so2_motor(&self) -> Option<usize> {
        self.so2_motor
    }

    /// Contiguous range of the 4 `so2-motor` outputs `(m_e, m_c, m_perp, m_prop)`,
    /// if active (`V2-05`).
    #[must_use]
    pub const fn motor_slots(&self) -> Option<Range<usize>> {
        match self.so2_motor {
            Some(start) => Some(start..(start + SO2_MOTOR_OUTPUTS)),
            None => None,
        }
    }

    /// Start index of the 2 `so2-budding` outputs `(o_rad, o_tan)`, if active.
    #[must_use]
    pub const fn so2_budding(&self) -> Option<usize> {
        self.so2_budding
    }

    /// Contiguous range of the 2 `so2-budding` outputs `(o_rad, o_tan)`,
    /// if active (`V2-06`).
    #[must_use]
    pub const fn budding_slots(&self) -> Option<Range<usize>> {
        match self.so2_budding {
            Some(start) => Some(start..(start + SO2_BUDDING_OUTPUTS)),
            None => None,
        }
    }

    /// Index of the `emt-gate` output `o_adh`, if active.
    #[must_use]
    pub const fn emt_gate(&self) -> Option<usize> {
        self.emt_gate
    }

    /// Index of the `emt-gate` output `o_adh`, if active (`V2-08`).
    #[must_use]
    pub const fn emt_slot(&self) -> Option<usize> {
        self.emt_gate
    }

    /// Index of the `wake-offset` neural pump output `o_pump`, if active.
    #[must_use]
    pub const fn wake_pump(&self) -> Option<usize> {
        self.wake_pump
    }

    /// Index of the `wake-offset` neural pump output `o_pump`, if active (`V2-04`).
    #[must_use]
    pub const fn pump_slot(&self) -> Option<usize> {
        self.wake_pump
    }

    /// Range of the `S` recurrent candidate pre-activations `cand[0..S]`.
    #[must_use]
    pub const fn recurrent_candidates(&self) -> Range<usize> {
        self.action_width..(self.action_width + self.recurrent_state_width)
    }

    /// Range of the `S` recurrent update-gate pre-activations `gate[0..S]`.
    #[must_use]
    pub const fn recurrent_gates(&self) -> Range<usize> {
        (self.action_width + self.recurrent_state_width)..self.raw_output_width()
    }

    /// Range of the full `2 * S` recurrent output tail `[cand..., gate...]`.
    #[must_use]
    pub const fn recurrent_tail(&self) -> Range<usize> {
        self.action_width..self.raw_output_width()
    }
}

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

/// Decoded `so2-motor` (`V2-05`) SO(2)-equivariant steering controls in `[-1, 1]`,
/// each decoded via host-independent [`flife_core::numerics::tanh`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MotorSteering {
    /// `m_e = tanh(o_0)`: energy-gradient chemotaxis modulation.
    pub energy_gradient: f64,
    /// `m_c = tanh(o_1)`: chemical-gradient chemotaxis modulation.
    pub chemical_gradient: f64,
    /// `m_perp = tanh(o_2)`: chiral contour-following modulation (`g^⊥`).
    pub chiral_contour: f64,
    /// `m_prop = tanh(o_3)`: guarded heading-propulsion modulation (`ĥ`).
    pub heading_propulsion: f64,
}

impl MotorSteering {
    /// Neutral zero steering (recovers baseline static chemotaxis bit-for-bit).
    pub const ZERO: Self = Self {
        energy_gradient: 0.0,
        chemical_gradient: 0.0,
        chiral_contour: 0.0,
        heading_propulsion: 0.0,
    };

    /// Decode the four linear `so2-motor` logits `[o_e, o_c, o_perp, o_prop]`
    /// via host-independent [`flife_core::numerics::tanh`].
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] if any logit is nonfinite.
    pub fn decode(raw: [f64; SO2_MOTOR_OUTPUTS]) -> Result<Self, CoreError> {
        let squash = |index: usize, value: f64| {
            tanh(value).ok_or_else(|| CoreError::NumericalInvalid {
                quantity: "so2-motor output",
                detail: format!("motor output {index} is {value}"),
            })
        };
        Ok(Self {
            energy_gradient: squash(0, raw[0])?,
            chemical_gradient: squash(1, raw[1])?,
            chiral_contour: squash(2, raw[2])?,
            heading_propulsion: squash(3, raw[3])?,
        })
    }

    /// Whether all four steering controls are numerically zero (`±0.0`), enabling
    /// the bit-exact baseline single-loop motion fast-path.
    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.energy_gradient == 0.0
            && self.chemical_gradient == 0.0
            && self.chiral_contour == 0.0
            && self.heading_propulsion == 0.0
    }

    /// The four decoded controls in canonical slot order
    /// `[energy_gradient, chemical_gradient, chiral_contour, heading_propulsion]`.
    #[must_use]
    pub const fn to_array(&self) -> [f64; SO2_MOTOR_OUTPUTS] {
        [
            self.energy_gradient,
            self.chemical_gradient,
            self.chiral_contour,
            self.heading_propulsion,
        ]
    }
}

impl From<MotorSteering> for MotorSteeringInputs {
    /// Convert decoded `so2-motor` steering to phase 6's plain-data settle port (`V2-05`).
    fn from(motor: MotorSteering) -> Self {
        Self {
            energy_gradient: motor.energy_gradient,
            chemical_gradient: motor.chemical_gradient,
            chiral_contour: motor.chiral_contour,
            heading_propulsion: motor.heading_propulsion,
        }
    }
}

/// Decoded `so2-budding` (`V2-06`) local-frame budding offset
/// `u_local = clamp_radius([tanh(o_rad), tanh(o_tan)], 1.0)` in the
/// `(∇̂q, ∇̂q^⊥)` basis, guaranteed to satisfy exact left-fold
/// `radial² + tangential² <= 1.0`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BuddingSteering {
    /// Radial component along `e_1` (`∇̂q` or fallback unit axis), in `[-1, 1]`.
    pub radial: f64,
    /// Tangential component along `e_2 = [-e_1[1], e_1[0]]`, in `[-1, 1]`.
    pub tangential: f64,
}

impl BuddingSteering {
    /// Neutral zero budding offset (places daughter inside `r_local` only).
    pub const ZERO: Self = Self {
        radial: 0.0,
        tangential: 0.0,
    };

    /// Decode the two linear `so2-budding` logits `[o_rad, o_tan]` via
    /// host-independent [`flife_core::numerics::tanh`], then radially clamp to
    /// the unit disk via [`shrink_to_left_fold_norm_limit`].
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] if either logit is nonfinite.
    pub fn decode(raw: [f64; SO2_BUDDING_OUTPUTS]) -> Result<Self, CoreError> {
        let radial = tanh(raw[0]).ok_or_else(|| CoreError::NumericalInvalid {
            quantity: "so2-budding output",
            detail: format!("budding output 0 is {}", raw[0]),
        })?;
        let tangential = tanh(raw[1]).ok_or_else(|| CoreError::NumericalInvalid {
            quantity: "so2-budding output",
            detail: format!("budding output 1 is {}", raw[1]),
        })?;
        let mut local = [radial, tangential];
        shrink_to_left_fold_norm_limit(&mut local, 1.0)?;
        Ok(Self {
            radial: local[0],
            tangential: local[1],
        })
    }

    /// Whether both local components are numerically zero (`±0.0`).
    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.radial == 0.0 && self.tangential == 0.0
    }

    /// The decoded local-frame offset `[radial, tangential]`.
    #[must_use]
    pub const fn to_array(&self) -> [f64; SO2_BUDDING_OUTPUTS] {
        [self.radial, self.tangential]
    }
}

/// One tick's decoded actions.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
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
    /// Decoded `so2-motor` (`V2-05`) steering controls, when active in `layout`.
    pub motor: Option<MotorSteering>,
    /// Decoded `so2-budding` (`V2-06`) local-frame budding offset, when active in `layout`.
    pub budding: Option<BuddingSteering>,
    /// Decoded `emt-gate` (`V2-08`) sigmoid `σ(o_adh) ∈ [0, 1]`, when active in `layout`.
    /// (`V2-08` multiplies by `2.0` to form the adhesion multiplier `2 σ(o_adh)`.)
    pub emt_gate: Option<f64>,
    /// Decoded `wake-offset` (`V2-04`) neural pump control `tanh(o_pump) ∈ [-1, 1]`,
    /// when `mode = "polarized-pump-v1"` and `pump_direction_mode = "neural-output-v1"`.
    pub wake_pump: Option<f64>,
}

/// Decode `raw`, the 36 baseline linear outputs of one controller evaluation,
/// for a tick of `step`.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for a nonfinite output, or a direction
/// whose norm overflows.
pub fn decode(
    raw: &[f64; ACTION_WIDTH],
    step: TimeStep,
    decoding: &ActionDecoding,
) -> Result<DecodedActions, CoreError> {
    decode_slice(raw, step, decoding)
}

/// Decode the baseline `0..36` actions from a variable-width slice `raw`
/// (`raw.len() >= ACTION_WIDTH`) while checking finiteness across every entry
/// in `raw`. Optional v2 fields are `None`.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] if `raw.len() < ACTION_WIDTH`;
/// [`CoreError::NumericalInvalid`] for any nonfinite output, or an emission
/// direction whose norm overflows.
pub fn decode_slice(
    raw: &[f64],
    step: TimeStep,
    decoding: &ActionDecoding,
) -> Result<DecodedActions, CoreError> {
    decode_with_layout(raw, step, decoding, &ActionLayout::v1())
}

/// Decode the baseline `0..36` actions plus any active `field-ecology-v2`
/// action slots declared by `layout` from `raw`
/// (`raw.len() >= layout.action_width()`), checking finiteness across every
/// entry in `raw`.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] if `raw.len() < layout.action_width()`;
/// [`CoreError::NumericalInvalid`] for any nonfinite output, or an emission
/// direction whose norm overflows.
pub fn decode_with_layout(
    raw: &[f64],
    step: TimeStep,
    decoding: &ActionDecoding,
    layout: &ActionLayout,
) -> Result<DecodedActions, CoreError> {
    let required = layout.action_width();
    if raw.len() < required {
        return Err(CoreError::LayoutMismatch {
            buffer: "controller outputs",
            detail: format!("expected at least {required} outputs, got {}", raw.len()),
        });
    }
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

    let motor = match layout.motor_slots() {
        Some(slots) => {
            let start = slots.start;
            Some(MotorSteering::decode([
                output(start),
                output(start + 1),
                output(start + 2),
                output(start + 3),
            ])?)
        }
        None => None,
    };
    let budding = match layout.budding_slots() {
        Some(slots) => {
            let start = slots.start;
            Some(BuddingSteering::decode([output(start), output(start + 1)])?)
        }
        None => None,
    };
    let emt_gate = match layout.emt_slot() {
        Some(slot) => Some(squash(slot)?),
        None => None,
    };
    let wake_pump = match layout.pump_slot() {
        Some(slot) => Some(
            tanh(output(slot)).ok_or_else(|| CoreError::NumericalInvalid {
                quantity: "wake-offset pump output",
                detail: format!("tanh(output {slot}) is undefined"),
            })?,
        ),
        None => None,
    };

    Ok(DecodedActions {
        attempt_rate,
        attempt_probability,
        child_fraction,
        emission_gate,
        emission,
        motor,
        budding,
        emt_gate,
        wake_pump,
    })
}
