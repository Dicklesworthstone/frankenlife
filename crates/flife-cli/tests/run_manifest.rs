//! Appendix B.1 `manifest.json` (`frankenlife.run-manifest.v1`) emitted on
//! standalone run directories: `flife run --run-dir`, `flife resume`,
//! `flife step`, `flife branch`, and `flife transplant`.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use flife_cli::{CONFIG_FILE, MANIFEST_FILE, MANIFEST_SCHEMA, TRANSPLANT_FILE};
use flife_core::ValidatedConfig;
use serde_json::Value;

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

fn ecology_text() -> String {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    let mut lines: Vec<String> = appendix[start..start + length]
        .lines()
        .map(str::to_owned)
        .collect();
    for (section, key, value) in [
        ("world", "width", "32.0"),
        ("world", "height", "32.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", "24"),
        ("population", "maximum_count", "256"),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "2"),
        ("sources", "power_per_source", "60.0"),
        ("sources", "sigma_world_units", "3.0"),
        ("sources", "path_direction_interval_seconds", "2.0"),
        ("controller", "hidden_widths", "[8]"),
        (
            "execution",
            "command_durability",
            "\"write-ahead-before-apply\"",
        ),
    ] {
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

fn scratch() -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("flife-run-manifest");
    fs::create_dir_all(&path).expect("creates scratch dir");
    path
}

fn fresh(name: &str) -> PathBuf {
    let path = scratch().join(name);
    if path.exists() {
        fs::remove_dir_all(&path).expect("clears previous run dir");
    }
    path
}

fn text(path: &Path) -> &str {
    path.to_str().expect("UTF-8")
}

fn flife(args: &[&str]) -> (Option<i32>, Vec<Value>, Vec<Value>) {
    let output = Command::new(env!("CARGO_BIN_EXE_flife"))
        .args(args)
        .output()
        .expect("flife runs");
    let lines = |bytes: &[u8]| -> Vec<Value> {
        String::from_utf8_lossy(bytes)
            .lines()
            .map(|line| serde_json::from_str(line).expect("JSON line"))
            .collect()
    };
    (
        output.status.code(),
        lines(&output.stdout),
        lines(&output.stderr),
    )
}

fn read_manifest(run_dir: &Path) -> Value {
    let raw = fs::read_to_string(run_dir.join(MANIFEST_FILE)).expect("manifest.json exists");
    serde_json::from_str(&raw).expect("manifest.json parses as JSON")
}

fn assert_common_groups(manifest: &Value, expected_hash: &str) {
    assert_eq!(manifest["schema"], MANIFEST_SCHEMA);
    for group in [
        "identity",
        "model",
        "source",
        "numerics",
        "clocks",
        "hardware",
        "capacity",
        "provenance",
        "results",
        "artifacts",
    ] {
        assert!(
            manifest.get(group).is_some(),
            "missing Appendix B.1 group `{group}`: {manifest}"
        );
    }
    assert_eq!(manifest["model"]["config_hash"], expected_hash);
    assert!(
        manifest["model"]["descriptor"]
            .as_str()
            .is_some_and(|s| s.len() == 64)
    );
    assert!(manifest["hardware"]["host"]["logical_cores"].as_u64() >= Some(1));
    assert!(manifest["source"]["profile"].as_str().is_some());
    assert!(manifest["source"]["target"].as_str().is_some());
    let artifacts = manifest["artifacts"].as_array().expect("artifacts list");
    assert_eq!(artifacts[0]["path"], CONFIG_FILE);
    assert_eq!(artifacts[0]["sha256"], expected_hash);
}

