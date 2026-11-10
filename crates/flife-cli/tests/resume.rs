//! FL-037 evidence (plan G3, §12.3, §16.3–§16.7, C.13, C.14): a run kept in
//! a run directory and stopped, by a damaged newest checkpoint or by
//! SIGKILL part way, resumes with `flife resume` to exactly the state of the
//! uninterrupted run: the same committed world digest and byte-identical
//! final checkpoint (world and command closure). The recovered checkpoint
//! holds controller outputs evaluated before it, cooldowns running across
//! it, newborn genomes and future commands; the RNG is addressed by tick and
//! identity (§16.2), so births and mutations after it reproduce only if the
//! restored state is exact.

#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

mod common;

use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use flife_core::ids::CommandId;
use flife_core::{Tick, ValidatedConfig};
use flife_engine::NumericalProfile;
use flife_runtime::{CommandStatus, Journal, Outcome, restore};
use flife_store::{CommandJournal, Durability};
use serde_json::{Value, json};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// The ticks every run here reaches.
const HORIZON: u64 = 240;

/// Energy brushes at ticks 3, 66, 150 and 230: commands 1 to 4.
const INJECTIONS: [&str; 4] = ["3:5,6:2.0", "66:9,2:3.0", "150:20,11:4.0", "230:1,30:1.0"];

/// Appendix A with `extra` overrides.
fn appendix_a(extra: &[(&str, &str, &str)]) -> String {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    let mut lines: Vec<String> = appendix[start..start + length]
        .lines()
        .map(str::to_owned)
        .collect();
    for &(section, key, value) in extra {
        let header = format!("[{section}]");
        let first = lines
            .iter()
            .position(|line| line.trim() == header)
            .expect("a section");
        let line = lines[first + 1..]
            .iter()
            .take_while(|line| !line.starts_with('['))
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .map(|offset| first + 1 + offset)
            .expect("a key");
        lines[line] = format!("{key} = {value}");
    }
    lines.join("\n")
}

/// A 32 × 32 ecology of 48 founders that breeds within the horizon, its
/// controllers evaluated every 4 ticks in per-organism phases, with
/// `durability` (Appendix A declares `write-ahead-before-apply`), written to
/// a file.
fn ecology(durability: &str) -> PathBuf {
    let text = appendix_a(&[
        ("world", "width", "32.0"),
        ("world", "height", "32.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", "48"),
        ("population", "maximum_count", "1024"),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "4"),
        ("sources", "power_per_source", "120.0"),
        ("sources", "sigma_world_units", "3.0"),
        ("sources", "path_direction_interval_seconds", "2.0"),
        ("chemistry", "uptake_capacity_per_second", "8.0"),
        ("costs", "basal_energy_per_second", "0.6"),
        (
            "reproduction",
            "minimum_parent_energy_before_attempt",
            "2.2",
        ),
        ("reproduction", "maximum_attempt_rate_per_second", "4.0"),
        ("reproduction", "cooldown_seconds", "0.25"),
        ("controller", "evaluation_interval_ticks", "4"),
        (
            "execution",
            "command_durability",
            &format!("\"{durability}\""),
        ),
    ]);
    let config = ValidatedConfig::from_toml(&text).expect("a valid configuration");
    assert_eq!(config.config().execution.command_durability, durability);
    let path = scratch().join(format!("ecology-{durability}.toml"));
    write_whole(&path, &text);
    path
}

/// Write `text` to `path` through a uniquely named temporary file renamed
/// into place. Tests run in parallel and several write the same
/// configuration path: written in place, a `flife` started by another test
/// could read it truncated (seen on Linux: "missing field `schema`").
fn write_whole(path: &Path, text: &str) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temporary = path.with_extension(format!(
        "{}.{}.partial",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&temporary, text).expect("writes the configuration");
    fs::rename(&temporary, path).expect("renames it into place");
}

fn scratch() -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("flife-resume");
    fs::create_dir_all(&path).expect("creates the scratch directory");
    path
}

