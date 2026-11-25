//! `flife analyze`: what several experiments' catalogs say together (plan
//! §14.6, §15.2–§15.5), read-only.
//!
//! [`analyze`] opens each experiment directory's catalog
//! ([`EXPERIMENT_CATALOG_FILE`]) only to read it: the design it was created
//! with and every cataloged run's result. Nothing is run or written.
//!
//! - Experiments are comparable only when their designs share the base
//!   configuration, ticks, metrics interval, measures and control. Anything
//!   else is refused, naming the difference.
//! - A condition named in several experiments is the same condition only if
//!   each defines it the same way (its ablation and every key it sets).
//! - For every condition and split, the paired differences from the control,
//!   seed by seed, are pooled across the experiments that ran it, in the
//!   order given. The pooled effect comes from the experiment report's own
//!   statistics: mean paired difference, rise/fall/tie counts and the exact
//!   sign-flip p-value up to
//!   [`EXACT_TEST_MAX_SEEDS`](crate::experiment::EXACT_TEST_MAX_SEEDS) pairs.
//! - Each experiment's own count and mean difference per measure are listed
//!   beside the pooled effect, as its report has them.
//! - A seed may enter one condition and split only once. Under one base
//!   configuration a seed's run is the same run in every experiment, so a
//!   seed pooled twice would be counted twice.
//! - A seed is in one split across all the experiments. Each experiment runs
//!   its control on every seed it has, so a discovery seed of one that is a
//!   held-out seed of another would put the same control run in both
//!   splits. The held-out comparison would then not be held out.
//!
//! The analysis is one versioned JSON record ([`ANALYSIS_SCHEMA`]) and
//! depends on the catalogs alone: no path, host or time. The same catalogs
//! give the same bytes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use flife_core::CoreError;
use flife_core::canonical::Sha256Digest;
use flife_store::{CatalogedRun, ExperimentCatalog};
use serde::{Deserialize, Serialize};

use crate::experiment::{
    DISCOVERY, EXPERIMENT_CATALOG_FILE, EXPERIMENT_SCHEMA, Effect, HELD_OUT, MEASURES,
    ReportedMeasure, RunResult, Seeds, effects, paired, ran_in, results,
};

/// The analysis's schema.
pub const ANALYSIS_SCHEMA: &str = "frankenlife.analysis.v1";

/// The part of an experiment's cataloged design an analysis reads.
#[derive(Debug, Deserialize)]
struct Design {
    schema: String,
    name: String,
    base: String,
    ticks: u64,
    metrics_every: u64,
    control: String,
    seeds: Seeds,
    conditions: Vec<DesignedCondition>,
    measures: Vec<String>,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip_serializing_if requires &bool"
)]
const fn is_false(value: &bool) -> bool {
    !*value
}

/// A condition as an experiment's design resolved it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DesignedCondition {
    /// Its name.
    pub name: String,
    /// The named ablation, if any.
    pub ablation: Option<String>,
    /// Every key it sets, the shared overrides included, as TOML values.
    pub set: BTreeMap<String, String>,
    /// Whether all 21 `field-ecology-v2` features are disabled first (`--v1-compat`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub v1_compat: bool,
    /// Per-condition disabled `field-ecology-v2` feature slugs or aliases.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disable: Vec<String>,
    /// Per-condition enabled `field-ecology-v2` feature slugs or aliases.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enable: Vec<String>,
}

/// One experiment analyzed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnalyzedExperiment {
    /// Its identity: the SHA-256 of its cataloged design.
    pub experiment: String,
    /// Its name.
    pub name: String,
    /// Its seeds.
    pub seeds: Seeds,
    /// Runs cataloged.
    pub runs: usize,
}

/// One experiment's own effect of a measure, as its report has it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Part {
    /// Its pairs with the measure defined.
    pub n: usize,
    /// Its mean of condition minus control.
    pub mean_difference: f64,
}

/// One condition compared with the control over one split, pooled across
/// the experiments that ran it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Pooled {
    /// `discovery` or `held-out`.
    pub split: String,
    /// The condition.
    pub condition: String,
    /// The control.
    pub control: String,
    /// The experiments that ran it in this split, in the order given.
    pub experiments: Vec<String>,
    /// Each of those experiments' paired seeds, in order.
    pub seeds: Vec<Vec<u64>>,
    /// Each one's seeds left out because either run did not reach its
    /// tick, or one was not run.
    pub unpaired_seeds: Vec<Vec<u64>>,
    /// Each measure's effect over every pair, the experiments' in order.
    pub effects: BTreeMap<&'static str, Effect>,
    /// Each measure's effect in each experiment alone, aligned with
    /// `experiments`.
    pub parts: BTreeMap<&'static str, Vec<Part>>,
}

