//! FL-057 evidence (plan §20.4, ADR-11): distributed independent experiment
//! ensembles with reproducible job identities, Asupersync-supervised parallel
//! execution, cooperative mid-run cancellation, disjoint multi-machine
//! sharding, cryptographically verified durable shard import, and exact
//! byte-for-bit `report.json` parity with single-node serial execution.

#![forbid(unsafe_code)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests: unexpected failure is a test failure"
)]

use std::collections::BTreeSet;
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use flife_cli::experiment::{DISCOVERY, HELD_OUT, RUNS_DIR};
use flife_cli::{
    CancelToken, ControllerChoice, EXPERIMENT_CATALOG_FILE, EnsembleOptions, EnsembleOutcome,
    EnsembleStage, Experiment, ExperimentOptions, ExperimentRecord, REPORT_FILE, ShardSpec,
    experiment, import_shards, run_ensemble,
};
use flife_core::CoreError;
use flife_store::ExperimentCatalog;
use serde_json::Value;

const REFERENCE: &str = include_str!("../../../presets/reference-small-world.toml");

fn scratch_dir(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("flife-fl057-distributed")
        .join(format!("{label}-{}-{n}", std::process::id()));
    fs::create_dir_all(&dir).expect("create FL-057 scratch dir");
    dir
}

const COMPACT_OVERRIDES: &str = r#"
[set]
"world.width" = 32.0
"world.height" = 32.0
"world.grid_width" = 32
"world.grid_height" = 32
"population.initial_count" = 32
"population.maximum_count" = 256
"population.initial_energy" = 2.8
"sources.count" = 4
"sources.power_per_source" = 120.0
"sources.sigma_world_units" = 3.0
"sources.path_direction_interval_seconds" = 2.0
"chemistry.uptake_capacity_per_second" = 8.0
"costs.basal_energy_per_second" = 0.6
"reproduction.minimum_parent_energy_before_attempt" = 2.2
"reproduction.maximum_attempt_rate_per_second" = 4.0
"reproduction.cooldown_seconds" = 0.25
"controller.evaluation_interval_ticks" = 2
"execution.command_durability" = "volatile-in-memory"
"#;

fn write_experiment_spec(root: &Path, name: &str, ticks: u64, with_select: bool) -> PathBuf {
    fs::write(root.join("base.toml"), REFERENCE).expect("write base preset");
    let select_block = if with_select {
        r#"
[select]
measure = "final_live"
rule = "max"
"#
    } else {
        ""
    };
    let spec_text = format!(
        r#"schema = "frankenlife.experiment.v1"
name = "{name}"
hypothesis = "H3: paid field emission shifts population dynamics across independent seeds"
base = "base.toml"
ticks = {ticks}
metrics_every = 16
control = "control"
selection = "Every discovery and held-out seed is cataloged and reported."

[seeds]
discovery = [11, 12, 13]
held_out = [21, 22]
{select_block}
{COMPACT_OVERRIDES}

[[conditions]]
name = "control"

[[conditions]]
name = "no-emission"
ablation = "no-emission"

[[conditions]]
name = "fixed-topology"
ablation = "fixed-topology"
"#
    );
    let path = root.join("experiment.toml");
    fs::write(&path, spec_text).expect("write spec");
    path
}