/// A run directory path that does not exist yet.
fn fresh(name: &str) -> PathBuf {
    let path = scratch().join(name);
    if path.exists() {
        fs::remove_dir_all(&path).expect("clears the last run's directory");
    }
    path
}

fn text(path: &Path) -> &str {
    path.to_str().expect("UTF-8")
}

fn lines(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line is one JSON record"))
        .collect()
}

/// Run `flife` to completion: its status, stdout records and stderr
/// records.
fn flife(args: &[&str]) -> (Option<i32>, Vec<Value>, Vec<Value>) {
    let output = common::output(Command::new(env!("CARGO_BIN_EXE_flife")).args(args));
    (
        output.status.code(),
        lines(&output.stdout),
        lines(&output.stderr),
    )
}

/// `flife run` keeping a run directory, checkpointing every 7 ticks
/// (process-crash durability), with [`INJECTIONS`], to `ticks`.
fn run_args(config: &Path, run_dir: &Path, ticks: u64, every: u64) -> Vec<String> {
    let mut args = run_args_flushed(config, run_dir, ticks, every);
    args.extend([
        "--checkpoint-durability".to_owned(),
        "process-crash".to_owned(),
    ]);
    args
}

/// [`run_args`] with the default `power-loss` durability.
fn run_args_flushed(config: &Path, run_dir: &Path, ticks: u64, every: u64) -> Vec<String> {
    let mut args: Vec<String> = [
        "run",
        text(config),
        "--ticks",
        &ticks.to_string(),
        "--every",
        &every.to_string(),
        "--run-dir",
        text(run_dir),
        "--checkpoint-every",
        "7",
    ]
    .map(str::to_owned)
    .to_vec();
    for injection in INJECTIONS {
        args.extend(["--inject".to_owned(), injection.to_owned()]);
    }
    args
}

/// `flife resume` of `run_dir` to [`HORIZON`], checkpointing as the run did.
fn resume(run_dir: &Path) -> (Option<i32>, Vec<Value>, Vec<Value>) {
    flife(&[
        "resume",
        text(run_dir),
        "--until",
        &HORIZON.to_string(),
        "--every",
        "0",
        "--checkpoint-every",
        "7",
        "--checkpoint-durability",
        "process-crash",
    ])
}

fn strs(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

/// Every record of `kind`.
fn all<'a>(records: &'a [Value], kind: &str) -> Vec<&'a Value> {
    records
        .iter()
        .filter(|record| record["record"] == kind)
        .collect()
}

/// The one record of `kind`.
fn one<'a>(records: &'a [Value], kind: &str) -> &'a Value {
    let found = all(records, kind);
    assert_eq!(found.len(), 1, "one {kind} record in {records:?}");
    found[0]
}

/// The checkpoint record at `tick`.
fn checkpoint_at(records: &[Value], tick: u64) -> &Value {
    all(records, "checkpoint")
        .into_iter()
        .find(|record| record["tick"] == tick)
        .unwrap_or_else(|| panic!("a checkpoint at tick {tick}"))
}

/// What the uninterrupted write-ahead run reaches: its committed digest at
/// [`HORIZON`], its final checkpoint's SHA-256, and every checkpoint's tick
/// and SHA-256.
struct Uninterrupted {
    digest: Value,
    last: Value,
    checkpoints: Vec<(u64, Value)>,
}

/// Each checkpoint record's tick and SHA-256.
fn stored(records: &[Value]) -> Vec<(u64, Value)> {
    all(records, "checkpoint")
        .iter()
        .map(|record| {
            (
                record["tick"].as_u64().expect("a tick"),
                record["sha256"].clone(),
            )
        })
        .collect()
}

