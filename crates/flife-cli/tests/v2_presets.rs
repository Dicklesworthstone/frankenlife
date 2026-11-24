//! V2-17 acceptance tests for the modular `field-ecology-v2` showcase presets
//! (`presets/morphogenesis-tissue-v2.toml` Bundle A and
//! `presets/supercolony-v2.toml` Bundle B), simultaneous composition of all 21
//! `V2Feature` toggles (`112/44/48/34` Metal-F16-aligned ABI), and exhaustive
//! leave-one-out `--disable <slug>` sweeps across every `V2Feature::ALL` entry.

#![forbid(unsafe_code)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use flife_backend_metal::MetalControllers;
use flife_cli::{
    DISCOVERY_SCHEMA, ERROR_SCHEMA, INSPECT_SCHEMA, METRICS_SCHEMA, RENDER_SCHEMA, RUN_SCHEMA,
    VALIDATE_SCHEMA,
};
use flife_core::schema::Scope;
use flife_core::{CoreError, FeatureOverride, ModelDescriptor, V2Abi, V2Feature, ValidatedConfig};
use serde_json::Value;

const BUNDLE_A_RAW: &str = include_str!("../../../presets/morphogenesis-tissue-v2.toml");
const BUNDLE_B_RAW: &str = include_str!("../../../presets/supercolony-v2.toml");

const BUNDLE_A_MODEL_FEATURES: [&str; 13] = [
    "cadherin-4d",
    "epigenetic-recurrence",
    "hippo-crowding",
    "lysis-recycling",
    "notch-delta",
    "pcp-polarity",
    "senescence",
    "so2-motor",
    "surplus-emission",
    "symplectic-waste",
    "tight-junctions",
    "trophallaxis",
    "wake-offset",
];

const BUNDLE_B_MODEL_FEATURES: [&str; 10] = [
    "dual-carrier-trails",
    "excitable-relay",
    "lysis-recycling",
    "oases",
    "senescence",
    "so2-motor",
    "surplus-emission",
    "tight-junctions",
    "vascular-field",
    "wake-offset",
];

/// Maximum normalized energy-ledger residual tolerated for short reference
/// runs under `INV-03` (reference runs are typically ~1e-10..1e-8).
const MAX_LEDGER_RESIDUAL: f64 = 1.0e-5;

const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

fn preset_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../presets")
        .join(name)
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("flife-v2-presets")
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
            .map(|line| {
                serde_json::from_str(line)
                    .unwrap_or_else(|err| panic!("invalid JSON line `{line}`: {err}"))
            })
            .collect()
    };
    (
        output.status.code(),
        parse(&output.stdout),
        parse(&output.stderr),
    )
}

fn find_record<'a>(records: &'a [Value], kind: &str) -> &'a Value {
    records
        .iter()
        .find(|record| record["record"] == kind)
        .unwrap_or_else(|| panic!("missing `{kind}` record in {records:?}"))
}

fn json_strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("JSON string array")
        .iter()
        .map(|entry| entry.as_str().expect("string element").to_owned())
        .collect()
}

fn assert_hex_sha256(digest: &str, context: &str) {
    assert_eq!(
        digest.len(),
        64,
        "{context}: digest must be 64 hex chars, got `{digest}`"
    );
    assert!(
        digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "{context}: digest must be hexadecimal, got `{digest}`"
    );
}

fn assert_healthy_end(end: &Value, expected_tick: u64, context: &str) -> (String, u64, f64) {
    assert_eq!(end["schema"], RUN_SCHEMA, "{context}: run schema");
    assert_eq!(
        end["stop"]["end"], "reached",
        "{context}: run must reach requested ticks, got {end:?}"
    );
    assert_eq!(end["tick"], expected_tick, "{context}: committed tick");
    let digest = end["digest"]
        .as_str()
        .unwrap_or_else(|| panic!("{context}: missing digest in {end:?}"))
        .to_owned();
    assert_hex_sha256(&digest, context);
    let live = end["survey"]["live"]
        .as_u64()
        .unwrap_or_else(|| panic!("{context}: missing survey.live in {end:?}"));
    assert!(
        live > 0,
        "{context}: population went extinct in {expected_tick} ticks"
    );
    let residual = end["tally"]["normalized_residual"]
        .as_f64()
        .unwrap_or_else(|| panic!("{context}: missing tally.normalized_residual in {end:?}"));
    assert!(
        residual.is_finite() && (0.0..=MAX_LEDGER_RESIDUAL).contains(&residual),
        "{context}: INV-03 normalized residual {residual:?} out of bounds [0, {MAX_LEDGER_RESIDUAL}]"
    );
    (digest, live, residual)
}

fn set_toml_key(lines: &mut [String], section: &str, key: &str, value: &str) {
    let start = if section.is_empty() {
        0
    } else {
        let header = format!("[{section}]");
        lines
            .iter()
            .position(|line| line.trim() == header)
            .unwrap_or_else(|| panic!("missing TOML section `{header}`"))
            + 1
    };
    let offset = lines[start..]
        .iter()
        .take_while(|line| !line.trim_start().starts_with('['))
        .position(|line| line.split('=').next().map(str::trim) == Some(key))
        .unwrap_or_else(|| panic!("missing TOML key `{key}` in section `{section}`"));
    lines[start + offset] = format!("{key} = {value}");
}

/// Compact 64×64 (`h = 0.25`, `16×16` torus) copy of a showcase preset that
/// preserves its exact `[contact]` and `[v2]` feature closure for fast E2E runs.
fn scale_preset_64(raw_toml: &str, initial_count: u32, extra: &[(&str, &str, &str)]) -> String {
    let mut lines: Vec<String> = raw_toml.lines().map(str::to_owned).collect();
    let founders = initial_count.to_string();
    let defaults = [
        ("world", "width", "16.0"),
        ("world", "height", "16.0"),
        ("world", "grid_width", "64"),
        ("world", "grid_height", "64"),
        ("population", "initial_count", founders.as_str()),
        ("population", "maximum_count", "256"),
        ("sources", "count", "3"),
        ("sources", "power_per_source", "200.0"),
        ("sources", "sigma_world_units", "1.5"),
        ("v2.oases", "sigma_min_world_units", "1.0"),
        ("v2.oases", "sigma_max_world_units", "1.8"),
    ];
    for (section, key, value) in defaults {
        set_toml_key(&mut lines, section, key, value);
    }
    for &(section, key, value) in extra {
        set_toml_key(&mut lines, section, key, value);
    }
    lines.join("\n")
}

