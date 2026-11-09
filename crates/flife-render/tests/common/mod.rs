//! Shared helpers for the FL-017 tests: Appendix A with a few values
//! changed, FL-013's small ecology, and worlds stepped directly.

#![allow(dead_code, reason = "each test file uses a different subset")]

use flife_core::ValidatedConfig;
use flife_engine::{Arrangement, NumericalProfile, Stepped, TickReport, World};

const PLAN: &str = include_str!("../../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

fn appendix_a_toml() -> String {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    appendix[start..start + length].to_owned()
}

/// Appendix A with `overrides`, each `(section, key, value)` replacing the
/// value of `key` in `[section]`.
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
pub(crate) fn config(overrides: &[(&str, &str, &str)]) -> ValidatedConfig {
    let mut lines: Vec<String> = appendix_a_toml().lines().map(str::to_owned).collect();
    for &(section, key, value) in overrides {
        let header = format!("[{section}]");
        let start = lines
            .iter()
            .position(|line| line.trim() == header)
            .unwrap_or_else(|| panic!("no section {header}"));
        let line = lines[start + 1..]
            .iter()
            .take_while(|line| !line.starts_with('['))
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .map(|offset| start + 1 + offset)
            .unwrap_or_else(|| panic!("no key {key} in {header}"));
        lines[line] = format!("{key} = {value}");
    }
    ValidatedConfig::from_toml(&lines.join("\n")).expect("a valid configuration")
}

/// FL-013's small ecology (32 × 32, 48 founders, four sources), plus
/// `extra`.
pub(crate) fn ecology(extra: &[(&str, &str, &str)]) -> ValidatedConfig {
    let mut overrides = vec![
        ("world", "width", "32.0"),
        ("world", "height", "32.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", "48"),
        ("population", "maximum_count", "1024"),
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
    ];
    overrides.extend_from_slice(extra);
    config(&overrides)
}

/// A world founded from `config` with `arrangement`.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
pub(crate) fn found(config: &ValidatedConfig, arrangement: Arrangement) -> World {
    World::found(
        config,
        arrangement,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("the world founds")
}

/// Step `world` once, which must not pause.
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
pub(crate) fn step(world: &mut World) -> TickReport {
    match world.step().expect("the tick advances") {
        Stepped::Published(report) => *report,
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    }
}
