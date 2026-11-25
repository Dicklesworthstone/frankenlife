//! The sensor ABI `energy-moment-gradient-104-v1` with
//! `fixed-local-scales-v1` normalization (plan §6.1, A.1).
//!
//! From the blended raw densities `R, M` and their physical derivatives:
//! - regularized composition `Z = M/(R + R_floor)` with
//!   `∂Z = (∂M − Z ∂R)/(R + R_floor)`;
//! - energy observation `q = R/(R + R_scale)` with
//!   `∂q = R_scale ∂R/(R + R_scale)²`;
//! - neural gradient inputs `tanh(L_gradient ∂q)` and
//!   `tanh(L_gradient ∂Z_k)`, with a fixed model length `L_gradient`;
//! - intrinsic inputs `E/(E + E_scale)`, `age/(age + age_scale)`,
//!   `tanh(v_x/v_scale)`, `tanh(v_y/v_scale)` and
//!   `tanh(energy_change_rate/change_scale)`.
//!
//! The order is the values `[q, Z_0, …, Z_31]`, their x derivatives in that
//! order, their y derivatives in that order, then the five intrinsic inputs.
//! Motion uses the unsaturated physical derivatives of `q` and `Z`. Every
//! normalizer is a fixed local constant: no population statistic reaches a
//! controller (A.1).

use std::ops::Range;

use flife_core::numerics::{tanh_f64_as_f32, tanh_f64_as_f32_4};
use flife_core::{CoreError, V2Abi, ValidatedConfig};
use flife_field::grid::{
    CARRIER0_MOMENT_CHANNELS, CARRIER1_ENERGY_PLANE, CARRIER1_FIRST_MOMENT_PLANE,
    CARRIER1_MOMENT_CHANNELS,
};
use flife_field::numerics::tanh;
use flife_field::pyramid::{DualRawSample, PLANES, RawSample};

use crate::controller::{
    INDEPENDENT_DENSE_MLP_V1, INDEPENDENT_RECURRENT_MLP_V1, MAXIMUM_RECURRENT_STATE_WIDTH,
};

/// The sensor schema this module implements.
pub const ENERGY_MOMENT_GRADIENT_104_V1: &str = "energy-moment-gradient-104-v1";

/// The sensor normalization this module implements.
pub const FIXED_LOCAL_SCALES_V1: &str = "fixed-local-scales-v1";

/// Baseline `field-ecology-v1` controller inputs (`3 * 33 + 5 = 104`).
pub const SENSOR_WIDTH: usize = 3 * PLANES + INTRINSIC_WIDTH;

/// Maximum controller input width across all `field-ecology-v2` feature
/// combinations and recurrent state widths (matches `WIDEST_HIDDEN = 128`).
pub const MAX_SENSOR_WIDTH: usize = 128;

/// Number of extra inputs appended by `dual-carrier-trails`:
/// `(q_1, tanh(L ∂_x q_1), tanh(L ∂_y q_1), ‖Z^(1)‖_2.min(1.0))`.
pub const DUAL_CARRIER_TRAIL_INPUTS: usize = 4;

/// Number of extra inputs appended by `hippo-crowding` (`expose_sensor`):
/// `c_den = N_loc / (N_loc + 4)`.
pub const HIPPO_CROWDING_INPUTS: usize = 1;

/// Number of extra inputs appended by `notch-delta`
/// (`mode == "sensor-neighbor-mean-v1"`): `last_neighbor_h0`.
pub const NOTCH_DELTA_INPUTS: usize = 1;

/// Pure scientific sensor input layout (V2-01).
///
/// Computed solely from scientific model configuration and feature flags —
/// identical on Reference CPU, FrankenTorch CPU, and Metal. Never adds silent
/// Metal-only padding lanes (AGENTS.md Rule 15).
///
/// Fixed slot order:
/// - `0..104`: baseline `energy-moment-gradient-104-v1` (immutable);
/// - `+4` if `dual-carrier-trails`;
/// - `+1` if `hippo-crowding` with `expose_sensor`;
/// - `+1` if `notch-delta` with `mode == "sensor-neighbor-mean-v1"`;
/// - trailing `+S` if recurrent / `epigenetic-recurrence` (`h_0..h_{S-1}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SensorLayout {
    dual_carrier_trails: Option<usize>,
    hippo_crowding: Option<usize>,
    notch_delta: Option<usize>,
    observation_width: usize,
    recurrent_state_width: usize,
}

impl SensorLayout {
    /// Baseline stateless `field-ecology-v1` layout (`104` inputs).
    #[must_use]
    pub const fn v1() -> Self {
        Self {
            dual_carrier_trails: None,
            hippo_crowding: None,
            notch_delta: None,
            observation_width: SENSOR_WIDTH,
            recurrent_state_width: 0,
        }
    }

    /// `field-ecology-v1` layout with FL-054 recurrent state width `state_width`
    /// (`104 + S` inputs).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] if `state_width` exceeds
    /// [`MAXIMUM_RECURRENT_STATE_WIDTH`].
    pub fn v1_recurrent(state_width: usize) -> Result<Self, CoreError> {
        Self::new(false, false, false, state_width)
    }

    /// Build a canonical pure-scientific [`SensorLayout`] from the active
    /// layout-affecting feature flags and recurrent state width `S`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] if `recurrent_state_width` exceeds
    /// [`MAXIMUM_RECURRENT_STATE_WIDTH`] or the total width exceeds
    /// [`MAX_SENSOR_WIDTH`].
    pub fn new(
        dual_carrier_trails: bool,
        hippo_crowding_sensor: bool,
        notch_delta_sensor: bool,
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
        let mut cursor = SENSOR_WIDTH;
        let dual_carrier_trails = if dual_carrier_trails {
            let start = cursor;
            cursor += DUAL_CARRIER_TRAIL_INPUTS;
            Some(start)
        } else {
            None
        };
        let hippo_crowding = if hippo_crowding_sensor {
            let start = cursor;
            cursor += HIPPO_CROWDING_INPUTS;
            Some(start)
        } else {
            None
        };
        let notch_delta = if notch_delta_sensor {
            let start = cursor;
            cursor += NOTCH_DELTA_INPUTS;
            Some(start)
        } else {
            None
        };
        let observation_width = cursor;
        let total = observation_width + recurrent_state_width;
        if total > MAX_SENSOR_WIDTH {
            return Err(CoreError::NumericalInvalid {
                quantity: "sensors.input_width",
                detail: format!("sensor input width {total} exceeds maximum {MAX_SENSOR_WIDTH}"),
            });
        }
        Ok(Self {
            dual_carrier_trails,
            hippo_crowding,
            notch_delta,
            observation_width,
            recurrent_state_width,
        })
    }

    /// Build a [`SensorLayout`] directly from `flife-core`'s canonical
    /// [`V2Abi`].
    ///
    /// # Errors
    /// As [`SensorLayout::new`] if `abi` carries an out-of-range state width
    /// or total width.
    pub fn from_v2_abi(abi: V2Abi) -> Result<Self, CoreError> {
        let state_width =
            usize::try_from(abi.recurrent_state_width).map_err(|_| CoreError::Overflow {
                quantity: "controller.recurrent_state_width",
            })?;
        let layout = Self::new(
            abi.dual_carrier_sensors,
            abi.hippo_crowding_sensor,
            abi.notch_delta_sensor,
            state_width,
        )?;
        if layout.observation_width() != abi.sensor_width as usize
            || layout.input_width() != abi.input_width as usize
        {
            return Err(CoreError::LayoutMismatch {
                buffer: "sensor layout",
                detail: format!(
                    "V2Abi ({}/{}) disagrees with SensorLayout ({}/{})",
                    abi.sensor_width,
                    abi.input_width,
                    layout.observation_width(),
                    layout.input_width()
                ),
            });
        }
        Ok(layout)
    }

    /// Resolve the sensor layout of a validated configuration (`v1` or `v2`).
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

    /// Non-recurrent sensor observation width (`104` + active v2 sensor lanes).
    #[must_use]
    pub const fn observation_width(&self) -> usize {
        self.observation_width
    }

    /// Trailing recurrent internal state width `S` (`0` when stateless).
    #[must_use]
    pub const fn recurrent_state_width(&self) -> usize {
        self.recurrent_state_width
    }

    /// Total network Layer-0 input width (`observation_width + S`, `<= 128`).
    #[must_use]
    pub const fn input_width(&self) -> usize {
        self.observation_width + self.recurrent_state_width
    }

    /// Start index of the 4 `dual-carrier-trails` inputs, if active.
    #[must_use]
    pub const fn dual_carrier_trails(&self) -> Option<usize> {
        self.dual_carrier_trails
    }

