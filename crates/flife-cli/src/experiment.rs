//! Seed ensembles, sweeps and ablations (FL-041; plan §15.3–§15.5, §21.6,
//! Appendix B.1).
//!
//! An experiment is a TOML specification ([`Spec`], schema
//! [`EXPERIMENT_SCHEMA`]): a base configuration, conditions that change it
//! (each a named [`Ablation`], explicit key overrides, or a combination
//! from a sweep), a control condition, discovery seeds and held-out seeds,
//! and the selection procedure in words. [`experiment`] runs every
//! condition on every discovery seed, then, when the specification has a
//! `[select]` rule, picks one condition from the discovery runs alone and
//! runs it and the control on the held-out seeds; without one, every
//! condition runs on the held-out seeds too.
//!
//! Every run is cataloged when it ends ([`flife_store::ExperimentCatalog`]),
//! whether it reached its tick, died out, paused at a capacity limit or
//! failed, with its result and its run manifest (Appendix B.1), and its
//! records are kept in `runs/`. An experiment directory that already
//! catalogs the same experiment continues: cataloged runs are not run
//! again. No run is ever left out.
//!
//! The report ([`Report`], `report.json`) is computed from the catalog
//! alone and holds only scientific content: the same specification gives
//! byte-identical reports on any qualified host, at any worker count, and
//! across an interruption. For each non-control condition and each split
//! it compares every [`MEASURES`] entry with the control seed by seed:
//! the mean difference, how many seeds rose, fell or tied, and an exact
//! two-sided sign-flip test over the paired differences (AGENTS.md's
//! "effect e over k of n seeds versus control"). Runs that did not reach
//! their tick are listed and counted, and left out of the paired
//! differences, never silently. Each measure states its basis (§15.2):
//! complete over the run, complete at its end, or sampled from the render
//! epoch at the run's last metrics record. A sampled measure can be
//! undefined (no organism left to clump); its effect names the paired
//! seeds it leaves out.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use flife_backend_metal::ControllerChoice;
use flife_core::config::CONFIG_SCHEMA;
use flife_core::{
    ConfigIssue, CoreError, ModelDescriptor, RunConfig, Sha256Digest, ValidatedConfig,
};
use flife_store::{CatalogedRun, ExperimentCatalog};
use serde::{Deserialize, Serialize};

use crate::metrics::Metrics;
use crate::observe::{Survey, Tally};
use crate::run::{ControllerSummary, Finished, Record, RunOptions, Stop};

/// The schema of an experiment specification and of the records
/// `flife experiment` prints.
pub const EXPERIMENT_SCHEMA: &str = "frankenlife.experiment.v1";

/// The schema of an experiment report.
pub const REPORT_SCHEMA: &str = "frankenlife.experiment-report.v1";

/// The schema of a run manifest (Appendix B.1).
pub const MANIFEST_SCHEMA: &str = "frankenlife.run-manifest.v1";

/// The experiment catalog's file in an experiment directory.
pub const EXPERIMENT_CATALOG_FILE: &str = "experiment.sqlite";

/// The report's file in an experiment directory.
pub const REPORT_FILE: &str = "report.json";

/// The directory of the runs' record files.
pub const RUNS_DIR: &str = "runs";

/// The most paired seeds the exact sign-flip test enumerates (`2^20`
/// sign assignments). Larger splits report no p-value.
pub const EXACT_TEST_MAX_SEEDS: usize = 20;

/// `discovery`: the seeds a selection may look at.
pub const DISCOVERY: &str = "discovery";

/// `held-out`: the seeds no selection looks at.
pub const HELD_OUT: &str = "held-out";

/// A named ablation: one of §21.6's controls as configuration overrides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ablation {
    /// Its name in a specification.
    pub name: &'static str,
    /// The question it addresses.
    pub question: &'static str,
    /// The keys it sets, with TOML values.
    pub set: &'static [(&'static str, &'static str)],
}

/// The named ablations a specification may use. Each changes only the
/// rules it names, through validated configuration keys, so every ablated
/// run has its own configuration hash.
pub const ABLATIONS: &[Ablation] = &[
    Ablation {
        name: "no-emission",
        question: "Does field emission matter? (§15.3, H3): an open gate spends \
                   1 - exp(0) = 0 of the emitter's energy, so no emission is paid or deposited",
        set: &[("emission.maximum_fractional_rate_per_second", "0.0")],
    },
    Ablation {
        name: "fixed-topology",
        question: "Does architecture evolution matter? (§15.3, H5): no birth changes topology",
        set: &[("architecture_mutation.enabled", "false")],
    },
    Ablation {
        name: "no-complexity-cost",
        question: "Is network simplification an artifact of the complexity cost? (§6.2, §21.6): \
                   controller evaluations cost nothing",
        set: &[("costs.neural_energy_per_parameter_evaluation", "0.0")],
    },
    Ablation {
        name: "fixed-controller",
        question: "Does controller evolution matter? (§21.6): children inherit their parent's \
                   controller unchanged; traits still mutate",
        set: &[
            ("mutation.weight_probability", "0.0"),
            ("mutation.replacement_probability", "0.0"),
            ("architecture_mutation.enabled", "false"),
        ],
    },
    Ablation {
        name: "chemistry-only",
        question: "Do neural decisions matter? (§15.3, H2): controllers are evaluated and \
                   charged as before, but actions decode from an all-zero output",
        set: &[("model", "\"field-ecology-v1-chemistry-only\"")],
    },
    Ablation {
        name: "fresh-offspring",
        question: "Does inheritance matter? (§15.3): each newborn gets the genome a founder \
                   would have at its identity, unmutated, instead of its parent's mutated genome",
        set: &[("model", "\"field-ecology-v1-fresh-offspring\"")],
    },
];

/// The named ablation called `name`.
///
/// # Errors
/// [`CoreError::UnknownAlgorithm`] for another name.
pub fn ablation(name: &str) -> Result<&'static Ablation, CoreError> {
    ABLATIONS
        .iter()
        .find(|ablation| ablation.name == name)
        .ok_or_else(|| CoreError::UnknownAlgorithm {
            role: "experiment ablation",
            name: name.to_owned(),
        })
}

/// An experiment specification, as parsed.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// Must equal [`EXPERIMENT_SCHEMA`].
    pub schema: String,
    /// The experiment's name.
    pub name: String,
    /// The question or hypothesis it tests (§15.4).
    pub hypothesis: String,
    /// The base configuration, relative to the specification's directory.
    pub base: PathBuf,
    /// Ticks each run publishes.
    pub ticks: u64,
    /// A metrics record every this many ticks in each run's records; zero
    /// for none.
    #[serde(default)]
    pub metrics_every: u64,
    /// The control condition's name.
    pub control: String,
    /// The selection procedure, in words (§15.5): which runs a claim may
    /// rest on and how they were chosen.
    pub selection: String,
    /// The seeds.
    pub seeds: Seeds,
    /// Overrides every condition shares, by dotted key.
    #[serde(default)]
    pub set: BTreeMap<String, toml::Value>,
    /// The conditions, in order.
    #[serde(default)]
    pub conditions: Vec<ConditionSpec>,
    /// Keys and their values; every combination is a further condition,
    /// named `key=value;…` in key order.
    #[serde(default)]
    pub sweep: BTreeMap<String, Vec<toml::Value>>,
    /// A rule picking one non-control condition from the discovery runs.
    #[serde(default)]
    pub select: Option<Select>,
}

/// The seeds an experiment runs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Seeds {
    /// Seeds a selection may look at.
    pub discovery: Vec<u64>,
    /// Seeds no selection looks at.
    #[serde(default)]
    pub held_out: Vec<u64>,
}

