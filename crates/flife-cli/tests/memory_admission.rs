//! FL-043 evidence (plan §7.5, §19.3, §19.4, INV-12, INV-15): the
//! peak-memory preflight, measured by a counting global allocator.
//!
//! - The preset is admitted term by term: the allocated genome quota, the
//!   founded world's footprint, the batched path's lists, the step's
//!   staging, the checkpoint's peak and the reserve, with what they leave
//!   granted to the controller backend; installing the backend reserves
//!   exactly the lists charged for it.
//! - A budget below those terms is refused before anything is created.
//! - Under a budget that leaves the backend little room, it holds no more,
//!   defers what does not fit, and the run reaches the reference's digest.
//! - A full population's run, stepped on the CPU backend, holds no more
//!   heap than its admitted terms, with its genomes at their actual bytes.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::cast_precision_loss,
    reason = "test helpers: a failure in one is a test failure; figures are printed"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use flife_cli::memory::admit;
use flife_cli::{
    CheckpointMode, ControllerChoice, MemoryAdmission, Persistence, RESERVE_BYTES, Record,
    RunOptions, configure, found, run_with,
};
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Intervention, Stepped};
use peak_alloc::PeakAlloc;
use serde_json::Value;

#[global_allocator]
static ALLOC: PeakAlloc = PeakAlloc;

/// The allocator's counters are global: one measured test at a time.
static SERIAL: Mutex<()> = Mutex::new(());

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The preset with each `key = value` replaced.
fn preset_with(pairs: &[(&str, u64)]) -> ValidatedConfig {
    let mut lines: Vec<String> = PRESET.lines().map(str::to_owned).collect();
    for &(key, value) in pairs {
        let line = lines
            .iter()
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .expect("the key is in the preset");
        lines[line] = format!("{key} = {value}");
    }
    ValidatedConfig::from_toml(&lines.join("\n")).expect("still valid")
}

fn preset() -> ValidatedConfig {
    preset_with(&[])
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / f64::from(1 << 20)
}

fn scratch() -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("flife-memory-admission");
    fs::create_dir_all(&path).expect("creates the scratch directory");
    path
}

/// A directory path that does not exist yet.
fn fresh(name: &str) -> PathBuf {
    let path = scratch().join(name);
    if path.exists() {
        fs::remove_dir_all(&path).expect("clears the last run's directory");
    }
    path
}

fn options(ticks: u64, controllers: ControllerChoice) -> RunOptions {
    RunOptions {
        ticks,
        every: 0,
        cpu_workers: None,
        controllers,
        metrics_every: 0,
    }
}

/// Run `config` headlessly; its records as JSON.
fn records(config: &ValidatedConfig, options: RunOptions) -> Vec<Value> {
    let held = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&held);
    run_with(config, options, &[], None, move |record: &Record| {
        sink.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(serde_json::to_value(record).expect("a record serializes"));
    })
    .expect("the run ends");
    held.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

fn record<'a>(records: &'a [Value], kind: &str) -> &'a Value {
    records
        .iter()
        .find(|record| record["record"] == kind)
        .unwrap_or_else(|| panic!("a {kind} record"))
}

fn number(value: &Value) -> u64 {
    value
        .as_u64()
        .unwrap_or_else(|| panic!("{value} is a count"))
}

/// The preset's admission on `choice`, from a founded world.
fn admitted(config: &ValidatedConfig, choice: ControllerChoice) -> MemoryAdmission {
    let mut world = found(config).expect("the preset founds");
    configure(&mut world, None, choice, None).expect("admitted")
}

fn print(admission: &MemoryAdmission) {
    println!(
        "budget {:.1} MiB: genomes {:.1} ({} logical parameters), state {:.2}, field {:.2}, \
         scratch {:.2}, births {:.2}, render {:.2}, checkpoint {:.2}, reserve {:.1}, backend \
         {:.1}; peak {:.1} MiB",
        mib(admission.budget_bytes),
        mib(admission.genomes),
        admission.logical_parameters,
        mib(admission.state),
        mib(admission.field),
        mib(admission.scratch),
        mib(admission.births),
        mib(admission.render),
        mib(admission.checkpoint),
        mib(admission.reserve),
        mib(admission.backend),
        mib(admission.peak_bytes),
    );
}