    /// Index of the `hippo-crowding` `c_den` input, if active.
    #[must_use]
    pub const fn hippo_crowding(&self) -> Option<usize> {
        self.hippo_crowding
    }

    /// Index of the `notch-delta` `last_neighbor_h0` input, if active.
    #[must_use]
    pub const fn notch_delta(&self) -> Option<usize> {
        self.notch_delta
    }

    /// Start index of the trailing recurrent state inputs `h_0..h_{S-1}`, if
    /// `S > 0`.
    #[must_use]
    pub const fn recurrent_state(&self) -> Option<usize> {
        if self.recurrent_state_width > 0 {
            Some(self.observation_width)
        } else {
            None
        }
    }

    /// Index range of the trailing recurrent state inputs (empty when `S == 0`).
    #[must_use]
    pub const fn recurrent_state_range(&self) -> Range<usize> {
        self.observation_width..self.input_width()
    }

    /// Whether `input_width()` is a multiple of 4 (eligible for coalesced
    /// `float4`/`half4` Layer-0 loads on Metal).
    #[must_use]
    pub const fn is_aligned4(&self) -> bool {
        self.input_width().is_multiple_of(4)
    }
}

/// Index of `q`; `Z_k` follows at `VALUES + 1 + k`.
pub const VALUES: usize = 0;
/// Index of `tanh(L ∂q/∂x)`; channel `k` follows at `X_DERIVATIVES + 1 + k`.
pub const X_DERIVATIVES: usize = PLANES;
/// Index of `tanh(L ∂q/∂y)`; channel `k` follows at `Y_DERIVATIVES + 1 + k`.
pub const Y_DERIVATIVES: usize = 2 * PLANES;
/// Index of `E/(E + E_scale)`.
pub const INTERNAL_ENERGY: usize = 3 * PLANES;
/// Index of `age/(age + age_scale)`.
pub const AGE: usize = INTERNAL_ENERGY + 1;
/// Index of `tanh(v_x/v_scale)`.
pub const VELOCITY_X: usize = INTERNAL_ENERGY + 2;
/// Index of `tanh(v_y/v_scale)`.
pub const VELOCITY_Y: usize = INTERNAL_ENERGY + 3;
/// Index of `tanh(energy_change_rate/change_scale)`.
pub const ENERGY_CHANGE: usize = INTERNAL_ENERGY + 4;

const INTRINSIC_WIDTH: usize = 5;

/// The fixed local normalizers of `fixed-local-scales-v1` and the admitted
/// sensing-scale range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SensorScales {
    energy_density: f64,
    gradient_length: f64,
    composition_floor: f64,
    internal_energy: f64,
    age_seconds: f64,
    velocity: f64,
    energy_change_per_second: f64,
    minimum_scale: f64,
    maximum_scale: f64,
}

impl SensorScales {
    /// Normalizers `[R_scale, L_gradient, R_floor, E_scale, age_scale,
    /// v_scale, change_scale]` (all positive) and the sensing-scale range
    /// `minimum ≤ maximum`, both positive.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonpositive or nonfinite value,
    /// or a range out of order.
    pub fn new(
        normalizers: [f64; 7],
        minimum_scale: f64,
        maximum_scale: f64,
    ) -> Result<Self, CoreError> {
        const NAMES: [&str; 7] = [
            "sensors.energy_density_scale",
            "sensors.gradient_length_scale_world_units",
            "sensors.composition_energy_floor_density",
            "sensors.internal_energy_scale",
            "sensors.age_scale_seconds",
            "sensors.velocity_scale",
            "sensors.energy_change_scale_per_second",
        ];
        for (quantity, value) in NAMES.into_iter().zip(normalizers).chain([
            ("sensors.minimum_scale_world_units", minimum_scale),
            ("sensors.maximum_scale_world_units", maximum_scale),
        ]) {
            if !value.is_finite() || value <= 0.0 {
                return Err(CoreError::NumericalInvalid {
                    quantity,
                    detail: format!("{value} is not a positive scale"),
                });
            }
        }
        if minimum_scale > maximum_scale {
            return Err(CoreError::NumericalInvalid {
                quantity: "sensors.maximum_scale_world_units",
                detail: format!("{maximum_scale} is below the minimum {minimum_scale}"),
            });
        }
        let [
            energy_density,
            gradient_length,
            composition_floor,
            internal_energy,
            age_seconds,
            velocity,
            energy_change_per_second,
        ] = normalizers;
        Ok(Self {
            energy_density,
            gradient_length,
            composition_floor,
            internal_energy,
            age_seconds,
            velocity,
            energy_change_per_second,
            minimum_scale,
            maximum_scale,
        })
    }

    /// The normalizers of a validated configuration.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for another sensor schema or
    /// normalization; otherwise as [`SensorScales::new`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let sensors = &config.config().sensors;
        if sensors.schema != ENERGY_MOMENT_GRADIENT_104_V1 {
            return Err(CoreError::UnknownAlgorithm {
                role: "sensors.schema",
                name: sensors.schema.clone(),
            });
        }
        if sensors.normalization != FIXED_LOCAL_SCALES_V1 {
            return Err(CoreError::UnknownAlgorithm {
                role: "sensors.normalization",
                name: sensors.normalization.clone(),
            });
        }
        Self::new(
            [
                sensors.energy_density_scale,
                sensors.gradient_length_scale_world_units,
                sensors.composition_energy_floor_density,
                sensors.internal_energy_scale,
                sensors.age_scale_seconds,
                sensors.velocity_scale,
                sensors.energy_change_scale_per_second,
            ],
            sensors.minimum_scale_world_units,
            sensors.maximum_scale_world_units,
        )
    }

    /// `R_floor`, the positive density that regularizes composition.
    #[must_use]
    pub const fn composition_floor(&self) -> f64 {
        self.composition_floor
    }

    /// `L_gradient`, the fixed model length for gradient inputs.
    #[must_use]
    pub const fn gradient_length(&self) -> f64 {
        self.gradient_length
    }

    /// Whether `scale` is an admitted sensing scale.
    #[must_use]
    pub fn admits_scale(&self, scale: f64) -> bool {
        (self.minimum_scale..=self.maximum_scale).contains(&scale)
    }
}

/// An organism's own state, as its intrinsic inputs see it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Intrinsic {
    /// Internal energy `E ≥ 0`.
    pub energy: f64,
    /// Age in simulation-seconds, `≥ 0`.
    pub age_seconds: f64,
    /// Velocity, world-length per second.
    pub velocity: [f64; 2],
    /// The committed one-tick energy change rate, per second (§8.2).
    pub energy_change_per_second: f64,
}

/// One organism's encoded observation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observation {
    /// The 104 controller inputs in ABI order.
    pub inputs: [f64; SENSOR_WIDTH],
    /// `[∂q, ∂Z_0, …, ∂Z_31]` as `[∂/∂x, ∂/∂y]`, unsaturated, per
    /// world-length: what motion follows (§5.4).
    pub gradients: [[f64; 2]; PLANES],
}

/// Encode `raw` (blended at the organism's sensing scale) and `intrinsic`.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for a negative or nonfinite density, a
/// negative energy or age, or any nonfinite input.
pub fn observe(
    raw: &RawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
) -> Result<Observation, CoreError> {
    observe_with(raw, intrinsic, scales, tanh)
}

/// Encode `raw` and `intrinsic` for a controller whose first layer narrows
/// every input to F32 (`input as f32`).
///
/// The unsaturated F64 `gradients` (used by motion) are identical to
/// [`observe`], and each `tanh`-saturated entry in `inputs` holds the exact
/// F32 rounding of `tanh(v)` widened to F64 via [`tanh_f64_as_f32_4`], so
/// narrowing `inputs` to F32 produces the exact same bits as [`observe`].
///
/// # Errors
/// As [`observe`].
pub fn observe_for_controller(
    raw: &RawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
) -> Result<Observation, CoreError> {
    let (values, gradients, energy_norm, age_norm) = normalize_single(raw, intrinsic, scales)?;
    let (x_f32, y_f32, vx, vy, de) = saturate_single_gradients_f32(&gradients, intrinsic, scales)?;
    let mut inputs = [0.0; SENSOR_WIDTH];
    let (observed, own) = inputs.split_at_mut(3 * PLANES);
    let (value_slots, derivative_slots) = observed.split_at_mut(PLANES);
    let (x_slots, y_slots) = derivative_slots.split_at_mut(PLANES);
    value_slots.copy_from_slice(&values);
    for (dst, &src) in x_slots.iter_mut().zip(&x_f32) {
        *dst = f64::from(src);
    }
    for (dst, &src) in y_slots.iter_mut().zip(&y_f32) {
        *dst = f64::from(src);
    }
    own.copy_from_slice(&[
        energy_norm,
        age_norm,
        f64::from(vx),
        f64::from(vy),
        f64::from(de),
    ]);
    check_finite_single(&inputs, &gradients)?;
    Ok(Observation { inputs, gradients })
}