/// The catalog of `run_dir`, run with the configuration at `config`, indexes
/// every checkpoint `out` published, retired or not, under its sequence,
/// tick and SHA-256.
#[track_caller]
fn indexes_what_it_published(run_dir: &Path, config: &Path, out: &[Value]) {
    let mut catalog = CommandJournal::open(&run_dir.join("catalog.db")).expect("opens");
    let hash = ValidatedConfig::from_toml(&fs::read_to_string(config).expect("reads"))
        .expect("valid")
        .config_hash();
    catalog
        .open_run(flife_cli::RUN_SESSION, hash)
        .expect("the run is recorded under its configuration");
    let indexed: Vec<(u64, u64, String)> = catalog
        .checkpoints()
        .expect("lists")
        .iter()
        .map(|row| (row.sequence, row.tick.get(), row.digest.to_hex()))
        .collect();
    let published: Vec<(u64, u64, String)> = all(out, "checkpoint")
        .iter()
        .map(|record| {
            (
                record["sequence"].as_u64().expect("a sequence"),
                record["tick"].as_u64().expect("a tick"),
                record["sha256"].as_str().expect("a digest").to_owned(),
            )
        })
        .collect();
    assert_eq!(indexed, published);
}

fn uninterrupted() -> &'static Uninterrupted {
    static REFERENCE: OnceLock<Uninterrupted> = OnceLock::new();
    REFERENCE.get_or_init(|| {
        let config = ecology("write-ahead-before-apply");
        let run_dir = fresh("uninterrupted");
        let (status, out, err) = flife(&strs(&run_args(&config, &run_dir, HORIZON, 0)));
        assert_eq!(status, Some(0), "{err:?}");
        let end = one(&out, "end");
        assert_eq!(end["stop"]["end"], "reached");
        assert_eq!(end["tick"], HORIZON);
        assert!(
            end["tally"]["births"]
                .as_u64()
                .is_some_and(|births| births > 0),
            "the ecology breeds: {end}"
        );
        // The founding state (with its commands) and every multiple of 7
        // are published, then the end.
        let ticks: Vec<u64> = all(&out, "checkpoint")
            .iter()
            .map(|record| record["tick"].as_u64().expect("a tick"))
            .collect();
        let expected: Vec<u64> = (0..=HORIZON)
            .filter(|tick| tick.is_multiple_of(7))
            .chain([HORIZON])
            .collect();
        assert_eq!(ticks, expected);
        let persisted = &end["persisted"];
        assert_eq!(persisted["durability"], "write-ahead-before-apply");
        assert_eq!(persisted["journal"], json!({"admitted": 4, "outcomes": 4}));
        assert_eq!(persisted["checkpoint"]["tick"], HORIZON);
        // The configuration's retain_recent_checkpoints is 4.
        let store = common::open_store(run_dir.join("store"), Durability::ProcessCrash)
            .expect("the store opens");
        assert_eq!(store.sequences().expect("lists").len(), 4);
        indexes_what_it_published(&run_dir, &config, &out);
        Uninterrupted {
            digest: end["digest"].clone(),
            last: checkpoint_at(&out, HORIZON)["sha256"].clone(),
            checkpoints: stored(&out),
        }
    })
}

/// A resumed run reached the uninterrupted run's state at the horizon: the
/// same digest and a byte-identical final checkpoint.
#[track_caller]
fn reaches_the_uninterrupted_state(out: &[Value], reference: (&Value, &Value)) {
    let end = one(out, "end");
    assert_eq!(end["stop"]["end"], "reached");
    assert_eq!(end["tick"], HORIZON);
    assert_eq!(&end["digest"], reference.0, "the committed world");
    assert_eq!(
        &checkpoint_at(out, HORIZON)["sha256"],
        reference.1,
        "the final checkpoint's bytes"
    );
}

/// Run `args` and SIGKILL the process once it has reported tick `tick`; its
/// records up to its death.
fn killed_after(args: &[&str], tick: u64) -> Vec<Value> {
    killed_when(args, tick, || ())
}

