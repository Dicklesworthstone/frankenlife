//! The `frankenlife.config.v1` run configuration (plan Appendix A).
//!
//! Every key is required and unknown keys are refused, so the effective
//! configuration is exactly what the file says. There are no hidden
//! defaults to canonicalize. A parsed [`RunConfig`] is untrusted until
//! [`validate`](crate::validate::validate) turns it into a
//! [`ValidatedConfig`](crate::validate::ValidatedConfig). Units, bounds and
//! the algorithm role of each key are declared once, in [`crate::schema`].

use serde::Deserialize;

use crate::error::{ConfigIssue, CoreError};

/// The configuration schema this build reads.
pub const CONFIG_SCHEMA: &str = "frankenlife.config.v1";

/// A complete run configuration, as parsed.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunConfig {
    /// Must equal [`CONFIG_SCHEMA`].
    pub schema: String,
    /// A short run-profile name (`[A-Za-z0-9._-]`, at most 64 bytes).
    pub name: String,
    /// The model identity (a registry name).
    pub model: String,
    /// The world seed.
    pub seed: u64,
    /// `[world]`
    pub world: WorldConfig,
    /// `[population]`
    pub population: PopulationConfig,
    /// `[field_initialization]`
    pub field_initialization: FieldInitializationConfig,
    /// `[sources]`
    pub sources: SourcesConfig,
    /// `[chemistry]`
    pub chemistry: ChemistryConfig,
    /// `[sensors]`
    pub sensors: SensorsConfig,
    /// `[controller]`
    pub controller: ControllerConfig,
    /// `[architecture_mutation]`
    pub architecture_mutation: ArchitectureMutationConfig,
    /// `[mutation]`
    pub mutation: MutationConfig,
    /// `[costs]`
    pub costs: CostsConfig,
    /// `[emission]`
    pub emission: EmissionConfig,
    /// `[reproduction]`
    pub reproduction: ReproductionConfig,
    /// `[death]`
    pub death: DeathConfig,
    /// `[execution]`
    pub execution: ExecutionConfig,
    /// `[presentation]`
    pub presentation: PresentationConfig,
    /// `[recording]`
    pub recording: RecordingConfig,
    /// `[persistence]`
    pub persistence: PersistenceConfig,
}

impl RunConfig {
    /// Parse TOML. Unknown and missing keys are refused here; values are
    /// checked by [`validate`](crate::validate::validate).
    ///
    /// # Errors
    /// [`CoreError::InvalidConfig`] with one `toml` issue.
    pub fn from_toml(text: &str) -> Result<Self, CoreError> {
        toml::from_str(text).map_err(|error| {
            CoreError::InvalidConfig(vec![ConfigIssue {
                path: "toml".into(),
                problem: error.to_string().trim().to_owned(),
            }])
        })
    }
}

/// `[world]` (plan §4.1–§4.5). Units and bounds: [`crate::schema`].
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    missing_docs,
    reason = "fields mirror Appendix A; units and bounds live in crate::schema"
)]
pub struct WorldConfig {
    pub width: f64,
    pub height: f64,
    pub grid_width: u32,
    pub grid_height: u32,
    pub boundary: String,
    pub moment_channels: u32,
    pub field_dtype: String,
    pub tick_numerator: u32,
    pub tick_denominator: u32,
    pub transport_scheme: String,
    pub diffusion: f64,
    pub decay_per_second: f64,
    pub stability_policy: String,
    pub exchange_kernel: String,
    pub tiny_energy_policy: String,
    pub tiny_energy_threshold: f64,
}

/// `[population]` (plan §7, A.3 item 1).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    missing_docs,
    reason = "fields mirror Appendix A; units and bounds live in crate::schema"
)]
pub struct PopulationConfig {
    pub initial_count: u32,
    pub maximum_count: u32,
    pub initial_energy: f64,
    pub initial_velocity: String,
    pub initial_positions: String,
    pub capacity_policy: String,
    pub automatic_reseed: bool,
}

