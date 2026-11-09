//! Shared helpers for the FL-014 tests: Appendix A with a few values
//! changed, the same small ecology as FL-013's tests, and a reference world
//! stepped directly so a session's result can be compared with it.

#![allow(dead_code, reason = "each test file uses a different subset")]

use flife_core::ids::{CommandId, SessionId};
use flife_core::{Tick, ValidatedConfig};
use flife_engine::{Arrangement, Intervention, NumericalProfile, Stepped, TickReport, World};
use flife_runtime::{Command, CommandBook, Target};

const PLAN: &str = include_str!("../../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// The session the tests open.
pub(crate) const SESSION: SessionId = SessionId::new(7);

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

/// FL-013's small ecology (32 × 32, 48 founders, four sources) with
/// `volatile-in-memory` durability, plus `extra`.
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

/// An energy injection at `cell`.
pub(crate) const fn inject(cell: [u32; 2], amount: f64) -> Intervention {
    Intervention::InjectEnergy { cell, amount }
}

/// A command from this test's session.
pub(crate) const fn command(id: u64, target: Target, intervention: Intervention) -> Command {
    Command {
        session: SESSION,
        id: CommandId::new(id),
        target,
        expected_revision: None,
        intervention,
    }
}

/// An explicit target without retargeting.
pub(crate) const fn at(tick: u64) -> Target {
    Target::At {
        tick: Tick::new(tick),
        retarget: false,
    }
}

/// Step `world` once with `interventions`, which must not pause.
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
pub(crate) fn publish(world: &mut World, interventions: &[Intervention]) -> TickReport {
    match world.step_with(interventions).expect("the tick advances") {
        Stepped::Published(report) => *report,
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    }
}

/// Seal `book` at `world`'s boundary, step with the sealed set, and record
/// the publication, as the session owner does.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
pub(crate) fn advance(world: &mut World, book: &mut CommandBook) -> TickReport {
    let sealed = book.seal(world.tick()).expect("seal");
    let interventions: Vec<Intervention> = sealed.iter().map(|&(_, i)| i).collect();
    let report = publish(world, &interventions);
    book.published(&report, &sealed).expect("recorded");
    report
}

/// The reference: `config`'s world stepped directly to `ticks`, applying
/// each `(tick, intervention)` at its boundary, in the order given.
pub(crate) fn reference(
    config: &ValidatedConfig,
    ticks: u64,
    schedule: &[(u64, Intervention)],
) -> World {
    let mut world = found(config);
    while world.tick().get() < ticks {
        let now = world.tick().get();
        let due: Vec<Intervention> = schedule
            .iter()
            .filter(|&&(tick, _)| tick == now)
            .map(|&(_, intervention)| intervention)
            .collect();
        publish(&mut world, &due);
    }
    world
}
