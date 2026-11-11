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
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use flife_cli::memory::admit;
use flife_cli::{
    CheckpointMode, ControllerChoice, MemoryAdmission, Persistence, RESERVE_BYTES, Record,
    RenderOptions, RenderRecord, RunOptions, Style, configure, encoding_bytes, found, render,
    run_with,
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
    let reference = admit(&world, ControllerChoice::Cpu, None, 0).expect("admitted");
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

    let torch = admit(&world, ControllerChoice::Torch, None, 0).expect("admitted");
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

/// What `flife render`'s observers hold: the most heap a render of the
/// preset holds is at most what the same world, stepped the same ticks
/// without observers, holds plus the admitted render term.
#[test]
fn a_render_holds_no_more_heap_than_a_plain_run_and_its_render_term() {
    let _serial = serial();
    let config = preset();
    let ticks = 8;
    let workers = NonZeroUsize::new(4);

    let base = ALLOC.current_usage();
    ALLOC.reset_peak_usage();
    {
        let mut world = found(&config).expect("founds");
        configure(&mut world, workers, ControllerChoice::Cpu, None).expect("admitted");
        for _ in 0..ticks {
            assert!(matches!(
                world.step().expect("steps"),
                Stepped::Published(_)
            ));
        }
    }
    let plain = u64::try_from(ALLOC.peak_usage().saturating_sub(base)).expect("fits");

    for (size, style) in [
        ([1280, 720], Style::default()),
        (
            [1280, 720],
            Style {
                particles: flife_cli::Particles::Density,
                field: true,
                ..Style::default()
            },
        ),
    ] {
        let options = RenderOptions {
            ticks,
            every: 2,
            size,
            out: fresh("render-frames"),
            cpu_workers: workers,
            controllers: ControllerChoice::Cpu,
            render_threads: NonZeroUsize::new(4).expect("four"),
            video: None,
            fps: 30,
            style,
        };
        let mut admitted = None;
        let base = ALLOC.current_usage();
        ALLOC.reset_peak_usage();
        render(&config, &options, |record| {
            if let RenderRecord::Start { memory, .. } = record {
                admitted = Some(memory.render);
            }
        })
        .expect("renders");
        let peak = u64::try_from(ALLOC.peak_usage().saturating_sub(base)).expect("fits");
        let term = admitted.expect("a start record");
        println!(
            "{size:?} {}: render heap peak {:.2} MiB; plain run {:.2} MiB + render term {:.2} MiB \
             = {:.2} MiB; observers held {:.2} MiB above the plain run",
            style.particles.name(),
            mib(peak),
            mib(plain),
            mib(term),
            mib(plain + term),
            mib(peak.saturating_sub(plain)),
        );
        assert!(
            peak <= plain + term,
            "render held {peak} > plain run {plain} + render term {term}"
        );
    }
}

/// One frame being encoded holds no more than its allowance: on noise,
/// which leaves the compressor no match and so the most tokens and the
/// longest stream, and on a flat frame, from 96 x 64 to 1920 x 1080. The
/// frame's own pixels are counted with the encoder's working set.
#[test]
fn a_frames_encoding_holds_no_more_than_its_allowance() {
    let _serial = serial();
    for size in [[96_u32, 64_u32], [640, 360], [1280, 720], [1920, 1080]] {
        let [width, height] = size;
        let pixels = u64::from(width) * u64::from(height);
        let mut noise = vec![0_u8; usize::try_from(pixels * 4).expect("fits")];
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        for byte in &mut noise {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *byte = state.to_le_bytes()[3];
        }
        let flat = vec![7_u8; noise.len()];
        for (name, rgba) in [("noise", &noise), ("flat", &flat)] {
            let base = ALLOC.current_usage();
            ALLOC.reset_peak_usage();
            let png = fmn_codec::png::encode_rgba8(
                width,
                height,
                rgba,
                fmn_codec::CompressionLevel::Fast,
            );
            let held =
                u64::try_from(ALLOC.peak_usage().saturating_sub(base)).expect("fits") + pixels * 4;
            let allowance = encoding_bytes(size, 1);
            println!(
                "{width}x{height} {name}: held {held} B ({:.1} B/px), allowance {allowance} B, png {} B",
                held as f64 / pixels as f64,
                png.len()
            );
            assert!(
                held <= allowance,
                "{width}x{height} {name}: {held} > {allowance}"
            );
        }
    }
}

/// Render's observer term is its feed's slots, its view's buffers, one
/// frame's job, the CPU route's frame buffers and the frames being encoded.
/// A budget with room for only two encoders gets two, and one short of
/// room for one is refused.
#[test]
fn a_renders_term_is_its_observers_and_a_short_budget_encodes_fewer_frames() {
    let _serial = serial();
    let size = [320, 180];
    let threads = NonZeroUsize::new(2).expect("two");
    // The observers render makes, made the same way.
    let world = found(&preset()).expect("founds");
    let publisher = flife_render::Publisher::new(&world, 3, || {}).expect("a publisher");
    let lease = publisher
        .feed()
        .viewer(1)
        .expect("a viewer")
        .latest()
        .expect("the founding epoch");
    let view = flife_manim::WorldView::for_epoch(&lease, size, [0, 0, 0]).expect("a view");
    let drawing = publisher.reserved_bytes()
        + view.reserved_bytes()
        + view.frame_bytes()
        + view.offline_bytes(threads.get());
    drop(lease);

    let start = |config: &ValidatedConfig| {
        let options = RenderOptions {
            ticks: 0,
            every: 1,
            size,
            out: fresh("render-term"),
            cpu_workers: None,
            controllers: ControllerChoice::Cpu,
            render_threads: threads,
            video: None,
            fps: 30,
            style: Style::default(),
        };
        let mut admitted = None;
        render(config, &options, |record| {
            if let RenderRecord::Start {
                memory, encoders, ..
            } = record
            {
                admitted = Some((memory.clone(), *encoders));
            }
        })
        .map(|()| admitted.expect("a start record"))
    };
    let (memory, encoders) = start(&preset()).expect("admitted");
    print(&memory);
    assert_eq!(encoders, flife_cli::ENCODERS);
    assert_eq!(memory.render, drawing + encoding_bytes(size, encoders));
    let others = memory.fixed_bytes() - memory.render;

    let two = preset_with(&[(
        "maximum_total_bytes",
        others + drawing + encoding_bytes(size, 2),
    )]);
    let (memory, encoders) = start(&two).expect("admitted with fewer encoders");
    assert_eq!(encoders, 2);
    assert_eq!(memory.render, drawing + encoding_bytes(size, 2));
    assert_eq!(memory.peak_bytes, memory.budget_bytes);

    let short = preset_with(&[(
        "maximum_total_bytes",
        others + drawing + encoding_bytes(size, 1) - 1,
    )]);
    match start(&short) {
        Err(CoreError::BudgetExhausted { budget, .. }) => {
            assert!(budget.starts_with("peak memory"), "{budget}");
        }
        other => panic!("expected a peak-memory refusal, got {other:?}"),
    }
}

/// The preset at its population ceiling (16,384), as the view sees it.
fn full_epoch() -> flife_render::Epoch {
    let config = preset_with(&[("initial_count", 16_384), ("maximum_count", 16_384)]);
    let world = found(&config).expect("founds");
    let epoch = flife_render::Epoch::standalone(&world).expect("an epoch");
    assert_eq!(epoch.len(), 16_384);
    epoch
}

/// The view's own buffers, measured: made for a full world, shown in
/// points and then in density (so each grid is built while the last one
/// is held), a view holds no more heap than its reserved bytes.
#[test]
fn a_views_own_buffers_hold_no_more_heap_than_its_reserved_bytes() {
    let _serial = serial();
    let epoch = full_epoch();
    for size in [[1280, 720], [3840, 2160]] {
        let base = ALLOC.current_usage();
        ALLOC.reset_peak_usage();
        let mut view = flife_manim::WorldView::for_epoch(&epoch, size, [0, 0, 0]).expect("a view");
        view.show(&epoch).expect("shows");
        view.show(&epoch).expect("shows again");
        view.set_style(Style {
            particles: flife_cli::Particles::Density,
            ..Style::default()
        });
        view.show(&epoch).expect("shows the density");
        view.show(&epoch).expect("shows the density again");
        let held = u64::try_from(ALLOC.peak_usage().saturating_sub(base)).expect("fits");
        let reserved = view.reserved_bytes();
        println!(
            "{size:?}: the view held {:.2} MiB of its {:.2} MiB",
            mib(held),
            mib(reserved)
        );
        assert!(
            held <= reserved,
            "{size:?}: the view held {held} > {reserved}"
        );
    }
}

/// One frame on the CPU route, measured: a full world at 3840 x 2160 in
/// points and in density, on four threads, holds no more heap above the
/// view than the view's frame and offline bytes.
#[test]
fn a_cpu_frame_holds_no_more_heap_than_the_views_frame_terms() {
    let _serial = serial();
    let epoch = full_epoch();
    for particles in [flife_cli::Particles::Points, flife_cli::Particles::Density] {
        let mut view =
            flife_manim::WorldView::for_epoch(&epoch, [3840, 2160], [0, 0, 0]).expect("a view");
        view.set_style(Style {
            particles,
            ..Style::default()
        });
        view.show(&epoch).expect("shows");
        let base = ALLOC.current_usage();
        ALLOC.reset_peak_usage();
        let draws = view.draws().expect("draws");
        let listed = u64::try_from(ALLOC.peak_usage().saturating_sub(base)).expect("fits");
        drop(draws);
        assert!(
            listed <= view.frame_bytes() - view.preparation_limit(),
            "{}: the draw list held {listed} beyond the preparation's limit",
            particles.name()
        );
        let base = ALLOC.current_usage();
        ALLOC.reset_peak_usage();
        let job = view.job().expect("a job");
        let compiled = u64::try_from(ALLOC.peak_usage().saturating_sub(base)).expect("fits");
        drop(job);
        let base = ALLOC.current_usage();
        ALLOC.reset_peak_usage();
        let rgba = view.render_rgba8(4).expect("renders");
        let held = u64::try_from(ALLOC.peak_usage().saturating_sub(base)).expect("fits");
        drop(rgba);
        let offline = view.offline_bytes(4);
        println!(
            "{}: a 4K frame's draw list held {:.2} MiB; the frame {:.2} MiB, its job {:.2} of {:.2} MiB, \
             the rest {:.2} of {:.2} MiB",
            particles.name(),
            mib(listed),
            mib(held),
            mib(compiled),
            mib(view.frame_bytes()),
            mib(held - compiled),
            mib(offline),
        );
        assert!(
            compiled <= view.frame_bytes(),
            "{}: the job held {compiled}",
            particles.name()
        );
        assert!(
            held - compiled <= offline,
            "{}: rendering held {} above its job > {offline}",
            particles.name(),
            held - compiled
        );
    }
}