/// The preset's terms are the configuration's quotas and the founded
/// world's own footprint, they add up to the peak, and the backend gets
/// what the rest leaves. The CPU reference has no backend term and no
/// batched lists; installing a backend reserves exactly the lists charged.
#[test]
fn the_preset_is_admitted_term_by_term() {
    let _serial = serial();
    let config = preset();
    let execution = &config.config().execution;
    let mut world = found(&config).expect("the preset founds");
    let footprint = world.footprint();
    let batched = world.batched_bytes();
    let reference = admit(&config, &world, ControllerChoice::Cpu, None, 0).expect("admitted");
    print(&reference);
    assert_eq!(reference.budget_bytes, execution.maximum_total_bytes);
    assert_eq!(reference.genomes, execution.maximum_allocated_genome_bytes);
    assert_eq!(
        reference.logical_genome_bytes,
        execution.maximum_logical_genome_bytes
    );
    assert_eq!(
        reference.logical_parameters * 4,
        execution.maximum_logical_genome_bytes
    );
    assert_eq!(
        [
            reference.state,
            reference.field,
            reference.scratch,
            reference.births
        ],
        [
            footprint.state,
            footprint.field,
            footprint.scratch,
            footprint.staging
        ]
    );
    assert_eq!(
        (reference.render, reference.checkpoint, reference.reserve),
        (0, 0, RESERVE_BYTES)
    );
    assert_eq!(reference.backend, 0, "the reference has no backend");
    let fixed = reference.genomes
        + reference.state
        + reference.field
        + reference.scratch
        + reference.births
        + reference.reserve;
    assert_eq!(reference.peak_bytes, fixed);

    let torch = admit(&config, &world, ControllerChoice::Torch, None, 0).expect("admitted");
    print(&torch);
    assert!(batched > 0);
    assert_eq!(torch.scratch, footprint.scratch + batched);
    assert_eq!(torch.backend, torch.budget_bytes - (fixed + batched));
    assert_eq!(torch.peak_bytes, torch.budget_bytes);

    // Installing the backend reserves exactly what was charged for it.
    let configured = configure(&mut world, None, ControllerChoice::Torch, None).expect("admitted");
    assert_eq!(configured, torch);
    assert_eq!(world.footprint().scratch, torch.scratch);
    assert_eq!(world.allocated().scratch, torch.scratch);
}

/// A budget the terms other than the backend's exceed is refused, naming
/// the shortfall, before the run directory exists.
#[test]
fn a_budget_below_the_terms_is_refused_before_anything_is_created() {
    let _serial = serial();
    let base = preset();
    let genomes = base.config().execution.maximum_allocated_genome_bytes;
    // Room for a checkpoint (FL-038's admission passes) and not the rest.
    let config = preset_with(&[
        ("maximum_total_bytes", genomes + (8 << 20)),
        ("checkpoint_retained_bytes", 4 << 20),
    ]);
    let run_dir = fresh("refused");
    let mut persistence = Persistence::new(&run_dir, &config);
    persistence.mode = CheckpointMode::PauseAndStream;
    let result = run_with(
        &config,
        options(4, ControllerChoice::Cpu),
        &[],
        Some(&persistence),
        |_: &Record| {},
    );
    match result {
        Err(CoreError::BudgetExhausted {
            budget,
            required,
            available,
        }) => {
            println!("refused: {budget}: {required} > {available}");
            assert!(budget.starts_with("peak memory"), "{budget}");
            assert_eq!(available, genomes + (8 << 20));
            assert!(required > available);
        }
        other => panic!("expected a peak-memory refusal, got {other:?}"),
    }
    assert!(!run_dir.exists(), "nothing is created before admission");
}

/// With the budget leaving the CPU backend 48 MiB, about four shards of the
/// preset's 4,096 genomes, it holds no more than that, defers the rest to
/// the reference, and the run reaches the reference's digest. The start
/// record carries the admission and the end record the backend's peak.
#[test]
fn a_small_backend_budget_defers_and_never_changes_the_run() {
    let _serial = serial();
    let roomy = admitted(&preset(), ControllerChoice::Torch);
    let fixed = roomy.peak_bytes - roomy.backend;
    let grant = 48 << 20;
    let config = preset_with(&[("maximum_total_bytes", fixed + grant)]);
    let ticks = 24;

    let reference = records(&config, options(ticks, ControllerChoice::Cpu));
    let tight = records(&config, options(ticks, ControllerChoice::Torch));
    let memory = &record(&tight, "start")["memory"];
    let end = record(&tight, "end");
    let controllers = &end["controllers"];
    println!("memory {memory}\ncontrollers {controllers}");
    assert_eq!(number(&memory["backend"]), grant);
    assert_eq!(number(&memory["peak_bytes"]), fixed + grant);
    let peak = number(&controllers["resident_peak_bytes"]);
    assert!(peak <= grant, "the backend held {peak} of {grant}");
    assert!(peak > grant / 2, "the backend filled its budget: {peak}");
    assert!(number(&controllers["deferred"]) > 0, "{controllers}");
    assert!(number(&controllers["batched"]) > 0, "{controllers}");
    assert_eq!(controllers["failure"], Value::Null);
    assert_eq!(
        end["digest"],
        record(&reference, "end")["digest"],
        "the budget changed the run"
    );
    // The reference run's record says it has no backend term.
    assert_eq!(number(&record(&reference, "start")["memory"]["backend"]), 0);
}

