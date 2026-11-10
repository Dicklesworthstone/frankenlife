//! FL-022 evidence (plan §7.6, §8.1 phase 1, §12.3, §14.3; FL-048's H4):
//! `Intervention::RemoveOrganisms` takes out exactly the organisms in its
//! disk at the boundary, each dying once with its whole energy settled by
//! the command's disposition, and changes nothing else.
//!
//! The oracle for "which organisms" is this file's own minimum-image
//! distance over the committed population, computed with `rem_euclid`
//! rather than the field crate's `displacement`. The ledger oracle is the
//! sum of the removed organisms' committed energies. A disposition must
//! move that residual to its own flow and nowhere else: three worlds from
//! one founding remove the same disk at the same boundary and must agree on
//! everything except where the residual went.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::num::NonZeroUsize;

use flife_core::ledger::Transfer;
use flife_core::model::FIELD_ECOLOGY_V1;
use flife_core::{CoreError, Flow, Tick, ValidatedConfig};
use flife_engine::{
    AppliedIntervention, Arrangement, DeathCause, Disposition, Intervention, NumericalProfile,
    Organism, Stepped, TickReport, World,
};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// The world's side, in world units, in [`ecology`].
const SIDE: f64 = 32.0;

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

/// A world of [`ecology`] stepped to `tick`.
fn world_at(config: &ValidatedConfig, tick: u64) -> World {
    let mut world = found(config);
    while world.tick().get() < tick {
        published(world.step().expect("the tick advances"));
    }
    world
}

/// The minimum-image component `to − from` on a period of [`SIDE`].
fn wrapped(from: f64, to: f64) -> f64 {
    (to - from + SIDE / 2.0).rem_euclid(SIDE) - SIDE / 2.0
}

/// Whether `position` lies within `radius` of `center` on the torus.
fn inside(center: [f64; 2], radius: f64, position: [f64; 2]) -> bool {
    let dx = wrapped(center[0], position[0]);
    let dy = wrapped(center[1], position[1]);
    dx * dx + dy * dy <= radius * radius
}

fn removal(center: [f64; 2], radius: f64, disposition: Disposition) -> Intervention {
    Intervention::RemoveOrganisms {
        center,
        radius,
        disposition,
    }
}

fn close(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() <= 1.0e-12 * expected.abs().max(1.0)
}

/// A disk whose centre is one world unit past the organism with the
/// largest `x`, wrapped. When that organism sits within a unit of the
/// right edge, the centre lands on the left and the organism is inside
/// only by wrapping.
fn across_the_seam(world: &World) -> ([f64; 2], f64) {
    let rightmost = world
        .population()
        .iter()
        .max_by(|a, b| a.position[0].total_cmp(&b.position[0]))
        .expect("a live world");
    let center = [
        (rightmost.position[0] + 1.0).rem_euclid(SIDE),
        rightmost.position[1],
    ];
    (center, 3.0)
}

/// A removal takes out exactly the organisms its disk holds on the torus,
/// across the seam too: each dies once at the boundary, at its committed
/// position, with its whole committed energy as the residual and the
/// command's disposition as its cause, listed before the tick's other
/// deaths. The applied record names the energy and the count it removed,
/// and the removed energy leaves the world as `command-removal`.
#[test]
fn a_removal_takes_out_exactly_the_organisms_in_its_disk_across_the_seam() {
    let config = ecology();
    let mut world = world_at(&config, 40);
    let from = world.tick();
    let committed: Vec<Organism> = world.population().iter().collect();
    let (center, radius) = across_the_seam(&world);
    let expected: Vec<&Organism> = committed
        .iter()
        .filter(|organism| inside(center, radius, organism.position))
        .collect();
    assert!(
        expected.len() >= 2,
        "the disk holds organisms: {expected:?}"
    );
    assert!(
        expected
            .iter()
            .any(|organism| (organism.position[0] - center[0]).abs() > SIDE / 2.0),
        "an organism is inside only across the seam (centre {center:?})"
    );
    let energy: f64 = expected.iter().map(|organism| organism.energy).sum();

    let intervention = removal(center, radius, Disposition::Remove);
    let report = published(world.step_with(&[intervention]).expect("the tick advances"));

    assert_eq!(report.interventions.len(), 1);
    let applied = report.interventions[0];
    assert_eq!(
        (
            applied.tick,
            applied.position,
            applied.intervention,
            applied.removed
        ),
        (from, 0, intervention, expected.len() as u64)
    );
    assert!(close(applied.applied, energy), "{applied:?} vs {energy}");

    let (removed, rest) = report.deaths.split_at(expected.len());
    for (death, organism) in removed.iter().zip(&expected) {
        assert_eq!(death.id, organism.id);
        assert_eq!(death.cause, DeathCause::Removed(Disposition::Remove));
        assert_eq!(death.tick, from.next().expect("a next tick"));
        assert_eq!(death.position, organism.position);
        assert_eq!(death.residual.to_bits(), organism.energy.to_bits());
        assert_eq!(
            (death.genome, death.lineage, death.generation),
            (organism.genome, organism.lineage, organism.generation)
        );
    }
    for death in rest {
        assert!(
            matches!(death.cause, DeathCause::Exhausted(_)),
            "only the removal removes: {death:?}"
        );
        assert!(!expected.iter().any(|organism| organism.id == death.id));
    }
    for organism in &expected {
        assert!(
            world.population().genome(organism.id).is_none(),
            "{} is gone",
            organism.id
        );
    }
    assert!(close(report.closure.flow(Flow::CommandRemoval), energy));
    assert_eq!(report.closure.flow(Flow::UnrecycledDeath), 0.0);
    assert_eq!(report.closure.debited(Transfer::Recycling), 0.0);
}

