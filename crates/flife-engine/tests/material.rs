//! FL-022 evidence (plan §14.3's material brush, §12.3): an
//! `Intervention::InjectMaterial` adds declared energy exactly as an energy
//! injection does, and composition on top of it; a direction outside the
//! unit ball is refused unchanged; the world it makes depends on no worker
//! count.
//!
//! The metamorphic oracle: a material with no composition must make the
//! very world an energy injection of the same amount makes. The packet's
//! exact per-channel arithmetic is checked against a bare field in the
//! intervention module's unit tests.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::num::NonZeroUsize;

use flife_core::model::FIELD_ECOLOGY_V1;
use flife_core::{CoreError, Flow, Tick, ValidatedConfig};
use flife_engine::{
    Arrangement, Intervention, MATERIAL_CHANNELS, NumericalProfile, Stepped, TickReport, World,
};
use flife_field::sources::squared_norm;

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// A cell near the 32 × 32 ecology's middle.
const CELL: [u32; 2] = [15, 17];

/// Appendix A's 32 × 32 test ecology (the FL-013 world tests' values).
fn ecology() -> ValidatedConfig {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    let mut lines: Vec<String> = appendix[start..start + length]
        .lines()
        .map(str::to_owned)
        .collect();
    let overrides = [
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
    ];
    for (section, key, value) in overrides {
        let header = format!("[{section}]");
        let first = lines
            .iter()
            .position(|line| line.trim() == header)
            .expect("a section");
        let line = lines[first + 1..]
            .iter()
            .take_while(|line| !line.starts_with('['))
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .map(|offset| first + 1 + offset)
            .expect("a key");
        lines[line] = format!("{key} = {value}");
    }
    let config = ValidatedConfig::from_toml(&lines.join("\n")).expect("a valid configuration");
    assert_eq!(config.config().model, FIELD_ECOLOGY_V1);
    config
}