/// Compact 32×32 (`h = 0.25`, `8×8` torus) copy for the 21-iteration
/// leave-one-out sweep (`32 % 16 == 0` satisfies the 4-level sensor pyramid).
fn scale_preset_32(raw_toml: &str, initial_count: u32, extra: &[(&str, &str, &str)]) -> String {
    let mut lines: Vec<String> = raw_toml.lines().map(str::to_owned).collect();
    let founders = initial_count.to_string();
    let defaults = [
        ("world", "width", "8.0"),
        ("world", "height", "8.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", founders.as_str()),
        ("population", "maximum_count", "256"),
        ("sources", "count", "3"),
        ("sources", "power_per_source", "200.0"),
        ("sources", "sigma_world_units", "0.8"),
        ("v2.oases", "sigma_min_world_units", "0.6"),
        ("v2.oases", "sigma_max_world_units", "0.9"),
    ];
    for (section, key, value) in defaults {
        set_toml_key(&mut lines, section, key, value);
    }
    for &(section, key, value) in extra {
        set_toml_key(&mut lines, section, key, value);
    }
    lines.join("\n")
}

/// Whether this host opens a Metal device, probed directly from the device.
fn metal_available() -> bool {
    match MetalControllers::open() {
        Ok(_) => true,
        Err(CoreError::MissingCapability { .. }) => false,
        Err(error) => panic!("Metal device opened but failed to compile kernels: {error}"),
    }
}

fn assert_png_frames_exist(records: &[Value], expected_frames: u64, context: &str) {
    let frame_records: Vec<&Value> = records
        .iter()
        .filter(|record| record["record"] == "frame")
        .collect();
    assert_eq!(
        u64::try_from(frame_records.len()).expect("frame count fits u64"),
        expected_frames,
        "{context}: emitted frame count"
    );
    for frame in frame_records {
        let path = frame["path"].as_str().expect("frame path");
        let bytes = fs::read(path)
            .unwrap_or_else(|err| panic!("{context}: failed reading frame `{path}`: {err}"));
        assert!(
            bytes.starts_with(&PNG_MAGIC),
            "{context}: frame `{path}` is not a valid PNG"
        );
    }
}

fn verify_unmodified_preset(
    file_name: &str,
    raw_toml: &str,
    expected_name: &str,
    expected_inv08: bool,
    expected_abi: (u32, u32, u32, u32),
    expected_tag: &str,
    expected_features: &[&str],
) {
    let validated = ValidatedConfig::from_toml(raw_toml)
        .unwrap_or_else(|err| panic!("{file_name} must validate unmodified: {err}"));
    let abi = validated.v2_abi();
    assert_eq!(
        (
            abi.input_width,
            abi.action_width,
            abi.raw_output_width,
            abi.field_planes,
        ),
        expected_abi,
        "{file_name}: exact V2Abi"
    );
    assert!(
        abi.input_width.is_multiple_of(4) && abi.raw_output_width.is_multiple_of(4),
        "{file_name}: ABI must be 4-aligned for zero Metal F16 alignment deferrals: {abi:?}"
    );
    assert_eq!(abi.descriptor_tag(), expected_tag, "{file_name}: ABI tag");
    assert_eq!(
        validated.inv08_pure_field(),
        expected_inv08,
        "{file_name}: inv08_pure_field"
    );
    assert!(
        validated.is_v2_feature_enabled(V2Feature::V2VisualMappings),
        "{file_name}: v2-visual-mappings must be active"
    );
    assert!(
        validated.report().worst_case_fits_logical_quota,
        "{file_name}: worst-case allocation must fit logical quota"
    );

    let path = preset_path(file_name);
    let (status, out, err) = flife(&["validate", text(&path)]);
    assert_eq!(
        status,
        Some(0),
        "{file_name} flife validate failed: {err:?}"
    );
    assert_eq!(out.len(), 1, "{file_name}: single validate JSON record");
    assert_eq!(out[0]["schema"], VALIDATE_SCHEMA);
    assert_eq!(out[0]["name"], expected_name);
    assert_eq!(out[0]["model"], "field-ecology-v2");
    assert_eq!(out[0]["config_hash"], validated.config_hash().to_hex());
    assert_eq!(out[0]["inv08_pure_field"], expected_inv08);
    assert_eq!(out[0]["worst_case_fits_logical_quota"], true);
    assert_eq!(json_strings(&out[0]["v2_features"]), expected_features);
}