/// Reproducible content-addressed `job_id` + Asupersync-supervised parallel
/// execution (`parallel = 3` vs serial `parallel = 1`): proves identical job
/// identities, identical manifests, and byte-for-bit identical `report.json`.
#[test]
fn reproducible_job_identity_and_supervised_parallel_report_parity() {
    let root = scratch_dir("parallel-parity");
    let spec_path = write_experiment_spec(&root, "fl057-parallel", 24, false);
    let exp = Experiment::load(&spec_path).expect("load spec");

    // Job identities are deterministic, collision-free, and sensitive to every field.
    let id_a = exp.job_id(1, "control", DISCOVERY, 11);
    let id_a_repeat = exp.job_id(1, "control", DISCOVERY, 11);
    assert_eq!(id_a, id_a_repeat);
    assert_ne!(id_a, exp.job_id(2, "control", DISCOVERY, 11));
    assert_ne!(id_a, exp.job_id(1, "no-emission", DISCOVERY, 11));
    assert_ne!(id_a, exp.job_id(1, "control", HELD_OUT, 11));
    assert_ne!(id_a, exp.job_id(1, "control", DISCOVERY, 12));

    // Serial baseline (`experiment()`).
    let serial_dir = root.join("serial");
    let mut serial_job_ids = BTreeSet::new();
    let _serial_report = experiment(
        &exp,
        &serial_dir,
        ExperimentOptions {
            cpu_workers: NonZeroUsize::new(2),
            controllers: ControllerChoice::Cpu,
        },
        |rec| {
            if let ExperimentRecord::ExperimentRun { job_id, .. } = rec {
                serial_job_ids.insert(job_id.clone());
            }
        },
    )
    .expect("serial run succeeds");
    assert_eq!(
        serial_job_ids.len(),
        15,
        "3 conditions x (3 + 2) seeds = 15 jobs"
    );

    // Supervised parallel run (`parallel = 3`).
    let parallel_dir = root.join("parallel");
    let mut parallel_job_ids = BTreeSet::new();
    let outcome = run_ensemble(
        &exp,
        &parallel_dir,
        &EnsembleOptions {
            cpu_workers: NonZeroUsize::new(1),
            controllers: ControllerChoice::Cpu,
            parallel: NonZeroUsize::new(3).expect("3"),
            ..EnsembleOptions::default()
        },
        |rec| {
            if let ExperimentRecord::ExperimentRun { job_id, .. } = rec {
                parallel_job_ids.insert(job_id.clone());
            }
        },
    )
    .expect("parallel run succeeds");
    assert!(matches!(outcome, EnsembleOutcome::Complete(_)));
    assert_eq!(
        parallel_job_ids, serial_job_ids,
        "job identities must be identical regardless of concurrency"
    );

    // Byte-for-bit identical `report.json`!
    let serial_json = fs::read(serial_dir.join(REPORT_FILE)).expect("serial report");
    let parallel_json = fs::read(parallel_dir.join(REPORT_FILE)).expect("parallel report");
    assert_eq!(
        parallel_json, serial_json,
        "supervised parallel ensemble must produce byte-identical report.json"
    );

    // Every cataloged run manifest carries the exact reproducible `job_id`.
    let cat =
        ExperimentCatalog::read(&parallel_dir.join(EXPERIMENT_CATALOG_FILE)).expect("read catalog");
    for row in cat.runs().expect("runs") {
        let m: Value = serde_json::from_str(&row.manifest).expect("manifest json");
        let expected = exp
            .job_id(row.ordinal, &row.condition, &row.split, row.seed)
            .to_hex();
        assert_eq!(m["identity"]["job_id"].as_str(), Some(expected.as_str()));
    }
}