/// One condition, as specified.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConditionSpec {
    /// Its name.
    pub name: String,
    /// A named ablation it applies.
    #[serde(default)]
    pub ablation: Option<String>,
    /// Its own overrides, by dotted key; each replaces the shared value of
    /// its key.
    #[serde(default)]
    pub set: BTreeMap<String, toml::Value>,
}

/// A selection rule (§15.5).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Select {
    /// One of [`MEASURES`].
    pub measure: String,
    /// `max` or `min` of its mean over the complete discovery runs.
    pub rule: String,
}

/// A condition resolved: its ablation and every override it applies.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Condition {
    /// Its name.
    pub name: String,
    /// The named ablation, if any.
    pub ablation: Option<&'static str>,
    /// Every key it sets, the shared overrides included, as TOML values.
    pub set: BTreeMap<String, String>,
}

/// An experiment ready to run: its specification resolved against its
/// base configuration.
#[derive(Debug, Clone)]
pub struct Experiment {
    /// The specification.
    pub spec: Spec,
    /// The resolved conditions, in order: explicit ones, then the sweep's.
    pub conditions: Vec<Condition>,
    /// The control's index in `conditions`.
    pub control: usize,
    base: toml::Table,
    base_hash: Sha256Digest,
    design: String,
    identity: Sha256Digest,
}

fn spec_error(path: impl Into<String>, problem: impl Into<String>) -> CoreError {
    CoreError::InvalidConfig(vec![ConfigIssue {
        path: path.into(),
        problem: problem.into(),
    }])
}

/// A TOML value written back as TOML: integers, floats in shortest
/// round-trip form, booleans, strings and arrays of those.
fn literal(value: &toml::Value) -> Result<String, CoreError> {
    Ok(match value {
        toml::Value::Integer(integer) => integer.to_string(),
        toml::Value::Float(float) if float.is_finite() => format!("{float:?}"),
        toml::Value::Boolean(boolean) => boolean.to_string(),
        toml::Value::String(text) => {
            serde_json::to_string(text).map_err(|error| spec_error("set", error.to_string()))?
        }
        toml::Value::Array(items) => {
            let items: Result<Vec<_>, _> = items.iter().map(literal).collect();
            format!("[{}]", items?.join(", "))
        }
        other => {
            return Err(spec_error(
                "set",
                format!("{other:?} is not an integer, float, boolean, string or array of them"),
            ));
        }
    })
}

/// `text` parsed as one TOML value.
fn parse_literal(text: &str) -> Result<toml::Value, CoreError> {
    let table: toml::Table = toml::from_str(&format!("v = {text}"))
        .map_err(|error| spec_error("set", error.to_string().trim().to_owned()))?;
    table
        .get("v")
        .cloned()
        .ok_or_else(|| spec_error("set", format!("`{text}` is not a TOML value")))
}

/// Set the existing dotted `key` of `table` to `value`.
fn assign(table: &mut toml::Table, key: &str, value: toml::Value) -> Result<(), CoreError> {
    if matches!(key, "seed" | "schema") {
        return Err(spec_error(
            key,
            "an experiment sets the seed from [seeds] and keeps the schema",
        ));
    }
    let mut parts = key.split('.').peekable();
    let mut current = table;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return match current.get_mut(part) {
                Some(slot) if !slot.is_table() => {
                    *slot = value;
                    Ok(())
                }
                _ => Err(spec_error(key, "not a key of the base configuration")),
            };
        }
        current = match current.get_mut(part) {
            Some(toml::Value::Table(inner)) => inner,
            _ => return Err(spec_error(key, "not a key of the base configuration")),
        };
    }
    Err(spec_error(key, "an empty key"))
}

