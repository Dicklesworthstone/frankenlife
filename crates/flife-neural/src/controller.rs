//! The reference controller evaluator for `independent-dense-mlp-v1` (plan
//! §6.2, §6.5, §10.1, §10.3, Appendix C.11, C.16).
//!
//! This is the scalar known-answer oracle for neural outputs. It is written
//! for clarity and a declared order of operations, not for speed;
//! accelerated routes (FL-025, FL-030) are compared against it.
//!
//! Declared arithmetic, the reference F32 profile (`accumulation_dtype =
//! "f32"`):
//! - Each of the 104 F64 sensor inputs is rounded once to F32 (nearest,
//!   ties to even).
//! - Output neuron `o` of a layer with fan-in `I` accumulates
//!   `z_o = ((b_o + w_o,0 x_0) + w_o,1 x_1) + … + w_o,I−1 x_I−1` in F32: the
//!   bias first, then the inputs in ascending index, with every product
//!   and every sum rounded to F32. There is no fused multiply-add.
//! - The hidden activation `tanh` is `flife_core::numerics::tanh` of the F32
//!   pre-activation, evaluated in F64 and rounded once to F32. It is
//!   computed as `flife_core::numerics::tanh_f32`, which gives those bits
//!   for every F32 argument.
//! - The output activation is `linear`: the final affine layer's `z` is the
//!   raw output. Sigmoid and every other decoding happen once, in
//!   [`crate::actions::decode`].
//!
//! Parameter layout: layers in evaluation order, and within each layer
//! `weight[out][in]` row-major followed by `bias[out]`. That is the order of
//! torch's `parameters_to_vector`. A neuron's identity is its layer and
//! index, never its packed offset (A.3 item 5); [`LayerShape`] maps one to
//! the other.
//!
//! Every organism's controller is a view over that organism's own
//! parameters. Organisms of one topology share only the shape (§6.5,
//! INV-13).

use std::ops::{Deref, DerefMut};

use flife_core::memory::capacity_bytes;
use flife_core::model::{MAXIMUM_HIDDEN_LAYERS, dense_parameter_count};
use flife_core::numerics::{
    normalize_turns, shrink_to_left_fold_norm_limit, sigmoid, sin_cos_turns,
    symplectic_rotate_channels, tanh_f32,
};
use flife_core::{CoreError, ValidatedConfig};

use crate::actions::{ACTION_WIDTH, ActionLayout, MAX_ACTION_WIDTH, MAX_CONTROLLER_OUTPUT_WIDTH};
use crate::sensors::{MAX_SENSOR_WIDTH, SENSOR_WIDTH, SensorLayout};
use crate::weights::{Weight, WeightDtype, Weights};

/// The baseline stateless dense MLP controller family.
pub const INDEPENDENT_DENSE_MLP_V1: &str = "independent-dense-mlp-v1";

/// The gated recurrent MLP controller family (plan §17.5, §25.2; FL-054):
/// appends per-organism internal state `h ∈ [-1, 1]^S` to the 104 sensor
/// inputs and extends the linear output layer by `2S` pre-activations
/// `(candidate, gate)` to compute the bounded gated transition
/// `h'_k = (1 − g_k) h_k + g_k tanh(c_k)` where `g_k = σ(z_g,k)`.
pub const INDEPENDENT_RECURRENT_MLP_V1: &str = "independent-recurrent-mlp-v1";

/// Maximum recurrent internal state width `S` (FL-054, plan §25.2).
pub const MAXIMUM_RECURRENT_STATE_WIDTH: usize =
    flife_core::model::MAXIMUM_RECURRENT_STATE_WIDTH as usize;

/// The hidden activation this module implements.
pub const TANH: &str = "tanh";

/// The output activation this module implements.
pub const LINEAR: &str = "linear";

/// The weight dtype and accumulation dtype this module implements.
pub const F32: &str = "f32";

/// One affine layer's shape and where its parameters sit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerShape {
    /// Fan-in.
    pub inputs: usize,
    /// Fan-out.
    pub outputs: usize,
    /// Offset of `weight[0][0]`.
    pub weights: usize,
    /// Offset of `bias[0]`.
    pub biases: usize,
}

impl LayerShape {
    /// The offset of `weight[output][input]`.
    #[must_use]
    pub const fn weight(&self, output: usize, input: usize) -> Option<usize> {
        if output < self.outputs && input < self.inputs {
            match output.checked_mul(self.inputs) {
                Some(row) => match row.checked_add(input) {
                    Some(offset) => self.weights.checked_add(offset),
                    None => None,
                },
                None => None,
            }
        } else {
            None
        }
    }

    /// The offset of `bias[output]`.
    #[must_use]
    pub const fn bias(&self, output: usize) -> Option<usize> {
        if output < self.outputs {
            self.biases.checked_add(output)
        } else {
            None
        }
    }

    /// One past this layer's last parameter.
    #[must_use]
    pub const fn end(&self) -> usize {
        self.biases.saturating_add(self.outputs)
    }
}

/// Variable-width raw linear outputs of one controller evaluation, stored
/// inline up to [`MAX_CONTROLLER_OUTPUT_WIDTH`] (`80`) elements.
///
/// The first `action_width` entries are the action logits stored in
/// `HeldOutput.outputs` (up to [`MAX_ACTION_WIDTH`] = `48`); for recurrent
/// topologies with state width `S`, the trailing `2 * S` entries hold
/// `[cand[0..S], gate[0..S]]` consumed by [`update_recurrent_state`].
#[derive(Debug, Clone, Copy)]
pub struct RawOutputs {
    values: [f32; MAX_CONTROLLER_OUTPUT_WIDTH],
    len: u8,
}

impl PartialEq for RawOutputs {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl Deref for RawOutputs {
    type Target = [f32];

    #[inline]
    fn deref(&self) -> &[f32] {
        self.as_slice()
    }
}

impl DerefMut for RawOutputs {
    #[inline]
    fn deref_mut(&mut self) -> &mut [f32] {
        self.as_mut_slice()
    }
}

impl RawOutputs {
    /// Zeroed raw outputs of length `len` (`1 <= len <= MAX_CONTROLLER_OUTPUT_WIDTH`).
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if `len` is zero or exceeds
    /// [`MAX_CONTROLLER_OUTPUT_WIDTH`].
    pub fn zeroed(len: usize) -> Result<Self, CoreError> {
        if len == 0 || len > MAX_CONTROLLER_OUTPUT_WIDTH {
            return Err(CoreError::LayoutMismatch {
                buffer: "controller outputs",
                detail: format!(
                    "raw output length {len} must lie in 1..={MAX_CONTROLLER_OUTPUT_WIDTH}"
                ),
            });
        }
        let len_u8 = u8::try_from(len).map_err(|_| CoreError::Overflow {
            quantity: "controller output width",
        })?;
        Ok(Self {
            values: [0.0; MAX_CONTROLLER_OUTPUT_WIDTH],
            len: len_u8,
        })
    }

