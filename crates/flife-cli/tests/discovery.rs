//! FL-042 evidence (plan §14.5, §16.7): saved discoveries through the
//! `flife` binary.
//!
//! - A discovery bundle holds the run/checkpoint reference, the selected
//!   region or lineage measured in the committed world, a thumbnail drawn
//!   from that checkpoint, the presentation settings and the description,
//!   all in `discovery.json`, which the record repeats.
//! - Full-world semantics: the bundle's `world/` is a complete,
//!   reproducible world checkpoint. Resumed on its own, it reaches the
//!   state the source run reaches from that checkpoint, including a
//!   command the source had scheduled after it.
//! - Pinning: the source run's retention, continuing past the discovery,
//!   removes every unpinned checkpoint older than its newest kept ones and
//!   keeps the discovery's, which still restores.
//! - Refusals leave no bundle behind.

#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use flife_core::canonical::Sha256Digest;
use flife_store::Durability;
use fmn_codec::png::{PngLimits, decode};
use serde_json::Value;

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// Where the source run first stops.
const FIRST: u64 = 120;

/// The checkpoint discovered: kept by the source's retention of 4 at
/// [`FIRST`] (105, 112, 119 and 120 are kept), removed by it later.
const AT: u64 = 112;

/// Where the source run and the export are both resumed to.
const LATER: u64 = 161;

/// Energy brushes at ticks 3 and 150: the second is pending at [`AT`].
const INJECTIONS: [&str; 2] = ["3:5,6:2.0", "150:20,11:4.0"];

/// The thumbnail size the bundles here are drawn at.
const SIZE: [u32; 2] = [160, 90];

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

/// FL-037's 32 × 32 ecology of 48 founders, keeping its 4 newest
/// checkpoints and preserving pins, written to a file.
fn ecology() -> PathBuf {
    ecology_with("ecology.toml", &[])
}

/// [`ecology`] with `extra` overrides, written to `name`.
fn ecology_with(name: &str, extra: &[(&str, &str, &str)]) -> PathBuf {
    let mut overrides = vec![
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
        ("persistence", "retain_recent_checkpoints", "4"),
        ("persistence", "preserve_pinned_discoveries", "true"),
        (
            "execution",
            "command_durability",
            "\"write-ahead-before-apply\"",
        ),
    ];
    overrides.extend_from_slice(extra);
    let path = scratch().join(name);
    write_whole(&path, &appendix_a(&overrides));
    path
}

/// Write `text` to `path` through a uniquely named temporary file renamed
/// into place, so a `flife` another test starts never reads it truncated.
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
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("flife-discovery");
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

/// The one record of `kind`.
fn one<'a>(records: &'a [Value], kind: &str) -> &'a Value {
    let found: Vec<&Value> = records
        .iter()
        .filter(|record| record["record"] == kind)
        .collect();
    assert_eq!(found.len(), 1, "one {kind} record in {records:?}");
    found[0]
}

/// `flife run` of the ecology to `ticks` in `run_dir` with [`INJECTIONS`],
/// checkpointing every 7 ticks: its records.
fn run(run_dir: &Path, ticks: u64) -> Vec<Value> {
    run_with(&ecology(), run_dir, ticks)
}

