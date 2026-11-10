//! FL-038 evidence (plan §16.5, §19.4, A.2): checkpoint modes bound the
//! heap a checkpoint holds, measured by a counting global allocator.
//!
//! - A `pause-and-stream` checkpoint of the reference preset holds at most
//!   [`CHECKPOINT_FIXED_BYTES`] beyond the live world while it is encoded,
//!   committed, verified and retained; a container encoded whole in memory
//!   (the negative control) holds the container.
//! - Runs in each mode store byte-identical checkpoints and reach the
//!   world a run without checkpoints reaches; a `bounded-background` run's
//!   pages never exceed its budget, and its heap peak stays within its
//!   admitted peak of the run without checkpoints.
//! - Admission follows the budget by the numbers, falls back to
//!   `pause-and-stream` with its reason, and refuses a run whose checkpoint
//!   cannot fit before anything is created.

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

use flife_cli::{
    BOUNDED_BACKGROUND, CHECKPOINT_FIXED_BYTES, CheckpointMode, ControllerChoice, PAUSE_AND_STREAM,
    Persistence, RUN_SESSION, Record, RunOptions, StoreDurability, admit, found, run_with,
};
use flife_core::checkpoint::CHECKPOINT_PAGE_BYTES;
use flife_core::{CoreError, Sha256Digest, ValidatedConfig};
use flife_engine::NumericalProfile;
use flife_runtime::{CommandBook, checkpoint, checkpoint_into, restore};
use flife_store::{ChunkStore, Durability};
use peak_alloc::PeakAlloc;

#[global_allocator]
static ALLOC: PeakAlloc = PeakAlloc;

/// The allocator's counters are global: one measured test at a time.
static SERIAL: Mutex<()> = Mutex::new(());

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// The ticks each run here reaches, checkpointing every [`EVERY`].
const TICKS: u64 = 64;
const EVERY: u64 = 32;

/// The heap a run directory may hold that a run without one does not,
/// apart from checkpoints: the catalog's database connections and the
/// store's handles. A declared bound, far below one preset container
/// (160.7 MiB) and the 421.5 MiB its whole encoding peaks at. Measured on
/// an M5 Ultra: the `pause-and-stream` and one-page runs peak exactly where
/// the run without checkpoints does (its stepping peaks above the live
/// world plus a checkpoint), the four-page run 1.95 MiB above it.
const RUN_DIRECTORY_ALLOWANCE: u64 = 8 << 20;

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

fn preset() -> ValidatedConfig {
    ValidatedConfig::from_toml(PRESET).expect("the preset validates")
}

/// The preset with `[execution]` `key = value`.
fn preset_with(key: &str, value: u64) -> ValidatedConfig {
    let mut lines: Vec<String> = PRESET.lines().map(str::to_owned).collect();
    let line = lines
        .iter()
        .position(|line| line.split('=').next().map(str::trim) == Some(key))
        .expect("the key is in the preset");
    lines[line] = format!("{key} = {value}");
    ValidatedConfig::from_toml(&lines.join("\n")).expect("still valid")
}

fn scratch() -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("flife-checkpoint-memory");
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

fn mib(bytes: u64) -> f64 {
    bytes as f64 / f64::from(1 << 20)
}

/// `body`'s result and the most heap it held beyond what was allocated
/// when it started.
fn peak_beyond<T>(body: impl FnOnce() -> T) -> (T, u64) {
    let base = ALLOC.current_usage();
    ALLOC.reset_peak_usage();
    let out = body();
    let peak = ALLOC.peak_usage().saturating_sub(base);
    (out, u64::try_from(peak).expect("a usize fits"))
}

