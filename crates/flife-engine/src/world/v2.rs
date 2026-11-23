//! Modular `field-ecology-v2` rules and phase-hook containers (V2-01, Wave 1).
//!
//! Keeps v2 feature resolution, pure-scientific sensor/action layouts,
//! senescence (`V2-03`), emission reform (`V2-04`), crowding (`V2-07`), and
//! multi-carrier Phase 9 specs isolated from `world.rs` so Wave 1–3 feature
//! beads drop into named hook seams without destabilizing baseline v1.

use flife_core::clock::{Tick, age_seconds};
use flife_core::model::is_v2_model;
use flife_core::{CoreError, TimeStep, V2Abi, V2Config, V2Feature, ValidatedConfig};
use flife_field::grid::CARRIER0_MOMENT_CHANNELS;
use flife_field::settle::MotorSteeringConstants;
use flife_field::state::CHANNELS;
use flife_field::{
    DualCarrierRules, FieldGrid, OasisSchedule, TightJunctions, TransportPlan, VascularField,
    field_planes_for_config,
};
use flife_neural::{ActionLayout, SensorLayout};

use crate::costs::senescence_multiplier;
use crate::emission::EmissionV2Rules;

/// Which carrier field a Phase 9 deposit stream reduces into (V2-01 / V2-15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CarrierSpec {
    /// Primary metabolic field (`Carrier 0`, 33 planes).
    Carrier0,
    /// Secondary slow trail carrier (`Carrier 1`, added by `V2-15`).
    Carrier1,
}

/// Resolved Gompertz metabolic senescence parameters (`V2-03`, `senescence`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SenescenceRules {
    /// Characteristic senescence age `τ_sen > 0` in simulation seconds.
    pub scale_seconds: f64,
    /// Maximum basal multiplier `M_max >= 1.0`.
    pub maximum_multiplier: f64,
}

/// Resolved local crowding measurement mode (`V2-07`, `hippo-crowding`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CrowdingMode {
    /// Exact torus neighbor count within `radius_world_units` (`inv08_pure_field = false`).
    LocalNeighbors {
        /// Neighbor counting radius in world units.
        radius_world_units: f64,
    },
    /// Pure-field grid cell co-occupant count `n_c - 1` (`inv08_pure_field = true`).
    CellOccupancy,
}

/// Resolved Hippo-YAP crowding birth gate parameters (`V2-07`, `hippo-crowding`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrowdingRules {
    /// Neighbor counting mode.
    pub mode: CrowdingMode,
    /// Mitotic arrest threshold `c_max >= 2.0`.
    pub arrest_threshold: f64,
    /// Whether `c_den = N_loc / (N_loc + 4)` is appended to controller sensors.
    pub expose_sensor: bool,
}

/// Resolved `V2-06` (`so2-budding`) placement parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BuddingRules {
    /// Local uniform disk radius `r_local` (`<= reproduction.child_position_offset_radius`).
    pub local_offset_radius: f64,
}

/// Chemical direction carried by a lysed corpse (`V2-11`, `lysis-recycling`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CorpseDirection {
    /// `-a_eff`: negated effective affinity (necrophoresis, immune to Trap D).
    NegatedAffinity,
    /// `J a_eff`: 90°-rotated effective affinity (strictly orthogonal to `a_eff`).
    SymplecticAffinity,
    /// `+a_eff`: organism's own effective affinity direction.
    GeneticAffinity,
}

/// Resolved viability-floor corpse lysis parameters (`V2-11`, `lysis-recycling`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LysisRules {
    /// Viability energy floor `E_viable > 0`.
    pub viability_energy_floor: f64,
    /// Chemical direction deposited when a lysed corpse recycles.
    pub corpse_direction: CorpseDirection,
}

/// Resolved obligate symplectic metabolic waste parameters (`V2-11`,
/// `symplectic-waste`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WasteRules {
    /// Fraction `ρ_waste ∈ [0.0, 0.65]` of unconverted uptake `U − G` returned
    /// along `J a_eff`.
    pub byproduct_recycle_fraction: f64,
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
    field_planes: u32,
    oases: Option<OasisSchedule>,
    tight_junctions: Option<TightJunctions>,
    vascular: Option<VascularField>,
    dual_carrier: Option<DualCarrierRules>,
    trail_plan: Option<TransportPlan>,
    senescence: Option<SenescenceRules>,
    emission: EmissionV2Rules,
    motor: Option<MotorSteeringConstants>,
    budding: Option<BuddingRules>,
    crowding: Option<CrowdingRules>,
    lysis: Option<LysisRules>,
    waste: Option<WasteRules>,
    config: Option<V2Config>,
}