#[cold]
#[inline(never)]
fn err_sensor_invalid(quantity: &'static str, detail: String) -> CoreError {
    CoreError::NumericalInvalid { quantity, detail }
}

#[cold]
#[inline(never)]
fn err_nonfinite_sensor_value(value: f64) -> CoreError {
    CoreError::NumericalInvalid {
        quantity: "sensor input",
        detail: format!("{value} is not finite"),
    }
}

#[cold]
#[inline(never)]
fn err_tanh_undef(value: f64) -> CoreError {
    CoreError::NumericalInvalid {
        quantity: "sensor input",
        detail: format!("tanh({value}) is undefined"),
    }
}

#[cold]
#[inline(never)]
fn err_tanh4_undef(xs: [f64; 4]) -> CoreError {
    CoreError::NumericalInvalid {
        quantity: "sensor input",
        detail: format!("tanh({xs:?}) is undefined"),
    }
}

#[cold]
#[inline(never)]
fn check_finite_single_parts_slow(
    values: &[f64; PLANES],
    energy_norm: f64,
    age_norm: f64,
    gradients: &[[f64; 2]; PLANES],
) -> Result<(), CoreError> {
    for &value in values.iter().chain(&[energy_norm, age_norm]) {
        if !value.is_finite() {
            return Err(err_nonfinite_sensor_value(value));
        }
    }
    check_finite_gradients_slow(gradients)
}

#[cold]
#[inline(never)]
fn check_finite_gradients_slow(gradients: &[[f64; 2]; PLANES]) -> Result<(), CoreError> {
    for &[gx, gy] in gradients {
        if !gx.is_finite() {
            return Err(err_nonfinite_sensor_value(gx));
        }
        if !gy.is_finite() {
            return Err(err_nonfinite_sensor_value(gy));
        }
    }
    Ok(())
}

/// Encode `raw` and `intrinsic` directly into a controller's F32 input buffer
/// `out_inputs_f32` and caller-owned unsaturated F64 `out_gradients`, with
/// zero intermediate array copies or large struct returns.
///
/// Every entry written to `out_inputs_f32` is bit-identical to
/// `observe(raw, intrinsic, scales)?.inputs[k] as f32`, and `out_gradients` is
/// bit-identical to `observe(raw, intrinsic, scales)?.gradients`.
///
/// # Errors
/// As [`observe`].
#[inline]
pub fn observe_for_controller_f32_into(
    raw: &RawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
    out_inputs_f32: &mut [f32; SENSOR_WIDTH],
    out_gradients: &mut [[f64; 2]; PLANES],
) -> Result<(), CoreError> {
    let (observed, own) = out_inputs_f32.split_at_mut(3 * PLANES);
    let (value_slots, derivative_slots) = observed.split_at_mut(PLANES);
    let (x_slots, y_slots) = derivative_slots.split_at_mut(PLANES);
    let (energy_norm, age_norm, val_finite_acc) =
        normalize_single_to_f32(raw, intrinsic, scales, value_slots, out_gradients)?;
    let (vx, vy, de) =
        saturate_single_gradients_f32_into(out_gradients, intrinsic, scales, x_slots, y_slots)?;
    let mut finite_acc = val_finite_acc + (energy_norm + age_norm);
    for &[gx, gy] in out_gradients.iter() {
        finite_acc += gx + gy;
    }
    if !finite_acc.is_finite() {
        return check_finite_single_from_raw_slow(
            raw,
            intrinsic,
            scales,
            energy_norm,
            age_norm,
            out_gradients,
        );
    }
    own.copy_from_slice(&[energy_norm as f32, age_norm as f32, vx, vy, de]);
    Ok(())
}

/// Encode `raw` and `intrinsic` directly into a controller's F32 input buffer
/// `out_inputs_f32` via 4-lane [`tanh_f64_as_f32_4`], returning the exact
/// unsaturated F64 motion gradients without an intermediate `[f64; 104]`
/// widening/narrowing round-trip.
///
/// Every entry written to `out_inputs_f32` is bit-identical to
/// `observe(raw, intrinsic, scales)?.inputs[k] as f32`, and the returned
/// gradients are bit-identical to `observe(raw, intrinsic, scales)?.gradients`.
///
/// # Errors
/// As [`observe`].
#[inline]
pub fn observe_for_controller_f32(
    raw: &RawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
    out_inputs_f32: &mut [f32; SENSOR_WIDTH],
) -> Result<[[f64; 2]; PLANES], CoreError> {
    let mut gradients = [[0.0; 2]; PLANES];
    observe_for_controller_f32_into(raw, intrinsic, scales, out_inputs_f32, &mut gradients)?;
    Ok(gradients)
}

/// Write only the unsaturated F64 motion gradients directly into
/// `out_gradients` for an organism whose controller is not due this tick
/// (skipping all 69 `tanh` controller-input saturations), while enforcing the
/// exact same normalization and finiteness error checks as [`observe`].
///
/// # Errors
/// As [`observe`].
#[inline]
pub fn observe_gradients_only_into(
    raw: &RawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
    out_gradients: &mut [[f64; 2]; PLANES],
) -> Result<(), CoreError> {
    let (energy_norm, age_norm, val_finite_acc) =
        normalize_single_gradients_into(raw, intrinsic, scales, out_gradients)?;
    let l = scales.gradient_length;
    let mut finite_acc = 0.0_f64;
    for &[gx, gy] in out_gradients.iter() {
        finite_acc += l * gx + l * gy;
    }
    let vx_arg = intrinsic.velocity[0] / scales.velocity;
    let vy_arg = intrinsic.velocity[1] / scales.velocity;
    let de_arg = intrinsic.energy_change_per_second / scales.energy_change_per_second;
    finite_acc += (vx_arg + vy_arg) + de_arg;
    if !finite_acc.is_finite() {
        for &[gx, gy] in out_gradients.iter() {
            let sx = l * gx;
            let sy = l * gy;
            if !sx.is_finite() || !sy.is_finite() {
                let bad = if !sx.is_finite() { sx } else { sy };
                return Err(err_tanh_undef(bad));
            }
        }
        for arg in [vx_arg, vy_arg, de_arg] {
            if !arg.is_finite() {
                return Err(err_tanh_undef(arg));
            }
        }
    }
    let mut parts_acc = val_finite_acc + (energy_norm + age_norm);
    for &[gx, gy] in out_gradients.iter() {
        parts_acc += gx + gy;
    }
    if !parts_acc.is_finite() {
        return check_finite_single_from_raw_slow(
            raw,
            intrinsic,
            scales,
            energy_norm,
            age_norm,
            out_gradients,
        );
    }
    Ok(())
}

/// Compute only the unsaturated F64 motion gradients for an organism whose
/// controller is not due this tick (skipping all 69 `tanh` controller-input
/// saturations), while enforcing the exact same normalization and finiteness
/// error checks as [`observe`].
///
/// # Errors
/// As [`observe`].
#[inline]
pub fn observe_gradients_only(
    raw: &RawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
) -> Result<[[f64; 2]; PLANES], CoreError> {
    let mut gradients = [[0.0; 2]; PLANES];
    observe_gradients_only_into(raw, intrinsic, scales, &mut gradients)?;
    Ok(gradients)
}

#[cold]
#[inline(never)]
fn check_finite_single_from_raw_slow(
    raw: &RawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
    energy_norm: f64,
    age_norm: f64,
    gradients: &[[f64; 2]; PLANES],
) -> Result<(), CoreError> {
    let (values, _, _, _) = normalize_single(raw, intrinsic, scales)?;
    check_finite_single_parts_slow(&values, energy_norm, age_norm, gradients)
}

#[inline]
fn saturate_4(xs: [f64; 4]) -> Result<[f32; 4], CoreError> {
    tanh_f64_as_f32_4(xs).ok_or_else(|| err_tanh4_undef(xs))
}

type NormalizedSingle = ([f64; PLANES], [[f64; 2]; PLANES], f64, f64);
type SaturatedSingle = ([f32; PLANES], [f32; PLANES], f32, f32, f32);

