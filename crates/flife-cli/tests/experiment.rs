//! FL-041 evidence (plan §15.3–§15.5, §21.6, Appendix B.1): repeatable
//! comparisons and a complete run catalog.
//!
//! - The same experiment reports byte-identical results at any worker
//!   count, with or without the Metal controller backend, and when it is
//!   killed mid-way and continued: the report depends on the catalog alone,
//!   and the catalog on the specification alone.
//! - The report's comparisons equal paired differences, seed counts and an
//!   exact sign-flip test recomputed here, from the listed runs, by other
//!   code.
//! - Every run is cataloged whatever its ending (reached, extinct, paused
//!   at a capacity limit, refused), listed, counted and kept out of the
//!   paired differences only by name; each has its records and a run
//!   manifest naming its configuration, seed, source, host and artifacts.
//! - A selection rule picks from the discovery runs alone, and only the
//!   control and the pick run on the held-out seeds.
//! - Each named ablation changes what it names, read from the runs' own
//!   end and metrics records.
//! - A specification that would leave a choice hidden is refused.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::cast_precision_loss,
    reason = "test helpers and oracles: a failure in one is a test failure, and counts here are \
              far below 2^53"
)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use flife_cli::experiment::{DISCOVERY, EXACT_TEST_MAX_SEEDS, HELD_OUT, RUNS_DIR, sign_flip_p};
use flife_cli::{
    ABLATIONS, ControllerChoice, EXPERIMENT_CATALOG_FILE, Experiment, ExperimentOptions,
    ExperimentRecord, MANIFEST_SCHEMA, MEASURES, REPORT_FILE, Report, RunResult, Spec,
};
use flife_core::{CoreError, Sha256Digest};
use flife_store::{CatalogedRun, ExperimentCatalog};
use serde_json::Value;

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");
const CARGO_LOCK: &str = include_str!("../../../Cargo.lock");

/// Appendix A, as the base configuration every experiment here starts from.
fn appendix_a() -> &'static str {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    &appendix[start..start + length]
}

/// The FL-013 32 × 32 test ecology as shared overrides, volatile.
const ECOLOGY: &str = r#"
[set]
"world.width" = 32.0
"world.height" = 32.0
"world.grid_width" = 32
"world.grid_height" = 32
"population.initial_count" = 48
"population.maximum_count" = 1024
"population.initial_energy" = 3.0
"sources.count" = 4
"sources.power_per_source" = 120.0
"sources.sigma_world_units" = 3.0
"sources.path_direction_interval_seconds" = 2.0
"chemistry.uptake_capacity_per_second" = 8.0
"costs.basal_energy_per_second" = 0.6
"reproduction.minimum_parent_energy_before_attempt" = 2.2
"reproduction.maximum_attempt_rate_per_second" = 4.0
"reproduction.cooldown_seconds" = 0.25
"controller.evaluation_interval_ticks" = 4
"execution.command_durability" = "volatile-in-memory"
"#;

fn scratch(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("flife-experiment")
        .join(name);
    if path.exists() {
        fs::remove_dir_all(&path).expect("clears the last run's directory");
    }
    fs::create_dir_all(&path).expect("creates the scratch directory");
    path
}

/// Write `base.toml` (Appendix A) and `experiment.toml` (`header`, the
/// ecology, then `rest`) into `directory`; the specification's path.
fn write_spec(directory: &Path, header: &str, rest: &str) -> PathBuf {
    fs::write(directory.join("base.toml"), appendix_a()).expect("writes the base");
    let spec = directory.join("experiment.toml");
    let text = format!(
        "schema = \"frankenlife.experiment.v1\"\nbase = \"base.toml\"\n{header}\n{ECOLOGY}\n{rest}"
    );
    fs::write(&spec, text).expect("writes the specification");
    spec
}

fn run(spec: &Path, out: &Path, cpu_workers: usize, controllers: ControllerChoice) -> Report {
    let experiment = Experiment::load(spec).expect("the specification loads");
    flife_cli::experiment(
        &experiment,
        out,
        ExperimentOptions {
            cpu_workers: std::num::NonZeroUsize::new(cpu_workers),
            controllers,
        },
        |_| {},
    )
    .expect("the experiment runs")
}