/// One disk, three dispositions, three worlds from one founding. The tick
/// is the same in all three except where the removed residual went:
/// `remove` records it as `command-removal`, `dissipate` as
/// `unrecycled-death`, and `recycle` debits it to the recycling transfer,
/// whose credit (at most the configured half) is exactly what the field
/// holds beyond the dissipating world's field. The survivors are identical.
#[test]
fn each_disposition_sends_the_residual_where_it_says_and_nowhere_else() {
    let config = ecology();
    let base = world_at(&config, 40);
    let (center, radius) = across_the_seam(&base);
    let energy: f64 = base
        .population()
        .iter()
        .filter(|organism| inside(center, radius, organism.position))
        .map(|organism| organism.energy)
        .sum();
    assert!(energy > 0.0);

    let after = |disposition| {
        let mut world = world_at(&config, 40);
        let report = published(
            world
                .step_with(&[removal(center, radius, disposition)])
                .expect("the tick advances"),
        );
        (world, report)
    };
    let (removed, removing) = after(Disposition::Remove);
    let (dissipated, dissipating) = after(Disposition::Dissipate);
    let (recycled, recycling) = after(Disposition::Recycle);

    let flows = |report: &TickReport| {
        [
            report.closure.flow(Flow::CommandRemoval),
            report.closure.flow(Flow::UnrecycledDeath),
            report.closure.debited(Transfer::Recycling),
        ]
    };
    let [remove, unrecycled, debited] = flows(&removing);
    assert!(close(remove, energy) && unrecycled == 0.0 && debited == 0.0);
    let [remove, unrecycled, debited] = flows(&dissipating);
    assert!(remove == 0.0 && close(unrecycled, energy) && debited == 0.0);
    let [remove, unrecycled, debited] = flows(&recycling);
    let credited = recycling.closure.credited(Transfer::Recycling);
    assert!(remove == 0.0 && close(debited, energy));
    assert!(
        credited > 0.0 && credited <= 0.5 * energy * (1.0 + 1.0e-12),
        "{credited} of {energy}"
    );
    // The transfer's loss, what the field did not receive, is
    // `unrecycled-death`.
    assert!(
        (unrecycled - (energy - credited)).abs() <= 1.0e-12 * energy,
        "{unrecycled} vs {energy} − {credited}"
    );
    let gained = recycled.field().energy_total() - dissipated.field().energy_total();
    assert!(
        (gained - credited).abs() <= 1.0e-9 * credited,
        "the field holds the credit and nothing else: {gained} vs {credited}"
    );
    assert_eq!(
        removed.field().energy_total().to_bits(),
        dissipated.field().energy_total().to_bits(),
        "removing and dissipating leave the same field"
    );

    let survivors =
        |world: &World| -> Vec<Organism> { world.population().iter().collect::<Vec<_>>() };
    assert_eq!(survivors(&removed), survivors(&dissipated));
    assert_eq!(survivors(&removed), survivors(&recycled));
    assert_eq!(
        removing.deaths,
        dissipating
            .deaths
            .iter()
            .map(|death| {
                let mut death = *death;
                if death.cause == DeathCause::Removed(Disposition::Dissipate) {
                    death.cause = DeathCause::Removed(Disposition::Remove);
                }
                death
            })
            .collect::<Vec<_>>()
    );
}