fn verify_preset_round_trip(
    dir: &Path,
    label: &str,
    raw_toml: &str,
    expected_inv08: bool,
    expected_features: &[&str],
    disable_slug: &str,
    diagnostic_renders: &[(&str, &[&str], &str, &str)],
) {
    let cfg_path = dir.join(format!("{label}_64.toml"));
    fs::write(&cfg_path, scale_preset_64(raw_toml, 24, &[])).expect("writes compact preset");

    // 1. Uninterrupted 8-tick reference run.
    let (status, full_out, err) = flife(&[
        "run",
        text(&cfg_path),
        "--ticks",
        "8",
        "--every",
        "0",
        "--controllers",
        "cpu",
    ]);
    assert_eq!(status, Some(0), "{label} 8-tick run failed: {err:?}");
    let (full_digest, _, _) =
        assert_healthy_end(find_record(&full_out, "end"), 8, &format!("{label} t8"));

    // 2. Checkpointed 4-tick run -> inspect --field -> resume to tick 8 (v3 parity).
    let run_dir = dir.join(format!("{label}_run"));
    let (status, leg1_out, err) = flife(&[
        "run",
        text(&cfg_path),
        "--ticks",
        "4",
        "--every",
        "0",
        "--controllers",
        "cpu",
        "--run-dir",
        text(&run_dir),
        "--checkpoint-every",
        "4",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{label} 4-tick run failed: {err:?}");
    let (tick4_digest, _, _) =
        assert_healthy_end(find_record(&leg1_out, "end"), 4, &format!("{label} t4"));

    let (status, inspected, err) = flife(&["inspect", text(&run_dir), "--field"]);
    assert_eq!(status, Some(0), "{label} inspect failed: {err:?}");
    let header = find_record(&inspected, "inspect");
    assert_eq!(header["schema"], INSPECT_SCHEMA);
    assert_eq!(header["model"], "field-ecology-v2");
    assert_eq!(header["digest"], tick4_digest);
    assert_eq!(header["inv08_pure_field"], expected_inv08);
    assert_eq!(json_strings(&header["v2_features"]), expected_features);
    assert!(header["live"].as_u64().expect("live count") > 0);
    assert!(
        header["field_energy"]
            .as_f64()
            .is_some_and(|val| val.is_finite() && val >= 0.0)
    );
    assert!(
        header["organism_energy"]
            .as_f64()
            .is_some_and(|val| val.is_finite() && val > 0.0)
    );

    let org = find_record(&inspected, "organism");
    let field = find_record(&inspected, "field");
    assert_eq!(
        field["energy"].as_array().expect("field energy").len(),
        4096
    );
    if expected_inv08 {
        assert!(org["secretory_readiness"].as_f64().is_some());
        assert!(org.get("polarity").is_none());
        assert!(org.get("cadherin_tag").is_none());
        assert_eq!(
            field["trail_energy"]
                .as_array()
                .expect("trail_energy")
                .len(),
            4096
        );
        assert_eq!(
            field["vascular_conductance"]
                .as_array()
                .expect("vascular_conductance")
                .len(),
            4096
        );
    } else {
        assert_eq!(org["polarity"].as_array().expect("polarity").len(), 2);
        assert_eq!(
            org["epigenetic_state"]
                .as_array()
                .expect("epigenetic_state")
                .len(),
            2
        );
        assert_eq!(
            org["effective_affinity"]
                .as_array()
                .expect("effective_affinity")
                .len(),
            32
        );
        assert!(org["last_neighbor_h0"].as_f64().is_some());
        assert_eq!(
            org["cadherin_tag"].as_array().expect("cadherin_tag").len(),
            4
        );
        assert!(org["cohesiveness"].as_f64().is_some());
        assert!(field.get("trail_energy").is_none());
        assert!(field.get("vascular_conductance").is_none());
    }

    let (status, resumed_out, err) = flife(&[
        "resume",
        text(&run_dir),
        "--until",
        "8",
        "--every",
        "0",
        "--controllers",
        "cpu",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{label} resume failed: {err:?}");
    let (resumed_digest, _, _) = assert_healthy_end(
        find_record(&resumed_out, "end"),
        8,
        &format!("{label} resumed t8"),
    );
    assert_eq!(
        resumed_digest, full_digest,
        "{label}: checkpoint v3 resume must match uninterrupted run bit-for-bit"
    );

    // 3. Selective `--disable` toggle omits the feature and diverges cleanly.
    let (status, dis_val, err) = flife(&["validate", text(&cfg_path), "--disable", disable_slug]);
    assert_eq!(
        status,
        Some(0),
        "{label} --disable {disable_slug} validate failed: {err:?}"
    );
    let dis_features = json_strings(&dis_val[0]["v2_features"]);
    assert!(!dis_features.iter().any(|slug| slug == disable_slug));
    assert_eq!(dis_val[0]["inv08_pure_field"], expected_inv08);

    let (status, dis_run, err) = flife(&[
        "run",
        text(&cfg_path),
        "--disable",
        disable_slug,
        "--ticks",
        "8",
        "--every",
        "0",
        "--controllers",
        "cpu",
    ]);
    assert_eq!(
        status,
        Some(0),
        "{label} --disable {disable_slug} run failed: {err:?}"
    );
    let (dis_digest, _, _) = assert_healthy_end(
        find_record(&dis_run, "end"),
        8,
        &format!("{label} disable {disable_slug}"),
    );
    assert_ne!(
        dis_digest, full_digest,
        "{label}: disabling `{disable_slug}` must alter trajectory"
    );

    // 4. Raw render (ticks 0, 4, 8 -> 3 PNGs) preserves exact scientific digest (C.7).
    let raw_out_dir = dir.join(format!("{label}_render_raw"));
    let (status, raw_render, err) = flife(&[
        "render",
        text(&cfg_path),
        "--ticks",
        "8",
        "--every",
        "4",
        "--width",
        "32",
        "--height",
        "32",
        "--controllers",
        "cpu",
        "--look",
        "raw",
        "--out",
        text(&raw_out_dir),
    ]);
    assert_eq!(status, Some(0), "{label} raw render failed: {err:?}");
    let raw_end = find_record(&raw_render, "end");
    assert_eq!(raw_end["schema"], RENDER_SCHEMA);
    assert_eq!(raw_end["look"], "raw");
    assert_eq!(raw_end["frames"], 3);
    assert_eq!(
        raw_end["digest"], full_digest,
        "{label}: C.7 raw render must preserve exact headless world digest"
    );
    assert_png_frames_exist(&raw_render, 3, &format!("{label} raw render"));

    // 5. Diagnostic V2 visual mappings (ticks 0, 4 -> 2 PNGs) preserve tick-4 digest.
    for &(tag, extra_flags, expect_mapping, expect_field_mapping) in diagnostic_renders {
        let diag_dir = dir.join(format!("{label}_render_{tag}"));
        let mut args = vec![
            "render",
            text(&cfg_path),
            "--ticks",
            "4",
            "--every",
            "4",
            "--width",
            "32",
            "--height",
            "32",
            "--controllers",
            "cpu",
            "--out",
            text(&diag_dir),
        ];
        args.extend_from_slice(extra_flags);
        let (status, diag_render, err) = flife(&args);
        assert_eq!(
            status,
            Some(0),
            "{label} diagnostic render `{tag}` failed: {err:?}"
        );
        let diag_end = find_record(&diag_render, "end");
        assert_eq!(diag_end["schema"], RENDER_SCHEMA);
        assert_eq!(diag_end["mapping"], expect_mapping);
        assert_eq!(diag_end["field_mapping"], expect_field_mapping);
        assert_eq!(diag_end["frames"], 2);
        assert_eq!(
            diag_end["digest"], tick4_digest,
            "{label}: diagnostic render `{tag}` must preserve tick-4 digest"
        );
        assert_png_frames_exist(&diag_render, 2, &format!("{label} render {tag}"));
    }
}

/// Validates both unmodified `field-ecology-v2` showcase presets (`Bundle A`
/// `108/40/44/33` and `Bundle B` `108/40/40/34`), then runs compact 64×64
/// copies through `run` -> `inspect --field` -> `resume` (checkpoint v3
/// bit-exactness), selective `--disable` toggles, and raw + V2 diagnostic
/// `flife render`.
#[test]
fn bundle_a_and_bundle_b_presets_validate_and_round_trip() {
    verify_unmodified_preset(
        "morphogenesis-tissue-v2.toml",
        BUNDLE_A_RAW,
        "morphogenesis-tissue-v2",
        false,
        (108, 40, 44, 33),
        "108/40/44/33",
        &BUNDLE_A_MODEL_FEATURES,
    );
    verify_unmodified_preset(
        "supercolony-v2.toml",
        BUNDLE_B_RAW,
        "supercolony-v2",
        true,
        (108, 40, 40, 34),
        "108/40/40/34",
        &BUNDLE_B_MODEL_FEATURES,
    );

    let dir = scratch("presets_round_trip");
    verify_preset_round_trip(
        &dir,
        "bundle_a",
        BUNDLE_A_RAW,
        false,
        &BUNDLE_A_MODEL_FEATURES,
        "pcp-polarity",
        &[
            (
                "cell_state",
                &["--color", "cell-state"],
                "cell-state",
                "energy",
            ),
            ("polarity", &["--color", "polarity"], "polarity", "energy"),
        ],
    );
    verify_preset_round_trip(
        &dir,
        "bundle_b",
        BUNDLE_B_RAW,
        true,
        &BUNDLE_B_MODEL_FEATURES,
        "vascular-field",
        &[
            (
                "trail_carrier",
                &["--field-mapping", "trail-carrier"],
                "lineage",
                "trail-carrier",
            ),
            (
                "vascular",
                &["--field-mapping", "vascular-conductance"],
                "lineage",
                "vascular-conductance",
            ),
        ],
    );
}

fn verify_all21_inspection(inspected: &[Value], expected_digest: &str) -> [f64; 2] {
    let header = find_record(inspected, "inspect");
    assert_eq!(header["schema"], INSPECT_SCHEMA);
    assert_eq!(header["model"], "field-ecology-v2");
    assert_eq!(header["digest"], expected_digest);
    assert_eq!(header["inv08_pure_field"], false);
    assert_eq!(json_strings(&header["v2_features"]).len(), 20);
    assert!(header["live"].as_u64().expect("live count") > 0);
    assert!(
        header["field_energy"]
            .as_f64()
            .is_some_and(|val| val.is_finite() && val >= 0.0)
    );
    assert!(
        header["organism_energy"]
            .as_f64()
            .is_some_and(|val| val.is_finite() && val > 0.0)
    );

    let org = inspected
        .iter()
        .find(|record| record["record"] == "organism" && record["held"].is_object())
        .expect("at least one evaluated organism at tick 4");
    let readiness = org["secretory_readiness"]
        .as_f64()
        .expect("secretory_readiness");
    assert!((0.0..=1.0).contains(&readiness));

    let pol = org["polarity"].as_array().expect("polarity");
    assert_eq!(pol.len(), 2);
    let pol_norm = pol[0]
        .as_f64()
        .expect("px")
        .hypot(pol[1].as_f64().expect("py"));
    assert!((pol_norm - 1.0).abs() <= 1.0e-5, "unit polarity: {pol:?}");
    let pol_strength = org["polarity_strength"]
        .as_f64()
        .expect("polarity_strength");
    assert!((0.0..=1.0).contains(&pol_strength));
    assert!(org["wedge_curvature"].as_f64().is_some_and(f64::is_finite));

    let epi = org["epigenetic_state"]
        .as_array()
        .expect("epigenetic_state");
    assert_eq!(epi.len(), 2);
    assert!(
        epi.iter()
            .all(|val| val.as_f64().is_some_and(|x| (-1.0..=1.0).contains(&x)))
    );
    assert_eq!(
        org["effective_affinity"]
            .as_array()
            .expect("effective_affinity")
            .len(),
        32
    );
    let neighbor_h0 = org["last_neighbor_h0"].as_f64().expect("last_neighbor_h0");
    assert!((-1.0..=1.0).contains(&neighbor_h0));

    let cad = org["cadherin_tag"].as_array().expect("cadherin_tag");
    assert_eq!(cad.len(), 4);
    let cad_norm_sq: f64 = cad
        .iter()
        .map(|entry| {
            let value = entry.as_f64().expect("cadherin component");
            value * value
        })
        .sum();
    assert!(
        (cad_norm_sq.sqrt() - 1.0).abs() <= 1.0e-5,
        "unit 4-D cadherin tag: {cad:?}"
    );
    let cohesiveness = org["cohesiveness"].as_f64().expect("cohesiveness");
    assert!((0.0..=1.0).contains(&cohesiveness));

    assert_eq!(
        org["held"]["outputs"]
            .as_array()
            .expect("held outputs")
            .len(),
        44
    );
    assert_eq!(
        org["held"]["recurrent_state"]
            .as_array()
            .expect("held recurrent_state")
            .len(),
        2
    );

    let field = find_record(inspected, "field");
    let energy = field["energy"].as_array().expect("field energy");
    let trail = field["trail_energy"].as_array().expect("trail_energy");
    let vascular = field["vascular_conductance"]
        .as_array()
        .expect("vascular_conductance");
    assert_eq!(energy.len(), 4096);
    assert_eq!(trail.len(), 4096);
    assert_eq!(vascular.len(), 4096);
    assert!(
        energy
            .iter()
            .all(|val| val.as_f64().is_some_and(|x| x.is_finite() && x >= 0.0))
    );
    assert!(
        trail
            .iter()
            .all(|val| val.as_f64().is_some_and(|x| x.is_finite() && x >= 0.0))
    );
    assert!(
        vascular
            .iter()
            .all(|val| val.as_f64().is_some_and(|x| x.is_finite() && x > 0.0))
    );

    let pos = org["position"].as_array().expect("organism position");
    [
        pos[0].as_f64().expect("pos x"),
        pos[1].as_f64().expect("pos y"),
    ]
}

fn verify_all21_branch_and_transplant(
    dir: &Path,
    parent_dir: &Path,
    host_cfg_path: &Path,
    region_center: [f64; 2],
    parent_t8_digest: &str,
) {
    // Control branch resumes to exact parent tick-8 digest.
    let ctrl_dir = dir.join("branch_ctrl");
    let (status, ctrl_branch, err) = flife(&[
        "branch",
        text(parent_dir),
        text(&ctrl_dir),
        "--at",
        "4",
        "--name",
        "all21-ctrl",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "all21 control branch failed: {err:?}");
    assert_eq!(find_record(&ctrl_branch, "branch")["schema"], RUN_SCHEMA);

    let (status, ctrl_resumed, err) = flife(&[
        "resume",
        text(&ctrl_dir),
        "--until",
        "8",
        "--every",
        "0",
        "--controllers",
        "cpu",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "all21 control resume failed: {err:?}");
    let (ctrl_digest, _, _) =
        assert_healthy_end(find_record(&ctrl_resumed, "end"), 8, "all21 ctrl resume");
    assert_eq!(
        ctrl_digest, parent_t8_digest,
        "all21 control branch must reproduce parent tick-8 digest bit-for-bit"
    );

    // Intervention branch with energy brush at tick 5 diverges cleanly.
    let int_dir = dir.join("branch_inject");
    let (status, int_branch, err) = flife(&[
        "branch",
        text(parent_dir),
        text(&int_dir),
        "--at",
        "4",
        "--name",
        "all21-inject",
        "--inject",
        "5:8,8:50.0",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "all21 intervention branch failed: {err:?}");
    assert_eq!(find_record(&int_branch, "branch")["schema"], RUN_SCHEMA);

    let (status, int_resumed, err) = flife(&[
        "resume",
        text(&int_dir),
        "--until",
        "8",
        "--every",
        "0",
        "--controllers",
        "cpu",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "all21 intervention resume failed: {err:?}");
    let (int_digest, _, _) = assert_healthy_end(
        find_record(&int_resumed, "end"),
        8,
        "all21 intervention resume",
    );
    assert_ne!(
        int_digest, parent_t8_digest,
        "all21 intervention branch must diverge from control after tick-5 brush"
    );

    // Discover region around an inspected live organism -> transplant into
    // a different-seed all-21 host -> resume cleanly.
    let bundle_dir = dir.join("discovery_bundle");
    let region_arg = format!("{:.4},{:.4},3.0", region_center[0], region_center[1]);
    let (status, disc_out, err) = flife(&[
        "discover",
        text(parent_dir),
        text(&bundle_dir),
        "--at",
        "4",
        "--region",
        &region_arg,
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
    assert_eq!(status, Some(0), "all21 discover --region failed: {err:?}");
    let disc = find_record(&disc_out, "discovery");
    assert_eq!(disc["schema"], DISCOVERY_SCHEMA);
    let package_dir = bundle_dir.join("package");
    assert!(
        package_dir.is_dir(),
        "discover must emit package/ directory"
    );
    let thumb_bytes = fs::read(bundle_dir.join("thumbnail.png")).expect("reads discovery thumb");
    assert!(thumb_bytes.starts_with(&PNG_MAGIC));

    let graft_dir = dir.join("transplant_run");
    let (status, graft_out, err) = flife(&[
        "transplant",
        text(&package_dir),
        text(&graft_dir),
        "--config",
        text(host_cfg_path),
        "--enable",
        "all-v2",
        "--at",
        "4.0,12.0",
        "--name",
        "all21-graft",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "all21 transplant failed: {err:?}");
    let graft_rec = find_record(&graft_out, "transplant");
    assert_eq!(graft_rec["schema"], RUN_SCHEMA);
    assert!(
        graft_rec["graft"]["organisms"]
            .as_u64()
            .expect("grafted organisms")
            >= 1,
        "region centered on a live organism must graft >= 1 organism: {graft_rec:?}"
    );
    assert!(graft_rec["graft"]["cells"].as_u64().expect("grafted cells") > 0);

    let (status, graft_resumed, err) = flife(&[
        "resume",
        text(&graft_dir),
        "--until",
        "4",
        "--every",
        "0",
        "--controllers",
        "cpu",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "all21 transplant resume failed: {err:?}");
    assert_healthy_end(
        find_record(&graft_resumed, "end"),
        4,
        "all21 transplant resume",
    );
}

/// Composes ALL 21 `V2Feature` slugs simultaneously (`112/44/48/34` exact
/// 4-aligned ABI, 0 Metal F16 alignment deferrals) and proves:
/// - `INV-01..15` energy-ledger health;
/// - `--cpu-workers 1` vs `--cpu-workers 4` bit-exact parity;
/// - `--controllers cpu` vs `--controllers torch` (and `--controllers metal`
///   with `deferred == 0` when Metal is present) bit-exact parity;
/// - `C.7`/`C.8` observer & render presentation independence;
/// - checkpoint v3 `resume`, `inspect --field`, `branch` (control &
///   intervention), and `discover --region` -> `transplant` -> `resume`.
#[test]
fn e2e_all_21_v2_features_composed_simultaneously() {
    let dir = scratch("all_21_composed");
    let all21_toml = scale_preset_64(
        BUNDLE_A_RAW,
        32,
        &[
            (
                "v2.emission.offset",
                "pump_direction_mode",
                "\"neural-output-v1\"",
            ),
            ("execution", "command_durability", "\"volatile-in-memory\""),
        ],
    );
    let all21_durable_toml = scale_preset_64(
        BUNDLE_A_RAW,
        32,
        &[
            (
                "v2.emission.offset",
                "pump_direction_mode",
                "\"neural-output-v1\"",
            ),
            (
                "execution",
                "command_durability",
                "\"write-ahead-before-apply\"",
            ),
        ],
    );
    let host_toml = scale_preset_64(
        BUNDLE_A_RAW,
        32,
        &[
            ("", "seed", "99"),
            (
                "v2.emission.offset",
                "pump_direction_mode",
                "\"neural-output-v1\"",
            ),
            (
                "execution",
                "command_durability",
                "\"write-ahead-before-apply\"",
            ),
        ],
    );
    let cfg_path = dir.join("all21.toml");
    let durable_cfg_path = dir.join("all21_durable.toml");
    let host_path = dir.join("all21_host.toml");
    fs::write(&cfg_path, &all21_toml).expect("writes all21 config");
    fs::write(&durable_cfg_path, &all21_durable_toml).expect("writes all21 durable config");
    fs::write(&host_path, &host_toml).expect("writes all21 host config");

    // 1. Exact 4-aligned 112/44/48/34 ABI with all 21 V2Feature slugs active.
    let validated = ValidatedConfig::from_toml_with_overrides(
        &all21_toml,
        &[FeatureOverride::Enable("all-v2".to_owned())],
    )
    .expect("all-21 config validates");
    for feature in V2Feature::ALL {
        assert!(
            validated.is_v2_feature_enabled(feature),
            "feature `{}` must be enabled in all-21 config",
            feature.slug()
        );
    }
    let abi = validated.v2_abi();
    assert_eq!(abi.sensor_width, 110);
    assert_eq!(abi.recurrent_state_width, 2);
    assert_eq!(abi.input_width, 112);
    assert_eq!(abi.action_width, 44);
    assert_eq!(abi.raw_output_width, 48);
    assert_eq!(abi.field_planes, 34);
    assert!(abi.input_width.is_multiple_of(4) && abi.raw_output_width.is_multiple_of(4));
    assert_eq!(abi.descriptor_tag(), "112/44/48/34");
    assert!(
        abi.dual_carrier_sensors
            && abi.hippo_crowding_sensor
            && abi.notch_delta_sensor
            && abi.so2_motor_actions
            && abi.so2_budding_actions
            && abi.emt_gate_action
            && abi.wake_pump_action
    );

    let (status, val_out, err) = flife(&["validate", text(&cfg_path), "--enable", "all-v2"]);
    assert_eq!(status, Some(0), "all-21 validate failed: {err:?}");
    assert_eq!(val_out[0]["schema"], VALIDATE_SCHEMA);
    assert_eq!(val_out[0]["model"], "field-ecology-v2");
    assert_eq!(val_out[0]["inv08_pure_field"], false);
    assert_eq!(json_strings(&val_out[0]["v2_features"]).len(), 20);

    // 2. Reference CPU-1 baseline vs CPU-4 worker parity.
    let (status, cpu1_out, err) = flife(&[
        "run",
        text(&cfg_path),
        "--enable",
        "all-v2",
        "--ticks",
        "8",
        "--every",
        "0",
        "--cpu-workers",
        "1",
        "--controllers",
        "cpu",
    ]);
    assert_eq!(status, Some(0), "all-21 cpu w=1 failed: {err:?}");
    let (digest_t8, _, _) = assert_healthy_end(find_record(&cpu1_out, "end"), 8, "all-21 cpu w=1");

    let (status, cpu4_out, err) = flife(&[
        "run",
        text(&cfg_path),
        "--enable",
        "all-v2",
        "--ticks",
        "8",
        "--every",
        "0",
        "--cpu-workers",
        "4",
        "--controllers",
        "cpu",
    ]);
    assert_eq!(status, Some(0), "all-21 cpu w=4 failed: {err:?}");
    let (digest_w4, _, _) = assert_healthy_end(find_record(&cpu4_out, "end"), 8, "all-21 cpu w=4");
    assert_eq!(
        digest_w4, digest_t8,
        "all-21 --cpu-workers 4 must match --cpu-workers 1 bit-for-bit"
    );

    // 3. Batched controller parity: FrankenTorch CPU (always) + Metal (when available).
    let (status, torch_out, err) = flife(&[
        "run",
        text(&cfg_path),
        "--enable",
        "all-v2",
        "--ticks",
        "8",
        "--every",
        "0",
        "--cpu-workers",
        "1",
        "--controllers",
        "torch",
    ]);
    assert_eq!(status, Some(0), "all-21 torch run failed: {err:?}");
    let torch_end = find_record(&torch_out, "end");
    let (digest_torch, _, _) = assert_healthy_end(torch_end, 8, "all-21 torch");
    assert_eq!(
        digest_torch, digest_t8,
        "all-21 --controllers torch must match cpu-reference bit-for-bit"
    );
    assert!(
        torch_end["controllers"]["batched"]
            .as_u64()
            .expect("torch batched")
            > 0
    );
    assert_eq!(torch_end["controllers"]["deferred"], 0);

    if metal_available() {
        let (status, metal_out, err) = flife(&[
            "run",
            text(&cfg_path),
            "--enable",
            "all-v2",
            "--ticks",
            "8",
            "--every",
            "0",
            "--cpu-workers",
            "1",
            "--controllers",
            "metal",
        ]);
        assert_eq!(status, Some(0), "all-21 metal run failed: {err:?}");
        let metal_end = find_record(&metal_out, "end");
        let (digest_metal, _, _) = assert_healthy_end(metal_end, 8, "all-21 metal");
        assert_eq!(
            digest_metal, digest_t8,
            "all-21 --controllers metal must match cpu-reference bit-for-bit"
        );
        assert!(
            metal_end["controllers"]["batched"]
                .as_u64()
                .expect("metal batched")
                > 0
        );
        assert_eq!(
            metal_end["controllers"]["deferred"], 0,
            "4-aligned 112/48 ABI must incur zero Metal F16 alignment deferrals"
        );
    }

    // 4. C.7 / C.8 presentation & observer independence: metrics + render.
    let (status, metrics_out, err) = flife(&[
        "run",
        text(&cfg_path),
        "--enable",
        "all-v2",
        "--ticks",
        "8",
        "--every",
        "2",
        "--metrics-every",
        "4",
        "--cpu-workers",
        "1",
        "--controllers",
        "cpu",
    ]);
    assert_eq!(status, Some(0), "all-21 metrics run failed: {err:?}");
    let metrics_records: Vec<&Value> = metrics_out
        .iter()
        .filter(|record| record["record"] == "metrics")
        .collect();
    assert!(metrics_records.len() >= 2);
    assert_eq!(metrics_records[0]["schema"], RUN_SCHEMA);
    assert_eq!(
        find_record(&metrics_out, "start")["metrics"]["schema"],
        METRICS_SCHEMA
    );
    let (digest_metrics, _, _) =
        assert_healthy_end(find_record(&metrics_out, "end"), 8, "all-21 metrics");
    assert_eq!(
        digest_metrics, digest_t8,
        "C.8 observer metrics/progress must not alter scientific state"
    );

    let render_dir = dir.join("render_all21");
    let (status, render_out, err) = flife(&[
        "render",
        text(&cfg_path),
        "--enable",
        "all-v2",
        "--ticks",
        "8",
        "--every",
        "4",
        "--width",
        "32",
        "--height",
        "32",
        "--controllers",
        "cpu",
        "--color",
        "polarity",
        "--field-mapping",
        "vascular-conductance",
        "--out",
        text(&render_dir),
    ]);
    assert_eq!(status, Some(0), "all-21 render failed: {err:?}");
    let render_end = find_record(&render_out, "end");
    assert_eq!(render_end["schema"], RENDER_SCHEMA);
    assert_eq!(render_end["mapping"], "polarity");
    assert_eq!(render_end["field_mapping"], "vascular-conductance");
    assert_eq!(
        render_end["digest"], digest_t8,
        "C.7 V2 render must not alter scientific state"
    );
    assert_png_frames_exist(&render_out, 3, "all-21 render");

    // 5. Checkpointed tick-4 run -> inspect --field -> branch + discover/transplant -> resume to 8.
    let parent_dir = dir.join("parent_run");
    let (status, leg1_out, err) = flife(&[
        "run",
        text(&durable_cfg_path),
        "--enable",
        "all-v2",
        "--ticks",
        "4",
        "--every",
        "0",
        "--cpu-workers",
        "1",
        "--controllers",
        "cpu",
        "--run-dir",
        text(&parent_dir),
        "--checkpoint-every",
        "4",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "all-21 parent run failed: {err:?}");
    let (digest_t4, _, _) =
        assert_healthy_end(find_record(&leg1_out, "end"), 4, "all-21 parent t4");

    let (status, inspected, err) = flife(&["inspect", text(&parent_dir), "--field"]);
    assert_eq!(status, Some(0), "all-21 inspect failed: {err:?}");
    let region_center = verify_all21_inspection(&inspected, &digest_t4);

    verify_all21_branch_and_transplant(&dir, &parent_dir, &host_path, region_center, &digest_t8);

    let (status, resumed_out, err) = flife(&[
        "resume",
        text(&parent_dir),
        "--until",
        "8",
        "--every",
        "0",
        "--cpu-workers",
        "1",
        "--controllers",
        "cpu",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "all-21 parent resume failed: {err:?}");
    let (resumed_digest, _, _) =
        assert_healthy_end(find_record(&resumed_out, "end"), 8, "all-21 parent resume");
    assert_eq!(
        resumed_digest, digest_t8,
        "all-21 checkpoint v3 resume must match uninterrupted 8-tick run bit-for-bit"
    );
}

fn expected_leave_one_out_abi(disabled: V2Feature) -> (u32, u32, u32, u32) {
    match disabled {
        V2Feature::DualCarrierTrails => (108, 44, 48, 33),
        V2Feature::HippoCrowding | V2Feature::NotchDelta => (111, 44, 48, 34),
        V2Feature::EpigeneticRecurrence => (109, 44, 44, 34),
        V2Feature::So2Motor => (112, 40, 44, 34),
        V2Feature::So2Budding => (112, 42, 46, 34),
        V2Feature::EmtGate | V2Feature::WakeOffset => (112, 43, 47, 34),
        _ => (112, 44, 48, 34),
    }
}

fn assert_abi_tuple(abi: V2Abi, expected: (u32, u32, u32, u32), context: &str) {
    assert_eq!(
        (
            abi.input_width,
            abi.action_width,
            abi.raw_output_width,
            abi.field_planes,
        ),
        expected,
        "{context}: unexpected V2Abi {abi:?}"
    );
}

/// Exhaustive leave-one-out `--disable <slug>` sweep across EVERY one of the
/// 21 `V2Feature::ALL` entries starting from the compact all-21 configuration:
/// - `epigenetic-recurrence` alone is explicitly rejected (`notch-delta`
///   dependency), and passes cleanly when paired with `--disable notch-delta`;
/// - every other slug validates and executes cleanly, returning structured
///   `validate` + `run` JSON telemetry (`active_features`, exact `V2Abi`,
///   64-hex `digest`, `live > 0`, bounded `INV-03` residual);
/// - disabling `v2-visual-mappings` (`Scope::Presentation`) leaves
///   `ModelDescriptor::digest`, `v2_features`, and `digest` byte-identical
///   (`INV-09`) while materializing `v2.visual.enabled = false` in `config_hash`.
#[test]
fn e2e_leave_one_out_disable_all_21_v2_features() {
    let dir = scratch("leave_one_out_21");
    let toml = scale_preset_32(
        BUNDLE_A_RAW,
        24,
        &[(
            "v2.emission.offset",
            "pump_direction_mode",
            "\"neural-output-v1\"",
        )],
    );
    let cfg_path = dir.join("all21_32.toml");
    fs::write(&cfg_path, &toml).expect("writes 32x32 all21 config");

    // All-21 baseline validate + 4-tick run.
    let base_validated = ValidatedConfig::from_toml_with_overrides(
        &toml,
        &[FeatureOverride::Enable("all-v2".to_owned())],
    )
    .expect("baseline all-21 validates");
    let base_descriptor = ModelDescriptor::of(&base_validated).digest();
    let (status, base_val, err) = flife(&["validate", text(&cfg_path), "--enable", "all-v2"]);
    assert_eq!(status, Some(0), "baseline all-21 validate failed: {err:?}");
    let base_hash = base_val[0]["config_hash"]
        .as_str()
        .expect("baseline hash")
        .to_owned();
    let base_features = json_strings(&base_val[0]["v2_features"]);
    assert_eq!(base_features.len(), 20);

    let (status, base_run, err) = flife(&[
        "run",
        text(&cfg_path),
        "--enable",
        "all-v2",
        "--ticks",
        "4",
        "--every",
        "0",
        "--controllers",
        "cpu",
    ]);
    assert_eq!(status, Some(0), "baseline all-21 run failed: {err:?}");
    let (base_digest, _, _) =
        assert_healthy_end(find_record(&base_run, "end"), 4, "all-21 32x32 baseline");

    let mut checked = 0_usize;
    for feature in V2Feature::ALL {
        let slug = feature.slug();
        checked += 1;

        if feature == V2Feature::EpigeneticRecurrence {
            // 1. Disabling `epigenetic-recurrence` alone MUST be rejected because
            //    `notch-delta` depends on `epigenetic-recurrence` (`state_width >= 1`).
            let (status, _, err) = flife(&[
                "validate",
                text(&cfg_path),
                "--enable",
                "all-v2",
                "--disable",
                slug,
            ]);
            assert_eq!(
                status,
                Some(2),
                "disabling `{slug}` alone must exit 2: {err:?}"
            );
            assert_eq!(err[0]["schema"], ERROR_SCHEMA);
            let detail = err[0]["detail"].as_str().expect("error detail");
            assert!(
                detail.contains("notch-delta") && detail.contains("epigenetic-recurrence"),
                "rejection must cite notch-delta -> epigenetic-recurrence dependency: {detail}"
            );

            let (run_status, _, run_err) = flife(&[
                "run",
                text(&cfg_path),
                "--enable",
                "all-v2",
                "--disable",
                slug,
                "--ticks",
                "4",
                "--every",
                "0",
                "--controllers",
                "cpu",
            ]);
            assert_eq!(run_status, Some(2), "run must also reject `{slug}` alone");
            assert_eq!(run_err[0]["schema"], ERROR_SCHEMA);

            // 2. Paired `--disable epigenetic-recurrence --disable notch-delta` succeeds cleanly.
            let validated = ValidatedConfig::from_toml_with_overrides(
                &toml,
                &[
                    FeatureOverride::Enable("all-v2".to_owned()),
                    FeatureOverride::Disable(slug.to_owned()),
                    FeatureOverride::Disable(V2Feature::NotchDelta.slug().to_owned()),
                ],
            )
            .expect("paired disable validates");
            assert!(!validated.is_v2_feature_enabled(V2Feature::EpigeneticRecurrence));
            assert!(!validated.is_v2_feature_enabled(V2Feature::NotchDelta));
            assert_abi_tuple(
                validated.v2_abi(),
                expected_leave_one_out_abi(feature),
                "disable epigenetic-recurrence+notch-delta",
            );

            let (status, val, err) = flife(&[
                "validate",
                text(&cfg_path),
                "--enable",
                "all-v2",
                "--disable",
                slug,
                "--disable",
                V2Feature::NotchDelta.slug(),
            ]);
            assert_eq!(status, Some(0), "paired disable validate failed: {err:?}");
            let expected_active: Vec<String> = base_features
                .iter()
                .filter(|item| {
                    item.as_str() != slug && item.as_str() != V2Feature::NotchDelta.slug()
                })
                .cloned()
                .collect();
            assert_eq!(json_strings(&val[0]["v2_features"]), expected_active);
            assert_ne!(val[0]["config_hash"], base_hash);

            let (status, run_out, err) = flife(&[
                "run",
                text(&cfg_path),
                "--enable",
                "all-v2",
                "--disable",
                slug,
                "--disable",
                V2Feature::NotchDelta.slug(),
                "--ticks",
                "4",
                "--every",
                "0",
                "--controllers",
                "cpu",
            ]);
            assert_eq!(status, Some(0), "paired disable run failed: {err:?}");
            assert_healthy_end(
                find_record(&run_out, "end"),
                4,
                "disable epigenetic-recurrence+notch-delta",
            );
            continue;
        }

        // Every other slug (20 slugs) must disable cleanly on its own.
        let validated = ValidatedConfig::from_toml_with_overrides(
            &toml,
            &[
                FeatureOverride::Enable("all-v2".to_owned()),
                FeatureOverride::Disable(slug.to_owned()),
            ],
        )
        .unwrap_or_else(|err| panic!("leave-one-out `--disable {slug}` must validate: {err}"));
        assert!(
            !validated.is_v2_feature_enabled(feature),
            "`{slug}` must be disabled"
        );
        for other in V2Feature::ALL {
            if other != feature {
                assert!(
                    validated.is_v2_feature_enabled(other),
                    "disabling `{slug}` must leave `{}` enabled",
                    other.slug()
                );
            }
        }
        assert_abi_tuple(
            validated.v2_abi(),
            expected_leave_one_out_abi(feature),
            &format!("leave-one-out --disable {slug}"),
        );

        let (status, val, err) = flife(&[
            "validate",
            text(&cfg_path),
            "--enable",
            "all-v2",
            "--disable",
            slug,
        ]);
        assert_eq!(
            status,
            Some(0),
            "leave-one-out validate `--disable {slug}` failed: {err:?}"
        );
        let active = json_strings(&val[0]["v2_features"]);
        let descriptor = ModelDescriptor::of(&validated).digest();
        assert_ne!(
            val[0]["config_hash"], base_hash,
            "materializing `--disable {slug}` must change config_hash"
        );
        if feature.scope() == Scope::Presentation {
            assert_eq!(active, base_features);
            assert_eq!(
                descriptor, base_descriptor,
                "INV-09: disabling presentation feature `{slug}` must not alter ModelDescriptor"
            );
        } else {
            let expected_active: Vec<String> = base_features
                .iter()
                .filter(|item| item.as_str() != slug)
                .cloned()
                .collect();
            assert_eq!(active, expected_active, "active features without `{slug}`");
            assert_ne!(
                descriptor, base_descriptor,
                "disabling scientific feature `{slug}` must alter ModelDescriptor"
            );
        }

        let (status, run_out, err) = flife(&[
            "run",
            text(&cfg_path),
            "--enable",
            "all-v2",
            "--disable",
            slug,
            "--ticks",
            "4",
            "--every",
            "0",
            "--controllers",
            "cpu",
        ]);
        assert_eq!(
            status,
            Some(0),
            "leave-one-out run `--disable {slug}` failed: {err:?}"
        );
        let (digest, _, _) = assert_healthy_end(
            find_record(&run_out, "end"),
            4,
            &format!("leave-one-out --disable {slug}"),
        );
        if feature.scope() == Scope::Presentation {
            assert_eq!(
                digest, base_digest,
                "INV-09: disabling `{slug}` must not alter world digest"
            );
        }
    }

    assert_eq!(
        checked,
        V2Feature::ALL.len(),
        "leave-one-out sweep must cover all 21 V2Feature::ALL entries"
    );
}
