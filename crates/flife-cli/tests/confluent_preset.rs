//! `presets/confluent-world.toml`, the world the website's film shows, is
//! `presets/contact-world.toml` on a field four times finer, fed to its
//! ceiling, with scattered offspring, chosen by trials scored against image
//! statistics of the reference screenshots (experiments/showcase/README.md,
//! "Stage E", states the selection). These tests keep it exactly what the
//! trials chose: the contact preset with its declared changes, each named in
//! its header; and valid, with its session admitting it.

mod preset_diff;

use flife_cli::{RUN_SESSION, VALIDATE_SCHEMA, Validation};
use flife_core::ValidatedConfig;
use flife_runtime::SessionConfig;
use preset_diff::assert_exactly_declared;

const PRESET: &str = include_str!("../../../presets/confluent-world.toml");
const CONTACT: &str = include_str!("../../../presets/contact-world.toml");

/// The preset's declared changes from the contact preset: the trial
/// `w80-o25` of stage E.
const DECLARED: [(&str, &str, &str); 22] = [
    ("name", "\"contact-world\"", "\"confluent-world\""),
    ("world.width", "128.0", "80.0"),
    ("world.height", "128.0", "80.0"),
    ("world.grid_width", "128", "320"),
    ("world.grid_height", "128", "320"),
    ("world.diffusion", "0.477947", "0.0298717"),
    ("population.maximum_count", "177536", "262144"),
    ("sources.count", "64", "25"),
    ("sources.power_per_source", "4941.03", "37970.7"),
    ("sources.sigma_world_units", "7.48417", "14.9683"),
    ("chemistry.motion_response_norm_limit", "100.46", "6.27875"),
    ("chemistry.maximum_acceleration", "19.8961", "4.97403"),
    ("chemistry.maximum_speed", "13.7045", "3.42612"),
    ("sensors.gradient_length_scale_world_units", "1.0", "0.25"),
    ("sensors.velocity_scale", "4.0", "1.0"),
    ("sensors.initial_scale_world_units", "1.0", "0.25"),
    ("sensors.minimum_scale_world_units", "1.0", "0.25"),
    ("sensors.maximum_scale_world_units", "16.0", "4.0"),
    ("costs.sensor_unit_scale_world_units", "1.0", "0.25"),
    (
        "costs.motion_energy_per_speed_squared_second",
        "0.000451687",
        "0.00722699",
    ),
    ("reproduction.child_position_offset_radius", "0.45", "2.5"),
    ("contact.diameter", "0.45", "0.25"),
];

/// The preset is the contact preset with exactly its declared changes, and
/// its header declares each one.
#[test]
fn the_preset_is_the_contact_preset_with_exactly_its_declared_changes() {
    assert_exactly_declared(CONTACT, PRESET, &DECLARED);
}

/// The field's cell is the contact preset's divided by four, and so is
/// every speed, acceleration and sensing scale, to the six significant
/// figures the presets carry. Diffusion and the motion-response limit are
/// divided by sixteen, and the motion cost per speed squared multiplied by
/// sixteen, so a move of one cell costs what it did. The body and the child
/// placement are not scaled: stage E chose them by trial.
#[test]
fn the_finer_scale_is_one_factor_of_four() {
    let contact: toml::Table = toml::from_str(CONTACT).unwrap_or_default();
    let preset: toml::Table = toml::from_str(PRESET).unwrap_or_default();
    let get = |table: &toml::Table, section: &str, key: &str| {
        table
            .get(section)
            .and_then(|inner| inner.get(key))
            .and_then(toml::Value::as_float)
            .unwrap_or(f64::NAN)
    };
    for (section, key, factor) in [
        ("world", "diffusion", 1.0 / 16.0),
        ("chemistry", "motion_response_norm_limit", 1.0 / 16.0),
        ("chemistry", "maximum_acceleration", 0.25),
        ("chemistry", "maximum_speed", 0.25),
        ("sensors", "gradient_length_scale_world_units", 0.25),
        ("sensors", "velocity_scale", 0.25),
        ("sensors", "initial_scale_world_units", 0.25),
        ("sensors", "minimum_scale_world_units", 0.25),
        ("sensors", "maximum_scale_world_units", 0.25),
        ("costs", "sensor_unit_scale_world_units", 0.25),
        ("costs", "motion_energy_per_speed_squared_second", 16.0),
    ] {
        let (before, after) = (get(&contact, section, key), get(&preset, section, key));
        assert!(
            (after - before * factor).abs() <= 5e-6 * after.abs(),
            "{section}.{key}: {after} is not {before} x {factor}"
        );
    }
    let cells = |table: &toml::Table| {
        get(table, "world", "width") / {
            table
                .get("world")
                .and_then(|inner| inner.get("grid_width"))
                .and_then(toml::Value::as_integer)
                .map_or(f64::NAN, |n| n as f64)
        }
    };
    assert!((cells(&contact) / cells(&preset) - 4.0).abs() < 1e-12);
}

/// The preset validates under the contact model, its worst-case genomes fit
/// its quota, and its session admits it.
#[test]
fn the_preset_validates_and_its_session_admits_it() {
    let config = ValidatedConfig::from_toml(PRESET);
    assert!(config.is_ok(), "{config:?}");
    let Ok(config) = config else { return };
    let summary = Validation::of(&config);
    assert_eq!(summary.schema, VALIDATE_SCHEMA);
    assert_eq!(summary.name, "confluent-world");
    assert_eq!(summary.model, "field-ecology-v1-contact");
    assert_eq!(summary.seed, 42);
    assert_eq!(summary.grid, [320, 320]);
    assert_eq!(summary.founders, 10_954);
    assert_eq!(summary.maximum_population, 262_144);
    assert_eq!(summary.timestep, [1, 32]);
    assert!(summary.worst_case_fits_logical_quota);
    assert!(SessionConfig::from_config(RUN_SESSION, &config).is_ok());
    assert_eq!(
        flife_engine::contact::body_radius(&config),
        Some(0.125),
        "drawn at its body's radius"
    );
}