/// Four disks of radius 16 at the corners and centres of a 32 × 32 torus
/// cover it, so every organism dies at the boundary, each once, by the
/// first disk in sealed order that holds it. The removed take no part in
/// their tick: nothing is evaluated, requested, granted, emitted or born.
#[test]
fn overlapping_removals_kill_each_organism_once_and_the_removed_take_no_part() {
    let config = ecology();
    let mut world = world_at(&config, 40);
    let committed: Vec<Organism> = world.population().iter().collect();
    let centers = [[0.0, 0.0], [16.0, 16.0], [0.0, 16.0], [16.0, 0.0]];
    let dispositions = [
        Disposition::Remove,
        Disposition::Dissipate,
        Disposition::Recycle,
        Disposition::Remove,
    ];
    let interventions: Vec<Intervention> = centers
        .iter()
        .zip(dispositions)
        .map(|(&center, disposition)| removal(center, 16.0, disposition))
        .collect();
    let report = published(world.step_with(&interventions).expect("the tick advances"));

    let mut taken = vec![false; committed.len()];
    let mut expected_deaths = Vec::new();
    for (position, (&center, disposition)) in centers.iter().zip(dispositions).enumerate() {
        let mut count = 0;
        let mut energy = 0.0;
        for (organism, taken) in committed.iter().zip(taken.iter_mut()) {
            if !*taken && inside(center, 16.0, organism.position) {
                *taken = true;
                count += 1;
                energy += organism.energy;
                expected_deaths.push((organism.id, DeathCause::Removed(disposition)));
            }
        }
        let applied: AppliedIntervention = report.interventions[position];
        assert_eq!(applied.position, position);
        assert_eq!(applied.removed, count, "disk {position}");
        assert!(close(applied.applied, energy), "disk {position}");
    }
    assert!(
        taken.iter().all(|&taken| taken),
        "the four disks cover the torus"
    );
    let deaths: Vec<_> = report
        .deaths
        .iter()
        .map(|death| (death.id, death.cause))
        .collect();
    assert_eq!(deaths, expected_deaths);
    assert_eq!(
        (
            report.live,
            report.evaluated,
            report.emitted,
            report.attempted,
            report.births.len()
        ),
        (0, 0, 0, 0, 0)
    );
    assert_eq!(report.uptake.requested, 0.0);
    assert_eq!(report.uptake.granted, 0.0);
    assert_eq!(world.population().iter().count(), 0);
    let next = published(world.step().expect("an empty world still advances"));
    assert_eq!((next.live, next.deaths.len()), (0, 0));
}

/// A removal whose disk holds no organism removes nothing, reports zero,
/// and leaves the world exactly as a tick without it.
#[test]
fn a_disk_that_holds_no_organism_changes_nothing() {
    let config = ecology();
    let mut plain = world_at(&config, 40);
    let mut removing = world_at(&config, 40);
    let positions: Vec<[f64; 2]> = plain
        .population()
        .iter()
        .map(|organism| organism.position)
        .collect();
    // The point of a 64 × 64 lattice farthest from every organism.
    let clearance = |point: [f64; 2]| {
        positions
            .iter()
            .map(|&position| {
                let (dx, dy) = (
                    wrapped(point[0], position[0]),
                    wrapped(point[1], position[1]),
                );
                dx * dx + dy * dy
            })
            .fold(f64::INFINITY, f64::min)
            .sqrt()
    };
    let center = (0..64 * 64)
        .map(|at| [f64::from(at % 64) * 0.5, f64::from(at / 64) * 0.5])
        .max_by(|a, b| clearance(*a).total_cmp(&clearance(*b)))
        .expect("a lattice point");
    let radius = 0.5 * clearance(center);
    assert!(radius > 0.0);

    let plain_report = published(plain.step().expect("the tick advances"));
    let intervention = removal(center, radius, Disposition::Recycle);
    let report = published(
        removing
            .step_with(&[intervention])
            .expect("the tick advances"),
    );
    assert_eq!(
        report.interventions,
        vec![AppliedIntervention {
            tick: Tick::new(40),
            position: 0,
            intervention,
            applied: 0.0,
            removed: 0,
            introduced: 0,
        }]
    );
    assert_eq!(removing.digest(), plain.digest());
    assert_eq!(report.deaths, plain_report.deaths);
}

