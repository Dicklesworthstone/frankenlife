//! Modular `field-ecology-v2` feature registry, configuration, profile
//! materialization, CLI/config toggle resolution, and canonical ABI dimensions
//! (V2-01; plan §17.2, §17.5).
//!
//! Every `field-ecology-v1*` configuration leaves `RunConfig::v2` as `None`
//! and never emits a `[v2]` key, so v1 canonical TOML, configuration hashes,
//! model descriptors, and checkpoints remain 100% byte-identical.
//!
//! On `field-ecology-v2*` configurations, `validate()` materializes every
//! profile default, TOML toggle, and founding CLI override into explicit
//! `enabled = true/false` fields and concrete parameter values before
//! canonicalization and hashing (`INV-14`), so checkpoints carry the complete
//! effective feature closure with zero hidden defaults.

use serde::Deserialize;

use crate::config::{ControllerConfig, RunConfig};
use crate::error::{ConfigIssue, CoreError};
use crate::model::{
    ACTION_WIDTH, MAX_ACTION_WIDTH, MAX_CONTROLLER_OUTPUT_WIDTH, MAX_SENSOR_WIDTH,
    MAXIMUM_RECURRENT_STATE_WIDTH, SENSOR_WIDTH, is_v2_model,
};
use crate::schema::Scope;

/// The single-carrier field plane count (1 energy + 32 moment planes).
pub const SINGLE_CARRIER_PLANES: u32 = 33;

/// The dual-carrier hybrid field plane count (`V2-15`: 25 Carrier-0 + 9 Carrier-1).
pub const DUAL_CARRIER_PLANES: u32 = 34;

/// Every modular `field-ecology-v2` feature toggle (21 stable kebab-case slugs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum V2Feature {
    /// Phase-staggered ephemeral oasis resource landscapes (`V2-02`).
    Oases = 0,
    /// Gompertz metabolic senescence maintenance ramp (`V2-03`).
    Senescence = 1,
    /// Surplus-gated emission energy floor (`V2-04`).
    SurplusEmission = 2,
    /// Excitable refractory pulse-relay emission (`V2-04`).
    ExcitableRelay = 3,
    /// Wake-velocity or polarized-pump emission offset (`V2-04`).
    WakeOffset = 4,
    /// 4-output SO(2)-equivariant neural chemotaxis & propulsion (`V2-05`).
    So2Motor = 5,
    /// 2-output SO(2)-equivariant oriented budding & dispersal (`V2-06`).
    So2Budding = 6,
    /// Hippo-YAP local crowding birth gate (`V2-07`).
    HippoCrowding = 7,
    /// Persistent apical-basal PCP lateral adhesion, wedge & mitosis (`V2-08`).
    PcpPolarity = 8,
    /// Neural EMT adhesion gate (`V2-08`).
    EmtGate = 9,
    /// Passive symmetric flux-form tight-junction diffusion (`V2-09`).
    TightJunctions = 10,
    /// Viability-floor corpse lysis recycling (`V2-11`).
    LysisRecycling = 11,
    /// Obligate symplectic metabolic waste deposit (`V2-11`).
    SymplecticWaste = 12,
    /// Epigenetic recurrent cell-state dynamics & differentiation (`V2-12`).
    EpigeneticRecurrence = 13,
    /// Notch-Delta lateral inhibition coupling (`V2-12`).
    NotchDelta = 14,
    /// Passive exact-quantum kin trophallaxis (`V2-13`).
    Trophallaxis = 15,
    /// Decoupled 4-D cadherin adhesion tag (`V2-13`).
    Cadherin4d = 16,
    /// Dissipative kin velocity alignment (`V2-13`).
    VelocityAlignment = 17,
    /// 4-neighbor flux-reinforced Murray/Physarum vascular field (`V2-14`).
    VascularField = 18,
    /// 34-plane two-carrier non-destructive pheromone trail hybrid (`V2-15`).
    DualCarrierTrails = 19,
    /// Pure read-only (`INV-09`) render & visual mappings (`V2-16`).
    V2VisualMappings = 20,
}

impl V2Feature {
    /// All 21 features in canonical bit order.
    pub const ALL: [Self; 21] = [
        Self::Oases,
        Self::Senescence,
        Self::SurplusEmission,
        Self::ExcitableRelay,
        Self::WakeOffset,
        Self::So2Motor,
        Self::So2Budding,
        Self::HippoCrowding,
        Self::PcpPolarity,
        Self::EmtGate,
        Self::TightJunctions,
        Self::LysisRecycling,
        Self::SymplecticWaste,
        Self::EpigeneticRecurrence,
        Self::NotchDelta,
        Self::Trophallaxis,
        Self::Cadherin4d,
        Self::VelocityAlignment,
        Self::VascularField,
        Self::DualCarrierTrails,
        Self::V2VisualMappings,
    ];

    /// Bundle A (Animal-Tissue) default-enabled features (14 features).
    /// Used by `v2.profile = "tissue"` and `v2.profile = "auto"` when `[contact]` is present.
    pub const BUNDLE_A_DEFAULTS: [Self; 14] = [
        Self::HippoCrowding,
        Self::PcpPolarity,
        Self::TightJunctions,
        Self::Senescence,
        Self::LysisRecycling,
        Self::SymplecticWaste,
        Self::EpigeneticRecurrence,
        Self::NotchDelta,
        Self::Trophallaxis,
        Self::Cadherin4d,
        Self::So2Motor,
        Self::SurplusEmission,
        Self::WakeOffset,
        Self::V2VisualMappings,
    ];

    /// Bundle B (Pure-Field Super-Colony) default-enabled features (13 features).
    /// Used by `v2.profile = "supercolony"` and `v2.profile = "auto"` when `[contact]` is absent.
    pub const BUNDLE_B_DEFAULTS: [Self; 13] = [
        Self::Oases,
        Self::Senescence,
        Self::SurplusEmission,
        Self::ExcitableRelay,
        Self::WakeOffset,
        Self::So2Motor,
        Self::So2Budding,
        Self::TightJunctions,
        Self::LysisRecycling,
        Self::SymplecticWaste,
        Self::VascularField,
        Self::DualCarrierTrails,
        Self::V2VisualMappings,
    ];