impl Experiment {
    /// Parse the specification at `path` and resolve it against its base
    /// configuration. Every condition's configuration is validated at the
    /// first discovery seed, so an experiment with an invalid condition is
    /// refused before anything runs.
    ///
    /// # Errors
    /// [`CoreError::InvalidConfig`] for an unreadable, malformed or
    /// inconsistent specification or an invalid condition;
    /// [`CoreError::UnknownAlgorithm`] for an unknown ablation.
    pub fn load(path: &Path) -> Result<Self, CoreError> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| spec_error(path.display().to_string(), error.to_string()))?;
        let spec: Spec = toml::from_str(&text)
            .map_err(|error| spec_error("experiment", error.to_string().trim().to_owned()))?;
        let directory = path.parent().unwrap_or_else(|| Path::new("."));
        let base_path = directory.join(&spec.base);
        let base_text = std::fs::read_to_string(&base_path)
            .map_err(|error| spec_error("base", format!("{}: {error}", base_path.display())))?;
        Self::new(spec, &base_text)
    }

    /// Resolve `spec` against the base configuration `base_text`.
    ///
    /// # Errors
    /// As [`Experiment::load`].
    pub fn new(spec: Spec, base_text: &str) -> Result<Self, CoreError> {
        if spec.schema != EXPERIMENT_SCHEMA {
            return Err(CoreError::SchemaMismatch {
                expected: EXPERIMENT_SCHEMA.into(),
                found: spec.schema,
            });
        }
        if spec.name.is_empty()
            || spec.hypothesis.trim().is_empty()
            || spec.selection.trim().is_empty()
        {
            return Err(spec_error(
                "experiment",
                "name, hypothesis and selection must not be empty",
            ));
        }
        if spec.ticks == 0 {
            return Err(spec_error("ticks", "must be at least 1"));
        }
        let base_hash = ValidatedConfig::from_toml(base_text)?.config_hash();
        let base: toml::Table = toml::from_str(base_text)
            .map_err(|error| spec_error("base", error.to_string().trim().to_owned()))?;
        Self::check_seeds(&spec.seeds)?;

        let mut conditions = Vec::new();
        for condition in &spec.conditions {
            conditions.push(Self::resolve(
                &spec.set,
                &condition.name,
                condition.ablation.as_deref(),
                &condition.set,
            )?);
        }
        for combination in Self::combinations(&spec.sweep)? {
            let name = combination
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join(";");
            let mut set = BTreeMap::new();
            for (key, value) in combination {
                set.insert(key, parse_literal(&value)?);
            }
            conditions.push(Self::resolve(&spec.set, &name, None, &set)?);
        }
        let mut names = BTreeSet::new();
        for condition in &conditions {
            if condition.name.is_empty() || !names.insert(condition.name.as_str()) {
                return Err(spec_error(
                    "conditions",
                    format!("condition name `{}` is empty or repeated", condition.name),
                ));
            }
        }
        let control = conditions
            .iter()
            .position(|condition| condition.name == spec.control)
            .ok_or_else(|| {
                spec_error(
                    "control",
                    format!("no condition is named `{}`", spec.control),
                )
            })?;
        if conditions.len() < 2 {
            return Err(spec_error(
                "conditions",
                "an experiment compares at least one condition with its control",
            ));
        }
        if let Some(select) = &spec.select {
            if !MEASURES
                .iter()
                .any(|measure| measure.name == select.measure)
            {
                return Err(spec_error(
                    "select.measure",
                    format!("`{}` is not a measure", select.measure),
                ));
            }
            if !matches!(select.rule.as_str(), "max" | "min") {
                return Err(spec_error("select.rule", "must be `max` or `min`"));
            }
        }

        let mut experiment = Self {
            spec,
            conditions,
            control,
            base,
            base_hash,
            design: String::new(),
            identity: Sha256Digest::of(&[]),
        };
        let first = experiment
            .spec
            .seeds
            .discovery
            .first()
            .copied()
            .unwrap_or(0);
        for index in 0..experiment.conditions.len() {
            experiment
                .configuration(index, first)
                .map_err(|error| match error {
                    CoreError::InvalidConfig(issues) => CoreError::InvalidConfig(
                        issues
                            .into_iter()
                            .map(|issue| ConfigIssue {
                                path: format!(
                                    "condition `{}`: {}",
                                    experiment.conditions[index].name, issue.path
                                ),
                                problem: issue.problem,
                            })
                            .collect(),
                    ),
                    other => other,
                })?;
        }
        experiment.design = serde_json::to_string(&Design {
            schema: EXPERIMENT_SCHEMA,
            name: &experiment.spec.name,
            hypothesis: &experiment.spec.hypothesis,
            selection: &experiment.spec.selection,
            base: experiment.base_hash.to_hex(),
            ticks: experiment.spec.ticks,
            metrics_every: experiment.spec.metrics_every,
            control: &experiment.spec.control,
            seeds: &experiment.spec.seeds,
            select: experiment.spec.select.as_ref(),
            conditions: &experiment.conditions,
            measures: MEASURES.iter().map(|measure| measure.name).collect(),
        })
        .map_err(|error| spec_error("experiment", error.to_string()))?;
        experiment.identity = Sha256Digest::of(experiment.design.as_bytes());
        Ok(experiment)
    }

    fn check_seeds(seeds: &Seeds) -> Result<(), CoreError> {
        if seeds.discovery.is_empty() {
            return Err(spec_error("seeds.discovery", "needs at least one seed"));
        }
        let mut seen = BTreeSet::new();
        for &seed in seeds.discovery.iter().chain(&seeds.held_out) {
            if i64::try_from(seed).is_err() {
                return Err(spec_error("seeds", format!("{seed} is not a TOML integer")));
            }
            if !seen.insert(seed) {
                return Err(spec_error(
                    "seeds",
                    format!(
                        "seed {seed} is repeated; held-out seeds are separate from discovery seeds"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// One condition: the shared overrides, replaced key by key by its own,
    /// then its ablation's. An ablation's keys are its definition, so a
    /// condition that also sets one is refused.
    fn resolve(
        shared: &BTreeMap<String, toml::Value>,
        name: &str,
        ablation_name: Option<&str>,
        own: &BTreeMap<String, toml::Value>,
    ) -> Result<Condition, CoreError> {
        let ablation = ablation_name.map(ablation).transpose()?;
        let mut set = BTreeMap::new();
        for (key, value) in shared.iter().chain(own) {
            set.insert(key.clone(), literal(value)?);
        }
        for &(key, value) in ablation.map_or(&[][..], |ablation| ablation.set) {
            if set.insert(key.to_owned(), value.to_owned()).is_some() {
                return Err(spec_error(
                    format!("condition `{name}`"),
                    format!(
                        "its ablation sets `{key}`; neither the condition nor [set] may change it"
                    ),
                ));
            }
        }
        Ok(Condition {
            name: name.to_owned(),
            ablation: ablation.map(|ablation| ablation.name),
            set,
        })
    }

    /// Every combination of the sweep's values, keys in order, each value
    /// written as TOML.
    fn combinations(
        sweep: &BTreeMap<String, Vec<toml::Value>>,
    ) -> Result<Vec<Vec<(String, String)>>, CoreError> {
        let mut combinations = vec![Vec::new()];
        for (key, values) in sweep {
            if values.is_empty() {
                return Err(spec_error(
                    format!("sweep.{key}"),
                    "needs at least one value",
                ));
            }
            let mut next = Vec::with_capacity(combinations.len() * values.len());
            for combination in &combinations {
                for value in values {
                    let mut extended: Vec<(String, String)> = combination.clone();
                    extended.push((key.clone(), literal(value)?));
                    next.push(extended);
                }
            }
            combinations = next;
        }
        if sweep.is_empty() {
            return Ok(Vec::new());
        }
        Ok(combinations)
    }

    /// Condition `index`'s configuration at `seed`.
    ///
    /// # Errors
    /// [`CoreError::InvalidConfig`] when an override names no key of the
    /// base configuration or the result does not validate.
    pub fn configuration(&self, index: usize, seed: u64) -> Result<ValidatedConfig, CoreError> {
        let condition = self
            .conditions
            .get(index)
            .ok_or_else(|| spec_error("conditions", format!("no condition {index}")))?;
        let mut table = self.base.clone();
        for (key, value) in &condition.set {
            assign(&mut table, key, parse_literal(value)?)?;
        }
        let seed = i64::try_from(seed)
            .map_err(|_| spec_error("seeds", format!("{seed} is not a TOML integer")))?;
        table.insert("seed".to_owned(), toml::Value::Integer(seed));
        let config: RunConfig =
            toml::Value::Table(table)
                .try_into()
                .map_err(|error: toml::de::Error| {
                    spec_error("toml", error.to_string().trim().to_owned())
                })?;
        flife_core::validate(config)
    }

    /// The experiment's identity: SHA-256 of its canonical design.
    #[must_use]
    pub const fn identity(&self) -> Sha256Digest {
        self.identity
    }

    /// The canonical design (JSON) the identity hashes.
    #[must_use]
    pub fn design(&self) -> &str {
        &self.design
    }

    /// The base configuration's canonical hash.
    #[must_use]
    pub const fn base_hash(&self) -> Sha256Digest {
        self.base_hash
    }
}

/// What an experiment's identity hashes: everything that decides its runs
/// and its report.
#[derive(Serialize)]
struct Design<'a> {
    schema: &'static str,
    name: &'a str,
    hypothesis: &'a str,
    selection: &'a str,
    base: String,
    ticks: u64,
    metrics_every: u64,
    control: &'a str,
    seeds: &'a Seeds,
    select: Option<&'a Select>,
    conditions: &'a [Condition],
    /// The measures a report compares: a catalog of runs measured by
    /// another set is another experiment.
    measures: Vec<&'static str>,
}

// ------------------------------------------------------------------ results

/// A run's outcome measures: its tally, the survey of its committed world,
/// and its last metrics record.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Measures {
    /// 1 when any organism is alive at the end, else 0.
    pub survived: u64,
    /// Live organisms at the end.
    pub final_live: u64,
    /// The largest live population after a tick.
    pub peak_live: u64,
    /// The smallest live population after a tick.
    pub least_live: u64,
    /// Births committed.
    pub births: u64,
    /// Deaths committed.
    pub deaths: u64,
    /// The largest live generation at the end.
    pub max_generation: u64,
    /// Founder lineages with a live member at the end.
    pub lineages_alive: u64,
    /// Distinct genomes alive at the end.
    pub distinct_genomes: u64,
    /// Organism-ticks with a paid emission.
    pub emitter_ticks: u64,
    /// Energy organisms paid to emit.
    pub emission_paid: f64,
    /// Births whose child changed topology.
    pub topology_changes: u64,
    /// Parameters the births perturbed or replaced.
    pub mutated_parameters: u64,
    /// Topologies with a live organism at the end: the architecture
    /// distribution's support.
    pub topologies_alive: u64,
    /// From the last metrics record (none without metrics records), at the
    /// run's end: what it holds of the committed world, and what it sampled
    /// from the render epoch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<LastRecord>,
}

/// What a run's last metrics record says of its end (§15.2).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LastRecord {
    /// Its tick: the run's last committed tick.
    pub tick: u64,
    /// Complete: the entropy of the live lineage shares, in nats.
    pub lineage_entropy: Option<f64>,
    /// Complete: the largest lineage's share of the live population.
    pub dominant_lineage_share: Option<f64>,
    /// Complete: the mean live generation.
    pub mean_generation: Option<f64>,
    /// Sampled: the fraction of the 16 × 16 bins holding an organism.
    pub occupied_fraction: f64,
    /// Sampled: Morisita's index of the organisms over the bins (1 random,
    /// above 1 clumped).
    pub organism_morisita: Option<f64>,
    /// Sampled: Moran's I of the bin counts.
    pub organism_morans_i: Option<f64>,
    /// Sampled: the Gini coefficient of the field's energy density.
    pub field_gini: Option<f64>,
    /// Sampled: Moran's I of the field's energy density.
    pub field_morans_i: Option<f64>,
    /// Sampled: the mean speed, world units per simulated second.
    pub mean_speed: Option<f64>,
    /// Sampled: the fraction of organisms moving.
    pub moving_fraction: Option<f64>,
}

impl LastRecord {
    /// The end of a run as `metrics`, its last record, holds it.
    #[must_use]
    pub fn of(metrics: &Metrics) -> Self {
        let sampled = &metrics.sampled;
        Self {
            tick: metrics.tick,
            lineage_entropy: metrics.lineages.entropy,
            dominant_lineage_share: metrics.lineages.dominant_share,
            mean_generation: metrics.population.mean_generation,
            occupied_fraction: sampled.dispersion.occupied,
            organism_morisita: sampled.dispersion.morisita,
            organism_morans_i: sampled.dispersion.morans_i,
            field_gini: sampled.resources.gini,
            field_morans_i: sampled.resources.morans_i,
            mean_speed: sampled.motility.mean_speed,
            moving_fraction: sampled.motility.moving,
        }
    }
}

fn wide(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

impl Measures {
    /// The measures of a run with `tally` whose committed world is
    /// `survey`, and whose last metrics record, if any, is `last`. A record
    /// of another tick than the survey's is not the run's end and is not
    /// used.
    #[must_use]
    pub fn of(tally: &Tally, survey: &Survey, last: Option<&Metrics>) -> Self {
        Self {
            survived: u64::from(survey.live > 0),
            final_live: wide(survey.live),
            peak_live: wide(tally.peak_live),
            least_live: wide(tally.least_live.unwrap_or(survey.live)),
            births: tally.births,
            deaths: tally.deaths,
            max_generation: u64::from(survey.max_generation),
            lineages_alive: wide(survey.lineages_alive),
            distinct_genomes: wide(survey.distinct_genomes),
            emitter_ticks: tally.emitter_ticks,
            emission_paid: tally.emission_paid,
            topology_changes: tally.topology_changes,
            mutated_parameters: tally.perturbed_parameters + tally.replaced_parameters,
            topologies_alive: wide(survey.topologies.values().filter(|&&live| live > 0).count()),
            last: last
                .filter(|metrics| metrics.tick == survey.tick)
                .map(LastRecord::of),
        }
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "counts compared as means; exact below 2^53"
)]
const fn real(value: u64) -> f64 {
    value as f64
}

/// How a measure is read from a run's [`Measures`]: none when it is
/// undefined for that run.
pub type Read = fn(&Measures) -> Option<f64>;

/// One measure a report compares.
#[derive(Debug, Clone, Copy)]
pub struct Measure {
    /// Its name.
    pub name: &'static str,
    /// What it is computed from (§15.2).
    pub basis: &'static str,
    /// How it is read.
    pub read: Read,
}

/// Complete over the whole run: every published tick's report.
pub const RUN_TALLY: &str = "complete: every tick of the run";
/// Complete at the end: the committed world.
pub const END_STATE: &str = "complete: the committed world at the end";
/// Sampled at the end: the render epoch at the last metrics record.
pub const END_SAMPLE: &str = "sampled: the render epoch at the last metrics record";

const fn tally(name: &'static str, read: Read) -> Measure {
    Measure {
        name,
        basis: RUN_TALLY,
        read,
    }
}

const fn end(name: &'static str, read: Read) -> Measure {
    Measure {
        name,
        basis: END_STATE,
        read,
    }
}

const fn sample(name: &'static str, read: Read) -> Measure {
    Measure {
        name,
        basis: END_SAMPLE,
        read,
    }
}

/// Every measure the report compares, by name, with its basis.
pub const MEASURES: [Measure; 24] = [
    end("survived", |m| Some(real(m.survived))),
    end("final_live", |m| Some(real(m.final_live))),
    tally("peak_live", |m| Some(real(m.peak_live))),
    tally("least_live", |m| Some(real(m.least_live))),
    tally("births", |m| Some(real(m.births))),
    tally("deaths", |m| Some(real(m.deaths))),
    end("max_generation", |m| Some(real(m.max_generation))),
    end("lineages_alive", |m| Some(real(m.lineages_alive))),
    end("distinct_genomes", |m| Some(real(m.distinct_genomes))),
    tally("emitter_ticks", |m| Some(real(m.emitter_ticks))),
    tally("emission_paid", |m| Some(m.emission_paid)),
    tally("topology_changes", |m| Some(real(m.topology_changes))),
    tally("mutated_parameters", |m| Some(real(m.mutated_parameters))),
    end("topologies_alive", |m| Some(real(m.topologies_alive))),
    end("lineage_entropy", |m| {
        m.last.and_then(|l| l.lineage_entropy)
    }),
    end("dominant_lineage_share", |m| {
        m.last.and_then(|l| l.dominant_lineage_share)
    }),
    end("mean_generation", |m| {
        m.last.and_then(|l| l.mean_generation)
    }),
    sample("occupied_fraction", |m| m.last.map(|l| l.occupied_fraction)),
    sample("organism_morisita", |m| {
        m.last.and_then(|l| l.organism_morisita)
    }),
    sample("organism_morans_i", |m| {
        m.last.and_then(|l| l.organism_morans_i)
    }),
    sample("field_gini", |m| m.last.and_then(|l| l.field_gini)),
    sample("field_morans_i", |m| m.last.and_then(|l| l.field_morans_i)),
    sample("mean_speed", |m| m.last.and_then(|l| l.mean_speed)),
    sample("moving_fraction", |m| {
        m.last.and_then(|l| l.moving_fraction)
    }),
];

/// A run's scientific result, as cataloged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunResult {
    /// `reached`, `capacity-paused`, `failed`, or `error` for a run refused
    /// or broken before its end record.
    pub status: String,
    /// What stopped it early, for any status but `reached`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The committed tick it ended at.
    pub tick: u64,
    /// The committed world's digest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// No organism alive at the end.
    pub extinct: bool,
    /// Its measures; none for an `error` run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measures: Option<Measures>,
}

impl RunResult {
    /// Whether it reached its tick: only such runs enter the paired
    /// differences. An extinct run that reached its tick is complete.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.status == "reached"
    }

    fn of(finished: &Finished, last: Option<&Metrics>) -> Self {
        let (status, detail) = match &finished.stop {
            Stop::Reached => ("reached", None),
            Stop::CapacityPaused { detail } => ("capacity-paused", Some(detail.clone())),
            Stop::Failed { kind, detail, .. } => ("failed", Some(format!("{kind}: {detail}"))),
        };
        Self {
            status: status.to_owned(),
            detail,
            tick: finished.closed.boundary.tick.get(),
            digest: Some(finished.closed.world.digest().to_hex()),
            extinct: finished.survey.live == 0,
            measures: Some(Measures::of(&finished.tally, &finished.survey, last)),
        }
    }

    fn error(error: &CoreError) -> Self {
        Self {
            status: "error".to_owned(),
            detail: Some(format!("{}: {error}", error.kind())),
            tick: 0,
            digest: None,
            extinct: false,
            measures: None,
        }
    }
}

// ------------------------------------------------------------------ manifest

const UNAVAILABLE: &str = "unavailable";

fn build_fact(value: &'static str) -> Option<&'static str> {
    (value != UNAVAILABLE).then_some(value)
}

/// The host a run executed on (Appendix B.1's hardware group). A fact this
/// host does not expose is absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Host {
    /// The CPU's brand name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<String>,
    /// Logical cores available to this process.
    pub logical_cores: usize,
    /// Physical memory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    /// The operating system and its version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
}

fn command_line(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let line = text.trim();
    (!line.is_empty()).then(|| line.to_owned())
}

fn proc_field(path: &str, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix(key))
        .map(|rest| {
            rest.trim_start_matches([' ', '\t', ':', '='])
                .trim()
                .trim_matches('"')
                .to_owned()
        })
        .filter(|value| !value.is_empty())
}