#[inline]
fn validate_single_denoms(
    density: f64,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
) -> Result<(f64, f64, f64, f64), CoreError> {
    if !density.is_finite() || density < 0.0 {
        return Err(err_sensor_invalid(
            "sensed energy density",
            format!("{density} is not a density"),
        ));
    }
    if !intrinsic.energy.is_finite() || intrinsic.energy < 0.0 {
        return Err(err_sensor_invalid(
            "internal energy",
            format!("{} is not an energy", intrinsic.energy),
        ));
    }
    if !intrinsic.age_seconds.is_finite() || intrinsic.age_seconds < 0.0 {
        return Err(err_sensor_invalid(
            "age",
            format!("{} is not an age", intrinsic.age_seconds),
        ));
    }
    let shifted = density + scales.energy_density;
    let regularized = density + scales.composition_floor;
    let energy_denom = intrinsic.energy + scales.internal_energy;
    let age_denom = intrinsic.age_seconds + scales.age_seconds;
    if !shifted.is_finite()
        || !regularized.is_finite()
        || !energy_denom.is_finite()
        || !age_denom.is_finite()
    {
        return Err(err_sensor_invalid(
            "sensor normalization denominator",
            "density, energy, or age scale sum overflowed".to_owned(),
        ));
    }
    Ok((shifted, regularized, energy_denom, age_denom))
}

#[inline]
fn normalize_single_to_f32(
    raw: &RawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
    value_slots: &mut [f32],
    out_gradients: &mut [[f64; 2]; PLANES],
) -> Result<(f64, f64, f64), CoreError> {
    let [density, ref moments @ ..] = raw.values;
    let [density_x, ref moments_x @ ..] = raw.dx;
    let [density_y, ref moments_y @ ..] = raw.dy;
    let (shifted, regularized, energy_denom, age_denom) =
        validate_single_denoms(density, intrinsic, scales)?;
    let slope = scales.energy_density / (shifted * shifted);
    let q0 = density / shifted;
    let mut val_acc = q0;
    if let Some(slot) = value_slots.first_mut() {
        *slot = q0 as f32;
    }
    out_gradients[0] = [slope * density_x, slope * density_y];
    for ((slot, gradient), ((&moment, &moment_x), &moment_y)) in value_slots
        .iter_mut()
        .skip(1)
        .zip(out_gradients.iter_mut().skip(1))
        .zip(moments.iter().zip(moments_x).zip(moments_y))
    {
        let z = moment / regularized;
        val_acc += z;
        *slot = z as f32;
        *gradient = [
            (moment_x - z * density_x) / regularized,
            (moment_y - z * density_y) / regularized,
        ];
    }
    Ok((
        intrinsic.energy / energy_denom,
        intrinsic.age_seconds / age_denom,
        val_acc,
    ))
}

#[inline]
fn normalize_single_gradients_into(
    raw: &RawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
    out_gradients: &mut [[f64; 2]; PLANES],
) -> Result<(f64, f64, f64), CoreError> {
    let [density, ref moments @ ..] = raw.values;
    let [density_x, ref moments_x @ ..] = raw.dx;
    let [density_y, ref moments_y @ ..] = raw.dy;
    let (shifted, regularized, energy_denom, age_denom) =
        validate_single_denoms(density, intrinsic, scales)?;
    let slope = scales.energy_density / (shifted * shifted);
    let mut val_acc = density / shifted;
    out_gradients[0] = [slope * density_x, slope * density_y];
    for (gradient, ((&moment, &moment_x), &moment_y)) in out_gradients
        .iter_mut()
        .skip(1)
        .zip(moments.iter().zip(moments_x).zip(moments_y))
    {
        let z = moment / regularized;
        val_acc += z;
        *gradient = [
            (moment_x - z * density_x) / regularized,
            (moment_y - z * density_y) / regularized,
        ];
    }
    Ok((
        intrinsic.energy / energy_denom,
        intrinsic.age_seconds / age_denom,
        val_acc,
    ))
}

#[inline]
fn normalize_single(
    raw: &RawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
) -> Result<NormalizedSingle, CoreError> {
    let [density, ref moments @ ..] = raw.values;
    let [density_x, ref moments_x @ ..] = raw.dx;
    let [density_y, ref moments_y @ ..] = raw.dy;
    let (shifted, regularized, energy_denom, age_denom) =
        validate_single_denoms(density, intrinsic, scales)?;

    let mut values = [0.0; PLANES];
    let mut gradients = [[0.0; 2]; PLANES];
    let slope = scales.energy_density / (shifted * shifted);
    if let (Some(value), Some(gradient)) = (values.first_mut(), gradients.first_mut()) {
        *value = density / shifted;
        *gradient = [slope * density_x, slope * density_y];
    }
    for ((value, gradient), ((&moment, &moment_x), &moment_y)) in values
        .iter_mut()
        .zip(gradients.iter_mut())
        .skip(1)
        .zip(moments.iter().zip(moments_x).zip(moments_y))
    {
        let z = moment / regularized;
        *value = z;
        *gradient = [
            (moment_x - z * density_x) / regularized,
            (moment_y - z * density_y) / regularized,
        ];
    }
    Ok((
        values,
        gradients,
        intrinsic.energy / energy_denom,
        intrinsic.age_seconds / age_denom,
    ))
}

#[inline]
fn saturate_single_gradients_f32_into(
    gradients: &[[f64; 2]; PLANES],
    intrinsic: &Intrinsic,
    scales: &SensorScales,
    x_f32: &mut [f32],
    y_f32: &mut [f32],
) -> Result<(f32, f32, f32), CoreError> {
    let l = scales.gradient_length;
    let [ex, ey, vx, vy] = saturate_4([
        l * gradients[0][0],
        l * gradients[0][1],
        intrinsic.velocity[0] / scales.velocity,
        intrinsic.velocity[1] / scales.velocity,
    ])?;
    if let (Some((x0, x_rest)), Some((y0, y_rest))) =
        (x_f32.split_first_mut(), y_f32.split_first_mut())
    {
        *x0 = ex;
        *y0 = ey;
        for ((x_chunk, y_chunk), grad_chunk) in x_rest
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(y_rest.as_chunks_mut::<4>().0.iter_mut())
            .zip(gradients[1..].as_chunks::<4>().0)
        {
            let tx = saturate_4([
                l * grad_chunk[0][0],
                l * grad_chunk[1][0],
                l * grad_chunk[2][0],
                l * grad_chunk[3][0],
            ])?;
            let ty = saturate_4([
                l * grad_chunk[0][1],
                l * grad_chunk[1][1],
                l * grad_chunk[2][1],
                l * grad_chunk[3][1],
            ])?;
            *x_chunk = tx;
            *y_chunk = ty;
        }
    }
    let de_arg = intrinsic.energy_change_per_second / scales.energy_change_per_second;
    let de = tanh_f64_as_f32(de_arg).ok_or_else(|| err_tanh_undef(de_arg))?;
    Ok((vx, vy, de))
}

#[inline]
fn saturate_single_gradients_f32(
    gradients: &[[f64; 2]; PLANES],
    intrinsic: &Intrinsic,
    scales: &SensorScales,
) -> Result<SaturatedSingle, CoreError> {
    let mut x_f32 = [0.0_f32; PLANES];
    let mut y_f32 = [0.0_f32; PLANES];
    let (vx, vy, de) =
        saturate_single_gradients_f32_into(gradients, intrinsic, scales, &mut x_f32, &mut y_f32)?;
    Ok((x_f32, y_f32, vx, vy, de))
}

fn check_finite_single(
    inputs: &[f64; SENSOR_WIDTH],
    gradients: &[[f64; 2]; PLANES],
) -> Result<(), CoreError> {
    for &value in inputs {
        if !value.is_finite() {
            return Err(err_nonfinite_sensor_value(value));
        }
    }
    check_finite_gradients(gradients)
}

#[inline]
fn check_finite_gradients(gradients: &[[f64; 2]; PLANES]) -> Result<(), CoreError> {
    let mut acc = 0.0_f64;
    for &[gx, gy] in gradients {
        acc += gx + gy;
    }
    if !acc.is_finite() {
        return check_finite_gradients_slow(gradients);
    }
    Ok(())
}