    /// The stable kebab-case feature slug used in TOML, CLI flags, and descriptors.
    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Oases => "oases",
            Self::Senescence => "senescence",
            Self::SurplusEmission => "surplus-emission",
            Self::ExcitableRelay => "excitable-relay",
            Self::WakeOffset => "wake-offset",
            Self::So2Motor => "so2-motor",
            Self::So2Budding => "so2-budding",
            Self::HippoCrowding => "hippo-crowding",
            Self::PcpPolarity => "pcp-polarity",
            Self::EmtGate => "emt-gate",
            Self::TightJunctions => "tight-junctions",
            Self::LysisRecycling => "lysis-recycling",
            Self::SymplecticWaste => "symplectic-waste",
            Self::EpigeneticRecurrence => "epigenetic-recurrence",
            Self::NotchDelta => "notch-delta",
            Self::Trophallaxis => "trophallaxis",
            Self::Cadherin4d => "cadherin-4d",
            Self::VelocityAlignment => "velocity-alignment",
            Self::VascularField => "vascular-field",
            Self::DualCarrierTrails => "dual-carrier-trails",
            Self::V2VisualMappings => "v2-visual-mappings",
        }
    }

    /// Stable bit position in the checkpoint v3 feature bitmask.
    #[must_use]
    pub const fn bit(self) -> u64 {
        1_u64 << (self as u32)
    }

    /// Identity scope (`Scope::Presentation` for `v2-visual-mappings`, `Scope::Model` otherwise).
    #[must_use]
    pub const fn scope(self) -> Scope {
        match self {
            Self::V2VisualMappings => Scope::Presentation,
            _ => Scope::Model,
        }
    }

    /// Whether this feature unconditionally requires `[contact]` (and sets `inv08_pure_field = false`).
    ///
    /// Note that [`Self::HippoCrowding`] is mode-dependent: `"local-neighbors-v1"` requires
    /// `[contact]`, whereas `"cell-occupancy-v1"` is pure-field `INV-08`.
    #[must_use]
    pub const fn requires_contact_unconditionally(self) -> bool {
        matches!(
            self,
            Self::PcpPolarity
                | Self::EmtGate
                | Self::NotchDelta
                | Self::Trophallaxis
                | Self::Cadherin4d
                | Self::VelocityAlignment
        )
    }

    /// Whether enabled by default in Bundle A (`tissue` / `auto` with `[contact]`).
    #[must_use]
    pub const fn default_in_bundle_a(self) -> bool {
        matches!(
            self,
            Self::HippoCrowding
                | Self::PcpPolarity
                | Self::TightJunctions
                | Self::Senescence
                | Self::LysisRecycling
                | Self::SymplecticWaste
                | Self::EpigeneticRecurrence
                | Self::NotchDelta
                | Self::Trophallaxis
                | Self::Cadherin4d
                | Self::So2Motor
                | Self::SurplusEmission
                | Self::WakeOffset
                | Self::V2VisualMappings
        )
    }

    /// Whether enabled by default in Bundle B (`supercolony` / `auto` without `[contact]`).
    #[must_use]
    pub const fn default_in_bundle_b(self) -> bool {
        matches!(
            self,
            Self::Oases
                | Self::Senescence
                | Self::SurplusEmission
                | Self::ExcitableRelay
                | Self::WakeOffset
                | Self::So2Motor
                | Self::So2Budding
                | Self::TightJunctions
                | Self::LysisRecycling
                | Self::SymplecticWaste
                | Self::VascularField
                | Self::DualCarrierTrails
                | Self::V2VisualMappings
        )
    }

    /// Human/machine summary of `INV-08` status for `--list-features`.
    #[must_use]
    pub const fn inv08_status(self) -> &'static str {
        match self {
            Self::V2VisualMappings => "presentation-only (INV-09)",
            Self::HippoCrowding => {
                "mode-dependent (cell-occupancy-v1 pure-field; local-neighbors-v1 contact)"
            }
            feature if feature.requires_contact_unconditionally() => {
                "contact-required (inv08_pure_field=false)"
            }
            _ => "pure-field (INV-08)",
        }
    }

    /// Human/machine summary of sensor/action/plane ABI impact for `--list-features`.
    #[must_use]
    pub const fn abi_impact(self) -> &'static str {
        match self {
            Self::DualCarrierTrails => "+4 sensors, 34 field planes",
            Self::HippoCrowding => "+1 sensor if expose_sensor=true",
            Self::NotchDelta => "+1 sensor if mode=sensor-neighbor-mean-v1",
            Self::EpigeneticRecurrence => "+S sensors, +2S raw outputs",
            Self::So2Motor => "+4 actions",
            Self::So2Budding => "+2 actions",
            Self::EmtGate => "+1 action",
            Self::WakeOffset => {
                "+1 action if mode=polarized-pump-v1 and pump_direction_mode=neural-output-v1"
            }
            _ => "none",
        }
    }

    /// Resolve a single canonical kebab-case slug.
    #[must_use]
    pub fn from_slug(slug: &str) -> Option<Self> {
        match slug {
            "oases" => Some(Self::Oases),
            "senescence" => Some(Self::Senescence),
            "surplus-emission" => Some(Self::SurplusEmission),
            "excitable-relay" => Some(Self::ExcitableRelay),
            "wake-offset" => Some(Self::WakeOffset),
            "so2-motor" => Some(Self::So2Motor),
            "so2-budding" => Some(Self::So2Budding),
            "hippo-crowding" => Some(Self::HippoCrowding),
            "pcp-polarity" => Some(Self::PcpPolarity),
            "emt-gate" => Some(Self::EmtGate),
            "tight-junctions" => Some(Self::TightJunctions),
            "lysis-recycling" => Some(Self::LysisRecycling),
            "symplectic-waste" => Some(Self::SymplecticWaste),
            "epigenetic-recurrence" => Some(Self::EpigeneticRecurrence),
            "notch-delta" => Some(Self::NotchDelta),
            "trophallaxis" => Some(Self::Trophallaxis),
            "cadherin-4d" => Some(Self::Cadherin4d),
            "velocity-alignment" => Some(Self::VelocityAlignment),
            "vascular-field" => Some(Self::VascularField),
            "dual-carrier-trails" => Some(Self::DualCarrierTrails),
            "v2-visual-mappings" => Some(Self::V2VisualMappings),
            _ => None,
        }
    }

    /// Expand a canonical feature slug or convenience alias (`all-v2`, `bundle-a`, `bundle-b`).
    #[must_use]
    pub fn expand_slug_or_alias(token: &str) -> Option<&'static [Self]> {
        const SINGLE: [[V2Feature; 1]; 21] = [
            [V2Feature::Oases],
            [V2Feature::Senescence],
            [V2Feature::SurplusEmission],
            [V2Feature::ExcitableRelay],
            [V2Feature::WakeOffset],
            [V2Feature::So2Motor],
            [V2Feature::So2Budding],
            [V2Feature::HippoCrowding],
            [V2Feature::PcpPolarity],
            [V2Feature::EmtGate],
            [V2Feature::TightJunctions],
            [V2Feature::LysisRecycling],
            [V2Feature::SymplecticWaste],
            [V2Feature::EpigeneticRecurrence],
            [V2Feature::NotchDelta],
            [V2Feature::Trophallaxis],
            [V2Feature::Cadherin4d],
            [V2Feature::VelocityAlignment],
            [V2Feature::VascularField],
            [V2Feature::DualCarrierTrails],
            [V2Feature::V2VisualMappings],
        ];
        match token {
            "all-v2" => Some(&Self::ALL),
            "bundle-a" => Some(&Self::BUNDLE_A_DEFAULTS),
            "bundle-b" => Some(&Self::BUNDLE_B_DEFAULTS),
            other => Self::from_slug(other).map(|feature| SINGLE[feature as usize].as_slice()),
        }
    }
}

/// Named `v2.profile` selection determining default feature activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum V2Profile {
    /// Tissue defaults when `[contact]` is present; supercolony defaults when absent.
    Auto,
    /// Bundle A animal-tissue defaults (14 features; requires `[contact]`).
    Tissue,
    /// Bundle B pure-field supercolony defaults (13 features; `inv08_pure_field = true`).
    Supercolony,
    /// All 21 v2 features enabled (requires `[contact]`).
    All,
    /// Zero v2 features enabled (bit-exact `v1` / `v1-contact` baseline).
    Minimal,
}

impl V2Profile {
    /// Parse a `v2.profile` string.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "auto" => Some(Self::Auto),
            "tissue" => Some(Self::Tissue),
            "supercolony" => Some(Self::Supercolony),
            "all" => Some(Self::All),
            "minimal" => Some(Self::Minimal),
            _ => None,
        }
    }

    /// Canonical profile name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Tissue => "tissue",
            Self::Supercolony => "supercolony",
            Self::All => "all",
            Self::Minimal => "minimal",
        }
    }

    /// Default activation state of `feature` under this profile.
    #[must_use]
    pub const fn default_enabled(self, feature: V2Feature, has_contact: bool) -> bool {
        match self {
            Self::Auto => {
                if has_contact {
                    feature.default_in_bundle_a()
                } else {
                    feature.default_in_bundle_b()
                }
            }
            Self::Tissue => feature.default_in_bundle_a(),
            Self::Supercolony => feature.default_in_bundle_b(),
            Self::All => true,
            Self::Minimal => false,
        }
    }
}

/// One CLI or programmatic feature-toggle operation applied before validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeatureOverride {
    /// Disable one slug or alias (`--disable <token>`).
    Disable(String),
    /// Enable one slug or alias (`--enable <token>`).
    Enable(String),
    /// Disable all 21 v2 features (`--v1-compat`, equivalent to `--disable all-v2`).
    V1Compat,
}

/// Pure scientific controller and field ABI dimensions derived from a
/// configuration. Identical across Reference CPU, FrankenTorch CPU, and Metal
/// (never carries Metal-only dead input lanes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct V2Abi {
    /// Non-recurrent sensor width (`104..=110`).
    pub sensor_width: u32,
    /// Recurrent internal state width `S` (`0..=16`).
    pub recurrent_state_width: u32,
    /// Total controller input width `sensor_width + recurrent_state_width` (`104..=128`).
    pub input_width: u32,
    /// Stored action output width in `HeldOutput.outputs` (`36..=48`).
    pub action_width: u32,
    /// Total raw controller output width `action_width + 2 * recurrent_state_width` (`36..=80`).
    pub raw_output_width: u32,
    /// Field plane count (`33` single-carrier, `34` dual-carrier).
    pub field_planes: u32,
    /// `+4` sensor inputs for `dual-carrier-trails`.
    pub dual_carrier_sensors: bool,
    /// `+1` sensor input for `hippo-crowding` (`expose_sensor = true`).
    pub hippo_crowding_sensor: bool,
    /// `+1` sensor input for `notch-delta` (`mode = "sensor-neighbor-mean-v1"`).
    pub notch_delta_sensor: bool,
    /// `+4` action outputs for `so2-motor`.
    pub so2_motor_actions: bool,
    /// `+2` action outputs for `so2-budding`.
    pub so2_budding_actions: bool,
    /// `+1` action output for `emt-gate`.
    pub emt_gate_action: bool,
    /// `+1` action output for `wake-offset` (`polarized-pump-v1` + `neural-output-v1`).
    pub wake_pump_action: bool,
}

impl V2Abi {
    /// The baseline `field-ecology-v1` ABI for a given recurrent state width `S`.
    #[must_use]
    pub const fn v1(recurrent_state_width: u32) -> Self {
        Self {
            sensor_width: SENSOR_WIDTH,
            recurrent_state_width,
            input_width: SENSOR_WIDTH.saturating_add(recurrent_state_width),
            action_width: ACTION_WIDTH,
            raw_output_width: ACTION_WIDTH.saturating_add(recurrent_state_width.saturating_mul(2)),
            field_planes: SINGLE_CARRIER_PLANES,
            dual_carrier_sensors: false,
            hippo_crowding_sensor: false,
            notch_delta_sensor: false,
            so2_motor_actions: false,
            so2_budding_actions: false,
            emt_gate_action: false,
            wake_pump_action: false,
        }
    }