impl Host {
    /// This host, read from `sysctl` and `sw_vers` on macOS and from
    /// `/proc` and `/etc/os-release` on Linux.
    #[must_use]
    pub fn current() -> Self {
        let logical_cores = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        if cfg!(target_os = "macos") {
            Self {
                cpu: command_line("sysctl", &["-n", "machdep.cpu.brand_string"]),
                logical_cores,
                memory_bytes: command_line("sysctl", &["-n", "hw.memsize"])
                    .and_then(|bytes| bytes.parse().ok()),
                os: command_line("sw_vers", &["-productVersion"])
                    .map(|version| format!("macOS {version}")),
            }
        } else {
            Self {
                cpu: proc_field("/proc/cpuinfo", "model name"),
                logical_cores,
                memory_bytes: proc_field("/proc/meminfo", "MemTotal")
                    .and_then(|kib| kib.trim_end_matches("kB").trim().parse::<u64>().ok())
                    .map(|kib| kib.saturating_mul(1024)),
                os: proc_field("/etc/os-release", "PRETTY_NAME"),
            }
        }
    }
}

/// Where a run sits in its experiment.
#[derive(Debug, Clone, Copy)]
struct Placement<'a> {
    experiment: &'a Experiment,
    ordinal: u64,
    condition: &'a Condition,
    split: &'static str,
    seed: u64,
}