fn observe_with(
    raw: &RawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
    saturate_fn: impl Fn(f64) -> Option<f64>,
) -> Result<Observation, CoreError> {
    let invalid =
        |quantity: &'static str, detail: String| CoreError::NumericalInvalid { quantity, detail };
    let (values, gradients, energy_norm, age_norm) = normalize_single(raw, intrinsic, scales)?;

    let saturate = |value: f64| {
        saturate_fn(value)
            .ok_or_else(|| invalid("sensor input", format!("tanh({value}) is undefined")))
    };
    let mut inputs = [0.0; SENSOR_WIDTH];
    let (observed, own) = inputs.split_at_mut(3 * PLANES);
    let (value_slots, derivative_slots) = observed.split_at_mut(PLANES);
    let (x_slots, y_slots) = derivative_slots.split_at_mut(PLANES);
    value_slots.copy_from_slice(&values);
    for ((x_slot, y_slot), gradient) in x_slots.iter_mut().zip(y_slots.iter_mut()).zip(&gradients) {
        *x_slot = saturate(scales.gradient_length * gradient[0])?;
        *y_slot = saturate(scales.gradient_length * gradient[1])?;
    }
    own.copy_from_slice(&[
        energy_norm,
        age_norm,
        saturate(intrinsic.velocity[0] / scales.velocity)?,
        saturate(intrinsic.velocity[1] / scales.velocity)?,
        saturate(intrinsic.energy_change_per_second / scales.energy_change_per_second)?,
    ]);
    check_finite_single(&inputs, &gradients)?;
    Ok(Observation { inputs, gradients })
}

/// One organism's encoded observation on a 34-plane dual-carrier hybrid field
/// (`V2-15` `dual-carrier-trails`).
///
/// `base` holds the 104 baseline controller inputs (with `Z_{0..23}` normalized
/// by `R_0 + R_floor` and `Z_{24..31}` normalized by `R_1 + R_floor` so trails
/// never wash out near bright food) plus the exact 33 unsaturated motion
/// gradients `[∂q_0, ∂Z_0..∂Z_31]`; `trail_inputs` holds the 4 appended
/// Carrier-1 signals `[q_1, tanh(L ∂_x q_1), tanh(L ∂_y q_1), ||Z^(1)||_2.min(1.0)]`
/// placed at [`SensorLayout::dual_carrier_trails`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DualObservation {
    /// Base 104 controller inputs and 33 unsaturated motion gradients.
    pub base: Observation,
    /// Appended 4 Carrier-1 trail sensor channels (`q_1`, `tanh(L ∂_x q_1)`,
    /// `tanh(L ∂_y q_1)`, `‖Z^(1)‖_2.min(1.0)`).
    pub trail_inputs: [f64; DUAL_CARRIER_TRAIL_INPUTS],
}

/// Encode a 34-plane [`DualRawSample`] and `intrinsic` using `scales` and
/// Carrier-1 energy density scale `trail_energy_density_scale` (`V2-15`).
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for a nonpositive/nonfinite
/// `trail_energy_density_scale`, negative or nonfinite densities (`R_0`, `R_1`),
/// negative energy/age, or any nonfinite output.
pub fn observe_dual(
    raw: &DualRawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
    trail_energy_density_scale: f64,
) -> Result<DualObservation, CoreError> {
    observe_dual_with(raw, intrinsic, scales, trail_energy_density_scale, tanh)
}

/// [`observe_dual`] with 4-lane [`tanh_f64_as_f32_4`] saturation matching an
/// F32-input controller bit-for-bit.
///
/// # Errors
/// As [`observe_dual`].
pub fn observe_dual_for_controller(
    raw: &DualRawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
    trail_energy_density_scale: f64,
) -> Result<DualObservation, CoreError> {
    let dual = normalize_dual(raw, intrinsic, scales, trail_energy_density_scale)?;
    let sat =
        saturate_dual_gradients_f32(&dual.gradients, dual.dq1_x, dual.dq1_y, intrinsic, scales)?;
    let mut inputs = [0.0; SENSOR_WIDTH];
    let (observed, own) = inputs.split_at_mut(3 * PLANES);
    let (value_slots, derivative_slots) = observed.split_at_mut(PLANES);
    let (x_slots, y_slots) = derivative_slots.split_at_mut(PLANES);
    value_slots.copy_from_slice(&dual.values);
    for (dst, &src) in x_slots.iter_mut().zip(&sat.x) {
        *dst = f64::from(src);
    }
    for (dst, &src) in y_slots.iter_mut().zip(&sat.y) {
        *dst = f64::from(src);
    }
    own.copy_from_slice(&[
        dual.energy_norm,
        dual.age_norm,
        f64::from(sat.vx),
        f64::from(sat.vy),
        f64::from(sat.de),
    ]);
    let trail_inputs = [
        dual.q1,
        f64::from(sat.t1x),
        f64::from(sat.t1y),
        dual.z1_norm,
    ];
    for &value in inputs.iter().chain(&trail_inputs) {
        if !value.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "sensor input",
                detail: format!("{value} is not finite"),
            });
        }
    }
    check_finite_gradients(&dual.gradients)?;
    Ok(DualObservation {
        base: Observation {
            inputs,
            gradients: dual.gradients,
        },
        trail_inputs,
    })
}

#[cold]
#[inline(never)]
fn check_finite_dual_parts_slow(dual: &NormalizedDual) -> Result<(), CoreError> {
    for &value in
        dual.values
            .iter()
            .chain(&[dual.energy_norm, dual.age_norm, dual.q1, dual.z1_norm])
    {
        if !value.is_finite() {
            return Err(err_nonfinite_sensor_value(value));
        }
    }
    check_finite_gradients_slow(&dual.gradients)
}

/// Encode a 34-plane [`DualRawSample`] directly into F32 controller buffers
/// `out_inputs_f32`, `out_trail_f32`, and caller-owned unsaturated F64
/// `out_gradients`, eliminating intermediate gradient/array stack copies.
///
/// # Errors
/// As [`observe_dual`].
#[inline]
pub fn observe_dual_for_controller_f32_into(
    raw: &DualRawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
    trail_energy_density_scale: f64,
    out_inputs_f32: &mut [f32; SENSOR_WIDTH],
    out_trail_f32: &mut [f32; DUAL_CARRIER_TRAIL_INPUTS],
    out_gradients: &mut [[f64; 2]; PLANES],
) -> Result<(), CoreError> {
    let (observed, own) = out_inputs_f32.split_at_mut(3 * PLANES);
    let (value_slots, derivative_slots) = observed.split_at_mut(PLANES);
    let (x_slots, y_slots) = derivative_slots.split_at_mut(PLANES);
    let scalars = normalize_dual_to_f32(
        raw,
        intrinsic,
        scales,
        trail_energy_density_scale,
        Some(value_slots),
        out_gradients,
    )?;
    let (vx, vy, de, t1x, t1y) = saturate_dual_gradients_f32_into(
        out_gradients,
        scalars.dq1_x,
        scalars.dq1_y,
        intrinsic,
        scales,
        x_slots,
        y_slots,
    )?;
    let mut acc =
        scalars.val_acc + (scalars.energy_norm + scalars.age_norm) + (scalars.q1 + scalars.z1_norm);
    for &[gx, gy] in out_gradients.iter() {
        acc += gx + gy;
    }
    if !acc.is_finite() {
        return check_finite_dual_from_raw_slow(raw, intrinsic, scales, trail_energy_density_scale);
    }
    own.copy_from_slice(&[
        scalars.energy_norm as f32,
        scalars.age_norm as f32,
        vx,
        vy,
        de,
    ]);
    *out_trail_f32 = [scalars.q1 as f32, t1x, t1y, scalars.z1_norm as f32];
    Ok(())
}

/// Encode a 34-plane [`DualRawSample`] directly into F32 controller buffers
/// `out_inputs_f32` (104 base inputs) and `out_trail_f32` (4 Carrier-1 trail
/// inputs) via 4-lane [`tanh_f64_as_f32_4`], returning the exact unsaturated
/// F64 motion gradients.
///
/// Every F32 output is bit-identical to narrowing [`observe_dual`]'s outputs to
/// `f32`, and the returned gradients are bit-identical to [`observe_dual`].
///
/// # Errors
/// As [`observe_dual`].
#[inline]
pub fn observe_dual_for_controller_f32(
    raw: &DualRawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
    trail_energy_density_scale: f64,
    out_inputs_f32: &mut [f32; SENSOR_WIDTH],
    out_trail_f32: &mut [f32; DUAL_CARRIER_TRAIL_INPUTS],
) -> Result<[[f64; 2]; PLANES], CoreError> {
    let mut gradients = [[0.0; 2]; PLANES];
    observe_dual_for_controller_f32_into(
        raw,
        intrinsic,
        scales,
        trail_energy_density_scale,
        out_inputs_f32,
        out_trail_f32,
        &mut gradients,
    )?;
    Ok(gradients)
}

