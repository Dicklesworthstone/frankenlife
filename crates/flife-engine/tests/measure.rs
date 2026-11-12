//! Plan §14.3 evidence: `World::measure_region` is the Studio's temporary
//! measurement region. A disk measures exactly what a removal of it at the
//! same boundary takes, and the field cells whose centres it holds, and
//! measuring changes nothing.
//!
//! The oracles are this file's own: a minimum-image distance computed with
//! `rem_euclid` rather than the field crate's `displacement`, the
//! committed population and field read directly, and sums taken in
//! identity order and cell-index order. The removal oracle is the engine's
//! own removal, applied at the measured boundary.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::collections::BTreeSet;

use flife_core::model::FIELD_ECOLOGY_V1;
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_engine::{
    Arrangement, Disposition, Intervention, NumericalProfile, Organism, RegionMeasurement, Stepped,
    TickReport, World,
};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// The world's side, in world units, in [`ecology`]: one field cell a unit.
const SIDE: f64 = 32.0;

/// Appendix A's 32 × 32 test ecology (the removal tests' values).
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

/// The measurement this file's oracle makes of `world`: the organisms and
/// the cells whose centres lie in the disk, summed in identity order and
/// cell-index order.
fn oracle(world: &World, center: [f64; 2], radius: f64) -> RegionMeasurement {
    let held: Vec<Organism> = world
        .population()
        .iter()
        .filter(|organism| inside(center, radius, organism.position))
        .collect();
    let mut organism_energy = 0.0;
    for organism in &held {
        organism_energy += organism.energy;
    }
    let lineages: BTreeSet<_> = held.iter().map(|organism| organism.lineage).collect();
    let energy = world.field().energy();
    let side = 32_u32;
    let mut cells = 0_u64;
    let mut field_energy = 0.0;
    for y in 0..side {
        for x in 0..side {
            let at = [f64::from(x) + 0.5, f64::from(y) + 0.5];
            if inside(center, radius, at) {
                cells += 1;
                field_energy += f64::from(energy[(y * side + x) as usize]);
            }
        }
    }
    RegionMeasurement {
        tick: world.tick(),
        center,
        radius,
        organisms: held.len() as u64,
        organism_energy,
        lineages: lineages.len() as u64,
        cells,
        field_energy,
    }
}

/// `measurement` equals `expected`, the sums bit for bit.
fn assert_measures(measurement: &RegionMeasurement, expected: &RegionMeasurement) {
    assert_eq!(measurement, expected);
    assert_eq!(
        (
            measurement.organism_energy.to_bits(),
            measurement.field_energy.to_bits()
        ),
        (
            expected.organism_energy.to_bits(),
            expected.field_energy.to_bits()
        ),
        "{measurement:?}"
    );
}

/// A disk whose centre is one world unit past the organism with the
/// largest `x`, wrapped (the removal tests' seam disk).
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

/// A disk measures the organisms a removal of it at this boundary takes,
/// across the seam: the same count and, bit for bit, the same energy,
/// with their distinct lineages; and the field cells whose centres lie in
/// it, with their energy. Several disks, several ticks.
#[test]
fn a_measured_disk_holds_what_a_removal_of_it_takes() {
    let config = ecology();
    for tick in [0, 40, 75] {
        let world = world_at(&config, tick);
        let (seam, radius) = across_the_seam(&world);
        for (center, radius) in [
            (seam, radius),
            ([16.0, 16.0], 9.5),
            ([0.25, 31.75], 6.0),
            ([10.0, 20.0], SIDE / 2.0),
        ] {
            let measurement = world.measure_region(center, radius).expect("a world disk");
            assert_measures(&measurement, &oracle(&world, center, radius));
            assert!(measurement.cells > 0, "{measurement:?}");

            let mut removed = world_at(&config, tick);
            let report = published(
                removed
                    .step_with(&[Intervention::RemoveOrganisms {
                        center,
                        radius,
                        disposition: Disposition::Remove,
                    }])
                    .expect("the tick advances"),
            );
            let applied = report.interventions[0];
            assert_eq!(
                (applied.removed, applied.applied.to_bits()),
                (measurement.organisms, measurement.organism_energy.to_bits()),
                "tick {tick}, {center:?} r {radius}"
            );
        }
        let measurement = world.measure_region(seam, radius).expect("a world disk");
        assert!(
            measurement.organisms >= 2 && measurement.lineages >= 1,
            "the seam disk holds organisms at tick {tick}: {measurement:?}"
        );
    }
}

