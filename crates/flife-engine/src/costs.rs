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
use flife_core::numerics::pow_positive;
use flife_core::{CoreError, Flow, TimeStep, ValidatedConfig};

/// The cost coefficients of `[costs]` that maintenance uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaintenanceCosts {
    basal: f64,
    sensor: f64,
    sensor_unit_scale: f64,
    sensor_scale_exponent: f64,
    neural: f64,
}

impl MaintenanceCosts {
    /// Coefficients `c_basal`, `c_sensor` (both ecological-energy per
    /// simulation-second), `s_unit` (world-length), the sensing-scale
    /// exponent `α ≥ 0`, and `c_neural` (ecological-energy per parameter
    /// evaluation).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a negative or nonfinite cost or
    /// exponent, or a nonpositive unit scale.
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
                "costs.sensor_scale_cost_exponent",
                exponent,
                exponent >= 0.0,
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
        Ok(Self {
            basal,
            sensor,
            sensor_unit_scale,
            sensor_scale_exponent: exponent,
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

    /// `(c_basal, c_sensor, s_unit, α, c_neural)`, for a device's phase 6
    /// (FL-032).
    pub(crate) const fn coefficients(&self) -> (f64, f64, f64, f64, f64) {
        (
            self.basal,
            self.sensor,
            self.sensor_unit_scale,
            self.sensor_scale_exponent,
            self.neural,
        )
    }

    /// The mandatory cost of one tick for an organism with sensing scale
    /// `sensing_scale` (world-length) and a controller of `parameters`
    /// weights and biases, evaluated this tick or not: the
    /// [`MaintenanceBill::total`] of [`MaintenanceCosts::bill`].
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonpositive or nonfinite
    /// sensing scale, or a sensing-scale power that overflows.
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
        self.bill_with_basal_multiplier(step, sensing_scale, parameters, evaluated, 1.0)
    }

    /// The mandatory cost of one tick, by ledger flow, scaling the basal term
    /// by `basal_multiplier` (`1.0` on `field-ecology-v1` and whenever
    /// `v2.senescence` is disabled; `m_age >= 1.0` under `V2-03`).
    ///
    /// Computes `(Δt · c_basal) · basal_multiplier` so that `basal_multiplier ==
    /// 1.0` is bit-identical to baseline and matches `motion.metal`'s software
    /// binary64 multiplication order.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonpositive or nonfinite sensing
    /// scale, a negative or nonfinite `basal_multiplier`, or an overflowing
    /// basal or sensing-scale term.
    pub fn bill_with_basal_multiplier(
        &self,
        step: TimeStep,
        sensing_scale: f64,
        parameters: u32,
        evaluated: bool,
        basal_multiplier: f64,
    ) -> Result<MaintenanceBill, CoreError> {
        if !sensing_scale.is_finite() || sensing_scale <= 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "sensing scale",
                detail: format!("{sensing_scale} is not a positive length"),
            });
        }
        if !basal_multiplier.is_finite() || basal_multiplier < 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "basal maintenance multiplier",
                detail: format!("{basal_multiplier} is not finite and nonnegative"),
            });
        }
        let factor = if self.sensor_scale_exponent.to_bits() == 1.0_f64.to_bits() {
            let ratio = sensing_scale / self.sensor_unit_scale;
            if !ratio.is_finite() {
                return Err(CoreError::NumericalInvalid {
                    quantity: "sensor cost",
                    detail: format!(
                        "({sensing_scale} / {}) is not finite",
                        self.sensor_unit_scale
                    ),
                });
            }
            ratio
        } else if self.sensor_scale_exponent == 0.0 {
            1.0
        } else {
            let ratio = sensing_scale / self.sensor_unit_scale;
            if !ratio.is_finite() {
                return Err(CoreError::NumericalInvalid {
                    quantity: "sensor cost",
                    detail: format!(
                        "({sensing_scale} / {})^{} is not finite",
                        self.sensor_unit_scale, self.sensor_scale_exponent
                    ),
                });
            }
            if ratio == 0.0 {
                0.0
            } else {
                pow_positive(ratio, self.sensor_scale_exponent).ok_or_else(|| {
                    CoreError::NumericalInvalid {
                        quantity: "sensor cost",
                        detail: format!(
                            "({sensing_scale} / {})^{} overflowed",
                            self.sensor_unit_scale, self.sensor_scale_exponent
                        ),
                    }
                })?
            }
        };
        let seconds = step.seconds_per_tick();
        let base_basal = seconds * self.basal;
        let basal = if basal_multiplier.to_bits() == 1.0_f64.to_bits() {
            base_basal
        } else {
            base_basal * basal_multiplier
        };
        if !basal.is_finite() || basal < 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "basal maintenance",
                detail: format!("{basal} is not finite and nonnegative"),
            });
        }
        let sensor = seconds * (self.sensor * factor);
        if !sensor.is_finite() || sensor < 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "sensor cost",
                detail: format!("{sensor} is not finite and nonnegative"),
            });
        }
        let controller = if evaluated {
            self.neural * f64::from(parameters)
        } else {
            0.0
        };
        if !controller.is_finite() || controller < 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "controller maintenance",
                detail: format!("{controller} is not finite and nonnegative"),
            });
        }
        let bill = MaintenanceBill {
            basal,
            sensor,
            controller,
        };
        if !bill.total().is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "maintenance",
                detail: format!("total {} is not finite", bill.total()),
            });
        }
        Ok(bill)
    }
}

/// Quadratic Gompertz metabolic senescence multiplier (`V2-03`):
/// `m_age = 1 + min((age_seconds / scale_seconds)^2, maximum_multiplier - 1)`.
///
/// Overflow-guarded so arbitrarily large ages clamp cleanly to
/// `maximum_multiplier` without producing `Inf` or `NaN`.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] if `age_seconds` is nonfinite or negative,
/// `scale_seconds` is nonfinite or `<= 0`, or `maximum_multiplier` is
/// nonfinite or `< 1`.
pub fn senescence_multiplier(
    age_seconds: f64,
    scale_seconds: f64,
    maximum_multiplier: f64,
) -> Result<f64, CoreError> {
    if !age_seconds.is_finite() || age_seconds < 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "organism age",
            detail: format!("{age_seconds} is not finite and nonnegative"),
        });
    }
    if !scale_seconds.is_finite() || scale_seconds <= 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "v2.senescence.scale_seconds",
            detail: format!("{scale_seconds} is not finite and positive"),
        });
    }
    if !maximum_multiplier.is_finite() || maximum_multiplier < 1.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "v2.senescence.maximum_multiplier",
            detail: format!("{maximum_multiplier} must be finite and >= 1.0"),
        });
    }
    let cap = maximum_multiplier - 1.0;
    let ratio = age_seconds / scale_seconds;
    let quad = ratio * ratio;
    let excess = if quad.is_finite() { quad.min(cap) } else { cap };
    Ok(1.0 + excess)
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