/// Run `args`, call `meanwhile` once the process has reported tick `tick`,
/// then SIGKILL it; its records up to its death.
fn killed_when(args: &[&str], tick: u64, meanwhile: impl FnOnce()) -> Vec<Value> {
    let mut child = common::spawn(
        Command::new(env!("CARGO_BIN_EXE_flife"))
            .args(args)
            .stdout(Stdio::piped()),
    );
    let stdout = child.stdout.take().expect("its output");
    let mut reader = BufReader::new(stdout).lines();
    let mut records = Vec::new();
    for line in reader.by_ref() {
        let record: Value = serde_json::from_str(&line.expect("a line")).expect("JSON");
        let reached = record["record"] == "progress" && record["tick"] == tick;
        records.push(record);
        if reached {
            break;
        }
    }
    meanwhile();
    child.kill().expect("SIGKILL");
    for line in reader {
        records.push(serde_json::from_str(&line.expect("a line")).expect("JSON"));
    }
    let status = child.wait().expect("reaped");
    assert_eq!(status.signal(), Some(9), "killed, not finished: {status:?}");
    assert!(all(&records, "end").is_empty(), "the run did not end");
    records
}

/// The deterministic case. A run stopped at tick 152 whose newest
/// checkpoint (152) is damaged resumes from the one before (147, between
/// controller evaluations, with cooldowns running, newborns, and commands 3
/// and 4 pending), replays command 3's journaled outcome at tick 150, and
/// continues to the uninterrupted state. The damaged checkpoint is reported
/// and passed over, not used.
#[test]
fn a_run_resumes_from_its_newest_restorable_checkpoint_to_the_uninterrupted_state() {
    let reference = uninterrupted();
    let config = ecology("write-ahead-before-apply");
    let run_dir = fresh("damaged-newest");
    let (status, out, err) = flife(&strs(&run_args(&config, &run_dir, 152, 0)));
    assert_eq!(status, Some(0), "{err:?}");
    let end = one(&out, "end");
    assert_eq!(end["tick"], 152);
    assert_eq!(
        end["persisted"]["journal"],
        json!({"admitted": 4, "outcomes": 3})
    );
    let newest = checkpoint_at(&out, 152)["sequence"]
        .as_u64()
        .expect("a sequence");
    let before = checkpoint_at(&out, 147)["sequence"]
        .as_u64()
        .expect("a sequence");

    {
        let store = common::open_store(run_dir.join("store"), Durability::ProcessCrash)
            .expect("the store opens");
        let bytes = store.load(before).expect("checkpoint 147 loads");
        let restored =
            restore(&bytes, NumericalProfile::reference_f32().expect("profile")).expect("restores");
        let at = Tick::new(147);
        assert_eq!(restored.world.tick(), at);
        let population: Vec<_> = restored.world.population().iter().collect();
        assert!(
            population.iter().any(|o| o
                .held
                .evaluated_at()
                .is_some_and(|evaluated| evaluated < at)),
            "controller outputs evaluated before the checkpoint are held across it"
        );
        assert!(
            population.iter().any(|o| o.cooldown_deadline > at),
            "a reproduction cooldown runs across it"
        );
        assert!(
            population.iter().any(|o| o.parent.is_some()),
            "newborn genomes cross it"
        );
        let status = |id: u64| restored.book.status(CommandId::new(id)).cloned();
        assert!(matches!(
            status(2),
            Some(CommandStatus::Terminal(Outcome::Applied { .. }))
        ));
        assert_eq!(status(3), Some(CommandStatus::Pending), "due at 150");
        assert_eq!(status(4), Some(CommandStatus::Pending), "due at 230");
        fs::write(store.manifest_path(newest), b"damaged").expect("damages the manifest");
    }

    let (status, out, err) = resume(&run_dir);
    assert_eq!(status, Some(0), "{err:?}");
    assert_eq!(out[0]["record"], "start");
    let resumed = one(&out, "resume");
    assert_eq!(
        resumed["checkpoint"],
        json!({"sequence": before, "tick": 147})
    );
    let skipped = resumed["skipped"].as_array().expect("a list");
    assert_eq!(skipped.len(), 1, "{resumed}");
    assert_eq!(skipped[0]["sequence"], newest);
    // The journal's outcome for command 3 is replayed past the checkpoint.
    assert_eq!(resumed["journal"], json!({"admitted": 4, "outcomes": 3}));
    assert!(
        resumed["tick"].as_u64().is_some_and(|tick| tick >= 150),
        "replayed past tick 150: {resumed}"
    );
    reaches_the_uninterrupted_state(&out, (&reference.digest, &reference.last));
    assert_eq!(
        one(&out, "end")["persisted"]["journal"],
        json!({"admitted": 4, "outcomes": 4})
    );
}

