//! FL-042 evidence (plan §14.5): a discovery's local package grafted into
//! a founded host through the `flife` binary, as a run that resumes.
//!
//! - The record and `transplant.json` agree, and agree with this file's own
//!   oracle: the whole-cell offset and the centre it puts the disk at, the
//!   session, the organisms and lineages the package declares, the
//!   endowment, and the holdings identity (the host's, less what was
//!   displaced, plus the endowment).
//! - The founding checkpoint holds the grafted world: `flife inspect` at
//!   tick 0 reads the host's founders outside the disk and the package's
//!   organisms as new founders, placed by the oracle, with their energy.
//! - It resumes from that checkpoint, not from its configuration's founding
//!   world, and reproducibly: straight through and in two legs alike.
//! - With no checkpoint that restores, it is refused: it has no founding
//!   world to fall back to.
//! - Refusals write nothing.
//! - Its memory is admitted as a run's is, with the package as a term of
//!   its own (FL-043): a budget of exactly the terms' sum is admitted, and
//!   one byte fewer refused.

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

use flife_cli::{CHECKPOINT_FIXED_BYTES, RESERVE_BYTES, found, read_package, transplant_session};
use flife_core::ValidatedConfig;
use flife_core::canonical::Sha256Digest;
use flife_engine::NumericalProfile;
use flife_store::Durability;
use serde_json::Value;

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// The source run's tick, the discovery's checkpoint.
const SOURCE_TICKS: u64 = 60;

/// The discovered region.
const REGION: &str = "16,16,6";

/// Where the transplant puts the disk's centre: snapped to whole cells,
/// 8 cells west and 9 north of the source's (16, 16).
const AT: [f64; 2] = [8.3, 24.6];

/// The world's side, in world units: one field cell a unit.
const SIDE: f64 = 32.0;

/// Appendix A with `extra` overrides; an empty section names a top-level
/// key.
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
        let first = if section.is_empty() {
            0
        } else {
            let header = format!("[{section}]");
            lines
                .iter()
                .position(|line| line.trim() == header)
                .expect("a section")
                + 1
        };
        let line = lines[first..]
            .iter()
            .take_while(|line| !line.starts_with('['))
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .map(|offset| first + offset)
            .expect("a key");
        lines[line] = format!("{key} = {value}");
    }
    lines.join("\n")
}

/// FL-037's 32 × 32 ecology of 48 founders with `extra` overrides, written
/// to `name`: its path and its validated configuration.
fn ecology(name: &str, extra: &[(&str, &str, &str)]) -> (PathBuf, ValidatedConfig) {
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
        (
            "execution",
            "command_durability",
            "\"write-ahead-before-apply\"",
        ),
    ];
    overrides.extend_from_slice(extra);
    let text = appendix_a(&overrides);
    let config = ValidatedConfig::from_toml(&text).expect("a valid configuration");
    let path = scratch().join(name);
    write_whole(&path, &text);
    (path, config)
}

/// The host: the ecology founded from another seed.
fn host() -> (PathBuf, ValidatedConfig) {
    ecology("host.toml", &[("", "seed", "7")])
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
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("flife-transplant");
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

/// The package a discovery of [`REGION`] at [`SOURCE_TICKS`] holds, made
/// once.
fn package() -> &'static PathBuf {
    static PACKAGE: OnceLock<PathBuf> = OnceLock::new();
    PACKAGE.get_or_init(|| {
        let (config, _) = ecology("source.toml", &[]);
        let run_dir = fresh("source");
        let ticks = SOURCE_TICKS.to_string();
        let (status, _, err) = flife(&[
            "run",
            text(&config),
            "--ticks",
            &ticks,
            "--every",
            "0",
            "--run-dir",
            text(&run_dir),
            "--checkpoint-every",
            "10",
            "--checkpoint-durability",
            "process-crash",
        ]);
        assert_eq!(status, Some(0), "{err:?}");
        let bundle = fresh("bundle");
        let (status, _, err) = flife(&[
            "discover",
            text(&run_dir),
            text(&bundle),
            "--at",
            &ticks,
            "--region",
            REGION,
            "--no-world",
            "--width",
            "64",
            "--height",
            "36",
            "--render-threads",
            "2",
            "--checkpoint-durability",
            "process-crash",
        ]);
        assert_eq!(status, Some(0), "{err:?}");
        bundle.join("package")
    })
}