    /// Format `<inputs>/<actions>/<raw_outputs>/<planes>` for `ModelDescriptor`.
    #[must_use]
    pub fn descriptor_tag(self) -> String {
        format!(
            "{}/{}/{}/{}",
            self.input_width, self.action_width, self.raw_output_width, self.field_planes
        )
    }
}

fn default_profile() -> String {
    "auto".to_owned()
}
fn default_bloom_period() -> f64 {
    24.0
}
fn default_zero() -> f64 {
    0.0
}
fn default_senescence_scale() -> f64 {
    30.0
}
fn default_senescence_max_mult() -> f64 {
    64.0
}
fn default_excitable_mode() -> String {
    "vesicle-readiness-v1".to_owned()
}
fn default_fire_readiness_threshold() -> f64 {
    0.5
}
fn default_depletion_fraction() -> f64 {
    0.85
}
fn default_burst_rate_multiplier() -> f64 {
    5.0
}
fn default_recovery_time_seconds() -> f64 {
    1.0
}
fn default_refractory_cooldown_ticks() -> u32 {
    24
}
fn default_offset_mode() -> String {
    "wake-velocity-v1".to_owned()
}
fn default_pump_direction_mode() -> String {
    "fixed-sign-v1".to_owned()
}
fn default_one() -> f64 {
    1.0
}
fn default_two() -> f64 {
    2.0
}
fn default_half() -> f64 {
    0.5
}
fn default_crowding_mode() -> String {
    "local-neighbors-v1".to_owned()
}
fn default_arrest_threshold() -> f64 {
    4.5
}
fn default_gradient_cue_strength() -> f64 {
    0.25
}
fn default_initial_polarity_strength_max() -> f64 {
    0.3
}
fn default_maximum_wedge_curvature() -> f64 {
    0.35
}
fn default_small_mutation_std() -> f64 {
    0.05
}
fn default_true() -> bool {
    true
}
fn default_barrier_mode() -> String {
    "harmonic-v1".to_owned()
}
fn default_minimum_permeability() -> f64 {
    0.04
}
fn default_occupancy_impedance() -> f64 {
    1.2
}
fn default_corpse_direction() -> String {
    "negated-affinity".to_owned()
}
fn default_byproduct_recycle_fraction() -> f64 {
    0.45
}
fn default_state_width() -> u32 {
    2
}
fn default_epigenetic_gate_bias() -> f64 {
    2.5
}
fn default_state_inheritance() -> String {
    "symmetric-epigenetic-v1".to_owned()
}
fn default_inheritance_retention() -> f64 {
    0.8
}
fn default_stem_asymmetry() -> f64 {
    -0.8
}
fn default_notch_delta_mode() -> String {
    "sensor-neighbor-mean-v1".to_owned()
}
fn default_inhibition_gain() -> f64 {
    0.2
}
fn default_transfer_efficiency() -> f64 {
    0.95
}
fn default_export_fraction() -> f64 {
    0.25
}
fn default_kin_exponent() -> u32 {
    4
}
fn default_velocity_alignment_strength() -> f64 {
    0.20
}
fn default_minimum_conductance_ratio() -> f64 {
    0.025
}
fn default_reinforcement_rate() -> f64 {
    0.5
}
fn default_pruning_rate() -> f64 {
    0.1
}
fn default_flux_half_saturation() -> f64 {
    0.1
}
fn default_flux_exponent() -> f64 {
    1.5
}
fn default_trail_emission_share() -> f64 {
    0.20
}

/// `[v2]` modular configuration table for `field-ecology-v2*` models.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V2Config {
    /// `"auto" | "tissue" | "supercolony" | "all" | "minimal"`.
    #[serde(default = "default_profile")]
    pub profile: String,
    /// Feature slugs or aliases to disable before materialization.
    /// Cleared after `validate()` materializes each module's `enabled` flag.
    #[serde(default)]
    pub disabled_features: Vec<String>,
    /// Feature slugs or aliases to enable before materialization.
    /// Cleared after `validate()` materializes each module's `enabled` flag.
    #[serde(default)]
    pub enabled_features: Vec<String>,
    /// `[v2.oases]` (`V2-02`)
    #[serde(default)]
    pub oases: V2OasesConfig,
    /// `[v2.senescence]` (`V2-03`)
    #[serde(default)]
    pub senescence: V2SenescenceConfig,
    /// `[v2.emission]` (`V2-04`)
    #[serde(default)]
    pub emission: V2EmissionConfig,
    /// `[v2.motor]` (`V2-05`)
    #[serde(default)]
    pub motor: V2MotorConfig,
    /// `[v2.budding]` (`V2-06`)
    #[serde(default)]
    pub budding: V2BuddingConfig,
    /// `[v2.crowding]` (`V2-07`)
    #[serde(default)]
    pub crowding: V2CrowdingConfig,
    /// `[v2.polarity]` (`V2-08`)
    #[serde(default)]
    pub polarity: V2PolarityConfig,
    /// `[v2.barrier]` (`V2-09`)
    #[serde(default)]
    pub barrier: V2BarrierConfig,
    /// `[v2.metabolism]` (`V2-11`)
    #[serde(default)]
    pub metabolism: V2MetabolismConfig,
    /// `[v2.differentiation]` (`V2-12`)
    #[serde(default)]
    pub differentiation: V2DifferentiationConfig,
    /// `[v2.trophallaxis]` (`V2-13`)
    #[serde(default)]
    pub trophallaxis: V2TrophallaxisConfig,
    /// `[v2.contact]` (`V2-13`)
    #[serde(default)]
    pub contact: V2ContactConfig,
    /// `[v2.vascular]` (`V2-14`)
    #[serde(default)]
    pub vascular: V2VascularConfig,
    /// `[v2.carriers]` (`V2-15`)
    #[serde(default)]
    pub carriers: V2CarriersConfig,
    /// `[v2.visual]` (`V2-16`, `Scope::Presentation`)
    #[serde(default)]
    pub visual: V2VisualConfig,
}

impl Default for V2Config {
    fn default() -> Self {
        Self {
            profile: default_profile(),
            disabled_features: Vec::new(),
            enabled_features: Vec::new(),
            oases: V2OasesConfig::default(),
            senescence: V2SenescenceConfig::default(),
            emission: V2EmissionConfig::default(),
            motor: V2MotorConfig::default(),
            budding: V2BuddingConfig::default(),
            crowding: V2CrowdingConfig::default(),
            polarity: V2PolarityConfig::default(),
            barrier: V2BarrierConfig::default(),
            metabolism: V2MetabolismConfig::default(),
            differentiation: V2DifferentiationConfig::default(),
            trophallaxis: V2TrophallaxisConfig::default(),
            contact: V2ContactConfig::default(),
            vascular: V2VascularConfig::default(),
            carriers: V2CarriersConfig::default(),
            visual: V2VisualConfig::default(),
        }
    }
}

/// `[v2.oases]` (`V2-02`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2OasesConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default = "default_bloom_period")]
    pub bloom_period_seconds: f64,
    #[serde(default = "default_zero")]
    pub minimum_power_fraction: f64,
    #[serde(default)]
    pub sigma_min_world_units: Option<f64>,
    #[serde(default)]
    pub sigma_max_world_units: Option<f64>,
}

impl Default for V2OasesConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            bloom_period_seconds: default_bloom_period(),
            minimum_power_fraction: default_zero(),
            sigma_min_world_units: None,
            sigma_max_world_units: None,
        }
    }
}

/// `[v2.senescence]` (`V2-03`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2SenescenceConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default = "default_senescence_scale")]
    pub scale_seconds: f64,
    #[serde(default = "default_senescence_max_mult")]
    pub maximum_multiplier: f64,
}

impl Default for V2SenescenceConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            scale_seconds: default_senescence_scale(),
            maximum_multiplier: default_senescence_max_mult(),
        }
    }
}

/// `[v2.emission]` (`V2-04`).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2EmissionConfig {
    #[serde(default)]
    pub surplus_floor: V2SurplusFloorConfig,
    #[serde(default)]
    pub excitable: V2ExcitableConfig,
    #[serde(default)]
    pub offset: V2EmissionOffsetConfig,
}

/// `[v2.emission.surplus_floor]` (`V2-04`).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2SurplusFloorConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub energy_floor: Option<f64>,
}