/// A `pause-and-stream` checkpoint of the reference preset, streamed into a
/// chunk store, committed, verified and retained as a run does it, holds at
/// most [`CHECKPOINT_FIXED_BYTES`] beyond the live world. The same
/// checkpoint encoded whole in memory, as before FL-038, holds the
/// container: the negative control, which the bound would refuse. Both
/// containers are the same bytes.
#[test]
fn a_streamed_checkpoint_holds_pages_while_a_whole_one_holds_the_container() {
    let _serial = serial();
    let config = preset();
    let world = found(&config).expect("the preset founds");
    let book = CommandBook::new(RUN_SESSION, config.config_hash(), 16);
    let root = fresh("streamed");
    let mut store = ChunkStore::open(&root, Durability::ProcessCrash).expect("creates");

    let ((sequence, digest), streamed) = peak_beyond(|| {
        let mut stream = store.stream().expect("streams");
        let sealed = checkpoint_into(&world, &book, &mut stream).expect("encodes");
        let publication = store.commit(stream, &sealed).expect("commits");
        let digest = store.verify(publication.sequence).expect("verifies");
        store.retain_intact(4).expect("retains");
        (publication.sequence, digest)
    });
    let (whole, in_memory) = peak_beyond(|| checkpoint(&world, &book).expect("encodes"));
    let container = u64::try_from(whole.len()).expect("fits");
    println!(
        "preset checkpoint {:.1} MiB: streamed peak {:.2} MiB (bound {:.2} MiB), in memory \
         {:.1} MiB",
        mib(container),
        mib(streamed),
        mib(CHECKPOINT_FIXED_BYTES),
        mib(in_memory)
    );
    assert!(
        streamed <= CHECKPOINT_FIXED_BYTES,
        "streamed peak {streamed} > {CHECKPOINT_FIXED_BYTES}"
    );
    assert!(in_memory >= container, "{in_memory} < {container}");
    assert!(
        container > 16 * CHECKPOINT_FIXED_BYTES,
        "the bound is meaningful: a {container}-byte container"
    );
    assert_eq!(digest, Sha256Digest::of(&whole));
    assert_eq!(store.load(sequence).expect("loads"), whole);
}

/// What one run of the preset recorded.
struct Ran {
    admission: Option<flife_cli::CheckpointAdmission>,
    /// Each checkpoint's tick, SHA-256, mode and retained peak.
    checkpoints: Vec<(u64, String, &'static str, u64)>,
    digest: String,
    peak: u64,
}

fn run(persistence: Option<&Persistence>) -> Ran {
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&records);
    let (finished, peak) = peak_beyond(|| {
        run_with(
            &preset(),
            RunOptions {
                ticks: TICKS,
                every: 0,
                cpu_workers: None,
                controllers: ControllerChoice::Cpu,
                metrics_every: 0,
            },
            &[],
            persistence,
            move |record: &Record| sink.lock().expect("records").push(record.clone()),
        )
        .expect("the run runs")
    });
    drop(finished);
    let records = records.lock().expect("records").clone();
    let mut ran = Ran {
        admission: None,
        checkpoints: Vec::new(),
        digest: String::new(),
        peak,
    };
    for record in records {
        match record {
            Record::Start { checkpoints, .. } => ran.admission = checkpoints,
            Record::Checkpoint {
                tick,
                sha256,
                mode,
                retained_peak,
                ..
            } => ran.checkpoints.push((tick, sha256, mode, retained_peak)),
            Record::End { tick, digest, .. } => {
                assert_eq!(tick, TICKS);
                ran.digest = digest;
            }
            _ => {}
        }
    }
    ran
}

fn persistence(name: &str, mode: CheckpointMode, retained_bytes: u64) -> Persistence {
    Persistence {
        checkpoint_every: EVERY,
        durability: StoreDurability::ProcessCrash,
        mode,
        retained_bytes,
        ..Persistence::new(fresh(name), &preset())
    }
}

