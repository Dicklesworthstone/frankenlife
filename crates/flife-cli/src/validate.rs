//! `flife validate`: a configuration's canonical summary (plan §14.6, A.2).

use flife_core::ValidatedConfig;
use serde::Serialize;

/// The schema of the summary `flife validate` prints.
pub const VALIDATE_SCHEMA: &str = "frankenlife.validate.v1";

/// What validating a configuration established.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Validation {
    /// [`VALIDATE_SCHEMA`].
    pub schema: &'static str,
    /// The configuration's name.
    pub name: String,
    /// Its model.
    pub model: String,
    /// Its canonical hash.
    pub config_hash: String,
    /// Its world seed.
    pub seed: u64,
    /// Δt as `[numerator, denominator]` simulation-seconds.
    pub timestep: [u64; 2],
    /// The field grid, `[x, y]` cells.
    pub grid: [u64; 2],
    /// Founders.
    pub founders: u64,
    /// The population ceiling.
    pub maximum_population: u64,
    /// Parameters of the default network.
    pub default_parameters: u64,
    /// Logical genome bytes of the founders.
    pub initial_genome_bytes: u64,
    /// Logical genome bytes with every admitted organism at the parameter
    /// cap: the worst admitted live allocation.
    pub worst_genome_bytes: u64,
    /// Whether that worst case fits the logical genome quota.
    pub worst_case_fits_logical_quota: bool,
    /// The reproduction cooldown in ticks.
    pub cooldown_ticks: u64,
    /// The command durability the session will use.
    pub command_durability: String,
}

impl Validation {
    /// Summarize `config`.
    #[must_use]
    pub fn of(config: &ValidatedConfig) -> Self {
        let model = config.config();
        let report = config.report();
        Self {
            schema: VALIDATE_SCHEMA,
            name: model.name.clone(),
            model: model.model.clone(),
            config_hash: config.config_hash().to_hex(),
            seed: model.seed,
            timestep: [
                u64::from(report.timestep.numerator()),
                u64::from(report.timestep.denominator()),
            ],
            grid: [
                u64::from(model.world.grid_width),
                u64::from(model.world.grid_height),
            ],
            founders: u64::from(model.population.initial_count),
            maximum_population: u64::from(model.population.maximum_count),
            default_parameters: report.default_parameter_count,
            initial_genome_bytes: report.initial_genome_bytes,
            worst_genome_bytes: report.parameter_cap_genome_bytes_at_maximum,
            worst_case_fits_logical_quota: report.worst_case_fits_logical_quota,
            cooldown_ticks: report.cooldown_ticks.get(),
            command_durability: model.execution.command_durability.clone(),
        }
    }
}