/// `[v2.emission.excitable]` (`V2-04`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2ExcitableConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default = "default_excitable_mode")]
    pub mode: String,
    #[serde(default = "default_fire_readiness_threshold")]
    pub fire_readiness_threshold: f64,
    #[serde(default = "default_depletion_fraction")]
    pub depletion_fraction: f64,
    #[serde(default = "default_burst_rate_multiplier")]
    pub burst_rate_multiplier: f64,
    #[serde(default = "default_recovery_time_seconds")]
    pub recovery_time_seconds: f64,
    #[serde(default = "default_refractory_cooldown_ticks")]
    pub refractory_cooldown_ticks: u32,
}

impl Default for V2ExcitableConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            mode: default_excitable_mode(),
            fire_readiness_threshold: default_fire_readiness_threshold(),
            depletion_fraction: default_depletion_fraction(),
            burst_rate_multiplier: default_burst_rate_multiplier(),
            recovery_time_seconds: default_recovery_time_seconds(),
            refractory_cooldown_ticks: default_refractory_cooldown_ticks(),
        }
    }
}

/// `[v2.emission.offset]` (`V2-04`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2EmissionOffsetConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default = "default_offset_mode")]
    pub mode: String,
    #[serde(default = "default_pump_direction_mode")]
    pub pump_direction_mode: String,
    #[serde(default = "default_one")]
    pub pump_sign: f64,
    #[serde(default)]
    pub distance_world_units: Option<f64>,
}

impl Default for V2EmissionOffsetConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            mode: default_offset_mode(),
            pump_direction_mode: default_pump_direction_mode(),
            pump_sign: default_one(),
            distance_world_units: None,
        }
    }
}

/// `[v2.motor]` (`V2-05`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2MotorConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default = "default_two")]
    pub energy_gradient_gain: f64,
    #[serde(default = "default_two")]
    pub chemical_gradient_gain: f64,
    #[serde(default = "default_one")]
    pub chiral_contour_gain: f64,
    #[serde(default = "default_half")]
    pub heading_propulsion_fraction: f64,
}

impl Default for V2MotorConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            energy_gradient_gain: default_two(),
            chemical_gradient_gain: default_two(),
            chiral_contour_gain: default_one(),
            heading_propulsion_fraction: default_half(),
        }
    }
}

/// `[v2.budding]` (`V2-06`).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2BuddingConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub local_offset_radius: Option<f64>,
}

/// `[v2.crowding]` (`V2-07`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2CrowdingConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default = "default_crowding_mode")]
    pub mode: String,
    #[serde(default)]
    pub radius_world_units: Option<f64>,
    #[serde(default = "default_arrest_threshold")]
    pub arrest_threshold: f64,
    #[serde(default)]
    pub expose_sensor: bool,
}

impl Default for V2CrowdingConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            mode: default_crowding_mode(),
            radius_world_units: None,
            arrest_threshold: default_arrest_threshold(),
            expose_sensor: false,
        }
    }
}

/// `[v2.polarity]` (`V2-08`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2PolarityConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default = "default_two")]
    pub torque_mobility: f64,
    #[serde(default = "default_gradient_cue_strength")]
    pub gradient_cue_strength: f64,
    #[serde(default = "default_initial_polarity_strength_max")]
    pub initial_polarity_strength_max: f64,
    #[serde(default = "default_maximum_wedge_curvature")]
    pub maximum_wedge_curvature: f64,
    #[serde(default = "default_small_mutation_std")]
    pub polarity_mutation_std: f64,
    #[serde(default = "default_true")]
    pub oriented_lateral_mitosis: bool,
    #[serde(default)]
    pub emt_gate_enabled: Option<bool>,
}

impl Default for V2PolarityConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            torque_mobility: default_two(),
            gradient_cue_strength: default_gradient_cue_strength(),
            initial_polarity_strength_max: default_initial_polarity_strength_max(),
            maximum_wedge_curvature: default_maximum_wedge_curvature(),
            polarity_mutation_std: default_small_mutation_std(),
            oriented_lateral_mitosis: true,
            emt_gate_enabled: None,
        }
    }
}

/// `[v2.barrier]` (`V2-09`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2BarrierConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default = "default_barrier_mode")]
    pub mode: String,
    #[serde(default = "default_minimum_permeability")]
    pub minimum_permeability: f64,
    #[serde(default = "default_occupancy_impedance")]
    pub occupancy_impedance: f64,
}

impl Default for V2BarrierConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            mode: default_barrier_mode(),
            minimum_permeability: default_minimum_permeability(),
            occupancy_impedance: default_occupancy_impedance(),
        }
    }
}

/// `[v2.metabolism]` (`V2-11`).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2MetabolismConfig {
    #[serde(default)]
    pub lysis: V2LysisConfig,
    #[serde(default)]
    pub waste: V2WasteConfig,
}

/// `[v2.metabolism.lysis]` (`V2-11`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2LysisConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub viability_energy_floor: Option<f64>,
    #[serde(default = "default_corpse_direction")]
    pub corpse_direction: String,
}

impl Default for V2LysisConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            viability_energy_floor: None,
            corpse_direction: default_corpse_direction(),
        }
    }
}

/// `[v2.metabolism.waste]` (`V2-11`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2WasteConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default = "default_byproduct_recycle_fraction")]
    pub byproduct_recycle_fraction: f64,
}

impl Default for V2WasteConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            byproduct_recycle_fraction: default_byproduct_recycle_fraction(),
        }
    }
}

/// `[v2.differentiation]` (`V2-12`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2DifferentiationConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default = "default_state_width")]
    pub state_width: u32,
    #[serde(default = "default_epigenetic_gate_bias")]
    pub epigenetic_gate_bias: f64,
    #[serde(default = "default_state_inheritance")]
    pub state_inheritance: String,
    #[serde(default = "default_inheritance_retention")]
    pub inheritance_retention: f64,
    #[serde(default = "default_small_mutation_std")]
    pub inheritance_jitter: f64,
    #[serde(default = "default_stem_asymmetry")]
    pub stem_asymmetry: f64,
    #[serde(default = "default_true")]
    pub affinity_rotation_enabled: bool,
    #[serde(default = "default_true")]
    pub emt_adhesion_enabled: bool,
    #[serde(default)]
    pub notch_delta: V2NotchDeltaConfig,
}

impl Default for V2DifferentiationConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            state_width: default_state_width(),
            epigenetic_gate_bias: default_epigenetic_gate_bias(),
            state_inheritance: default_state_inheritance(),
            inheritance_retention: default_inheritance_retention(),
            inheritance_jitter: default_small_mutation_std(),
            stem_asymmetry: default_stem_asymmetry(),
            affinity_rotation_enabled: true,
            emt_adhesion_enabled: true,
            notch_delta: V2NotchDeltaConfig::default(),
        }
    }
}

/// `[v2.differentiation.notch_delta]` (`V2-12`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2NotchDeltaConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default = "default_notch_delta_mode")]
    pub mode: String,
    #[serde(default = "default_inhibition_gain")]
    pub inhibition_gain: f64,
}

impl Default for V2NotchDeltaConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            mode: default_notch_delta_mode(),
            inhibition_gain: default_inhibition_gain(),
        }
    }
}

/// `[v2.trophallaxis]` (`V2-13`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2TrophallaxisConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default = "default_two")]
    pub conductance_per_second: f64,
    #[serde(default = "default_transfer_efficiency")]
    pub transfer_efficiency: f64,
    #[serde(default)]
    pub reserve_energy: Option<f64>,
    #[serde(default = "default_export_fraction")]
    pub maximum_export_fraction: f64,
    #[serde(default = "default_kin_exponent")]
    pub kin_cosine_exponent: u32,
    #[serde(default = "default_half")]
    pub kin_threshold: f64,
}

impl Default for V2TrophallaxisConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            conductance_per_second: default_two(),
            transfer_efficiency: default_transfer_efficiency(),
            reserve_energy: None,
            maximum_export_fraction: default_export_fraction(),
            kin_cosine_exponent: default_kin_exponent(),
            kin_threshold: default_half(),
        }
    }
}

/// `[v2.contact]` (`V2-13`).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2ContactConfig {
    #[serde(default)]
    pub cadherin: V2CadherinConfig,
    #[serde(default)]
    pub velocity_alignment: V2VelocityAlignmentConfig,
}

/// `[v2.contact.cadherin]` (`V2-13`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2CadherinConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default = "default_small_mutation_std")]
    pub mutation_std: f64,
}

impl Default for V2CadherinConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            mutation_std: default_small_mutation_std(),
        }
    }
}

/// `[v2.contact.velocity_alignment]` (`V2-13`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2VelocityAlignmentConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default = "default_velocity_alignment_strength")]
    pub strength: f64,
}

impl Default for V2VelocityAlignmentConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            strength: default_velocity_alignment_strength(),
        }
    }
}

/// `[v2.vascular]` (`V2-14`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2VascularConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default = "default_minimum_conductance_ratio")]
    pub minimum_conductance_ratio: f64,
    #[serde(default)]
    pub maximum_diffusion: Option<f64>,
    #[serde(default = "default_reinforcement_rate")]
    pub reinforcement_rate_per_second: f64,
    #[serde(default = "default_pruning_rate")]
    pub pruning_rate_per_second: f64,
    #[serde(default = "default_flux_half_saturation")]
    pub flux_half_saturation: f64,
    #[serde(default = "default_flux_exponent")]
    pub flux_exponent: f64,
}