    /// Copy `slice` into a [`RawOutputs`] value (`1 <= slice.len() <= 80`).
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if `slice` is empty or exceeds
    /// [`MAX_CONTROLLER_OUTPUT_WIDTH`].
    pub fn from_slice(slice: &[f32]) -> Result<Self, CoreError> {
        let mut out = Self::zeroed(slice.len())?;
        out.as_mut_slice().copy_from_slice(slice);
        Ok(out)
    }

    /// Wrap a baseline `field-ecology-v1` 36-element output array.
    #[must_use]
    pub fn from_v1(values: [f32; ACTION_WIDTH]) -> Self {
        let mut storage = [0.0; MAX_CONTROLLER_OUTPUT_WIDTH];
        storage[..ACTION_WIDTH].copy_from_slice(&values);
        Self {
            values: storage,
            len: ACTION_WIDTH as u8,
        }
    }

    /// Active output slice (`len <= MAX_CONTROLLER_OUTPUT_WIDTH`).
    #[must_use]
    #[inline]
    pub fn as_slice(&self) -> &[f32] {
        &self.values[..usize::from(self.len)]
    }

    /// Mutable active output slice.
    #[must_use]
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [f32] {
        &mut self.values[..usize::from(self.len)]
    }

    /// Number of active output elements.
    #[must_use]
    #[inline]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// Whether there are zero outputs (never true for a constructed value).
    #[must_use]
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Copy the active outputs into a `[f32; ACTION_WIDTH]` when `len == 36`.
    #[must_use]
    pub fn as_v1_array(&self) -> Option<[f32; ACTION_WIDTH]> {
        if self.len() != ACTION_WIDTH {
            return None;
        }
        let mut out = [0.0; ACTION_WIDTH];
        out.copy_from_slice(self.as_slice());
        Some(out)
    }

    /// Return a new [`RawOutputs`] holding the first `len` elements.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if `len == 0` or `len > self.len()`.
    pub fn prefix(&self, len: usize) -> Result<Self, CoreError> {
        let slice = self
            .as_slice()
            .get(..len)
            .ok_or_else(|| CoreError::LayoutMismatch {
                buffer: "controller outputs",
                detail: format!("requested prefix {len} of {} outputs", self.len()),
            })?;
        Self::from_slice(slice)
    }

    /// The action prefix `0..action_width`, if in bounds.
    #[must_use]
    pub fn action_slice(&self, action_width: usize) -> Option<&[f32]> {
        self.as_slice().get(..action_width)
    }

    /// The trailing recurrent raw logits `action_width..len`, if in bounds.
    #[must_use]
    pub fn recurrent_tail(&self, action_width: usize) -> Option<&[f32]> {
        self.as_slice().get(action_width..)
    }
}

/// Apply the canonical bounded gated recurrent state update (FL-054, V2-01,
/// V2-12) shared across Reference CPU, FrankenTorch CPU, and Metal backends so
/// all routes stay 100% bit-identical with zero `d_sigmoid` shader duplication.
///
/// For each state unit `k ∈ 0..S`:
/// - `cand_k = tanh_f32(raw_tail[k])`
/// - `gate_k = sigmoid(f64::from(raw_tail[S + k]) - gate_bias) as f32`
///   (with `gate_bias == 0.0` passing `f64::from(raw_tail[S + k])` unmodified
///   so `v1` `FL-054` remains bit-exact, and positive `b_epi = 2.5` closing
///   the gate to `σ(-2.5) ≈ 0.076` at `z_g = 0`)
/// - `next_state[k] = ((1.0_f32 - gate_k) * state[k] + gate_k * cand_k).clamp(-1.0, 1.0)`
///
/// # Errors
/// [`CoreError::LayoutMismatch`] if `state.len() != next_state.len()`,
/// `raw_tail.len() != 2 * state.len()`, or `state.len() > MAXIMUM_RECURRENT_STATE_WIDTH`;
/// [`CoreError::NumericalInvalid`] if `gate_bias` is nonfinite, any `state[k]`
/// lies outside `[-1, 1]`, or any candidate/gate pre-activation or updated state
/// is nonfinite.
pub fn update_recurrent_state(
    state: &[f32],
    raw_tail: &[f32],
    gate_bias: f64,
    next_state: &mut [f32],
) -> Result<(), CoreError> {
    if !gate_bias.is_finite() {
        return Err(CoreError::NumericalInvalid {
            quantity: "recurrent gate bias",
            detail: format!("gate bias {gate_bias} is not finite"),
        });
    }
    let width = state.len();
    if width > MAXIMUM_RECURRENT_STATE_WIDTH
        || next_state.len() != width
        || raw_tail.len() != width.saturating_mul(2)
    {
        return Err(CoreError::LayoutMismatch {
            buffer: "controller state",
            detail: format!(
                "expected state={width} (<= {MAXIMUM_RECURRENT_STATE_WIDTH}), \
                 next_state={width}, raw_tail={}, got next_state={} raw_tail={}",
                width.saturating_mul(2),
                next_state.len(),
                raw_tail.len(),
            ),
        });
    }
    for (index, &h) in state.iter().enumerate() {
        if !h.is_finite() || !(-1.0..=1.0).contains(&h) {
            return Err(CoreError::NumericalInvalid {
                quantity: "recurrent controller state",
                detail: format!("state[{index}] is {h}, outside [-1, 1]"),
            });
        }
    }
    let (raw_candidates, raw_gates) = raw_tail.split_at(width);
    let mut computed = [0.0_f32; MAXIMUM_RECURRENT_STATE_WIDTH];
    for k in 0..width {
        let z_c = raw_candidates[k];
        if !z_c.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "recurrent candidate pre-activation",
                detail: format!("candidate {k} pre-activation {z_c} overflowed"),
            });
        }
        let cand = tanh_f32(z_c).ok_or_else(|| CoreError::NumericalInvalid {
            quantity: "recurrent candidate pre-activation",
            detail: format!("candidate {k} pre-activation {z_c} overflowed"),
        })?;
        let z_g = raw_gates[k];
        if !z_g.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "recurrent gate pre-activation",
                detail: format!("gate {k} pre-activation {z_g} overflowed"),
            });
        }
        let gate_arg = if gate_bias == 0.0 {
            f64::from(z_g)
        } else {
            f64::from(z_g) - gate_bias
        };
        let gate = sigmoid(gate_arg).ok_or_else(|| CoreError::NumericalInvalid {
            quantity: "recurrent gate pre-activation",
            detail: format!("gate {k} pre-activation {z_g} overflowed"),
        })? as f32;
        if !gate.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "recurrent update gate",
                detail: format!("gate {k} is {gate}"),
            });
        }
        let updated = ((1.0_f32 - gate) * state[k] + gate * cand).clamp(-1.0_f32, 1.0_f32);
        if !updated.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "recurrent controller state",
                detail: format!("next_state[{k}] is {updated}"),
            });
        }
        computed[k] = updated;
    }
    next_state.copy_from_slice(&computed[..width]);
    Ok(())
}