#[test]
fn standalone_run_resume_branch_and_transplant_write_appendix_b1_manifests() {
    let config_toml = ecology_text();
    let validated = ValidatedConfig::from_toml(&config_toml).expect("valid config");
    let expected_hash = validated.config_hash().to_hex();
    let config_path = scratch().join("manifest-ecology.toml");
    fs::write(&config_path, &config_toml).expect("writes config");

    // 1. `flife run --run-dir` writes manifest.json with intervention count,
    //    achieved results, and verified checkpoint artifact hash.
    let run_dir = fresh("root-run");
    let (status, out, err) = flife(&[
        "run",
        text(&config_path),
        "--ticks",
        "6",
        "--every",
        "0",
        "--run-dir",
        text(&run_dir),
        "--checkpoint-every",
        "6",
        "--checkpoint-durability",
        "process-crash",
        "--inject",
        "2:4,4:5.0",
        "--inject",
        "8:6,6:5.0",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    let end = out
        .iter()
        .find(|r| r["record"] == "end")
        .expect("end record");
    let ckpt = out
        .iter()
        .rfind(|r| r["record"] == "checkpoint")
        .expect("final checkpoint");

    let m_run = read_manifest(&run_dir);
    assert_common_groups(&m_run, &expected_hash);
    assert_eq!(m_run["identity"]["kind"], "run");
    assert_eq!(m_run["identity"]["session"], 1);
    assert!(m_run["identity"]["parent"].is_null());
    assert_eq!(m_run["provenance"]["interventions"], 2);
    assert_eq!(m_run["results"]["status"], "reached");
    assert_eq!(m_run["results"]["tick"], 6);
    assert_eq!(m_run["results"]["ticks_published"], 6);
    assert_eq!(m_run["results"]["digest"], end["digest"]);
    assert!(
        m_run["results"]["ticks_per_second"]
            .as_f64()
            .is_some_and(|r| r > 0.0)
    );
    assert_eq!(m_run["artifacts"][1]["kind"], "checkpoint");
    assert_eq!(m_run["artifacts"][1]["sequence"], ckpt["sequence"]);
    assert_eq!(m_run["artifacts"][1]["tick"], 6);
    assert_eq!(m_run["artifacts"][1]["sha256"], ckpt["sha256"]);

    // 2. An orphaned `.partial` file from a prior crash does not block `resume`,
    //    and `resume` updates manifest.json atomically to tick 10.
    fs::write(
        run_dir.join(format!("{MANIFEST_FILE}.partial")),
        b"truncated",
    )
    .expect("simulates interrupted manifest write");
    let (status, out, err) = flife(&[
        "resume",
        text(&run_dir),
        "--until",
        "10",
        "--every",
        "0",
        "--checkpoint-every",
        "4",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    let end_resume = out
        .iter()
        .find(|r| r["record"] == "end")
        .expect("end record");
    let ckpt_resume = out
        .iter()
        .rfind(|r| r["record"] == "checkpoint")
        .expect("resume checkpoint");
    let m_resume = read_manifest(&run_dir);
    assert_common_groups(&m_resume, &expected_hash);
    assert_eq!(m_resume["identity"]["kind"], "resume");
    assert_eq!(m_resume["results"]["tick"], 10);
    assert_eq!(m_resume["results"]["ticks_published"], 4);
    assert_eq!(m_resume["results"]["digest"], end_resume["digest"]);
    assert_eq!(m_resume["artifacts"][1]["sha256"], ckpt_resume["sha256"]);
    assert!(!run_dir.join(format!("{MANIFEST_FILE}.partial")).exists());

    // 3. `flife branch` writes manifest.json recording parent provenance and
    //    founded status (without fabricating wall_seconds/ticks_per_second),
    //    and resuming the branch preserves its parent fork lineage.
    let branch_dir = fresh("child-branch");
    let (status, out, err) = flife(&[
        "branch",
        text(&run_dir),
        text(&branch_dir),
        "--at",
        "6",
        "--name",
        "probe-branch",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    let branch_rec = &out[0];
    let m_branch = read_manifest(&branch_dir);
    assert_common_groups(&m_branch, &expected_hash);
    assert_eq!(m_branch["identity"]["kind"], "branch");
    assert_eq!(m_branch["identity"]["run"], "probe-branch");
    assert_eq!(m_branch["identity"]["session"], branch_rec["session"]);
    assert_eq!(m_branch["identity"]["parent"]["kind"], "branch");
    assert_eq!(m_branch["identity"]["parent"]["parent_session"], 1);
    assert_eq!(m_branch["identity"]["parent"]["tick"], 6);
    assert_eq!(m_branch["results"]["status"], "founded");
    assert_eq!(m_branch["results"]["digest"], branch_rec["digest"]);
    assert!(m_branch["results"].get("ticks_per_second").is_none());

    let (status, _, err) = flife(&[
        "step",
        text(&branch_dir),
        "--ticks",
        "2",
        "--every",
        "0",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    let m_branch_step = read_manifest(&branch_dir);
    assert_eq!(m_branch_step["identity"]["kind"], "resume");
    assert_eq!(m_branch_step["identity"]["parent"]["kind"], "branch");
    assert_eq!(m_branch_step["identity"]["parent"]["parent_session"], 1);
    assert_eq!(m_branch_step["results"]["tick"], 8);
    assert_eq!(m_branch_step["results"]["ticks_published"], 2);

    // 4. `flife discover` + `flife transplant` writes manifest.json with
    //    package parent provenance and `transplant.json` artifact hash,
    //    and resuming the transplant preserves its package parent provenance.
    let bundle_dir = fresh("discovery-bundle");
    let (status, _, err) = flife(&[
        "discover",
        text(&run_dir),
        text(&bundle_dir),
        "--at",
        "10",
        "--region",
        "16,16,6",
        "--no-world",
        "--width",
        "64",
        "--height",
        "36",
        "--render-threads",
        "1",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");

    let transplant_dir = fresh("grafted-run");
    let (status, out, err) = flife(&[
        "transplant",
        text(&bundle_dir.join("package")),
        text(&transplant_dir),
        "--config",
        text(&config_path),
        "--at",
        "10,10",
        "--name",
        "probe-transplant",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    let transplant_rec = &out[0];
    let m_transplant = read_manifest(&transplant_dir);
    assert_common_groups(&m_transplant, &expected_hash);
    assert_eq!(m_transplant["identity"]["kind"], "transplant");
    assert_eq!(
        m_transplant["identity"]["session"],
        transplant_rec["session"]
    );
    assert_eq!(m_transplant["identity"]["parent"]["kind"], "package");
    assert_eq!(
        m_transplant["identity"]["parent"]["package"]["sha256"],
        transplant_rec["package"]["sha256"]
    );
    assert_eq!(m_transplant["results"]["status"], "founded");
    assert_eq!(m_transplant["artifacts"][2]["path"], TRANSPLANT_FILE);

    let (status, _, err) = flife(&[
        "step",
        text(&transplant_dir),
        "--ticks",
        "2",
        "--every",
        "0",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    let m_transplant_step = read_manifest(&transplant_dir);
    assert_eq!(m_transplant_step["identity"]["kind"], "resume");
    assert_eq!(m_transplant_step["identity"]["parent"]["kind"], "package");
    assert_eq!(
        m_transplant_step["identity"]["parent"]["package"]["sha256"],
        transplant_rec["package"]["sha256"]
    );
    assert_eq!(m_transplant_step["results"]["tick"], 2);
}
