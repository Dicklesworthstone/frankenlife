//! `flife render --run-dir` (plan §13.8's checkpoint replay export; FL-023,
//! G10): a run directory rendered from the state `flife resume` continues
//! it from, its commands applying at their ticks, and nothing written to
//! it.
//!
//! - A branch with a brush renders from its own founding checkpoint to the
//!   world `flife resume` of the same branch reaches, frame by frame at the
//!   ticks asked for; its control renders to another world, the parent's.
//! - A write-ahead run whose newest checkpoint is damaged renders from the
//!   one before, its journal replayed past it, to the world the run never
//!   interrupted reaches.
//! - A run another process holds, a directory holding no run, and a video
//!   without an encoder are refused before any frame, and the directory is
//!   left as it was.

#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use flife_cli::{ControllerChoice, RenderOptions, RenderRecord, Style, render_run};
use flife_core::canonical::Sha256Digest;
use flife_runtime::Journal as _;
use flife_store::{CommandJournal, Durability};
use serde_json::{Value, json};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// The ticks every run here reaches.
const HORIZON: u64 = 240;

/// Energy brushes at ticks 3, 66, 150 and 230: commands 1 to 4.
const INJECTIONS: [&str; 4] = ["3:5,6:2.0", "66:9,2:3.0", "150:20,11:4.0", "230:1,30:1.0"];

/// Where the parent stops, and the checkpoint the branches start from.
const PARENT_TICKS: u64 = 120;
const FORK: u64 = 112;

/// The intervention branch's brush.
const BRUSH: &str = "130:9,2:30.0";

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

/// FL-037's 32 × 32 ecology of 48 founders, with `durability`, written to
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
    let path = scratch().join(format!("ecology-{durability}.toml"));
    write_whole(&path, &text);
    path
}

/// Write `text` to `path` through a uniquely named temporary file renamed
/// into place: tests run in parallel and write the same configurations.
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
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("flife-render-run");
    fs::create_dir_all(&path).expect("creates the scratch directory");
    path
}

/// A path that does not exist yet.
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

/// Run `flife` to completion with `path` as its `PATH` (inherited when
/// `None`): its status, stdout records and stderr records.
fn flife_on(args: &[&str], path: Option<&str>) -> (Option<i32>, Vec<Value>, Vec<Value>) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_flife"));
    command.args(args);
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let output = common::output(&mut command);
    (
        output.status.code(),
        lines(&output.stdout),
        lines(&output.stderr),
    )
}

fn flife(args: &[&str]) -> (Option<i32>, Vec<Value>, Vec<Value>) {
    flife_on(args, None)
}

/// `flife run` of `config` to `ticks` in `run_dir` with [`INJECTIONS`],
/// checkpointing every 7 ticks: its records.
fn run(config: &Path, run_dir: &Path, ticks: u64) -> Vec<Value> {
    let ticks = ticks.to_string();
    let mut args = vec![
        "run",
        text(config),
        "--ticks",
        &ticks,
        "--every",
        "0",
        "--run-dir",
        text(run_dir),
        "--checkpoint-every",
        "7",
        "--checkpoint-durability",
        "process-crash",
    ];
    for injection in INJECTIONS {
        args.extend(["--inject", injection]);
    }
    let (status, out, err) = flife(&args);
    assert_eq!(status, Some(0), "{err:?}");
    out
}