/// Rotate a 32-channel chemical affinity vector `affinity` in the `(a, J a)`
/// symplectic plane by angle `(π / 2) · h0` for bounded epigenetic state
/// `h0 ∈ [-1.0, 1.0]` (`V2-12` clonal metabolic caste differentiation):
///
/// `a_eff(h0) = cos((π / 2) h0) · a + sin((π / 2) h0) · J a`
///
/// Wraps `0.25 * f64::from(h0)` through [`flife_core::numerics::normalize_turns`]
/// before [`flife_core::numerics::sin_cos_turns`] so negative `h0 ∈ [-1, 0)`
/// maps cleanly into `[0.75, 1.0)` without triggering `sin_cos_turns`'s
/// negative-turn refusal. Applies [`flife_core::numerics::symplectic_rotate_channels`]
/// over `0..active_channels` (`32` single-carrier or `24` Carrier-0 under
/// `V2-15`; trailing unpaired/Carrier-1 channels are preserved untouched from
/// `affinity`) and enforces exact left-fold `‖a_eff‖₂² ≤ norm_limit²` via
/// [`flife_core::numerics::shrink_to_left_fold_norm_limit`].
///
/// # Errors
/// [`CoreError::NumericalInvalid`] if `h0` is nonfinite or outside `[-1, 1]`,
/// `active_channels > 32`, `norm_limit` is nonfinite/nonpositive, any component
/// of `affinity` is nonfinite, or norm-limit enforcement fails.
pub fn rotate_epigenetic_affinity(
    affinity: &[f64; 32],
    h0: f32,
    active_channels: usize,
    norm_limit: f64,
) -> Result<[f64; 32], CoreError> {
    if !h0.is_finite() || !(-1.0_f32..=1.0_f32).contains(&h0) {
        return Err(CoreError::NumericalInvalid {
            quantity: "epigenetic state h0",
            detail: format!("h0 = {h0} is outside [-1, 1]"),
        });
    }
    let phase =
        normalize_turns(0.25 * f64::from(h0)).ok_or_else(|| CoreError::NumericalInvalid {
            quantity: "epigenetic rotation phase",
            detail: format!("could not normalize turn angle for h0 = {h0}"),
        })?;
    let (sin_a, cos_a) = sin_cos_turns(phase).ok_or_else(|| CoreError::NumericalInvalid {
        quantity: "epigenetic rotation phase",
        detail: format!("normalized phase {phase} left [0, 1)"),
    })?;
    let ja = symplectic_rotate_channels(affinity, active_channels, norm_limit)?;
    let rotated_end = (active_channels / 2) * 2;
    let mut out = *affinity;
    if sin_a == 0.0 {
        for (dst, &src) in out[..rotated_end].iter_mut().zip(&affinity[..rotated_end]) {
            *dst = cos_a * src;
        }
    } else if cos_a == 0.0 {
        for (dst, &j) in out[..rotated_end].iter_mut().zip(&ja[..rotated_end]) {
            *dst = sin_a * j;
        }
    } else {
        for ((dst, &src), &j) in out[..rotated_end]
            .iter_mut()
            .zip(&affinity[..rotated_end])
            .zip(&ja[..rotated_end])
        {
            *dst = cos_a * src + sin_a * j;
        }
    }
    shrink_to_left_fold_norm_limit(&mut out, norm_limit)?;
    Ok(out)
}

/// A dense topology `input_width → hidden… → output_width` with one or two
/// hidden layers (`input_width <= MAX_SENSOR_WIDTH`,
/// `output_width <= MAX_CONTROLLER_OUTPUT_WIDTH`).
///
/// For stateless `field-ecology-v1`, `input_width = 104`, `action_width = 36`,
/// `output_width = 36`, `recurrent_state_width = 0`. For gated recurrent
/// topologies, `input_width = sensor_width + S` and
/// `output_width = action_width + 2 * S` with `1 <= S <= 16`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DenseTopology {
    input_width: usize,
    action_width: usize,
    output_width: usize,
    hidden_widths: Vec<u32>,
    layers: Vec<LayerShape>,
    parameters: usize,
    widest_hidden: usize,
    recurrent_state_width: usize,
}

impl DenseTopology {
    /// The stateless baseline topology `104 → hidden… → 36` with these hidden
    /// widths, in evaluation order.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for no hidden layer, more than two,
    /// or a zero width; [`CoreError::Overflow`] for a parameter count past
    /// `usize`.
    pub fn new(hidden_widths: &[u32]) -> Result<Self, CoreError> {
        Self::build_io(SENSOR_WIDTH, hidden_widths, ACTION_WIDTH, 0)
    }

    /// A gated recurrent baseline topology `(104 + S) → hidden… → (36 + 2S)`
    /// for `independent-recurrent-mlp-v1` (FL-054, plan §25.2).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for `state_width` outside
    /// `1..=MAXIMUM_RECURRENT_STATE_WIDTH`, or as [`DenseTopology::new`].
    pub fn new_recurrent(hidden_widths: &[u32], state_width: u32) -> Result<Self, CoreError> {
        let width = usize::try_from(state_width).map_err(|_| CoreError::Overflow {
            quantity: "controller.recurrent_state_width",
        })?;
        if width == 0 || width > MAXIMUM_RECURRENT_STATE_WIDTH {
            return Err(CoreError::NumericalInvalid {
                quantity: "controller.recurrent_state_width",
                detail: format!(
                    "recurrent state width must lie in 1..={MAXIMUM_RECURRENT_STATE_WIDTH}, got \
                     {state_width}"
                ),
            });
        }
        Self::build_io(SENSOR_WIDTH + width, hidden_widths, ACTION_WIDTH, width)
    }

