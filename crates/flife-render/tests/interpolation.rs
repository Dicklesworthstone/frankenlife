//! FL-020 evidence (plan §13.5, C.15): an epoch drawn between two committed
//! epochs follows each organism by identity; it takes the true path across
//! the seams when the gap is unambiguous and snaps to the latest state when
//! it is not; a birth appears only with the epoch that first shows it, and
//! a reused storage slot never continues another organism's path.
//!
//! The oracle for a gap's true displacement is each organism's tick-by-tick
//! path in the world: per-tick minimum-image steps, each far under half the
//! world, summed. It never uses the two endpoints' minimum image.

mod common;

use std::collections::BTreeMap;

use common::{ecology, found, step};
use flife_core::CoreError;
use flife_engine::{Arrangement, Disposition, Intervention, Stepped, World};
use flife_render::{Interpolation, Lease, Publication, Publisher, interpolate, wrapped_between};

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn publisher(world: &World) -> Publisher {
    Publisher::new(world, 3, || {}).expect("a publisher")
}

/// Offer `world` and lease what the feed then shows.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn offer_and_lease(publisher: &mut Publisher, world: &World) -> Lease {
    let publication = publisher.offer(world).expect("an offer");
    assert!(
        matches!(publication, Publication::Published { .. }),
        "{publication:?}"
    );
    publisher
        .feed()
        .viewer(1)
        .expect("a viewer")
        .latest()
        .expect("a lease")
}

fn positions(world: &World) -> BTreeMap<u64, [f64; 2]> {
    world
        .population()
        .iter()
        .map(|organism| (organism.id.get(), organism.position))
        .collect()
}

/// The minimum wrapped difference `b − a` on an axis of `length`.
fn minimum_image(a: f64, b: f64, length: f64) -> f64 {
    let d = (b - a).rem_euclid(length);
    if d >= length / 2.0 { d - length } else { d }
}

/// Step `world` `ticks` times, adding each surviving organism's per-tick
/// displacement to `travelled`.
fn walk(world: &mut World, ticks: u64, travelled: &mut BTreeMap<u64, [f64; 2]>) {
    let [width, height] = [32.0, 32.0];
    for _ in 0..ticks {
        let before = positions(world);
        step(world);
        for (id, now) in positions(world) {
            if let Some(then) = before.get(&id) {
                let path = travelled.entry(id).or_insert([0.0, 0.0]);
                path[0] += minimum_image(then[0], now[0], width);
                path[1] += minimum_image(then[1], now[1], height);
            }
        }
    }
}

/// The periodic distance between two coordinates.
fn periodic(a: f64, b: f64, length: f64) -> f64 {
    minimum_image(a, b, length).abs()
}

