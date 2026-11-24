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

/// Resolved persistent apical-basal PCP polarity parameters (`V2-08`,
/// `pcp-polarity`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolarityRules {
    /// Angular torque mobility `μ_θ >= 0` (`0.0` freezes polarity vectors).
    pub torque_mobility: f64,
    /// Basal-apical energy-gradient cue strength `k_cue >= 0`.
    pub gradient_cue_strength: f64,
    /// Founder polarity strength ceiling `∈ [0.0, 1.0]`.
    pub initial_polarity_strength_max: f64,
    /// Maximum heritable wedge curvature `β_max ∈ [0.0, 0.4]`.
    pub maximum_wedge_curvature: f64,
    /// Heritable polarity trait mutation std `>= 0.0`.
    pub polarity_mutation_std: f64,
    /// Whether epithelial division aligns daughters laterally along `±p_i^⊥`.
    pub oriented_lateral_mitosis: bool,
}

/// Newborn epigenetic recurrent-state inheritance rule (`V2-12`,
/// `epigenetic-recurrence`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StateInheritanceMode {
    /// Newborns start with zero / empty recurrent state (`HeldOutput::INVALID`).
    Zero,
    /// Symmetric inheritance with retention and semantic-Philox Gaussian jitter:
    /// `h_child[k] = clamp(α_epi * h_p[k] + σ_epi * ξ_k, -1, 1)`.
    SymmetricEpigenetic,
    /// Asymmetric stem-cell division on `k = 0`:
    /// `c_stem = σ(3 * h_p[0])`,
    /// `h_child[0] = clamp(α_epi * h_p[0] + c_stem * β_asym, -1, 1)`,
    /// and symmetric inheritance on `k = 1..S`.
    AsymmetricStem,
}

/// Juxtacrine Notch-Delta lateral inhibition coupling mode (`V2-12`,
/// `notch-delta`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotchDeltaMode {
    /// Feed kin-weighted mean neighbor `h̄_0` into `SensorLayout::notch_delta()`.
    SensorNeighborMean,
    /// Subtract `η_ND * h̄_0` directly from post-Phase-4 `h'_0` (0 extra sensors).
    DirectLateralInhibition,
}

/// Resolved epigenetic recurrence & clonal differentiation rules (`V2-12`,
/// `epigenetic-recurrence`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DifferentiationRules {
    /// Recurrent state width `S ∈ 1..=16`.
    pub state_width: usize,
    /// Epigenetic slow-gate bias `b_epi ∈ [0.0, 4.0]`.
    pub epigenetic_gate_bias: f64,
    /// Newborn state inheritance mode.
    pub state_inheritance: StateInheritanceMode,
    /// Retention factor `α_epi ∈ [0.0, 1.0]`.
    pub inheritance_retention: f64,
    /// Jitter standard deviation `σ_epi >= 0.0`.
    pub inheritance_jitter: f64,
    /// Asymmetric stem push `β_asym ∈ [-2.0, 2.0]`.
    pub stem_asymmetry: f64,
    /// Whether `h_0` rotates effective metabolic affinity `a_eff(h_0)`.
    pub affinity_rotation_enabled: bool,
    /// Whether `h_1` modulates EMT adhesion multiplier `m(h_1) = clamp(1 + h_1, 0, 1)`.
    pub emt_adhesion_enabled: bool,
}

/// Resolved juxtacrine Notch-Delta rules (`V2-12`, `notch-delta`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NotchDeltaRules {
    /// Coupling mode (`sensor-neighbor-mean-v1` or `direct-lateral-inhibition-v1`).
    pub mode: NotchDeltaMode,
    /// Direct lateral inhibition gain `η_ND ∈ [0.0, 0.3]`.
    pub inhibition_gain: f64,
}

/// Resolved passive exact-quantum kin trophallaxis rules (`V2-13`, `C013`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrophallaxisRules {
    /// Gap-junction conductance `G_gap >= 0` in `s^-1`.
    pub conductance_per_second: f64,
    /// Transfer efficiency `ρ_tr ∈ (0, 0.99]`.
    pub transfer_efficiency: f64,
    /// Donor energy reserve floor `E_reserve` (`> tiny_energy_threshold` and
    /// `> E_viable` when `lysis-recycling` is active).
    pub reserve_energy: f64,
    /// Maximum fraction `f_max ∈ (0, 1]` of `(E_i - E_reserve).max(0)` exported per tick.
    pub maximum_export_fraction: f64,
    /// Exponent `p` applied to `max(0, cos(a_i, a_j))` (default `4`).
    pub kin_cosine_exponent: u32,
    /// Minimum genetic affinity cosine required for transfer (`∈ [0, 1]`, default `0.5`).
    pub kin_threshold: f64,
}