/// `[field_initialization]` (plan §4.2).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    missing_docs,
    reason = "fields mirror Appendix A; units and bounds live in crate::schema"
)]
pub struct FieldInitializationConfig {
    pub energy_per_cell: f64,
    pub composition: String,
}

/// `[sources]` (plan §4.5, A.3 items 7–9).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    missing_docs,
    reason = "fields mirror Appendix A; units and bounds live in crate::schema"
)]
pub struct SourcesConfig {
    pub count: u32,
    pub power_per_source: f64,
    pub sigma_world_units: f64,
    pub spatial_normalization: String,
    pub kernel: String,
    pub kernel_cutoff_sigma: f64,
    pub chemical_directions: String,
    pub chemical_direction_policy: String,
    pub path: String,
    pub path_speed: f64,
    pub path_direction_interval_seconds: f64,
}

/// `[chemistry]` (plan §5.3–§5.4).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    missing_docs,
    reason = "fields mirror Appendix A; units and bounds live in crate::schema"
)]
pub struct ChemistryConfig {
    pub family: String,
    pub heritable_traits: Vec<String>,
    pub maximum_efficiency: f64,
    pub affinity_norm_limit: f64,
    pub affinity_gain: f64,
    pub discrimination_threshold: f64,
    pub uptake_capacity_per_second: f64,
    pub uptake_half_saturation_density: f64,
    pub toxicity_sensitivity: f64,
    pub chemistry_evaluation: String,
    pub motion_response_norm_limit: f64,
    pub maximum_acceleration: f64,
    pub drag_per_second: f64,
    pub maximum_speed: f64,
}

/// `[sensors]` (plan §6.1, A.1).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    missing_docs,
    reason = "fields mirror Appendix A; units and bounds live in crate::schema"
)]
pub struct SensorsConfig {
    pub schema: String,
    pub normalization: String,
    pub energy_density_scale: f64,
    pub gradient_length_scale_world_units: f64,
    pub composition_energy_floor_density: f64,
    pub internal_energy_scale: f64,
    pub age_scale_seconds: f64,
    pub velocity_scale: f64,
    pub energy_change_scale_per_second: f64,
    pub initial_scale_world_units: f64,
    pub minimum_scale_world_units: f64,
    pub maximum_scale_world_units: f64,
    pub pyramid_filter: String,
    pub pyramid_scale_blend: String,
}

/// `[controller]` (plan §6.2–§6.6).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    missing_docs,
    reason = "fields mirror Appendix A; units and bounds live in crate::schema"
)]
pub struct ControllerConfig {
    pub family: String,
    pub input_width: u32,
    pub hidden_widths: Vec<u32>,
    pub output_width: u32,
    pub hidden_activation: String,
    pub output_activation: String,
    pub canonical_weight_dtype: String,
    pub accumulation_dtype: String,
    pub evaluation_interval_ticks: u32,
    pub phase_policy: String,
    pub autograd: bool,
    pub initializer: String,
    pub maximum_absolute_parameter: f64,
}

/// `[architecture_mutation]` (plan §6.4, A.3 items 4–6).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    missing_docs,
    reason = "fields mirror Appendix A; units and bounds live in crate::schema"
)]
pub struct ArchitectureMutationConfig {
    pub enabled: bool,
    pub catalog: String,
    pub maximum_parameters: u64,
    pub probability_per_birth: f64,
    pub inheritance_map: String,
    pub depth_change_map: String,
    pub target_selection: String,
}

/// `[mutation]` (plan §7.3, A.3 items 3–4).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    missing_docs,
    reason = "fields mirror Appendix A; units and bounds live in crate::schema"
)]
pub struct MutationConfig {
    pub rng: String,
    pub weight_probability: f64,
    pub weight_standard_deviation: f64,
    pub replacement_probability: f64,
    pub chemistry_standard_deviation: f64,
    pub sensing_log_scale_standard_deviation: f64,
    pub reject_effective_noop: bool,
    pub maximum_candidate_proposals_per_birth: u32,
    pub exhausted_proposals_policy: String,
    pub meta_evolution: bool,
}