fn catalog_runs(out: &Path, spec: &Path) -> Vec<CatalogedRun> {
    let experiment = Experiment::load(spec).expect("the specification loads");
    ExperimentCatalog::open(
        &out.join(EXPERIMENT_CATALOG_FILE),
        &experiment.identity().to_hex(),
        experiment.design(),
    )
    .expect("the catalog opens")
    .runs()
    .expect("the runs read")
}

const EMISSION: &str = r#"
name = "emission-and-decisions"
hypothesis = "H2 and H3: neural decisions and paid emission change the ecology"
ticks = 120
control = "control"
selection = "Every run of every condition is reported; none is excluded."

[seeds]
discovery = [1, 2, 3]
held_out = [11, 12]
"#;

const EMISSION_CONDITIONS: &str = r#"
[[conditions]]
name = "control"

[[conditions]]
name = "no-emission"
ablation = "no-emission"

[[conditions]]
name = "chemistry-only"
ablation = "chemistry-only"
"#;

/// Two's-complement-free brute force over sign assignments, by recursion:
/// how many give `|Σ ± d|` at least the observed magnitude.
fn extreme(differences: &[f64], index: usize, sum: f64, threshold: f64) -> u64 {
    if index == differences.len() {
        return u64::from(sum.abs() >= threshold);
    }
    extreme(differences, index + 1, sum + differences[index], threshold)
        + extreme(differences, index + 1, sum - differences[index], threshold)
}

/// Recompute every comparison of `report` from its run listing.
fn check_comparisons(report: &Report) {
    let find = |condition: &str, split: &str, seed: u64| {
        report
            .runs
            .iter()
            .find(|run| run.condition == condition && run.split == split && run.seed == seed)
            .map(|run| &run.result)
    };
    let mut expected = 0;
    for (split, seeds) in [
        (DISCOVERY, &report.seeds.discovery),
        (HELD_OUT, &report.seeds.held_out),
    ] {
        for condition in &report.conditions {
            if condition.name == report.control
                || !seeds
                    .iter()
                    .any(|&seed| find(&condition.name, split, seed).is_some())
            {
                continue;
            }
            expected += 1;
            let comparison = report
                .comparisons
                .iter()
                .find(|c| c.split == split && c.condition == condition.name)
                .expect("a comparison for every condition that ran");
            fn complete(result: Option<&RunResult>) -> Option<&RunResult> {
                result.filter(|result| result.status == "reached")
            }
            let paired: Vec<u64> = seeds
                .iter()
                .copied()
                .filter(|&seed| {
                    complete(find(&report.control, split, seed)).is_some()
                        && complete(find(&condition.name, split, seed)).is_some()
                })
                .collect();
            assert_eq!(comparison.seeds, paired);
            assert_eq!(comparison.unpaired_seeds.len() + paired.len(), seeds.len());
            for (name, read) in MEASURES {
                let effect = &comparison.effects[name];
                let pairs: Vec<(f64, f64)> = paired
                    .iter()
                    .map(|&seed| {
                        let a = find(&report.control, split, seed)
                            .and_then(|r| r.measures)
                            .expect("measures");
                        let b = find(&condition.name, split, seed)
                            .and_then(|r| r.measures)
                            .expect("measures");
                        (read(&a), read(&b))
                    })
                    .collect();
                let differences: Vec<f64> = pairs.iter().map(|(a, b)| b - a).collect();
                assert_eq!(
                    effect.differences, differences,
                    "{split} {} {name}",
                    condition.name
                );
                let n = differences.len();
                assert_eq!(effect.n, n);
                let mean = |values: &mut dyn Iterator<Item = f64>| {
                    let mut sum = 0.0;
                    for value in values {
                        sum += value;
                    }
                    sum / n as f64
                };
                if n > 0 {
                    assert_eq!(
                        effect.mean_difference,
                        mean(&mut differences.iter().copied())
                    );
                    assert_eq!(effect.control_mean, mean(&mut pairs.iter().map(|p| p.0)));
                    assert_eq!(effect.condition_mean, mean(&mut pairs.iter().map(|p| p.1)));
                    let observed = differences.iter().sum::<f64>().abs();
                    let count = extreme(&differences, 0, 0.0, observed - observed * 1.0e-12);
                    assert_eq!(
                        effect.p_two_sided,
                        Some(count as f64 / (1_u64 << n) as f64),
                        "{split} {} {name}",
                        condition.name
                    );
                }
                assert_eq!(
                    effect.higher,
                    differences.iter().filter(|d| **d > 0.0).count()
                );
                assert_eq!(
                    effect.lower,
                    differences.iter().filter(|d| **d < 0.0).count()
                );
                assert_eq!(
                    effect.equal,
                    differences.iter().filter(|d| **d == 0.0).count()
                );
            }
        }
    }
    assert_eq!(report.comparisons.len(), expected);
}