/// Runs of the preset in each mode store byte-identical checkpoints at the
/// same ticks and reach the world a run without checkpoints reaches. A
/// `bounded-background` run's pages reach but never exceed its budget; each
/// run's heap peak stays within its admitted peak (plus the run directory's
/// own allowance) of the run without checkpoints, far below one container.
#[test]
fn each_mode_stores_the_same_checkpoints_within_its_admitted_peak() {
    let _serial = serial();
    // The first run warms whatever lives as long as the process (thread
    // pools); the second is the reference.
    run(None);
    let plain = run(None);
    assert!(plain.checkpoints.is_empty());
    println!("no run directory: heap peak {:.1} MiB", mib(plain.peak));

    let page = CHECKPOINT_PAGE_BYTES as u64;
    let modes = [
        ("stream", CheckpointMode::PauseAndStream, 0),
        (
            "background-4-pages",
            CheckpointMode::BoundedBackground,
            4 * page,
        ),
        ("background-1-page", CheckpointMode::BoundedBackground, page),
    ];
    let mut reference: Option<Vec<(u64, String)>> = None;
    for (name, mode, budget) in modes {
        let persistence = persistence(name, mode, budget);
        let ran = run(Some(&persistence));
        let admission = ran
            .admission
            .expect("a run directory reports its admission");
        assert_eq!(admission.mode(), mode, "{admission:?}");
        assert_eq!(admission.fallback, None);
        assert_eq!(admission.retained_bytes, budget);
        assert_eq!(admission.peak_bytes, CHECKPOINT_FIXED_BYTES + budget);
        assert_eq!(ran.digest, plain.digest, "{name}: the world reached");
        let stored: Vec<(u64, String)> = ran
            .checkpoints
            .iter()
            .map(|(tick, sha256, _, _)| (*tick, sha256.clone()))
            .collect();
        assert_eq!(
            stored.iter().map(|(tick, _)| *tick).collect::<Vec<_>>(),
            [0, EVERY, TICKS],
            "{name}"
        );
        for &(tick, _, taken, retained) in &ran.checkpoints {
            assert_eq!(taken, mode.name(), "{name} at {tick}");
            if mode == CheckpointMode::PauseAndStream {
                assert_eq!(retained, 0);
            } else {
                assert!(
                    retained > 0 && retained <= budget,
                    "{name} at {tick}: {retained} retained of {budget}"
                );
            }
        }
        match &reference {
            None => reference = Some(stored),
            Some(reference) => assert_eq!(&stored, reference, "{name}: the same checkpoints"),
        }
        // The last checkpoint restores to the world the run reached.
        let store = ChunkStore::open(persistence.run_dir.join("store"), Durability::ProcessCrash)
            .expect("opens");
        let newest = *store.sequences().expect("lists").last().expect("one");
        let restored = restore(
            &store.load(newest).expect("loads"),
            NumericalProfile::reference_f32().expect("the profile"),
        )
        .expect("restores");
        assert_eq!(restored.world.digest().to_hex(), plain.digest, "{name}");
        drop(restored);
        let beyond = ran.peak.saturating_sub(plain.peak);
        println!(
            "{name}: heap peak {:.1} MiB, {:.2} MiB beyond the run without checkpoints \
             (admitted {:.2} MiB + {:.0} MiB run directory); retained pages {:?}",
            mib(ran.peak),
            mib(beyond),
            mib(admission.peak_bytes),
            mib(RUN_DIRECTORY_ALLOWANCE),
            ran.checkpoints.iter().map(|c| c.3).collect::<Vec<_>>()
        );
        assert!(
            beyond <= admission.peak_bytes + RUN_DIRECTORY_ALLOWANCE,
            "{name}: {beyond} beyond the run without checkpoints"
        );
    }
}