impl Default for V2VascularConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            minimum_conductance_ratio: default_minimum_conductance_ratio(),
            maximum_diffusion: None,
            reinforcement_rate_per_second: default_reinforcement_rate(),
            pruning_rate_per_second: default_pruning_rate(),
            flux_half_saturation: default_flux_half_saturation(),
            flux_exponent: default_flux_exponent(),
        }
    }
}

/// `[v2.carriers]` (`V2-15`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2CarriersConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub trail_diffusion: Option<f64>,
    #[serde(default)]
    pub trail_decay_per_second: Option<f64>,
    #[serde(default = "default_trail_emission_share")]
    pub trail_emission_share: f64,
    #[serde(default)]
    pub trail_energy_density_scale: Option<f64>,
}

impl Default for V2CarriersConfig {
    fn default() -> Self {
        Self {
            enabled: None,
            trail_diffusion: None,
            trail_decay_per_second: None,
            trail_emission_share: default_trail_emission_share(),
            trail_energy_density_scale: None,
        }
    }
}

/// `[v2.visual]` (`V2-16`, `Scope::Presentation`).
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(missing_docs, reason = "units and bounds live in crate::schema")]
pub struct V2VisualConfig {
    #[serde(default)]
    pub enabled: Option<bool>,
}

impl V2Config {
    /// Raw optional toggle for `feature`.
    #[must_use]
    pub const fn raw_enabled(&self, feature: V2Feature) -> Option<bool> {
        match feature {
            V2Feature::Oases => self.oases.enabled,
            V2Feature::Senescence => self.senescence.enabled,
            V2Feature::SurplusEmission => self.emission.surplus_floor.enabled,
            V2Feature::ExcitableRelay => self.emission.excitable.enabled,
            V2Feature::WakeOffset => self.emission.offset.enabled,
            V2Feature::So2Motor => self.motor.enabled,
            V2Feature::So2Budding => self.budding.enabled,
            V2Feature::HippoCrowding => self.crowding.enabled,
            V2Feature::PcpPolarity => self.polarity.enabled,
            V2Feature::EmtGate => self.polarity.emt_gate_enabled,
            V2Feature::TightJunctions => self.barrier.enabled,
            V2Feature::LysisRecycling => self.metabolism.lysis.enabled,
            V2Feature::SymplecticWaste => self.metabolism.waste.enabled,
            V2Feature::EpigeneticRecurrence => self.differentiation.enabled,
            V2Feature::NotchDelta => self.differentiation.notch_delta.enabled,
            V2Feature::Trophallaxis => self.trophallaxis.enabled,
            V2Feature::Cadherin4d => self.contact.cadherin.enabled,
            V2Feature::VelocityAlignment => self.contact.velocity_alignment.enabled,
            V2Feature::VascularField => self.vascular.enabled,
            V2Feature::DualCarrierTrails => self.carriers.enabled,
            V2Feature::V2VisualMappings => self.visual.enabled,
        }
    }

    /// Mutable slot for `feature`'s optional toggle.
    pub const fn raw_enabled_mut(&mut self, feature: V2Feature) -> &mut Option<bool> {
        match feature {
            V2Feature::Oases => &mut self.oases.enabled,
            V2Feature::Senescence => &mut self.senescence.enabled,
            V2Feature::SurplusEmission => &mut self.emission.surplus_floor.enabled,
            V2Feature::ExcitableRelay => &mut self.emission.excitable.enabled,
            V2Feature::WakeOffset => &mut self.emission.offset.enabled,
            V2Feature::So2Motor => &mut self.motor.enabled,
            V2Feature::So2Budding => &mut self.budding.enabled,
            V2Feature::HippoCrowding => &mut self.crowding.enabled,
            V2Feature::PcpPolarity => &mut self.polarity.enabled,
            V2Feature::EmtGate => &mut self.polarity.emt_gate_enabled,
            V2Feature::TightJunctions => &mut self.barrier.enabled,
            V2Feature::LysisRecycling => &mut self.metabolism.lysis.enabled,
            V2Feature::SymplecticWaste => &mut self.metabolism.waste.enabled,
            V2Feature::EpigeneticRecurrence => &mut self.differentiation.enabled,
            V2Feature::NotchDelta => &mut self.differentiation.notch_delta.enabled,
            V2Feature::Trophallaxis => &mut self.trophallaxis.enabled,
            V2Feature::Cadherin4d => &mut self.contact.cadherin.enabled,
            V2Feature::VelocityAlignment => &mut self.contact.velocity_alignment.enabled,
            V2Feature::VascularField => &mut self.vascular.enabled,
            V2Feature::DualCarrierTrails => &mut self.carriers.enabled,
            V2Feature::V2VisualMappings => &mut self.visual.enabled,
        }
    }

    /// Whether `feature` is materialized as enabled (returns `false` if unset).
    #[must_use]
    pub const fn is_enabled(&self, feature: V2Feature) -> bool {
        matches!(self.raw_enabled(feature), Some(true))
    }

    /// Set `feature`'s explicit toggle.
    pub fn set_enabled(&mut self, feature: V2Feature, enabled: bool) {
        *self.raw_enabled_mut(feature) = Some(enabled);
    }

    /// Bitmask of all enabled features (bits `0..=20` per [`V2Feature::bit`]).
    #[must_use]
    pub fn feature_bitmask(&self) -> u64 {
        let mut mask = 0_u64;
        for feature in V2Feature::ALL {
            if self.is_enabled(feature) {
                mask |= feature.bit();
            }
        }
        mask
    }

    /// Enabled features in canonical bit order.
    #[must_use]
    pub fn enabled_features(&self) -> Vec<V2Feature> {
        V2Feature::ALL
            .into_iter()
            .filter(|&feature| self.is_enabled(feature))
            .collect()
    }

    /// Enabled scientific (`Scope::Model`) feature slugs in ascending lexicographic order.
    #[must_use]
    pub fn enabled_scientific_slugs_sorted(&self) -> Vec<&'static str> {
        let mut slugs: Vec<&'static str> = V2Feature::ALL
            .into_iter()
            .filter(|&feature| feature.scope() == Scope::Model && self.is_enabled(feature))
            .map(V2Feature::slug)
            .collect();
        slugs.sort_unstable();
        slugs
    }

    /// Whether any active feature requires direct organism-to-organism contact.
    #[must_use]
    pub fn any_contact_feature_active(&self) -> bool {
        if V2Feature::ALL
            .into_iter()
            .any(|feature| feature.requires_contact_unconditionally() && self.is_enabled(feature))
        {
            return true;
        }
        self.is_enabled(V2Feature::HippoCrowding) && self.crowding.mode == "local-neighbors-v1"
    }

    /// Whether the world satisfies `INV-08` (purely field-mediated organism interaction).
    #[must_use]
    pub fn inv08_pure_field(&self, has_contact: bool) -> bool {
        !has_contact && !self.any_contact_feature_active()
    }

    /// Compute the pure scientific [`V2Abi`] for this materialized `V2Config` and `controller`.
    #[must_use]
    pub fn abi(&self, controller: &ControllerConfig) -> V2Abi {
        let dual_carrier_sensors = self.is_enabled(V2Feature::DualCarrierTrails);
        let hippo_crowding_sensor =
            self.is_enabled(V2Feature::HippoCrowding) && self.crowding.expose_sensor;
        let notch_delta_sensor = self.is_enabled(V2Feature::NotchDelta)
            && self.differentiation.notch_delta.mode == "sensor-neighbor-mean-v1";

        let sensor_width = SENSOR_WIDTH
            + if dual_carrier_sensors { 4 } else { 0 }
            + u32::from(hippo_crowding_sensor)
            + u32::from(notch_delta_sensor);

        let recurrent_state_width = if self.is_enabled(V2Feature::EpigeneticRecurrence) {
            self.differentiation.state_width
        } else if controller.family == "independent-recurrent-mlp-v1" {
            controller.recurrent_state_width.unwrap_or(0)
        } else {
            0
        };

        let so2_motor_actions = self.is_enabled(V2Feature::So2Motor);
        let so2_budding_actions = self.is_enabled(V2Feature::So2Budding);
        let emt_gate_action = self.is_enabled(V2Feature::EmtGate);
        let wake_pump_action = self.is_enabled(V2Feature::WakeOffset)
            && self.emission.offset.mode == "polarized-pump-v1"
            && self.emission.offset.pump_direction_mode == "neural-output-v1";

        let action_width = ACTION_WIDTH
            + if so2_motor_actions { 4 } else { 0 }
            + if so2_budding_actions { 2 } else { 0 }
            + u32::from(emt_gate_action)
            + u32::from(wake_pump_action);

        let field_planes = if dual_carrier_sensors {
            DUAL_CARRIER_PLANES
        } else {
            SINGLE_CARRIER_PLANES
        };

        V2Abi {
            sensor_width,
            recurrent_state_width,
            input_width: sensor_width.saturating_add(recurrent_state_width),
            action_width,
            raw_output_width: action_width.saturating_add(recurrent_state_width.saturating_mul(2)),
            field_planes,
            dual_carrier_sensors,
            hippo_crowding_sensor,
            notch_delta_sensor,
            so2_motor_actions,
            so2_budding_actions,
            emt_gate_action,
            wake_pump_action,
        }
    }
}