/// Write only the unsaturated F64 motion gradients directly into
/// `out_gradients` for a 34-plane dual-carrier organism whose controller is not
/// due this tick (skipping all 71 `tanh` saturations), while enforcing the
/// exact same normalization and finiteness error checks as [`observe_dual`].
///
/// # Errors
/// As [`observe_dual`].
#[inline]
pub fn observe_dual_gradients_only_into(
    raw: &DualRawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
    trail_energy_density_scale: f64,
    out_gradients: &mut [[f64; 2]; PLANES],
) -> Result<(), CoreError> {
    let scalars = normalize_dual_to_f32(
        raw,
        intrinsic,
        scales,
        trail_energy_density_scale,
        None,
        out_gradients,
    )?;
    let l = scales.gradient_length;
    let mut finite_acc = 0.0_f64;
    for &[gx, gy] in out_gradients.iter() {
        finite_acc += l * gx + l * gy;
    }
    let vx_arg = intrinsic.velocity[0] / scales.velocity;
    let vy_arg = intrinsic.velocity[1] / scales.velocity;
    let de_arg = intrinsic.energy_change_per_second / scales.energy_change_per_second;
    let t1x_arg = l * scalars.dq1_x;
    let t1y_arg = l * scalars.dq1_y;
    finite_acc += ((vx_arg + vy_arg) + de_arg) + (t1x_arg + t1y_arg);
    if !finite_acc.is_finite() {
        for &[gx, gy] in out_gradients.iter() {
            let sx = l * gx;
            let sy = l * gy;
            if !sx.is_finite() || !sy.is_finite() {
                let bad = if !sx.is_finite() { sx } else { sy };
                return Err(err_tanh_undef(bad));
            }
        }
        for arg in [vx_arg, vy_arg, de_arg, t1x_arg, t1y_arg] {
            if !arg.is_finite() {
                return Err(err_tanh_undef(arg));
            }
        }
    }
    let mut parts_acc =
        scalars.val_acc + (scalars.energy_norm + scalars.age_norm) + (scalars.q1 + scalars.z1_norm);
    for &[gx, gy] in out_gradients.iter() {
        parts_acc += gx + gy;
    }
    if !parts_acc.is_finite() {
        return check_finite_dual_from_raw_slow(raw, intrinsic, scales, trail_energy_density_scale);
    }
    Ok(())
}

/// Compute only the unsaturated F64 motion gradients for a 34-plane
/// dual-carrier organism whose controller is not due this tick (skipping all
/// 71 `tanh` saturations), while enforcing the exact same normalization and
/// finiteness error checks as [`observe_dual`].
///
/// # Errors
/// As [`observe_dual`].
#[inline]
pub fn observe_dual_gradients_only(
    raw: &DualRawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
    trail_energy_density_scale: f64,
) -> Result<[[f64; 2]; PLANES], CoreError> {
    let mut gradients = [[0.0; 2]; PLANES];
    observe_dual_gradients_only_into(
        raw,
        intrinsic,
        scales,
        trail_energy_density_scale,
        &mut gradients,
    )?;
    Ok(gradients)
}

#[cold]
#[inline(never)]
fn check_finite_dual_from_raw_slow(
    raw: &DualRawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
    trail_energy_density_scale: f64,
) -> Result<(), CoreError> {
    let dual = normalize_dual(raw, intrinsic, scales, trail_energy_density_scale)?;
    check_finite_dual_parts_slow(&dual)
}

struct DualScalars {
    val_acc: f64,
    energy_norm: f64,
    age_norm: f64,
    q1: f64,
    dq1_x: f64,
    dq1_y: f64,
    z1_norm: f64,
}

#[inline]
fn normalize_dual_to_f32(
    raw: &DualRawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
    trail_energy_density_scale: f64,
    mut value_slots: Option<&mut [f32]>,
    out_gradients: &mut [[f64; 2]; PLANES],
) -> Result<DualScalars, CoreError> {
    if !trail_energy_density_scale.is_finite() || trail_energy_density_scale <= 0.0 {
        return Err(err_sensor_invalid(
            "v2.carriers.trail_energy_density_scale",
            format!("{trail_energy_density_scale} is not a positive scale"),
        ));
    }
    let density0 = raw.values[0];
    let density0_x = raw.dx[0];
    let density0_y = raw.dy[0];
    let c1_e = CARRIER1_ENERGY_PLANE as usize;
    let density1 = raw.values[c1_e];
    let density1_x = raw.dx[c1_e];
    let density1_y = raw.dy[c1_e];
    if !density0.is_finite() || density0 < 0.0 {
        return Err(err_sensor_invalid(
            "sensed energy density",
            format!("{density0} is not a density"),
        ));
    }
    if !density1.is_finite() || density1 < 0.0 {
        return Err(err_sensor_invalid(
            "sensed trail energy density",
            format!("{density1} is not a density"),
        ));
    }
    if !intrinsic.energy.is_finite() || intrinsic.energy < 0.0 {
        return Err(err_sensor_invalid(
            "internal energy",
            format!("{} is not an energy", intrinsic.energy),
        ));
    }
    if !intrinsic.age_seconds.is_finite() || intrinsic.age_seconds < 0.0 {
        return Err(err_sensor_invalid(
            "age",
            format!("{} is not an age", intrinsic.age_seconds),
        ));
    }
    let shifted0 = density0 + scales.energy_density;
    let reg0 = density0 + scales.composition_floor;
    let shifted1 = density1 + trail_energy_density_scale;
    let reg1 = density1 + scales.composition_floor;
    let energy_denom = intrinsic.energy + scales.internal_energy;
    let age_denom = intrinsic.age_seconds + scales.age_seconds;
    if !shifted0.is_finite()
        || !reg0.is_finite()
        || !shifted1.is_finite()
        || !reg1.is_finite()
        || !energy_denom.is_finite()
        || !age_denom.is_finite()
    {
        return Err(err_sensor_invalid(
            "sensor normalization denominator",
            "density, energy, or age scale sum overflowed".to_owned(),
        ));
    }
    let slope0 = scales.energy_density / (shifted0 * shifted0);
    let q0 = density0 / shifted0;
    let mut val_acc = q0;
    if let Some(slots) = value_slots.as_deref_mut()
        && let Some(slot) = slots.first_mut()
    {
        *slot = q0 as f32;
    }
    out_gradients[0] = [slope0 * density0_x, slope0 * density0_y];

    let c0_count = CARRIER0_MOMENT_CHANNELS as usize;
    for k in 0..c0_count {
        let p = 1 + k;
        let z = raw.values[p] / reg0;
        val_acc += z;
        if let Some(slots) = value_slots.as_deref_mut()
            && let Some(slot) = slots.get_mut(p)
        {
            *slot = z as f32;
        }
        out_gradients[p] = [
            (raw.dx[p] - z * density0_x) / reg0,
            (raw.dy[p] - z * density0_y) / reg0,
        ];
    }

    let c1_first = CARRIER1_FIRST_MOMENT_PLANE as usize;
    let c1_count = CARRIER1_MOMENT_CHANNELS as usize;
    let mut z1_sq = 0.0_f64;
    for j in 0..c1_count {
        let src = c1_first + j;
        let dst = 1 + c0_count + j;
        let z = raw.values[src] / reg1;
        z1_sq += z * z;
        val_acc += z;
        if let Some(slots) = value_slots.as_deref_mut()
            && let Some(slot) = slots.get_mut(dst)
        {
            *slot = z as f32;
        }
        out_gradients[dst] = [
            (raw.dx[src] - z * density1_x) / reg1,
            (raw.dy[src] - z * density1_y) / reg1,
        ];
    }

    let slope1 = trail_energy_density_scale / (shifted1 * shifted1);
    let q1 = density1 / shifted1;
    let dq1_x = slope1 * density1_x;
    let dq1_y = slope1 * density1_y;
    let z1_norm = if z1_sq.is_finite() {
        z1_sq.sqrt().min(1.0)
    } else {
        f64::NAN
    };
    Ok(DualScalars {
        val_acc,
        energy_norm: intrinsic.energy / energy_denom,
        age_norm: intrinsic.age_seconds / age_denom,
        q1,
        dq1_x,
        dq1_y,
        z1_norm,
    })
}

struct SaturatedDual {
    x: [f32; PLANES],
    y: [f32; PLANES],
    vx: f32,
    vy: f32,
    de: f32,
    t1x: f32,
    t1y: f32,
}

struct NormalizedDual {
    values: [f64; PLANES],
    gradients: [[f64; 2]; PLANES],
    energy_norm: f64,
    age_norm: f64,
    q1: f64,
    dq1_x: f64,
    dq1_y: f64,
    z1_norm: f64,
}

