//! FL-042 evidence (plan §14.5): `World::graft` puts a local package's
//! matter into a founding host.
//!
//! The oracles are this file's own: minimum-image displacements computed
//! with `rem_euclid` on the package's torus, positions and cells placed on
//! the host's torus the same way, and the host's and the package's
//! populations, genomes and fields read directly before the graft.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::collections::BTreeMap;

use flife_core::checkpoint::{CheckpointReader, CheckpointWriter};
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{
    Arrangement, Graft, HeldOutput, NumericalProfile, Organism, Stepped, WORLD_CHECKPOINT_CHUNKS,
    World,
};
use flife_field::Cell;

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// The controllers' evaluation interval in [`ecology`].
const INTERVAL: u64 = 4;

/// Appendix A's 32 × 32 test ecology with `extra` overrides.
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
        // Controllers every [`INTERVAL`] ticks, so a founder's phase depends
        // on its identity.
        ("controller", "evaluation_interval_ticks", "4"),
    ];
    overrides.extend_from_slice(extra);
    for (section, key, value) in overrides {
        // An empty section names a top-level key, before the first header.
        let first = if section.is_empty() {
            0
        } else {
            let header = format!("[{section}]");
            lines
                .iter()
                .position(|line| line.trim() == header)
                .expect("a section")
                + 1
        };
        let line = lines[first..]
            .iter()
            .take_while(|line| !line.starts_with('['))
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .map(|offset| first + offset)
            .expect("a key");
        lines[line] = format!("{key} = {value}");
    }
    ValidatedConfig::from_toml(&lines.join("\n")).expect("a valid configuration")
}

/// A 48 × 48 host of one-unit cells, seeded otherwise.
fn wide() -> ValidatedConfig {
    ecology(&[
        ("world", "width", "48.0"),
        ("world", "height", "48.0"),
        ("world", "grid_width", "48"),
        ("world", "grid_height", "48"),
        ("", "seed", "7"),
    ])
}

fn profile() -> NumericalProfile {
    NumericalProfile::reference_f32().expect("profile")
}

fn found(config: &ValidatedConfig) -> World {
    World::found(config, Arrangement::FreeList, profile()).expect("founds")
}

fn step(world: &mut World) {
    match world.step().expect("the tick advances") {
        Stepped::Published(_) => {}
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    }
}

fn world_at(config: &ValidatedConfig, tick: u64) -> World {
    let mut world = found(config);
    while world.tick().get() < tick {
        step(&mut world);
    }
    world
}

/// The minimum-image component `to − from` on a period of `side`.
fn wrapped(from: f64, to: f64, side: f64) -> f64 {
    (to - from + side / 2.0).rem_euclid(side) - side / 2.0
}

fn inside(center: [f64; 2], radius: f64, position: [f64; 2], side: f64) -> bool {
    let dx = wrapped(center[0], position[0], side);
    let dy = wrapped(center[1], position[1], side);
    dx * dx + dy * dy <= radius * radius
}

fn bits(cell: Cell) -> Vec<u32> {
    std::iter::once(cell.energy)
        .chain(cell.moments)
        .map(f32::to_bits)
        .collect()
}

fn round_trip(world: &World) -> World {
    let mut writer = CheckpointWriter::new(&world.config().config().model);
    world.write_checkpoint(&mut writer).expect("checkpoints");
    let bytes = writer.finish().expect("seals");
    let reader = CheckpointReader::parse(&bytes, &WORLD_CHECKPOINT_CHUNKS).expect("parses");
    World::read_checkpoint(&reader, profile()).expect("restores")
}

