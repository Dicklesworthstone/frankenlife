//! Configuration validation (plan A.2).
//!
//! A syntactically valid file is not yet a valid run. [`validate`] first
//! applies each key's generic rule from [`crate::schema`]: finiteness, bound,
//! TOML-representable integer, registry role and text format. Then it
//! applies the cross-key rules A.2 names: channel counts, sensor and action
//! widths, catalog depth and parameter cap, pyramid levels and grid
//! divisibility, source support, tick alignment, birth viability, impossible
//! strict mutation, memory quotas, and features outside the model. It
//! reports every issue, not just the first. On success the
//! [`ValidationReport`] records the derived quantities A.2 asks for,
//! including the default network's parameter count and the worst admitted
//! live allocation.

use crate::canonical::{Sha256Digest, canonical_toml};
use crate::clock::{TickSpan, TimeStep};
use crate::config::{CONFIG_SCHEMA, RunConfig};
use crate::error::{ConfigIssue, CoreError};
use crate::model::{
    ACTION_WIDTH, INTRINSIC_INPUTS, MAXIMUM_HIDDEN_LAYERS, MOMENT_CHANNELS, SENSOR_WIDTH,
    dense_parameter_count, genome_bank_bytes,
};
use crate::registry::{self, REGISTRY};
use crate::schema::{self, FloatBound, IntBound, Kind, Value};

/// Quantities derived while validating (plan A.2).
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationReport {
    /// The scientific timestep in lowest terms.
    pub timestep: TimeStep,
    /// Field cell width `h_x`, in world length.
    pub cell_width: f64,
    /// Field cell height `h_y`, in world length.
    pub cell_height: f64,
    /// The coarsest sensor pyramid level needed for the maximum sensing
    /// scale; both grid dimensions are divisible by `2^pyramid_levels`.
    pub pyramid_levels: u32,
    /// Reproduction cooldown, `ceil(cooldown_seconds / Δt)` (§7.4).
    pub cooldown_ticks: TickSpan,
    /// Source path segment length, exactly `T / Δt` (A.3 item 8).
    pub source_segment_ticks: TickSpan,
    /// Checkpoint interval, `ceil(seconds / Δt)`.
    pub checkpoint_interval_ticks: TickSpan,
    /// Parameters (weights and biases) of the configured default network.
    pub default_parameter_count: u64,
    /// Logical genome bytes of the initial population.
    pub initial_genome_bytes: u64,
    /// Logical genome bytes with every admitted organism on the default
    /// topology.
    pub default_genome_bytes_at_maximum: u64,
    /// Logical genome bytes with every admitted organism at the parameter
    /// cap: the worst admitted live allocation.
    pub parameter_cap_genome_bytes_at_maximum: u64,
    /// Whether the worst admitted allocation fits the logical genome quota.
    /// When false, the quota and not the population ceiling binds: births
    /// beyond it pause or are refused by admission (§7.5), not by this
    /// validator.
    pub worst_case_fits_logical_quota: bool,
}

/// A configuration that passed [`validate`]. Only this type can be
/// serialized canonically or hashed.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedConfig {
    config: RunConfig,
    report: ValidationReport,
}

impl ValidatedConfig {
    /// Parse and validate TOML.
    ///
    /// # Errors
    /// [`CoreError::InvalidConfig`] listing every issue found.
    pub fn from_toml(text: &str) -> Result<Self, CoreError> {
        validate(RunConfig::from_toml(text)?)
    }

    /// The validated configuration.
    #[must_use]
    pub fn config(&self) -> &RunConfig {
        &self.config
    }

    /// The derived quantities.
    #[must_use]
    pub fn report(&self) -> &ValidationReport {
        &self.report
    }

    /// The canonical TOML: every key in schema order, floats in shortest
    /// round-trip form, `-0.0` written as `0.0`.
    #[must_use]
    pub fn canonical_toml(&self) -> String {
        canonical_toml(&self.config)
    }

    /// SHA-256 of the canonical TOML: the run manifest's configuration hash
    /// (plan B.1).
    #[must_use]
    pub fn config_hash(&self) -> Sha256Digest {
        Sha256Digest::of(self.canonical_toml().as_bytes())
    }
}

#[derive(Default)]
struct Issues(Vec<ConfigIssue>);

