//! `presets/contact-world.toml` is `presets/tissue-world.toml` under the
//! contact model (plan §5.4), with the body and adhesion an outer search chose
//! against image statistics of the reference screenshots
//! (experiments/showcase/README.md, "Stage C", states the selection). These
//! tests keep it exactly what the search chose: the tissue preset with its
//! declared changes and the `[contact]` table, each named in its header; the
//! search's sample a10 at its own scale; and valid, with its session admitting
//! it.

mod preset_diff;

use flife_cli::{RUN_SESSION, VALIDATE_SCHEMA, Validation};
use flife_core::ValidatedConfig;
use flife_runtime::SessionConfig;
use preset_diff::assert_exactly_declared;

const PRESET: &str = include_str!("../../../presets/contact-world.toml");
const TISSUE: &str = include_str!("../../../presets/tissue-world.toml");
const SAMPLE: &str = include_str!("../../../experiments/showcase/contact.json");

/// The preset's declared changes from the tissue preset, outside `[contact]`.
const DECLARED: [(&str, &str, &str); 3] = [
    ("name", "\"tissue-world\"", "\"contact-world\""),
    (
        "model",
        "\"field-ecology-v1\"",
        "\"field-ecology-v1-contact\"",
    ),
    ("reproduction.child_position_offset_radius", "0.25", "0.45"),
];

/// The `[contact]` table: `(key, value)`.
const CONTACT: [(&str, f64); 4] = [
    ("diameter", 0.45),
    ("stiffness", 0.5),
    ("adhesion", 1.0),
    ("adhesion_range", 1.5),
];

/// The preset without its `[contact]` table, which the tissue preset lacks.
fn without_contact(text: &str) -> String {
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with('[') {
            inside = line.trim() == "[contact]";
        }
        if !inside {
            out.push(line);
        }
    }
    out.join("\n")
}

/// The preset is the tissue preset with exactly its declared changes and
/// the `[contact]` table, and its header declares each one.
#[test]
fn the_preset_is_the_tissue_preset_with_exactly_its_declared_changes() {
    assert_exactly_declared(TISSUE, &without_contact(PRESET), &DECLARED);
    let preset: toml::Table = toml::from_str(PRESET).unwrap_or_default();
    let Some(table) = preset.get("contact").and_then(toml::Value::as_table) else {
        panic!("the preset has no [contact] table");
    };
    assert_eq!(table.len(), CONTACT.len());
    let header: Vec<&str> = PRESET
        .lines()
        .take_while(|line| line.starts_with('#'))
        .collect();
    for (key, value) in CONTACT {
        assert_eq!(
            table.get(key).and_then(toml::Value::as_float),
            Some(value),
            "contact.{key}"
        );
        let path = format!("contact.{key}");
        assert!(
            header
                .iter()
                .any(|line| line.contains(&path) && line.contains(&format!("{value:?}"))),
            "the header does not declare {path} {value:?}"
        );
    }
}

/// The preset is the search's sample a10 (experiments/showcase/contact.json,
/// its parameters as the search ran them) at its own 128 × 128 scale: every
/// parameter to the 6 significant figures the search's configuration
/// carried, and the same counts and names. (The genome quotas, raised as in
/// the tissue preset so the worst case fits, are not parameters of the
/// sample; the preset's run reaches the sample's digest, so they never bound
/// it.)
#[test]
fn the_preset_is_the_chosen_sample_at_its_own_scale() {
    let sample: serde_json::Value = serde_json::from_str(SAMPLE).unwrap_or_default();
    let preset: toml::Table = toml::from_str(PRESET).unwrap_or_default();
    assert_eq!(
        sample.get("sample").and_then(serde_json::Value::as_str),
        Some("a10")
    );
    let Some(params) = sample.get("params").and_then(serde_json::Value::as_object) else {
        panic!("contact.json has no params");
    };
    assert_eq!(params.len(), 35);
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

/// The preset validates under the contact model, its worst-case genomes fit
/// its quota, and its session admits it.
#[test]
fn the_preset_validates_and_its_session_admits_it() {
    let config = ValidatedConfig::from_toml(PRESET);
    assert!(config.is_ok(), "{config:?}");
    let Ok(config) = config else { return };
    let summary = Validation::of(&config);
    assert_eq!(summary.schema, VALIDATE_SCHEMA);
    assert_eq!(summary.name, "contact-world");
    assert_eq!(summary.model, "field-ecology-v1-contact");
    assert_eq!(summary.seed, 42);
    assert_eq!(summary.grid, [128, 128]);
    assert_eq!(summary.founders, 10_954);
    assert_eq!(summary.maximum_population, 177_536);
    assert_eq!(summary.timestep, [1, 32]);
    assert!(summary.worst_case_fits_logical_quota);
    assert!(SessionConfig::from_config(RUN_SESSION, &config).is_ok());
    assert_eq!(
        flife_engine::contact::body_radius(&config),
        Some(0.225),
        "drawn at its body's radius"
    );
}
