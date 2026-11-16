//! `presets/tissue-world.toml` is the world the website's film shows: a
//! parameter set an outer search chose for organism-like structure, scored
//! against image statistics of the reference screenshots
//! (experiments/showcase/README.md, "Stage W", states the selection). These
//! tests keep it exactly what the search chose: the Desktop preset with its
//! declared changes, each named in its header; the search's sample c4-x1 at
//! its own scale; and valid, with its session admitting it.

mod preset_diff;

use flife_cli::{RUN_SESSION, VALIDATE_SCHEMA, Validation};
use flife_core::ValidatedConfig;
use flife_runtime::SessionConfig;
use preset_diff::assert_exactly_declared;

const PRESET: &str = include_str!("../../../presets/tissue-world.toml");
const DESKTOP: &str = include_str!("../../../presets/desktop-world.toml");
const SAMPLE: &str = include_str!("../../../experiments/showcase/tissue.json");

/// The preset's declared changes from the Desktop preset, as TOML values:
/// `(key, the Desktop preset's value, the preset's value)`.
const DECLARED: [(&str, &str, &str); 38] = [
    ("name", "\"desktop-world\"", "\"tissue-world\""),
    ("world.width", "512.0", "128.0"),
    ("world.height", "512.0", "128.0"),
    ("world.grid_width", "512", "128"),
    ("world.grid_height", "512", "128"),
    ("world.tick_denominator", "128", "32"),
    ("world.diffusion", "0.15", "0.477947"),
    ("world.decay_per_second", "0.01", "0.0818013"),
    ("population.initial_count", "100000", "10954"),
    ("population.maximum_count", "160000", "177536"),
    (
        "population.capacity_policy",
        "\"pause-before-commit\"",
        "\"refuse-excess-births-v1\"",
    ),
    ("sources.count", "128", "64"),
    ("sources.power_per_source", "1248.0", "4941.03"),
    ("sources.sigma_world_units", "10.0", "7.48417"),
    ("sources.path_speed", "1.0", "6.41031"),
    ("chemistry.maximum_efficiency", "0.8", "0.777059"),
    ("chemistry.affinity_gain", "4.0", "14.5263"),
    ("chemistry.discrimination_threshold", "0.15", "0.265017"),
    ("chemistry.uptake_capacity_per_second", "6.0", "2.94923"),
    (
        "chemistry.uptake_half_saturation_density",
        "0.1",
        "0.805307",
    ),
    ("chemistry.toxicity_sensitivity", "0.2", "0.0800432"),
    ("chemistry.motion_response_norm_limit", "256.0", "100.46"),
    ("chemistry.maximum_acceleration", "4.0", "19.8961"),
    ("chemistry.drag_per_second", "2.0", "0.608376"),
    ("chemistry.maximum_speed", "4.0", "13.7045"),
    ("sensors.energy_density_scale", "1.0", "8.0"),
    ("costs.basal_energy_per_second", "0.8", "0.690777"),
    (
        "costs.motion_energy_per_speed_squared_second",
        "0.002",
        "0.000451687",
    ),
    ("emission.gate_threshold", "0.5", "0.355002"),
    (
        "emission.maximum_fractional_rate_per_second",
        "0.25",
        "0.97945",
    ),
    ("emission.transfer_efficiency", "0.9", "0.95"),
    (
        "reproduction.maximum_attempt_rate_per_second",
        "1.0",
        "1.81041",
    ),
    (
        "reproduction.minimum_parent_energy_before_attempt",
        "3.0",
        "1.77835",
    ),
    ("reproduction.cooldown_seconds", "0.5", "0.0642813"),
    ("death.residual_energy_recycle_fraction", "0.5", "0.525187"),
    (
        "execution.maximum_logical_genome_bytes",
        "4294967296",
        "17179869184",
    ),
    (
        "execution.maximum_allocated_genome_bytes",
        "4831838208",
        "19327352832",
    ),
    (
        "execution.maximum_total_bytes",
        "17179869184",
        "68719476736",
    ),
];

/// The preset is the Desktop preset with exactly its declared changes, and
/// its header declares each one.
#[test]
fn the_preset_is_the_desktop_preset_with_exactly_its_declared_changes() {
    assert_exactly_declared(DESKTOP, PRESET, &DECLARED);
}

/// The preset is the search's sample c4-x1 (experiments/showcase/tissue.json,
/// its parameters as the search ran them) at its own 128 × 128 scale: every
/// parameter to the 6 significant figures the search's configuration
/// carried, and the same counts. A number that moved would be a world neither
/// the search nor its held-out seeds ran. (The genome quotas, raised so the
/// worst case fits, are the one change, declared above; the preset's run
/// reaches the sample's digest, so they never bound it.)
#[test]
fn the_preset_is_the_chosen_sample_at_its_own_scale() {
    let sample: serde_json::Value = serde_json::from_str(SAMPLE).unwrap_or_default();
    let preset: toml::Table = toml::from_str(PRESET).unwrap_or_default();
    assert_eq!(
        sample.get("sample").and_then(serde_json::Value::as_str),
        Some("c4-x1")
    );
    let Some(params) = sample.get("params").and_then(serde_json::Value::as_object) else {
        panic!("tissue.json has no params");
    };
    assert_eq!(params.len(), 30);
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
            "model.model" | "population.capacity_policy" => {
                assert_eq!(preset_value.as_str(), drawn.as_str(), "{flat}");
            }
            "sources.count" | "population.initial_count" | "population.maximum_count" => {
                assert_eq!(preset_value.as_integer(), drawn.as_i64(), "{flat}");
            }
            _ => {
                let (Some(ours), Some(theirs)) = (preset_value.as_float(), drawn.as_f64()) else {
                    panic!("{flat}: {preset_value:?} or {drawn:?} is not a number");
                };
                assert!(
                    (ours - theirs).abs() <= 5e-6 * theirs.abs(),
                    "{flat}: the preset has {ours}, the search ran {theirs}"
                );
            }
        }
    }
}

/// The preset validates, its worst-case genomes fit its quota, and its
/// session admits it.
#[test]
fn the_preset_validates_and_its_session_admits_it() {
    let config = ValidatedConfig::from_toml(PRESET);
    assert!(config.is_ok(), "{config:?}");
    let Ok(config) = config else { return };
    let summary = Validation::of(&config);
    assert_eq!(summary.schema, VALIDATE_SCHEMA);
    assert_eq!(summary.name, "tissue-world");
    assert_eq!(summary.model, "field-ecology-v1");
    assert_eq!(summary.seed, 42);
    assert_eq!(summary.grid, [128, 128]);
    assert_eq!(summary.founders, 10_954);
    assert_eq!(summary.maximum_population, 177_536);
    assert_eq!(summary.timestep, [1, 32]);
    assert_eq!(summary.default_parameters, 9988);
    // F16 canonical weights: two bytes a parameter.
    assert_eq!(summary.initial_genome_bytes, 10_954 * 9988 * 2);
    assert!(summary.worst_case_fits_logical_quota);
    assert!(SessionConfig::from_config(RUN_SESSION, &config).is_ok());
}