/// What several experiments' catalogs say together.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Analysis {
    /// [`ANALYSIS_SCHEMA`].
    pub schema: &'static str,
    /// The shared base configuration's canonical hash.
    pub base_config_hash: String,
    /// The shared ticks per run.
    pub ticks: u64,
    /// The shared metrics interval.
    pub metrics_every: u64,
    /// The shared control.
    pub control: String,
    /// The experiments, in the order given.
    pub experiments: Vec<AnalyzedExperiment>,
    /// Every condition any of them defines, in first appearance.
    pub conditions: Vec<DesignedCondition>,
    /// Each measure compared, with its basis.
    pub measures: Vec<ReportedMeasure>,
    /// Every condition compared with the control, discovery first, in
    /// condition order.
    pub comparisons: Vec<Pooled>,
}

fn refused(detail: impl Into<String>) -> CoreError {
    CoreError::StateConflict {
        operation: "analyze experiments",
        detail: detail.into(),
    }
}

/// One experiment as read from its catalog.
struct Loaded {
    identity: String,
    design: Design,
    results: Vec<(CatalogedRun, RunResult)>,
}

fn load(directory: &Path) -> Result<Loaded, CoreError> {
    let path = directory.join(EXPERIMENT_CATALOG_FILE);
    let catalog = ExperimentCatalog::read(&path)?;
    let text = catalog
        .design()?
        .ok_or_else(|| refused(format!("{} records no experiment design", path.display())))?;
    let design: Design = serde_json::from_str(&text).map_err(|error| {
        refused(format!(
            "{}'s experiment design is unreadable: {error}",
            path.display()
        ))
    })?;
    if design.schema != EXPERIMENT_SCHEMA {
        return Err(CoreError::SchemaMismatch {
            expected: EXPERIMENT_SCHEMA.into(),
            found: design.schema,
        });
    }
    Ok(Loaded {
        identity: Sha256Digest::of(text.as_bytes()).to_hex(),
        design,
        results: results(&catalog.runs()?)?,
    })
}

/// Check that `loaded` can be pooled: one base, ticks, metrics interval,
/// control and this build's measures, and no experiment twice.
fn check_comparable(loaded: &[Loaded]) -> Result<(), CoreError> {
    let measures: Vec<&str> = MEASURES.iter().map(|measure| measure.name).collect();
    let Some(first) = loaded.first() else {
        return Err(refused("no experiment to analyze"));
    };
    let mut identities = BTreeSet::new();
    for experiment in loaded {
        let design = &experiment.design;
        let named = || format!("experiment `{}` ({})", design.name, experiment.identity);
        if !identities.insert(experiment.identity.as_str()) {
            return Err(refused(format!("{} is given twice", named())));
        }
        if design.measures != measures {
            return Err(refused(format!(
                "{} compares other measures than this build: {:?}",
                named(),
                design.measures
            )));
        }
        let shared = &first.design;
        for (what, this, that) in [
            (
                "base configuration",
                design.base.clone(),
                shared.base.clone(),
            ),
            ("ticks", design.ticks.to_string(), shared.ticks.to_string()),
            (
                "metrics interval",
                design.metrics_every.to_string(),
                shared.metrics_every.to_string(),
            ),
            ("control", design.control.clone(), shared.control.clone()),
        ] {
            if this != that {
                return Err(refused(format!(
                    "{} has {what} {this}, the first experiment {that}: not comparable",
                    named()
                )));
            }
        }
    }
    Ok(())
}

/// Check that no seed is a discovery seed of one experiment and a held-out
/// seed of another. One experiment's own splits are disjoint already
/// (`flife experiment` refuses a seed in both).
fn check_splits(loaded: &[Loaded]) -> Result<(), CoreError> {
    let mut discovery: BTreeMap<u64, &Loaded> = BTreeMap::new();
    for experiment in loaded {
        for &seed in &experiment.design.seeds.discovery {
            discovery.entry(seed).or_insert(experiment);
        }
    }
    for experiment in loaded {
        for seed in &experiment.design.seeds.held_out {
            if let Some(other) = discovery.get(seed) {
                return Err(refused(format!(
                    "seed {seed} is held out in experiment `{}` ({}) and a discovery seed of \
                     experiment `{}` ({}): its control run would be in both splits, so the \
                     held-out comparison would not be held out",
                    experiment.design.name, experiment.identity, other.design.name, other.identity
                )));
            }
        }
    }
    Ok(())
}

