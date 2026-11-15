//! FL-034 evidence (plan §7.2, §19.5): `presets/interactive-world.toml` is
//! the Interactive operating profile's preset. It is the reference
//! small-world preset scaled to 25,000 founders on 256 × 256 × 33, with the
//! source power that the declared experiment E1 selected
//! (experiments/fl034/README.md). These tests keep it exactly that: the
//! reference preset with its declared changes, each named in its header;
//! E1's base with only the selected power; and valid at the profile's
//! scale.

mod preset_diff;

use flife_cli::{RUN_SESSION, VALIDATE_SCHEMA, Validation};
use flife_core::ValidatedConfig;
use flife_runtime::SessionConfig;
use preset_diff::{assert_exactly_declared, changes, value};

const PRESET: &str = include_str!("../../../presets/interactive-world.toml");
const REFERENCE: &str = include_str!("../../../presets/reference-small-world.toml");
const E1_BASE: &str = include_str!("../../../experiments/fl034/interactive-base.toml");

/// The preset's declared changes from the reference small-world preset, as
/// TOML values: `(key, the reference's value, the preset's value)`.
const DECLARED: [(&str, &str, &str); 12] = [
    ("name", "\"reference-small-world\"", "\"interactive-world\""),
    ("world.width", "128.0", "256.0"),
    ("world.height", "128.0", "256.0"),
    ("world.grid_width", "128", "256"),
    ("world.grid_height", "128", "256"),
    ("population.initial_count", "4096", "25000"),
    ("population.maximum_count", "16384", "40000"),
    ("sources.count", "8", "32"),
    ("sources.power_per_source", "960.0", "1248.0"),
    (
        "execution.maximum_logical_genome_bytes",
        "1073741824",
        "2147483648",
    ),
    (
        "execution.maximum_allocated_genome_bytes",
        "1207959552",
        "2415919104",
    ),
    ("execution.maximum_total_bytes", "2147483648", "8589934592"),
];

/// The preset is the reference preset with exactly its declared changes,
/// and its header declares each one.
#[test]
fn the_preset_is_the_reference_preset_with_exactly_its_declared_changes() {
    assert_exactly_declared(REFERENCE, PRESET, &DECLARED);
}

/// The preset is E1's base, its control, with only its name and the source
/// power E1 selected: the experiment tested this world.
#[test]
fn the_preset_is_e1s_base_with_the_power_e1_selected() {
    assert_eq!(
        changes(E1_BASE, PRESET),
        vec![
            (
                "name".to_owned(),
                value("\"interactive-scale-up-base\""),
                value("\"interactive-world\"")
            ),
            (
                "sources.power_per_source".to_owned(),
                value("960.0"),
                value("1248.0")
            ),
        ]
    );
}

/// The preset validates at the Interactive profile's scale (§19.5), and its
/// session admits it.
#[test]
fn the_preset_validates_at_the_interactive_scale_and_its_session_admits_it() {
    let config = ValidatedConfig::from_toml(PRESET);
    assert!(config.is_ok(), "{config:?}");
    let Ok(config) = config else { return };
    let summary = Validation::of(&config);
    assert_eq!(summary.schema, VALIDATE_SCHEMA);
    assert_eq!(summary.name, "interactive-world");
    assert_eq!(summary.model, "field-ecology-v1");
    assert_eq!(summary.seed, 42);
    assert_eq!(summary.grid, [256, 256]);
    assert_eq!(summary.founders, 25_000);
    assert_eq!(summary.maximum_population, 40_000);
    assert_eq!(summary.timestep, [1, 128]);
    assert_eq!(summary.default_parameters, 9988);
    assert_eq!(summary.initial_genome_bytes, 25_000 * 9988 * 4);
    assert_eq!(summary.command_durability, "volatile-in-memory");
    assert!(SessionConfig::from_config(RUN_SESSION, &config).is_ok());
}