/// [`run`] of the configuration in `config`.
fn run_with(config: &Path, run_dir: &Path, ticks: u64) -> Vec<Value> {
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

/// `flife resume` of `run_dir` to `until`, checkpointing every 7 ticks:
/// the committed digest it reached.
fn resume(run_dir: &Path, until: u64) -> Value {
    let (status, out, err) = flife(&[
        "resume",
        text(run_dir),
        "--until",
        &until.to_string(),
        "--every",
        "0",
        "--checkpoint-every",
        "7",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    let end = one(&out, "end");
    assert_eq!(end["stop"]["end"], "reached");
    assert_eq!(end["tick"], until);
    end["digest"].clone()
}

/// The source run at [`FIRST`], made once: its directory and records.
fn source() -> &'static (PathBuf, Vec<Value>) {
    static SOURCE: OnceLock<(PathBuf, Vec<Value>)> = OnceLock::new();
    SOURCE.get_or_init(|| {
        let run_dir = fresh("source");
        let out = run(&run_dir, FIRST);
        (run_dir, out)
    })
}

/// A fresh copy of the source run at [`FIRST`]: a test that continues or
/// pins a run works on its own copy.
fn copy_of_source(name: &str) -> PathBuf {
    let (source, _) = source();
    let copy = fresh(name);
    copy_tree(source, &copy);
    copy
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("creates the copy");
    for entry in fs::read_dir(from).expect("lists") {
        let entry = entry.expect("an entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("a type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copies");
        }
    }
}

/// The checkpoint record at `tick`.
fn checkpoint_at(records: &[Value], tick: u64) -> &Value {
    records
        .iter()
        .find(|record| record["record"] == "checkpoint" && record["tick"] == tick)
        .unwrap_or_else(|| panic!("a checkpoint at tick {tick}"))
}

/// `flife discover` of `run_dir` into `bundle` with `extra` flags: its
/// status, records and errors.
fn discover(
    run_dir: &Path,
    bundle: &Path,
    extra: &[&str],
) -> (Option<i32>, Vec<Value>, Vec<Value>) {
    let [width, height] = SIZE.map(|side| side.to_string());
    let mut args = vec![
        "discover",
        text(run_dir),
        text(bundle),
        "--width",
        &width,
        "--height",
        &height,
        "--render-threads",
        "2",
    ];
    args.extend(extra);
    flife(&args)
}

/// The thumbnail's pixels, checked to be [`SIZE`].
fn thumbnail(bundle: &Path) -> Vec<u8> {
    let png = fs::read(bundle.join("thumbnail.png")).expect("the thumbnail exists");
    let decoded = decode(&png, &PngLimits::default()).expect("the thumbnail decodes");
    assert_eq!([decoded.width, decoded.height], SIZE);
    decoded.rgba
}

/// The distinct RGBA values in `pixels`.
fn colours(pixels: &[u8]) -> usize {
    let mut seen: Vec<[u8; 4]> = pixels.as_chunks::<4>().0.to_vec();
    seen.sort_unstable();
    seen.dedup();
    seen.len()
}

/// The pale pixels (every channel at least 160: the measurement outline's
/// pale cyan; the field and the dots stay darker here) and their mean
/// distance from the thumbnail's centre.
fn pale(pixels: &[u8]) -> (usize, f64) {
    let [width, height] = SIZE.map(f64::from);
    let distances: Vec<f64> = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .enumerate()
        .filter(|(_, pixel)| pixel[..3].iter().all(|channel| *channel >= 160))
        .map(|(index, _)| {
            let index = index as f64;
            let x = index % width + 0.5 - width / 2.0;
            let y = (index / width).floor() + 0.5 - height / 2.0;
            x.hypot(y)
        })
        .collect();
    let mean = distances.iter().sum::<f64>() / distances.len().max(1) as f64;
    (distances.len(), mean)
}

fn sha(record: &Value) -> Sha256Digest {
    let hex = record.as_str().expect("a hex digest");
    let mut bytes = [0_u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * index..2 * index + 2], 16).expect("hex");
    }
    Sha256Digest::from_bytes(bytes)
}

/// `flife inspect` of `run_dir` at `tick`: its status and records.
fn inspect(run_dir: &Path, tick: u64) -> (Option<i32>, Vec<Value>) {
    let (status, out, _) = flife(&["inspect", text(run_dir), "--at", &tick.to_string()]);
    (status, out)
}