impl Issues {
    fn add(&mut self, path: impl Into<String>, problem: impl Into<String>) {
        self.0.push(ConfigIssue {
            path: path.into(),
            problem: problem.into(),
        });
    }
}

const TOML_MAX_INTEGER: u64 = i64::MAX.unsigned_abs();

fn check_generic(config: &RunConfig, issues: &mut Issues) {
    for field in schema::fields(config) {
        let path = field.path();
        match (field.kind, field.value) {
            (Kind::Float(_, bound), Value::Float(value)) => {
                if !value.is_finite() {
                    issues.add(path, format!("must be finite, got {value:?}"));
                    continue;
                }
                let (ok, rule) = match bound {
                    FloatBound::Finite => (true, ""),
                    FloatBound::NonNegative => (value >= 0.0, "must be >= 0"),
                    FloatBound::Positive => (value > 0.0, "must be > 0"),
                    FloatBound::UnitClosed => ((0.0..=1.0).contains(&value), "must be in [0, 1]"),
                    FloatBound::UnitPositive => (value > 0.0 && value <= 1.0, "must be in (0, 1]"),
                };
                if !ok {
                    issues.add(path, format!("{rule}, got {value:?}"));
                }
            }
            (Kind::Int(_, bound), Value::Int(value)) => {
                if value > TOML_MAX_INTEGER {
                    issues.add(path, format!("{value} exceeds the TOML integer range"));
                } else if bound == IntBound::AtLeastOne && value == 0 {
                    issues.add(path, "must be at least 1, got 0");
                } else if bound == IntBound::AtLeastTwo && value < 2 {
                    issues.add(path, format!("must be at least 2, got {value}"));
                }
            }
            (Kind::Name(role), Value::Str(name)) => {
                check_name(role, name, &path, issues);
            }
            (Kind::NameList(role), Value::StrList(names)) => {
                for (index, name) in names.iter().enumerate() {
                    check_name(role, name, &path, issues);
                    if names[..index].contains(name) {
                        issues.add(path.clone(), format!("lists `{name}` twice"));
                    }
                }
            }
            (Kind::IntList(_), Value::IntList(values)) => {
                if values.contains(&0) {
                    issues.add(path, "widths must be at least 1");
                }
            }
            (Kind::Text, Value::Str(text)) => check_text(&path, text, issues),
            _ => {}
        }
    }
}

fn check_name(role: registry::Role, name: &str, path: &str, issues: &mut Issues) {
    if registry::lookup(role, name).is_ok() {
        return;
    }
    let elsewhere: Vec<&str> = REGISTRY
        .iter()
        .filter(|entry| entry.name == name)
        .map(|entry| entry.role.key())
        .collect();
    if elsewhere.is_empty() {
        issues.add(
            path,
            format!("`{name}` is not a registered algorithm or policy"),
        );
    } else {
        issues.add(
            path,
            format!(
                "`{name}` is registered only for {}, not this key",
                elsewhere.join(", ")
            ),
        );
    }
}

fn check_text(path: &str, text: &str, issues: &mut Issues) {
    match path {
        "schema" => {
            if text != CONFIG_SCHEMA {
                issues.add(path, format!("expected `{CONFIG_SCHEMA}`, found `{text}`"));
            }
        }
        "name" => {
            let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
            if text.is_empty() || text.len() > 64 || !text.chars().all(allowed) {
                issues.add(path, "must be 1-64 characters of [A-Za-z0-9._-]");
            }
        }
        "presentation.background" => {
            let hex = text.strip_prefix('#').unwrap_or("");
            let lower_hex = |c: char| c.is_ascii_digit() || ('a'..='f').contains(&c);
            if hex.len() != 6 || !hex.chars().all(lower_hex) {
                issues.add(
                    path,
                    format!("must be `#rrggbb` in lowercase hex, got `{text}`"),
                );
            }
        }
        _ => issues.add(path, "has no text format rule"),
    }
}

/// Validate `config` against plan A.2.
///
/// # Errors
/// [`CoreError::InvalidConfig`] listing every issue found.
pub fn validate(config: RunConfig) -> Result<ValidatedConfig, CoreError> {
    let mut issues = Issues::default();
    check_generic(&config, &mut issues);
    let report = check_model(&config, &mut issues);
    match report {
        Some(report) if issues.0.is_empty() => Ok(ValidatedConfig { config, report }),
        _ => {
            if issues.0.is_empty() {
                issues.add("config", "derived quantities could not be computed");
            }
            Err(CoreError::InvalidConfig(issues.0))
        }
    }
}