/// `[costs]` (plan §5.6, A.1).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    missing_docs,
    reason = "fields mirror Appendix A; units and bounds live in crate::schema"
)]
pub struct CostsConfig {
    pub basal_energy_per_second: f64,
    pub sensor_energy_per_second_at_unit_scale: f64,
    pub sensor_unit_scale_world_units: f64,
    pub sensor_scale_cost_exponent: f64,
    pub neural_energy_per_parameter_evaluation: f64,
    pub motion_energy_per_speed_squared_second: f64,
    pub birth_energy: f64,
}

/// `[emission]` (plan §5.5, A.1).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    missing_docs,
    reason = "fields mirror Appendix A; units and bounds live in crate::schema"
)]
pub struct EmissionConfig {
    pub gate_threshold: f64,
    pub maximum_fractional_rate_per_second: f64,
    pub transfer_efficiency: f64,
    pub direction_zero_threshold: f64,
    pub zero_direction_policy: String,
    pub position: String,
    pub visibility: String,
}

/// `[reproduction]` (plan §7.4).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    missing_docs,
    reason = "fields mirror Appendix A; units and bounds live in crate::schema"
)]
pub struct ReproductionConfig {
    pub maximum_attempt_rate_per_second: f64,
    pub minimum_parent_energy_before_attempt: f64,
    pub minimum_child_energy: f64,
    pub minimum_parent_energy_after_split: f64,
    pub minimum_child_fraction: f64,
    pub maximum_child_fraction: f64,
    pub cooldown_seconds: f64,
    pub child_position_offset_radius: f64,
    pub newborn_activation: String,
}

/// `[death]` (plan §7.6).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    missing_docs,
    reason = "fields mirror Appendix A; units and bounds live in crate::schema"
)]
pub struct DeathConfig {
    pub residual_energy_recycle_fraction: f64,
    pub corpse_direction: String,
    pub numerical_fault_policy: String,
}

/// `[execution]` (plan §7.5, §12, §16).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    missing_docs,
    reason = "fields mirror Appendix A; units and bounds live in crate::schema"
)]
pub struct ExecutionConfig {
    pub backend: String,
    pub determinism: String,
    pub math_mode: String,
    pub cpu_workers: u32,
    pub command_queue_capacity: u32,
    pub control_queue_capacity: u32,
    pub command_durability: String,
    pub render_queue_capacity: u32,
    pub maximum_logical_genome_bytes: u64,
    pub maximum_allocated_genome_bytes: u64,
    pub maximum_total_bytes: u64,
    pub checkpoint_retained_bytes: u64,
    pub observer_pressure_policy: String,
}

/// `[presentation]` (plan §13).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    missing_docs,
    reason = "fields mirror Appendix A; units and bounds live in crate::schema"
)]
pub struct PresentationConfig {
    pub engine: String,
    pub width: u32,
    pub height: u32,
    pub frames_per_second: u32,
    pub color_mapping: String,
    pub background: String,
    pub point_radius_pixels: f64,
    pub bloom_enabled: bool,
    pub bloom_strength: f64,
    pub trails_enabled: bool,
    pub interpolation: String,
    pub show_clock_telemetry: bool,
}

/// `[recording]` (plan §13.8).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    missing_docs,
    reason = "fields mirror Appendix A; units and bounds live in crate::schema"
)]
pub struct RecordingConfig {
    pub enabled: bool,
    pub clock: String,
    pub include_provenance: bool,
}

/// `[persistence]` (plan §16).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    missing_docs,
    reason = "fields mirror Appendix A; units and bounds live in crate::schema"
)]
pub struct PersistenceConfig {
    pub catalog: String,
    pub checkpoint_mode: String,
    pub checkpoint_interval_simulation_seconds: f64,
    pub retain_recent_checkpoints: u32,
    pub preserve_pinned_discoveries: bool,
    pub metrics_interval_ticks: u32,
}