/// A region discovery: its bundle is complete and self-describing, its
/// world export resumes to the state the source reaches, and the source's
/// retention keeps its checkpoint while removing younger unpinned ones.
#[test]
fn a_region_discovery_is_a_pinned_reference_and_a_reproducible_world() {
    let (_, source_out) = source();
    let run_dir = copy_of_source("region-run");
    let bundle = fresh("region-bundle");
    let (status, out, err) = discover(
        &run_dir,
        &bundle,
        &[
            "--at",
            &AT.to_string(),
            "--region",
            "16,16,6",
            "--name",
            "a colony",
            "--description",
            "Dense ring of grazers around the north-east source.",
            "--checkpoint-durability",
            "process-crash",
        ],
    );
    assert_eq!(status, Some(0), "{err:?}");
    let record = one(&out, "discovery");

    // The record is discovery.json, tagged.
    let saved: Value = serde_json::from_slice(
        &fs::read(bundle.join("discovery.json")).expect("discovery.json exists"),
    )
    .expect("discovery.json is JSON");
    let mut untagged = record.clone();
    untagged
        .as_object_mut()
        .expect("an object")
        .remove("record");
    assert_eq!(saved, untagged);
    assert!(!bundle.join("discovery.json.partial").exists());

    // Source reference: the checkpoint the run published at AT.
    let published = checkpoint_at(source_out, AT);
    assert_eq!(record["schema"], "frankenlife.discovery.v1");
    assert_eq!(record["source"]["tick"], AT);
    assert_eq!(record["source"]["sequence"], published["sequence"]);
    assert_eq!(record["source"]["checkpoint_sha256"], published["sha256"]);
    assert_eq!(record["source"]["run_dir"], text(&run_dir));
    assert_eq!(record["source"]["pinned"], true);
    assert_eq!(record["source"]["pins_preserved"], true);
    // The identity, from its documented definition.
    let checkpoint = sha(&record["source"]["checkpoint_sha256"]);
    let mut identity = b"frankenlife.discovery.v1\0".to_vec();
    identity.extend_from_slice(checkpoint.as_bytes());
    identity.extend_from_slice(b"a colony");
    assert_eq!(record["id"], Sha256Digest::of(&identity).to_hex());
    assert_eq!(record["name"], "a colony");
    assert_eq!(
        record["description"],
        "Dense ring of grazers around the north-east source."
    );

    // Selection: the disk measured in the committed world.
    let selection = &record["selection"];
    assert_eq!(selection["kind"], "region");
    assert_eq!(selection["center"], serde_json::json!([16.0, 16.0]));
    assert_eq!(selection["radius"], 6.0);
    let cells = selection["cells"].as_u64().expect("cells");
    assert!(
        (100..=125).contains(&cells),
        "about 36π unit cells: {cells}"
    );
    assert!(selection["field_energy"].as_f64().expect("energy") > 0.0);
    // The organisms in the disk, counted from the inspected checkpoint on
    // the 32 × 32 torus.
    let (status, at) = inspect(&run_dir, AT);
    assert_eq!(status, Some(0));
    assert_eq!(
        one(&at, "inspect")["digest"],
        record["source"]["world_digest"]
    );
    let wrapped = |delta: f64| {
        let delta = delta.rem_euclid(32.0);
        delta.min(32.0 - delta)
    };
    let inside: Vec<f64> = at
        .iter()
        .filter(|record| record["record"] == "organism")
        .filter(|organism| {
            let [x, y] = [0, 1]
                .map(|axis| organism["position"][axis].as_f64().expect("a coordinate") - 16.0);
            wrapped(x).hypot(wrapped(y)) <= 6.0
        })
        .map(|organism| organism["energy"].as_f64().expect("an energy"))
        .collect();
    assert!(!inside.is_empty(), "the disk holds organisms at tick {AT}");
    assert_eq!(selection["organisms"], inside.len() as u64);
    let energy = selection["organism_energy"].as_f64().expect("energy");
    let expected: f64 = inside.iter().sum();
    assert!(
        (energy - expected).abs() <= 1e-9 * expected.abs().max(1.0),
        "{energy} {expected}"
    );

    // Presentation: framed on the region.
    let presentation = &record["presentation"];
    assert_eq!(
        presentation["camera"]["center"],
        serde_json::json!([16.0, 16.0])
    );
    assert!(presentation["camera"]["zoom"].as_f64().expect("zoom") > 1.0);
    assert_eq!(presentation["projection"]["divisor"], 32_f64.sqrt());

    // Thumbnail: a drawn picture, the hash the record names.
    let png = fs::read(bundle.join("thumbnail.png")).expect("reads");
    assert_eq!(
        record["thumbnail"]["sha256"],
        Sha256Digest::of(&png).to_hex()
    );
    let pixels = thumbnail(&bundle);
    assert!(colours(&pixels) > 16, "a picture, not a fill");
    // The region's outline, centred, its diameter REGION_FILL of the
    // shorter side: 33.75 pixels from the centre.
    let (outline, radius) = pale(&pixels);
    let expected = flife_cli::REGION_FILL * f64::from(SIZE[1]) / 2.0;
    assert!(outline > 150, "the outline is drawn: {outline} pale pixels");
    assert!(
        (radius - expected).abs() < 2.0,
        "the region is framed: outline {radius:.2} px from the centre, not {expected}"
    );

    // Full-world semantics: the export resumes to the source's own state.
    let world = &record["exports"]["full_world"];
    assert_eq!(world["path"], "world");
    assert!(record["exports"]["local_package"].is_null());
    assert!(world["chunks"][1].as_u64().expect("linked") > 0, "{world}");
    let exported = resume(&bundle.join("world"), LATER);
    let continued = resume(&run_dir, LATER);
    assert_eq!(exported, continued, "the export reproduces the source");

    // Pinning: the source's retention, continued to LATER, removed the
    // unpinned checkpoints 105, 119, 120, 126 and 133 and kept AT.
    let sequence = record["source"]["sequence"].as_u64().expect("a sequence");
    let store = common::open_store(run_dir.join("store"), Durability::ProcessCrash)
        .expect("the source store opens");
    assert_eq!(
        store.pins().expect("lists"),
        [(sequence, sha(&record["id"]))]
    );
    assert!(store.pin_path(sequence, &sha(&record["id"])).is_file());
    drop(store);
    let (status, kept) = inspect(&run_dir, AT);
    assert_eq!(status, Some(0), "the pinned checkpoint restores");
    assert_eq!(
        one(&kept, "inspect")["checkpoint"]["sha256"],
        published["sha256"]
    );
    for removed in [105, 119, 120, 126, 133] {
        assert_eq!(
            inspect(&run_dir, removed).0,
            Some(2),
            "retention removed the unpinned checkpoint at {removed}"
        );
    }
}