/// `flife transplant` of [`package`] into a host of `config` at `at`, named
/// `name`, into `run_dir`.
fn transplant(
    config: &Path,
    run_dir: &Path,
    at: &str,
    name: &str,
) -> (Option<i32>, Vec<Value>, Vec<Value>) {
    flife(&[
        "transplant",
        text(package()),
        text(run_dir),
        "--config",
        text(config),
        "--at",
        at,
        "--name",
        name,
        "--checkpoint-durability",
        "process-crash",
    ])
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
        "--checkpoint-every",
        "5",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    let end = one(&out, "end");
    assert_eq!(end["stop"]["end"], "reached");
    assert_eq!(end["tick"], until);
    out
}

fn number(value: &Value) -> f64 {
    value.as_f64().expect("a number")
}

/// The minimum-image component `to − from` on a period of [`SIDE`].
fn wrapped(from: f64, to: f64) -> f64 {
    (to - from + SIDE / 2.0).rem_euclid(SIDE) - SIDE / 2.0
}

/// The transplant's record, `transplant.json` and the founding checkpoint
/// agree with the oracle; the run resumes from that checkpoint,
/// reproducibly.
#[test]
fn a_transplant_is_a_run_that_resumes_from_the_grafted_world() {
    let (host_path, host_config) = host();
    let run_dir = fresh("transplanted");
    let (status, out, err) = transplant(&host_path, &run_dir, "8.3,24.6", "t1");
    assert_eq!(status, Some(0), "{err:?}");
    let record = one(&out, "transplant");
    let kept: Value = serde_json::from_str(
        &fs::read_to_string(run_dir.join("transplant.json")).expect("the record is kept"),
    )
    .expect("parses");
    assert_eq!(&kept, record, "transplant.json is the record");

    // The package as it declares itself, and as it restores.
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(package().join("package.json")).expect("a manifest"),
    )
    .expect("parses");
    let profile = NumericalProfile::reference_f32().expect("profile");
    let (declared, crop) = read_package(package(), profile).expect("the package verifies");
    let center = declared.crop.disk().center;
    assert_eq!(center, [16.0, 16.0]);
    assert!(declared.organisms > 0, "the package holds organisms");

    // The offset, the centre and the session.
    let offset = [0, 1].map(|axis| (AT[axis] - center[axis]).round() as i64);
    assert_eq!(offset, [-8, 9]);
    let graft = &record["graft"];
    assert_eq!(graft["offset"], serde_json::json!(offset));
    assert_eq!(graft["at"], serde_json::json!(AT));
    let placed = [0, 1].map(|axis| (center[axis] + offset[axis] as f64).rem_euclid(SIDE));
    assert_eq!(graft["center"], serde_json::json!(placed));
    let session = transplant_session(
        &declared.world.sha256,
        host_config.config_hash(),
        offset,
        "t1",
    );
    assert_eq!(record["session"], session.get());
    assert_ne!(session.get(), 1, "never a founded run's session");
    assert_eq!(record["name"], "t1");
    assert_eq!(record["package"]["sha256"], manifest["world"]["sha256"]);
    assert_eq!(record["package"]["source"], manifest["source"]);
    assert_eq!(
        record["host"]["config_hash"],
        host_config.config_hash().to_hex()
    );
    assert_eq!(record["checkpoint"]["tick"], 0);

    // What it grafted, as the package declares, and the holdings identity.
    assert_eq!(graft["organisms"], manifest["organisms"]);
    assert_eq!(graft["lineages"], manifest["lineages"]);
    for key in ["field", "organisms", "total"] {
        let (granted, declared) = (
            number(&graft["endowment"][key]),
            number(&manifest["endowment"][key]),
        );
        assert!(
            (granted - declared).abs() <= 1e-12 * declared.abs(),
            "{key}"
        );
    }
    let expected = number(&graft["before"]["total"])
        - number(&graft["displaced_field"])
        - number(&graft["displaced_organism_energy"])
        + number(&graft["endowment"]["total"]);
    let after = number(&graft["after"]["total"]);
    assert!((after - expected).abs() <= 1e-9 * expected, "{graft}");

    // The same graft made here reaches the record's digest.
    let mut grafted = found(&host_config).expect("founds");
    assert_eq!(record["host"]["digest"], grafted.digest().to_hex());
    let founders = grafted.population().len() as u64;
    assert_eq!(record["host"]["founders"], founders);
    // The newest identity the host issued: the newcomers' come after it.
    let latest = grafted.population().latest().expect("founders").get();
    grafted
        .graft(&crop, center, declared.crop.disk().radius, offset)
        .expect("grafts");
    assert_eq!(record["digest"], grafted.digest().to_hex());

    // The run's store holds the founding checkpoint, and only it.
    let store = common::open_store(run_dir.join("store"), Durability::ProcessCrash)
        .expect("the store opens");
    let recovered = store
        .latest_valid(|_, bytes| Ok(Sha256Digest::of(bytes).to_hex()))
        .expect("lists");
    assert!(recovered.skipped.is_empty());
    let (sequence, sha256) = recovered.latest.expect("a checkpoint");
    assert_eq!(record["checkpoint"]["sequence"], sequence);
    assert_eq!(record["sha256"], sha256);
    drop(store);

    // The founding checkpoint: the host's founders outside the disk, then
    // the package's organisms as founders, placed and endowed.
    let (status, inspected, err) = flife(&["inspect", text(&run_dir), "--at", "0"]);
    assert_eq!(status, Some(0), "{err:?}");
    assert_eq!(one(&inspected, "inspect")["digest"], record["digest"]);
    assert_eq!(
        one(&inspected, "inspect")["checkpoint"]["sha256"],
        record["sha256"]
    );
    let organisms: Vec<&Value> = inspected
        .iter()
        .filter(|record| record["record"] == "organism")
        .collect();
    let displaced = graft["displaced_organisms"].as_u64().expect("a count");
    assert_eq!(
        organisms.len() as u64,
        founders - displaced + declared.organisms
    );
    let newcomers = &organisms[(founders - displaced) as usize..];
    let packed: Vec<_> = crop.population().iter().collect();
    for (index, (new, old)) in newcomers.iter().zip(&packed).enumerate() {
        assert_eq!(new["id"], latest + 1 + index as u64);
        assert_eq!(new["generation"], 0);
        assert!(new["parent"].is_null());
        assert_eq!(new["birth_tick"], 0);
        assert_eq!(number(&new["energy"]).to_bits(), old.energy.to_bits());
        for axis in 0..2 {
            let want = (placed[axis] + wrapped(center[axis], old.position[axis])).rem_euclid(SIDE);
            let got = number(&new["position"][axis]);
            assert!(
                (got - want).abs() < 1e-9,
                "{} at {got}, not {want}",
                new["id"]
            );
        }
    }
    for organism in &organisms[..(founders - displaced) as usize] {
        let id = organism["id"].as_u64().expect("an id");
        assert!(id <= latest, "a host founder");
        let dx = wrapped(placed[0], number(&organism["position"][0]));
        let dy = wrapped(placed[1], number(&organism["position"][1]));
        let radius = declared.crop.disk().radius;
        assert!(
            dx * dx + dy * dy > radius * radius,
            "{id} is outside the disk"
        );
    }

    // It resumes from the grafted world, straight through and in two legs
    // alike, and not as its configuration's founding world would.
    let straight = scratch().join("straight");
    let legs = scratch().join("legs");
    for copy in [&straight, &legs] {
        if copy.exists() {
            fs::remove_dir_all(copy).expect("clears the last copy");
        }
        copy_tree(&run_dir, copy);
    }
    let through = resume(&straight, 30);
    let from = one(&through, "resume");
    assert_eq!(from["checkpoint"]["tick"], 0);
    assert_eq!(from["digest"], record["digest"]);
    resume(&legs, 13);
    let second = resume(&legs, 30);
    assert_eq!(one(&second, "resume")["checkpoint"]["tick"], 13);
    assert_eq!(
        one(&second, "end")["digest"],
        one(&through, "end")["digest"]
    );
    let plain_dir = fresh("plain");
    let (status, plain, err) = flife(&[
        "run",
        text(&host_path),
        "--ticks",
        "30",
        "--every",
        "0",
        "--run-dir",
        text(&plain_dir),
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    assert_ne!(
        one(&plain, "end")["digest"],
        one(&through, "end")["digest"],
        "the host alone runs otherwise"
    );
}

/// A transplant none of whose checkpoints restores is refused: its world
/// is not the one its configuration founds.
#[test]
fn a_transplant_without_a_checkpoint_that_restores_is_refused() {
    let (host_path, _) = host();
    let run_dir = fresh("damaged");
    let (status, _, err) = transplant(&host_path, &run_dir, "20,20", "damaged");
    assert_eq!(status, Some(0), "{err:?}");
    fs::write(
        run_dir.join("store/checkpoints/0000000000000001.manifest"),
        b"x",
    )
    .expect("damages the manifest");
    let (status, out, err) = flife(&["resume", text(&run_dir), "--until", "10", "--every", "0"]);
    assert_eq!(status, Some(2), "{out:?}");
    assert_eq!(err[0]["kind"], "state-conflict", "{err:?}");
    let detail = err[0]["detail"].as_str().expect("a detail");
    assert!(
        detail.contains("no founding world to fall back to"),
        "{detail}"
    );
}

/// The error record of a refused `flife transplant` of `package` into a
/// host of `config` at `at`, named `name`, into `run_dir`: exit status 2,
/// nothing on standard output, one error of `kind` whose detail holds
/// `why`.
fn refused_with(
    package: &Path,
    config: &Path,
    run_dir: &Path,
    (at, name): (&str, &str),
    (kind, why): (&str, &str),
) {
    let (status, out, err) = flife(&[
        "transplant",
        text(package),
        text(run_dir),
        "--config",
        text(config),
        "--at",
        at,
        "--name",
        name,
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(2), "{why}: {out:?}");
    assert!(out.is_empty(), "{why}: {out:?}");
    assert_eq!(err.len(), 1, "{why}: {err:?}");
    assert_eq!(err[0]["kind"], kind, "{why}: {err:?}");
    let detail = err[0]["detail"].as_str().expect("a detail");
    assert!(detail.contains(why), "{why}: {detail}");
}

/// Refusals: each before anything is written.
#[test]
fn refused_transplants_write_nothing() {
    let (host_path, _) = host();
    let run_dir = fresh("refused");
    let refused = |config: &Path, at: &str, name: &str, kind: &str, why: &str| {
        refused_with(package(), config, &run_dir, (at, name), (kind, why));
        assert!(!run_dir.exists(), "{why}: nothing is written");
    };
    refused(
        &host_path,
        "32,5",
        "outside",
        "state-conflict",
        "outside the host",
    );
    refused(
        &host_path,
        "5,5",
        "",
        "state-conflict",
        "a transplant name is 1 to",
    );
    let (finer, _) = ecology(
        "finer.toml",
        &[
            ("world", "grid_width", "64"),
            ("world", "grid_height", "64"),
        ],
    );
    refused(
        &finer,
        "5,5",
        "finer",
        "state-conflict",
        "the package's cells are",
    );

    // A host that cannot take the organisms that arrive.
    let (full_path, full) = ecology(
        "full.toml",
        &[("", "seed", "7"), ("population", "maximum_count", "48")],
    );
    let profile = NumericalProfile::reference_f32().expect("profile");
    let (declared, _) = read_package(package(), profile).expect("verifies");
    let target = [5.0, 5.0];
    let leaving = found(&full)
        .expect("founds")
        .measure_region(target, declared.crop.disk().radius)
        .expect("measures")
        .organisms;
    assert!(leaving < declared.organisms, "more arrive than leave");
    refused(
        &full_path,
        "5,5",
        "full",
        "budget-exhausted",
        "live population budget exhausted",
    );

    // Founders' motion responses lie on the sphere of the configured norm,
    // 1 in the source: a host allowing a hundredth of it founds its own
    // founders and refuses the package's.
    let (timid, _) = ecology(
        "timid.toml",
        &[
            ("", "seed", "7"),
            ("chemistry", "motion_response_norm_limit", "0.01"),
        ],
    );
    refused(
        &timid,
        "5,5",
        "timid",
        "numerical-invalid",
        "motion response",
    );

    // A package that does not verify, and a directory that is not one.
    let damaged = scratch().join("damaged-package");
    if damaged.exists() {
        fs::remove_dir_all(&damaged).expect("clears the last copy");
    }
    copy_tree(package(), &damaged);
    let container = damaged.join("world.ckpt");
    let mut bytes = fs::read(&container).expect("reads");
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0x01;
    fs::write(&container, bytes).expect("writes");
    refused_with(
        &damaged,
        &host_path,
        &run_dir,
        ("5,5", "damaged"),
        ("state-conflict", "SHA-256"),
    );
    assert!(!run_dir.exists());
    let bundle = package().parent().expect("the bundle");
    refused_with(
        bundle,
        &host_path,
        &run_dir,
        ("5,5", "bundle"),
        ("state-conflict", "is not a package directory"),
    );
    assert!(!run_dir.exists());

    // A directory that already holds a run: refused before the package is
    // read, its record left as it was.
    let (status, _, err) = transplant(&host_path, &run_dir, "5,5", "first");
    assert_eq!(status, Some(0), "{err:?}");
    let first = fs::read(run_dir.join("transplant.json")).expect("the first's record");
    assert!(!run_dir.join("transplant.json.partial").exists());
    refused_with(
        &damaged,
        &host_path,
        &run_dir,
        ("6,6", "second"),
        ("state-conflict", "already holds a run"),
    );
    assert_eq!(
        fs::read(run_dir.join("transplant.json")).expect("the first's record"),
        first
    );
}

/// Memory is admitted as a run's is (FL-043): the host's terms, its
/// streamed founding checkpoint's and the package's restored world with its
/// container, against the host's `maximum_total_bytes`. A budget of exactly
/// their sum is admitted; one byte fewer is refused before anything is
/// written, though it holds the host alone.
#[test]
fn a_transplant_is_admitted_with_its_package_or_refused() {
    // The host's terms, added up here: the allocated genome quota, the
    // founded world's state, field, scratch and staging, the streamed
    // checkpoint's fixed heap and the reserve. The package's: its restored
    // world, its container and the graft's parts. No term depends on the
    // budget.
    let (_, probe) = ecology("budget-probe.toml", &[("", "seed", "7")]);
    let world = found(&probe).expect("founds");
    let footprint = world.footprint();
    let profile = NumericalProfile::reference_f32().expect("profile");
    let (declared, crop) = read_package(package(), profile).expect("verifies");
    let graft = world.graft_bytes(&crop);
    assert!(
        graft > world.quota().allocated_bytes() + crop.quota().allocated_bytes(),
        "the graft rebuilds both worlds' genomes in a second population"
    );
    let package_term = crop.held_bytes() + declared.world.bytes + graft;
    drop((world, crop));
    let host_terms = probe.config().execution.maximum_allocated_genome_bytes
        + footprint.state
        + footprint.field
        + footprint.scratch
        + footprint.staging
        + CHECKPOINT_FIXED_BYTES
        + RESERVE_BYTES;
    let needed = host_terms + package_term;

    let budgeted = |name: &str, budget: u64| {
        ecology(
            name,
            &[
                ("", "seed", "7"),
                ("execution", "maximum_total_bytes", &budget.to_string()),
            ],
        )
        .0
    };
    let run_dir = fresh("budgeted");
    refused_with(
        package(),
        &budgeted("short.toml", needed - 1),
        &run_dir,
        ("5,5", "short"),
        ("budget-exhausted", "peak memory (maximum_total_bytes)"),
    );
    assert!(!run_dir.exists(), "nothing is written");

    let (status, out, err) = transplant(&budgeted("exact.toml", needed), &run_dir, "5,5", "exact");
    assert_eq!(status, Some(0), "{err:?}");
    let memory = &one(&out, "transplant")["memory"];
    let term = |name: &str| memory[name].as_u64().expect("a count");
    assert_eq!(term("package"), package_term, "{memory}");
    assert_eq!(term("checkpoint"), CHECKPOINT_FIXED_BYTES, "{memory}");
    assert_eq!(term("render"), 0, "{memory}");
    assert_eq!(term("backend"), 0, "{memory}");
    assert_eq!(term("budget_bytes"), needed, "{memory}");
    assert_eq!(term("peak_bytes"), needed, "{memory}");
    assert!(run_dir.join("transplant.json").exists());
}

/// The session derives from every input: the package, the host, the
/// offset and the name.
#[test]
fn a_transplant_session_names_its_inputs() {
    let host = Sha256Digest::of(b"host");
    let base = transplant_session("aa", host, [1, 2], "n");
    for other in [
        transplant_session("ab", host, [1, 2], "n"),
        transplant_session("aa", Sha256Digest::of(b"other"), [1, 2], "n"),
        transplant_session("aa", host, [2, 1], "n"),
        transplant_session("aa", host, [1, 2], "m"),
    ] {
        assert_ne!(other, base);
    }
    assert_eq!(transplant_session("aa", host, [1, 2], "n"), base);
}