/// Graft the crop of `center`/`radius` of the 32 × 32 source at tick 75
/// into a founding host of `host` moved by `offset`, and check it against
/// the oracle: the field, the host's survivors, the grafted founders and
/// their genomes, the holdings, and a checkpoint that restores and runs on
/// identically.
fn grafts_as_the_oracle_says(
    host: &ValidatedConfig,
    center: [f64; 2],
    radius: f64,
    offset: [i64; 2],
) {
    let source = world_at(&ecology(&[]), 75);
    let (package, _) = source.cropped(center, radius).expect("a crop");
    let before = found(host);
    let mut world = found(host);
    let report: Graft = world
        .graft(&package, center, radius, offset)
        .expect("grafts");

    let side = world.field().grid().world_width();
    let cells = world.field().grid().cells_x();
    let target = [0, 1].map(|axis| (center[axis] + offset[axis] as f64).rem_euclid(side));
    assert_eq!(report.center, target);
    let place = |position: [f64; 2]| {
        [0, 1].map(|axis| {
            (target[axis] + wrapped(center[axis], position[axis], 32.0)).rem_euclid(side)
        })
    };

    // The field: package cells to their places, the rest the host's.
    let mut landed = BTreeMap::new();
    for y in 0..32_u32 {
        for x in 0..32_u32 {
            let at = [f64::from(x) + 0.5, f64::from(y) + 0.5];
            if inside(center, radius, at, 32.0) {
                let [hx, hy] = place(at).map(|value| value.floor() as u32);
                assert!(
                    landed.insert((hx, hy), (x, y)).is_none(),
                    "one cell per place"
                );
            }
        }
    }
    assert_eq!(report.cells, landed.len() as u64);
    let mut displaced_field = 0.0;
    for y in 0..cells {
        for x in 0..cells {
            let grafted = bits(world.field().cell(x, y).expect("a cell"));
            let host_cell = before.field().cell(x, y).expect("a cell");
            match landed.get(&(x, y)) {
                Some(&(px, py)) => {
                    displaced_field += f64::from(host_cell.energy);
                    assert_eq!(grafted, bits(package.field().cell(px, py).expect("a cell")));
                }
                None => assert_eq!(grafted, bits(host_cell)),
            }
        }
    }
    assert!((report.displaced_field - displaced_field).abs() <= 1e-12 * displaced_field.max(1.0));

    // The host's founders outside the disk, unchanged; those in it, gone.
    let survivors: Vec<Organism> = before
        .population()
        .iter()
        .filter(|organism| !inside(target, radius, organism.position, side))
        .collect();
    let displaced: Vec<Organism> = before
        .population()
        .iter()
        .filter(|organism| inside(target, radius, organism.position, side))
        .collect();
    assert_eq!(report.displaced_organisms, displaced.len() as u64);
    let rows: Vec<Organism> = world.population().iter().collect();
    assert_eq!(&rows[..survivors.len()], survivors.as_slice());
    for organism in &survivors {
        assert_eq!(
            world.population().genome(organism.id),
            before.population().genome(organism.id)
        );
    }

    // The package's organisms as founders with the host's next identities.
    let founder = before.population().iter().next().expect("a founder");
    let grafted = &rows[survivors.len()..];
    let packed: Vec<Organism> = package.population().iter().collect();
    assert_eq!(grafted.len(), packed.len());
    assert!(!packed.is_empty(), "the crop holds organisms");
    assert_eq!(report.organisms, packed.len() as u64);
    let first = before.population().latest().expect("founders").get() + 1;
    let mut lineages = BTreeMap::new();
    for (index, (new, old)) in grafted.iter().zip(&packed).enumerate() {
        let id = first + index as u64;
        assert_eq!(new.id.get(), id);
        let lineage = *lineages.entry(old.lineage).or_insert(id);
        assert_eq!(new.lineage.get(), lineage, "kin stay kin");
        assert_eq!(
            (new.parent, new.generation.get(), new.birth_tick.get()),
            (None, 0, 0)
        );
        assert_eq!(new.cooldown_deadline, founder.cooldown_deadline);
        assert_eq!(
            u64::from(new.phase),
            id % INTERVAL,
            "a founder's stable phase"
        );
        assert_eq!(new.held, HeldOutput::INVALID);
        assert_eq!(new.energy_change_rate, 0.0);
        assert_eq!(
            (new.velocity, new.energy.to_bits()),
            (old.velocity, old.energy.to_bits())
        );
        let placed = place(old.position);
        for axis in 0..2 {
            assert!(
                (new.position[axis] - placed[axis]).abs() < 1e-9,
                "{} at {:?}, not {placed:?}",
                new.id,
                new.position
            );
        }
        let genome = world.population().genome(new.id).expect("a genome");
        let original = package.population().genome(old.id).expect("a genome");
        assert_eq!(genome.id(), new.genome);
        assert_eq!(genome.parent(), None);
        assert_eq!(genome.topology(), original.topology());
        assert_eq!(genome.traits(), original.traits());
        assert_eq!(genome.parameters(), original.parameters());
    }
    assert_eq!(report.lineages, lineages.len() as u64);

    // Holdings: the host's, less what was displaced, plus the endowment.
    let expected =
        report.before.total() - report.displaced_field - report.displaced_organism_energy
            + report.endowment.total();
    assert!(
        (report.after.total() - expected).abs() <= 1e-9 * expected,
        "{report:?}"
    );
    assert_eq!(
        world.ledger().holdings().total().to_bits(),
        report.after.total().to_bits()
    );
    assert_eq!(
        report.endowment.total().to_bits(),
        package.ledger().holdings().total().to_bits()
    );

    // It is a world: its checkpoint restores and runs on identically.
    let mut restored = round_trip(&world);
    assert_eq!(restored.digest(), world.digest());
    for _ in 0..30 {
        step(&mut world);
        step(&mut restored);
        assert_eq!(restored.digest(), world.digest());
    }
}

