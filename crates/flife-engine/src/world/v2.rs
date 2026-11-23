//! Modular `field-ecology-v2` rules and phase-hook containers (V2-01).
//!
//! Keeps v2 feature resolution, pure-scientific sensor/action layouts, and
//! multi-carrier Phase 9 specs isolated from `world.rs` so Wave 1–3 feature
//! beads drop into named hook seams without destabilizing baseline v1.

use flife_core::model::is_v2_model;
use flife_core::{CoreError, V2Abi, V2Config, V2Feature, ValidatedConfig};
use flife_neural::{ActionLayout, SensorLayout};

/// Which carrier field a Phase 9 deposit stream reduces into (V2-01 / V2-15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CarrierSpec {
    /// Primary metabolic field (`Carrier 0`, 33 planes).
    Carrier0,
    /// Secondary slow trail carrier (`Carrier 1`, added by `V2-15`).
    Carrier1,
}

/// Run-constant `field-ecology-v2` rules resolved from a [`ValidatedConfig`].
///
/// For `field-ecology-v1*` worlds, `config` is `None`, `feature_bitmask` is
/// `0`, `epigenetic_gate_bias` is `0.0`, and `sensors`/`actions` equal the
/// baseline v1 (or FL-054 recurrent) layouts.
#[derive(Debug, Clone)]
pub struct V2Rules {
    abi: V2Abi,
    sensors: SensorLayout,
    actions: ActionLayout,
    feature_bitmask: u64,
    inv08_pure_field: bool,
    epigenetic_gate_bias: f64,
    config: Option<V2Config>,
}

impl V2Rules {
    /// Resolve `V2Rules` from a validated configuration.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] or [`CoreError::Overflow`] if the
    /// resolved ABI widths exceed the neural limits.
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let abi = config.v2_abi();
        let state_width =
            usize::try_from(abi.recurrent_state_width).map_err(|_| CoreError::Overflow {
                quantity: "controller.recurrent_state_width",
            })?;
        let sensors = SensorLayout::new(
            abi.dual_carrier_sensors,
            abi.hippo_crowding_sensor,
            abi.notch_delta_sensor,
            state_width,
        )?;
        let actions = ActionLayout::new(
            abi.so2_motor_actions,
            abi.so2_budding_actions,
            abi.emt_gate_action,
            abi.wake_pump_action,
            state_width,
        )?;
        let v2_config = if is_v2_model(&config.config().model) {
            config.config().v2.clone()
        } else {
            None
        };
        let epigenetic_gate_bias = v2_config
            .as_ref()
            .filter(|v2| v2.is_enabled(V2Feature::EpigeneticRecurrence))
            .map_or(0.0, |v2| v2.differentiation.epigenetic_gate_bias);
        Ok(Self {
            abi,
            sensors,
            actions,
            feature_bitmask: config.v2_feature_bitmask(),
            inv08_pure_field: config.inv08_pure_field(),
            epigenetic_gate_bias,
            config: v2_config,
        })
    }

    /// Whether this world runs a `field-ecology-v2*` model.
    #[must_use]
    pub const fn is_v2(&self) -> bool {
        self.config.is_some()
    }

    /// Whether `feature` is enabled on this world (always `false` on v1).
    #[must_use]
    pub fn is_enabled(&self, feature: V2Feature) -> bool {
        self.config
            .as_ref()
            .is_some_and(|v2| v2.is_enabled(feature))
    }

    /// Canonical bitmask of enabled scientific `V2Feature`s (`0` on v1 or `--v1-compat`).
    #[must_use]
    pub const fn feature_bitmask(&self) -> u64 {
        self.feature_bitmask
    }

    /// Whether the world satisfies `INV-08` (purely field-mediated interaction).
    #[must_use]
    pub const fn inv08_pure_field(&self) -> bool {
        self.inv08_pure_field
    }

    /// Resolved pure-scientific ABI summary.
    #[must_use]
    pub const fn abi(&self) -> V2Abi {
        self.abi
    }

    /// Resolved controller sensor layout.
    #[must_use]
    pub const fn sensors(&self) -> SensorLayout {
        self.sensors
    }

    /// Resolved controller action layout.
    #[must_use]
    pub const fn actions(&self) -> ActionLayout {
        self.actions
    }

    /// Epigenetic recurrent gate bias (`0.0` unless `epigenetic-recurrence` is active).
    #[must_use]
    pub const fn epigenetic_gate_bias(&self) -> f64 {
        self.epigenetic_gate_bias
    }

    /// Materialized `[v2]` configuration, if this is a `field-ecology-v2*` world.
    #[must_use]
    pub const fn config(&self) -> Option<&V2Config> {
        self.config.as_ref()
    }
}