/// A write-ahead run SIGKILLed part way (after reporting tick 70, wherever
/// it then is) resumes to the uninterrupted state.
#[test]
fn a_killed_write_ahead_run_resumes_to_the_uninterrupted_state() {
    let reference = uninterrupted();
    let config = ecology("write-ahead-before-apply");
    let run_dir = fresh("killed-write-ahead");
    // No horizon: the kill, not the tick count, ends it.
    let records = killed_after(&strs(&run_args(&config, &run_dir, 1_000_000, 1)), 70);
    let published = all(&records, "checkpoint").len();
    let (status, out, err) = resume(&run_dir);
    assert_eq!(status, Some(0), "{err:?}");
    let resumed = one(&out, "resume");
    eprintln!("killed after {published} checkpoints; resumed: {resumed}");
    assert!(
        resumed["checkpoint"]["tick"]
            .as_u64()
            .is_some_and(|tick| tick >= 63),
        "{resumed}"
    );
    assert_eq!(resumed["journal"]["admitted"], 4);
    publishes_after(&out, resumed);
    reaches_the_uninterrupted_state(&out, (&reference.digest, &reference.last));
}

/// [`run_args`] taking checkpoints `bounded-background` with a one-page
/// budget, so the encoder waits on the writer (FL-038).
fn in_background(mut args: Vec<String>) -> Vec<String> {
    args.extend(
        [
            "--checkpoint-mode",
            "bounded-background",
            "--checkpoint-retained-bytes",
            "1048576",
        ]
        .map(str::to_owned),
    );
    args
}

/// FL-038: the write-ahead run with its commands, its checkpoints taken
/// `bounded-background`, stores the checkpoints the `pause-and-stream` run
/// stores, byte for byte at the same ticks, each retaining at most its
/// budget of pages while the run steps on; it indexes every one in the
/// catalog (on the paused session, while its journal is idle) and reaches
/// the same state. SIGKILLed part way, with a checkpoint perhaps in flight,
/// it resumes to the uninterrupted state.
#[test]
fn a_bounded_background_run_stores_the_same_checkpoints_and_resumes_after_a_kill() {
    let reference = uninterrupted();
    let config = ecology("write-ahead-before-apply");
    let run_dir = fresh("background");
    let (status, out, err) = flife(&strs(&in_background(run_args(
        &config, &run_dir, HORIZON, 0,
    ))));
    assert_eq!(status, Some(0), "{err:?}");
    let admission = &one(&out, "start")["checkpoints"];
    assert_eq!(admission["admitted"], "bounded-background", "{admission}");
    assert_eq!(admission["retained_bytes"], 1_048_576);
    assert_eq!(stored(&out), reference.checkpoints);
    for record in all(&out, "checkpoint") {
        assert_eq!(record["mode"], "bounded-background");
        assert!(
            record["retained_peak"]
                .as_u64()
                .is_some_and(|peak| peak > 0 && peak <= 1_048_576),
            "{record}"
        );
    }
    let end = one(&out, "end");
    assert_eq!(end["digest"], reference.digest);
    assert_eq!(
        end["persisted"]["journal"],
        json!({"admitted": 4, "outcomes": 4})
    );
    indexes_what_it_published(&run_dir, &config, &out);

    let run_dir = fresh("killed-background");
    let records = killed_after(
        &strs(&in_background(run_args(&config, &run_dir, 1_000_000, 1))),
        70,
    );
    let published = all(&records, "checkpoint").len();
    let (status, out, err) = resume(&run_dir);
    assert_eq!(status, Some(0), "{err:?}");
    let resumed = one(&out, "resume");
    eprintln!("killed after {published} background checkpoint records; resumed: {resumed}");
    publishes_after(&out, resumed);
    reaches_the_uninterrupted_state(&out, (&reference.digest, &reference.last));
}