#[inline]
fn normalize_dual(
    raw: &DualRawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
    trail_energy_density_scale: f64,
) -> Result<NormalizedDual, CoreError> {
    if !trail_energy_density_scale.is_finite() || trail_energy_density_scale <= 0.0 {
        return Err(err_sensor_invalid(
            "v2.carriers.trail_energy_density_scale",
            format!("{trail_energy_density_scale} is not a positive scale"),
        ));
    }
    let density0 = raw.values[0];
    let density0_x = raw.dx[0];
    let density0_y = raw.dy[0];
    let c1_e = CARRIER1_ENERGY_PLANE as usize;
    let density1 = raw.values[c1_e];
    let density1_x = raw.dx[c1_e];
    let density1_y = raw.dy[c1_e];
    if !density0.is_finite() || density0 < 0.0 {
        return Err(err_sensor_invalid(
            "sensed energy density",
            format!("{density0} is not a density"),
        ));
    }
    if !density1.is_finite() || density1 < 0.0 {
        return Err(err_sensor_invalid(
            "sensed trail energy density",
            format!("{density1} is not a density"),
        ));
    }
    if !intrinsic.energy.is_finite() || intrinsic.energy < 0.0 {
        return Err(err_sensor_invalid(
            "internal energy",
            format!("{} is not an energy", intrinsic.energy),
        ));
    }
    if !intrinsic.age_seconds.is_finite() || intrinsic.age_seconds < 0.0 {
        return Err(err_sensor_invalid(
            "age",
            format!("{} is not an age", intrinsic.age_seconds),
        ));
    }

    let mut values = [0.0; PLANES];
    let mut gradients = [[0.0; 2]; PLANES];
    let shifted0 = density0 + scales.energy_density;
    let reg0 = density0 + scales.composition_floor;
    let shifted1 = density1 + trail_energy_density_scale;
    let reg1 = density1 + scales.composition_floor;
    let energy_denom = intrinsic.energy + scales.internal_energy;
    let age_denom = intrinsic.age_seconds + scales.age_seconds;
    if !shifted0.is_finite()
        || !reg0.is_finite()
        || !shifted1.is_finite()
        || !reg1.is_finite()
        || !energy_denom.is_finite()
        || !age_denom.is_finite()
    {
        return Err(err_sensor_invalid(
            "sensor normalization denominator",
            "density, energy, or age scale sum overflowed".to_owned(),
        ));
    }
    let slope0 = scales.energy_density / (shifted0 * shifted0);
    values[0] = density0 / shifted0;
    gradients[0] = [slope0 * density0_x, slope0 * density0_y];

    let c0_count = CARRIER0_MOMENT_CHANNELS as usize;
    for k in 0..c0_count {
        let p = 1 + k;
        let z = raw.values[p] / reg0;
        values[p] = z;
        gradients[p] = [
            (raw.dx[p] - z * density0_x) / reg0,
            (raw.dy[p] - z * density0_y) / reg0,
        ];
    }

    let c1_first = CARRIER1_FIRST_MOMENT_PLANE as usize;
    let c1_count = CARRIER1_MOMENT_CHANNELS as usize;
    let mut z1_sq = 0.0_f64;
    for j in 0..c1_count {
        let src = c1_first + j;
        let dst = 1 + c0_count + j;
        let z = raw.values[src] / reg1;
        z1_sq += z * z;
        values[dst] = z;
        gradients[dst] = [
            (raw.dx[src] - z * density1_x) / reg1,
            (raw.dy[src] - z * density1_y) / reg1,
        ];
    }

    let slope1 = trail_energy_density_scale / (shifted1 * shifted1);
    let q1 = density1 / shifted1;
    let dq1_x = slope1 * density1_x;
    let dq1_y = slope1 * density1_y;
    let z1_norm = if z1_sq.is_finite() {
        z1_sq.sqrt().min(1.0)
    } else {
        f64::NAN
    };
    Ok(NormalizedDual {
        values,
        gradients,
        energy_norm: intrinsic.energy / energy_denom,
        age_norm: intrinsic.age_seconds / age_denom,
        q1,
        dq1_x,
        dq1_y,
        z1_norm,
    })
}

#[inline]
fn saturate_dual_gradients_f32_into(
    gradients: &[[f64; 2]; PLANES],
    dq1_x: f64,
    dq1_y: f64,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
    x_f32: &mut [f32],
    y_f32: &mut [f32],
) -> Result<(f32, f32, f32, f32, f32), CoreError> {
    let l = scales.gradient_length;
    let [ex, ey, vx, vy] = saturate_4([
        l * gradients[0][0],
        l * gradients[0][1],
        intrinsic.velocity[0] / scales.velocity,
        intrinsic.velocity[1] / scales.velocity,
    ])?;
    if let (Some((x0, x_rest)), Some((y0, y_rest))) =
        (x_f32.split_first_mut(), y_f32.split_first_mut())
    {
        *x0 = ex;
        *y0 = ey;
        for ((x_chunk, y_chunk), grad_chunk) in x_rest
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(y_rest.as_chunks_mut::<4>().0.iter_mut())
            .zip(gradients[1..].as_chunks::<4>().0)
        {
            let tx = saturate_4([
                l * grad_chunk[0][0],
                l * grad_chunk[1][0],
                l * grad_chunk[2][0],
                l * grad_chunk[3][0],
            ])?;
            let ty = saturate_4([
                l * grad_chunk[0][1],
                l * grad_chunk[1][1],
                l * grad_chunk[2][1],
                l * grad_chunk[3][1],
            ])?;
            *x_chunk = tx;
            *y_chunk = ty;
        }
    }
    let [de, t1x, t1y, _] = saturate_4([
        intrinsic.energy_change_per_second / scales.energy_change_per_second,
        l * dq1_x,
        l * dq1_y,
        0.0,
    ])?;
    Ok((vx, vy, de, t1x, t1y))
}

#[inline]
fn saturate_dual_gradients_f32(
    gradients: &[[f64; 2]; PLANES],
    dq1_x: f64,
    dq1_y: f64,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
) -> Result<SaturatedDual, CoreError> {
    let mut x_f32 = [0.0_f32; PLANES];
    let mut y_f32 = [0.0_f32; PLANES];
    let (vx, vy, de, t1x, t1y) = saturate_dual_gradients_f32_into(
        gradients, dq1_x, dq1_y, intrinsic, scales, &mut x_f32, &mut y_f32,
    )?;
    Ok(SaturatedDual {
        x: x_f32,
        y: y_f32,
        vx,
        vy,
        de,
        t1x,
        t1y,
    })
}

fn observe_dual_with(
    raw: &DualRawSample,
    intrinsic: &Intrinsic,
    scales: &SensorScales,
    trail_energy_density_scale: f64,
    saturate_fn: impl Fn(f64) -> Option<f64>,
) -> Result<DualObservation, CoreError> {
    let invalid =
        |quantity: &'static str, detail: String| CoreError::NumericalInvalid { quantity, detail };
    let dual = normalize_dual(raw, intrinsic, scales, trail_energy_density_scale)?;

    let saturate = |value: f64| {
        saturate_fn(value)
            .ok_or_else(|| invalid("sensor input", format!("tanh({value}) is undefined")))
    };
    let mut inputs = [0.0; SENSOR_WIDTH];
    let (observed, own) = inputs.split_at_mut(3 * PLANES);
    let (value_slots, derivative_slots) = observed.split_at_mut(PLANES);
    let (x_slots, y_slots) = derivative_slots.split_at_mut(PLANES);
    value_slots.copy_from_slice(&dual.values);
    for ((x_slot, y_slot), gradient) in x_slots
        .iter_mut()
        .zip(y_slots.iter_mut())
        .zip(&dual.gradients)
    {
        *x_slot = saturate(scales.gradient_length * gradient[0])?;
        *y_slot = saturate(scales.gradient_length * gradient[1])?;
    }
    own.copy_from_slice(&[
        dual.energy_norm,
        dual.age_norm,
        saturate(intrinsic.velocity[0] / scales.velocity)?,
        saturate(intrinsic.velocity[1] / scales.velocity)?,
        saturate(intrinsic.energy_change_per_second / scales.energy_change_per_second)?,
    ]);

    let trail_inputs = [
        dual.q1,
        saturate(scales.gradient_length * dual.dq1_x)?,
        saturate(scales.gradient_length * dual.dq1_y)?,
        dual.z1_norm,
    ];

    for &value in inputs.iter().chain(&trail_inputs) {
        if !value.is_finite() {
            return Err(invalid("sensor input", format!("{value} is not finite")));
        }
    }
    check_finite_gradients(&dual.gradients)?;
    Ok(DualObservation {
        base: Observation {
            inputs,
            gradients: dual.gradients,
        },
        trail_inputs,
    })
}