/// A lineage discovery without the world export, and a whole-world one:
/// the lineage's live organisms are those the checkpoint holds; nothing
/// else is written.
#[test]
fn lineage_and_whole_world_discoveries_record_what_the_checkpoint_holds() {
    let run_dir = copy_of_source("lineage-run");
    let (status, inspected) = inspect(&run_dir, AT);
    assert_eq!(status, Some(0));
    let organisms: Vec<&Value> = inspected
        .iter()
        .filter(|record| record["record"] == "organism")
        .collect();
    let lineage = organisms[0]["lineage"].as_u64().expect("a lineage");
    let members: Vec<u64> = organisms
        .iter()
        .filter(|organism| organism["lineage"] == lineage)
        .map(|organism| organism["generation"].as_u64().expect("a generation"))
        .collect();

    let bundle = fresh("lineage-bundle");
    let (status, out, err) = discover(
        &run_dir,
        &bundle,
        &[
            "--at",
            &AT.to_string(),
            "--lineage",
            &lineage.to_string(),
            "--no-world",
        ],
    );
    assert_eq!(status, Some(0), "{err:?}");
    let record = one(&out, "discovery");
    assert_eq!(record["name"], "lineage-bundle", "named after the bundle");
    let selection = &record["selection"];
    assert_eq!(selection["kind"], "lineage");
    assert_eq!(selection["lineage"], lineage);
    assert_eq!(selection["organisms"], members.len() as u64);
    assert_eq!(
        selection["generations"],
        serde_json::json!([members.iter().min(), members.iter().max()])
    );
    assert!(record["exports"]["full_world"].is_null());
    assert!(!bundle.join("world").exists());
    let mut written: Vec<String> = fs::read_dir(&bundle)
        .expect("lists")
        .map(|entry| {
            entry
                .expect("a file")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    written.sort_unstable();
    assert_eq!(written, ["discovery.json", "thumbnail.png"]);
    let lineage_pixels = thumbnail(&bundle);

    // The newest checkpoint, the whole world, fitted.
    let whole = fresh("whole-bundle");
    let (status, out, err) = discover(&run_dir, &whole, &["--no-world"]);
    assert_eq!(status, Some(0), "{err:?}");
    let record = one(&out, "discovery");
    assert_eq!(record["source"]["tick"], FIRST, "the newest that restores");
    assert_eq!(record["selection"]["kind"], "world");
    let (status, newest) = inspect(&run_dir, FIRST);
    assert_eq!(status, Some(0));
    let newest = one(&newest, "inspect");
    assert_eq!(record["selection"]["organisms"], newest["live"]);
    assert_eq!(record["source"]["world_digest"], newest["digest"]);
    assert_eq!(record["presentation"]["camera"]["zoom"], 1.0);
    let whole_pixels = thumbnail(&whole);
    assert!(colours(&whole_pixels) > 16);
    assert_eq!(pale(&whole_pixels).0, 0, "no region, no outline");
    assert_eq!(pale(&lineage_pixels).0, 0, "no region, no outline");
    assert_ne!(
        lineage_pixels, whole_pixels,
        "another checkpoint draws otherwise"
    );

    // Two discoveries, two pins.
    assert_eq!(
        fs::read_dir(run_dir.join("store/pins"))
            .expect("pins")
            .count(),
        2
    );
}

/// Refusals: before anything is written, and never into a directory that
/// holds something.
#[test]
fn refused_discoveries_write_nothing() {
    let run_dir = copy_of_source("refused-run");
    let refused = |bundle: &Path, extra: &[&str], kind: &str| {
        let (status, out, err) = discover(&run_dir, bundle, extra);
        assert_eq!(status, Some(2), "{out:?} {err:?}");
        assert!(out.is_empty());
        assert_eq!(err.len(), 1, "{err:?}");
        assert_eq!(err[0]["kind"], kind, "{err:?}");
        assert!(!bundle.join("discovery.json").exists());
    };
    let bundle = fresh("refused-bundle");
    refused(&bundle, &["--at", "999"], "state-conflict");
    refused(&bundle, &["--lineage", "4294967295"], "state-conflict");
    refused(&bundle, &["--region", "16,16,0"], "numerical-invalid");
    refused(&bundle, &["--region", "16,16,40"], "numerical-invalid");
    let (status, out, err) = flife(&["discover", text(&run_dir), text(&bundle), "--width", "0"]);
    assert_eq!(
        (status, out.len(), err[0]["kind"].as_str()),
        (Some(2), 0, Some("state-conflict")),
        "{err:?}"
    );
    assert!(!bundle.exists(), "no refusal created the bundle");

    fs::create_dir_all(&bundle).expect("creates");
    fs::write(bundle.join("notes.txt"), "mine").expect("writes");
    refused(&bundle, &[], "state-conflict");
    assert_eq!(
        fs::read_to_string(bundle.join("notes.txt")).expect("reads"),
        "mine"
    );
    assert_eq!(
        fs::read_dir(run_dir.join("store/pins"))
            .expect("pins")
            .count(),
        0,
        "no refusal pinned anything"
    );

    // A usage error is clap's, in plain text.
    let both = fresh("both-bundle");
    let output = common::output(Command::new(env!("CARGO_BIN_EXE_flife")).args([
        "discover",
        text(&run_dir),
        text(&both),
        "--region",
        "1,1,1",
        "--lineage",
        "1",
    ]));
    assert_eq!(
        output.status.code(),
        Some(2),
        "--region and --lineage conflict"
    );
    assert!(!both.exists());
}

/// A thumbnail the configuration's memory budget cannot hold is refused
/// before the bundle directory exists. The run's budget is its own fixed
/// terms (the start record's budget less the backend's grant) and 8 MiB:
/// a 4096 × 4096 frame alone is 64 MiB, while the usual thumbnail fits.
#[test]
fn a_thumbnail_over_the_memory_budget_is_refused_before_the_bundle_exists() {
    let probe = fresh("budget-probe");
    let out = run(&probe, 7);
    let memory = &one(&out, "start")["memory"];
    let fixed = memory["budget_bytes"].as_u64().expect("a budget")
        - memory["backend"].as_u64().expect("a backend grant");
    let tight = ecology_with(
        "tight.toml",
        &[(
            "execution",
            "maximum_total_bytes",
            &(fixed + (8 << 20)).to_string(),
        )],
    );
    let run_dir = fresh("budget-run");
    run_with(&tight, &run_dir, 7);

    let bundle = fresh("budget-bundle");
    let (status, out, err) = flife(&[
        "discover",
        text(&run_dir),
        text(&bundle),
        "--width",
        "4096",
        "--height",
        "4096",
        "--render-threads",
        "2",
        "--no-world",
    ]);
    assert_eq!(status, Some(2), "{out:?} {err:?}");
    assert!(out.is_empty());
    assert_eq!(err.len(), 1, "{err:?}");
    assert_eq!(err[0]["kind"], "budget-exhausted", "{err:?}");
    assert!(!bundle.exists(), "the refused discovery made no directory");
    assert_eq!(
        fs::read_dir(run_dir.join("store/pins"))
            .expect("pins")
            .count(),
        0,
        "the refused discovery pinned nothing"
    );

    // The same run's usual thumbnail fits: the refusal was the frame's.
    let (status, out, err) = discover(&run_dir, &bundle, &["--no-world"]);
    assert_eq!(status, Some(0), "{err:?}");
    assert_eq!(
        one(&out, "discovery")["memory"]["budget_bytes"],
        fixed + (8 << 20)
    );
}
