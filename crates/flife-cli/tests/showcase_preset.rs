//! `presets/showcase-world.toml` was the world the website's film showed
//! until `presets/colony-world.toml`: a
//! parameter set an outer search chose for organism-like structure
//! (experiments/showcase/README.md states the selection). These tests keep it
//! exactly what the search chose: the Desktop preset with its declared
//! changes, each named in its header; the search's sample s1000 on 4× its
//! area; and valid, with its session admitting it.

mod preset_diff;

use flife_cli::{RUN_SESSION, VALIDATE_SCHEMA, Validation};
use flife_core::ValidatedConfig;
use flife_runtime::SessionConfig;
use preset_diff::assert_exactly_declared;

const PRESET: &str = include_str!("../../../presets/showcase-world.toml");
const DESKTOP: &str = include_str!("../../../presets/desktop-world.toml");
const SAMPLE: &str = include_str!("../../../experiments/showcase/s1000.json");

/// The preset's declared changes from the Desktop preset, as TOML values:
/// `(key, the Desktop preset's value, the preset's value)`.
const DECLARED: [(&str, &str, &str); 36] = [
    ("name", "\"desktop-world\"", "\"showcase-world\""),
    ("world.width", "512.0", "256.0"),
    ("world.height", "512.0", "256.0"),
    ("world.grid_width", "512", "256"),
    ("world.grid_height", "512", "256"),
    ("world.tick_denominator", "128", "32"),
    ("world.diffusion", "0.15", "0.414856"),
    ("world.decay_per_second", "0.01", "0.0131865"),
    ("population.initial_count", "100000", "43816"),
    ("population.maximum_count", "160000", "240000"),
    ("sources.count", "128", "64"),
    ("sources.power_per_source", "1248.0", "3721.77"),
    ("sources.sigma_world_units", "10.0", "9.95882"),
    ("sources.path_speed", "1.0", "5.3505"),
    ("chemistry.maximum_efficiency", "0.8", "0.698545"),
    ("chemistry.affinity_gain", "4.0", "10.9355"),
    ("chemistry.discrimination_threshold", "0.15", "0.386861"),
    ("chemistry.uptake_capacity_per_second", "6.0", "2.6027"),
    (
        "chemistry.uptake_half_saturation_density",
        "0.1",
        "0.923996",
    ),
    ("chemistry.toxicity_sensitivity", "0.2", "0.0959457"),
    ("chemistry.motion_response_norm_limit", "256.0", "80.0496"),
    ("chemistry.maximum_acceleration", "4.0", "28.4759"),
    ("chemistry.drag_per_second", "2.0", "0.691841"),
    ("chemistry.maximum_speed", "4.0", "8.49133"),
    ("costs.basal_energy_per_second", "0.8", "0.81893"),
    (
        "costs.motion_energy_per_speed_squared_second",
        "0.002",
        "0.000864311",
    ),
    ("emission.gate_threshold", "0.5", "0.468461"),
    (
        "emission.maximum_fractional_rate_per_second",
        "0.25",
        "0.205339",
    ),
    ("emission.transfer_efficiency", "0.9", "0.630672"),
    (
        "reproduction.maximum_attempt_rate_per_second",
        "1.0",
        "1.61554",
    ),
    (
        "reproduction.minimum_parent_energy_before_attempt",
        "3.0",
        "2.26573",
    ),
    ("reproduction.cooldown_seconds", "0.5", "0.090866"),
    ("death.residual_energy_recycle_fraction", "0.5", "0.540951"),
    (
        "execution.maximum_logical_genome_bytes",
        "4294967296",
        "6442450944",
    ),
    (
        "execution.maximum_allocated_genome_bytes",
        "4831838208",
        "7247757312",
    ),
    (
        "execution.maximum_total_bytes",
        "17179869184",
        "34359738368",
    ),
];

/// The preset is the Desktop preset with exactly its declared changes, and
/// its header declares each one.
#[test]
fn the_preset_is_the_desktop_preset_with_exactly_its_declared_changes() {
    assert_exactly_declared(DESKTOP, PRESET, &DECLARED);
}

/// The preset is the search's sample s1000 (experiments/showcase/s1000.json,
/// its parameters as the search drew them) on 4× its area: every parameter
/// to the 6 significant figures the search's configuration carried, and 4×
/// its sources and founders, so the same source density and organism
/// density. A number that moved would be a world neither the search nor its
/// held-out seeds ran.
#[test]
fn the_preset_is_sample_s1000_on_four_times_its_area() {
    let sample: serde_json::Value = serde_json::from_str(SAMPLE).unwrap_or_default();
    let preset: toml::Table = toml::from_str(PRESET).unwrap_or_default();
    let Some(params) = sample.get("params").and_then(serde_json::Value::as_object) else {
        panic!("s1000.json has no params");
    };
    assert_eq!(params.len(), 28);
    for (flat, drawn) in params {
        let (section, key) = flat.split_once('.').unwrap_or_default();
        let preset_value = if section == "model" {
            preset.get("model")
        } else {
            preset.get(section).and_then(|table| table.get(key))
        };
        let Some(preset_value) = preset_value else {
            panic!("the preset has no {flat}");
        };
        match flat.as_str() {
            "model.model" => assert_eq!(preset_value.as_str(), drawn.as_str()),
            "sources.count" | "population.initial_count" => {
                assert_eq!(
                    preset_value.as_integer(),
                    drawn.as_i64().map(|count| 4 * count),
                    "{flat}"
                );
            }
            // The ceiling is the preset's own: no run reached either.
            "population.maximum_count" => {}
            _ => {
                let (Some(ours), Some(theirs)) = (preset_value.as_float(), drawn.as_f64()) else {
                    panic!("{flat}: {preset_value:?} or {drawn:?} is not a number");
                };
                assert!(
                    (ours - theirs).abs() <= 5e-6 * theirs.abs(),
                    "{flat}: the preset has {ours}, the search drew {theirs}"
                );
            }
        }
    }
}

/// The preset validates and its session admits it.
#[test]
fn the_preset_validates_and_its_session_admits_it() {
    let config = ValidatedConfig::from_toml(PRESET);
    assert!(config.is_ok(), "{config:?}");
    let Ok(config) = config else { return };
    let summary = Validation::of(&config);
    assert_eq!(summary.schema, VALIDATE_SCHEMA);
    assert_eq!(summary.name, "showcase-world");
    assert_eq!(summary.model, "field-ecology-v1");
    assert_eq!(summary.seed, 42);
    assert_eq!(summary.grid, [256, 256]);
    assert_eq!(summary.founders, 43_816);
    assert_eq!(summary.maximum_population, 240_000);
    assert_eq!(summary.timestep, [1, 32]);
    assert_eq!(summary.default_parameters, 9988);
    // F16 canonical weights: two bytes a parameter.
    assert_eq!(summary.initial_genome_bytes, 43_816 * 9988 * 2);
    assert!(SessionConfig::from_config(RUN_SESSION, &config).is_ok());
}
