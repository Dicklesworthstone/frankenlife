//! Integration tests for `flife_cli::v2_diagnostics`: live `World`,
//! `OrganismSnapshot`, and checkpointed `Inspection` parity across treatment,
//! `chemistry-only`, `so2-motor`-disabled, and `field-ecology-v1` worlds.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use flife_cli::{
    ControllerChoice, Experiment, ExperimentOptions, InspectOptions, Inspection, MEASURES,
    MotorAgencyDiagnostics, Spec, V2Diagnostics, V2Measures, analyze, experiment, v2_diagnostics,
};
use flife_core::{CoreError, V2Feature, ValidatedConfig};
use flife_engine::{OrganismSnapshot, Stepped, World};
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
        ("population", "initial_count", "48"),
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
        .join("flife-v2-diagnostics")
        .join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("clears previous scratch dir");
    }
    fs::create_dir_all(&dir).expect("creates scratch dir");
    dir
}

fn step_world(toml_text: &str, ticks: u64) -> World {
    let config = ValidatedConfig::from_toml(toml_text).expect("valid configuration");
    let mut world = flife_cli::found(&config).expect("world founds");
    while world.tick().get() < ticks {
        match world.step().expect("tick steps") {
            Stepped::Published(_) => {}
            Stepped::Paused(shortfall) => panic!("paused unexpectedly: {shortfall:?}"),
        }
    }
    world
}

fn all_snapshots(world: &World) -> Vec<OrganismSnapshot> {
    world
        .population()
        .iter()
        .map(|organism| {
            world
                .organism_snapshot(organism.id)
                .expect("snapshot succeeds")
                .expect("live organism present")
        })
        .collect()
}