/// `flife resume` of `run_dir` to `until`: its records.
fn resume(run_dir: &Path, until: u64) -> Vec<Value> {
    let (status, out, err) = flife(&[
        "resume",
        text(run_dir),
        "--until",
        &until.to_string(),
        "--every",
        "0",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    out
}

/// `flife branch` of `parent` at `at` into `run_dir` with `extra` flags.
fn branch(parent: &Path, run_dir: &Path, at: u64, extra: &[&str]) {
    let at = at.to_string();
    let mut args = vec![
        "branch",
        text(parent),
        text(run_dir),
        "--at",
        &at,
        "--checkpoint-durability",
        "process-crash",
    ];
    args.extend(extra);
    let (status, out, err) = flife(&args);
    assert_eq!(status, Some(0), "{err:?}");
    assert_eq!(out.len(), 1, "{out:?}");
}

/// `flife render --run-dir run_dir` of `ticks` ticks, a 48 × 48 frame every
/// `every` into `out`, with `extra` flags.
fn render_args(run_dir: &Path, out: &Path, ticks: u64, every: u64) -> Vec<String> {
    [
        "render",
        "--run-dir",
        text(run_dir),
        "--ticks",
        &ticks.to_string(),
        "--every",
        &every.to_string(),
        "--width",
        "48",
        "--height",
        "48",
        "--out",
        text(out),
    ]
    .map(str::to_owned)
    .to_vec()
}

fn strs(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).collect()
}

/// The one record of `kind`.
fn one<'a>(records: &'a [Value], kind: &str) -> &'a Value {
    let found: Vec<&Value> = records
        .iter()
        .filter(|record| record["record"] == kind)
        .collect();
    assert_eq!(found.len(), 1, "one {kind} record in {records:?}");
    found[0]
}

/// The committed digest a run or resume reached, checking it reached `tick`.
fn reached(records: &[Value], tick: u64) -> Value {
    let end = one(records, "end");
    assert_eq!(end["stop"]["end"], "reached", "{end}");
    assert_eq!(end["tick"], tick);
    end["digest"].clone()
}

/// Every file under `dir` by its path relative to it, with its contents'
/// digest.
fn files(dir: &Path) -> BTreeMap<String, Sha256Digest> {
    let mut found = BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(&next).expect("lists the directory") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let name = path
                    .strip_prefix(dir)
                    .expect("under the directory")
                    .display()
                    .to_string();
                let bytes = fs::read(&path).expect("reads the file");
                found.insert(name, Sha256Digest::of(&bytes));
            }
        }
    }
    found
}

/// What `run_dir` holds: every file but the catalog's by its bytes, and the
/// catalog by its contents: its run, the run's journal and lineage, and the
/// checkpoints it indexes. Opening a catalog, as every reader of a run does,
/// lets FrankenSQLite rewrite the log files beside it; its contents do not
/// change.
fn held(run_dir: &Path) -> (BTreeMap<String, Sha256Digest>, String) {
    let files = files(run_dir)
        .into_iter()
        .filter(|(name, _)| !name.starts_with("catalog.db"))
        .collect();
    let mut catalog = CommandJournal::open(&run_dir.join("catalog.db")).expect("the catalog opens");
    let runs = catalog.runs().expect("its runs");
    let [(session, revision)] = runs[..] else {
        panic!("one run: {runs:?}");
    };
    let journal = catalog.open_run(session, revision).expect("its journal");
    let lineage = catalog.lineage().expect("its lineage");
    let checkpoints = catalog.checkpoints().expect("its checkpoints");
    (
        files,
        format!("{runs:?} {journal:?} {lineage:?} {checkpoints:?}"),
    )
}

/// The frame records' ticks, each frame checked to be a 48 × 48 PNG.
fn frame_ticks(records: &[Value]) -> Vec<u64> {
    records
        .iter()
        .filter(|record| record["record"] == "frame")
        .map(|record| {
            let png = fs::read(record["path"].as_str().expect("a path")).expect("the frame");
            assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
            assert_eq!(&png[16..24], &[0, 0, 0, 48, 0, 0, 0, 48]);
            record["tick"].as_u64().expect("a tick")
        })
        .collect()
}