    /// A variable-width stateless dense topology `input_width → hidden… → output_width`
    /// (`1 <= input_width <= MAX_SENSOR_WIDTH`,
    /// `1 <= output_width <= MAX_CONTROLLER_OUTPUT_WIDTH`).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for widths outside the admitted bounds
    /// or invalid hidden layers; [`CoreError::Overflow`] for parameter count
    /// overflow.
    pub fn with_io(
        input_width: u32,
        hidden_widths: &[u32],
        output_width: u32,
    ) -> Result<Self, CoreError> {
        let inputs = usize::try_from(input_width).map_err(|_| CoreError::Overflow {
            quantity: "controller.input_width",
        })?;
        let outputs = usize::try_from(output_width).map_err(|_| CoreError::Overflow {
            quantity: "controller.output_width",
        })?;
        Self::build_io(inputs, hidden_widths, outputs, 0)
    }

    /// A variable-width gated recurrent dense topology
    /// `(sensor_width + S) → hidden… → (action_width + 2S)`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for `state_width` outside `1..=16`,
    /// `action_width > MAX_ACTION_WIDTH`, or total widths past
    /// `(MAX_SENSOR_WIDTH, MAX_CONTROLLER_OUTPUT_WIDTH)`.
    pub fn with_io_recurrent(
        sensor_width: u32,
        hidden_widths: &[u32],
        action_width: u32,
        state_width: u32,
    ) -> Result<Self, CoreError> {
        let sensors = usize::try_from(sensor_width).map_err(|_| CoreError::Overflow {
            quantity: "controller.input_width",
        })?;
        let actions = usize::try_from(action_width).map_err(|_| CoreError::Overflow {
            quantity: "controller.output_width",
        })?;
        let state = usize::try_from(state_width).map_err(|_| CoreError::Overflow {
            quantity: "controller.recurrent_state_width",
        })?;
        if state == 0 || state > MAXIMUM_RECURRENT_STATE_WIDTH {
            return Err(CoreError::NumericalInvalid {
                quantity: "controller.recurrent_state_width",
                detail: format!(
                    "recurrent state width must lie in 1..={MAXIMUM_RECURRENT_STATE_WIDTH}, got \
                     {state_width}"
                ),
            });
        }
        if actions > MAX_ACTION_WIDTH {
            return Err(CoreError::NumericalInvalid {
                quantity: "controller.output_width",
                detail: format!("action width {actions} exceeds maximum {MAX_ACTION_WIDTH}"),
            });
        }
        let input_width = sensors.checked_add(state).ok_or(CoreError::Overflow {
            quantity: "controller.input_width",
        })?;
        Self::build_io(input_width, hidden_widths, actions, state)
    }

    /// Build a topology directly from canonical [`SensorLayout`] and
    /// [`ActionLayout`].
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if the layouts disagree on
    /// `recurrent_state_width`, or as [`DenseTopology::with_io`].
    pub fn from_layouts(
        sensors: &SensorLayout,
        hidden_widths: &[u32],
        actions: &ActionLayout,
    ) -> Result<Self, CoreError> {
        if sensors.recurrent_state_width() != actions.recurrent_state_width() {
            return Err(CoreError::LayoutMismatch {
                buffer: "controller layout",
                detail: format!(
                    "sensor layout recurrent width {} differs from action layout {}",
                    sensors.recurrent_state_width(),
                    actions.recurrent_state_width()
                ),
            });
        }
        Self::build_io(
            sensors.input_width(),
            hidden_widths,
            actions.action_width(),
            sensors.recurrent_state_width(),
        )
    }

    fn build_io(
        input_width: usize,
        hidden_widths: &[u32],
        action_width: usize,
        recurrent_state_width: usize,
    ) -> Result<Self, CoreError> {
        if hidden_widths.is_empty() || hidden_widths.len() > MAXIMUM_HIDDEN_LAYERS {
            return Err(CoreError::NumericalInvalid {
                quantity: "controller.hidden_widths",
                detail: format!(
                    "the dense catalog admits 1 to {MAXIMUM_HIDDEN_LAYERS} hidden layers, got {}",
                    hidden_widths.len()
                ),
            });
        }
        if recurrent_state_width > MAXIMUM_RECURRENT_STATE_WIDTH {
            return Err(CoreError::NumericalInvalid {
                quantity: "controller.recurrent_state_width",
                detail: format!(
                    "recurrent state width {recurrent_state_width} exceeds maximum \
                     {MAXIMUM_RECURRENT_STATE_WIDTH}"
                ),
            });
        }
        if input_width == 0
            || input_width <= recurrent_state_width
            || input_width > MAX_SENSOR_WIDTH
        {
            return Err(CoreError::NumericalInvalid {
                quantity: "controller.input_width",
                detail: format!(
                    "input width {input_width} (recurrent {recurrent_state_width}) outside \
                     1..={MAX_SENSOR_WIDTH}"
                ),
            });
        }
        if action_width == 0
            || (recurrent_state_width > 0 && action_width > MAX_ACTION_WIDTH)
            || action_width > MAX_CONTROLLER_OUTPUT_WIDTH
        {
            return Err(CoreError::NumericalInvalid {
                quantity: "controller.output_width",
                detail: format!(
                    "action width {action_width} outside 1..={MAX_CONTROLLER_OUTPUT_WIDTH}"
                ),
            });
        }
        let overflow = CoreError::Overflow {
            quantity: "controller parameter count",
        };
        let output_width = action_width
            .checked_add(
                recurrent_state_width
                    .checked_mul(2)
                    .ok_or_else(|| overflow.clone())?,
            )
            .ok_or_else(|| overflow.clone())?;
        if output_width > MAX_CONTROLLER_OUTPUT_WIDTH {
            return Err(CoreError::NumericalInvalid {
                quantity: "controller.output_width",
                detail: format!(
                    "raw output width {output_width} exceeds maximum \
                     {MAX_CONTROLLER_OUTPUT_WIDTH}"
                ),
            });
        }
        let total = dense_parameter_count(
            u32::try_from(input_width).map_err(|_| overflow.clone())?,
            hidden_widths,
            u32::try_from(output_width).map_err(|_| overflow.clone())?,
        )?;
        let parameters = usize::try_from(total).map_err(|_| overflow.clone())?;

        // Every offset below is at most `parameters`, which fits.
        let mut layers = Vec::with_capacity(hidden_widths.len() + 1);
        let mut offset = 0;
        let mut fan_in = input_width;
        let mut widest_hidden = 0;
        for (index, width) in hidden_widths
            .iter()
            .map(|&width| usize::try_from(width).map_err(|_| overflow.clone()))
            .chain([Ok(output_width)])
            .enumerate()
        {
            let width = width?;
            let biases = offset + fan_in * width;
            layers.push(LayerShape {
                inputs: fan_in,
                outputs: width,
                weights: offset,
                biases,
            });
            if index < hidden_widths.len() {
                widest_hidden = widest_hidden.max(width);
            }
            offset = biases + width;
            fan_in = width;
        }
        if offset != parameters {
            return Err(CoreError::LayoutMismatch {
                buffer: "controller parameters",
                detail: format!("layers end at {offset}, the count is {parameters}"),
            });
        }
        Ok(Self {
            input_width,
            action_width,
            output_width,
            hidden_widths: hidden_widths.to_vec(),
            layers,
            parameters,
            widest_hidden,
            recurrent_state_width,
        })
    }