/// What the run did beyond its scientific result: wall time and the
/// controller backend. Not scientific state.
struct Execution<'a> {
    wall_seconds: f64,
    cpu_workers: Option<usize>,
    controllers: Option<&'a ControllerSummary>,
    records: &'a str,
    records_sha256: &'a str,
}

/// A run manifest (Appendix B.1), grouped as the appendix groups it. It is
/// not part of the report: it holds the host, the build and wall time.
fn manifest(
    placement: Placement<'_>,
    config: Option<&ValidatedConfig>,
    result: &RunResult,
    execution: &Execution<'_>,
    host: &Host,
) -> serde_json::Value {
    let Placement {
        experiment,
        ordinal,
        condition,
        split,
        seed,
    } = placement;
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let model = config.map(|config| {
        let c = config.config();
        let timestep = config.report().timestep;
        serde_json::json!({
            "model": {
                "config_schema": CONFIG_SCHEMA,
                "model": c.model,
                "descriptor": ModelDescriptor::of(config).digest().to_hex(),
                "config_hash": config.config_hash().to_hex(),
                "frankenlife_version": env!("CARGO_PKG_VERSION"),
            },
            "numerics": {
                "field_dtype": c.world.field_dtype,
                "weight_dtype": c.controller.canonical_weight_dtype,
                "accumulation_dtype": c.controller.accumulation_dtype,
                "math_mode": c.execution.math_mode,
                "determinism": c.execution.determinism,
                "rng": c.mutation.rng,
                "residual_budget": "reference-f32 (§21.4)",
            },
            "clocks": {
                "timestep": [timestep.numerator(), timestep.denominator()],
                "controller_interval_ticks": c.controller.evaluation_interval_ticks,
                "phase_policy": c.controller.phase_policy,
                "ticks": experiment.spec.ticks,
            },
            "capacity": {
                "maximum_count": c.population.maximum_count,
                "maximum_parameters": c.architecture_mutation.maximum_parameters,
                "maximum_logical_genome_bytes": c.execution.maximum_logical_genome_bytes,
                "maximum_allocated_genome_bytes": c.execution.maximum_allocated_genome_bytes,
                "maximum_total_bytes": c.execution.maximum_total_bytes,
                "checkpoint_retained_bytes": c.execution.checkpoint_retained_bytes,
                "command_queue_capacity": c.execution.command_queue_capacity,
                "control_queue_capacity": c.execution.control_queue_capacity,
                "render_queue_capacity": c.execution.render_queue_capacity,
            },
            "provenance": {
                "seed": c.seed,
                "founders": c.population.initial_count,
                "initializer": c.controller.initializer,
                "rng": c.mutation.rng,
                "command_durability": c.execution.command_durability,
                "interventions": 0,
                "source_path": c.sources.path,
            },
        })
    });
    let ticks_per_second = (execution.wall_seconds > 0.0 && result.tick > 0)
        .then(|| real(result.tick) / execution.wall_seconds);
    let mut manifest = serde_json::json!({
        "schema": MANIFEST_SCHEMA,
        "identity": {
            "run": format!("{}/{ordinal}", experiment.spec.name),
            "experiment": experiment.spec.name,
            "experiment_identity": experiment.identity.to_hex(),
            "ordinal": ordinal,
            "condition": condition.name,
            "ablation": condition.ablation,
            "split": split,
            "seed": seed,
            "parent": serde_json::Value::Null,
            "created_unix_seconds": created,
        },
        "source": {
            "revision": build_fact(env!("FLIFE_REVISION")),
            "suite": build_fact(env!("FLIFE_SUITE")).map(|suite| suite.split(',').collect::<Vec<_>>()),
            "rustc": build_fact(env!("FLIFE_RUSTC")),
            "target": build_fact(env!("FLIFE_TARGET")),
            "profile": build_fact(env!("FLIFE_PROFILE")),
        },
        "hardware": {
            "host": host,
            "cpu_workers": execution.cpu_workers,
            "controllers": execution.controllers,
        },
        "results": {
            "result": result,
            "wall_seconds": execution.wall_seconds,
            "ticks_per_second": ticks_per_second,
        },
        "artifacts": [{
            "path": execution.records,
            "sha256": execution.records_sha256,
        }],
    });
    if let (Some(serde_json::Value::Object(groups)), serde_json::Value::Object(out)) =
        (model, &mut manifest)
    {
        out.extend(groups);
    }
    manifest
}

// ------------------------------------------------------------------ running

/// How to run an experiment's worlds. No result depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExperimentOptions {
    /// Threads for each run's organism phases.
    pub cpu_workers: Option<NonZeroUsize>,
    /// Where due controllers are evaluated.
    pub controllers: ControllerChoice,
}

/// One line of `flife experiment`'s output.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "record")]
pub enum ExperimentRecord {
    /// What is about to run.
    ExperimentStart {
        /// [`EXPERIMENT_SCHEMA`].
        schema: &'static str,
        /// The experiment's name.
        name: String,
        /// Its identity.
        experiment: String,
        /// Its conditions, in order.
        conditions: Vec<String>,
        /// Runs already cataloged, which will not run again.
        cataloged: usize,
    },
    /// A run ended and was cataloged.
    ExperimentRun {
        /// [`EXPERIMENT_SCHEMA`].
        schema: &'static str,
        /// Its ordinal.
        ordinal: u64,
        /// Its condition.
        condition: String,
        /// `discovery` or `held-out`.
        split: &'static str,
        /// Its seed.
        seed: u64,
        /// Its result.
        result: Box<RunResult>,
        /// Wall-clock seconds it took. Not scientific state.
        wall_seconds: f64,
    },
    /// The selection rule picked a condition from the discovery runs.
    ExperimentSelect {
        /// [`EXPERIMENT_SCHEMA`].
        schema: &'static str,
        /// What was picked, and from what.
        selection: Selection,
    },
    /// The experiment's report was written.
    ExperimentEnd {
        /// [`EXPERIMENT_SCHEMA`].
        schema: &'static str,
        /// Runs cataloged.
        runs: usize,
        /// Of those, runs that reached their tick.
        complete: usize,
        /// Of those, runs with no organism alive at the end.
        extinct: usize,
        /// The report file.
        report: String,
        /// Its SHA-256.
        report_sha256: String,
    },
}