/// Apply founding CLI/programmatic feature toggles onto `config` before
/// [`crate::validate::validate`].
///
/// Refuses any toggle against a `field-ecology-v1*` model so v1 configs never
/// silently ignore or mutate flags.
///
/// # Errors
/// [`CoreError::InvalidConfig`] when toggles are passed for a non-v2 model or
/// name an unknown slug/alias.
pub fn apply_feature_overrides(
    config: &mut RunConfig,
    overrides: &[FeatureOverride],
) -> Result<(), CoreError> {
    if overrides.is_empty() {
        return Ok(());
    }
    if !is_v2_model(&config.model) {
        return Err(CoreError::InvalidConfig(vec![ConfigIssue {
            path: "v2".into(),
            problem: format!(
                "v2 feature toggles (--disable/--enable/--v1-compat) require a field-ecology-v2* \
                 model, not `{}`",
                config.model
            ),
        }]));
    }
    let has_contact = config.contact.is_some();
    let v2 = config.v2.get_or_insert_with(V2Config::default);
    let profile = V2Profile::parse(&v2.profile).ok_or_else(|| {
        CoreError::InvalidConfig(vec![ConfigIssue {
            path: "v2.profile".into(),
            problem: format!(
                "unknown v2.profile `{}`; expected auto, tissue, supercolony, all, or minimal",
                v2.profile
            ),
        }])
    })?;

    // First materialize profile + TOML toggles so CLI overrides layer cleanly
    // in exact command-line order (e.g., `--disable all-v2 --enable so2-motor`).
    let mut issues = Vec::new();
    resolve_toggles(v2, profile, has_contact, &mut issues);
    if !issues.is_empty() {
        return Err(CoreError::InvalidConfig(issues));
    }

    for item in overrides {
        match item {
            FeatureOverride::V1Compat => {
                for feature in V2Feature::ALL {
                    v2.set_enabled(feature, false);
                }
            }
            FeatureOverride::Disable(raw) => {
                for token in raw.split(',').map(str::trim) {
                    if token.is_empty() {
                        issues.push(ConfigIssue {
                            path: "v2.disabled_features".into(),
                            problem: "empty v2 feature slug or alias in override list".into(),
                        });
                        continue;
                    }
                    let Some(expanded) = V2Feature::expand_slug_or_alias(token) else {
                        issues.push(ConfigIssue {
                            path: "v2.disabled_features".into(),
                            problem: format!("unknown v2 feature slug or alias `{token}`"),
                        });
                        continue;
                    };
                    for &feature in expanded {
                        v2.set_enabled(feature, false);
                    }
                }
            }
            FeatureOverride::Enable(raw) => {
                for token in raw.split(',').map(str::trim) {
                    if token.is_empty() {
                        issues.push(ConfigIssue {
                            path: "v2.enabled_features".into(),
                            problem: "empty v2 feature slug or alias in override list".into(),
                        });
                        continue;
                    }
                    let Some(expanded) = V2Feature::expand_slug_or_alias(token) else {
                        issues.push(ConfigIssue {
                            path: "v2.enabled_features".into(),
                            problem: format!("unknown v2 feature slug or alias `{token}`"),
                        });
                        continue;
                    };
                    for &feature in expanded {
                        v2.set_enabled(feature, true);
                    }
                }
            }
        }
    }
    if !issues.is_empty() {
        return Err(CoreError::InvalidConfig(issues));
    }
    Ok(())
}

/// Resolve profile defaults + TOML sub-table toggles + `disabled_features` /
/// `enabled_features` into explicit `Some(bool)` on every feature, then clear
/// the convenience lists so canonical TOML has one unique form.
fn resolve_toggles(
    v2: &mut V2Config,
    profile: V2Profile,
    has_contact: bool,
    issues: &mut Vec<ConfigIssue>,
) {
    for feature in V2Feature::ALL {
        if v2.raw_enabled(feature).is_none() {
            v2.set_enabled(feature, profile.default_enabled(feature, has_contact));
        }
    }
    let disabled = core::mem::take(&mut v2.disabled_features);
    for token in disabled {
        match V2Feature::expand_slug_or_alias(token.trim()) {
            Some(expanded) => {
                for &feature in expanded {
                    v2.set_enabled(feature, false);
                }
            }
            None => issues.push(ConfigIssue {
                path: "v2.disabled_features".into(),
                problem: format!("unknown v2 feature slug or alias `{token}`"),
            }),
        }
    }
    let enabled = core::mem::take(&mut v2.enabled_features);
    for token in enabled {
        match V2Feature::expand_slug_or_alias(token.trim()) {
            Some(expanded) => {
                for &feature in expanded {
                    v2.set_enabled(feature, true);
                }
            }
            None => issues.push(ConfigIssue {
                path: "v2.enabled_features".into(),
                problem: format!("unknown v2 feature slug or alias `{token}`"),
            }),
        }
    }
}

/// Materialize `config.v2` for `field-ecology-v2*` models before generic
/// schema checks, or record an issue if `[v2]` appears on `field-ecology-v1*`.
pub(crate) fn prepare_v2(config: &mut RunConfig, issues: &mut Vec<ConfigIssue>) {
    if !is_v2_model(&config.model) {
        if config.v2.is_some() {
            issues.push(ConfigIssue {
                path: "v2".into(),
                problem: format!(
                    "[v2] belongs to a field-ecology-v2 model, not {}",
                    config.model
                ),
            });
        }
        return;
    }
    let has_contact = config.contact.is_some();
    let v2 = config.v2.get_or_insert_with(V2Config::default);
    let profile = match V2Profile::parse(&v2.profile) {
        Some(profile) => profile,
        None => {
            issues.push(ConfigIssue {
                path: "v2.profile".into(),
                problem: format!(
                    "unknown v2.profile `{}`; expected auto, tissue, supercolony, all, or minimal",
                    v2.profile
                ),
            });
            V2Profile::Auto
        }
    };
    resolve_toggles(v2, profile, has_contact, issues);

    // Materialize dependent parameter defaults cleanly so canonical TOML holds
    // every concrete value and round-trips idempotently.
    let source_sigma =
        if config.sources.sigma_world_units.is_finite() && config.sources.sigma_world_units > 0.0 {
            config.sources.sigma_world_units
        } else {
            1.0
        };
    if v2.oases.sigma_min_world_units.is_none() {
        v2.oases.sigma_min_world_units = Some(source_sigma);
    }
    if v2.oases.sigma_max_world_units.is_none() {
        v2.oases.sigma_max_world_units = Some(
            v2.oases
                .sigma_min_world_units
                .unwrap_or(source_sigma)
                .max(source_sigma),
        );
    }

    if v2.emission.surplus_floor.energy_floor.is_none() {
        let min_attempt = config.reproduction.minimum_parent_energy_before_attempt;
        let floor = if min_attempt.is_finite() && min_attempt > 0.0 {
            0.25 * min_attempt
        } else {
            0.0
        };
        v2.emission.surplus_floor.energy_floor = Some(floor);
    }

    let min_cell = if config.world.width.is_finite()
        && config.world.width > 0.0
        && config.world.height.is_finite()
        && config.world.height > 0.0
        && config.world.grid_width > 0
        && config.world.grid_height > 0
    {
        (config.world.width / f64::from(config.world.grid_width))
            .min(config.world.height / f64::from(config.world.grid_height))
    } else {
        0.25
    };
    if v2.emission.offset.distance_world_units.is_none() {
        v2.emission.offset.distance_world_units = Some(0.5 * min_cell);
    }

    if v2.budding.local_offset_radius.is_none() {
        let r_max = config.reproduction.child_position_offset_radius;
        let default_r = if r_max.is_finite() && r_max > 0.0 {
            0.25_f64.min(0.5 * r_max)
        } else {
            0.0
        };
        v2.budding.local_offset_radius = Some(default_r);
    }

    if v2.crowding.radius_world_units.is_none() {
        let default_radius = config
            .contact
            .as_ref()
            .filter(|c| c.diameter.is_finite() && c.diameter > 0.0)
            .map_or(0.375, |c| 1.5 * c.diameter);
        v2.crowding.radius_world_units = Some(default_radius);
    }

    let viable_cap = config
        .reproduction
        .minimum_child_energy
        .min(config.reproduction.minimum_parent_energy_after_split);
    if v2.metabolism.lysis.viability_energy_floor.is_none() {
        let floor = if viable_cap.is_finite() && viable_cap > 0.0 {
            0.10_f64.min(0.5 * viable_cap)
        } else {
            0.05
        };
        v2.metabolism.lysis.viability_energy_floor = Some(floor);
    }

    if v2.trophallaxis.reserve_energy.is_none() {
        let tiny = if config.world.tiny_energy_threshold.is_finite()
            && config.world.tiny_energy_threshold >= 0.0
        {
            config.world.tiny_energy_threshold
        } else {
            0.0
        };
        let lysis = v2
            .metabolism
            .lysis
            .viability_energy_floor
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(0.0);
        v2.trophallaxis.reserve_energy = Some(0.25_f64.max(tiny * 2.0).max(lysis * 2.0));
    }

    if v2.vascular.maximum_diffusion.is_none() {
        let base_d = if config.world.diffusion.is_finite() && config.world.diffusion > 0.0 {
            config.world.diffusion
        } else {
            0.03
        };
        v2.vascular.maximum_diffusion = Some((10.0 * base_d).max(0.01));
    }

    if v2.carriers.trail_diffusion.is_none() {
        let base_d = if config.world.diffusion.is_finite() && config.world.diffusion >= 0.0 {
            config.world.diffusion
        } else {
            0.0
        };
        v2.carriers.trail_diffusion = Some(0.01 * base_d);
    }
    if v2.carriers.trail_decay_per_second.is_none() {
        let base_l =
            if config.world.decay_per_second.is_finite() && config.world.decay_per_second >= 0.0 {
                config.world.decay_per_second
            } else {
                0.0
            };
        v2.carriers.trail_decay_per_second = Some(0.05 * base_l);
    }
    if v2.carriers.trail_energy_density_scale.is_none() {
        let base_s = if config.sensors.energy_density_scale.is_finite()
            && config.sensors.energy_density_scale > 0.0
        {
            config.sensors.energy_density_scale
        } else {
            1.0
        };
        v2.carriers.trail_energy_density_scale = Some(base_s);
    }
}