/// A branch with a brush renders from its own founding checkpoint, the
/// brush and the inherited injections applying at their ticks, to the world
/// `flife resume` of the branch reaches; the control branch renders to the
/// parent's. Nothing is written to either directory, and a render after
/// the branch is resumed starts where the resume stopped.
#[test]
fn a_branch_renders_to_the_world_its_resume_reaches_and_is_left_as_it_was() {
    let config = ecology("volatile-in-memory");
    let parent = fresh("parent");
    run(&config, &parent, PARENT_TICKS);
    let control = fresh("control");
    branch(&parent, &control, FORK, &[]);
    let treated = fresh("treated");
    branch(&parent, &treated, FORK, &["--inject", BRUSH]);

    let mut rendered = BTreeMap::new();
    for (name, dir) in [("control", &control), ("treated", &treated)] {
        let before = held(dir);
        let frames = fresh(&format!("{name}-frames"));
        let (status, out, err) = flife(&strs(&render_args(dir, &frames, HORIZON - FORK, 32)));
        assert_eq!(status, Some(0), "{err:?}");
        assert_eq!(held(dir), before, "{name}: nothing is written to the run");
        let start = one(&out, "start");
        let from = &start["run"];
        assert_eq!(from["run_dir"], text(dir));
        assert_eq!(from["checkpoint"]["tick"], FORK, "{from}");
        assert_eq!(from["skipped"], json!([]));
        assert_eq!(from["journal"], Value::Null, "a volatile run");
        assert_eq!(from["tick"], FORK);
        assert_eq!(frame_ticks(&out), [112, 144, 176, 208, 240]);
        let end = one(&out, "end");
        assert_eq!((&end["frames"], &end["tick"]), (&json!(5), &json!(HORIZON)));
        rendered.insert(name, (from.clone(), end["digest"].clone()));
    }
    let (control_from, control_digest) = &rendered["control"];
    let (treated_from, treated_digest) = &rendered["treated"];
    // The parent's two injections still due at the fork, and the brush.
    assert_eq!(control_from["pending"], 2);
    assert_eq!(treated_from["pending"], 3);
    assert_eq!(
        control_from["digest"], treated_from["digest"],
        "the same world at the fork"
    );
    assert_ne!(control_digest, treated_digest, "the brush applied");

    // The render reaches the world resume reaches, branch by branch; the
    // control's is the parent's.
    assert_eq!(
        *treated_digest,
        reached(&resume(&treated, HORIZON), HORIZON)
    );
    assert_eq!(
        *control_digest,
        reached(&resume(&control, HORIZON), HORIZON)
    );
    assert_eq!(*control_digest, reached(&resume(&parent, HORIZON), HORIZON));

    // Resumed, the branch renders from where the resume stopped: a single
    // frame of the same world, and nothing pending.
    let frames = fresh("treated-after-resume");
    let (status, out, err) = flife(&strs(&render_args(&treated, &frames, 0, 1)));
    assert_eq!(status, Some(0), "{err:?}");
    let from = &one(&out, "start")["run"];
    assert_eq!(
        (&from["tick"], &from["pending"]),
        (&json!(HORIZON), &json!(0))
    );
    assert_eq!(frame_ticks(&out), [HORIZON]);
    assert_eq!(one(&out, "end")["digest"], *treated_digest);
}

/// The options [`render_run`] is called with here: 48 × 48 frames every
/// `every` ticks into `out`, on the CPU controllers.
fn options(out: PathBuf, ticks: u64, every: u64) -> RenderOptions {
    RenderOptions {
        ticks,
        every,
        size: [48, 48],
        out,
        cpu_workers: NonZeroUsize::new(2),
        controllers: ControllerChoice::Cpu,
        render_threads: NonZeroUsize::MIN,
        video: None,
        fps: 30,
        style: Style::default(),
    }
}