/// A centre outside the world, a radius that is not finite and positive,
/// or a disk wider than half the world is refused before anything changes.
/// Half the world exactly is admitted.
#[test]
fn removals_outside_the_world_or_wider_than_half_of_it_are_refused_unchanged() {
    let config = ecology();
    let mut world = world_at(&config, 10);
    let digest = world.digest();
    for (center, radius) in [
        ([-0.1, 5.0], 1.0),
        ([SIDE, 5.0], 1.0),
        ([5.0, SIDE], 1.0),
        ([f64::NAN, 5.0], 1.0),
        ([5.0, f64::INFINITY], 1.0),
        ([5.0, 5.0], 0.0),
        ([5.0, 5.0], -1.0),
        ([5.0, 5.0], f64::NAN),
        ([5.0, 5.0], f64::INFINITY),
        ([5.0, 5.0], SIDE / 2.0 + 1.0e-9),
    ] {
        let refused = world.step_with(&[removal(center, radius, Disposition::Remove)]);
        assert!(
            matches!(refused, Err(CoreError::NumericalInvalid { .. })),
            "{center:?} r {radius}: {refused:?}"
        );
        assert_eq!(world.tick(), Tick::new(10));
        assert_eq!(world.digest(), digest, "{center:?} r {radius}");
    }
    published(
        world
            .step_with(&[removal([5.0, 5.0], SIDE / 2.0, Disposition::Remove)])
            .expect("half the world is admitted"),
    );
}

/// The removal's result depends on no worker count.
#[test]
fn a_removal_world_is_the_same_at_any_worker_count() {
    let config = ecology();
    let digests: Vec<_> = [1_usize, 3, 8]
        .into_iter()
        .map(|workers| {
            let mut world = world_at(&config, 40);
            world
                .set_cpu_workers(NonZeroUsize::new(workers).expect("nonzero"))
                .expect("workers");
            let (center, radius) = across_the_seam(&world);
            published(
                world
                    .step_with(&[removal(center, radius, Disposition::Recycle)])
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

/// The disk is closed and wraps: from a centre at the origin, `[3, 4]` is
/// exactly 5 away and inside a radius of 5, `[29, 28]` is 5 away across
/// both seams, and nothing farther is inside. An injection removes nothing.
#[test]
fn the_disk_includes_its_circle_and_wraps_both_seams() {
    let config = ecology();
    let world = found(&config);
    let grid = world.field().grid();
    let disk = removal([0.0, 0.0], 5.0, Disposition::Remove);
    for (position, expected) in [
        ([3.0, 4.0], true),
        ([29.0, 28.0], true),
        ([4.0, 3.0], true),
        ([3.0, 4.000_001], false),
        ([28.999_999, 28.0], false),
        ([16.0, 16.0], false),
    ] {
        assert_eq!(
            disk.removes_at(grid, position).expect("a world position"),
            expected,
            "{position:?}"
        );
    }
    let brush = Intervention::InjectEnergy {
        cell: [0, 0],
        amount: 1.0,
    };
    assert!(
        !brush
            .removes_at(grid, [0.0, 0.0])
            .expect("a world position")
    );
}

/// An injection sealed before a removal at one boundary is reported first,
/// in its sealed position, though the removal applies earlier in the tick.
#[test]
fn an_injection_and_a_removal_at_one_boundary_are_reported_in_sealed_order() {
    let config = ecology();
    let mut world = world_at(&config, 40);
    let (center, radius) = across_the_seam(&world);
    let sealed = [
        Intervention::InjectEnergy {
            cell: [3, 3],
            amount: 2.0,
        },
        removal(center, radius, Disposition::Dissipate),
    ];
    let report = published(world.step_with(&sealed).expect("the tick advances"));
    let reported: Vec<(usize, Intervention)> = report
        .interventions
        .iter()
        .map(|applied| (applied.position, applied.intervention))
        .collect();
    assert_eq!(reported, vec![(0, sealed[0]), (1, sealed[1])]);
    assert_eq!(report.interventions[0].removed, 0);
    assert!(report.interventions[1].removed > 0);
}
