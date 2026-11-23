//! V2-01 E2E CLI tests: `--list-features`, founding-only `--disable`/`--enable`/
//! `--v1-compat` toggle boundary, exact left-to-right flag order, checkpoint
//! provenance across `run` -> `inspect` -> `resume`, `INV-14` refusal on
//! `resume` / `step` / `render --run-dir`, and model-family-aware `experiment`
//! ablations on `field-ecology-v2`.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use flife_cli::{ERROR_SCHEMA, Experiment, FEATURES_SCHEMA, INSPECT_SCHEMA};
use flife_core::V2Feature;
use serde_json::Value;

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

fn ecology_toml(model: &str, extra_tail: &str) -> String {
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
        ("population", "initial_count", "32"),
        ("population", "maximum_count", "512"),
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
        ("execution", "command_durability", "\"volatile-in-memory\""),
    ] {
        let header = format!("[{section}]");
        let first = lines
            .iter()
            .position(|line| line.trim() == header)
            .expect("section");
        let line = lines[first + 1..]
            .iter()
            .take_while(|line| !line.starts_with('['))
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .map(|offset| first + 1 + offset)
            .expect("key");
        lines[line] = format!("{key} = {value}");
    }
    let model_line = lines
        .iter()
        .position(|line| line.starts_with("model = "))
        .expect("model line");
    lines[model_line] = format!("model = \"{model}\"");
    let mut out = lines.join("\n");
    if !extra_tail.is_empty() {
        out.push('\n');
        out.push_str(extra_tail);
    }
    out
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("flife-v2-toggles")
        .join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("clears previous test dir");
    }
    fs::create_dir_all(&dir).expect("creates test dir");
    dir
}

fn text(path: &Path) -> &str {
    path.to_str().expect("UTF-8 path")
}

fn flife(args: &[&str]) -> (Option<i32>, Vec<Value>, Vec<Value>) {
    let output = Command::new(env!("CARGO_BIN_EXE_flife"))
        .args(args)
        .output()
        .expect("flife executes");
    let parse = |bytes: &[u8]| -> Vec<Value> {
        String::from_utf8_lossy(bytes)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("JSON line"))
            .collect()
    };
    (
        output.status.code(),
        parse(&output.stdout),
        parse(&output.stderr),
    )
}