/// Resolved decoupled 4-D cadherin adhesion rules (`V2-13`, `C023`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CadherinRules {
    /// Per-birth Gaussian mutation standard deviation (`>= 0`).
    pub mutation_std: f64,
}

/// Resolved dissipative kin velocity alignment rules (`V2-13`, `C019`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VelocityAlignmentRules {
    /// Alignment blend weight `μ_align ∈ [0, 0.4]`.
    pub strength: f64,
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
    polarity: Option<PolarityRules>,
    emt_gate: bool,
    differentiation: Option<DifferentiationRules>,
    notch_delta: Option<NotchDeltaRules>,
    trophallaxis: Option<TrophallaxisRules>,
    cadherin: Option<CadherinRules>,
    velocity_alignment: Option<VelocityAlignmentRules>,
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
            let limit = config.config().reproduction.minimum_child_energy.min(
                config
                    .config()
                    .reproduction
                    .minimum_parent_energy_after_split,
            );
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
        let polarity = if let Some(v2) = v2_config
            .as_ref()
            .filter(|v2| v2.is_enabled(V2Feature::PcpPolarity))
        {
            let pol = &v2.polarity;
            if !pol.torque_mobility.is_finite() || pol.torque_mobility < 0.0 {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.polarity.torque_mobility",
                    detail: format!("{} must be finite and >= 0", pol.torque_mobility),
                });
            }
            if !pol.gradient_cue_strength.is_finite() || pol.gradient_cue_strength < 0.0 {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.polarity.gradient_cue_strength",
                    detail: format!("{} must be finite and >= 0", pol.gradient_cue_strength),
                });
            }
            if !pol.initial_polarity_strength_max.is_finite()
                || !(0.0..=1.0).contains(&pol.initial_polarity_strength_max)
            {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.polarity.initial_polarity_strength_max",
                    detail: format!(
                        "{} must lie in [0.0, 1.0]",
                        pol.initial_polarity_strength_max
                    ),
                });
            }
            if !pol.maximum_wedge_curvature.is_finite()
                || !(0.0..=0.4).contains(&pol.maximum_wedge_curvature)
            {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.polarity.maximum_wedge_curvature",
                    detail: format!("{} must lie in [0.0, 0.4]", pol.maximum_wedge_curvature),
                });
            }
            if !pol.polarity_mutation_std.is_finite() || pol.polarity_mutation_std < 0.0 {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.polarity.polarity_mutation_std",
                    detail: format!("{} must be finite and >= 0", pol.polarity_mutation_std),
                });
            }
            Some(PolarityRules {
                torque_mobility: pol.torque_mobility + 0.0,
                gradient_cue_strength: pol.gradient_cue_strength + 0.0,
                initial_polarity_strength_max: pol.initial_polarity_strength_max + 0.0,
                maximum_wedge_curvature: pol.maximum_wedge_curvature + 0.0,
                polarity_mutation_std: pol.polarity_mutation_std + 0.0,
                oriented_lateral_mitosis: pol.oriented_lateral_mitosis,
            })
        } else {
            None
        };
        let emt_gate = v2_config
            .as_ref()
            .is_some_and(|v2| v2.is_enabled(V2Feature::EmtGate));
        let differentiation = if let Some(v2) = v2_config
            .as_ref()
            .filter(|v2| v2.is_enabled(V2Feature::EpigeneticRecurrence))
        {
            let df = &v2.differentiation;
            if state_width == 0 {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.differentiation.state_width",
                    detail: "epigenetic-recurrence requires state_width >= 1".to_owned(),
                });
            }
            if !df.epigenetic_gate_bias.is_finite()
                || !(0.0..=4.0).contains(&df.epigenetic_gate_bias)
            {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.differentiation.epigenetic_gate_bias",
                    detail: format!("{} must lie in [0.0, 4.0]", df.epigenetic_gate_bias),
                });
            }
            if !df.inheritance_retention.is_finite()
                || !(0.0..=1.0).contains(&df.inheritance_retention)
            {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.differentiation.inheritance_retention",
                    detail: format!("{} must lie in [0.0, 1.0]", df.inheritance_retention),
                });
            }
            if !df.inheritance_jitter.is_finite() || df.inheritance_jitter < 0.0 {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.differentiation.inheritance_jitter",
                    detail: format!("{} must be finite and >= 0", df.inheritance_jitter),
                });
            }
            if !df.stem_asymmetry.is_finite() || !(-2.0..=2.0).contains(&df.stem_asymmetry) {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.differentiation.stem_asymmetry",
                    detail: format!("{} must lie in [-2.0, 2.0]", df.stem_asymmetry),
                });
            }
            let state_inheritance = match df.state_inheritance.as_str() {
                "zero" => StateInheritanceMode::Zero,
                "symmetric-epigenetic-v1" => StateInheritanceMode::SymmetricEpigenetic,
                "asymmetric-stem-v1" => StateInheritanceMode::AsymmetricStem,
                other => {
                    return Err(CoreError::UnknownAlgorithm {
                        role: "v2.differentiation.state_inheritance",
                        name: other.to_owned(),
                    });
                }
            };
            Some(DifferentiationRules {
                state_width,
                epigenetic_gate_bias: df.epigenetic_gate_bias + 0.0,
                state_inheritance,
                inheritance_retention: df.inheritance_retention + 0.0,
                inheritance_jitter: df.inheritance_jitter + 0.0,
                stem_asymmetry: df.stem_asymmetry + 0.0,
                affinity_rotation_enabled: df.affinity_rotation_enabled,
                emt_adhesion_enabled: df.emt_adhesion_enabled,
            })
        } else {
            None
        };
        let notch_delta = if let Some(v2) = v2_config
            .as_ref()
            .filter(|v2| v2.is_enabled(V2Feature::NotchDelta))
        {
            let nd = &v2.differentiation.notch_delta;
            if differentiation.is_none() {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.differentiation.notch_delta.enabled",
                    detail: "notch-delta requires epigenetic-recurrence enabled".to_owned(),
                });
            }
            if !nd.inhibition_gain.is_finite() || !(0.0..=0.3).contains(&nd.inhibition_gain) {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.differentiation.notch_delta.inhibition_gain",
                    detail: format!("{} must lie in [0.0, 0.3]", nd.inhibition_gain),
                });
            }
            let mode = match nd.mode.as_str() {
                "sensor-neighbor-mean-v1" => NotchDeltaMode::SensorNeighborMean,
                "direct-lateral-inhibition-v1" => NotchDeltaMode::DirectLateralInhibition,
                other => {
                    return Err(CoreError::UnknownAlgorithm {
                        role: "v2.differentiation.notch_delta.mode",
                        name: other.to_owned(),
                    });
                }
            };
            Some(NotchDeltaRules {
                mode,
                inhibition_gain: nd.inhibition_gain + 0.0,
            })
        } else {
            None
        };
        let has_contact = config.config().contact.is_some();
        let trophallaxis = if let Some(v2) = v2_config
            .as_ref()
            .filter(|v2| v2.is_enabled(V2Feature::Trophallaxis))
        {
            if !has_contact {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.trophallaxis.enabled",
                    detail: "trophallaxis requires a [contact] table".to_owned(),
                });
            }
            let tr = &v2.trophallaxis;
            if !tr.conductance_per_second.is_finite() || tr.conductance_per_second < 0.0 {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.trophallaxis.conductance_per_second",
                    detail: format!(
                        "{} must be finite and nonnegative",
                        tr.conductance_per_second
                    ),
                });
            }
            if !tr.transfer_efficiency.is_finite()
                || !(tr.transfer_efficiency > 0.0 && tr.transfer_efficiency <= 0.99)
            {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.trophallaxis.transfer_efficiency",
                    detail: format!("{} must lie in (0.0, 0.99]", tr.transfer_efficiency),
                });
            }
            if !tr.maximum_export_fraction.is_finite()
                || !(tr.maximum_export_fraction > 0.0 && tr.maximum_export_fraction <= 1.0)
            {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.trophallaxis.maximum_export_fraction",
                    detail: format!("{} must lie in (0.0, 1.0]", tr.maximum_export_fraction),
                });
            }
            if !tr.kin_threshold.is_finite() || !(0.0..=1.0).contains(&tr.kin_threshold) {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.trophallaxis.kin_threshold",
                    detail: format!("{} must lie in [0.0, 1.0]", tr.kin_threshold),
                });
            }
            let tiny = config.config().world.tiny_energy_threshold;
            let lysis_floor = lysis.map_or(0.0, |rule| rule.viability_energy_floor);
            let reserve_energy = tr
                .reserve_energy
                .unwrap_or_else(|| 0.25_f64.max(tiny * 2.0).max(lysis_floor * 2.0));
            if !reserve_energy.is_finite() || reserve_energy <= tiny {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.trophallaxis.reserve_energy",
                    detail: format!(
                        "{reserve_energy:?} must be finite and > tiny_energy_threshold ({tiny:?})"
                    ),
                });
            }
            if let Some(rule) = lysis
                && reserve_energy <= rule.viability_energy_floor
            {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.trophallaxis.reserve_energy",
                    detail: format!(
                        "{reserve_energy:?} must be > v2.metabolism.lysis.viability_energy_floor ({:?})",
                        rule.viability_energy_floor
                    ),
                });
            }
            Some(TrophallaxisRules {
                conductance_per_second: tr.conductance_per_second + 0.0,
                transfer_efficiency: tr.transfer_efficiency + 0.0,
                reserve_energy: reserve_energy + 0.0,
                maximum_export_fraction: tr.maximum_export_fraction + 0.0,
                kin_cosine_exponent: tr.kin_cosine_exponent,
                kin_threshold: tr.kin_threshold + 0.0,
            })
        } else {
            None
        };
        let cadherin = if let Some(v2) = v2_config
            .as_ref()
            .filter(|v2| v2.is_enabled(V2Feature::Cadherin4d))
        {
            if !has_contact {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.contact.cadherin.enabled",
                    detail: "cadherin-4d requires a [contact] table".to_owned(),
                });
            }
            let mutation_std = v2.contact.cadherin.mutation_std;
            if !mutation_std.is_finite() || mutation_std < 0.0 {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.contact.cadherin.mutation_std",
                    detail: format!("{mutation_std:?} must be finite and nonnegative"),
                });
            }
            Some(CadherinRules {
                mutation_std: mutation_std + 0.0,
            })
        } else {
            None
        };
        let velocity_alignment = if let Some(v2) = v2_config
            .as_ref()
            .filter(|v2| v2.is_enabled(V2Feature::VelocityAlignment))
        {
            if !has_contact {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.contact.velocity_alignment.enabled",
                    detail: "velocity-alignment requires a [contact] table".to_owned(),
                });
            }
            let strength = v2.contact.velocity_alignment.strength;
            if !strength.is_finite() || !(0.0..=0.4).contains(&strength) {
                return Err(CoreError::NumericalInvalid {
                    quantity: "v2.contact.velocity_alignment.strength",
                    detail: format!("{strength:?} must lie in [0.0, 0.4]"),
                });
            }
            Some(VelocityAlignmentRules {
                strength: strength + 0.0,
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
            polarity,
            emt_gate,
            differentiation,
            notch_delta,
            trophallaxis,
            cadherin,
            velocity_alignment,
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

    /// Resolved persistent apical-basal PCP polarity rules (`V2-08`), if enabled.
    #[must_use]
    pub const fn polarity(&self) -> Option<PolarityRules> {
        self.polarity
    }

    /// Whether the neural EMT adhesion gate (`V2-08`, `emt-gate`) is enabled.
    #[must_use]
    pub const fn emt_gate(&self) -> bool {
        self.emt_gate
    }

    /// Resolved epigenetic recurrence & clonal differentiation rules (`V2-12`),
    /// if enabled.
    #[must_use]
    pub const fn differentiation(&self) -> Option<DifferentiationRules> {
        self.differentiation
    }

    /// Resolved juxtacrine Notch-Delta lateral inhibition rules (`V2-12`), if
    /// enabled.
    #[must_use]
    pub const fn notch_delta(&self) -> Option<NotchDeltaRules> {
        self.notch_delta
    }

    /// Resolved passive exact-quantum kin trophallaxis rules (`V2-13`,
    /// `trophallaxis`), if enabled.
    #[must_use]
    pub const fn trophallaxis(&self) -> Option<TrophallaxisRules> {
        self.trophallaxis
    }

    /// Resolved decoupled 4-D cadherin adhesion rules (`V2-13`,
    /// `cadherin-4d`), if enabled.
    #[must_use]
    pub const fn cadherin(&self) -> Option<CadherinRules> {
        self.cadherin
    }

    /// Resolved dissipative kin velocity alignment rules (`V2-13`,
    /// `velocity-alignment`), if enabled.
    #[must_use]
    pub const fn velocity_alignment(&self) -> Option<VelocityAlignmentRules> {
        self.velocity_alignment
    }

    /// Whether `h_0` rotates effective metabolic affinity `a_eff(h_0)` (`V2-12`).
    #[must_use]
    pub const fn affinity_rotation_enabled(&self) -> bool {
        match self.differentiation {
            Some(diff) => diff.affinity_rotation_enabled,
            None => false,
        }
    }

    /// Whether recurrent state `h_1` modulates adhesion via
    /// `m(h_1) = clamp(1 + h_1, 0, 1)` (`V2-12`; requires `state_width >= 2`).
    #[must_use]
    pub const fn emt_h1_adhesion_enabled(&self) -> bool {
        match self.differentiation {
            Some(diff) => diff.emt_adhesion_enabled && diff.state_width >= 2,
            None => false,
        }
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