/// Validate cross-key and feature-dependency rules on a materialized `V2Config`.
#[expect(
    clippy::too_many_lines,
    reason = "single auditable checklist for all 21 v2 feature contracts"
)]
pub(crate) fn validate_v2(config: &RunConfig, issues: &mut Vec<ConfigIssue>) {
    if !is_v2_model(&config.model) {
        return;
    }
    let Some(v2) = config.v2.as_ref() else {
        return;
    };
    let has_contact = config.contact.is_some();

    // 1. Contact-feature dependency validation (INV-08 honesty).
    if !has_contact {
        let contact_checks = [
            (V2Feature::PcpPolarity, "v2.polarity.enabled"),
            (V2Feature::EmtGate, "v2.polarity.emt_gate_enabled"),
            (
                V2Feature::NotchDelta,
                "v2.differentiation.notch_delta.enabled",
            ),
            (V2Feature::Trophallaxis, "v2.trophallaxis.enabled"),
            (V2Feature::Cadherin4d, "v2.contact.cadherin.enabled"),
            (
                V2Feature::VelocityAlignment,
                "v2.contact.velocity_alignment.enabled",
            ),
        ];
        for (feature, path) in contact_checks {
            if v2.is_enabled(feature) {
                issues.push(ConfigIssue {
                    path: path.into(),
                    problem: format!(
                        "`{}` requires a [contact] table (direct organism interaction violates INV-08 pure-field)",
                        feature.slug()
                    ),
                });
            }
        }
        if v2.is_enabled(V2Feature::HippoCrowding) && v2.crowding.mode == "local-neighbors-v1" {
            issues.push(ConfigIssue {
                path: "v2.crowding.mode".into(),
                problem: "`hippo-crowding` with mode `local-neighbors-v1` requires a [contact] \
                          table; use mode `cell-occupancy-v1` for pure-field INV-08"
                    .into(),
            });
        }
    }

    // 2. Notch-Delta requires epigenetic-recurrence with state_width >= 1.
    if v2.is_enabled(V2Feature::NotchDelta)
        && (!v2.is_enabled(V2Feature::EpigeneticRecurrence) || v2.differentiation.state_width == 0)
    {
        issues.push(ConfigIssue {
            path: "v2.differentiation.notch_delta.enabled".into(),
            problem: "`notch-delta` requires `epigenetic-recurrence` enabled with state_width >= 1"
                .into(),
        });
    }

    // 3. Mode strings & module bounds.
    if let (Some(s_min), Some(s_max)) = (
        v2.oases.sigma_min_world_units,
        v2.oases.sigma_max_world_units,
    ) && s_min.is_finite()
        && s_max.is_finite()
        && s_min > s_max
    {
        issues.push(ConfigIssue {
            path: "v2.oases.sigma_min_world_units".into(),
            problem: format!("must be <= sigma_max_world_units ({s_max:?}), got {s_min:?}"),
        });
    }
    if v2.is_enabled(V2Feature::Oases)
        && config.sources.count > 0
        && config.sources.count < 3
        && v2.oases.minimum_power_fraction == 0.0
    {
        issues.push(ConfigIssue {
            path: "v2.oases.minimum_power_fraction".into(),
            problem: format!(
                "oases with sources.count = {} (< 3) and minimum_power_fraction = 0.0 can hit \
                 0/0 = NaN at phase zeroes; use sources.count >= 3 or minimum_power_fraction > 0",
                config.sources.count
            ),
        });
    }

    if v2.senescence.maximum_multiplier.is_finite() && v2.senescence.maximum_multiplier < 1.0 {
        issues.push(ConfigIssue {
            path: "v2.senescence.maximum_multiplier".into(),
            problem: format!("must be >= 1.0, got {:?}", v2.senescence.maximum_multiplier),
        });
    }

    if let Some(floor) = v2.emission.surplus_floor.energy_floor
        && v2.is_enabled(V2Feature::SurplusEmission)
        && floor.is_finite()
        && config
            .reproduction
            .minimum_parent_energy_before_attempt
            .is_finite()
        && floor >= config.reproduction.minimum_parent_energy_before_attempt
    {
        issues.push(ConfigIssue {
            path: "v2.emission.surplus_floor.energy_floor".into(),
            problem: format!(
                "must be < reproduction.minimum_parent_energy_before_attempt ({:?}), got {floor:?}",
                config.reproduction.minimum_parent_energy_before_attempt
            ),
        });
    }

    if v2.emission.excitable.burst_rate_multiplier.is_finite()
        && v2.emission.excitable.burst_rate_multiplier < 1.0
    {
        issues.push(ConfigIssue {
            path: "v2.emission.excitable.burst_rate_multiplier".into(),
            problem: format!(
                "must be >= 1.0, got {:?}",
                v2.emission.excitable.burst_rate_multiplier
            ),
        });
    }

    if v2.emission.offset.pump_sign != 1.0 && v2.emission.offset.pump_sign != -1.0 {
        issues.push(ConfigIssue {
            path: "v2.emission.offset.pump_sign".into(),
            problem: format!(
                "must be +1.0 or -1.0, got {:?}",
                v2.emission.offset.pump_sign
            ),
        });
    }
    if let Some(delta) = v2.emission.offset.distance_world_units
        && v2.is_enabled(V2Feature::WakeOffset)
        && delta.is_finite()
        && config.world.width.is_finite()
        && config.world.width > 0.0
        && config.world.height.is_finite()
        && config.world.height > 0.0
        && config.world.grid_width > 0
        && config.world.grid_height > 0
    {
        let min_cell = (config.world.width / f64::from(config.world.grid_width))
            .min(config.world.height / f64::from(config.world.grid_height));
        if delta > min_cell {
            issues.push(ConfigIssue {
                path: "v2.emission.offset.distance_world_units".into(),
                problem: format!("must be <= min(h_x, h_y) ({min_cell:?}), got {delta:?}"),
            });
        }
    }

    if let Some(local_r) = v2.budding.local_offset_radius
        && v2.is_enabled(V2Feature::So2Budding)
        && local_r.is_finite()
    {
        let max_r = config.reproduction.child_position_offset_radius;
        if max_r > 0.0 && (local_r <= 0.0 || local_r > max_r) {
            issues.push(ConfigIssue {
                path: "v2.budding.local_offset_radius".into(),
                problem: format!(
                    "must lie in (0, reproduction.child_position_offset_radius={max_r:?}], got \
                     {local_r:?}"
                ),
            });
        } else if max_r == 0.0 && local_r != 0.0 {
            issues.push(ConfigIssue {
                path: "v2.budding.local_offset_radius".into(),
                problem: format!(
                    "must be 0.0 when reproduction.child_position_offset_radius is 0.0, got \
                     {local_r:?}"
                ),
            });
        }
    }

    if v2.crowding.arrest_threshold.is_finite() && v2.crowding.arrest_threshold < 2.0 {
        issues.push(ConfigIssue {
            path: "v2.crowding.arrest_threshold".into(),
            problem: format!(
                "must be >= 2.0 to avoid stalling 1-cell sheets, got {:?}",
                v2.crowding.arrest_threshold
            ),
        });
    }

    if v2.polarity.maximum_wedge_curvature.is_finite()
        && !(0.0..=0.4).contains(&v2.polarity.maximum_wedge_curvature)
    {
        issues.push(ConfigIssue {
            path: "v2.polarity.maximum_wedge_curvature".into(),
            problem: format!(
                "must be in [0.0, 0.4], got {:?}",
                v2.polarity.maximum_wedge_curvature
            ),
        });
    }

    if let Some(floor) = v2.metabolism.lysis.viability_energy_floor
        && v2.is_enabled(V2Feature::LysisRecycling)
        && floor.is_finite()
    {
        let limit = config
            .reproduction
            .minimum_child_energy
            .min(config.reproduction.minimum_parent_energy_after_split);
        if floor <= 0.0 || floor >= limit {
            issues.push(ConfigIssue {
                path: "v2.metabolism.lysis.viability_energy_floor".into(),
                problem: format!(
                    "must satisfy 0 < floor < min(minimum_child_energy, \
                     minimum_parent_energy_after_split) ({limit:?}), got {floor:?}"
                ),
            });
        }
    }

    if v2.metabolism.waste.byproduct_recycle_fraction.is_finite()
        && !(0.0..=0.65).contains(&v2.metabolism.waste.byproduct_recycle_fraction)
    {
        issues.push(ConfigIssue {
            path: "v2.metabolism.waste.byproduct_recycle_fraction".into(),
            problem: format!(
                "must be in [0.0, 0.65], got {:?}",
                v2.metabolism.waste.byproduct_recycle_fraction
            ),
        });
    }

    if v2.differentiation.state_width == 0
        || v2.differentiation.state_width > MAXIMUM_RECURRENT_STATE_WIDTH
    {
        issues.push(ConfigIssue {
            path: "v2.differentiation.state_width".into(),
            problem: format!(
                "must lie in 1..={MAXIMUM_RECURRENT_STATE_WIDTH}, got {}",
                v2.differentiation.state_width
            ),
        });
    }
    if v2.is_enabled(V2Feature::EpigeneticRecurrence)
        && let Some(ctrl_width) = config.controller.recurrent_state_width
        && ctrl_width != v2.differentiation.state_width
    {
        issues.push(ConfigIssue {
            path: "v2.differentiation.state_width".into(),
            problem: format!(
                "differs from controller.recurrent_state_width ({ctrl_width} vs {})",
                v2.differentiation.state_width
            ),
        });
    }
    if v2.differentiation.epigenetic_gate_bias.is_finite()
        && !(0.0..=4.0).contains(&v2.differentiation.epigenetic_gate_bias)
    {
        issues.push(ConfigIssue {
            path: "v2.differentiation.epigenetic_gate_bias".into(),
            problem: format!(
                "must be in [0.0, 4.0], got {:?}",
                v2.differentiation.epigenetic_gate_bias
            ),
        });
    }
    if v2.differentiation.stem_asymmetry.is_finite()
        && !(-2.0..=2.0).contains(&v2.differentiation.stem_asymmetry)
    {
        issues.push(ConfigIssue {
            path: "v2.differentiation.stem_asymmetry".into(),
            problem: format!(
                "must be in [-2.0, 2.0], got {:?}",
                v2.differentiation.stem_asymmetry
            ),
        });
    }
    if v2.differentiation.notch_delta.inhibition_gain.is_finite()
        && !(0.0..=0.3).contains(&v2.differentiation.notch_delta.inhibition_gain)
    {
        issues.push(ConfigIssue {
            path: "v2.differentiation.notch_delta.inhibition_gain".into(),
            problem: format!(
                "must be in [0.0, 0.3], got {:?}",
                v2.differentiation.notch_delta.inhibition_gain
            ),
        });
    }

    if v2.trophallaxis.transfer_efficiency.is_finite()
        && !(v2.trophallaxis.transfer_efficiency > 0.0
            && v2.trophallaxis.transfer_efficiency <= 0.99)
    {
        issues.push(ConfigIssue {
            path: "v2.trophallaxis.transfer_efficiency".into(),
            problem: format!(
                "must be in (0.0, 0.99], got {:?}",
                v2.trophallaxis.transfer_efficiency
            ),
        });
    }
    if let Some(reserve) = v2.trophallaxis.reserve_energy
        && v2.is_enabled(V2Feature::Trophallaxis)
        && reserve.is_finite()
    {
        if reserve <= config.world.tiny_energy_threshold {
            issues.push(ConfigIssue {
                path: "v2.trophallaxis.reserve_energy".into(),
                problem: format!(
                    "must be > world.tiny_energy_threshold ({:?}), got {reserve:?}",
                    config.world.tiny_energy_threshold
                ),
            });
        }
        if v2.is_enabled(V2Feature::LysisRecycling)
            && let Some(floor) = v2.metabolism.lysis.viability_energy_floor
            && reserve <= floor
        {
            issues.push(ConfigIssue {
                path: "v2.trophallaxis.reserve_energy".into(),
                problem: format!(
                    "must be > v2.metabolism.lysis.viability_energy_floor ({floor:?}), got \
                     {reserve:?}"
                ),
            });
        }
    }

    if v2.contact.velocity_alignment.strength.is_finite()
        && !(0.0..=0.4).contains(&v2.contact.velocity_alignment.strength)
    {
        issues.push(ConfigIssue {
            path: "v2.contact.velocity_alignment.strength".into(),
            problem: format!(
                "must be in [0.0, 0.4], got {:?}",
                v2.contact.velocity_alignment.strength
            ),
        });
    }

    if v2.vascular.minimum_conductance_ratio.is_finite()
        && !(v2.vascular.minimum_conductance_ratio > 0.01
            && v2.vascular.minimum_conductance_ratio <= 1.0)
    {
        issues.push(ConfigIssue {
            path: "v2.vascular.minimum_conductance_ratio".into(),
            problem: format!(
                "must be in (0.01, 1.0], got {:?}",
                v2.vascular.minimum_conductance_ratio
            ),
        });
    }

    if v2.carriers.trail_emission_share.is_finite()
        && !(v2.carriers.trail_emission_share > 0.0 && v2.carriers.trail_emission_share <= 0.5)
    {
        issues.push(ConfigIssue {
            path: "v2.carriers.trail_emission_share".into(),
            problem: format!(
                "must be in (0.0, 0.5], got {:?}",
                v2.carriers.trail_emission_share
            ),
        });
    }

    // 4. ABI width limits.
    let abi = v2.abi(&config.controller);
    if abi.input_width > MAX_SENSOR_WIDTH {
        issues.push(ConfigIssue {
            path: "v2".into(),
            problem: format!(
                "effective controller input width {} exceeds MAX_SENSOR_WIDTH ({MAX_SENSOR_WIDTH})",
                abi.input_width
            ),
        });
    }
    if abi.action_width > MAX_ACTION_WIDTH {
        issues.push(ConfigIssue {
            path: "v2".into(),
            problem: format!(
                "effective action width {} exceeds MAX_ACTION_WIDTH ({MAX_ACTION_WIDTH})",
                abi.action_width
            ),
        });
    }
    if abi.raw_output_width > MAX_CONTROLLER_OUTPUT_WIDTH {
        issues.push(ConfigIssue {
            path: "v2".into(),
            problem: format!(
                "effective raw output width {} exceeds MAX_CONTROLLER_OUTPUT_WIDTH \
                 ({MAX_CONTROLLER_OUTPUT_WIDTH})",
                abi.raw_output_width
            ),
        });
    }
}

