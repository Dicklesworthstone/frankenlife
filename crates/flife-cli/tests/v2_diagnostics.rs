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

use flife_cli::{InspectOptions, Inspection, MotorAgencyDiagnostics, v2_diagnostics};
use flife_core::ValidatedConfig;
use flife_engine::{OrganismSnapshot, Stepped, World};

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
