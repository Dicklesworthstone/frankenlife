//! Mandatory maintenance (plan §5.6, A.1).
//!
//! The mandatory cost in one tick is
//! `Δt [c_basal + c_sensor (s / s_unit)^α] + c_neural P × evaluated`, where
//! `P` counts weights and biases and `evaluated` is whether the controller
//! ran this tick (including a newborn's forced evaluation). Costs come from
//! canonical model work, never from wall-clock time, cache behavior, kernel
//! padding or the backend, so every backend charges the same amount.
//!
//! A [`MaintenanceBill`] keeps the three terms apart, one per ledger flow:
//! basal `Δt c_basal` (`maintenance`), sensor `Δt c_sensor (s / s_unit)`
//! (`sensor-cost`) and controller `c_neural P` (`controller-cost`). The
//! total is `(basal + sensor) + controller` in that order. For a
//! power-of-two `Δt`, as in Appendix A, scaling by `Δt` is exact, so this is
//! bit for bit `Δt [c_basal + c_sensor (s / s_unit)] + c_neural P`.
//!
//! Maintenance is one payment of the total, paid up to what the organism
//! holds (§8.2). A partial payment covers the terms in the declared order
//! basal, then sensor, then controller. The attribution rounds down, so the
//! recorded flows never exceed the payment. Unpaid maintenance is never
//! recorded as dissipation.

use flife_core::ledger::{EnergyBudget, Ledger, Payment, sub_down};
use flife_core::{CoreError, Flow, TimeStep, ValidatedConfig};

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

    /// `(c_basal, c_sensor, s_unit, c_neural)`, for a device's phase 6
    /// (FL-032).
    pub(crate) const fn coefficients(&self) -> (f64, f64, f64, f64) {
        (self.basal, self.sensor, self.sensor_unit_scale, self.neural)
    }

    /// The mandatory cost of one tick for an organism with sensing scale
    /// `sensing_scale` (world-length) and a controller of `parameters`
    /// weights and biases, evaluated this tick or not: the
    /// [`MaintenanceBill::total`] of [`MaintenanceCosts::bill`].
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
        Ok(self
            .bill(step, sensing_scale, parameters, evaluated)?
            .total())
    }

    /// The mandatory cost of one tick, by ledger flow.
    ///
    /// # Errors
    /// As [`MaintenanceCosts::mandatory`].
    pub fn bill(
        &self,
        step: TimeStep,
        sensing_scale: f64,
        parameters: u32,
        evaluated: bool,
    ) -> Result<MaintenanceBill, CoreError> {
        if !sensing_scale.is_finite() || sensing_scale <= 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "sensing scale",
                detail: format!("{sensing_scale} is not a positive length"),
            });
        }
        let seconds = step.seconds_per_tick();
        Ok(MaintenanceBill {
            basal: seconds * self.basal,
            sensor: seconds * (self.sensor * (sensing_scale / self.sensor_unit_scale)),
            controller: if evaluated {
                self.neural * f64::from(parameters)
            } else {
                0.0
            },
        })
    }
}

/// One tick's mandatory cost, by ledger flow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaintenanceBill {
    /// `Δt c_basal`: the `maintenance` flow.
    pub basal: f64,
    /// `Δt c_sensor (s / s_unit)`: the `sensor-cost` flow.
    pub sensor: f64,
    /// `c_neural P` when the controller ran, else zero: the `controller-cost`
    /// flow.
    pub controller: f64,
}

impl MaintenanceBill {
    /// `(basal + sensor) + controller`.
    #[must_use]
    pub fn total(&self) -> f64 {
        (self.basal + self.sensor) + self.controller
    }

    /// Pay the total from `budget`, up to what it holds, and attribute the
    /// payment to basal, then sensor, then controller.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a negative or nonfinite term.
    pub fn settle(&self, budget: &mut EnergyBudget) -> Result<MaintenanceSettlement, CoreError> {
        for (quantity, value) in [
            ("basal maintenance", self.basal),
            ("sensor cost", self.sensor),
            ("controller cost", self.controller),
        ] {
            if !(value.is_finite() && value >= 0.0) {
                return Err(CoreError::NumericalInvalid {
                    quantity,
                    detail: format!("{value} is not a finite nonnegative energy"),
                });
            }
        }
        let cost = budget.pay(self.total())?;
        let basal = cost.paid.min(self.basal);
        let rest = sub_down(cost.paid, basal);
        let sensor = rest.min(self.sensor);
        let controller = sub_down(rest, sensor);
        Ok(MaintenanceSettlement {
            cost,
            paid: Self {
                basal,
                sensor,
                controller,
            },
        })
    }
}

/// One organism's maintenance payment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaintenanceSettlement {
    /// The total, requested and paid.
    pub cost: Payment,
    /// What was paid toward each term.
    pub paid: MaintenanceBill,
}

impl MaintenanceSettlement {
    /// Record the paid terms as `maintenance`, `sensor-cost` and
    /// `controller-cost`.
    ///
    /// # Errors
    /// As [`Ledger::record`].
    pub fn record(&self, ledger: &mut impl Ledger) -> Result<(), CoreError> {
        for (flow, amount) in [
            (Flow::Maintenance, self.paid.basal),
            (Flow::SensorCost, self.paid.sensor),
            (Flow::ControllerCost, self.paid.controller),
        ] {
            if amount > 0.0 {
                ledger.record(flow, amount)?;
            }
        }
        Ok(())
    }
}