/// Multi-machine disjoint sharding (`3` shards) + cryptographically verified
/// durable shard import + tamper/mismatch refusal — tested on BOTH a full
/// non-`[select]` ensemble AND a two-stage `[select]` ensemble!
#[test]
fn disjoint_multi_machine_sharding_verified_import_and_tamper_refusal() {
    let root = scratch_dir("sharding");
    let spec_path = write_experiment_spec(&root, "fl057-sharded", 20, false);
    let exp = Experiment::load(&spec_path).expect("load spec");

    // Control single-machine run.
    let control_dir = root.join("control");
    let _ = experiment(
        &exp,
        &control_dir,
        ExperimentOptions {
            cpu_workers: NonZeroUsize::new(2),
            controllers: ControllerChoice::Cpu,
        },
        |_| {},
    )
    .expect("control succeeds");
    let control_json = fs::read(control_dir.join(REPORT_FILE)).expect("control report");

    // Execute 3 completely independent shards (`1/3`, `2/3`, `3/3`).
    let mut shard_dirs = Vec::new();
    let mut seen_ordinals = BTreeSet::new();
    for k in 1..=3_u32 {
        let shard = ShardSpec::from_str(&format!("{k}/3")).expect("valid shard spec");
        let sdir = root.join(format!("shard-{k}"));
        let outcome = run_ensemble(
            &exp,
            &sdir,
            &EnsembleOptions {
                cpu_workers: NonZeroUsize::new(1),
                controllers: ControllerChoice::Cpu,
                parallel: NonZeroUsize::new(2).expect("2"),
                shard: Some(shard),
                ..EnsembleOptions::default()
            },
            |_| {},
        )
        .expect("shard succeeds");
        match outcome {
            EnsembleOutcome::Partial {
                shard: Some(s),
                executed,
                skipped,
                cataloged,
                ..
            } => {
                assert_eq!(s, shard);
                assert_eq!(executed, 5, "15 runs / 3 shards = 5 runs per shard");
                assert_eq!(skipped, 0);
                assert_eq!(cataloged, 5);
            }
            other => panic!("expected Partial outcome for shard {k}/3, got {other:?}"),
        }
        assert!(
            !sdir.join(REPORT_FILE).exists(),
            "partial shard must not write a premature report.json"
        );
        let cat = ExperimentCatalog::read(&sdir.join(EXPERIMENT_CATALOG_FILE)).expect("shard cat");
        for row in cat.runs().expect("shard runs") {
            assert!(shard.owns(row.ordinal));
            assert!(
                seen_ordinals.insert(row.ordinal),
                "shards must be strictly disjoint"
            );
        }
        shard_dirs.push(sdir);
    }
    assert_eq!(seen_ordinals.len(), 15, "3 shards must cover all 15 runs");

    // Incomplete `--report-only` with only 2 of 3 shards must be refused cleanly.
    let incomplete_dir = root.join("incomplete");
    let inc_err = run_ensemble(
        &exp,
        &incomplete_dir,
        &EnsembleOptions {
            imports: shard_dirs[..2].to_vec(),
            report_only: true,
            ..EnsembleOptions::default()
        },
        |_| {},
    )
    .expect_err("incomplete report-only must be refused");
    assert!(matches!(inc_err, CoreError::StateConflict { .. }));
    assert!(!incomplete_dir.join(REPORT_FILE).exists());

    // Importing all 3 shards with `--report-only` produces byte-exact `report.json`!
    let merged_dir = root.join("merged");
    let merged_outcome = run_ensemble(
        &exp,
        &merged_dir,
        &EnsembleOptions {
            imports: shard_dirs.clone(),
            report_only: true,
            ..EnsembleOptions::default()
        },
        |_| {},
    )
    .expect("merged report-only succeeds");
    assert!(matches!(merged_outcome, EnsembleOutcome::Complete(_)));
    let merged_json = fs::read(merged_dir.join(REPORT_FILE)).expect("merged report");
    assert_eq!(
        merged_json, control_json,
        "merged 3-shard distributed ensemble must match single-node control byte-for-bit"
    );

    // Re-importing the exact same shards is 100% idempotent.
    let reimport = import_shards(&exp, &merged_dir, &shard_dirs).expect("idempotent re-import");
    assert_eq!(reimport.imported, 0);
    assert_eq!(reimport.skipped_existing, 15);

    // Tamper detection: flipping 1 byte in shard-1's records must be refused!
    let tampered_file = shard_dirs[0].join(RUNS_DIR).join("run-000001.ndjson");
    let mut raw = fs::read(&tampered_file).expect("read shard-1 run-1");
    raw[0] ^= 0xFF;
    fs::write(&tampered_file, raw).expect("corrupt shard-1 run-1");
    let tamper_dest = root.join("tamper-check");
    let tamper_err = import_shards(&exp, &tamper_dest, &shard_dirs)
        .expect_err("corrupted shard records must be refused");
    assert!(
        matches!(tamper_err, CoreError::StateConflict { .. }),
        "expected StateConflict for tampered shard, got {tamper_err:?}"
    );

    // Two-stage `[select]` sharded workflow: discovery shards -> import -> held-out shards -> merge.
    let sel_root = root.join("select-two-stage");
    fs::create_dir_all(&sel_root).expect("mkdir");
    let sel_spec_path = write_experiment_spec(&sel_root, "fl057-select", 20, true);
    let sel_exp = Experiment::load(&sel_spec_path).expect("load select spec");
    let sel_control = sel_root.join("control");
    let _ = experiment(
        &sel_exp,
        &sel_control,
        ExperimentOptions {
            cpu_workers: NonZeroUsize::new(2),
            controllers: ControllerChoice::Cpu,
        },
        |_| {},
    )
    .expect("select control succeeds");
    let sel_control_json = fs::read(sel_control.join(REPORT_FILE)).expect("select control json");

    // Attempting `--shard 1/2 --stage all` on a `[select]` experiment without
    // discovery complete must fail fast BEFORE executing any discovery jobs!
    let bad_dir = sel_root.join("bad-one-shot");
    let bad_err = run_ensemble(
        &sel_exp,
        &bad_dir,
        &EnsembleOptions {
            shard: Some(ShardSpec::from_one_based(1, 2).expect("1/2")),
            stage: EnsembleStage::All,
            controllers: ControllerChoice::Cpu,
            ..EnsembleOptions::default()
        },
        |_| {},
    )
    .expect_err("partial-shard one-shot on [select] must refuse before held-out");
    assert!(matches!(bad_err, CoreError::StateConflict { .. }));
    let bad_cat = ExperimentCatalog::read(&bad_dir.join(EXPERIMENT_CATALOG_FILE))
        .expect("bad_dir catalog readable");
    assert!(
        bad_cat.runs().expect("bad_dir runs").is_empty(),
        "fail-fast refusal must not waste compute executing partial discovery jobs"
    );

    // Stage 1: two disjoint discovery shards (`1/2`, `2/2`).
    let mut disc_shards = Vec::new();
    for k in 1..=2_u32 {
        let sdir = sel_root.join(format!("disc-{k}"));
        let out = run_ensemble(
            &sel_exp,
            &sdir,
            &EnsembleOptions {
                cpu_workers: NonZeroUsize::new(1),
                controllers: ControllerChoice::Cpu,
                shard: Some(ShardSpec::from_one_based(k, 2).expect("shard")),
                stage: EnsembleStage::DiscoveryOnly,
                ..EnsembleOptions::default()
            },
            |_| {},
        )
        .expect("discovery shard succeeds");
        assert!(matches!(out, EnsembleOutcome::Partial { .. }));
        disc_shards.push(sdir);
    }

    // Stage 2: two disjoint held-out shards (`1/2`, `2/2`), each importing the discovery shards first.
    let mut held_shards = Vec::new();
    for k in 1..=2_u32 {
        let sdir = sel_root.join(format!("held-{k}"));
        let out = run_ensemble(
            &sel_exp,
            &sdir,
            &EnsembleOptions {
                cpu_workers: NonZeroUsize::new(1),
                controllers: ControllerChoice::Cpu,
                shard: Some(ShardSpec::from_one_based(k, 2).expect("shard")),
                stage: EnsembleStage::HeldOutOnly,
                imports: disc_shards.clone(),
                ..EnsembleOptions::default()
            },
            |_| {},
        )
        .expect("held-out shard succeeds");
        assert!(matches!(out, EnsembleOutcome::Partial { .. }));
        held_shards.push(sdir);
    }

    // Final assembly from all discovery + held-out shards: byte-identical `report.json`!
    let mut all_shards = disc_shards;
    all_shards.extend(held_shards);
    let sel_final = sel_root.join("final");
    let sel_out = run_ensemble(
        &sel_exp,
        &sel_final,
        &EnsembleOptions {
            imports: all_shards,
            report_only: true,
            ..EnsembleOptions::default()
        },
        |_| {},
    )
    .expect("two-stage select merge succeeds");
    assert!(matches!(sel_out, EnsembleOutcome::Complete(_)));
    assert_eq!(
        fs::read(sel_final.join(REPORT_FILE)).expect("final select report"),
        sel_control_json,
        "two-stage sharded `[select]` report must match single-node control byte-for-bit"
    );
}