/// A resumed run publishes nothing at or before the checkpoint it recovered:
/// that one is already stored.
#[track_caller]
fn publishes_after(out: &[Value], resumed: &Value) {
    let recovered = resumed["checkpoint"]["tick"]
        .as_u64()
        .expect("a checkpoint");
    let first = all(out, "checkpoint")[0]["tick"].as_u64().expect("a tick");
    assert!(first > recovered, "{first} after {recovered}");
}

/// A `volatile-in-memory` run keeps no journal, but its commands are in the
/// founding checkpoint: SIGKILLed part way, it resumes to the state of the
/// same configuration run uninterrupted without a run directory, so keeping
/// one changes nothing. It publishes under the default `power-loss`
/// durability.
#[test]
fn a_killed_volatile_run_resumes_to_the_uninterrupted_state() {
    let config = ecology("volatile-in-memory");
    let mut args = vec![
        "run".to_owned(),
        text(&config).to_owned(),
        "--ticks".to_owned(),
        HORIZON.to_string(),
        "--every".to_owned(),
        "0".to_owned(),
    ];
    for injection in INJECTIONS {
        args.extend(["--inject".to_owned(), injection.to_owned()]);
    }
    let (status, plain, err) = flife(&strs(&args));
    assert_eq!(status, Some(0), "{err:?}");
    let plain = one(&plain, "end");
    assert!(plain.get("persisted").is_none());

    let run_dir = fresh("killed-volatile");
    killed_after(
        &strs(&run_args_flushed(&config, &run_dir, 1_000_000, 1)),
        70,
    );
    let (status, out, err) = resume(&run_dir);
    assert_eq!(status, Some(0), "{err:?}");
    let resumed = one(&out, "resume");
    assert!(resumed["journal"].is_null(), "{resumed}");
    publishes_after(&out, resumed);
    let end = one(&out, "end");
    assert_eq!(end["tick"], HORIZON);
    assert_eq!(end["digest"], plain["digest"]);
    assert_eq!(end["persisted"]["durability"], "volatile-in-memory");
}

#[track_caller]
fn refused(outcome: (Option<i32>, Vec<Value>, Vec<Value>), kind: &str, detail: &str) {
    let (status, out, err) = outcome;
    assert_eq!(status, Some(2), "{out:?} {err:?}");
    assert!(out.is_empty(), "nothing ran: {out:?}");
    assert_eq!(err[0]["kind"], kind, "{err:?}");
    assert!(
        err[0]["detail"]
            .as_str()
            .is_some_and(|text| text.contains(detail)),
        "{detail}: {err:?}"
    );
}