/// The cross-key rules. Returns the report when every derived quantity
/// could be computed; issues are recorded either way.
#[expect(
    clippy::too_many_lines,
    reason = "the A.2 rules read best as one ordered checklist"
)]
fn check_model(config: &RunConfig, issues: &mut Issues) -> Option<ValidationReport> {
    let world = &config.world;
    let sensors = &config.sensors;
    let controller = &config.controller;
    let reproduction = &config.reproduction;
    let execution = &config.execution;

    // Channel counts and the fixed sensor/action ABI (§6.1, §6.3).
    if world.moment_channels != MOMENT_CHANNELS {
        issues.add(
            "world.moment_channels",
            format!(
                "field-ecology-v1 has {MOMENT_CHANNELS} channels, got {}",
                world.moment_channels
            ),
        );
    }
    let expected_inputs = u64::from(world.moment_channels)
        .saturating_add(1)
        .saturating_mul(3)
        + u64::from(INTRINSIC_INPUTS);
    if u64::from(controller.input_width) != expected_inputs
        || controller.input_width != SENSOR_WIDTH
    {
        issues.add(
            "controller.input_width",
            format!(
                "the energy-moment-gradient-104-v1 sensors produce {SENSOR_WIDTH} inputs, got {}",
                controller.input_width
            ),
        );
    }
    if controller.output_width != ACTION_WIDTH {
        issues.add(
            "controller.output_width",
            format!(
                "field-ecology-v1 decodes {ACTION_WIDTH} outputs, got {}",
                controller.output_width
            ),
        );
    }
    if controller.hidden_widths.is_empty() || controller.hidden_widths.len() > MAXIMUM_HIDDEN_LAYERS
    {
        issues.add(
            "controller.hidden_widths",
            format!(
                "the dense catalog admits 1 or 2 hidden layers, got {}",
                controller.hidden_widths.len()
            ),
        );
    }
    let default_parameter_count = dense_parameter_count(
        controller.input_width,
        &controller.hidden_widths,
        controller.output_width,
    );
    if let Err(error) = &default_parameter_count {
        issues.add("controller.hidden_widths", error.to_string());
    }
    if let Ok(count) = default_parameter_count
        && count > config.architecture_mutation.maximum_parameters
    {
        issues.add(
            "architecture_mutation.maximum_parameters",
            format!(
                "the default network has {count} parameters, above the cap {}",
                config.architecture_mutation.maximum_parameters
            ),
        );
    }

    // Features the model does not have (§7.5, §1).
    if controller.autograd {
        issues.add(
            "controller.autograd",
            "field-ecology-v1 is never trained by gradient",
        );
    }
    if config.mutation.meta_evolution {
        issues.add(
            "mutation.meta_evolution",
            "mutation-rate evolution is not part of field-ecology-v1",
        );
    }
    if config.population.automatic_reseed {
        issues.add(
            "population.automatic_reseed",
            "the baseline never reseeds after extinction (§7.5)",
        );
    }
    if config.population.initial_count > config.population.maximum_count {
        issues.add(
            "population.initial_count",
            format!(
                "{} exceeds maximum_count {}",
                config.population.initial_count, config.population.maximum_count
            ),
        );
    }

    // Child fractions and birth viability at the minimum attempt energy (§7.4).
    let low_fraction = reproduction.minimum_child_fraction;
    let high_fraction = reproduction.maximum_child_fraction;
    if !(low_fraction > 0.0 && low_fraction <= high_fraction && high_fraction < 1.0) {
        issues.add(
            "reproduction.minimum_child_fraction",
            format!("need 0 < minimum <= maximum < 1, got [{low_fraction:?}, {high_fraction:?}]"),
        );
    } else {
        let available =
            reproduction.minimum_parent_energy_before_attempt - config.costs.birth_energy;
        let viable = available > 0.0 && {
            let low = low_fraction.max(reproduction.minimum_child_energy / available);
            let high =
                high_fraction.min(1.0 - reproduction.minimum_parent_energy_after_split / available);
            low <= high
        };
        if !viable {
            issues.add(
                "reproduction.minimum_parent_energy_before_attempt",
                "no admitted child fraction leaves both a viable child and a viable parent at the \
                 minimum attempt energy after the birth cost",
            );
        }
    }

    // A strict no-op rule with no mutation source can never produce a child (A.2).
    let mutation = &config.mutation;
    let architecture = &config.architecture_mutation;
    let any_mutation = (mutation.weight_probability > 0.0
        && mutation.weight_standard_deviation > 0.0)
        || mutation.replacement_probability > 0.0
        || mutation.chemistry_standard_deviation > 0.0
        || mutation.sensing_log_scale_standard_deviation > 0.0
        || (architecture.enabled && architecture.probability_per_birth > 0.0);
    if mutation.reject_effective_noop && !any_mutation {
        issues.add(
            "mutation.reject_effective_noop",
            "every mutation source is zero, so every candidate is a no-op and every birth fails",
        );
    }
    // Strict mutation tries at most eight candidates per attempted birth (§7.3).
    if mutation.maximum_candidate_proposals_per_birth > 8 {
        issues.add(
            "mutation.maximum_candidate_proposals_per_birth",
            "strict mutation tries at most eight candidate proposals per attempted birth (§7.3)",
        );
    }

    // Memory quotas are simultaneous constraints (§7.5, A.2).
    if execution.maximum_logical_genome_bytes > execution.maximum_allocated_genome_bytes {
        issues.add(
            "execution.maximum_logical_genome_bytes",
            "exceeds maximum_allocated_genome_bytes",
        );
    }
    if execution.maximum_allocated_genome_bytes > execution.maximum_total_bytes {
        issues.add(
            "execution.maximum_allocated_genome_bytes",
            "exceeds maximum_total_bytes",
        );
    }
    if execution.checkpoint_retained_bytes > execution.maximum_total_bytes {
        issues.add(
            "execution.checkpoint_retained_bytes",
            "exceeds maximum_total_bytes",
        );
    }

    // Everything below needs a valid timestep and finite geometry.
    let timestep = match TimeStep::new(world.tick_numerator, world.tick_denominator) {
        Ok(step) => step,
        Err(error) => {
            issues.add("world.tick_numerator", error.to_string());
            return None;
        }
    };
    let geometry_ok = [
        world.width,
        world.height,
        sensors.minimum_scale_world_units,
        sensors.initial_scale_world_units,
        sensors.maximum_scale_world_units,
        config.sources.sigma_world_units,
        config.sources.kernel_cutoff_sigma,
    ]
    .iter()
    .all(|value| value.is_finite() && *value > 0.0)
        && world.grid_width > 0
        && world.grid_height > 0;
    if !geometry_ok {
        return None;
    }
    let cell_width = world.width / f64::from(world.grid_width);
    let cell_height = world.height / f64::from(world.grid_height);

    // Sensing scales and pyramid levels (§6.1).
    if !(sensors.minimum_scale_world_units <= sensors.initial_scale_world_units
        && sensors.initial_scale_world_units <= sensors.maximum_scale_world_units)
    {
        issues.add(
            "sensors.initial_scale_world_units",
            "need minimum <= initial <= maximum sensing scale",
        );
    }
    let finest = cell_width.max(cell_height);
    if sensors.minimum_scale_world_units < finest {
        issues.add(
            "sensors.minimum_scale_world_units",
            format!(
                "{:?} is below the finest pyramid level {finest:?}",
                sensors.minimum_scale_world_units
            ),
        );
    }
    let mut pyramid_levels = 0_u32;
    let mut level_scale = finest;
    while level_scale < sensors.maximum_scale_world_units && pyramid_levels < 32 {
        level_scale *= 2.0;
        pyramid_levels += 1;
    }
    let divisor = 1_u64 << pyramid_levels;
    if u64::from(world.grid_width) % divisor != 0 || u64::from(world.grid_height) % divisor != 0 {
        issues.add(
            "world.grid_width",
            format!(
                "the maximum sensing scale needs {pyramid_levels} pyramid levels, so both grid \
                 dimensions must be divisible by {divisor}"
            ),
        );
    }

    // A source's truncated kernel must reach at least one cell center (A.3 item 9).
    let support_radius = config.sources.kernel_cutoff_sigma * config.sources.sigma_world_units;
    let farthest_from_center = (cell_width / 2.0).hypot(cell_height / 2.0);
    if support_radius < farthest_from_center {
        issues.add(
            "sources.kernel_cutoff_sigma",
            format!(
                "support radius {support_radius:?} can miss every cell center (needs >= \
                 {farthest_from_center:?})"
            ),
        );
    }

    // Tick alignment (A.3 item 8, §7.4).
    let ticks = |seconds: f64, path: &str, exact: bool, issues: &mut Issues| {
        let result = if exact {
            timestep.ticks_exact(seconds, "duration")
        } else {
            // Exact conversion uses the f64's binary value, so a decimal
            // such as 0.1 s at Δt = 1/10 lands just past a tick boundary and
            // would round up a whole tick. Refuse that ambiguity.
            let quotient = seconds / timestep.seconds_per_tick();
            let off_boundary = timestep.ticks_exact(seconds, "duration").is_err();
            if off_boundary
                && (quotient - quotient.round()).abs() <= quotient.abs() * 4.0 * f64::EPSILON
            {
                issues.add(
                    path,
                    format!(
                        "{seconds:?} s is within f64 rounding of {} ticks but not exactly on it; \
                         use a tick-aligned duration",
                        quotient.round()
                    ),
                );
                return None;
            }
            timestep.ticks_ceil(seconds, "duration")
        };
        result
            .map_err(|error| issues.add(path, error.to_string()))
            .ok()
    };
    let source_segment_ticks = ticks(
        config.sources.path_direction_interval_seconds,
        "sources.path_direction_interval_seconds",
        true,
        issues,
    );
    let cooldown_ticks = ticks(
        reproduction.cooldown_seconds,
        "reproduction.cooldown_seconds",
        false,
        issues,
    );
    let checkpoint_interval_ticks = ticks(
        config.persistence.checkpoint_interval_simulation_seconds,
        "persistence.checkpoint_interval_simulation_seconds",
        false,
        issues,
    );

    // Logical genome quota: the initial population must fit; report the
    // default-topology and parameter-cap totals at the population ceiling.
    let dtype = &controller.canonical_weight_dtype;
    let Some(dtype_bytes) = registry::dtype_bytes(dtype) else {
        if registry::lookup(registry::Role::WeightDtype, dtype).is_ok() {
            issues.add(
                "controller.canonical_weight_dtype",
                format!("`{dtype}` has no byte size"),
            );
        }
        return None;
    };
    let default_parameter_count = default_parameter_count.ok()?;
    let bank = |organisms: u32, parameters: u64, path: &str, issues: &mut Issues| {
        genome_bank_bytes(u64::from(organisms), parameters, dtype_bytes)
            .map_err(|error| issues.add(path, error.to_string()))
            .ok()
    };
    let initial_genome_bytes = bank(
        config.population.initial_count,
        default_parameter_count,
        "population.initial_count",
        issues,
    )?;
    if initial_genome_bytes > execution.maximum_logical_genome_bytes {
        issues.add(
            "execution.maximum_logical_genome_bytes",
            format!(
                "the initial population needs {initial_genome_bytes} logical genome bytes, above \
                 the quota {}",
                execution.maximum_logical_genome_bytes
            ),
        );
    }
    let default_genome_bytes_at_maximum = bank(
        config.population.maximum_count,
        default_parameter_count,
        "population.maximum_count",
        issues,
    )?;
    let parameter_cap_genome_bytes_at_maximum = bank(
        config.population.maximum_count,
        architecture.maximum_parameters,
        "architecture_mutation.maximum_parameters",
        issues,
    )?;

    Some(ValidationReport {
        timestep,
        cell_width,
        cell_height,
        pyramid_levels,
        cooldown_ticks: cooldown_ticks?,
        source_segment_ticks: source_segment_ticks?,
        checkpoint_interval_ticks: checkpoint_interval_ticks?,
        default_parameter_count,
        initial_genome_bytes,
        default_genome_bytes_at_maximum,
        parameter_cap_genome_bytes_at_maximum,
        worst_case_fits_logical_quota: parameter_cap_genome_bytes_at_maximum
            <= execution.maximum_logical_genome_bytes,
    })
}