/// Cooperative cancellation mid-ensemble (`parallel = 2`) + durable resume:
/// triggers `CancelToken::cancel()` immediately after the first completed run
/// while peer workers are executing, verifies completed runs are durable and
/// cancelled runs are NOT falsely cataloged, then resumes to a byte-identical
/// `report.json`!
#[test]
fn cooperative_cancellation_preserves_durable_runs_and_resumes_identically() {
    let root = scratch_dir("cancel-resume");
    let spec_path = write_experiment_spec(&root, "fl057-cancel", 32, false);
    let exp = Experiment::load(&spec_path).expect("load spec");

    let control_dir = root.join("control");
    let _ = experiment(
        &exp,
        &control_dir,
        ExperimentOptions {
            cpu_workers: NonZeroUsize::new(2),
            controllers: ControllerChoice::Cpu,
        },
        |_| {},
    )
    .expect("control run succeeds");
    let control_json = fs::read(control_dir.join(REPORT_FILE)).expect("control report");

    let work_dir = root.join("work");
    let token = CancelToken::new();
    let trigger = token.clone();
    let mut saw_cancelled_record = false;
    let first_outcome = run_ensemble(
        &exp,
        &work_dir,
        &EnsembleOptions {
            cpu_workers: NonZeroUsize::new(1),
            controllers: ControllerChoice::Cpu,
            parallel: NonZeroUsize::new(2).expect("2"),
            cancel: Some(token),
            ..EnsembleOptions::default()
        },
        |rec| match rec {
            ExperimentRecord::ExperimentRun { .. } => {
                // Cancel as soon as the first job finishes while job #2 is still in flight.
                trigger.cancel();
            }
            ExperimentRecord::ExperimentCancelled { .. } => {
                saw_cancelled_record = true;
            }
            _ => {}
        },
    )
    .expect("cancelled ensemble exits cleanly");

    assert!(saw_cancelled_record, "must emit ExperimentCancelled record");
    let cataloged_before_resume = match first_outcome {
        EnsembleOutcome::Cancelled {
            cataloged,
            executed,
            ..
        } => {
            assert!(
                (1..15).contains(&cataloged),
                "expected partial durable progress (1..15), got {cataloged}"
            );
            assert_eq!(cataloged, executed);
            cataloged
        }
        other => panic!("expected Cancelled outcome, got {other:?}"),
    };
    assert!(
        !work_dir.join(REPORT_FILE).exists(),
        "cancelled ensemble must not emit an incomplete report.json"
    );

    // Every cataloged run must have its matching `.ndjson` file, and NO
    // uncataloged cancelled run may leave a stray `.ndjson` file behind.
    let cat = ExperimentCatalog::read(&work_dir.join(EXPERIMENT_CATALOG_FILE))
        .expect("read partial catalog");
    let durable_rows = cat.runs().expect("durable runs");
    assert_eq!(durable_rows.len(), cataloged_before_resume);
    let record_files: Vec<PathBuf> = fs::read_dir(work_dir.join(RUNS_DIR))
        .expect("read runs dir")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    assert_eq!(
        record_files.len(),
        durable_rows.len(),
        "cancelled jobs must not leave orphaned partial record files"
    );

    // Resume on the same directory without cancellation: skips already-durable runs
    // and finishes cleanly to a byte-for-bit identical `report.json`!
    let mut resumed_runs = 0_usize;
    let resumed_outcome = run_ensemble(
        &exp,
        &work_dir,
        &EnsembleOptions {
            cpu_workers: NonZeroUsize::new(1),
            controllers: ControllerChoice::Cpu,
            parallel: NonZeroUsize::new(2).expect("2"),
            ..EnsembleOptions::default()
        },
        |rec| {
            if matches!(rec, ExperimentRecord::ExperimentRun { .. }) {
                resumed_runs += 1;
            }
        },
    )
    .expect("resume succeeds");
    assert!(matches!(resumed_outcome, EnsembleOutcome::Complete(_)));
    assert_eq!(
        resumed_runs,
        15 - cataloged_before_resume,
        "resume must execute only the uncataloged remainder"
    );
    assert_eq!(
        fs::read(work_dir.join(REPORT_FILE)).expect("resumed report"),
        control_json,
        "resumed ensemble must produce byte-identical report.json to uninterrupted control"
    );
}

