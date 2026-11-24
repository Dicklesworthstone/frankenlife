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
    assert!(
        inspected[1].get("secretory_readiness").is_none(),
        "secretory_readiness must be omitted when excitable-relay is disabled"
    );

    // 5b. With `--enable excitable-relay`, `flife inspect` exposes `secretory_readiness` in [0, 1].
    let excitable_dir = dir.join("run_excitable");
    let (status, _, err) = flife(&[
        "run",
        text(&v2_path),
        "--disable",
        "all-v2",
        "--enable",
        "excitable-relay",
        "--ticks",
        "8",
        "--every",
        "0",
        "--controllers",
        "cpu",
        "--run-dir",
        text(&excitable_dir),
        "--checkpoint-every",
        "8",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    let (status, inspected_ex, err) = flife(&["inspect", text(&excitable_dir)]);
    assert_eq!(status, Some(0), "{err:?}");
    let readiness = inspected_ex[1]["secretory_readiness"]
        .as_f64()
        .expect("secretory_readiness exposed when excitable-relay enabled");
    assert!((0.0..=1.0).contains(&readiness));

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

    // 8. `flife inspect --field` exposes `trail_energy` (V2-15) and
    //    `vascular_conductance` (V2-14) on default `field-ecology-v2`, and
    //    omits both when disabled (`--disable all-v2`).
    let wave2_dir = dir.join("wave2_run");
    let (status, _, err) = flife(&[
        "run",
        text(&v2_path),
        "--disable",
        "all-v2",
        "--enable",
        "vascular-field,dual-carrier-trails",
        "--ticks",
        "8",
        "--every",
        "0",
        "--controllers",
        "cpu",
        "--run-dir",
        text(&wave2_dir),
        "--checkpoint-every",
        "8",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    let (status, inspected_wave2, err) = flife(&["inspect", text(&wave2_dir), "--field"]);
    assert_eq!(status, Some(0), "{err:?}");
    let field_rec = inspected_wave2
        .iter()
        .find(|record| record["record"] == "field")
        .expect("field record");
    let cells_x = field_rec["cells"][0].as_u64().expect("cells_x") as usize;
    let cells_y = field_rec["cells"][1].as_u64().expect("cells_y") as usize;
    let total_cells = cells_x * cells_y;
    let trail = field_rec["trail_energy"]
        .as_array()
        .expect("trail_energy exposed when dual-carrier-trails enabled");
    assert_eq!(trail.len(), total_cells);
    let vascular = field_rec["vascular_conductance"]
        .as_array()
        .expect("vascular_conductance exposed when vascular-field enabled");
    assert_eq!(vascular.len(), total_cells);
    assert!(
        vascular
            .iter()
            .all(|v| v.as_f64().is_some_and(|g| (0.01..=1.0).contains(&g)))
    );

    let (status, inspected_v1_field, err) = flife(&["inspect", text(&run_dir), "--field"]);
    assert_eq!(status, Some(0), "{err:?}");
    let oases_only_field = inspected_v1_field
        .iter()
        .find(|record| record["record"] == "field")
        .expect("field record");
    assert!(
        oases_only_field.get("trail_energy").is_none(),
        "trail_energy must be omitted when dual-carrier-trails is disabled"
    );
    assert!(
        oases_only_field.get("vascular_conductance").is_none(),
        "vascular_conductance must be omitted when vascular-field is disabled"
    );

    // 9. `flife inspect` exposes `polarity`, `polarity_strength`, and
    //    `wedge_curvature` when `V2-08` `pcp-polarity` is enabled, and omits
    //    all three when disabled (plus exercises `V2-11` lysis + waste CLI run).
    let pcp_path = dir.join("v2_pcp.toml");
    fs::write(
        &pcp_path,
        ecology_toml(
            "field-ecology-v2",
            r#"
[contact]
diameter = 0.5
stiffness = 0.75
adhesion = 0.35
adhesion_range = 2.0
sweeps = 2

[v2]
profile = "minimal"

[v2.polarity]
enabled = true
maximum_wedge_curvature = 0.25

[v2.metabolism.lysis]
enabled = true

[v2.metabolism.waste]
enabled = true
"#,
        ),
    )
    .expect("pcp toml");
    let pcp_dir = dir.join("pcp_run");
    let (status, _, err) = flife(&[
        "run",
        text(&pcp_path),
        "--ticks",
        "8",
        "--every",
        "0",
        "--controllers",
        "cpu",
        "--run-dir",
        text(&pcp_dir),
        "--checkpoint-every",
        "8",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    let (status, inspected_pcp, err) = flife(&["inspect", text(&pcp_dir)]);
    assert_eq!(status, Some(0), "{err:?}");
    let org_pcp = inspected_pcp
        .iter()
        .find(|record| record["record"] == "organism")
        .expect("organism record");
    let pol = org_pcp["polarity"]
        .as_array()
        .expect("polarity exposed when pcp-polarity enabled");
    assert_eq!(pol.len(), 2);
    let px = pol[0].as_f64().expect("px");
    let py = pol[1].as_f64().expect("py");
    assert!((px.hypot(py) - 1.0).abs() <= 1e-12);
    let alpha = org_pcp["polarity_strength"]
        .as_f64()
        .expect("polarity_strength exposed");
    assert!((0.0..=1.0).contains(&alpha));
    let beta = org_pcp["wedge_curvature"]
        .as_f64()
        .expect("wedge_curvature exposed");
    assert!((-0.25..=0.25).contains(&beta));

    let org_no_pcp = inspected_v1_field
        .iter()
        .find(|record| record["record"] == "organism")
        .expect("non-pcp organism");
    assert!(org_no_pcp.get("polarity").is_none());
    assert!(org_no_pcp.get("polarity_strength").is_none());
    assert!(org_no_pcp.get("wedge_curvature").is_none());
    assert!(org_no_pcp.get("epigenetic_state").is_none());
    assert!(org_no_pcp.get("effective_affinity").is_none());
    assert!(org_no_pcp.get("last_neighbor_h0").is_none());

    // 10. `V2-12` (`epigenetic-recurrence` + `notch-delta`) CLI run with batched
    //     `--controllers torch` & `flife inspect` exposes `epigenetic_state`,
    //     `effective_affinity`, and `last_neighbor_h0` when enabled, and omits
    //     all three when disabled (`--disable epigenetic-recurrence,notch-delta`).
    let diff_path = dir.join("v2_diff.toml");
    fs::write(
        &diff_path,
        ecology_toml(
            "field-ecology-v2",
            r#"
[contact]
diameter = 0.5
stiffness = 0.75
adhesion = 0.35
adhesion_range = 2.0
sweeps = 2

[v2]
profile = "minimal"

[v2.differentiation]
enabled = true
state_width = 2
epigenetic_gate_bias = 2.5
state_inheritance = "asymmetric-stem-v1"

[v2.differentiation.notch_delta]
enabled = true
mode = "sensor-neighbor-mean-v1"
"#,
        ),
    )
    .expect("diff toml");
    let diff_cpu_dir = dir.join("diff_cpu");
    let (status, diff_cpu_out, err) = flife(&[
        "run",
        text(&diff_path),
        "--ticks",
        "8",
        "--every",
        "0",
        "--controllers",
        "cpu",
        "--run-dir",
        text(&diff_cpu_dir),
        "--checkpoint-every",
        "8",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    let diff_torch_dir = dir.join("diff_torch");
    let (status, diff_torch_out, err) = flife(&[
        "run",
        text(&diff_path),
        "--ticks",
        "8",
        "--every",
        "0",
        "--controllers",
        "torch",
        "--run-dir",
        text(&diff_torch_dir),
        "--checkpoint-every",
        "8",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    assert_eq!(
        diff_torch_out.last().expect("torch end")["digest"],
        diff_cpu_out.last().expect("cpu end")["digest"],
        "V2-12 torch batched recurrent run must match cpu-reference bit-for-bit"
    );
    let (status, inspected_diff, err) = flife(&["inspect", text(&diff_cpu_dir)]);
    assert_eq!(status, Some(0), "{err:?}");
    let org_diff = inspected_diff
        .iter()
        .find(|record| record["record"] == "organism")
        .expect("differentiated organism");
    let epi = org_diff["epigenetic_state"]
        .as_array()
        .expect("epigenetic_state exposed");
    assert_eq!(epi.len(), 2);
    let eff = org_diff["effective_affinity"]
        .as_array()
        .expect("effective_affinity exposed");
    assert_eq!(eff.len(), 32);
    let neighbor_h0 = org_diff["last_neighbor_h0"]
        .as_f64()
        .expect("last_neighbor_h0 exposed");
    assert!((-1.0..=1.0).contains(&neighbor_h0));
}