/// Over unambiguous 40-tick gaps, every continuing organism is drawn at its
/// prior position at fraction 0 and its current one at 1, exactly, and in
/// between on its true path: across a seam it is drawn near the seam, not
/// across the world (at least three crossings are seen). Every drawn
/// position is in the world.
#[test]
fn interpolation_follows_each_organism_on_its_true_path_across_the_seams() {
    // The ecology's founders barely move (a few hundredths of a unit per
    // gap); ten times the acceleration limit and almost no drag let some of
    // them travel across a seam within the test.
    let fast = [
        ("chemistry", "maximum_acceleration", "40.0"),
        ("chemistry", "drag_per_second", "0.1"),
    ];
    let mut world = found(&ecology(&fast), Arrangement::FreeList);
    let mut publisher = publisher(&world);
    let mut into = Interpolation::default();
    let mut crossed = 0;
    let mut checked = 0;
    let mut gaps = 0;
    while crossed < 3 && gaps < 400 {
        gaps += 1;
        let prior = publisher
            .feed()
            .viewer(1)
            .expect("a viewer")
            .latest()
            .expect("a lease");
        let mut travelled = BTreeMap::new();
        walk(&mut world, 40, &mut travelled);
        let current = offer_and_lease(&mut publisher, &world);
        for fraction in [0.0, 0.25, 0.5, 0.875, 1.0] {
            let gap = interpolate(&prior, &current, fraction, &mut into).expect("interpolates");
            assert_eq!(gap.ticks, 40);
            assert!(gap.unambiguous, "{gap:?}");
            assert!(!into.snapped);
            assert_eq!(into.positions.len(), current.len());
            for (row, &id) in current.ids().iter().enumerate() {
                let drawn = into.positions[row];
                assert!((0.0..32.0).contains(&drawn[0]) && (0.0..32.0).contains(&drawn[1]));
                let Some(from) = prior.row(flife_core::ids::OrganismId::new(id)) else {
                    continue;
                };
                assert!(into.shown[row]);
                let a = prior.positions()[from].map(f64::from);
                let b = current.positions()[row].map(f64::from);
                if fraction == 0.0 {
                    assert_eq!(drawn, a);
                } else if fraction == 1.0 {
                    assert_eq!(drawn, b);
                } else {
                    let path = travelled[&id];
                    for axis in 0..2 {
                        let expected = a[axis] + fraction * path[axis];
                        assert!(
                            periodic(drawn[axis], expected, 32.0) < 1e-4,
                            "organism {id} axis {axis} at {fraction}: drawn {drawn:?}, path {path:?} from {a:?}"
                        );
                    }
                    checked += 1;
                    if fraction == 0.5 && ((b[0] - a[0]).abs() > 16.0 || (b[1] - a[1]).abs() > 16.0)
                    {
                        // A seam crossing: the drawn point is within the
                        // gap's travel of both ends, not across the world.
                        crossed += 1;
                        for axis in 0..2 {
                            assert!(periodic(drawn[axis], a[axis], 32.0) <= gap.travel_bound);
                            assert!(periodic(drawn[axis], b[axis], 32.0) <= gap.travel_bound);
                        }
                    }
                }
            }
        }
        drop(prior);
    }
    assert!(checked > 1000, "{checked} interpolated positions checked");
    assert!(crossed >= 3, "{crossed} seam crossings in {gaps} gaps");
}

/// C.15: on a width-128 axis, an organism that moved 80 units forward would
/// be drawn 48 units in reverse by the minimum image. A gap long enough for
/// that is ambiguous, so every continuing organism snaps to the latest
/// state, exactly, and trails reset; no in-between path is drawn.
#[test]
fn c15_an_ambiguous_gap_snaps_to_the_latest_state_and_draws_no_reverse_path() {
    // What the minimum image would draw halfway: 10 to 90 forward, drawn
    // backwards through the seam (10 − 24 = −14, wrapped to 114).
    assert_eq!(wrapped_between(10.0, 90.0, 128.0, 0.5), 114.0);
    assert_eq!(wrapped_between(10.0, 90.0, 128.0, 0.0), 10.0);
    assert_eq!(wrapped_between(10.0, 90.0, 128.0, 1.0), 90.0);
    // Under half the world, it is the true path, across the seam too.
    assert_eq!(wrapped_between(120.0, 8.0, 128.0, 0.5), 0.0);
    assert_eq!(wrapped_between(8.0, 120.0, 128.0, 0.25), 4.0);

    let mut world = found(&ecology(&[]), Arrangement::FreeList);
    let mut publisher = publisher(&world);
    let prior = publisher
        .feed()
        .viewer(1)
        .expect("a viewer")
        .latest()
        .expect("a lease");
    for _ in 0..600 {
        step(&mut world);
    }
    let current = offer_and_lease(&mut publisher, &world);
    let mut into = Interpolation::default();
    let mut continuing = 0;
    for fraction in [0.0, 0.5, 1.0] {
        let gap = interpolate(&prior, &current, fraction, &mut into).expect("interpolates");
        assert_eq!(gap.ticks, 600);
        assert!(!gap.unambiguous, "{gap:?}");
        assert!(into.snapped);
        for (row, &id) in current.ids().iter().enumerate() {
            if prior.row(flife_core::ids::OrganismId::new(id)).is_some() {
                assert_eq!(into.positions[row], current.positions()[row].map(f64::from));
                continuing += 1;
            }
        }
    }
    assert!(continuing > 0, "some organism lived through the gap");
}