/// Write `bytes` to `path` through a temporary file, flushed, then renamed.
fn write_durably(path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    let failed = |operation: &'static str, error: std::io::Error| CoreError::StorageFailed {
        operation,
        path: path.display().to_string(),
        reason: error.to_string(),
    };
    let temporary = path.with_extension("partial");
    let mut file =
        std::fs::File::create(&temporary).map_err(|error| failed("create file", error))?;
    file.write_all(bytes)
        .map_err(|error| failed("write file", error))?;
    file.sync_all()
        .map_err(|error| failed("flush file", error))?;
    drop(file);
    std::fs::rename(&temporary, path).map_err(|error| failed("rename file", error))
}

/// A planned run.
#[derive(Debug, Clone, Copy)]
struct Planned {
    ordinal: u64,
    condition: usize,
    split: &'static str,
    seed: u64,
}

/// Run `experiment` in `directory`, creating it if needed, or continuing
/// the experiment it already catalogs: every run not yet cataloged, then
/// the report. Each record goes to `emit`.
///
/// # Errors
/// [`CoreError::StateConflict`] for a directory holding another experiment
/// or other files; the catalog's storage errors; a failure to write a
/// record file or the report. A run's own failure is cataloged, not
/// returned.
pub fn experiment(
    experiment: &Experiment,
    directory: &Path,
    options: ExperimentOptions,
    mut emit: impl FnMut(&ExperimentRecord),
) -> Result<Report, CoreError> {
    let catalog_path = directory.join(EXPERIMENT_CATALOG_FILE);
    if directory.exists() && !catalog_path.exists() {
        let mut entries =
            std::fs::read_dir(directory).map_err(|error| CoreError::StorageFailed {
                operation: "read experiment directory",
                path: directory.display().to_string(),
                reason: error.to_string(),
            })?;
        if entries.next().is_some() {
            return Err(CoreError::StateConflict {
                operation: "create experiment directory",
                detail: format!(
                    "{} holds files but no experiment catalog",
                    directory.display()
                ),
            });
        }
    }
    let runs_dir = directory.join(RUNS_DIR);
    std::fs::create_dir_all(&runs_dir).map_err(|error| CoreError::StorageFailed {
        operation: "create experiment directory",
        path: runs_dir.display().to_string(),
        reason: error.to_string(),
    })?;
    let mut catalog = ExperimentCatalog::open(
        &catalog_path,
        &experiment.identity.to_hex(),
        &experiment.design,
    )?;
    emit(&ExperimentRecord::ExperimentStart {
        schema: EXPERIMENT_SCHEMA,
        name: experiment.spec.name.clone(),
        experiment: experiment.identity.to_hex(),
        conditions: experiment
            .conditions
            .iter()
            .map(|condition| condition.name.clone())
            .collect(),
        cataloged: catalog.runs()?.len(),
    });
    let host = Host::current();

    let discovery: Vec<Planned> = plan(
        0,
        &(0..experiment.conditions.len()).collect::<Vec<_>>(),
        DISCOVERY,
        &experiment.spec.seeds.discovery,
    );
    run_all(
        experiment,
        &mut catalog,
        &discovery,
        &runs_dir,
        options,
        &host,
        &mut emit,
    )?;

    let selection = select(experiment, &catalog.runs()?)?;
    if let Some(selection) = &selection {
        emit(&ExperimentRecord::ExperimentSelect {
            schema: EXPERIMENT_SCHEMA,
            selection: selection.clone(),
        });
    }
    let held_out_conditions: Vec<usize> = match &selection {
        Some(selection) => {
            let mut chosen = vec![experiment.control];
            if let Some(index) = experiment
                .conditions
                .iter()
                .position(|condition| Some(&condition.name) == selection.selected.as_ref())
            {
                chosen.push(index);
            }
            chosen.sort_unstable();
            chosen
        }
        None => (0..experiment.conditions.len()).collect(),
    };
    let held_out = plan(
        wide(discovery.len()),
        &held_out_conditions,
        HELD_OUT,
        &experiment.spec.seeds.held_out,
    );
    run_all(
        experiment,
        &mut catalog,
        &held_out,
        &runs_dir,
        options,
        &host,
        &mut emit,
    )?;

    let runs = catalog.runs()?;
    let report = Report::of(experiment, &runs, selection)?;
    let text = report.to_json()?;
    let report_path = directory.join(REPORT_FILE);
    write_durably(&report_path, text.as_bytes())?;
    let complete: Vec<_> = report
        .runs
        .iter()
        .filter(|run| run.result.complete())
        .collect();
    emit(&ExperimentRecord::ExperimentEnd {
        schema: EXPERIMENT_SCHEMA,
        runs: report.runs.len(),
        complete: complete.len(),
        extinct: complete.iter().filter(|run| run.result.extinct).count(),
        report: report_path.display().to_string(),
        report_sha256: Sha256Digest::of(text.as_bytes()).to_hex(),
    });
    Ok(report)
}

fn plan(first: u64, conditions: &[usize], split: &'static str, seeds: &[u64]) -> Vec<Planned> {
    let mut planned = Vec::with_capacity(conditions.len() * seeds.len());
    for &condition in conditions {
        for &seed in seeds {
            planned.push(Planned {
                ordinal: first + wide(planned.len()) + 1,
                condition,
                split,
                seed,
            });
        }
    }
    planned
}

fn run_all(
    experiment: &Experiment,
    catalog: &mut ExperimentCatalog,
    planned: &[Planned],
    runs_dir: &Path,
    options: ExperimentOptions,
    host: &Host,
    emit: &mut impl FnMut(&ExperimentRecord),
) -> Result<(), CoreError> {
    let cataloged: BTreeSet<u64> = catalog.runs()?.iter().map(|run| run.ordinal).collect();
    for &run in planned {
        if cataloged.contains(&run.ordinal) {
            continue;
        }
        let cataloged = run_one(experiment, run, runs_dir, options, host)?;
        catalog.record(&cataloged.row)?;
        emit(&ExperimentRecord::ExperimentRun {
            schema: EXPERIMENT_SCHEMA,
            ordinal: run.ordinal,
            condition: cataloged.row.condition.clone(),
            split: run.split,
            seed: run.seed,
            result: Box::new(cataloged.result),
            wall_seconds: cataloged.wall_seconds,
        });
    }
    Ok(())
}

struct Ran {
    row: CatalogedRun,
    result: RunResult,
    wall_seconds: f64,
}