    /// The default topology of a validated configuration. The topology is
    /// the same for either weight dtype ([`WeightDtype`]).
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for another controller family,
    /// activation, weight dtype or accumulation dtype; otherwise as
    /// [`DenseTopology::from_layouts`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let controller = &config.config().controller;
        WeightDtype::named(&controller.canonical_weight_dtype)?;
        for (role, name, implemented) in [
            (
                "controller.hidden_activation",
                &controller.hidden_activation,
                TANH,
            ),
            (
                "controller.output_activation",
                &controller.output_activation,
                LINEAR,
            ),
            (
                "controller.accumulation_dtype",
                &controller.accumulation_dtype,
                F32,
            ),
        ] {
            if name != implemented {
                return Err(CoreError::UnknownAlgorithm {
                    role,
                    name: name.clone(),
                });
            }
        }
        let sensors = SensorLayout::from_config(config)?;
        let actions = ActionLayout::from_config(config)?;
        Self::from_layouts(&sensors, &controller.hidden_widths, &actions)
    }

    /// Hidden widths in evaluation order.
    #[must_use]
    pub fn hidden_widths(&self) -> &[u32] {
        &self.hidden_widths
    }

    /// Every affine layer in evaluation order; the last is the output layer.
    #[must_use]
    pub fn layers(&self) -> &[LayerShape] {
        &self.layers
    }

    /// `P`: weights plus biases.
    #[must_use]
    pub const fn parameter_count(&self) -> usize {
        self.parameters
    }

    /// Recurrent internal state width `S` (`0` for stateless topologies).
    #[must_use]
    pub const fn recurrent_state_width(&self) -> usize {
        self.recurrent_state_width
    }

    /// Whether this topology carries gated recurrent internal state.
    #[must_use]
    pub const fn is_recurrent(&self) -> bool {
        self.recurrent_state_width > 0
    }

    /// Non-recurrent sensor input width (`input_width - S`).
    #[must_use]
    pub const fn sensor_width(&self) -> usize {
        self.input_width - self.recurrent_state_width
    }

    /// Effective network input width (`sensor_width + S`, `<= 128`).
    #[must_use]
    pub const fn input_width(&self) -> usize {
        self.input_width
    }

    /// Action output prefix width stored in `HeldOutput.outputs` (`<= 48` when
    /// recurrent).
    #[must_use]
    pub const fn action_width(&self) -> usize {
        self.action_width
    }

    /// Effective network linear output width (`action_width + 2S`, `<= 80`).
    #[must_use]
    pub const fn output_width(&self) -> usize {
        self.output_width
    }

    /// Alias of [`DenseTopology::output_width`]: total raw linear outputs
    /// produced by the final layer (`action_width + 2S`, `<= 80`).
    #[must_use]
    pub const fn raw_output_width(&self) -> usize {
        self.output_width
    }
}

/// Reusable activation storage for evaluations. One scratch serves any
/// topology; it grows to the widest hidden layer it has seen. Nothing in
/// it survives from one evaluation into the next.
#[derive(Debug, Clone, Default)]
pub struct ControllerScratch {
    first: Vec<f32>,
    second: Vec<f32>,
}

impl ControllerScratch {
    /// Scratch sized for `topology`.
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when the host refuses the storage.
    pub fn new(topology: &DenseTopology) -> Result<Self, CoreError> {
        let mut scratch = Self::default();
        scratch.reserve(topology.widest_hidden)?;
        Ok(scratch)
    }

    /// Bytes its two activation buffers hold (FL-043).
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        capacity_bytes(&self.first).saturating_add(capacity_bytes(&self.second))
    }

    fn reserve(&mut self, width: usize) -> Result<(), CoreError> {
        for buffer in [&mut self.first, &mut self.second] {
            buffer.clear();
            buffer
                .try_reserve_exact(width)
                .map_err(|_| CoreError::AllocationFailed {
                    buffer: "controller activations",
                    bytes: u64::try_from(width.saturating_mul(size_of::<f32>()))
                        .unwrap_or(u64::MAX),
                })?;
        }
        Ok(())
    }
}

/// One organism's controller: its own parameters, read through a topology,
/// in either weight dtype ([`Weights`]).
#[derive(Debug, Clone, Copy)]
pub struct ReferenceController<'a> {
    topology: &'a DenseTopology,
    parameters: Weights<'a>,
}