fn run_and_inspect(name: &str, toml_text: &str, ticks: u64) -> Inspection {
    let dir = scratch(name);
    let cfg_path = dir.join("config.toml");
    let run_dir = dir.join("run");
    fs::write(&cfg_path, toml_text).expect("writes config");
    let ticks_str = ticks.to_string();
    let output = Command::new(env!("CARGO_BIN_EXE_flife"))
        .args([
            "run",
            cfg_path.to_str().expect("UTF-8 config path"),
            "--ticks",
            &ticks_str,
            "--every",
            "0",
            "--controllers",
            "cpu",
            "--run-dir",
            run_dir.to_str().expect("UTF-8 run dir"),
            "--checkpoint-every",
            &ticks_str,
            "--checkpoint-durability",
            "process-crash",
        ])
        .output()
        .expect("flife run executes");
    assert_eq!(
        output.status.code(),
        Some(0),
        "flife run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    flife_cli::inspect(&run_dir, InspectOptions::default()).expect("flife inspect succeeds")
}

#[test]
fn live_v2_world_snapshots_and_inspection_diagnostics_agree() {
    let tail = r#"
[contact]
diameter = 0.5
stiffness = 0.75
adhesion = 0.35
adhesion_range = 1.5

[v2]
profile = "minimal"

[v2.motor]
enabled = true
"#;
    let toml_text = ecology_toml("field-ecology-v2", tail);
    let ticks = 12_u64;
    let world = step_world(&toml_text, ticks);
    let snapshots = all_snapshots(&world);
    let inspection = run_and_inspect("treatment", &toml_text, ticks);

    assert_eq!(inspection.inspected.digest, world.digest().to_hex());

    // Geometry: world, explicit diameter, and restored inspection agree bit-for-bit.
    let geom_world = v2_diagnostics::geometry_of_world(&world)
        .expect("geometry_of_world")
        .expect("[contact] is configured");
    let geom_explicit =
        v2_diagnostics::geometry_of_world_with_diameter(&world, 0.5).expect("explicit diameter");
    let geom_insp =
        v2_diagnostics::geometry_of_inspection(&inspection, 0.5).expect("inspection geometry");
    assert_eq!(geom_world, geom_explicit);
    assert_eq!(geom_world, geom_insp);
    assert_eq!(geom_world.count, world.population().len());
    assert!(geom_world.count >= 2);
    assert!(
        geom_world
            .median_d1_over_d
            .is_some_and(|v| v.is_finite() && v >= 0.0)
    );
    assert!(
        geom_world.mean_overlaps_within_1d.is_finite() && geom_world.mean_overlaps_within_1d >= 0.0
    );
    let hist_sum: f64 = geom_world.coord_hist_0_to_8.iter().sum();
    assert!((hist_sum - 1.0).abs() < 1e-12);
    assert!(
        (geom_world.chain_fraction_z1_z2
            - (geom_world.coord_hist_0_to_8[1] + geom_world.coord_hist_0_to_8[2]))
            .abs()
            < 1e-12
    );
    assert!((0.0..=1.0 + 1e-12).contains(&geom_world.mean_abs_psi6));
    assert!((0.0..=geom_world.mean_abs_psi6 + 1e-12).contains(&geom_world.global_abs_mean_psi6));

    // Motor agency: world and live snapshots match bit-for-bit on both correlations;
    // checkpoint inspection matches bit-for-bit on corr(m_e, E) and reports None for dE/dt.
    let motor_world = v2_diagnostics::motor_agency_of_world(&world).expect("world motor agency");
    let motor_snaps =
        v2_diagnostics::motor_agency_of_snapshots(&snapshots).expect("snapshot motor agency");
    let motor_insp =
        v2_diagnostics::motor_agency_of_inspection(&inspection).expect("inspection motor agency");

    assert!(motor_world.so2_motor_enabled);
    assert_eq!(motor_world.total_count, world.population().len());
    assert!(motor_world.evaluated_count >= 2);
    assert!(motor_world.evaluated_count <= motor_world.total_count);
    assert!(
        motor_world
            .mean_me
            .is_some_and(|v| (-1.0..=1.0).contains(&v))
    );
    assert!(motor_world.std_me.is_some_and(|s| s > 0.0 && s <= 1.0));
    assert!(
        motor_world
            .corr_me_dedt
            .is_some_and(|r| (-1.0..=1.0).contains(&r))
    );
    assert!(
        motor_world
            .corr_me_energy
            .is_some_and(|r| (-1.0..=1.0).contains(&r))
    );

    assert_eq!(motor_snaps, motor_world);

    assert_eq!(motor_insp.so2_motor_enabled, motor_world.so2_motor_enabled);
    assert_eq!(motor_insp.total_count, motor_world.total_count);
    assert_eq!(motor_insp.evaluated_count, motor_world.evaluated_count);
    assert_eq!(motor_insp.mean_me, motor_world.mean_me);
    assert_eq!(motor_insp.std_me, motor_world.std_me);
    assert_eq!(motor_insp.corr_me_energy, motor_world.corr_me_energy);
    assert_eq!(motor_insp.corr_me_dedt, None);

    // Combined wrappers agree identically.
    let combined_world = v2_diagnostics::of_world(&world).expect("of_world");
    assert_eq!(combined_world.geometry, Some(geom_world));
    assert_eq!(combined_world.motor_agency, motor_world);

    let combined_insp =
        v2_diagnostics::of_inspection(&inspection, Some(0.5)).expect("of_inspection");
    assert_eq!(combined_insp.geometry, Some(geom_insp));
    assert_eq!(combined_insp.motor_agency, motor_insp);

    let combined_insp_no_d =
        v2_diagnostics::of_inspection(&inspection, None).expect("of_inspection without diameter");
    assert_eq!(combined_insp_no_d.geometry, None);
    assert_eq!(combined_insp_no_d.motor_agency, motor_insp);
}

#[test]
fn chemistry_only_control_zeroes_effective_motor_variance() {
    let tail = r#"
[contact]
diameter = 0.5
stiffness = 0.75
adhesion = 0.35
adhesion_range = 1.5

[v2]
profile = "minimal"

[v2.motor]
enabled = true
"#;
    let toml_text = ecology_toml("field-ecology-v2-chemistry-only", tail);
    let ticks = 12_u64;
    let world = step_world(&toml_text, ticks);
    let snapshots = all_snapshots(&world);
    let inspection = run_and_inspect("chemistry_only", &toml_text, ticks);

    let motor_world = v2_diagnostics::motor_agency_of_world(&world).expect("world motor agency");
    let motor_snaps =
        v2_diagnostics::motor_agency_of_snapshots(&snapshots).expect("snapshot motor agency");
    let motor_insp =
        v2_diagnostics::motor_agency_of_inspection(&inspection).expect("inspection motor agency");

    assert!(motor_world.so2_motor_enabled);
    assert!(motor_world.evaluated_count >= 2);
    assert_eq!(motor_world.mean_me, Some(0.0));
    assert_eq!(motor_world.std_me, Some(0.0));
    assert_eq!(motor_world.corr_me_dedt, None);
    assert_eq!(motor_world.corr_me_energy, None);

    assert_eq!(motor_snaps, motor_world);
    assert_eq!(motor_insp, motor_world);
}

#[test]
fn disabled_so2_motor_and_v1_report_explicit_unavailability() {
    let v2_minimal = ecology_toml(
        "field-ecology-v2",
        r#"
[v2]
profile = "minimal"
"#,
    );
    let world_v2 = step_world(&v2_minimal, 8);
    let snapshots_v2 = all_snapshots(&world_v2);
    let live_v2 = world_v2.population().len();

    assert_eq!(
        v2_diagnostics::geometry_of_world(&world_v2).expect("no contact"),
        None
    );
    let explicit_geom =
        v2_diagnostics::geometry_of_world_with_diameter(&world_v2, 1.0).expect("explicit");
    assert_eq!(explicit_geom.count, live_v2);
    assert_eq!(
        v2_diagnostics::motor_agency_of_world(&world_v2).expect("disabled"),
        MotorAgencyDiagnostics::unavailable(live_v2)
    );
    assert_eq!(
        v2_diagnostics::motor_agency_of_snapshots(&snapshots_v2).expect("disabled snaps"),
        MotorAgencyDiagnostics::unavailable(live_v2)
    );

    let v1_toml = ecology_toml("field-ecology-v1", "");
    let world_v1 = step_world(&v1_toml, 4);
    let live_v1 = world_v1.population().len();
    assert_eq!(
        v2_diagnostics::geometry_of_world(&world_v1).expect("v1 no contact"),
        None
    );
    assert_eq!(
        v2_diagnostics::motor_agency_of_world(&world_v1).expect("v1 motor"),
        MotorAgencyDiagnostics::unavailable(live_v1)
    );
}

#[test]
fn flife_inspect_diagnostics_cli_and_library_match_live_world_including_dedt() {
    let tail = r#"
[contact]
diameter = 0.5
stiffness = 0.75
adhesion = 0.35
adhesion_range = 1.5

[v2]
profile = "minimal"

[v2.motor]
enabled = true
"#;
    let toml_text = ecology_toml("field-ecology-v2", tail);
    let ticks = 12_u64;
    let world = step_world(&toml_text, ticks);
    let expected = v2_diagnostics::of_world(&world).expect("live world diagnostics");
    assert!(expected.geometry.is_some());
    assert!(expected.motor_agency.corr_me_dedt.is_some());

    let dir = scratch("inspect_cli_diagnostics");
    let cfg_path = dir.join("config.toml");
    let run_dir = dir.join("run");
    fs::write(&cfg_path, &toml_text).expect("writes config");
    let run_out = Command::new(env!("CARGO_BIN_EXE_flife"))
        .args([
            "run",
            cfg_path.to_str().expect("UTF-8"),
            "--ticks",
            "12",
            "--every",
            "0",
            "--controllers",
            "cpu",
            "--run-dir",
            run_dir.to_str().expect("UTF-8"),
            "--checkpoint-every",
            "12",
            "--checkpoint-durability",
            "process-crash",
        ])
        .output()
        .expect("flife run executes");
    assert_eq!(
        run_out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&run_out.stderr)
    );

    // Without --diagnostics: no diagnostics record emitted.
    let plain_out = Command::new(env!("CARGO_BIN_EXE_flife"))
        .args(["inspect", run_dir.to_str().expect("UTF-8")])
        .output()
        .expect("flife inspect executes");
    assert_eq!(plain_out.status.code(), Some(0));
    let plain_records: Vec<Value> = String::from_utf8(plain_out.stdout)
        .expect("UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("valid JSON"))
        .collect();
    assert!(
        plain_records.iter().all(|r| r["record"] != "diagnostics"),
        "plain inspect must not emit diagnostics"
    );

    // With --diagnostics: library and CLI both match live world bit-for-bit,
    // including real `corr_me_dedt` from checkpoint-restored `energy_change_rate`.
    let inspected_lib = flife_cli::inspect(
        &run_dir,
        InspectOptions {
            diagnostics: true,
            ..InspectOptions::default()
        },
    )
    .expect("inspect with diagnostics");
    let lib_diag = inspected_lib
        .diagnostics
        .expect("diagnostics present")
        .diagnostics;
    assert_eq!(lib_diag, expected);

    let diag_out = Command::new(env!("CARGO_BIN_EXE_flife"))
        .args(["inspect", run_dir.to_str().expect("UTF-8"), "--diagnostics"])
        .output()
        .expect("flife inspect --diagnostics executes");
    assert_eq!(
        diag_out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&diag_out.stderr)
    );
    let diag_records: Vec<Value> = String::from_utf8(diag_out.stdout)
        .expect("UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("valid JSON"))
        .collect();
    let diag_line = diag_records.last().expect("last record is diagnostics");
    assert_eq!(diag_line["record"], "diagnostics");
    let parsed: V2Diagnostics = serde_json::from_value(serde_json::json!({
        "geometry": diag_line["geometry"],
        "motor_agency": diag_line["motor_agency"],
    }))
    .expect("deserializes V2Diagnostics");
    assert_eq!(parsed, expected);

    // Explicit --diameter override changes geometry diameter cleanly.
    let expected_override =
        v2_diagnostics::geometry_of_world_with_diameter(&world, 1.25).expect("override");
    let override_out = Command::new(env!("CARGO_BIN_EXE_flife"))
        .args([
            "inspect",
            run_dir.to_str().expect("UTF-8"),
            "--diagnostics",
            "--diameter",
            "1.25",
        ])
        .output()
        .expect("flife inspect --diagnostics --diameter executes");
    assert_eq!(override_out.status.code(), Some(0));
    let override_records: Vec<Value> = String::from_utf8(override_out.stdout)
        .expect("UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("valid JSON"))
        .collect();
    let override_diag = override_records.last().expect("diagnostics record");
    let override_parsed: V2Diagnostics = serde_json::from_value(serde_json::json!({
        "geometry": override_diag["geometry"],
        "motor_agency": override_diag["motor_agency"],
    }))
    .expect("deserializes override V2Diagnostics");
    assert_eq!(override_parsed.geometry, Some(expected_override));
    assert_eq!(override_parsed.motor_agency, expected.motor_agency);

    // Library refuses dangling diameter without diagnostics and non-positive diameter.
    assert!(matches!(
        flife_cli::inspect(
            &run_dir,
            InspectOptions {
                diagnostics: false,
                diameter: Some(0.5),
                ..InspectOptions::default()
            }
        ),
        Err(CoreError::NumericalInvalid { .. })
    ));
    assert!(matches!(
        flife_cli::inspect(
            &run_dir,
            InspectOptions {
                diagnostics: true,
                diameter: Some(0.0),
                ..InspectOptions::default()
            }
        ),
        Err(CoreError::NumericalInvalid { .. })
    ));
}

#[test]
fn v2_experiment_and_analyze_support_per_condition_toggles_and_v2_measures() {
    let tail = r#"
[contact]
diameter = 0.5
stiffness = 0.75
adhesion = 0.35
adhesion_range = 1.5

[v2]
profile = "minimal"

[v2.motor]
enabled = true

[v2.polarity]
enabled = true
"#;
    let base_toml = ecology_toml("field-ecology-v2", tail);
    let make_spec = |name: &str, discovery: &str, held_out: &str, no_pcp_disable: &str| -> Spec {
        let text = format!(
            r#"
schema = "frankenlife.experiment.v1"
name = "{name}"
hypothesis = "Per-condition V2 toggles and end-state V2Measures flow through experiment and analyze."
base = "base.toml"
ticks = 12
metrics_every = 12
control = "control"
selection = "All conditions run on discovery and held-out seeds."

[seeds]
discovery = {discovery}
held_out = {held_out}

[[conditions]]
name = "control"

[[conditions]]
name = "no-pcp"
disable = [{no_pcp_disable}]

[[conditions]]
name = "v1-baseline"
v1_compat = true

[[conditions]]
name = "chemistry-only"
ablation = "chemistry-only"
"#
        );
        toml::from_str(&text).expect("valid spec TOML")
    };

    let exp_a = Experiment::new(
        make_spec("v2-a", "[1, 2]", "[11]", "\"pcp-polarity\""),
        &base_toml,
    )
    .expect("exp_a resolves");
    // Per-condition feature toggles resolve directly on each condition's ValidatedConfig.
    let cfg_control = exp_a.configuration(0, 1).expect("control cfg");
    let cfg_no_pcp = exp_a.configuration(1, 1).expect("no-pcp cfg");
    let cfg_v1 = exp_a.configuration(2, 1).expect("v1-baseline cfg");
    assert!(cfg_control.is_v2_feature_enabled(V2Feature::PcpPolarity));
    assert!(cfg_control.is_v2_feature_enabled(V2Feature::So2Motor));
    assert!(!cfg_no_pcp.is_v2_feature_enabled(V2Feature::PcpPolarity));
    assert!(cfg_no_pcp.is_v2_feature_enabled(V2Feature::So2Motor));
    assert!(!cfg_v1.is_v2_feature_enabled(V2Feature::PcpPolarity));
    assert!(!cfg_v1.is_v2_feature_enabled(V2Feature::So2Motor));

    // Selection rules accept V2 measures.
    let select_spec_text = r#"
schema = "frankenlife.experiment.v1"
name = "v2-select"
hypothesis = "Select on mean_abs_psi6."
base = "base.toml"
ticks = 12
control = "control"
selection = "Pick max mean_abs_psi6 on discovery."

[seeds]
discovery = [1]

[select]
measure = "mean_abs_psi6"
rule = "max"

[[conditions]]
name = "control"

[[conditions]]
name = "no-pcp"
disable = ["pcp-polarity"]
"#;
    assert!(
        Experiment::new(toml::from_str(select_spec_text).expect("parse"), &base_toml).is_ok(),
        "[select] must accept V2Measures"
    );

    // Invalid per-condition toggles are refused upfront before any run starts:
    // unknown feature slug, and V2 toggles on a field-ecology-v1 base.
    assert!(
        Experiment::new(
            make_spec("bad-slug", "[1]", "[]", "\"not-a-real-feature\""),
            &base_toml
        )
        .is_err()
    );
    let v1_base = ecology_toml("field-ecology-v1", "");
    assert!(
        Experiment::new(
            make_spec("bad-v1", "[1]", "[]", "\"pcp-polarity\""),
            &v1_base
        )
        .is_err()
    );

    // Run two disjoint-seed V2 experiments and pool them with `flife analyze`.
    let dir_a = scratch("v2_exp_a");
    let report_a = experiment(
        &exp_a,
        &dir_a,
        ExperimentOptions {
            cpu_workers: None,
            controllers: ControllerChoice::Cpu,
        },
        |_| {},
    )
    .expect("exp_a runs");
    assert_eq!(report_a.measures.len(), MEASURES.len());
    assert_eq!(MEASURES.len(), 33);

    // Verify cataloged V2Measures on control seed 1 match live `v2_diagnostics::of_world` bit-for-bit.
    let mut direct = flife_cli::found(&cfg_control).expect("direct world");
    for _ in 0..12 {
        match direct.step().expect("step") {
            Stepped::Published(_) => {}
            Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
        }
    }
    let expected_v2 = V2Measures::of_world(&direct)
        .expect("v2 measures")
        .expect("v2 active");
    let control_seed_1 = report_a
        .runs
        .iter()
        .find(|r| r.condition == "control" && r.seed == 1)
        .expect("control seed 1");
    assert_eq!(
        control_seed_1.result.measures.expect("measures").v2,
        Some(expected_v2)
    );
    assert!(expected_v2.median_d1_over_d.is_some());
    assert!(expected_v2.mean_abs_psi6.is_some());
    assert!(expected_v2.corr_me_dedt.is_some());
    assert!(expected_v2.corr_me_energy.is_some());

    // `chemistry-only` zeroes motor variance -> corr_me_dedt / corr_me_energy are undefined
    // and listed in `undefined_seeds` rather than treated as 0.
    let chem_comp = report_a
        .comparisons
        .iter()
        .find(|c| c.condition == "chemistry-only" && c.split == "discovery")
        .expect("chemistry-only discovery comparison");
    assert_eq!(chem_comp.effects["corr_me_dedt"].n, 0);
    assert_eq!(chem_comp.effects["corr_me_dedt"].undefined_seeds, [1, 2]);
    assert_eq!(chem_comp.effects["mean_abs_psi6"].n, 2);

    // `no-pcp` keeps both contact geometry and so2-motor active -> all 9 V2 measures paired.
    let no_pcp_comp = report_a
        .comparisons
        .iter()
        .find(|c| c.condition == "no-pcp" && c.split == "discovery")
        .expect("no-pcp discovery comparison");
    for name in [
        "median_d1_over_d",
        "mean_overlaps_within_1d",
        "chain_fraction_z1_z2",
        "mean_abs_psi6",
        "global_abs_mean_psi6",
        "mean_me",
        "std_me",
        "corr_me_dedt",
        "corr_me_energy",
    ] {
        assert_eq!(no_pcp_comp.effects[name].n, 2, "measure {name}");
    }

    // Second disjoint experiment pools cleanly through `analyze`.
    let exp_b = Experiment::new(
        make_spec("v2-b", "[3, 4]", "[12]", "\"pcp-polarity\""),
        &base_toml,
    )
    .expect("exp_b resolves");
    let dir_b = scratch("v2_exp_b");
    experiment(
        &exp_b,
        &dir_b,
        ExperimentOptions {
            cpu_workers: None,
            controllers: ControllerChoice::Cpu,
        },
        |_| {},
    )
    .expect("exp_b runs");
    let pooled =
        analyze(&[dir_a.as_path(), dir_b.as_path()]).expect("analyze pools V2 experiments");
    assert_eq!(pooled.measures.len(), MEASURES.len());
    let pooled_no_pcp = pooled
        .comparisons
        .iter()
        .find(|c| c.condition == "no-pcp" && c.split == "discovery")
        .expect("pooled no-pcp discovery");
    assert_eq!(pooled_no_pcp.effects["mean_abs_psi6"].n, 4);
    assert_eq!(pooled_no_pcp.effects["corr_me_dedt"].n, 4);

    // `analyze` refuses pooling when a condition of the same name has different feature toggles.
    let exp_mismatch = Experiment::new(
        make_spec(
            "v2-mismatch",
            "[5]",
            "[]",
            "\"pcp-polarity\", \"so2-motor\"",
        ),
        &base_toml,
    )
    .expect("mismatch resolves");
    let dir_m = scratch("v2_exp_mismatch");
    experiment(
        &exp_mismatch,
        &dir_m,
        ExperimentOptions {
            cpu_workers: None,
            controllers: ControllerChoice::Cpu,
        },
        |_| {},
    )
    .expect("mismatch runs");
    assert!(matches!(
        analyze(&[dir_a.as_path(), dir_m.as_path()]),
        Err(CoreError::StateConflict { .. })
    ));
}