/// Run one planned run and write its records; the caller catalogs it.
fn run_one(
    experiment: &Experiment,
    planned: Planned,
    runs_dir: &Path,
    options: ExperimentOptions,
    host: &Host,
) -> Result<Ran, CoreError> {
    let condition = experiment
        .conditions
        .get(planned.condition)
        .ok_or_else(|| spec_error("conditions", format!("no condition {}", planned.condition)))?;
    let lines = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&lines);
    let last_metrics: Arc<Mutex<Option<Metrics>>> = Arc::new(Mutex::new(None));
    let last_sink = Arc::clone(&last_metrics);
    let clock = Instant::now();
    let config = experiment.configuration(planned.condition, planned.seed);
    let finished = config.as_ref().map_err(Clone::clone).and_then(|config| {
        crate::run::run_with(
            config,
            RunOptions {
                ticks: experiment.spec.ticks,
                every: 0,
                cpu_workers: options.cpu_workers,
                controllers: options.controllers,
                metrics_every: experiment.spec.metrics_every,
            },
            &[],
            None,
            move |record| {
                if let Record::Metrics(metrics) = record {
                    *last_sink.lock().unwrap_or_else(PoisonError::into_inner) =
                        Some(metrics.as_ref().clone());
                }
                if let Ok(line) = serde_json::to_string(record) {
                    let mut held = sink.lock().unwrap_or_else(PoisonError::into_inner);
                    held.push_str(&line);
                    held.push('\n');
                }
            },
        )
    });
    let wall_seconds = clock.elapsed().as_secs_f64();
    let result = match &finished {
        Ok(finished) => RunResult::of(
            finished,
            last_metrics
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref(),
        ),
        Err(error) => RunResult::error(error),
    };
    let mut records = std::mem::take(&mut *lines.lock().unwrap_or_else(PoisonError::into_inner));
    if let Err(error) = &finished {
        let line = serde_json::json!({
            "schema": crate::ERROR_SCHEMA,
            "kind": error.kind(),
            "detail": error.to_string(),
        });
        records.push_str(&line.to_string());
        records.push('\n');
    }
    let relative = format!("{RUNS_DIR}/run-{:06}.ndjson", planned.ordinal);
    let records_path = runs_dir.join(format!("run-{:06}.ndjson", planned.ordinal));
    write_durably(&records_path, records.as_bytes())?;
    let records_sha256 = Sha256Digest::of(records.as_bytes()).to_hex();
    let finished = finished.ok();
    let controllers = finished
        .as_ref()
        .map(|finished| ControllerSummary::of(&finished.closed.world));
    let manifest = manifest(
        Placement {
            experiment,
            ordinal: planned.ordinal,
            condition,
            split: planned.split,
            seed: planned.seed,
        },
        config.as_ref().ok(),
        &result,
        &Execution {
            wall_seconds,
            cpu_workers: finished
                .as_ref()
                .map(|finished| finished.closed.world.cpu_workers()),
            controllers: controllers.as_ref(),
            records: &relative,
            records_sha256: &records_sha256,
        },
        host,
    );
    Ok(Ran {
        row: CatalogedRun {
            ordinal: planned.ordinal,
            condition: condition.name.clone(),
            split: planned.split.to_owned(),
            seed: planned.seed,
            config_hash: config
                .as_ref()
                .map_or_else(|_| String::new(), |config| config.config_hash().to_hex()),
            status: result.status.clone(),
            result: json(&result)?,
            manifest: json(&manifest)?,
            records_sha256,
        },
        result,
        wall_seconds,
    })
}

/// `value` as compact JSON.
fn json(value: &impl Serialize) -> Result<String, CoreError> {
    serde_json::to_string(value).map_err(|error| CoreError::StateConflict {
        operation: "serialize experiment record",
        detail: error.to_string(),
    })
}

// ------------------------------------------------------------------ report

/// What a selection rule picked, and from what (§15.5).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Selection {
    /// The procedure, in the specification's words.
    pub procedure: String,
    /// The rule, if the specification has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<Select>,
    /// The condition the rule picked; none when no candidate had a
    /// complete discovery run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
    /// Each candidate's mean of the rule's measure over its complete
    /// discovery runs, and how many there were.
    pub candidates: Vec<Candidate>,
}

/// One condition a selection rule considered.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidate {
    /// The condition.
    pub condition: String,
    /// Its complete discovery runs.
    pub runs: usize,
    /// The measure's mean over them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean: Option<f64>,
}

fn mean(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sum = 0.0;
    for value in values {
        sum += value;
    }
    Some(sum / real(wide(values.len())))
}

fn results(runs: &[CatalogedRun]) -> Result<Vec<(CatalogedRun, RunResult)>, CoreError> {
    runs.iter()
        .map(|run| {
            let result: RunResult =
                serde_json::from_str(&run.result).map_err(|error| CoreError::StateConflict {
                    operation: "read experiment catalog",
                    detail: format!("run {}'s result: {error}", run.ordinal),
                })?;
            Ok((run.clone(), result))
        })
        .collect()
}

fn measure_of(name: &str) -> Option<Read> {
    MEASURES
        .iter()
        .find(|measure| measure.name == name)
        .map(|measure| measure.read)
}

/// Apply the specification's selection rule to the cataloged discovery
/// runs; none without a rule.
fn select(experiment: &Experiment, runs: &[CatalogedRun]) -> Result<Option<Selection>, CoreError> {
    let Some(rule) = &experiment.spec.select else {
        return Ok(None);
    };
    let read = measure_of(&rule.measure).ok_or_else(|| {
        spec_error(
            "select.measure",
            format!("`{}` is not a measure", rule.measure),
        )
    })?;
    let results = results(runs)?;
    let mut candidates = Vec::new();
    let mut best: Option<(f64, &str)> = None;
    for (index, condition) in experiment.conditions.iter().enumerate() {
        if index == experiment.control {
            continue;
        }
        let values: Vec<f64> = results
            .iter()
            .filter(|(run, result)| {
                run.condition == condition.name && run.split == DISCOVERY && result.complete()
            })
            .filter_map(|(_, result)| result.measures.as_ref().and_then(read))
            .collect();
        let average = mean(&values);
        if let Some(average) = average {
            let better = best.is_none_or(|(held, _)| match rule.rule.as_str() {
                "min" => average < held,
                _ => average > held,
            });
            if better {
                best = Some((average, condition.name.as_str()));
            }
        }
        candidates.push(Candidate {
            condition: condition.name.clone(),
            runs: values.len(),
            mean: average,
        });
    }
    Ok(Some(Selection {
        procedure: experiment.spec.selection.clone(),
        rule: Some(rule.clone()),
        selected: best.map(|(_, name)| name.to_owned()),
        candidates,
    }))
}

/// One cataloged run, as the report lists it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReportedRun {
    /// Its ordinal.
    pub ordinal: u64,
    /// Its condition.
    pub condition: String,
    /// `discovery` or `held-out`.
    pub split: String,
    /// Its seed.
    pub seed: u64,
    /// Its configuration's canonical hash.
    pub config_hash: String,
    /// Its result.
    pub result: RunResult,
}

/// One measure's paired comparison of a condition with the control.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Effect {
    /// Paired seeds with the measure defined in both runs.
    pub n: usize,
    /// The control's mean over them.
    pub control_mean: f64,
    /// The condition's mean over them.
    pub condition_mean: f64,
    /// The mean of condition minus control, seed by seed.
    pub mean_difference: f64,
    /// Seeds where the condition is higher.
    pub higher: usize,
    /// Seeds where it is lower.
    pub lower: usize,
    /// Seeds where they are equal.
    pub equal: usize,
    /// The exact two-sided sign-flip p-value of the mean difference over
    /// every `2^n` sign assignment; absent above
    /// [`EXACT_TEST_MAX_SEEDS`] seeds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub p_two_sided: Option<f64>,
    /// Condition minus control, seed by seed, in seed order.
    pub differences: Vec<f64>,
    /// Paired seeds left out because the measure is undefined in either
    /// run (a sampled statistic of an empty or uniform world), in order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub undefined_seeds: Vec<u64>,
}

/// One condition compared with the control over one split.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Comparison {
    /// `discovery` or `held-out`.
    pub split: String,
    /// The condition.
    pub condition: String,
    /// The control.
    pub control: String,
    /// The paired seeds, in order.
    pub seeds: Vec<u64>,
    /// Seeds left out because either run did not reach its tick, or one
    /// was not run.
    pub unpaired_seeds: Vec<u64>,
    /// Each measure's effect, in [`MEASURES`] order.
    pub effects: BTreeMap<&'static str, Effect>,
}

