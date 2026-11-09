//! Shared helpers for the FL-018 adapter tests: Appendix A with a few
//! values changed, real worlds, and their render epochs.

#![allow(dead_code, reason = "each test file uses a different subset")]

use flife_core::ValidatedConfig;
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_render::{Lease, Publisher};

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

/// A small world: 32 × 32 world units, 48 founders, room for 1024.
pub(crate) fn small() -> ValidatedConfig {
    config(&[
        ("world", "width", "32.0"),
        ("world", "height", "32.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", "48"),
        ("population", "maximum_count", "1024"),
        ("population", "initial_energy", "3.0"),
        ("execution", "command_durability", "\"volatile-in-memory\""),
    ])
}

/// A world founded from `config`.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
pub(crate) fn found(config: &ValidatedConfig) -> World {
    World::found(
        config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("the world founds")
}

/// Step `world` `ticks` times, none of which may pause.
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
pub(crate) fn advance(world: &mut World, ticks: u64) {
    for _ in 0..ticks {
        match world.step().expect("the tick advances") {
            Stepped::Published(_) => {}
            Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
        }
    }
}

/// The render epoch of `world` as it stands.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
pub(crate) fn epoch(world: &World) -> Lease {
    let publisher = Publisher::new(world, 3, || {}).expect("a publisher");
    publisher
        .feed()
        .viewer(1)
        .expect("a viewer")
        .latest()
        .expect("the founding epoch")
}
