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

use flife_core::{CoreError, ValidatedConfig};
use flife_field::numerics::tanh;
use flife_field::pyramid::{PLANES, RawSample};

/// The sensor schema this module implements.
pub const ENERGY_MOMENT_GRADIENT_104_V1: &str = "energy-moment-gradient-104-v1";

/// The sensor normalization this module implements.
pub const FIXED_LOCAL_SCALES_V1: &str = "fixed-local-scales-v1";

/// Controller inputs.
pub const SENSOR_WIDTH: usize = 3 * PLANES + INTRINSIC_WIDTH;

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
    let invalid =
        |quantity: &'static str, detail: String| CoreError::NumericalInvalid { quantity, detail };
    let [density, moments @ ..] = raw.values;
    let [density_x, moments_x @ ..] = raw.dx;
    let [density_y, moments_y @ ..] = raw.dy;
    if !density.is_finite() || density < 0.0 {
        return Err(invalid(
            "sensed energy density",
            format!("{density} is not a density"),
        ));
    }
    if !intrinsic.energy.is_finite() || intrinsic.energy < 0.0 {
        return Err(invalid(
            "internal energy",
            format!("{} is not an energy", intrinsic.energy),
        ));
    }
    if !intrinsic.age_seconds.is_finite() || intrinsic.age_seconds < 0.0 {
        return Err(invalid(
            "age",
            format!("{} is not an age", intrinsic.age_seconds),
        ));
    }

    let mut values = [0.0; PLANES];
    let mut gradients = [[0.0; 2]; PLANES];
    // q = R/(R + R_scale) and ∂q = R_scale ∂R/(R + R_scale)².
    let shifted = density + scales.energy_density;
    let slope = scales.energy_density / (shifted * shifted);
    if let (Some(value), Some(gradient)) = (values.first_mut(), gradients.first_mut()) {
        *value = density / shifted;
        *gradient = [slope * density_x, slope * density_y];
    }
    // Z = M/(R + R_floor) and ∂Z = (∂M − Z ∂R)/(R + R_floor).
    let regularized = density + scales.composition_floor;
    for ((value, gradient), ((moment, moment_x), moment_y)) in values
        .iter_mut()
        .zip(gradients.iter_mut())
        .skip(1)
        .zip(moments.iter().zip(&moments_x).zip(&moments_y))
    {
        let z = moment / regularized;
        *value = z;
        *gradient = [
            (moment_x - z * density_x) / regularized,
            (moment_y - z * density_y) / regularized,
        ];
    }

    let saturate = |value: f64| {
        tanh(value).ok_or_else(|| invalid("sensor input", format!("tanh({value}) is undefined")))
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
        intrinsic.energy / (intrinsic.energy + scales.internal_energy),
        intrinsic.age_seconds / (intrinsic.age_seconds + scales.age_seconds),
        saturate(intrinsic.velocity[0] / scales.velocity)?,
        saturate(intrinsic.velocity[1] / scales.velocity)?,
        saturate(intrinsic.energy_change_per_second / scales.energy_change_per_second)?,
    ]);
    if let Some(bad) = inputs
        .iter()
        .chain(gradients.iter().flatten())
        .find(|value| !value.is_finite())
    {
        return Err(invalid("sensor input", format!("{bad} is not finite")));
    }
    Ok(Observation { inputs, gradients })
}