/// A full population (capacity 4,096, the founders) stepped on the CPU
/// backend: the most heap the world and its backend hold, from founding
/// through 48 ticks, is within the admitted terms with the genomes at the
/// most bytes they actually held. Neither the reserve nor the checkpoint
/// term is counted, so the bound is the world's and the backend's own.
#[test]
fn a_full_population_holds_no_more_heap_than_its_terms() {
    let _serial = serial();
    let config = preset_with(&[("maximum_count", 4_096)]);
    let base = ALLOC.current_usage();
    ALLOC.reset_peak_usage();
    let mut world = found(&config).expect("founds");
    let admission = configure(&mut world, None, ControllerChoice::Torch, None).expect("admitted");
    let founded = world.allocated();
    let mut genomes = world.quota().allocated_bytes();
    for _ in 0..48 {
        match world.step().expect("steps") {
            Stepped::Published(_) => {}
            Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
        }
        genomes = genomes.max(world.quota().allocated_bytes());
        // Nothing reserved for the full population grows.
        let now = world.allocated();
        assert_eq!(now.field, founded.field);
        assert_eq!(now.scratch, founded.scratch);
        assert!(now.state <= admission.state);
    }
    let peak = u64::try_from(ALLOC.peak_usage().saturating_sub(base)).expect("fits");
    let bound = admission.state
        + admission.field
        + admission.scratch
        + admission.births
        + genomes
        + admission
            .backend
            .min(world.controller_report().resident_peak_bytes);
    print(&admission);
    println!(
        "live {}: heap peak {:.1} MiB; bound {:.1} MiB (genomes {:.1}, backend held {:.1})",
        world.population().len(),
        mib(peak),
        mib(bound),
        mib(genomes),
        mib(world.controller_report().resident_peak_bytes),
    );
    assert_eq!(
        world.population().len(),
        4_096,
        "the population stayed full"
    );
    assert!(
        peak <= bound,
        "heap peak {peak} above the admitted terms {bound}"
    );
}

/// The staging term against the heaviest tick a test can make: 512 founders
/// of a capacity of 4,096, and at tick 0 an introduction of the other 3,584,
/// which stages an entrant and an event for each. What the step holds above
/// what the world held before it is at most the staging term, the
/// introduced genomes' parameters (charged to the genome quota) and the
/// state's room to grow to its bound.
#[test]
fn a_filling_introduction_stays_within_the_staging_term() {
    let _serial = serial();
    let config = preset_with(&[("initial_count", 512), ("maximum_count", 4_096)]);
    let mut world = found(&config).expect("founds");
    configure(&mut world, None, ControllerChoice::Cpu, None).expect("admitted");
    let footprint = world.footprint();
    let state = world.allocated().state;
    let genomes = world.quota().allocated_bytes();
    let introduce = Intervention::IntroduceOrganisms {
        cell: [0, 0],
        count: 3_584,
        energy: 2.0,
    };
    let base = ALLOC.current_usage();
    ALLOC.reset_peak_usage();
    let report = match world.step_with(&[introduce]).expect("steps") {
        Stepped::Published(report) => report,
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    };
    let held = u64::try_from(ALLOC.peak_usage().saturating_sub(base)).expect("fits");
    assert_eq!(report.introductions.len(), 3_584, "the population filled");
    assert!(
        report.deaths.is_empty() && report.births.is_empty(),
        "tick 0 only introduces"
    );
    let introduced = world.quota().allocated_bytes() - genomes;
    let room = footprint.state - state;
    println!(
        "a filling introduction held {:.2} MiB above the world: staging {:.2}, introduced genomes {:.2}, \
         state room {:.2}",
        mib(held),
        mib(footprint.staging),
        mib(introduced),
        mib(room),
    );
    assert!(
        held <= footprint.staging + introduced + room,
        "the step held {held} > staging {} + genomes {introduced} + state room {room}",
        footprint.staging
    );
}