/// The disk is closed for cells too: centred on a cell centre with a
/// radius of a whole number of cells, the cells whose centres lie exactly
/// on its circle count, across both seams, and those just outside do not.
#[test]
fn the_cells_on_the_circle_count_across_both_seams() {
    let config = ecology();
    let world = world_at(&config, 20);
    let center = [0.5, 0.5];
    let measurement = world.measure_region(center, 3.0).expect("a world disk");
    // Cell centres within 3 of a cell centre: offsets (i, j) with
    // i² + j² ≤ 9, 29 of them, 18 of them across a seam.
    assert_eq!(measurement.cells, 29);
    assert_measures(&measurement, &oracle(&world, center, 3.0));
    let narrower = world
        .measure_region(center, 3.0 - 1.0e-9)
        .expect("a world disk");
    assert_eq!(narrower.cells, 25, "the four cells on the circle are out");
}

/// Measuring changes nothing: the world's digest is the same after it, and
/// a world measured every tick reaches the same world as one never
/// measured.
#[test]
fn measuring_changes_nothing() {
    let config = ecology();
    let mut measured = found(&config);
    let mut untouched = found(&config);
    for _ in 0..30 {
        let digest = measured.digest();
        let (center, radius) = across_the_seam(&measured);
        measured
            .measure_region(center, radius)
            .expect("a world disk");
        measured
            .measure_region([8.0, 8.0], SIDE / 2.0)
            .expect("a world disk");
        assert_eq!(measured.digest(), digest);
        published(measured.step().expect("the tick advances"));
        published(untouched.step().expect("the tick advances"));
    }
    assert_eq!(measured.tick(), Tick::new(30));
    assert_eq!(measured.digest(), untouched.digest());
}

/// A disk a removal would refuse is refused as a measurement, named as
/// one; half the world is admitted.
#[test]
fn a_disk_off_the_world_or_wider_than_half_of_it_is_refused() {
    let config = ecology();
    let world = world_at(&config, 10);
    for (center, radius, quantity) in [
        ([-0.1, 5.0], 1.0, "measurement centre"),
        ([SIDE, 5.0], 1.0, "measurement centre"),
        ([5.0, SIDE], 1.0, "measurement centre"),
        ([f64::NAN, 5.0], 1.0, "measurement centre"),
        ([5.0, f64::INFINITY], 1.0, "measurement centre"),
        ([5.0, 5.0], 0.0, "measurement radius"),
        ([5.0, 5.0], -1.0, "measurement radius"),
        ([5.0, 5.0], f64::NAN, "measurement radius"),
        ([5.0, 5.0], f64::INFINITY, "measurement radius"),
        ([5.0, 5.0], SIDE / 2.0 + 1.0e-9, "measurement radius"),
    ] {
        let refused = world.measure_region(center, radius);
        assert!(
            matches!(
                &refused,
                Err(CoreError::NumericalInvalid { quantity: named, .. }) if *named == quantity
            ),
            "{center:?} r {radius}: {refused:?}"
        );
    }
    let whole = world
        .measure_region([5.0, 5.0], SIDE / 2.0)
        .expect("half the world is admitted");
    assert_measures(&whole, &oracle(&world, [5.0, 5.0], SIDE / 2.0));
}

/// A disk with no organism in it measures none, with no energy and no
/// lineage, and still the field under it.
#[test]
fn an_empty_disk_measures_no_organism() {
    let config = ecology();
    let world = world_at(&config, 40);
    let empty = (0..64)
        .map(|at| {
            let at = f64::from(at) / 2.0;
            [at, (at * 7.0).rem_euclid(SIDE)]
        })
        .find(|&center| {
            !world
                .population()
                .iter()
                .any(|organism| inside(center, 0.75, organism.position))
        })
        .expect("a disk with no organism");
    let measurement = world.measure_region(empty, 0.75).expect("a world disk");
    assert_eq!(
        (
            measurement.organisms,
            measurement.organism_energy.to_bits(),
            measurement.lineages
        ),
        (0, 0.0_f64.to_bits(), 0)
    );
    assert_measures(&measurement, &oracle(&world, empty, 0.75));
}
