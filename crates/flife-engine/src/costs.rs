//! Mandatory maintenance (plan §5.6, A.1).
//!
//! The mandatory cost in one tick is
//! `Δt [c_basal + c_sensor (s / s_unit)^α] + c_neural P × evaluated`, where
//! `P` counts weights and biases and `evaluated` is whether the controller
//! ran this tick (including a newborn's forced evaluation). Costs come from
//! canonical model work, never from wall-clock time, cache behavior, kernel
//! padding or the backend, so every backend charges the same amount.

use flife_core::{CoreError, TimeStep, ValidatedConfig};

/// The cost coefficients of `[costs]` that maintenance uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaintenanceCosts {
    basal: f64,
    sensor: f64,
    sensor_unit_scale: f64,
    neural: f64,
}

impl MaintenanceCosts {
    /// Coefficients `c_basal`, `c_sensor` (both ecological-energy per
    /// simulation-second), `s_unit` (world-length), the sensing-scale
    /// exponent `α`, and `c_neural` (ecological-energy per parameter
    /// evaluation).
    ///
    /// # Errors
    /// - [`CoreError::NumericalInvalid`] for a negative or nonfinite cost or
    ///   a nonpositive unit scale.
    /// - [`CoreError::MissingCapability`] for `α ≠ 1`: another exponent needs a
    ///   host-independent power function, which the reference profile does
    ///   not have yet.
    pub fn new(
        basal: f64,
        sensor: f64,
        sensor_unit_scale: f64,
        exponent: f64,
        neural: f64,
    ) -> Result<Self, CoreError> {
        for (quantity, value, admissible) in [
            ("costs.basal_energy_per_second", basal, basal >= 0.0),
            (
                "costs.sensor_energy_per_second_at_unit_scale",
                sensor,
                sensor >= 0.0,
            ),
            (
                "costs.sensor_unit_scale_world_units",
                sensor_unit_scale,
                sensor_unit_scale > 0.0,
            ),
            (
                "costs.neural_energy_per_parameter_evaluation",
                neural,
                neural >= 0.0,
            ),
        ] {
            if !value.is_finite() || !admissible {
                return Err(CoreError::NumericalInvalid {
                    quantity,
                    detail: format!("{value} is out of range"),
                });
            }
        }
        if exponent.to_bits() != 1.0_f64.to_bits() {
            return Err(CoreError::MissingCapability {
                capability: format!(
                    "a host-independent power function for costs.sensor_scale_cost_exponent = {exponent}"
                ),
            });
        }
        Ok(Self {
            basal,
            sensor,
            sensor_unit_scale,
            neural,
        })
    }

    /// The coefficients of a validated configuration.
    ///
    /// # Errors
    /// As [`MaintenanceCosts::new`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let costs = &config.config().costs;
        Self::new(
            costs.basal_energy_per_second,
            costs.sensor_energy_per_second_at_unit_scale,
            costs.sensor_unit_scale_world_units,
            costs.sensor_scale_cost_exponent,
            costs.neural_energy_per_parameter_evaluation,
        )
    }

    /// The mandatory cost of one tick for an organism with sensing scale
    /// `sensing_scale` (world-length) and a controller of `parameters`
    /// weights and biases, evaluated this tick or not.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonpositive or nonfinite
    /// sensing scale.
    pub fn mandatory(
        &self,
        step: TimeStep,
        sensing_scale: f64,
        parameters: u32,
        evaluated: bool,
    ) -> Result<f64, CoreError> {
        if !sensing_scale.is_finite() || sensing_scale <= 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "sensing scale",
                detail: format!("{sensing_scale} is not a positive length"),
            });
        }
        let per_second = self.basal + self.sensor * (sensing_scale / self.sensor_unit_scale);
        let neural = if evaluated {
            self.neural * f64::from(parameters)
        } else {
            0.0
        };
        Ok(step.seconds_per_tick() * per_second + neural)
    }
}