#[cfg(test)]
mod batch_sensor_tests {
    use super::*;

    fn test_scales() -> SensorScales {
        SensorScales {
            energy_density: 8.0,
            gradient_length: 0.25,
            composition_floor: 1.0e-6,
            internal_energy: 4.0,
            age_seconds: 60.0,
            velocity: 1.0,
            energy_change_per_second: 1.0,
            minimum_scale: 1.0,
            maximum_scale: 8.0,
        }
    }

    fn test_intrinsic() -> Intrinsic {
        Intrinsic {
            energy: 2.35,
            age_seconds: 12.5,
            velocity: [0.42, -1.15],
            energy_change_per_second: -0.73,
        }
    }

    #[test]
    fn observe_for_controller_and_f32_match_observe_bit_for_bit() {
        let scales = test_scales();
        let intrinsic = test_intrinsic();
        let mut raw = RawSample {
            values: [0.0; PLANES],
            dx: [0.0; PLANES],
            dy: [0.0; PLANES],
        };
        raw.values[0] = 3.25;
        raw.dx[0] = 1.75;
        raw.dy[0] = -0.85;
        for k in 1..PLANES {
            let s = if k.is_multiple_of(2) { 1.0 } else { -1.0 };
            raw.values[k] = s * (k as f64) * 0.04;
            raw.dx[k] = s * (k as f64) * 0.13;
            raw.dy[k] = -s * (k as f64) * 0.002; // mixes small & active tanh regimes
        }

        let exact = observe(&raw, &intrinsic, &scales).expect("observe");
        let ctrl = observe_for_controller(&raw, &intrinsic, &scales).expect("ctrl");
        let mut direct_f32 = [0.0_f32; SENSOR_WIDTH];
        let direct_grads = observe_for_controller_f32(&raw, &intrinsic, &scales, &mut direct_f32)
            .expect("direct f32");

        for (i, (&e, (&c, &d))) in exact
            .gradients
            .iter()
            .zip(ctrl.gradients.iter().zip(&direct_grads))
            .enumerate()
        {
            assert_eq!(e[0].to_bits(), c[0].to_bits(), "grad x {i}");
            assert_eq!(e[1].to_bits(), c[1].to_bits(), "grad y {i}");
            assert_eq!(e[0].to_bits(), d[0].to_bits(), "f32 grad x {i}");
            assert_eq!(e[1].to_bits(), d[1].to_bits(), "f32 grad y {i}");
        }
        for (i, (&e, (&c, &d))) in exact
            .inputs
            .iter()
            .zip(ctrl.inputs.iter().zip(&direct_f32))
            .enumerate()
        {
            assert_eq!((e as f32).to_bits(), (c as f32).to_bits(), "ctrl input {i}");
            assert_eq!((e as f32).to_bits(), d.to_bits(), "direct f32 input {i}");
        }
    }

    #[test]
    fn observe_dual_for_controller_and_f32_match_observe_dual_bit_for_bit() {
        let scales = test_scales();
        let intrinsic = test_intrinsic();
        let mut raw = DualRawSample {
            values: [0.0; flife_field::grid::DUAL_CARRIER_PLANES as usize],
            dx: [0.0; flife_field::grid::DUAL_CARRIER_PLANES as usize],
            dy: [0.0; flife_field::grid::DUAL_CARRIER_PLANES as usize],
        };
        raw.values[0] = 4.1;
        raw.dx[0] = -2.3;
        raw.dy[0] = 0.9;
        let c1_e = CARRIER1_ENERGY_PLANE as usize;
        raw.values[c1_e] = 1.4;
        raw.dx[c1_e] = 0.65;
        raw.dy[c1_e] = -0.35;
        for k in 1..raw.values.len() {
            if k == c1_e {
                continue;
            }
            let s = if k.is_multiple_of(2) { 1.0 } else { -1.0 };
            raw.values[k] = s * (k as f64) * 0.03;
            raw.dx[k] = -s * (k as f64) * 0.09;
            raw.dy[k] = s * (k as f64) * 0.0015;
        }

        let exact = observe_dual(&raw, &intrinsic, &scales, 8.0).expect("observe_dual");
        let ctrl = observe_dual_for_controller(&raw, &intrinsic, &scales, 8.0).expect("ctrl dual");
        let mut direct_f32 = [0.0_f32; SENSOR_WIDTH];
        let mut trail_f32 = [0.0_f32; DUAL_CARRIER_TRAIL_INPUTS];
        let direct_grads = observe_dual_for_controller_f32(
            &raw,
            &intrinsic,
            &scales,
            8.0,
            &mut direct_f32,
            &mut trail_f32,
        )
        .expect("direct dual f32");

        for (i, (&e, (&c, &d))) in exact
            .base
            .gradients
            .iter()
            .zip(ctrl.base.gradients.iter().zip(&direct_grads))
            .enumerate()
        {
            assert_eq!(e[0].to_bits(), c[0].to_bits(), "dual grad x {i}");
            assert_eq!(e[1].to_bits(), c[1].to_bits(), "dual grad y {i}");
            assert_eq!(e[0].to_bits(), d[0].to_bits(), "dual f32 grad x {i}");
            assert_eq!(e[1].to_bits(), d[1].to_bits(), "dual f32 grad y {i}");
        }
        for (i, (&e, (&c, &d))) in exact
            .base
            .inputs
            .iter()
            .zip(ctrl.base.inputs.iter().zip(&direct_f32))
            .enumerate()
        {
            assert_eq!(
                (e as f32).to_bits(),
                (c as f32).to_bits(),
                "dual ctrl input {i}"
            );
            assert_eq!(
                (e as f32).to_bits(),
                d.to_bits(),
                "dual direct f32 input {i}"
            );
        }
        for (i, (&e, (&c, &d))) in exact
            .trail_inputs
            .iter()
            .zip(ctrl.trail_inputs.iter().zip(&trail_f32))
            .enumerate()
        {
            assert_eq!(
                (e as f32).to_bits(),
                (c as f32).to_bits(),
                "dual ctrl trail {i}"
            );
            assert_eq!(
                (e as f32).to_bits(),
                d.to_bits(),
                "dual direct f32 trail {i}"
            );
        }
        let grad_only =
            observe_dual_gradients_only(&raw, &intrinsic, &scales, 8.0).expect("dual grad only");
        for (i, (&e, &g)) in exact.base.gradients.iter().zip(&grad_only).enumerate() {
            assert_eq!(e[0].to_bits(), g[0].to_bits(), "dual grad-only x {i}");
            assert_eq!(e[1].to_bits(), g[1].to_bits(), "dual grad-only y {i}");
        }

        // Error parity on nonfinite velocity and nonfinite gradient.
        let mut bad_intr = intrinsic;
        bad_intr.velocity[0] = f64::INFINITY;
        assert!(observe_dual_gradients_only(&raw, &bad_intr, &scales, 8.0).is_err());
        let mut bad_raw = raw;
        bad_raw.dx[0] = f64::NAN;
        assert!(observe_dual_gradients_only(&bad_raw, &intrinsic, &scales, 8.0).is_err());
    }

    #[test]
    fn observe_gradients_only_matches_observe_and_errors_bit_for_bit() {
        let scales = test_scales();
        let intrinsic = test_intrinsic();
        let mut raw = RawSample {
            values: [0.0; PLANES],
            dx: [0.0; PLANES],
            dy: [0.0; PLANES],
        };
        raw.values[0] = 2.75;
        raw.dx[0] = -1.25;
        raw.dy[0] = 0.65;
        for k in 1..PLANES {
            raw.values[k] = (k as f64) * 0.03;
            raw.dx[k] = -(k as f64) * 0.07;
            raw.dy[k] = (k as f64) * 0.05;
        }
        let exact = observe(&raw, &intrinsic, &scales).expect("observe");
        let grads = observe_gradients_only(&raw, &intrinsic, &scales).expect("grad only");
        for (i, (&e, &g)) in exact.gradients.iter().zip(&grads).enumerate() {
            assert_eq!(e[0].to_bits(), g[0].to_bits(), "grad-only x {i}");
            assert_eq!(e[1].to_bits(), g[1].to_bits(), "grad-only y {i}");
        }
        let mut bad_intr = intrinsic;
        bad_intr.energy_change_per_second = f64::NAN;
        assert!(observe_gradients_only(&raw, &bad_intr, &scales).is_err());
        let mut bad_raw = raw;
        bad_raw.values[0] = -0.1;
        assert!(observe_gradients_only(&bad_raw, &intrinsic, &scales).is_err());
    }
}