/// Validate a `v2.*` text field value in `validate::check_text`.
#[must_use]
pub(crate) fn validate_v2_text(path: &str, text: &str) -> Option<String> {
    let valid = match path {
        "v2.profile" => V2Profile::parse(text).is_some(),
        "v2.emission.excitable.mode" => {
            matches!(text, "vesicle-readiness-v1" | "tick-cooldown-v1")
        }
        "v2.emission.offset.mode" => matches!(text, "wake-velocity-v1" | "polarized-pump-v1"),
        "v2.emission.offset.pump_direction_mode" => {
            matches!(text, "fixed-sign-v1" | "neural-output-v1")
        }
        "v2.crowding.mode" => matches!(text, "local-neighbors-v1" | "cell-occupancy-v1"),
        "v2.barrier.mode" => matches!(text, "harmonic-v1" | "min-v1"),
        "v2.metabolism.lysis.corpse_direction" => matches!(
            text,
            "negated-affinity" | "symplectic-affinity" | "genetic-affinity"
        ),
        "v2.differentiation.state_inheritance" => matches!(
            text,
            "zero" | "symmetric-epigenetic-v1" | "asymmetric-stem-v1"
        ),
        "v2.differentiation.notch_delta.mode" => {
            matches!(
                text,
                "sensor-neighbor-mean-v1" | "direct-lateral-inhibition-v1"
            )
        }
        _ => return Some("has no text format rule".to_owned()),
    };
    if valid {
        None
    } else {
        Some(format!("unsupported v2 mode/value `{text}`"))
    }
}