/// A run directory is created once and continued only by `flife resume`;
/// a directory that is not one, a configuration edited after the run began,
/// a tick the run has passed and a run another process holds are refused
/// before anything runs.
#[test]
fn run_directories_refuse_what_they_cannot_continue() {
    let config = ecology("write-ahead-before-apply");
    let config = text(&config);
    let run_dir = fresh("refusals");
    let (status, out, err) = flife(&["run", config, "--ticks", "10", "--run-dir", text(&run_dir)]);
    assert_eq!(status, Some(0), "{err:?}");
    // The configured interval, 30 simulated seconds (3,840 ticks): the
    // founding checkpoint and the last.
    let ticks: Vec<&Value> = all(&out, "checkpoint")
        .iter()
        .map(|record| &record["tick"])
        .collect();
    assert_eq!(ticks, [&json!(0), &json!(10)]);
    refused(
        flife(&["run", config, "--ticks", "10", "--run-dir", text(&run_dir)]),
        "state-conflict",
        "already holds a run",
    );
    refused(
        flife(&["resume", text(&run_dir), "--until", "5"]),
        "state-conflict",
        "already at tick 10",
    );

    let other = fresh("refusals-other");
    fs::create_dir_all(&other).expect("creates a directory");
    fs::write(other.join("notes.txt"), "not a run").expect("writes a file");
    refused(
        flife(&["run", config, "--ticks", "10", "--run-dir", text(&other)]),
        "state-conflict",
        "is not empty",
    );
    refused(
        flife(&["resume", text(&other), "--until", "20"]),
        "state-conflict",
        "is not a run directory",
    );

    let edited = fresh("refusals-edited");
    let (status, _, err) = flife(&["run", config, "--ticks", "10", "--run-dir", text(&edited)]);
    assert_eq!(status, Some(0), "{err:?}");
    let path = edited.join("config.toml");
    let canonical = fs::read_to_string(&path).expect("reads the configuration");
    assert!(canonical.contains("seed = 42\n"));
    fs::write(&path, canonical.replace("seed = 42\n", "seed = 43\n")).expect("edits it");
    refused(
        flife(&["resume", text(&edited), "--until", "20"]),
        "state-conflict",
        "is recorded running configuration",
    );

    let held = fresh("refusals-held");
    killed_when(
        &strs(&run_args(Path::new(config), &held, 1_000_000, 1)),
        20,
        || {
            refused(
                flife(&["resume", text(&held), "--until", "2000000"]),
                "state-conflict",
                "another writer holds",
            );
        },
    );
}

