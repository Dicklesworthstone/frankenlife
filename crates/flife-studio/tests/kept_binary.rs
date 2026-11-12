//! FL-024: `flife-studio --run DIR`, the binary keeping its session in a
//! run directory. Its `open` record names the directory and the checkpoint
//! admission, charged in the memory admission; a `checkpoint` record
//! follows each checkpoint published, from tick 0 to the closing boundary,
//! which the `end` record names; the statuses reported while it closes are
//! printed before `end`; the window's telemetry line names the newest
//! checkpoint's tick (plan §14.1); and `flife inspect` restores the world
//! the Studio closed on. `flife-studio --resume DIR` then runs it on from
//! there: its `open` record names the checkpoint it resumed from, its
//! checkpoints follow the run's, and it closes on the world stepped directly
//! from the founding to its tick. A configuration beside `--resume` is
//! refused. Every run passes `--offscreen`, so none opens a window.
//! Headless Metal: without a Metal device the test returns early.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "test: a failure is a test failure; figures are printed"
)]

use std::path::PathBuf;
use std::process::Command;
use std::time::SystemTime;

use flife_cli::{CHECKPOINT_FIXED_BYTES, InspectOptions};
use flife_core::{Tick, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_studio::{TimelineRecord, read_timeline};
use fmn_render::metal::MetalRenderer;
use serde_json::Value;

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// The preset at 32 × 32 world units with 96 founders and a checkpoint
/// every 128 ticks (one simulated second), all retained.
fn configuration() -> String {
    let mut lines: Vec<String> = PRESET.lines().map(str::to_owned).collect();
    for (section, key, value) in [
        ("world", "width", "32.0"),
        ("world", "height", "32.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", "96"),
        ("population", "maximum_count", "1024"),
        (
            "persistence",
            "checkpoint_interval_simulation_seconds",
            "1.0",
        ),
        ("persistence", "retain_recent_checkpoints", "1000"),
    ] {
        let header = format!("[{section}]");
        let at = lines
            .iter()
            .position(|line| line.trim() == header)
            .unwrap_or_else(|| panic!("no section {header}"));
        let line = lines[at + 1..]
            .iter()
            .take_while(|line| !line.starts_with('['))
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .map(|offset| at + 1 + offset)
            .unwrap_or_else(|| panic!("no key {key} in {header}"));
        lines[line] = format!("{key} = {value}");
    }
    lines.join("\n")
}

fn number(record: &Value, key: &str) -> u64 {
    record[key]
        .as_u64()
        .unwrap_or_else(|| panic!("`{key}` in {record}"))
}

fn kind(record: &Value) -> &str {
    record["record"].as_str().unwrap_or_default()
}

/// The newest checkpoint tick an overlay's telemetry names, as
/// `checkpoint at tick 1,024`; `None` for `no checkpoint` or no telemetry.
fn overlay_checkpoint(text: &str) -> Option<u64> {
    let (_, rest) = text.split_once("checkpoint at tick ")?;
    let digits: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',')
        .filter(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

#[test]
fn an_offscreen_studio_keeps_a_run_the_command_line_inspects() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("after the epoch")
        .as_nanos();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("fl024-kept-binary-{}-{nanos}", std::process::id()));
    std::fs::create_dir(&dir).expect("a new directory");
    let config = dir.join("kept.toml");
    std::fs::write(&config, configuration()).expect("writes the configuration");
    let run = dir.join("run");
    let timeline = dir.join("session.ndjson");
    let output = Command::new(env!("CARGO_BIN_EXE_flife-studio"))
        .arg(&config)
        .args([
            "--offscreen",
            "2.5",
            "--scale",
            "1",
            "--width",
            "320",
            "--height",
            "200",
            "--controllers",
            "cpu",
            "--cpu-workers",
            "2",
            "--run",
        ])
        .arg(&run)
        .arg("--timeline")
        .arg(&timeline)
        .output()
        .expect("the binary runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "exit {:?}: {stderr}",
        output.status
    );
    let records: Vec<Value> = String::from_utf8(output.stdout)
        .expect("UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON record"))
        .collect();

    // The open record names the directory and its checkpoint admission,
    // which the memory admission charges.
    let open = &records[0];
    assert_eq!(kind(open), "open");
    assert_eq!(open["run_dir"], run.display().to_string());
    let admission = &open["checkpoints"];
    assert_eq!(admission["admitted"], "pause-and-stream", "{admission}");
    assert_eq!(number(admission, "peak_bytes"), CHECKPOINT_FIXED_BYTES);
    assert_eq!(
        number(&open["memory"], "checkpoint"),
        CHECKPOINT_FIXED_BYTES
    );

    // Checkpoints from tick 0 to the closing boundary, in order; the closing
    // statuses before `end`, which names the newest.
    let end = records.last().expect("records");
    assert_eq!(kind(end), "end");
    let closed_at = number(end, "tick");
    let kept: Vec<(u64, u64)> = records
        .iter()
        .filter(|record| kind(record) == "checkpoint")
        .map(|record| (number(record, "tick"), number(record, "sequence")))
        .collect();
    println!("checkpoints {kept:?}, closed at {closed_at}");
    assert!(kept.len() >= 3, "{kept:?}");
    assert_eq!(kept[0].0, 0);
    assert!(
        kept.windows(2)
            .all(|pair| pair[1].0 > pair[0].0 && pair[1].1 > pair[0].1)
    );
    assert_eq!(kept.last().expect("kept").0, closed_at);
    assert_eq!(number(end, "checkpoint_tick"), closed_at);
    let closing = &records[records.len() - 2];
    assert_eq!(
        (kind(closing), closing["status"].as_str()),
        ("status", Some("closed")),
        "{closing}"
    );
    assert_eq!(number(closing, "tick"), closed_at);
    // Closing paused the running session before its last checkpoint: the
    // run ended `paused` at the closing boundary, which that checkpoint
    // holds.
    let ended = records
        .iter()
        .rfind(|record| kind(record) == "status" && record["status"] == "ended")
        .expect("the run's end");
    assert_eq!(ended["why"], "paused", "{ended}");
    assert_eq!(number(ended, "tick"), closed_at);

    // The window's telemetry names a reported checkpoint, and a newer one
    // as they come.
    let text = std::fs::read_to_string(&timeline).expect("the timeline");
    let named: Vec<u64> = read_timeline(&text)
        .expect("reads")
        .into_iter()
        .filter_map(|record| match record {
            TimelineRecord::Overlay { text, .. } => overlay_checkpoint(&text),
            _ => None,
        })
        .collect();
    assert!(!named.is_empty(), "no overlay named a checkpoint");
    assert!(named.windows(2).all(|pair| pair[1] >= pair[0]), "{named:?}");
    assert!(
        named
            .iter()
            .all(|tick| kept.iter().any(|(at, _)| at == tick)),
        "{named:?} vs {kept:?}"
    );
    assert!(named.iter().any(|tick| *tick > 0), "{named:?}");

    // The command line restores the world the Studio closed on.
    let inspection = flife_cli::inspect(&run, InspectOptions::default()).expect("inspects");
    assert_eq!(inspection.inspected.checkpoint.tick, closed_at);
    assert_eq!(
        inspection.inspected.digest,
        end["digest"].as_str().expect("a digest")
    );

    // A configuration beside `--resume` is refused before anything runs.
    let both = Command::new(env!("CARGO_BIN_EXE_flife-studio"))
        .arg(&config)
        .arg("--resume")
        .arg(&run)
        .args(["--offscreen", "0.5"])
        .output()
        .expect("the binary runs");
    assert_eq!(both.status.code(), Some(2));
    assert!(both.stdout.is_empty());

    // `--resume` runs it on from the closing checkpoint, kept there.
    let output = Command::new(env!("CARGO_BIN_EXE_flife-studio"))
        .arg("--resume")
        .arg(&run)
        .args([
            "--offscreen",
            "1.5",
            "--scale",
            "1",
            "--width",
            "320",
            "--height",
            "200",
            "--controllers",
            "cpu",
            "--cpu-workers",
            "2",
        ])
        .output()
        .expect("the binary runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "exit {:?}: {stderr}",
        output.status
    );
    let records: Vec<Value> = String::from_utf8(output.stdout)
        .expect("UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON record"))
        .collect();
    let open = &records[0];
    assert_eq!(kind(open), "open");
    assert_eq!(open["run_dir"], run.display().to_string());
    let resumed = &open["resumed"];
    assert_eq!(
        number(&resumed["checkpoint"], "tick"),
        closed_at,
        "{resumed}"
    );
    assert_eq!(
        number(&resumed["checkpoint"], "sequence"),
        kept.last().expect("kept").1
    );
    assert_eq!(resumed["skipped"], Value::Array(Vec::new()));
    assert_eq!(number(resumed, "tick"), closed_at);
    assert_eq!(open["checkpoints"]["admitted"], "pause-and-stream");
    assert_eq!(
        number(&open["memory"], "checkpoint"),
        CHECKPOINT_FIXED_BYTES
    );
    let end = records.last().expect("records");
    assert_eq!(kind(end), "end");
    let until = number(end, "tick");
    assert!(until > closed_at, "it ran on: {until}");
    let later: Vec<(u64, u64)> = records
        .iter()
        .filter(|record| kind(record) == "checkpoint")
        .map(|record| (number(record, "tick"), number(record, "sequence")))
        .collect();
    println!("resumed at {closed_at}: checkpoints {later:?}, closed at {until}");
    let mut chain = kept.clone();
    chain.extend(&later);
    assert!(
        chain
            .windows(2)
            .all(|pair| pair[1].0 > pair[0].0 && pair[1].1 > pair[0].1),
        "{chain:?}"
    );
    assert_eq!(later.last().expect("a closing checkpoint").0, until);
    assert_eq!(number(end, "checkpoint_tick"), until);

    // The world it closed on is the one stepped directly from the founding.
    let configured = ValidatedConfig::from_toml(&configuration()).expect("valid");
    let mut world = World::found(
        &configured,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds");
    while world.tick() < Tick::new(until) {
        match world.step_with(&[]).expect("steps") {
            Stepped::Published(_) => {}
            Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
        }
    }
    assert_eq!(
        world.digest().to_hex(),
        end["digest"].as_str().expect("a digest")
    );
}