/// [`admit`] by the numbers on the preset (2 GiB total, 1,207,959,552
/// genome bytes): `pause-and-stream` peaks at [`CHECKPOINT_FIXED_BYTES`];
/// `bounded-background` adds its budget while that fits the headroom and
/// falls back, saying why, when the budget holds less than a page or the
/// peak exceeds the headroom; a configuration whose headroom cannot hold
/// even a streamed checkpoint is refused, and `run_with` refuses it before
/// it creates the run directory.
#[test]
fn admission_follows_the_budget_by_the_numbers() {
    let _serial = serial();
    let config = preset();
    let headroom: u64 = 2_147_483_648 - 1_207_959_552;
    let page = CHECKPOINT_PAGE_BYTES as u64;
    let streamed = admit(&config, CheckpointMode::PauseAndStream, 1 << 30).expect("admits");
    assert_eq!(
        (
            streamed.admitted,
            streamed.retained_bytes,
            streamed.peak_bytes
        ),
        (PAUSE_AND_STREAM, 0, CHECKPOINT_FIXED_BYTES)
    );
    assert_eq!(streamed.headroom_bytes, headroom);
    for budget in [page, 128 << 20, headroom - CHECKPOINT_FIXED_BYTES] {
        let admitted = admit(&config, CheckpointMode::BoundedBackground, budget).expect("admits");
        assert_eq!(admitted.admitted, BOUNDED_BACKGROUND, "{budget}");
        assert_eq!(admitted.peak_bytes, CHECKPOINT_FIXED_BYTES + budget);
        assert_eq!(admitted.fallback, None);
    }
    for (budget, why) in [
        (0, "less than one"),
        (page - 1, "less than one"),
        (headroom - CHECKPOINT_FIXED_BYTES + 1, "exceeds"),
    ] {
        let fell = admit(&config, CheckpointMode::BoundedBackground, budget).expect("admits");
        assert_eq!(fell.declared, BOUNDED_BACKGROUND);
        assert_eq!(fell.admitted, PAUSE_AND_STREAM, "{budget}");
        assert_eq!(fell.peak_bytes, CHECKPOINT_FIXED_BYTES);
        let reason = fell.fallback.expect("a reason");
        assert!(reason.contains(why), "{budget}: {reason}");
    }

    let tight = preset_with(
        "maximum_total_bytes",
        1_207_959_552 + CHECKPOINT_FIXED_BYTES - 1,
    );
    let refused = admit(&tight, CheckpointMode::PauseAndStream, 0);
    assert!(
        matches!(
            refused,
            Err(CoreError::BudgetExhausted {
                budget: "checkpoint memory",
                required: CHECKPOINT_FIXED_BYTES,
                available,
            }) if available == CHECKPOINT_FIXED_BYTES - 1
        ),
        "{refused:?}"
    );
    let run_dir = fresh("refused");
    let refused = run_with(
        &tight,
        RunOptions {
            ticks: 1,
            every: 0,
            cpu_workers: None,
            controllers: ControllerChoice::Cpu,
            metrics_every: 0,
        },
        &[],
        Some(&Persistence::new(&run_dir, &tight)),
        |_: &Record| {},
    );
    assert!(
        matches!(
            refused,
            Err(CoreError::BudgetExhausted {
                budget: "checkpoint memory",
                ..
            })
        ),
        "{:?}",
        refused.err()
    );
    assert!(!run_dir.exists(), "nothing was created");
    let fits = preset_with(
        "maximum_total_bytes",
        1_207_959_552 + CHECKPOINT_FIXED_BYTES,
    );
    assert!(admit(&fits, CheckpointMode::PauseAndStream, 0).is_ok());

    // The configuration names the mode a run directory takes.
    let named = ValidatedConfig::from_toml(&PRESET.replace(
        "checkpoint_mode = \"pause-and-stream\"",
        "checkpoint_mode = \"bounded-background\"",
    ))
    .expect("bounded-background is a registered mode");
    assert_eq!(
        Persistence::new(scratch(), &named).mode,
        CheckpointMode::BoundedBackground
    );
    assert_eq!(
        Persistence::new(scratch(), &config).mode,
        CheckpointMode::PauseAndStream
    );
}

/// A run asked for `bounded-background` with a budget smaller than a page
/// takes its checkpoints as `pause-and-stream`, and its start record says
/// why.
#[test]
fn a_run_whose_budget_cannot_hold_a_page_streams_and_says_why() {
    let _serial = serial();
    let persistence = persistence("fallback", CheckpointMode::BoundedBackground, 4096);
    let ran = run(Some(&persistence));
    let admission = ran.admission.expect("reported");
    assert_eq!(admission.declared, BOUNDED_BACKGROUND);
    assert_eq!(admission.admitted, PAUSE_AND_STREAM);
    assert!(
        admission
            .fallback
            .as_deref()
            .is_some_and(|reason| reason.contains("4096")),
        "{admission:?}"
    );
    assert!(!ran.checkpoints.is_empty());
    for (_, _, mode, retained) in &ran.checkpoints {
        assert_eq!((*mode, *retained), (PAUSE_AND_STREAM, 0));
    }
}