/// A store holding another configuration's checkpoints is no starting point:
/// resume passes over each, naming why, and replays the journal from the
/// founding world to the same state as the run never interrupted.
#[test]
fn checkpoints_of_another_configuration_are_passed_over() {
    let config = ecology("write-ahead-before-apply");
    let foreign = ecology("volatile-in-memory");
    let own = fresh("foreign-own");
    let (status, _, err) = flife(&strs(&run_args(&config, &own, 14, 0)));
    assert_eq!(status, Some(0), "{err:?}");
    let other = fresh("foreign-other");
    let (status, _, err) = flife(&strs(&run_args(&foreign, &other, 14, 0)));
    assert_eq!(status, Some(0), "{err:?}");
    fs::rename(own.join("store"), own.join("store-own")).expect("sets the run's store aside");
    fs::rename(other.join("store"), own.join("store")).expect("moves the other store in");

    let (status, out, err) = flife(&[
        "resume",
        text(&own),
        "--until",
        "20",
        "--every",
        "0",
        "--checkpoint-every",
        "7",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    let resumed = one(&out, "resume");
    assert!(resumed["checkpoint"].is_null(), "{resumed}");
    let skipped = resumed["skipped"].as_array().expect("a list");
    assert_eq!(skipped.len(), 3, "0, 7 and 14: {resumed}");
    for passed in skipped {
        assert_eq!(passed["kind"], "state-conflict");
        assert!(
            passed["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("another configuration")),
            "{passed}"
        );
    }
    assert_eq!(resumed["journal"], json!({"admitted": 4, "outcomes": 1}));

    let direct = fresh("foreign-direct");
    let (status, plain, err) = flife(&strs(&run_args(&config, &direct, 20, 0)));
    assert_eq!(status, Some(0), "{err:?}");
    assert_eq!(one(&out, "end")["digest"], one(&plain, "end")["digest"]);
}

/// FL-022's removals at ticks 100 (a disk of radius 10 at the centre,
/// recycled) and 160 (radius 5 at the corner, across both seams,
/// dissipated): commands 5 and 6, after the injections.
const REMOVALS: [&str; 2] = ["100:16,16:10:recycle", "160:0.5,31.5:5:dissipate"];

/// [`run_args`] with [`REMOVALS`].
fn with_removals(config: &Path, run_dir: &Path, ticks: u64, every: u64) -> Vec<String> {
    let mut args = run_args(config, run_dir, ticks, every);
    for removal in REMOVALS {
        args.extend(["--remove".to_owned(), removal.to_owned()]);
    }
    args
}

/// FL-022: removals given on the command line are journaled, held pending
/// in checkpoints and replayed like any command. A run with them removes
/// organisms and ends elsewhere than the run without them. Stopped at tick
/// 102 with its newest checkpoint damaged, it resumes from tick 98, before
/// the first removal, replays that removal's journaled outcome at tick 100,
/// applies the second at 160 from the journal, and reaches the
/// uninterrupted state. SIGKILLed between the two, it does too.
#[test]
fn removals_are_journaled_checkpointed_and_replayed_like_any_command() {
    let reference = uninterrupted();
    let config = ecology("write-ahead-before-apply");
    let whole = fresh("removal-whole");
    let (status, out, err) = flife(&strs(&with_removals(&config, &whole, HORIZON, 0)));
    assert_eq!(status, Some(0), "{err:?}");
    let end = one(&out, "end");
    assert_eq!(
        end["persisted"]["journal"],
        json!({"admitted": 6, "outcomes": 6})
    );
    assert!(
        end["tally"]["deaths_by_stage"]["removed"]
            .as_u64()
            .is_some_and(|removed| removed > 0),
        "the removals removed organisms: {end}"
    );
    assert_ne!(end["digest"], reference.digest, "and changed the world");
    let removing = (
        end["digest"].clone(),
        checkpoint_at(&out, HORIZON)["sha256"].clone(),
    );

    let part = fresh("removal-damaged-newest");
    let (status, out, err) = flife(&strs(&with_removals(&config, &part, 102, 0)));
    assert_eq!(status, Some(0), "{err:?}");
    // Injections 1 and 2 and removal 5 are applied; 3, 4 and 6 are pending.
    assert_eq!(
        one(&out, "end")["persisted"]["journal"],
        json!({"admitted": 6, "outcomes": 3})
    );
    let newest = checkpoint_at(&out, 102)["sequence"]
        .as_u64()
        .expect("a sequence");
    let before = checkpoint_at(&out, 98)["sequence"]
        .as_u64()
        .expect("a sequence");
    {
        let store = common::open_store(part.join("store"), Durability::ProcessCrash)
            .expect("the store opens");
        let bytes = store.load(before).expect("checkpoint 98 loads");
        let restored =
            restore(&bytes, NumericalProfile::reference_f32().expect("profile")).expect("restores");
        let status = |id: u64| restored.book.status(CommandId::new(id)).cloned();
        assert_eq!(status(5), Some(CommandStatus::Pending), "due at 100");
        assert_eq!(status(6), Some(CommandStatus::Pending), "due at 160");
        fs::write(store.manifest_path(newest), b"damaged").expect("damages the manifest");
    }
    let (status, out, err) = resume(&part);
    assert_eq!(status, Some(0), "{err:?}");
    let resumed = one(&out, "resume");
    assert_eq!(
        resumed["checkpoint"],
        json!({"sequence": before, "tick": 98})
    );
    assert_eq!(resumed["journal"], json!({"admitted": 6, "outcomes": 3}));
    reaches_the_uninterrupted_state(&out, (&removing.0, &removing.1));
    assert_eq!(
        one(&out, "end")["persisted"]["journal"],
        json!({"admitted": 6, "outcomes": 6})
    );

    let killed = fresh("removal-killed");
    killed_after(&strs(&with_removals(&config, &killed, 1_000_000, 1)), 130);
    let (status, out, err) = resume(&killed);
    assert_eq!(status, Some(0), "{err:?}");
    assert_eq!(one(&out, "resume")["journal"]["admitted"], 6);
    reaches_the_uninterrupted_state(&out, (&removing.0, &removing.1));
}