#[test]
fn e2e_cli_toggle_boundary_and_checkpoint_provenance() {
    let dir = scratch("boundary");
    let v1_path = dir.join("v1.toml");
    let v2_path = dir.join("v2.toml");
    fs::write(&v1_path, ecology_toml("field-ecology-v1", "")).expect("writes v1");
    fs::write(&v2_path, ecology_toml("field-ecology-v2", "")).expect("writes v2");

    // 1. `--list-features` prints structured table of all 21 feature slugs.
    let (status, out, err) = flife(&["--list-features"]);
    assert_eq!(status, Some(0), "{err:?}");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["schema"], FEATURES_SCHEMA);
    let listed = out[0]["features"].as_array().expect("features array");
    assert_eq!(listed.len(), V2Feature::ALL.len());
    for feature in V2Feature::ALL {
        assert!(
            listed.iter().any(|item| item["slug"] == feature.slug()),
            "missing feature slug {}",
            feature.slug()
        );
    }

    // 2. Toggles on v1 configs are explicitly refused.
    let (status, _, err) = flife(&["validate", text(&v1_path), "--v1-compat"]);
    assert_eq!(status, Some(2));
    assert_eq!(err[0]["schema"], ERROR_SCHEMA);

    // 3. `validate` applies `--disable` / `--enable` / `--v1-compat` in exact
    //    command-line order and alters `config_hash`.
    let (status, base_val, err) = flife(&["validate", text(&v2_path)]);
    assert_eq!(status, Some(0), "{err:?}");
    let (status, compat_val, err) = flife(&["validate", text(&v2_path), "--v1-compat"]);
    assert_eq!(status, Some(0), "{err:?}");
    let (status, single_val, err) = flife(&[
        "validate",
        text(&v2_path),
        "--disable",
        "all-v2",
        "--enable",
        "oases",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    assert_ne!(base_val[0]["config_hash"], compat_val[0]["config_hash"]);
    assert_ne!(compat_val[0]["config_hash"], single_val[0]["config_hash"]);
    assert_eq!(compat_val[0]["v2_features"], serde_json::json!([]));
    assert_eq!(single_val[0]["v2_features"], serde_json::json!(["oases"]));
    assert_eq!(single_val[0]["inv08_pure_field"], true);

    // 4. Capabilities refuses toggles without `--config`.
    let (status, _, err) = flife(&["capabilities", "--v1-compat"]);
    assert_eq!(status, Some(2));
    assert_eq!(err[0]["schema"], ERROR_SCHEMA);

    // 5. Founding `run` with `--disable all-v2 --enable oases` bakes the exact
    //    toggles into the run directory's config and v3 checkpoint; `inspect`
    //    surfaces `v2_features` & `inv08_pure_field`; `resume` WITHOUT flags
    //    continues bit-identically to an uninterrupted run.
    let run_dir = dir.join("run_resumed");
    let (status, _, err) = flife(&[
        "run",
        text(&v2_path),
        "--disable",
        "all-v2",
        "--enable",
        "oases",
        "--ticks",
        "12",
        "--every",
        "0",
        "--controllers",
        "cpu",
        "--run-dir",
        text(&run_dir),
        "--checkpoint-every",
        "12",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");

    let (status, inspected, err) = flife(&["inspect", text(&run_dir)]);
    assert_eq!(status, Some(0), "{err:?}");
    assert_eq!(inspected[0]["schema"], INSPECT_SCHEMA);
    assert_eq!(inspected[0]["model"], "field-ecology-v2");
    assert_eq!(inspected[0]["v2_features"], serde_json::json!(["oases"]));
    assert_eq!(inspected[0]["inv08_pure_field"], true);

    // Uninterrupted 24-tick oracle with the same toggles.
    let (status, uninterrupted, err) = flife(&[
        "run",
        text(&v2_path),
        "--disable",
        "all-v2",
        "--enable",
        "oases",
        "--ticks",
        "24",
        "--every",
        "0",
        "--controllers",
        "cpu",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    let want_end = uninterrupted
        .iter()
        .find(|record| record["record"] == "end")
        .expect("uninterrupted end");

    // Resume without re-passing flags -> must match bit-for-bit!
    let (status, resumed, err) = flife(&[
        "resume",
        text(&run_dir),
        "--until",
        "24",
        "--every",
        "0",
        "--controllers",
        "cpu",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    let got_end = resumed
        .iter()
        .find(|record| record["record"] == "end")
        .expect("resumed end");
    assert_eq!(got_end["digest"], want_end["digest"]);

    // 6. Resuming commands (`resume`, `step`, `render --run-dir`) MUST refuse
    //    `--disable` / `--enable` / `--v1-compat` citing INV-14.
    let frames_dir = dir.join("frames");
    for (label, args) in [
        (
            "resume --disable",
            vec![
                "resume",
                text(&run_dir),
                "--until",
                "25",
                "--disable",
                "oases",
            ],
        ),
        (
            "step --v1-compat",
            vec!["step", text(&run_dir), "--ticks", "1", "--v1-compat"],
        ),
        (
            "render --run-dir --enable",
            vec![
                "render",
                "--run-dir",
                text(&run_dir),
                "--out",
                text(&frames_dir),
                "--ticks",
                "1",
                "--enable",
                "senescence",
            ],
        ),
    ] {
        let (status, _, err) = flife(&args);
        assert_eq!(status, Some(2), "{label} must exit 2");
        assert_eq!(err[0]["schema"], ERROR_SCHEMA, "{label}");
        assert!(
            err[0]["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("INV-14")),
            "{label} error must cite INV-14: {:?}",
            err[0]
        );
    }

    // 7. `Experiment` routes `chemistry-only`, `fresh-offspring`, and
    //    `fixed-controller` to `field-ecology-v2-*` on a v2 base config.
    let exp_dir = dir.join("exp");
    fs::create_dir_all(&exp_dir).expect("exp dir");
    fs::write(
        exp_dir.join("base.toml"),
        ecology_toml("field-ecology-v2", ""),
    )
    .expect("base");
    let spec_path = exp_dir.join("experiment.toml");
    fs::write(
        &spec_path,
        r#"
schema = "frankenlife.experiment.v1"
name = "v2-controls"
hypothesis = "V2-01 model-family-aware ablations"
base = "base.toml"
ticks = 8
control = "control"
selection = "all runs reported"

[seeds]
discovery = [1]

[[conditions]]
name = "control"

[[conditions]]
name = "chemistry-only"
ablation = "chemistry-only"

[[conditions]]
name = "fresh-offspring"
ablation = "fresh-offspring"

[[conditions]]
name = "fixed-controller"
ablation = "fixed-controller"
"#,
    )
    .expect("spec");
    let exp = Experiment::load_with_overrides(&spec_path, &[flife_core::FeatureOverride::V1Compat])
        .expect("loads v2 experiment");
    assert_eq!(
        exp.configuration(0, 1).expect("ctrl").config().model,
        "field-ecology-v2"
    );
    assert_eq!(
        exp.configuration(1, 1).expect("chem").config().model,
        "field-ecology-v2-chemistry-only"
    );
    assert_eq!(
        exp.configuration(2, 1).expect("fresh").config().model,
        "field-ecology-v2-fresh-offspring"
    );
    assert_eq!(
        exp.configuration(3, 1).expect("fixed").config().model,
        "field-ecology-v2-fixed-controller"
    );
}