/// Every condition the experiments define, each once, in first appearance;
/// refused if two define one name differently.
fn conditions(loaded: &[Loaded]) -> Result<Vec<DesignedCondition>, CoreError> {
    let mut all: Vec<DesignedCondition> = Vec::new();
    for experiment in loaded {
        for condition in &experiment.design.conditions {
            match all.iter().find(|known| known.name == condition.name) {
                Some(known) if known != condition => {
                    return Err(refused(format!(
                        "condition `{}` is defined differently in experiment `{}` ({}): not the \
                         same condition",
                        condition.name, experiment.design.name, experiment.identity
                    )));
                }
                Some(_) => {}
                None => all.push(condition.clone()),
            }
        }
    }
    Ok(all)
}

/// `condition` compared with `control` over `split`, pooled across every
/// experiment that ran it there; `None` if none did.
fn pool(
    loaded: &[Loaded],
    control: &str,
    condition: &str,
    split: &str,
) -> Result<Option<Pooled>, CoreError> {
    let mut experiments = Vec::new();
    let mut seeds = Vec::new();
    let mut unpaired_seeds = Vec::new();
    let mut parts: BTreeMap<&'static str, Vec<Part>> = BTreeMap::new();
    let mut pooled = Vec::new();
    let mut pooled_by: BTreeMap<u64, &str> = BTreeMap::new();
    for experiment in loaded {
        let split_seeds = if split == DISCOVERY {
            &experiment.design.seeds.discovery
        } else {
            &experiment.design.seeds.held_out
        };
        if !ran_in(&experiment.results, condition, split, split_seeds) {
            continue;
        }
        let (pairs, unpaired) = paired(&experiment.results, control, condition, split, split_seeds);
        for (seed, _, _) in &pairs {
            if let Some(other) = pooled_by.insert(*seed, &experiment.identity) {
                return Err(refused(format!(
                    "seed {seed} of `{condition}` ({split}) is in experiments {other} and {}: \
                     under one base configuration it is the same run, so it would be counted twice",
                    experiment.identity
                )));
            }
        }
        for (measure, effect) in effects(&pairs) {
            parts.entry(measure).or_default().push(Part {
                n: effect.n,
                mean_difference: effect.mean_difference,
            });
        }
        experiments.push(experiment.identity.clone());
        seeds.push(pairs.iter().map(|(seed, _, _)| *seed).collect());
        unpaired_seeds.push(unpaired);
        pooled.extend(pairs);
    }
    if experiments.is_empty() {
        return Ok(None);
    }
    Ok(Some(Pooled {
        split: split.to_owned(),
        condition: condition.to_owned(),
        control: control.to_owned(),
        experiments,
        seeds,
        unpaired_seeds,
        effects: effects(&pooled),
        parts,
    }))
}

/// Analyze the experiments in `directories` together (`flife analyze`).
///
/// # Errors
/// [`CoreError::StateConflict`] for fewer than two directories, an
/// experiment given twice, experiments that are not comparable, a condition
/// defined two ways, a seed in both splits, a seed pooled twice, or a
/// malformed catalog;
/// [`CoreError::StorageFailed`] for a directory without a readable catalog;
/// [`CoreError::SchemaMismatch`] for a catalog or design of another format.
pub fn analyze(directories: &[&Path]) -> Result<Analysis, CoreError> {
    if directories.len() < 2 {
        return Err(refused("an analysis compares at least two experiments"));
    }
    let loaded = directories
        .iter()
        .map(|directory| load(directory))
        .collect::<Result<Vec<_>, _>>()?;
    check_comparable(&loaded)?;
    check_splits(&loaded)?;
    let conditions = conditions(&loaded)?;
    let shared = &loaded[0].design;
    let mut comparisons = Vec::new();
    for split in [DISCOVERY, HELD_OUT] {
        for condition in &conditions {
            if condition.name == shared.control {
                continue;
            }
            if let Some(pooled) = pool(&loaded, &shared.control, &condition.name, split)? {
                comparisons.push(pooled);
            }
        }
    }
    Ok(Analysis {
        schema: ANALYSIS_SCHEMA,
        base_config_hash: shared.base.clone(),
        ticks: shared.ticks,
        metrics_every: shared.metrics_every,
        control: shared.control.clone(),
        experiments: loaded
            .iter()
            .map(|experiment| AnalyzedExperiment {
                experiment: experiment.identity.clone(),
                name: experiment.design.name.clone(),
                seeds: experiment.design.seeds.clone(),
                runs: experiment.results.len(),
            })
            .collect(),
        conditions,
        measures: MEASURES
            .iter()
            .map(|measure| ReportedMeasure {
                name: measure.name,
                basis: measure.basis,
            })
            .collect(),
        comparisons,
    })
}