/// Into a host of the source's size, moved 8 cells east and 4 north.
#[test]
fn a_crop_grafts_into_a_host_of_its_size() {
    grafts_as_the_oracle_says(&ecology(&[("", "seed", "7")]), [16.0, 16.0], 6.0, [8, -4]);
}

/// Into a wider host, across both of the source's seams: a cell or an
/// organism the source wrapped lands beside the disk's centre in the host,
/// not a source-width away.
#[test]
fn a_crop_across_the_seams_grafts_into_a_wider_host() {
    grafts_as_the_oracle_says(&wide(), [0.5, 0.5], 5.0, [20, 30]);
}

/// Refused: a host that has run, cells of another size, a package holding
/// matter outside its disk, cells that would land twice, and too many
/// organisms for the host. Each leaves the host as it was.
#[test]
fn a_refused_graft_leaves_the_host_as_it_was() {
    let source = world_at(&ecology(&[]), 75);
    let (package, _) = source.cropped([16.0, 16.0], 6.0).expect("a crop");
    // `why` is a fragment of the refusal's detail, so each case is refused
    // for its own reason and not an earlier one.
    let refused = |host: &mut World, package: &World, center: [f64; 2], radius: f64, why: &str| {
        let digest = host.digest();
        let holdings = host.ledger().holdings().total().to_bits();
        let rows: Vec<Organism> = host.population().iter().collect();
        let result = host.graft(package, center, radius, [0, 0]);
        let matched = match &result {
            Err(CoreError::StateConflict { detail, .. }) => detail.contains(why),
            Err(CoreError::BudgetExhausted { .. }) => why == "budget-exhausted",
            _ => false,
        };
        assert!(matched, "{why}: {:?}", result.as_ref().err());
        assert_eq!(host.digest(), digest, "the host is unchanged");
        assert_eq!(host.ledger().holdings().total().to_bits(), holdings);
        assert_eq!(host.population().iter().collect::<Vec<_>>(), rows);
    };

    let mut ran = found(&ecology(&[]));
    step(&mut ran);
    refused(
        &mut ran,
        &package,
        [16.0, 16.0],
        6.0,
        "only a founding world",
    );

    let mut halves = found(&ecology(&[
        ("world", "grid_width", "64"),
        ("world", "grid_height", "64"),
    ]));
    refused(
        &mut halves,
        &package,
        [16.0, 16.0],
        6.0,
        "the package's cells are",
    );

    let mut halved = found(&ecology(&[(
        "controller",
        "canonical_weight_dtype",
        "\"f16\"",
    )]));
    refused(&mut halved, &package, [16.0, 16.0], 6.0, "parameter type");

    let mut host = found(&ecology(&[("", "seed", "7")]));
    refused(
        &mut host,
        &source,
        [16.0, 16.0],
        6.0,
        "outside the package's disk",
    );

    // A disk 17 cells across on a host 16 cells wide: its row through the
    // centre wraps onto itself.
    let (half, _) = source.cropped([16.5, 16.5], 8.0).expect("a crop");
    let mut narrow = found(&ecology(&[
        ("world", "width", "16.0"),
        ("world", "height", "16.0"),
        ("world", "grid_width", "16"),
        ("world", "grid_height", "16"),
    ]));
    refused(
        &mut narrow,
        &half,
        [16.5, 16.5],
        8.0,
        "land on the host's cell",
    );

    let mut full = found(&ecology(&[("population", "maximum_count", "48")]));
    let crowded = [16.0, 16.0];
    assert!(
        full.measure_region(crowded, 6.0)
            .expect("measures")
            .organisms
            < package.population().len() as u64,
        "more arrive than leave"
    );
    refused(&mut full, &package, crowded, 6.0, "budget-exhausted");
}