impl<'a> ReferenceController<'a> {
    /// A controller over `parameters`, laid out as `topology` declares, each
    /// finite and within `±maximum_absolute_parameter`. Within those bounds
    /// every admitted affine calculation stays finite for ABI inputs (§6.2).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonpositive or nonfinite bound,
    /// or a parameter outside it; [`CoreError::LayoutMismatch`] for a
    /// parameter count the topology does not have.
    pub fn new(
        topology: &'a DenseTopology,
        parameters: impl Into<Weights<'a>>,
        maximum_absolute_parameter: f64,
    ) -> Result<Self, CoreError> {
        let parameters = parameters.into();
        if !maximum_absolute_parameter.is_finite() || maximum_absolute_parameter <= 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "controller.maximum_absolute_parameter",
                detail: format!("{maximum_absolute_parameter} is not a positive bound"),
            });
        }
        if parameters.len() != topology.parameters {
            return Err(CoreError::LayoutMismatch {
                buffer: "controller parameters",
                detail: format!(
                    "{} values for a topology of {} parameters",
                    parameters.len(),
                    topology.parameters
                ),
            });
        }
        if let Some((index, value)) = parameters.iter().enumerate().find(|&(_, value)| {
            !value.is_finite() || f64::from(value.abs()) > maximum_absolute_parameter
        }) {
            return Err(CoreError::NumericalInvalid {
                quantity: "controller parameter",
                detail: format!(
                    "parameter {index} is {value}, outside ±{maximum_absolute_parameter}"
                ),
            });
        }
        Ok(Self {
            topology,
            parameters,
        })
    }

    /// A controller over `parameters` that [`ReferenceController::new`]
    /// already admitted, under a bound no larger than the one the caller
    /// would check, and that have not changed since: a sealed genome's
    /// immutable parameters. Only the layout is checked again, so evaluating a
    /// sealed genome does not rescan every parameter each tick.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for a parameter count the topology does
    /// not have.
    pub fn sealed(
        topology: &'a DenseTopology,
        parameters: impl Into<Weights<'a>>,
    ) -> Result<Self, CoreError> {
        let parameters = parameters.into();
        if parameters.len() != topology.parameters {
            return Err(CoreError::LayoutMismatch {
                buffer: "controller parameters",
                detail: format!(
                    "{} values for a topology of {} parameters",
                    parameters.len(),
                    topology.parameters
                ),
            });
        }
        Ok(Self {
            topology,
            parameters,
        })
    }

    /// The topology this controller is read through.
    #[must_use]
    pub const fn topology(&self) -> &'a DenseTopology {
        self.topology
    }

    /// The parameters, in the topology's layout and their canonical dtype.
    #[must_use]
    pub const fn parameters(&self) -> Weights<'a> {
        self.parameters
    }

    /// Evaluate any topology (`input_width <= MAX_SENSOR_WIDTH`,
    /// `raw_output_width <= MAX_CONTROLLER_OUTPUT_WIDTH`) on a slice of
    /// `input_width` F64 inputs, returning all `raw_output_width` linear raw
    /// outputs.
    ///
    /// For recurrent topologies, `inputs` holds `[sensors..., state...]` and
    /// the returned [`RawOutputs`] holds `[actions..., cand..., gate...]`.
    /// Passing the trailing `2 * S` slice to [`update_recurrent_state`] yields
    /// the exact same next state as [`ReferenceController::evaluate_recurrent`].
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if `inputs.len() != topology.input_width()`;
    /// [`CoreError::NumericalInvalid`] for an input that is not finite in F32,
    /// or a pre-activation or output that overflows;
    /// [`CoreError::AllocationFailed`] if `scratch` must grow and cannot.
    pub fn evaluate_raw(
        &self,
        inputs: &[f64],
        scratch: &mut ControllerScratch,
    ) -> Result<RawOutputs, CoreError> {
        let expected = self.topology.input_width();
        if inputs.len() != expected {
            return Err(CoreError::LayoutMismatch {
                buffer: "controller inputs",
                detail: format!("expected {expected} inputs, got {}", inputs.len()),
            });
        }
        let mut rounded = [0.0_f32; MAX_SENSOR_WIDTH];
        for (index, (slot, &value)) in rounded[..expected].iter_mut().zip(inputs).enumerate() {
            // Rounding each input once to F32 is the declared arithmetic.
            let narrowed = value as f32;
            if !narrowed.is_finite() {
                return Err(CoreError::NumericalInvalid {
                    quantity: "controller input",
                    detail: format!("input {index} is {value}, not finite in F32"),
                });
            }
            *slot = narrowed;
        }
        match self.parameters {
            Weights::F32(parameters) => {
                self.evaluate_raw_f32(parameters, &rounded[..expected], scratch)
            }
            Weights::F16(parameters) => {
                self.evaluate_raw_f32(parameters, &rounded[..expected], scratch)
            }
        }
    }

    /// Evaluate a stateless baseline (`104 → … → 36`) controller on one
    /// observation's inputs, giving the 36 raw outputs of its linear final
    /// layer.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if called on a recurrent topology (use
    /// [`ReferenceController::evaluate_recurrent`]) or a non-`(104, 36)`
    /// topology (use [`ReferenceController::evaluate_raw`]);
    /// [`CoreError::NumericalInvalid`] for an input that is not finite in
    /// F32, or a pre-activation that overflows;
    /// [`CoreError::AllocationFailed`] if `scratch` must grow and cannot.
    pub fn evaluate(
        &self,
        inputs: &[f64; SENSOR_WIDTH],
        scratch: &mut ControllerScratch,
    ) -> Result<[f32; ACTION_WIDTH], CoreError> {
        if self.topology.is_recurrent() {
            return Err(CoreError::LayoutMismatch {
                buffer: "controller state",
                detail: format!(
                    "recurrent controller (state width {}) requires evaluate_recurrent",
                    self.topology.recurrent_state_width()
                ),
            });
        }
        if self.topology.input_width() != SENSOR_WIDTH
            || self.topology.output_width() != ACTION_WIDTH
        {
            return Err(CoreError::LayoutMismatch {
                buffer: "controller topology",
                detail: format!(
                    "expected ({SENSOR_WIDTH}, {ACTION_WIDTH}) topology, got ({}, {})",
                    self.topology.input_width(),
                    self.topology.output_width()
                ),
            });
        }
        let raw = self.evaluate_raw(inputs, scratch)?;
        raw.as_v1_array().ok_or_else(|| CoreError::LayoutMismatch {
            buffer: "controller outputs",
            detail: format!("expected {ACTION_WIDTH} outputs, got {}", raw.len()),
        })
    }

    /// Evaluate a variable-width gated recurrent controller on
    /// `(sensor_inputs, state)` with optional `gate_bias` (`0.0` for FL-054,
    /// `b_epi` for V2-12), returning the `action_width` linear action outputs
    /// and writing the updated bounded internal state `h' ∈ [-1, 1]^S` into
    /// `next_state` via [`update_recurrent_state`].
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for a stateless topology or mismatched
    /// input/state lengths; [`CoreError::NumericalInvalid`] for nonfinite
    /// inputs, bias, state outside `[-1, 1]`, or nonfinite activations/outputs;
    /// [`CoreError::AllocationFailed`] if `scratch` must grow and cannot.
    pub fn evaluate_recurrent_slice(
        &self,
        sensor_inputs: &[f64],
        state: &[f32],
        gate_bias: f64,
        scratch: &mut ControllerScratch,
        next_state: &mut [f32],
    ) -> Result<RawOutputs, CoreError> {
        let width = self.topology.recurrent_state_width();
        if width == 0 {
            return Err(CoreError::LayoutMismatch {
                buffer: "controller state",
                detail: "stateless controller does not accept recurrent state".to_owned(),
            });
        }
        let sensor_width = self.topology.sensor_width();
        if sensor_inputs.len() != sensor_width {
            return Err(CoreError::LayoutMismatch {
                buffer: "controller inputs",
                detail: format!(
                    "expected {sensor_width} sensor inputs, got {}",
                    sensor_inputs.len()
                ),
            });
        }
        if state.len() != width || next_state.len() != width {
            return Err(CoreError::LayoutMismatch {
                buffer: "controller state",
                detail: format!(
                    "expected recurrent state width {width}, got state={} next_state={}",
                    state.len(),
                    next_state.len()
                ),
            });
        }
        if !gate_bias.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "recurrent gate bias",
                detail: format!("gate bias {gate_bias} is not finite"),
            });
        }
        let mut rounded = [0.0_f32; MAX_SENSOR_WIDTH];
        for (index, (slot, &value)) in rounded[..sensor_width]
            .iter_mut()
            .zip(sensor_inputs)
            .enumerate()
        {
            let narrowed = value as f32;
            if !narrowed.is_finite() {
                return Err(CoreError::NumericalInvalid {
                    quantity: "controller input",
                    detail: format!("input {index} is {value}, not finite in F32"),
                });
            }
            *slot = narrowed;
        }
        for (index, (slot, &h)) in rounded[sensor_width..sensor_width + width]
            .iter_mut()
            .zip(state)
            .enumerate()
        {
            if !h.is_finite() || !(-1.0..=1.0).contains(&h) {
                return Err(CoreError::NumericalInvalid {
                    quantity: "recurrent controller state",
                    detail: format!("state[{index}] is {h}, outside [-1, 1]"),
                });
            }
            *slot = h;
        }
        let total_in = sensor_width + width;
        let raw = match self.parameters {
            Weights::F32(parameters) => {
                self.evaluate_raw_f32(parameters, &rounded[..total_in], scratch)?
            }
            Weights::F16(parameters) => {
                self.evaluate_raw_f32(parameters, &rounded[..total_in], scratch)?
            }
        };
        let action_width = self.topology.action_width();
        let tail = raw
            .recurrent_tail(action_width)
            .ok_or_else(|| CoreError::LayoutMismatch {
                buffer: "controller outputs",
                detail: format!(
                    "output width {} shorter than action width {action_width}",
                    raw.len()
                ),
            })?;
        update_recurrent_state(state, tail, gate_bias, next_state)?;
        raw.prefix(action_width)
    }

    /// Evaluate a gated recurrent (`independent-recurrent-mlp-v1`) baseline
    /// controller on `(inputs, state)`, returning the 36 linear action outputs
    /// and writing the updated bounded internal state `h' ∈ [-1, 1]^S` into
    /// `next_state` (FL-054, plan §25.2).
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for a stateless topology, non-`(104, 36)`
    /// base dimensions, or mismatched state slice lengths;
    /// [`CoreError::NumericalInvalid`] for nonfinite inputs, state outside
    /// `[-1, 1]`, or nonfinite activations/outputs;
    /// [`CoreError::AllocationFailed`] if `scratch` must grow and cannot.
    pub fn evaluate_recurrent(
        &self,
        inputs: &[f64; SENSOR_WIDTH],
        state: &[f32],
        scratch: &mut ControllerScratch,
        next_state: &mut [f32],
    ) -> Result<[f32; ACTION_WIDTH], CoreError> {
        if self.topology.sensor_width() != SENSOR_WIDTH
            || self.topology.action_width() != ACTION_WIDTH
        {
            return Err(CoreError::LayoutMismatch {
                buffer: "controller topology",
                detail: format!(
                    "expected ({SENSOR_WIDTH}, {ACTION_WIDTH}) base widths, got ({}, {})",
                    self.topology.sensor_width(),
                    self.topology.action_width()
                ),
            });
        }
        let actions = self.evaluate_recurrent_slice(inputs, state, 0.0, scratch, next_state)?;
        actions
            .as_v1_array()
            .ok_or_else(|| CoreError::LayoutMismatch {
                buffer: "controller outputs",
                detail: format!("expected {ACTION_WIDTH} outputs, got {}", actions.len()),
            })
    }

    /// [`ReferenceController::evaluate_recurrent`], keeping each hidden
    /// layer's activations and the updated recurrent state (FL-021, FL-054).
    ///
    /// # Errors
    /// As [`ReferenceController::evaluate_recurrent`], and
    /// [`CoreError::AllocationFailed`] when the copies cannot be stored.
    pub fn trace_recurrent(
        &self,
        inputs: &[f64; SENSOR_WIDTH],
        state: &[f32],
        scratch: &mut ControllerScratch,
    ) -> Result<RecurrentTrace, CoreError> {
        let mut next_state = vec![0.0_f32; self.topology.recurrent_state_width()];
        let outputs = self.evaluate_recurrent(inputs, state, scratch, &mut next_state)?;
        let hidden = copy_hidden(self.topology, scratch)?;
        Ok(RecurrentTrace {
            hidden,
            outputs,
            next_state,
        })
    }

    /// Core forward pass over pre-narrowed F32 inputs and weights of one dtype.
    fn evaluate_raw_f32<W: Weight>(
        &self,
        parameters: &[W],
        inputs: &[f32],
        scratch: &mut ControllerScratch,
    ) -> Result<RawOutputs, CoreError> {
        let Some((output_layer, hidden_layers)) = self.topology.layers.split_last() else {
            return Err(CoreError::LayoutMismatch {
                buffer: "controller parameters",
                detail: "a topology without an output layer".into(),
            });
        };
        if hidden_layers.len() > MAXIMUM_HIDDEN_LAYERS {
            return Err(CoreError::LayoutMismatch {
                buffer: "controller parameters",
                detail: format!("{} hidden layers", hidden_layers.len()),
            });
        }
        scratch.reserve(self.topology.widest_hidden)?;
        let ControllerScratch { first, second } = scratch;

        let mut source: &[f32] = inputs;
        for (layer, activations) in hidden_layers.iter().zip([first, second]) {
            affine(parameters, layer, source, |_, z| {
                // `tanh_f32` is `tanh` in F64 rounded once to F32, the
                // declared arithmetic, bit for bit.
                let activated = tanh_f32(z).ok_or_else(|| CoreError::NumericalInvalid {
                    quantity: "controller pre-activation",
                    detail: format!("{z} overflowed"),
                })?;
                // The push never allocates: `reserve` sized it.
                activations.push(activated);
                Ok(())
            })?;
            source = activations.as_slice();
        }

        let mut raw = RawOutputs::zeroed(output_layer.outputs)?;
        let out_slice = raw.as_mut_slice();
        affine(parameters, output_layer, source, |output, z| {
            if !z.is_finite() {
                return Err(CoreError::NumericalInvalid {
                    quantity: "controller output",
                    detail: format!("output {output} is {z}"),
                });
            }
            if let Some(slot) = out_slice.get_mut(output) {
                *slot = z;
            }
            Ok(())
        })?;
        Ok(raw)
    }

    /// [`ReferenceController::evaluate_raw`], keeping each hidden layer's
    /// activations for variable-width topologies (FL-021, V2-01).
    ///
    /// # Errors
    /// As [`ReferenceController::evaluate_raw`], and
    /// [`CoreError::AllocationFailed`] when the copies cannot be stored.
    pub fn trace_raw(
        &self,
        inputs: &[f64],
        scratch: &mut ControllerScratch,
    ) -> Result<RawTrace, CoreError> {
        let outputs = self.evaluate_raw(inputs, scratch)?;
        let hidden = copy_hidden(self.topology, scratch)?;
        Ok(RawTrace { hidden, outputs })
    }

    /// [`ReferenceController::evaluate`], keeping each hidden layer's
    /// activations: the inspector's sampled activations (FL-021, plan
    /// §14.2). The outputs are `evaluate`'s, bit for bit; the activations
    /// are the F32 values the next layer read.
    ///
    /// # Errors
    /// As [`ReferenceController::evaluate`], and
    /// [`CoreError::AllocationFailed`] when the copies cannot be stored.
    pub fn trace(
        &self,
        inputs: &[f64; SENSOR_WIDTH],
        scratch: &mut ControllerScratch,
    ) -> Result<Trace, CoreError> {
        let outputs = self.evaluate(inputs, scratch)?;
        let hidden = copy_hidden(self.topology, scratch)?;
        Ok(Trace { hidden, outputs })
    }
}