fn found(config: &ValidatedConfig) -> World {
    World::found(
        config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("the world founds")
}

fn published(stepped: Stepped) -> TickReport {
    match stepped {
        Stepped::Published(report) => *report,
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    }
}

fn world_at(config: &ValidatedConfig, tick: u64) -> World {
    let mut world = found(config);
    while world.tick().get() < tick {
        published(world.step().expect("the tick advances"));
    }
    world
}

/// A direction with `values` in its first channels and zero elsewhere.
fn direction(values: &[f64]) -> [f64; MATERIAL_CHANNELS] {
    let mut direction = [0.0; MATERIAL_CHANNELS];
    direction[..values.len()].copy_from_slice(values);
    direction
}

fn material(cell: [u32; 2], amount: f64, values: &[f64]) -> Intervention {
    Intervention::InjectMaterial {
        cell,
        amount,
        direction: direction(values),
    }
}

/// A material with no composition makes exactly the world an energy
/// injection of the same amount makes: the same applied energy, the same
/// closure, the same digest then and twenty ticks on.
#[test]
fn a_material_with_no_composition_is_the_energy_brush() {
    let config = ecology();
    let mut energy = world_at(&config, 12);
    let mut brushed = world_at(&config, 12);
    let amount = 23.75;
    let a = published(
        energy
            .step_with(&[Intervention::InjectEnergy { cell: CELL, amount }])
            .expect("the tick advances"),
    );
    let b = published(
        brushed
            .step_with(&[material(CELL, amount, &[])])
            .expect("the tick advances"),
    );
    assert_eq!(a.interventions.len(), 1);
    assert_eq!(b.interventions.len(), 1);
    assert_eq!(
        a.interventions[0].applied.to_bits(),
        b.interventions[0].applied.to_bits()
    );
    assert_eq!(
        a.closure.flow(Flow::CommandInjection).to_bits(),
        b.closure.flow(Flow::CommandInjection).to_bits()
    );
    assert_eq!(energy.digest(), brushed.digest());
    for _ in 0..20 {
        published(energy.step().expect("the tick advances"));
        published(brushed.step().expect("the tick advances"));
    }
    assert_eq!(energy.digest(), brushed.digest());
}

/// Composition changes the world but not the energy: the material's applied
/// energy is the energy brush's, bit for bit, never above the request; the
/// ledger's `command-injection` is that energy and the tick closes within
/// its allowance; the worlds differ.
#[test]
fn a_material_adds_the_energy_brush_energy_and_its_composition() {
    let config = ecology();
    let mut energy = world_at(&config, 12);
    let mut brushed = world_at(&config, 12);
    let amount = 23.75;
    let a = published(
        energy
            .step_with(&[Intervention::InjectEnergy { cell: CELL, amount }])
            .expect("the tick advances"),
    );
    let b = published(
        brushed
            .step_with(&[material(CELL, amount, &[0.6, -0.8])])
            .expect("the tick advances"),
    );
    let applied = b.interventions[0].applied;
    assert_eq!(applied.to_bits(), a.interventions[0].applied.to_bits());
    assert!(applied <= amount && amount - applied < 1.0e-5, "{applied}");
    assert_eq!(b.interventions[0].removed, 0);
    // A material removes no organism, even one at its own cell's centre.
    let grid = brushed.field().grid();
    let centre = [f64::from(CELL[0]) + 0.5, f64::from(CELL[1]) + 0.5];
    assert!(
        !material(CELL, amount, &[0.6, -0.8])
            .removes_at(grid, centre)
            .expect("a world position")
    );
    assert_eq!(
        b.closure.flow(Flow::CommandInjection).to_bits(),
        applied.to_bits()
    );
    assert!(b.closure.residual().abs() <= b.closure.allowance());
    assert_ne!(energy.digest(), brushed.digest(), "composition is state");
}

/// A direction with a nonfinite component or a norm above one, or a bad
/// cell or amount, is refused before anything is prepared; the world is
/// unchanged and runs on. A norm of exactly one is admitted.
#[test]
fn materials_outside_the_unit_ball_are_refused_unchanged() {
    let config = ecology();
    let mut world = world_at(&config, 10);
    let digest = world.digest();
    let just_above = 0.8_f64.next_up();
    for (refused, why) in [
        (material(CELL, 1.0, &[0.6, just_above]), "norm above one"),
        (material(CELL, 1.0, &[1.0, 1.0e-7]), "norm above one"),
        (material(CELL, 1.0, &[f64::NAN]), "NaN component"),
        (
            material(CELL, 1.0, &[0.0, f64::INFINITY]),
            "infinite component",
        ),
        (
            material(CELL, 1.0, &[f64::NEG_INFINITY]),
            "infinite component",
        ),
        (material([32, 0], 1.0, &[0.5]), "cell outside"),
        (material(CELL, 0.0, &[0.5]), "zero amount"),
        (material(CELL, f64::NAN, &[0.5]), "NaN amount"),
    ] {
        let result = world.step_with(&[refused]);
        assert!(
            matches!(result, Err(CoreError::NumericalInvalid { .. })),
            "{why}: {result:?}"
        );
        assert_eq!(world.tick(), Tick::new(10), "{why}");
        assert_eq!(world.digest(), digest, "{why}");
    }
    assert_eq!(squared_norm(&[0.6, 0.8]), 1.0, "a unit direction");
    published(
        world
            .step_with(&[material(CELL, 1.0, &[0.6, 0.8])])
            .expect("a unit direction is admitted"),
    );
    let mut last = [0.0; MATERIAL_CHANNELS];
    last[MATERIAL_CHANNELS - 1] = -1.0;
    published(
        world
            .step_with(&[Intervention::InjectMaterial {
                cell: CELL,
                amount: 1.0,
                direction: last,
            }])
            .expect("the last channel is a channel"),
    );
}

/// The material's world depends on no worker count.
#[test]
fn a_material_world_is_the_same_at_any_worker_count() {
    let config = ecology();
    let digests: Vec<_> = [1_usize, 3, 8]
        .into_iter()
        .map(|workers| {
            let mut world = world_at(&config, 40);
            world
                .set_cpu_workers(NonZeroUsize::new(workers).expect("nonzero"))
                .expect("workers");
            published(
                world
                    .step_with(&[material(CELL, 40.0, &[0.0, 0.5, -0.5, 0.5])])
                    .expect("the tick advances"),
            );
            for _ in 0..20 {
                published(world.step().expect("the tick advances"));
            }
            world.digest()
        })
        .collect();
    assert!(
        digests.windows(2).all(|pair| pair[0] == pair[1]),
        "{digests:?}"
    );
}

/// A material and an energy injection into one cell at one boundary apply,
/// and are reported, in sealed order: each sees the cell the one before it
/// left, so the material's energy differs from its energy alone, and the
/// two orders make different worlds.
#[test]
fn a_material_and_an_injection_at_one_cell_apply_in_sealed_order() {
    let config = ecology();
    let energy = Intervention::InjectEnergy {
        cell: CELL,
        amount: 1.0e6,
    };
    let brush = material(CELL, 0.01, &[1.0]);
    let mut first = world_at(&config, 12);
    let mut second = world_at(&config, 12);
    let a = published(first.step_with(&[energy, brush]).expect("advances"));
    let b = published(second.step_with(&[brush, energy]).expect("advances"));
    let order = |report: &TickReport| {
        report
            .interventions
            .iter()
            .map(|applied| (applied.position, applied.intervention))
            .collect::<Vec<_>>()
    };
    assert_eq!(order(&a), vec![(0, energy), (1, brush)]);
    assert_eq!(order(&b), vec![(0, brush), (1, energy)]);
    // On a cell holding about a million, 0.01 is below one f32 ulp (1/16):
    // rounded down, the material after the large injection adds nothing.
    assert_eq!(a.interventions[1].applied, 0.0);
    assert!(b.interventions[0].applied > 0.0);
    assert_ne!(first.digest(), second.digest());
}