#[test]
fn repeated_experiments_report_byte_identically_at_any_worker_count_and_across_a_kill() {
    let root = scratch("repeat");
    let spec = write_spec(&root, EMISSION, EMISSION_CONDITIONS);
    let total = 3 * (3 + 2);

    let first = run(&spec, &root.join("one-worker"), 1, ControllerChoice::Cpu);
    let other = run(
        &spec,
        &root.join("three-workers"),
        3,
        ControllerChoice::Auto,
    );
    let one = fs::read(root.join("one-worker").join(REPORT_FILE)).expect("a report");
    let three = fs::read(root.join("three-workers").join(REPORT_FILE)).expect("a report");
    assert_eq!(
        one, three,
        "worker count and backend change no reported byte"
    );
    assert_eq!(first, other);
    assert_eq!(first.runs.len(), total);
    assert!(first.runs.iter().all(|run| run.result.status == "reached"));
    check_comparisons(&first);

    // Kill the binary once it has cataloged four runs, then continue.
    let killed = root.join("killed");
    let mut child = Command::new(env!("CARGO_BIN_EXE_flife"))
        .args(["experiment", "--cpu-workers", "2", "--out"])
        .arg(&killed)
        .arg(&spec)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("flife starts");
    let stdout = child.stdout.take().expect("its output");
    let mut ran = 0;
    for line in BufReader::new(stdout).lines() {
        if line
            .expect("a line")
            .contains("\"record\":\"experiment-run\"")
        {
            ran += 1;
            if ran == 4 {
                break;
            }
        }
    }
    child.kill().expect("SIGKILL");
    let status = child.wait().expect("the killed child");
    assert!(!status.success());
    let held = catalog_runs(&killed, &spec);
    assert!(
        held.len() >= 4 && held.len() < total,
        "{} cataloged",
        held.len()
    );
    assert!(!killed.join(REPORT_FILE).exists());

    let output = Command::new(env!("CARGO_BIN_EXE_flife"))
        .args(["experiment", "--cpu-workers", "4", "--out"])
        .arg(&killed)
        .arg(&spec)
        .output()
        .expect("flife runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).expect("NDJSON"))
        .collect();
    assert_eq!(lines[0]["record"], "experiment-start");
    assert_eq!(lines[0]["cataloged"], held.len());
    let rerun = lines
        .iter()
        .filter(|line| line["record"] == "experiment-run")
        .count();
    assert_eq!(
        rerun,
        total - held.len(),
        "cataloged runs are not run again"
    );
    let continued = fs::read(killed.join(REPORT_FILE)).expect("a report");
    assert_eq!(
        continued, one,
        "an interrupted experiment reports the same bytes"
    );
    let end = lines.last().expect("an end record");
    assert_eq!(
        end["report_sha256"],
        Sha256Digest::of(&continued).to_hex(),
        "the end record names the report's hash"
    );

    // Another experiment cannot write into this directory.
    let other_spec = write_spec(
        &root,
        &EMISSION.replace("ticks = 120", "ticks = 121"),
        EMISSION_CONDITIONS,
    );
    let refused = flife_cli::experiment(
        &Experiment::load(&other_spec).expect("loads"),
        &killed,
        ExperimentOptions {
            cpu_workers: None,
            controllers: ControllerChoice::Cpu,
        },
        |_| {},
    );
    assert!(
        matches!(refused, Err(CoreError::StateConflict { .. })),
        "{refused:?}"
    );
}

#[test]
fn the_catalog_keeps_every_run_whatever_its_ending() {
    let root = scratch("endings");
    let spec = write_spec(
        &root,
        r#"
name = "endings"
hypothesis = "Every ending is cataloged"
ticks = 120
metrics_every = 60
control = "control"
selection = "Every run of every condition is reported; none is excluded."

[seeds]
discovery = [1, 2]
held_out = [11]
"#,
        r#"
[[conditions]]
name = "control"

[[conditions]]
name = "starved"
[conditions.set]
"costs.basal_energy_per_second" = 40.0

[[conditions]]
name = "crowded"
[conditions.set]
"population.maximum_count" = 50

[[conditions]]
name = "journaled"
[conditions.set]
"execution.command_durability" = "write-ahead-before-apply"
"#,
    );
    let out = root.join("out");
    let mut records = Vec::new();
    let experiment = Experiment::load(&spec).expect("loads");
    let report = flife_cli::experiment(
        &experiment,
        &out,
        ExperimentOptions {
            cpu_workers: None,
            controllers: ControllerChoice::Cpu,
        },
        |record| records.push(record.clone()),
    )
    .expect("the experiment runs");

    let expected = [
        ("control", "reached", false),
        ("starved", "reached", true),
        ("crowded", "capacity-paused", false),
        ("journaled", "error", false),
    ];
    let rows = catalog_runs(&out, &spec);
    assert_eq!(rows.len(), 4 * 3, "every planned run is cataloged");
    assert_eq!(report.runs.len(), rows.len(), "and listed");
    let mut ordinal = 0;
    for split in [DISCOVERY, HELD_OUT] {
        let seeds: &[u64] = if split == DISCOVERY { &[1, 2] } else { &[11] };
        for (condition, status, extinct) in expected {
            for &seed in seeds {
                ordinal += 1;
                let row = &rows[ordinal - 1];
                assert_eq!(
                    (
                        row.ordinal,
                        row.condition.as_str(),
                        row.split.as_str(),
                        row.seed
                    ),
                    (ordinal as u64, condition, split, seed)
                );
                let result: RunResult = serde_json::from_str(&row.result).expect("a result");
                assert_eq!(result.status, status, "{condition} {seed}");
                assert_eq!(row.status, status);
                assert_eq!(result.extinct, extinct, "{condition} {seed}");
                match status {
                    "reached" => assert_eq!(result.tick, 120),
                    "capacity-paused" => {
                        assert!(result.tick < 120);
                        assert!(
                            result
                                .detail
                                .as_deref()
                                .is_some_and(|d| d.contains("PopulationSlots"))
                        );
                    }
                    _ => {
                        assert!(result.measures.is_none());
                        assert!(
                            result
                                .detail
                                .as_deref()
                                .is_some_and(|d| d.contains("command journal"))
                        );
                    }
                }
                if extinct {
                    let measures = result.measures.expect("measures");
                    assert_eq!((measures.survived, measures.final_live), (0, 0));
                }

                // The records file is the one the catalog hashes; its last
                // line is the end record, or the error.
                let text =
                    fs::read_to_string(out.join(RUNS_DIR).join(format!("run-{ordinal:06}.ndjson")))
                        .expect("a records file");
                assert_eq!(
                    Sha256Digest::of(text.as_bytes()).to_hex(),
                    row.records_sha256
                );
                let last: Value =
                    serde_json::from_str(text.lines().last().expect("a line")).expect("JSON");
                if status == "error" {
                    assert_eq!(last["schema"], "frankenlife.error.v1");
                } else {
                    assert_eq!(last["record"], "end");
                    assert_eq!(last["digest"], result.digest.clone().expect("a digest"));
                    assert_eq!(last["tick"], result.tick);
                    assert!(
                        text.lines()
                            .any(|line| line.contains("\"record\":\"metrics\""))
                    );
                }

                // The manifest (Appendix B.1).
                let manifest: Value = serde_json::from_str(&row.manifest).expect("a manifest");
                assert_eq!(manifest["schema"], MANIFEST_SCHEMA);
                assert_eq!(manifest["identity"]["condition"], condition);
                assert_eq!(manifest["identity"]["split"], split);
                assert_eq!(manifest["identity"]["seed"], seed);
                assert_eq!(manifest["identity"]["ordinal"], ordinal);
                assert_eq!(
                    manifest["identity"]["experiment_identity"],
                    experiment.identity().to_hex()
                );
                assert_eq!(manifest["model"]["config_hash"], row.config_hash);
                assert_eq!(manifest["provenance"]["seed"], seed);
                assert_eq!(
                    manifest["results"]["result"],
                    serde_json::to_value(&result).expect("JSON")
                );
                assert_eq!(
                    manifest["artifacts"][0]["path"],
                    format!("{RUNS_DIR}/run-{ordinal:06}.ndjson")
                );
                assert_eq!(manifest["artifacts"][0]["sha256"], row.records_sha256);
                assert!(manifest["hardware"]["host"]["logical_cores"].as_u64() >= Some(1));
                let suite = manifest["source"]["suite"].as_array().expect("suite pins");
                let asupersync = suite
                    .iter()
                    .filter_map(Value::as_str)
                    .find(|pin| pin.starts_with("asupersync@"))
                    .expect("the asupersync pin");
                let commit = asupersync.trim_start_matches("asupersync@");
                assert!(
                    CARGO_LOCK.contains(&format!("asupersync?rev={commit}#{commit}"))
                        || CARGO_LOCK.contains(&format!("#{commit}\"")),
                    "{asupersync} is Cargo.lock's"
                );
                assert_eq!(
                    manifest["source"]["profile"].as_str().is_some(),
                    manifest["source"]["target"].as_str().is_some()
                );
            }
        }
    }

    // Counts and comparisons name what they leave out.
    for (condition, status, extinct) in expected {
        for (split, runs) in [(DISCOVERY, 2), (HELD_OUT, 1)] {
            let counts = &report.counts[condition][split];
            assert_eq!(counts.runs, runs);
            let reached = if status == "reached" { runs } else { 0 };
            assert_eq!(counts.reached, reached);
            assert_eq!(counts.extinct, if extinct { runs } else { 0 });
            assert_eq!(
                counts.capacity_paused,
                if status == "capacity-paused" { runs } else { 0 }
            );
            assert_eq!(counts.error, if status == "error" { runs } else { 0 });
        }
    }
    check_comparisons(&report);
    for comparison in &report.comparisons {
        match comparison.condition.as_str() {
            "starved" => {
                assert_eq!(comparison.unpaired_seeds, Vec::<u64>::new());
                let survived = &comparison.effects["survived"];
                assert_eq!(survived.mean_difference, -1.0, "extinct runs are compared");
                assert_eq!(survived.lower, survived.n);
            }
            _ => {
                assert!(comparison.seeds.is_empty(), "{}", comparison.condition);
                assert!(!comparison.unpaired_seeds.is_empty());
            }
        }
    }
    let ended = records
        .iter()
        .filter(|record| matches!(record, ExperimentRecord::ExperimentRun { .. }))
        .count();
    assert_eq!(ended, rows.len());
}

#[test]
fn a_selection_rule_sees_only_discovery_runs_and_holds_out_the_rest() {
    let root = scratch("selection");
    let spec = write_spec(
        &root,
        r#"
name = "basal-sweep"
hypothesis = "Which maintenance cost keeps the largest population?"
ticks = 120
control = "control"
selection = "The sweep condition with the largest mean final population over the discovery seeds is evaluated on the held-out seeds against the control."

[seeds]
discovery = [1, 2]
held_out = [11, 12]

[select]
measure = "final_live"
rule = "max"
"#,
        r#"
[[conditions]]
name = "control"

[sweep]
"costs.basal_energy_per_second" = [0.3, 1.2]
"#,
    );
    let out = root.join("out");
    let report = run(&spec, &out, 2, ControllerChoice::Cpu);
    let written = fs::read(out.join(REPORT_FILE)).expect("a report");
    // Continuing a finished experiment runs nothing and selects again from
    // a catalog that now holds the held-out runs too: the same pick, the
    // same bytes.
    let mut ran = 0;
    let again = flife_cli::experiment(
        &Experiment::load(&spec).expect("loads"),
        &out,
        ExperimentOptions {
            cpu_workers: None,
            controllers: ControllerChoice::Cpu,
        },
        |record| ran += usize::from(matches!(record, ExperimentRecord::ExperimentRun { .. })),
    )
    .expect("continues");
    assert_eq!(ran, 0);
    assert_eq!(again, report);
    assert_eq!(fs::read(out.join(REPORT_FILE)).expect("a report"), written);
    let names: Vec<&str> = report.conditions.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "control",
            "costs.basal_energy_per_second=0.3",
            "costs.basal_energy_per_second=1.2"
        ]
    );
    let mean = |condition: &str| {
        let values: Vec<f64> = report
            .runs
            .iter()
            .filter(|run| run.condition == condition && run.split == DISCOVERY)
            .map(|run| run.result.measures.expect("measures").final_live as f64)
            .collect();
        values.iter().sum::<f64>() / values.len() as f64
    };
    let low = mean("costs.basal_energy_per_second=0.3");
    let high = mean("costs.basal_energy_per_second=1.2");
    assert_ne!(low, high);
    let expected = if low > high {
        "costs.basal_energy_per_second=0.3"
    } else {
        "costs.basal_energy_per_second=1.2"
    };
    let selection = &report.selection;
    assert_eq!(selection.selected.as_deref(), Some(expected));
    assert_eq!(selection.candidates.len(), 2);
    for candidate in &selection.candidates {
        assert_eq!(candidate.mean, Some(mean(&candidate.condition)));
        assert_eq!(candidate.runs, 2);
    }
    assert!(selection.procedure.starts_with("The sweep condition"));

    // Held out: the control and the pick, nothing else.
    let held_out: Vec<(&str, u64)> = report
        .runs
        .iter()
        .filter(|run| run.split == HELD_OUT)
        .map(|run| (run.condition.as_str(), run.seed))
        .collect();
    let mut planned = vec![
        ("control", 11),
        ("control", 12),
        (expected, 11),
        (expected, 12),
    ];
    planned.sort_by_key(|(condition, _)| names.iter().position(|name| name == condition));
    assert_eq!(held_out, planned);
    let held_out_comparisons: Vec<&str> = report
        .comparisons
        .iter()
        .filter(|c| c.split == HELD_OUT)
        .map(|c| c.condition.as_str())
        .collect();
    assert_eq!(held_out_comparisons, [expected]);
    check_comparisons(&report);
}