impl V2Rules {
    /// Resolve `V2Rules` from a validated configuration.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`], [`CoreError::UnknownAlgorithm`], or
    /// [`CoreError::Overflow`] if the resolved ABI widths or v2 sub-rules are
    /// invalid.
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
        let field_planes = field_planes_for_config(config);
        let oases = OasisSchedule::from_config(config)?;
        let tight_junctions = TightJunctions::from_config(config)?;
        let vascular = VascularField::from_config(config)?;
        let dual_carrier = DualCarrierRules::from_config(config)?;
        let trail_plan = match dual_carrier {
            Some(ref dc) => {
                let grid = FieldGrid::from_config(config)?;
                Some(dc.trail_plan(&grid, config.report().timestep)?)
            }
            None => None,
        };
        let senescence = v2_config
            .as_ref()
            .filter(|v2| v2.is_enabled(V2Feature::Senescence))
            .map(|v2| SenescenceRules {
                scale_seconds: v2.senescence.scale_seconds,
                maximum_multiplier: v2.senescence.maximum_multiplier,
            });
        let emission = EmissionV2Rules::from_config(config)?;
        let motor = v2_config
            .as_ref()
            .filter(|v2| v2.is_enabled(V2Feature::So2Motor))
            .map(|v2| MotorSteeringConstants {
                energy_gradient_gain: v2.motor.energy_gradient_gain,
                chemical_gradient_gain: v2.motor.chemical_gradient_gain,
                chiral_contour_gain: v2.motor.chiral_contour_gain,
                heading_propulsion_fraction: v2.motor.heading_propulsion_fraction,
                internal_energy_scale: config.config().sensors.internal_energy_scale,
            });
        let budding = v2_config
            .as_ref()
            .filter(|v2| v2.is_enabled(V2Feature::So2Budding))
            .map(|v2| {
                let r_max = config.config().reproduction.child_position_offset_radius;
                let local_offset_radius = v2
                    .budding
                    .local_offset_radius
                    .unwrap_or_else(|| 0.25_f64.min(0.5 * r_max));
                BuddingRules {
                    local_offset_radius,
                }
            });
        let crowding = if let Some(v2) = v2_config
            .as_ref()
            .filter(|v2| v2.is_enabled(V2Feature::HippoCrowding))
        {
            let mode = match v2.crowding.mode.as_str() {
                "local-neighbors-v1" => {
                    let radius = v2.crowding.radius_world_units.unwrap_or(0.375);
                    if !radius.is_finite() || radius <= 0.0 {
                        return Err(CoreError::NumericalInvalid {
                            quantity: "v2.crowding.radius_world_units",
                            detail: format!("{radius} must be finite and positive"),
                        });
                    }
                    CrowdingMode::LocalNeighbors {
                        radius_world_units: radius,
                    }
                }
                "cell-occupancy-v1" => CrowdingMode::CellOccupancy,
                other => {
                    return Err(CoreError::UnknownAlgorithm {
                        role: "v2.crowding.mode",
                        name: other.to_owned(),
                    });
                }
            };
            if !v2.crowding.arrest_threshold.is_finite() || v2.crowding.arrest_threshold < 2.0 {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.crowding.arrest_threshold",
                    detail: format!("{} must be finite and >= 2.0", v2.crowding.arrest_threshold),
                });
            }
            Some(CrowdingRules {
                mode,
                arrest_threshold: v2.crowding.arrest_threshold,
                expose_sensor: v2.crowding.expose_sensor,
            })
        } else {
            None
        };
        let lysis = if let Some(v2) = v2_config
            .as_ref()
            .filter(|v2| v2.is_enabled(V2Feature::LysisRecycling))
        {
            let limit = config
                .config()
                .reproduction
                .minimum_child_energy
                .min(config.config().reproduction.minimum_parent_energy_after_split);
            let floor = v2
                .metabolism
                .lysis
                .viability_energy_floor
                .unwrap_or_else(|| 0.10_f64.min(0.5 * limit));
            if !floor.is_finite() || floor <= 0.0 || floor >= limit {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.metabolism.lysis.viability_energy_floor",
                    detail: format!("{floor} must lie in (0, {limit})"),
                });
            }
            let corpse_direction = match v2.metabolism.lysis.corpse_direction.as_str() {
                "negated-affinity" => CorpseDirection::NegatedAffinity,
                "symplectic-affinity" => CorpseDirection::SymplecticAffinity,
                "genetic-affinity" => CorpseDirection::GeneticAffinity,
                other => {
                    return Err(CoreError::UnknownAlgorithm {
                        role: "v2.metabolism.lysis.corpse_direction",
                        name: other.to_owned(),
                    });
                }
            };
            Some(LysisRules {
                viability_energy_floor: floor + 0.0,
                corpse_direction,
            })
        } else {
            None
        };
        let waste = if let Some(v2) = v2_config
            .as_ref()
            .filter(|v2| v2.is_enabled(V2Feature::SymplecticWaste))
        {
            let frac = v2.metabolism.waste.byproduct_recycle_fraction;
            if !frac.is_finite() || !(0.0..=0.65).contains(&frac) {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.metabolism.waste.byproduct_recycle_fraction",
                    detail: format!("{frac} must lie in [0.0, 0.65]"),
                });
            }
            Some(WasteRules {
                byproduct_recycle_fraction: frac + 0.0,
            })
        } else {
            None
        };
        Ok(Self {
            abi,
            sensors,
            actions,
            feature_bitmask: config.v2_feature_bitmask(),
            inv08_pure_field: config.inv08_pure_field(),
            epigenetic_gate_bias,
            field_planes,
            oases,
            tight_junctions,
            vascular,
            dual_carrier,
            trail_plan,
            senescence,
            emission,
            motor,
            budding,
            crowding,
            lysis,
            waste,
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

    /// Number of field planes (`33` single-carrier, `34` under `V2-15`).
    #[must_use]
    pub const fn field_planes(&self) -> u32 {
        self.field_planes
    }

    /// Resolved phase-staggered oasis schedule (`V2-02`), if enabled.
    #[must_use]
    pub const fn oases(&self) -> Option<OasisSchedule> {
        self.oases
    }

    /// Resolved passive symmetric tight-junction rules (`V2-09`), if enabled.
    #[must_use]
    pub const fn tight_junctions(&self) -> Option<TightJunctions> {
        self.tight_junctions
    }

    /// Resolved Murray / *Physarum* vascular field rules (`V2-14`), if enabled.
    #[must_use]
    pub const fn vascular(&self) -> Option<VascularField> {
        self.vascular
    }

    /// Resolved 34-plane dual-carrier trail rules (`V2-15`), if enabled.
    #[must_use]
    pub const fn dual_carrier(&self) -> Option<DualCarrierRules> {
        self.dual_carrier
    }

    /// Carrier-1 slow trail transport plan (`V2-15`), if enabled.
    #[must_use]
    pub const fn trail_plan(&self) -> Option<&TransportPlan> {
        self.trail_plan.as_ref()
    }

    /// Resolved Gompertz senescence rules (`V2-03`), if enabled.
    #[must_use]
    pub const fn senescence(&self) -> Option<SenescenceRules> {
        self.senescence
    }

    /// Basal maintenance multiplier for an organism born at `birth_tick` when
    /// settling tick `from` (`1.0` when `senescence` is disabled).
    ///
    /// # Errors
    /// As [`age_seconds`] and [`senescence_multiplier`].
    pub fn basal_multiplier(
        &self,
        birth_tick: Tick,
        from: Tick,
        step: TimeStep,
    ) -> Result<f64, CoreError> {
        match self.senescence {
            None => Ok(1.0),
            Some(rule) => {
                let age_s = age_seconds(birth_tick, from, step)?;
                senescence_multiplier(age_s, rule.scale_seconds, rule.maximum_multiplier)
            }
        }
    }

    /// Resolved `V2-04` emission rules (`surplus-emission`, `excitable-relay`,
    /// `wake-offset`).
    #[must_use]
    pub const fn emission(&self) -> &EmissionV2Rules {
        &self.emission
    }

    /// Resolved `V2-05` (`so2-motor`) steering constants, if enabled.
    #[must_use]
    pub const fn motor(&self) -> Option<MotorSteeringConstants> {
        self.motor
    }

    /// Whether Phase 6 motion needs the `SettleBackend::motion_v2` route
    /// (`V2-03` `senescence` or `V2-05` `so2-motor`).
    #[must_use]
    pub const fn needs_motion_v2(&self) -> bool {
        self.senescence.is_some() || self.motor.is_some()
    }

    /// Resolved `V2-06` (`so2-budding`) rules, if enabled.
    #[must_use]
    pub const fn budding(&self) -> Option<BuddingRules> {
        self.budding
    }

    /// Resolved Hippo-YAP crowding rules (`V2-07`), if enabled.
    #[must_use]
    pub const fn crowding(&self) -> Option<CrowdingRules> {
        self.crowding
    }

    /// Resolved viability-floor corpse lysis rules (`V2-11`), if enabled.
    #[must_use]
    pub const fn lysis(&self) -> Option<LysisRules> {
        self.lysis
    }

    /// Resolved obligate symplectic metabolic waste rules (`V2-11`), if enabled.
    #[must_use]
    pub const fn waste(&self) -> Option<WasteRules> {
        self.waste
    }

    /// Number of active Carrier-0 chemical channels (`24` under `V2-15`
    /// dual-carrier, `32` on single-carrier fields).
    #[must_use]
    pub const fn active_carrier0_channels(&self) -> usize {
        if self.dual_carrier.is_some() {
            CARRIER0_MOMENT_CHANNELS as usize
        } else {
            CHANNELS
        }
    }

    /// Materialized `[v2]` configuration, if this is a `field-ecology-v2*` world.
    #[must_use]
    pub const fn config(&self) -> Option<&V2Config> {
        self.config.as_ref()
    }
}