fn copy_hidden(
    topology: &DenseTopology,
    scratch: &ControllerScratch,
) -> Result<Vec<Vec<f32>>, CoreError> {
    let layers = topology.layers.len().saturating_sub(1);
    let mut hidden = Vec::new();
    for activations in [&scratch.first, &scratch.second].into_iter().take(layers) {
        let mut copy = Vec::new();
        copy.try_reserve_exact(activations.len())
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "traced activations",
                bytes: u64::try_from(activations.len().saturating_mul(size_of::<f32>()))
                    .unwrap_or(u64::MAX),
            })?;
        copy.extend_from_slice(activations);
        hidden.push(copy);
    }
    Ok(hidden)
}

/// One evaluation with its hidden activations
/// ([`ReferenceController::trace`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Trace {
    /// Each hidden layer's activations, in evaluation order: `tanh` of its
    /// pre-activations, rounded once to F32.
    pub hidden: Vec<Vec<f32>>,
    /// The 36 raw outputs of the linear final layer.
    pub outputs: [f32; ACTION_WIDTH],
}

/// One variable-width evaluation with its hidden activations
/// ([`ReferenceController::trace_raw`], V2-01).
#[derive(Debug, Clone, PartialEq)]
pub struct RawTrace {
    /// Each hidden layer's activations, in evaluation order.
    pub hidden: Vec<Vec<f32>>,
    /// All raw outputs of the linear final layer (`<= MAX_CONTROLLER_OUTPUT_WIDTH`).
    pub outputs: RawOutputs,
}