/// The end record and the last metrics record of a run's records file.
fn end_and_metrics(out: &Path, ordinal: u64) -> (Value, Value) {
    let text = fs::read_to_string(out.join(RUNS_DIR).join(format!("run-{ordinal:06}.ndjson")))
        .expect("records");
    let lines: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).expect("JSON"))
        .collect();
    let end = lines
        .iter()
        .find(|line| line["record"] == "end")
        .expect("an end record")
        .clone();
    let metrics = lines
        .iter()
        .rev()
        .find(|line| line["record"] == "metrics")
        .expect("a metrics record")
        .clone();
    (end, metrics)
}

#[test]
fn every_named_ablation_changes_what_it_names() {
    let root = scratch("ablations");
    let mut conditions = String::from("[[conditions]]\nname = \"control\"\n");
    for ablation in ABLATIONS {
        conditions.push_str(&format!(
            "\n[[conditions]]\nname = \"{0}\"\nablation = \"{0}\"\n",
            ablation.name
        ));
    }
    // Frequent topology changes give the fixed-topology control something
    // to remove.
    let spec = write_spec(
        &root,
        r#"
name = "ablations"
hypothesis = "Each ablation removes the mechanism it names"
ticks = 240
metrics_every = 240
control = "control"
selection = "Every run of every condition is reported; none is excluded."

[seeds]
discovery = [5]
"#,
        &conditions,
    );
    let text = fs::read_to_string(&spec)
        .expect("the specification")
        .replace(
            "\"controller.evaluation_interval_ticks\" = 4",
            "\"controller.evaluation_interval_ticks\" = 4\n\"architecture_mutation.probability_per_birth\" = 0.5",
        );
    fs::write(&spec, text).expect("rewrites it");
    let out = root.join("out");
    let report = run(&spec, &out, 2, ControllerChoice::Cpu);
    let by_name: BTreeMap<&str, &flife_cli::experiment::ReportedRun> = report
        .runs
        .iter()
        .map(|run| (run.condition.as_str(), run))
        .collect();
    let measures = |name: &str| by_name[name].result.measures.expect("measures");
    let flows =
        |name: &str| end_and_metrics(&out, by_name[name].ordinal).1["window"]["flows"].clone();
    let heredity =
        |name: &str| end_and_metrics(&out, by_name[name].ordinal).0["survey"]["heredity"].clone();
    assert!(report.runs.iter().all(|run| run.result.status == "reached"));

    let control = measures("control");
    assert!(control.births > 0 && control.emitter_ticks > 0 && control.topology_changes > 0);
    assert!(control.mutated_parameters > 0);

    // An open gate at a zero rate spends nothing.
    assert!(control.emission_paid > 0.0);
    assert_eq!(measures("no-emission").emission_paid, 0.0);

    assert_eq!(measures("fixed-topology").topology_changes, 0);
    assert!(measures("fixed-topology").mutated_parameters > 0);

    // Zero per-parameter cost leaves only rounding: `MaintenanceBill::settle`
    // attributes what remains of the rounded payment after basal and sensor
    // to the controller term, a few ulps per tick.
    let charged = flows("control")["controller-cost"]
        .as_f64()
        .expect("a flow");
    let free = flows("no-complexity-cost")["controller-cost"]
        .as_f64()
        .expect("a flow");
    assert!(charged > 1.0e-3, "{charged}");
    assert!(free < charged * 1.0e-9, "{free} against {charged}");

    let fixed = measures("fixed-controller");
    assert_eq!((fixed.mutated_parameters, fixed.topology_changes), (0, 0));
    assert!(fixed.births > 0);
    let pairs = heredity("fixed-controller");
    assert!(pairs["pairs"].as_u64() > Some(0));
    assert_eq!(pairs["pair_parameters_identical"], pairs["pair_parameters"]);

    assert_eq!(measures("chemistry-only").emitter_ticks, 0);
    assert!(measures("chemistry-only").births > 0);

    let inherited = heredity("control");
    let fresh = heredity("fresh-offspring");
    let share = |h: &Value| {
        h["pair_parameters_identical"].as_f64().expect("count")
            / h["pair_parameters"].as_f64().expect("count")
    };
    assert!(share(&inherited) > 0.9, "{inherited}");
    assert!(share(&fresh) < 0.25, "{fresh}");
}