/// CLI end-to-end verification of `flife experiment --shard`, `--parallel`,
/// `--import`, and `--report-only` flags.
#[test]
fn cli_distributed_experiment_shard_import_and_report_only() {
    let root = scratch_dir("cli-e2e");
    let spec_path = write_experiment_spec(&root, "fl057-cli", 12, false);
    let bin = env!("CARGO_BIN_EXE_flife");

    let s1 = root.join("cli-shard-1");
    let s2 = root.join("cli-shard-2");
    let merged = root.join("cli-merged");
    let serial = root.join("cli-serial");

    for (args, label) in [
        (
            vec![
                "experiment".to_owned(),
                spec_path.display().to_string(),
                "--out".to_owned(),
                serial.display().to_string(),
                "--controllers".to_owned(),
                "cpu".to_owned(),
                "--cpu-workers".to_owned(),
                "2".to_owned(),
            ],
            "serial",
        ),
        (
            vec![
                "experiment".to_owned(),
                spec_path.display().to_string(),
                "--out".to_owned(),
                s1.display().to_string(),
                "--controllers".to_owned(),
                "cpu".to_owned(),
                "--parallel".to_owned(),
                "2".to_owned(),
                "--shard".to_owned(),
                "1/2".to_owned(),
            ],
            "shard 1/2",
        ),
        (
            vec![
                "experiment".to_owned(),
                spec_path.display().to_string(),
                "--out".to_owned(),
                s2.display().to_string(),
                "--controllers".to_owned(),
                "cpu".to_owned(),
                "--parallel".to_owned(),
                "2".to_owned(),
                "--shard".to_owned(),
                "2/2".to_owned(),
            ],
            "shard 2/2",
        ),
        (
            vec![
                "experiment".to_owned(),
                spec_path.display().to_string(),
                "--out".to_owned(),
                merged.display().to_string(),
                "--import".to_owned(),
                s1.display().to_string(),
                "--import".to_owned(),
                s2.display().to_string(),
                "--report-only".to_owned(),
            ],
            "merge report-only",
        ),
    ] {
        let out = Command::new(bin)
            .args(&args)
            .output()
            .unwrap_or_else(|e| panic!("run flife {label}: {e}"));
        assert!(
            out.status.success(),
            "flife {label} failed ({:?}):\nstdout: {}\nstderr: {}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    assert_eq!(
        fs::read(merged.join(REPORT_FILE)).expect("cli merged report"),
        fs::read(serial.join(REPORT_FILE)).expect("cli serial report"),
        "CLI sharded + imported ensemble must produce byte-identical report.json"
    );
}