/// A write-ahead run whose newest checkpoint is damaged renders from the one
/// before with its journal replayed past it, as `flife resume` recovers it,
/// to the world the uninterrupted run reaches.
#[test]
fn a_write_ahead_run_renders_past_its_checkpoint_through_its_journal() {
    let config = ecology("write-ahead-before-apply");
    let uninterrupted = fresh("uninterrupted");
    let reference = reached(&run(&config, &uninterrupted, HORIZON), HORIZON);

    let damaged = fresh("damaged-newest");
    let out = run(&config, &damaged, 152);
    let newest = out
        .iter()
        .find(|record| record["record"] == "checkpoint" && record["tick"] == 152)
        .and_then(|record| record["sequence"].as_u64())
        .expect("a checkpoint at 152");
    {
        let store = common::open_store(damaged.join("store"), Durability::ProcessCrash)
            .expect("the store opens");
        fs::write(store.manifest_path(newest), b"damaged").expect("damages the manifest");
    }
    let before = held(&damaged);

    let mut records = Vec::new();
    render_run(&damaged, &options(fresh("damaged-first"), 0, 1), |record| {
        records.push(record.clone());
    })
    .expect("renders the first frame");
    let Some(RenderRecord::Start {
        run: Some(from), ..
    }) = records.first()
    else {
        panic!("a start record with the run: {records:?}");
    };
    assert_eq!(from.checkpoint.map(|at| at.tick), Some(147));
    assert_eq!(from.skipped.len(), 1, "{from:?}");
    assert_eq!(from.skipped[0].sequence, newest);
    let journal = from.journal.expect("a write-ahead run's journal");
    assert_eq!((journal.admitted, journal.outcomes), (4, 3));
    // The journaled outcome of command 3, at tick 150, is replayed.
    assert!(from.tick > 150, "{from:?}");
    assert_eq!(from.pending, 1, "command 4, due at 230");
    let start = from.tick;

    let mut records = Vec::new();
    let mut released = None;
    render_run(
        &damaged,
        &options(fresh("damaged-frames"), HORIZON - start, 16),
        |record| {
            // The store is released before the first frame: another
            // writer can open it while the render runs.
            if released.is_none() && matches!(record, RenderRecord::Frame { .. }) {
                released = Some(
                    common::open_store(damaged.join("store"), Durability::ProcessCrash)
                        .map(drop)
                        .map_err(|error| error.to_string()),
                );
            }
            records.push(record.clone());
        },
    )
    .expect("renders");
    assert_eq!(released, Some(Ok(())), "the store is released");
    let Some(RenderRecord::End { tick, digest, .. }) = records.last() else {
        panic!("an end record: {records:?}");
    };
    assert_eq!(*tick, HORIZON);
    assert_eq!(json!(digest), reference);
    assert_eq!(held(&damaged), before, "nothing is written to the run");
}

/// A run another process holds, a directory holding no run, both a
/// configuration and a run directory, and a video with no encoder are
/// refused before any frame or record, and the run is left as it was.
#[test]
fn what_cannot_be_rendered_is_refused_before_the_first_frame() {
    let config = ecology("volatile-in-memory");
    let run_dir = fresh("refused");
    run(&config, &run_dir, 10);
    let before = held(&run_dir);

    let frames = fresh("refused-frames");
    let args = render_args(&run_dir, &frames, 4, 1);
    {
        let _held = common::open_store(run_dir.join("store"), Durability::ProcessCrash)
            .expect("the store opens");
        let (status, out, err) = flife(&strs(&args));
        assert_eq!(status, Some(1), "{out:?} {err:?}");
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(err[0]["kind"], "state-conflict", "{err:?}");
        assert!(
            err[0]["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("another writer holds")),
            "{err:?}"
        );
    }

    // Without ffmpeg on the PATH, a video is refused before the run is read.
    let mut video = args.clone();
    video.extend([
        "--video".to_owned(),
        text(&scratch().join("refused.mp4")).to_owned(),
    ]);
    let (status, out, err) = flife_on(&strs(&video), Some(""));
    assert_eq!(status, Some(1), "{out:?} {err:?}");
    assert!(out.is_empty(), "{out:?}");
    assert_eq!(err[0]["kind"], "missing-capability", "{err:?}");

    let empty = fresh("refused-empty");
    fs::create_dir_all(&empty).expect("creates a directory");
    let (status, out, err) = flife(&strs(&render_args(&empty, &frames, 4, 1)));
    assert_eq!(status, Some(2), "{out:?} {err:?}");
    assert!(out.is_empty(), "{out:?}");
    assert_eq!(err.len(), 1, "{err:?}");

    // Clap's usage error, in text.
    let both = common::output(Command::new(env!("CARGO_BIN_EXE_flife")).args([
        "render",
        text(&config),
        "--run-dir",
        text(&run_dir),
        "--out",
        text(&frames),
    ]));
    assert_eq!(both.status.code(), Some(2), "a usage error");
    assert!(both.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&both.stderr).contains("cannot be used with"),
        "{}",
        String::from_utf8_lossy(&both.stderr)
    );

    assert!(!frames.exists(), "no frame directory");
    assert_eq!(held(&run_dir), before, "the run is left as it was");

    // Released, the same render runs.
    let (status, out, err) = flife(&strs(&args));
    assert_eq!(status, Some(0), "{err:?}");
    assert_eq!(frame_ticks(&out), [10, 11, 12, 13, 14]);
    assert_eq!(held(&run_dir), before);
}
