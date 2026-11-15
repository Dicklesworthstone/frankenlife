//! FL-034 evidence (plan §7.2, §19.5): `presets/desktop-world.toml` is the
//! Desktop ecology operating profile's preset. It is the Interactive preset
//! scaled to 100,000 founders on 512 × 512 × 33 with F16 canonical weights,
//! at the energy per area that the declared experiment E2 tested and adopted
//! (experiments/fl034/README.md). These tests keep it exactly that: the
//! Interactive preset with its declared changes, each named in its header;
//! E2's base, its control, with only its name; and valid at the profile's
//! scale.

mod preset_diff;

use flife_cli::{RUN_SESSION, VALIDATE_SCHEMA, Validation};
use flife_core::ValidatedConfig;
use flife_runtime::SessionConfig;
use preset_diff::{assert_exactly_declared, changes, value};

const PRESET: &str = include_str!("../../../presets/desktop-world.toml");
const INTERACTIVE: &str = include_str!("../../../presets/interactive-world.toml");
const E2_BASE: &str = include_str!("../../../experiments/fl034/desktop-base.toml");

/// The preset's declared changes from the Interactive preset, as TOML
/// values: `(key, the Interactive preset's value, the preset's value)`.
const DECLARED: [(&str, &str, &str); 12] = [
    ("name", "\"interactive-world\"", "\"desktop-world\""),
    ("world.width", "256.0", "512.0"),
    ("world.height", "256.0", "512.0"),
    ("world.grid_width", "256", "512"),
    ("world.grid_height", "256", "512"),
    ("population.initial_count", "25000", "100000"),
    ("population.maximum_count", "40000", "160000"),
    ("sources.count", "32", "128"),
    ("controller.canonical_weight_dtype", "\"f32\"", "\"f16\""),
    (
        "execution.maximum_logical_genome_bytes",
        "2147483648",
        "4294967296",
    ),
    (
        "execution.maximum_allocated_genome_bytes",
        "2415919104",
        "4831838208",
    ),
    ("execution.maximum_total_bytes", "8589934592", "17179869184"),
];

/// The preset is the Interactive preset with exactly its declared changes,
/// and its header declares each one.
#[test]
fn the_preset_is_the_interactive_preset_with_exactly_its_declared_changes() {
    assert_exactly_declared(INTERACTIVE, PRESET, &DECLARED);
}

/// The preset is E2's base, the control E2 adopted, with only its name: the
/// experiment tested this world.
#[test]
fn the_preset_is_e2s_base_with_only_its_name() {
    assert_eq!(
        changes(E2_BASE, PRESET),
        vec![(
            "name".to_owned(),
            value("\"desktop-scale-up-base\""),
            value("\"desktop-world\"")
        )]
    );
}

/// The preset validates at the Desktop ecology profile's scale (§19.5), and
/// its session admits it.
#[test]
fn the_preset_validates_at_the_desktop_scale_and_its_session_admits_it() {
    let config = ValidatedConfig::from_toml(PRESET);
    assert!(config.is_ok(), "{config:?}");
    let Ok(config) = config else { return };
    let summary = Validation::of(&config);
    assert_eq!(summary.schema, VALIDATE_SCHEMA);
    assert_eq!(summary.name, "desktop-world");
    assert_eq!(summary.model, "field-ecology-v1");
    assert_eq!(summary.seed, 42);
    assert_eq!(summary.grid, [512, 512]);
    assert_eq!(summary.founders, 100_000);
    assert_eq!(summary.maximum_population, 160_000);
    assert_eq!(summary.timestep, [1, 128]);
    assert_eq!(summary.default_parameters, 9988);
    // F16 canonical weights: two bytes a parameter.
    assert_eq!(summary.initial_genome_bytes, 100_000 * 9988 * 2);
    assert_eq!(summary.command_durability, "volatile-in-memory");
    assert!(SessionConfig::from_config(RUN_SESSION, &config).is_ok());
}
