//! FL-022 evidence (plan §12.3's seeded introduction, §14.3): an
//! `Intervention::IntroduceOrganisms` adds founder-drawn organisms with
//! declared energy at the end of its tick, after the births, all or none.
//!
//! The oracle is a second founding, never the introduction code: an
//! organism introduced into a 48-founder world at tick 0 must carry exactly
//! the genome, traits and phase of the founder with its identity in the
//! same world founded with more founders, its position must be that
//! founder's position words scaled into the cell, and its cooldown deadline
//! must be that founder's, one tick later. A refused introduction must make
//! exactly the world in which it was never sent.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::num::NonZeroUsize;

use flife_core::ids::{Generation, LineageId, OrganismId};
use flife_core::model::FIELD_ECOLOGY_V1;
use flife_core::{CoreError, Flow, Tick, ValidatedConfig};
use flife_engine::{
    AppliedIntervention, Arrangement, Disposition, HeldOutput, Intervention, IntroductionEvent,
    MAXIMUM_INTRODUCTION, NumericalProfile, Stepped, TickReport, World,
};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// A cell near the 32 × 32 ecology's middle (cells are 1 × 1 world units).
const CELL: [u32; 2] = [15, 17];

/// Appendix A's 32 × 32 test ecology (the FL-013 world tests' values), with
/// `extra` overrides on top.
fn ecology(extra: &[(&str, &str, &str)]) -> ValidatedConfig {
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
    for &(section, key, value) in overrides.iter().chain(extra) {
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

const fn introduce(cell: [u32; 2], count: u32, energy: f64) -> Intervention {
    Intervention::IntroduceOrganisms {
        cell,
        count,
        energy,
    }
}

/// Five organisms introduced at tick 0 are the founders 48–52 of the same
/// world founded with 53: the same genomes bit for bit, traits and phases;
/// their positions are those founders' position words scaled into the
/// cell; they are founders of their own lineages, born at tick 1 with the
/// endowment, zero velocity, an invalid held output and a newborn's
/// cooldown deadline. The endowments are `command-injection`.
#[test]
fn introduced_organisms_are_the_founders_a_larger_founding_draws() {
    let config = ecology(&[]);
    let mut plain = found(&config);
    let plain_report = published(plain.step().expect("the tick advances"));
    let mut world = found(&config);
    let intervention = introduce(CELL, 5, 2.5);
    let report = published(world.step_with(&[intervention]).expect("the tick advances"));
    assert!(
        report.births.is_empty(),
        "no founder can reproduce at tick 0"
    );
    assert_eq!(
        report.interventions,
        vec![AppliedIntervention {
            tick: Tick::new(0),
            position: 0,
            intervention,
            applied: 12.5,
            removed: 0,
            introduced: 5,
        }]
    );
    assert_eq!(
        report.closure.flow(Flow::CommandInjection).to_bits(),
        12.5_f64.to_bits()
    );
    assert_eq!(report.live, plain_report.live + 5);
    assert_eq!(
        report.events.end - report.events.start,
        u64::try_from(report.deaths.len()).expect("a count") + 5
    );
    assert_eq!(report.deaths, plain_report.deaths);

    let larger = found(&ecology(&[("population", "initial_count", "53")]));
    let grid = *world.field().grid();
    let (width, height) = (grid.world_width(), grid.world_height());
    for raw in 48..53 {
        let id = OrganismId::new(raw);
        let organism = world.population().get(id).expect("introduced");
        let founder = larger.population().get(id).expect("founded");
        assert_eq!(organism.genome, founder.genome, "{id}");
        let (ours, theirs) = (
            world.population().genome(id).expect("a genome"),
            larger.population().genome(id).expect("a genome"),
        );
        let bits = |genome: &flife_genome::Genome| -> Vec<u32> {
            genome
                .parameters()
                .iter()
                .map(|value| value.to_bits())
                .collect()
        };
        assert_eq!(bits(ours), bits(theirs), "{id}'s parameters");
        assert_eq!(ours.traits(), theirs.traits(), "{id}'s traits");
        assert_eq!(organism.parent, None);
        assert_eq!(organism.lineage, LineageId::new(raw));
        assert_eq!(organism.generation, Generation::FOUNDER);
        assert_eq!(organism.birth_tick, Tick::new(1));
        assert_eq!(organism.energy.to_bits(), 2.5_f64.to_bits());
        assert_eq!(organism.velocity, [0.0, 0.0]);
        assert_eq!(organism.energy_change_rate.to_bits(), 0.0_f64.to_bits());
        assert_eq!(organism.held, HeldOutput::INVALID);
        assert_eq!(organism.phase, founder.phase);
        assert_eq!(
            organism.cooldown_deadline.get(),
            founder.cooldown_deadline.get() + 1
        );
        for axis in 0..2 {
            let (cells, length, size) = if axis == 0 {
                (CELL[0], width, grid.cell_width())
            } else {
                (CELL[1], height, grid.cell_height())
            };
            let expected = (f64::from(cells) + founder.position[axis] / length) * size;
            assert!(
                (organism.position[axis] - expected).abs() <= 1.0e-12,
                "{id} axis {axis}: {} against {expected}",
                organism.position[axis]
            );
            let lower = f64::from(cells) * size;
            assert!(
                (lower..=lower + size).contains(&organism.position[axis]),
                "{id} axis {axis} is outside its cell"
            );
        }
    }
    assert!(world.population().get(OrganismId::new(53)).is_none());
    // The report lists each introduced organism, after every birth.
    let listed: Vec<IntroductionEvent> = (48..53)
        .map(|raw| {
            let id = OrganismId::new(raw);
            let genome = larger.population().genome(id).expect("a genome");
            IntroductionEvent {
                organism: id,
                genome: genome.id(),
                topology: genome.topology(),
            }
        })
        .collect();
    assert_eq!(report.introductions, listed);

    // Their first advance evaluates them, and the world goes on.
    published(world.step().expect("the tick advances"));
    let evaluated = (48..53)
        .filter_map(|raw| world.population().get(OrganismId::new(raw)))
        .filter(|organism| organism.held.is_valid())
        .count();
    assert!(evaluated > 0, "the introduced organisms were evaluated");
    for _ in 0..30 {
        published(world.step().expect("the tick advances"));
    }
}

/// Under a controller interval of 4, introduced organisms take the stable
/// phases of the founders with their identities (48–52: 0, 1, 2, 3, 0), not
/// a common one.
#[test]
fn introduced_phases_follow_identity_under_a_longer_interval() {
    let interval = ("controller", "evaluation_interval_ticks", "4");
    let mut world = found(&ecology(&[interval]));
    published(
        world
            .step_with(&[introduce(CELL, 5, 2.0)])
            .expect("the tick advances"),
    );
    let larger = found(&ecology(&[interval, ("population", "initial_count", "53")]));
    let phases: Vec<(u32, u32)> = (48..53)
        .map(|raw| {
            let id = OrganismId::new(raw);
            (
                world.population().get(id).expect("introduced").phase,
                larger.population().get(id).expect("founded").phase,
            )
        })
        .collect();
    assert!(
        phases.iter().all(|(ours, theirs)| ours == theirs),
        "{phases:?}"
    );
    assert!(phases.iter().any(|&(ours, _)| ours != 0), "{phases:?}");
}

/// An introduction applies in phase 8 and an injection in phase 2, but the
/// report lists every intervention in its sealed position.
#[test]
fn the_report_lists_an_introduction_in_its_sealed_position() {
    let config = ecology(&[]);
    let mut world = world_at(&config, 3);
    let sealed = [
        introduce(CELL, 2, 1.0),
        Intervention::InjectEnergy {
            cell: [2, 2],
            amount: 5.0,
        },
        Intervention::RemoveOrganisms {
            center: [1.0, 1.0],
            radius: 0.5,
            disposition: Disposition::Remove,
        },
        introduce([4, 4], 1, 1.0),
    ];
    let report = published(world.step_with(&sealed).expect("the tick advances"));
    let listed: Vec<(usize, Intervention)> = report
        .interventions
        .iter()
        .map(|applied| (applied.position, applied.intervention))
        .collect();
    assert_eq!(
        listed,
        sealed.iter().copied().enumerate().collect::<Vec<_>>()
    );
    let introduced: Vec<u64> = report
        .interventions
        .iter()
        .map(|applied| applied.introduced)
        .collect();
    assert_eq!(introduced, vec![2, 0, 0, 1]);
}

/// Under `population.maximum_count = 51` with 48 live: introductions of 2,
/// 2 and 1 at one boundary apply 2, none and 1, in sealed order, and the
/// world equals one sent only the two that fit. Under a genome quota with
/// room for two more genomes, an introduction of 3 introduces none and one
/// of 2 introduces both.
#[test]
fn an_introduction_is_all_or_none_under_the_cap_and_the_quota() {
    let config = ecology(&[("population", "maximum_count", "51")]);
    let mut capped = found(&config);
    let first = introduce(CELL, 2, 1.5);
    let refused = introduce([3, 4], 2, 1.5);
    let last = introduce([30, 1], 1, 4.0);
    let report = published(
        capped
            .step_with(&[first, refused, last])
            .expect("the tick advances"),
    );
    assert!(report.deaths.is_empty() && report.births.is_empty());
    let applied: Vec<(usize, u64, u64)> = report
        .interventions
        .iter()
        .map(|applied| {
            (
                applied.position,
                applied.applied.to_bits(),
                applied.introduced,
            )
        })
        .collect();
    assert_eq!(
        applied,
        vec![
            (0, 3.0_f64.to_bits(), 2),
            (1, 0.0_f64.to_bits(), 0),
            (2, 4.0_f64.to_bits(), 1)
        ]
    );
    assert_eq!(
        report.closure.flow(Flow::CommandInjection).to_bits(),
        7.0_f64.to_bits()
    );
    assert_eq!(report.live, 51);
    let listed: Vec<u64> = report
        .introductions
        .iter()
        .map(|introduced| introduced.organism.get())
        .collect();
    assert_eq!(listed, vec![48, 49, 50], "the refused one lists nobody");
    let third = capped
        .population()
        .get(OrganismId::new(50))
        .expect("the last introduction's organism");
    assert_eq!(third.energy.to_bits(), 4.0_f64.to_bits());
    assert!((30.0..=31.0).contains(&third.position[0]));
    let mut unsent = found(&config);
    published(unsent.step_with(&[first, last]).expect("the tick advances"));
    assert_eq!(capped.digest(), unsent.digest());

    let per_genome = found(&ecology(&[])).quota().logical_bytes() / 48;
    let room = (50 * per_genome).to_string();
    let quota = ecology(&[("execution", "maximum_logical_genome_bytes", &room)]);
    let mut short = found(&quota);
    let report = published(
        short
            .step_with(&[introduce(CELL, 3, 1.0), introduce(CELL, 2, 1.0)])
            .expect("the tick advances"),
    );
    let introduced: Vec<u64> = report
        .interventions
        .iter()
        .map(|applied| applied.introduced)
        .collect();
    assert_eq!(introduced, vec![0, 2]);
    assert_eq!(short.quota().live(), 50);
    assert_eq!(short.identities().organisms.next_raw(), Some(50));
}

/// The births of the tick an introduction applies at are exactly the births
/// without it, and every introduced identity comes after theirs.
#[test]
fn introductions_enter_after_the_births_and_never_change_them() {
    let config = ecology(&[]);
    let mut scout = found(&config);
    let tick = loop {
        let report = published(scout.step().expect("the tick advances"));
        if !report.births.is_empty() {
            break report.from.get();
        }
        assert!(
            scout.tick().get() < 200,
            "the ecology reproduces within 200 ticks"
        );
    };
    let mut plain = world_at(&config, tick);
    let mut world = world_at(&config, tick);
    let expected = published(plain.step().expect("the tick advances"));
    let report = published(
        world
            .step_with(&[introduce(CELL, 3, 2.0)])
            .expect("the tick advances"),
    );
    assert!(!expected.births.is_empty());
    assert_eq!(report.births, expected.births);
    assert_eq!(report.rejected, expected.rejected);
    assert_eq!(report.deaths, expected.deaths);
    let newest = expected
        .births
        .iter()
        .map(|birth| birth.child)
        .max()
        .expect("a birth");
    let introduced: Vec<_> = world
        .population()
        .iter()
        .filter(|organism| organism.birth_tick == report.tick && organism.parent.is_none())
        .map(|organism| organism.id)
        .collect();
    assert_eq!(introduced.len(), 3);
    assert!(
        introduced.iter().all(|&id| id > newest),
        "{introduced:?} after {newest}"
    );
    let listed: Vec<_> = report
        .introductions
        .iter()
        .map(|event| event.organism)
        .collect();
    assert_eq!(listed, introduced);
    assert_eq!(
        report.events.end - report.events.start,
        u64::try_from(report.deaths.len() + report.births.len() + 3).expect("a count")
    );
    assert_eq!(report.live, expected.live + 3);
    assert_ne!(world.digest(), plain.digest());
}

/// Refused introductions change nothing: no organisms, a count above the
/// limit, an endowment that is not a finite positive energy at most
/// `f32::MAX`, or a cell outside the grid.
#[test]
fn introductions_outside_the_rules_are_refused_unchanged() {
    let config = ecology(&[]);
    let mut world = world_at(&config, 4);
    let digest = world.digest();
    for (refused, quantity) in [
        (introduce(CELL, 0, 1.0), "introduction count"),
        (
            introduce(CELL, MAXIMUM_INTRODUCTION + 1, 1.0),
            "introduction count",
        ),
        (introduce(CELL, 1, 0.0), "introduction energy"),
        (introduce(CELL, 1, -1.0), "introduction energy"),
        (introduce(CELL, 1, f64::NAN), "introduction energy"),
        (introduce(CELL, 1, f64::INFINITY), "introduction energy"),
        (
            introduce(CELL, 1, f64::from(f32::MAX) * 2.0),
            "introduction energy",
        ),
        (introduce([32, 0], 1, 1.0), "introduction cell"),
        (introduce([0, 32], 1, 1.0), "introduction cell"),
    ] {
        match world.step_with(&[refused]) {
            Err(CoreError::NumericalInvalid {
                quantity: named, ..
            }) => {
                assert_eq!(named, quantity, "{refused:?}");
            }
            other => panic!("{refused:?} was not refused: {other:?}"),
        }
        assert_eq!(world.tick(), Tick::new(4));
        assert_eq!(world.digest(), digest);
    }
    let grid = *world.field().grid();
    assert!(
        introduce(CELL, MAXIMUM_INTRODUCTION, f64::from(f32::MAX))
            .check(&grid)
            .is_ok()
    );
    assert_eq!(
        introduce(CELL, 4, 2.5).declared_energy().to_bits(),
        10.0_f64.to_bits(),
        "an introduction declares every endowment"
    );
    assert!(
        !introduce(CELL, 4, 2.5)
            .removes_at(&grid, [15.5, 17.5])
            .expect("a world position"),
        "an introduction removes nobody"
    );
    // Over the cap: admitted, and it introduces nothing.
    let report = published(
        world
            .step_with(&[introduce(CELL, MAXIMUM_INTRODUCTION, 1.0)])
            .expect("the tick advances"),
    );
    assert_eq!(report.interventions[0].introduced, 0);
    assert_eq!(report.interventions[0].applied.to_bits(), 0.0_f64.to_bits());
}

/// The same introductions make the same world on 1, 3 and 8 workers.
#[test]
fn an_introduced_world_is_the_same_at_any_worker_count() {
    let config = ecology(&[]);
    let digests: Vec<_> = [1, 3, 8]
        .into_iter()
        .map(|workers| {
            let mut world = world_at(&config, 10);
            world
                .set_cpu_workers(NonZeroUsize::new(workers).expect("nonzero"))
                .expect("workers");
            published(
                world
                    .step_with(&[introduce(CELL, 6, 2.0), introduce([2, 30], 4, 3.5)])
                    .expect("the tick advances"),
            );
            for _ in 0..40 {
                published(world.step().expect("the tick advances"));
            }
            world.digest()
        })
        .collect();
    assert_eq!(digests[0], digests[1]);
    assert_eq!(digests[0], digests[2]);
}