fn spec(text: &str) -> Spec {
    toml::from_str(text).expect("parses")
}

#[test]
fn specifications_that_would_hide_a_choice_are_refused() {
    let base = appendix_a();
    let good = format!(
        "schema = \"frankenlife.experiment.v1\"\nbase = \"base.toml\"\n{EMISSION}\n{ECOLOGY}\n{EMISSION_CONDITIONS}"
    );
    let accepted = Experiment::new(spec(&good), base).expect("the good specification resolves");
    // An ablation's keys and a condition's own overrides are resolved and
    // recorded: nothing about a condition is implicit.
    let chemistry = &accepted.conditions[2];
    assert_eq!(chemistry.ablation, Some("chemistry-only"));
    assert_eq!(
        chemistry.set["model"],
        "\"field-ecology-v1-chemistry-only\""
    );
    assert_eq!(chemistry.set["world.width"], "32.0");

    let refused = |text: String, why: &str| {
        let result = Experiment::new(spec(&text), base);
        assert!(result.is_err(), "{why}: accepted");
    };
    refused(
        good.replace("held_out = [11, 12]", "held_out = [3, 12]"),
        "a held-out seed is a discovery seed",
    );
    refused(
        good.replace("discovery = [1, 2, 3]", "discovery = []"),
        "no discovery seed",
    );
    refused(
        good.replace("control = \"control\"", "control = \"nothing\""),
        "no control",
    );
    refused(
        good.replace(
            "selection = \"Every run of every condition is reported; none is excluded.\"",
            "selection = \" \"",
        ),
        "no selection procedure",
    );
    refused(
        good.replace("\"world.width\" = 32.0", "\"world.widths\" = 32.0"),
        "an override names no key",
    );
    refused(
        good.replace("\"world.width\" = 32.0", "\"seed\" = 9"),
        "an override sets the seed",
    );
    refused(
        good.replace("ablation = \"no-emission\"", "ablation = \"no-emission\"\n[conditions.set]\n\"emission.maximum_fractional_rate_per_second\" = 0.1"),
        "a condition changes its ablation's key",
    );
    refused(
        good.replace("ablation = \"no-emission\"", "ablation = \"no-rules\""),
        "an unknown ablation",
    );
    refused(
        good.replace("name = \"no-emission\"", "name = \"control\""),
        "a repeated condition name",
    );
    refused(
        good.replace("\"world.width\" = 32.0", "\"world.width\" = -32.0"),
        "an invalid condition",
    );
    refused(
        format!("{good}\n[sweep]\n\"costs.birth_energy\" = []\n"),
        "an empty sweep",
    );
    refused(
        format!("{good}\n[select]\nmeasure = \"beauty\"\nrule = \"max\"\n"),
        "an unknown measure",
    );
    assert!(
        toml::from_str::<Spec>(&format!("{good}\nextra = 1\n")).is_err(),
        "an unknown key"
    );

    // A directory with files but no catalog is not taken over.
    let root = scratch("refusals");
    fs::write(root.join("notes.txt"), "mine").expect("writes");
    let taken = flife_cli::experiment(
        &accepted,
        &root,
        ExperimentOptions {
            cpu_workers: None,
            controllers: ControllerChoice::Cpu,
        },
        |_| {},
    );
    assert!(
        matches!(taken, Err(CoreError::StateConflict { .. })),
        "{taken:?}"
    );
}

#[test]
fn the_sign_flip_test_by_hand() {
    // [1, 2, 3]: only all-plus and all-minus reach |6| of 8 assignments.
    assert_eq!(sign_flip_p(&[1.0, 2.0, 3.0]), Some(0.25));
    // [3, 1]: sums 4, 2, -2, -4; two reach 4.
    assert_eq!(sign_flip_p(&[3.0, 1.0]), Some(0.5));
    // [1, -1]: the observed sum is 0, which every assignment reaches.
    assert_eq!(sign_flip_p(&[1.0, -1.0]), Some(1.0));
    assert_eq!(sign_flip_p(&[0.0, 0.0, 0.0]), Some(1.0));
    assert_eq!(sign_flip_p(&[]), None);
    assert_eq!(sign_flip_p(&[1.0; EXACT_TEST_MAX_SEEDS + 1]), None);
    assert_eq!(
        sign_flip_p(&[1.0; EXACT_TEST_MAX_SEEDS]),
        Some(2.0 / f64::from(1_u32 << EXACT_TEST_MAX_SEEDS))
    );
}
