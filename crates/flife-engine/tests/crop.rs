//! FL-042 evidence (plan §14.5): `World::cropped` is a local package's world,
//! one disk of a committed world with everything outside it emptied.
//!
//! The oracles are this file's own, as `tests/measure.rs`'s are: a
//! minimum-image distance computed with `rem_euclid`, and the source
//! world's population, genomes and field read directly. A crop's checkpoint
//! is read back through the engine's own restore, which checks the ledger
//! against the restored state bit for bit.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use flife_core::checkpoint::{CheckpointReader, CheckpointWriter};
use flife_core::model::FIELD_ECOLOGY_V1;
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{
    Arrangement, NumericalProfile, Organism, Stepped, WORLD_CHECKPOINT_CHUNKS, World,
};
use flife_field::Cell;

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// The world's side, in world units, in [`ecology`]: one field cell a unit.
const SIDE: f64 = 32.0;

/// Appendix A's 32 × 32 test ecology (the measurement tests' values).
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

fn profile() -> NumericalProfile {
    NumericalProfile::reference_f32().expect("profile")
}

/// `world` advanced one tick, which must publish.
fn step(world: &mut World) {
    match world.step().expect("the tick advances") {
        Stepped::Published(_) => {}
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    }
}

/// A world of [`ecology`] stepped to `tick`.
fn world_at(config: &ValidatedConfig, tick: u64) -> World {
    let mut world = World::found(config, Arrangement::FreeList, profile()).expect("founds");
    while world.tick().get() < tick {
        step(&mut world);
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

/// A cell's planes as bits.
fn bits(cell: Cell) -> Vec<u32> {
    std::iter::once(cell.energy)
        .chain(cell.moments)
        .map(f32::to_bits)
        .collect()
}

/// `world`'s world-only checkpoint, read back.
fn round_trip(world: &World) -> World {
    let mut writer = CheckpointWriter::new(&world.config().config().model);
    world.write_checkpoint(&mut writer).expect("checkpoints");
    let bytes = writer.finish().expect("seals");
    let reader = CheckpointReader::parse(&bytes, &WORLD_CHECKPOINT_CHUNKS).expect("parses");
    World::read_checkpoint(&reader, profile()).expect("restores")
}

/// A disk whose centre is one world unit past the organism with the
/// largest `x`, wrapped: it crosses the seam.
fn across_the_seam(world: &World) -> ([f64; 2], f64) {
    let rightmost = world
        .population()
        .iter()
        .max_by(|a, b| a.position[0].total_cmp(&b.position[0]))
        .expect("a live world");
    (
        [
            (rightmost.position[0] + 1.0).rem_euclid(SIDE),
            rightmost.position[1],
        ],
        3.0,
    )
}

/// A crop holds exactly the disk: the organisms in it, row for row and
/// genome for genome, and the cells whose centres lie in it, every plane
/// bit for bit; every other cell is zero. It keeps the tick, epoch and
/// identity allocators, measures as the source does, and its ledger opens
/// at exactly its own holdings.
#[test]
fn a_crop_holds_exactly_the_disk_and_nothing_else() {
    let config = ecology();
    let world = world_at(&config, 75);
    let (seam, seam_radius) = across_the_seam(&world);
    for (center, radius) in [
        (seam, seam_radius),
        ([16.0, 16.0], 9.5),
        ([0.25, 31.75], 6.0),
    ] {
        let (crop, measurement) = world.cropped(center, radius).expect("a world disk");
        assert_eq!(
            measurement,
            world.measure_region(center, radius).expect("measures")
        );
        assert_eq!((crop.tick(), crop.epoch()), (world.tick(), world.epoch()));

        let expected: Vec<Organism> = world
            .population()
            .iter()
            .filter(|organism| inside(center, radius, organism.position))
            .collect();
        let held: Vec<Organism> = crop.population().iter().collect();
        assert_eq!(held, expected, "{center:?} r {radius}");
        assert_eq!(held.len() as u64, measurement.organisms);
        assert!(!held.is_empty(), "{center:?} r {radius} holds organisms");
        for organism in &held {
            assert_eq!(
                crop.population().genome(organism.id),
                world.population().genome(organism.id),
                "{}'s genome",
                organism.id
            );
        }

        let mut cells = 0_u64;
        for y in 0..32_u32 {
            for x in 0..32_u32 {
                let at = [f64::from(x) + 0.5, f64::from(y) + 0.5];
                let cropped = bits(crop.field().cell(x, y).expect("a cell"));
                if inside(center, radius, at) {
                    cells += 1;
                    assert_eq!(cropped, bits(world.field().cell(x, y).expect("a cell")));
                } else {
                    assert!(cropped.iter().all(|value| *value == 0), "({x}, {y})");
                }
            }
        }
        assert_eq!(cells, measurement.cells);

        let organisms: f64 = held.iter().map(|organism| organism.energy).sum();
        let holdings = crop.ledger().holdings();
        assert_eq!(
            holdings.field().to_bits(),
            crop.field().energy_total().to_bits()
        );
        assert!((holdings.organisms() - organisms).abs() <= 1.0e-12 * organisms);
        assert!(
            (holdings.field() - measurement.field_energy).abs()
                <= 1.0e-9 * measurement.field_energy,
            "{holdings:?} {measurement:?}"
        );
        assert_ne!(crop.digest(), world.digest());
        assert_eq!(
            [crop.quota().live(), crop.population().len() as u64],
            [measurement.organisms; 2]
        );
    }
}

/// A crop's checkpoint restores to the same world, digest for digest, and
/// the crop runs on from the source's tick: its ledger closes every tick
/// from its fresh opening, and the restored copy stays identical.
#[test]
fn a_crop_restores_from_its_checkpoint_and_runs_on() {
    let config = ecology();
    let world = world_at(&config, 60);
    let (mut crop, _) = world.cropped([16.0, 16.0], 9.5).expect("a world disk");
    let mut restored = round_trip(&crop);
    assert_eq!(restored.digest(), crop.digest());
    assert_eq!(
        restored.ledger().holdings().total().to_bits(),
        crop.ledger().holdings().total().to_bits()
    );
    for _ in 0..40 {
        step(&mut crop);
        step(&mut restored);
        assert_eq!(
            restored.digest(),
            crop.digest(),
            "tick {}",
            crop.tick().get()
        );
    }
    assert_eq!(crop.tick().get(), 100);
    assert!(
        !crop.population().is_empty(),
        "the crop's organisms live on"
    );
}

/// A disk a measurement refuses is refused as a crop.
#[test]
fn a_crop_of_a_disk_a_measurement_refuses_is_refused() {
    let config = ecology();
    let world = world_at(&config, 10);
    for (center, radius, quantity) in [
        ([-0.1, 5.0], 1.0, "measurement centre"),
        ([5.0, 5.0], 0.0, "measurement radius"),
        ([5.0, 5.0], SIDE / 2.0 + 1.0e-9, "measurement radius"),
    ] {
        let refused = world.cropped(center, radius);
        assert!(
            matches!(
                &refused,
                Err(CoreError::NumericalInvalid { quantity: named, .. }) if *named == quantity
            ),
            "{center:?} r {radius}: {:?}",
            refused.as_ref().map(|(_, measurement)| measurement)
        );
    }
}