/// Organisms removed in a gap fade from their prior rows; organisms
/// introduced in it (new identities, in the reused storage slots of the
/// removed ones) are drawn only from fraction 1, at their current
/// positions, never on a path from the removed organisms' positions.
#[test]
fn a_birth_appears_with_its_epoch_and_a_reused_slot_continues_no_path() {
    let mut world = found(&ecology(&[]), Arrangement::FreeList);
    let mut publisher = publisher(&world);
    let prior = publisher
        .feed()
        .viewer(1)
        .expect("a viewer")
        .latest()
        .expect("a lease");
    let before = positions(&world);
    let slots: BTreeMap<u64, usize> = before
        .keys()
        .map(|&id| {
            let slot = world
                .population()
                .slot(flife_core::ids::OrganismId::new(id))
                .expect("a slot");
            (id, slot)
        })
        .collect();
    // Four disks of radius 16 on a 16-unit lattice cover the 32 × 32 torus
    // (no point is farther than 8√2 from a center).
    let mut interventions: Vec<Intervention> = [[0.0, 0.0], [16.0, 0.0], [0.0, 16.0], [16.0, 16.0]]
        .into_iter()
        .map(|center| Intervention::RemoveOrganisms {
            center,
            radius: 16.0,
            disposition: Disposition::Remove,
        })
        .collect();
    interventions.push(Intervention::IntroduceOrganisms {
        cell: [3, 29],
        count: 5,
        energy: 2.0,
    });
    let report = match world.step_with(&interventions).expect("the tick runs") {
        Stepped::Published(report) => report,
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    };
    assert_eq!(report.live, 5, "every founder removed, five introduced");
    let reused = world
        .population()
        .iter()
        .filter(|organism| {
            let slot = world.population().slot(organism.id);
            slots.values().any(|&old| Some(old) == slot)
        })
        .count();
    assert!(
        reused > 0,
        "an introduced organism took a removed one's slot"
    );
    let current = offer_and_lease(&mut publisher, &world);
    let mut into = Interpolation::default();
    for fraction in [0.0, 0.5, 0.999, 1.0] {
        let gap = interpolate(&prior, &current, fraction, &mut into).expect("interpolates");
        assert!(gap.unambiguous);
        assert_eq!(into.fading, (0..48).collect::<Vec<usize>>());
        assert_eq!(into.positions.len(), 5);
        for (row, &id) in current.ids().iter().enumerate() {
            assert!(id >= 48, "{id} is a new identity");
            assert!(!before.contains_key(&id));
            assert_eq!(into.shown[row], fraction >= 1.0);
            assert_eq!(into.positions[row], current.positions()[row].map(f64::from));
        }
    }
}

#[test]
fn interpolation_outside_its_rules_is_refused() {
    let mut world = found(&ecology(&[]), Arrangement::Compact);
    let mut publisher = publisher(&world);
    let prior = publisher
        .feed()
        .viewer(1)
        .expect("a viewer")
        .latest()
        .expect("a lease");
    step(&mut world);
    let current = offer_and_lease(&mut publisher, &world);
    let mut into = Interpolation::default();
    for fraction in [-0.01, 1.01, f64::NAN, f64::INFINITY] {
        assert!(
            matches!(
                interpolate(&prior, &current, fraction, &mut into),
                Err(CoreError::NumericalInvalid { .. })
            ),
            "{fraction}"
        );
    }
    assert!(matches!(
        interpolate(&current, &prior, 0.5, &mut into),
        Err(CoreError::StateConflict { .. })
    ));
}