/// Runs per condition and split, by how they ended.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    /// Runs cataloged.
    pub runs: usize,
    /// Runs that reached their tick.
    pub reached: usize,
    /// Of those, extinct ones.
    pub extinct: usize,
    /// Runs paused at a capacity limit.
    pub capacity_paused: usize,
    /// Runs stopped by a failed tick.
    pub failed: usize,
    /// Runs refused or broken before their end record.
    pub error: usize,
}

/// An experiment's report: only scientific content, computed from its
/// catalog.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    /// [`REPORT_SCHEMA`].
    pub schema: &'static str,
    /// The experiment's name.
    pub name: String,
    /// Its question or hypothesis.
    pub hypothesis: String,
    /// Its identity.
    pub experiment: String,
    /// The base configuration's canonical hash.
    pub base_config_hash: String,
    /// Ticks per run.
    pub ticks: u64,
    /// The seeds.
    pub seeds: Seeds,
    /// The conditions as resolved.
    pub conditions: Vec<Condition>,
    /// The control.
    pub control: String,
    /// How the runs a claim rests on were chosen.
    pub selection: Selection,
    /// Run counts by condition, then split.
    pub counts: BTreeMap<String, BTreeMap<String, Counts>>,
    /// Each measure compared, in order, with its basis (§15.2).
    pub measures: Vec<ReportedMeasure>,
    /// Every comparison of a condition with the control, discovery first.
    pub comparisons: Vec<Comparison>,
    /// Every cataloged run, in run order: none left out.
    pub runs: Vec<ReportedRun>,
}

/// A measure as the report declares it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReportedMeasure {
    /// Its name.
    pub name: &'static str,
    /// What it is computed from.
    pub basis: &'static str,
}

/// The exact two-sided sign-flip p-value of the mean of `differences`.
/// Assignments within `1e-12` relative of the observed magnitude count as
/// at least as extreme, so the observed one always counts.
#[must_use]
pub fn sign_flip_p(differences: &[f64]) -> Option<f64> {
    let n = differences.len();
    if n == 0 || n > EXACT_TEST_MAX_SEEDS {
        return None;
    }
    let observed: f64 = differences.iter().sum::<f64>().abs();
    let threshold = observed - observed * 1.0e-12;
    let total: u64 = 1 << n;
    let mut extreme: u64 = 0;
    for mask in 0..total {
        let mut sum = 0.0;
        for (index, difference) in differences.iter().enumerate() {
            if mask >> index & 1 == 1 {
                sum -= difference;
            } else {
                sum += difference;
            }
        }
        if sum.abs() >= threshold {
            extreme += 1;
        }
    }
    Some(real(extreme) / real(total))
}

fn effect(pairs: &[(f64, f64)], undefined_seeds: Vec<u64>) -> Effect {
    let differences: Vec<f64> = pairs
        .iter()
        .map(|(control, condition)| condition - control)
        .collect();
    let controls: Vec<f64> = pairs.iter().map(|pair| pair.0).collect();
    let conditions: Vec<f64> = pairs.iter().map(|pair| pair.1).collect();
    Effect {
        n: pairs.len(),
        control_mean: mean(&controls).unwrap_or(0.0),
        condition_mean: mean(&conditions).unwrap_or(0.0),
        mean_difference: mean(&differences).unwrap_or(0.0),
        higher: differences.iter().filter(|d| **d > 0.0).count(),
        lower: differences.iter().filter(|d| **d < 0.0).count(),
        equal: differences.iter().filter(|d| **d == 0.0).count(),
        p_two_sided: sign_flip_p(&differences),
        differences,
        undefined_seeds,
    }
}

impl Report {
    /// The report of `experiment` over its cataloged `runs`, with the
    /// selection already made.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for a malformed cataloged result.
    pub fn of(
        experiment: &Experiment,
        runs: &[CatalogedRun],
        selection: Option<Selection>,
    ) -> Result<Self, CoreError> {
        let results = results(runs)?;
        let control = &experiment.conditions[experiment.control].name;
        let mut counts: BTreeMap<String, BTreeMap<String, Counts>> = BTreeMap::new();
        for (run, result) in &results {
            let count = counts
                .entry(run.condition.clone())
                .or_default()
                .entry(run.split.clone())
                .or_default();
            count.runs += 1;
            match result.status.as_str() {
                "reached" => {
                    count.reached += 1;
                    count.extinct += usize::from(result.extinct);
                }
                "capacity-paused" => count.capacity_paused += 1,
                "failed" => count.failed += 1,
                _ => count.error += 1,
            }
        }
        let find = |condition: &str, split: &str, seed: u64| {
            results
                .iter()
                .find(|(run, _)| {
                    run.condition == condition && run.split == split && run.seed == seed
                })
                .map(|(_, result)| result)
        };
        let mut comparisons = Vec::new();
        for (split, seeds) in [
            (DISCOVERY, &experiment.spec.seeds.discovery),
            (HELD_OUT, &experiment.spec.seeds.held_out),
        ] {
            for condition in &experiment.conditions {
                if &condition.name == control {
                    continue;
                }
                let ran = seeds
                    .iter()
                    .any(|&seed| find(&condition.name, split, seed).is_some());
                if !ran {
                    continue;
                }
                let mut paired = Vec::new();
                let mut unpaired = Vec::new();
                for &seed in seeds {
                    match (
                        find(control, split, seed),
                        find(&condition.name, split, seed),
                    ) {
                        (Some(a), Some(b)) if a.complete() && b.complete() => {
                            if let (Some(a), Some(b)) = (a.measures, b.measures) {
                                paired.push((seed, a, b));
                            } else {
                                unpaired.push(seed);
                            }
                        }
                        _ => unpaired.push(seed),
                    }
                }
                let effects = MEASURES
                    .iter()
                    .map(|measure| {
                        let mut pairs = Vec::new();
                        let mut undefined = Vec::new();
                        for (seed, a, b) in &paired {
                            match ((measure.read)(a), (measure.read)(b)) {
                                (Some(a), Some(b)) => pairs.push((a, b)),
                                _ => undefined.push(*seed),
                            }
                        }
                        (measure.name, effect(&pairs, undefined))
                    })
                    .collect();
                comparisons.push(Comparison {
                    split: split.to_owned(),
                    condition: condition.name.clone(),
                    control: control.clone(),
                    seeds: paired.iter().map(|(seed, _, _)| *seed).collect(),
                    unpaired_seeds: unpaired,
                    effects,
                });
            }
        }
        Ok(Self {
            schema: REPORT_SCHEMA,
            name: experiment.spec.name.clone(),
            hypothesis: experiment.spec.hypothesis.clone(),
            experiment: experiment.identity.to_hex(),
            base_config_hash: experiment.base_hash.to_hex(),
            ticks: experiment.spec.ticks,
            seeds: experiment.spec.seeds.clone(),
            conditions: experiment.conditions.clone(),
            control: control.clone(),
            selection: selection.unwrap_or_else(|| Selection {
                procedure: experiment.spec.selection.clone(),
                rule: None,
                selected: None,
                candidates: Vec::new(),
            }),
            counts,
            measures: MEASURES
                .iter()
                .map(|measure| ReportedMeasure {
                    name: measure.name,
                    basis: measure.basis,
                })
                .collect(),
            comparisons,
            runs: results
                .into_iter()
                .map(|(run, result)| ReportedRun {
                    ordinal: run.ordinal,
                    condition: run.condition,
                    split: run.split,
                    seed: run.seed,
                    config_hash: run.config_hash,
                    result,
                })
                .collect(),
        })
    }

    /// The report as pretty-printed JSON, ending in a newline.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] if it cannot be serialized.
    pub fn to_json(&self) -> Result<String, CoreError> {
        let mut text =
            serde_json::to_string_pretty(self).map_err(|error| CoreError::StateConflict {
                operation: "serialize experiment report",
                detail: error.to_string(),
            })?;
        text.push('\n');
        Ok(text)
    }
}