/// One recurrent evaluation with its hidden activations and updated internal
/// state ([`ReferenceController::trace_recurrent`], FL-054).
#[derive(Debug, Clone, PartialEq)]
pub struct RecurrentTrace {
    /// Each hidden layer's activations, in evaluation order.
    pub hidden: Vec<Vec<f32>>,
    /// The 36 raw action outputs of the linear final layer.
    pub outputs: [f32; ACTION_WIDTH],
    /// The updated bounded internal state `h' ∈ [-1, 1]^S`.
    pub next_state: Vec<f32>,
}

/// The raw outputs as the F64 values [`crate::actions::decode`] reads.
/// Widening is exact.
#[must_use]
pub fn widen(raw: &[f32; ACTION_WIDTH]) -> [f64; ACTION_WIDTH] {
    raw.map(f64::from)
}

/// Widen a variable-width slice of raw F32 outputs into `out` (`out.len() == raw.len()`).
/// Widening is exact.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] if `raw.len() != out.len()`.
pub fn widen_slice(raw: &[f32], out: &mut [f64]) -> Result<(), CoreError> {
    if raw.len() != out.len() {
        return Err(CoreError::LayoutMismatch {
            buffer: "controller outputs",
            detail: format!(
                "cannot widen {} outputs into {} slots",
                raw.len(),
                out.len()
            ),
        });
    }
    for (dst, &src) in out.iter_mut().zip(raw) {
        *dst = f64::from(src);
    }
    Ok(())
}

/// One affine layer in the declared order: for each output neuron, the
/// bias, then each input's product in ascending index, all in F32 with each
/// weight read as [`Weight::widen`] gives it. `emit` receives each neuron's
/// `z` in output order.
fn affine<W: Weight>(
    parameters: &[W],
    layer: &LayerShape,
    input: &[f32],
    mut emit: impl FnMut(usize, f32) -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    let mismatch = |detail: String| CoreError::LayoutMismatch {
        buffer: "controller parameters",
        detail,
    };
    if input.len() != layer.inputs || layer.inputs == 0 {
        return Err(mismatch(format!(
            "a layer of fan-in {} read {} activations",
            layer.inputs,
            input.len()
        )));
    }
    let weights = parameters
        .get(layer.weights..layer.biases)
        .ok_or_else(|| mismatch(format!("weights {}..{}", layer.weights, layer.biases)))?;
    let biases = parameters
        .get(layer.biases..layer.end())
        .ok_or_else(|| mismatch(format!("biases {}..{}", layer.biases, layer.end())))?;
    for (output, (row, &bias)) in weights.chunks_exact(layer.inputs).zip(biases).enumerate() {
        let mut z = bias.widen();
        for (&weight, &value) in row.iter().zip(input) {
            z += weight.widen() * value;
        }
        emit(output, z)?;
    }
    Ok(())
}
