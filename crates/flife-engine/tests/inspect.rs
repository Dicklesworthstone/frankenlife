//! FL-021 evidence (plan §14.2, §21.2 INV-09): an organism snapshot is that
//! organism's committed row and its own genome at the world's tick, its held
//! output decoded as the world decodes it, and taking one changes nothing.
//!
//! The oracle reads the same world through its public accessors
//! (`Population::get`, `Population::genome`, the configuration's topology
//! catalog and action decoding), not through the snapshot.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use flife_core::ValidatedConfig;
use flife_core::ids::OrganismId;
use flife_core::model::FIELD_ECOLOGY_V1_CHEMISTRY_ONLY;
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_genome::TopologyCatalog;
use flife_neural::{ACTION_WIDTH, ActionDecoding, decode, widen};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// Appendix A at 32 × 32 world units with 48 founders and FL-013's small
/// ecology (four richer sources, cheaper upkeep, readier reproduction), so
/// it breeds within tens of ticks, under `model`.
fn small(model: &str) -> ValidatedConfig {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    let mut lines: Vec<String> = appendix[start..start + length]
        .lines()
        .map(str::to_owned)
        .collect();
    for (section, key, value) in [
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
    ] {
        let header = format!("[{section}]");
        let at = lines
            .iter()
            .position(|line| line.trim() == header)
            .unwrap_or_else(|| panic!("no section {header}"));
        let line = lines[at + 1..]
            .iter()
            .take_while(|line| !line.starts_with('['))
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .map(|offset| at + 1 + offset)
            .unwrap_or_else(|| panic!("no key {key} in {header}"));
        lines[line] = format!("{key} = {value}");
    }
    let model_line = lines
        .iter()
        .position(|line| line.starts_with("model = "))
        .expect("the model key");
    lines[model_line] = format!("model = \"{model}\"");
    ValidatedConfig::from_toml(&lines.join("\n")).expect("a valid configuration")
}

fn found(config: &ValidatedConfig) -> World {
    World::found(
        config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds")
}

fn step(world: &mut World) {
    assert!(matches!(
        world.step().expect("steps"),
        Stepped::Published(_)
    ));
}

/// Every live organism's snapshot against the world's public accessors.
fn check_every_snapshot(world: &World, config: &ValidatedConfig, chemistry_only: bool) {
    let catalog = TopologyCatalog::from_config(config).expect("catalog");
    let decoding = ActionDecoding::from_config(config).expect("decoding");
    let population = world.population();
    let mut held = 0;
    for id in population.ids() {
        let snapshot = world
            .organism_snapshot(id)
            .expect("inspects")
            .expect("a live organism");
        let organism = population.get(id).expect("live");
        let genome = population.genome(id).expect("its genome");
        assert_eq!(snapshot.tick, world.tick());
        assert_eq!(snapshot.organism, organism);
        assert_eq!(
            snapshot.age_ticks,
            world.tick().get() - organism.birth_tick.get()
        );
        assert_eq!(snapshot.topology, genome.topology().index());
        assert_eq!(
            snapshot.hidden_widths,
            catalog
                .topology(genome.topology())
                .expect("in the catalog")
                .hidden_widths()
        );
        assert_eq!(snapshot.parameters, genome.parameters().len());
        assert_eq!(snapshot.traits, *genome.traits());
        let expected = organism.held.outputs().map(|outputs| {
            let raw = if chemistry_only {
                [0.0; ACTION_WIDTH]
            } else {
                widen(outputs)
            };
            decode(&raw, world.step_size(), &decoding).expect("decodes")
        });
        assert_eq!(snapshot.decoded, expected);
        held += usize::from(snapshot.decoded.is_some());
    }
    assert!(held > 0, "some organism holds an output");
}

#[test]
fn an_organism_snapshot_is_its_committed_row_and_genome() {
    let config = small(flife_core::model::FIELD_ECOLOGY_V1);
    let mut world = found(&config);
    // Until children born at two different ticks live beside founders.
    let births = |world: &World| {
        world
            .population()
            .iter()
            .filter(|organism| organism.parent.is_some())
            .map(|organism| organism.birth_tick)
            .collect::<std::collections::BTreeSet<_>>()
    };
    while births(&world).len() < 2 {
        assert!(world.tick().get() < 400, "no births by tick 400");
        step(&mut world);
    }
    for _ in 0..3 {
        step(&mut world);
    }
    check_every_snapshot(&world, &config, false);
    let tick = world.tick().get();
    let children: Vec<_> = world
        .population()
        .iter()
        .filter(|organism| organism.parent.is_some())
        .collect();
    assert!(!children.is_empty());
    for child in children {
        let snapshot = world
            .organism_snapshot(child.id)
            .expect("inspects")
            .expect("live");
        assert!(child.birth_tick.get() > 0);
        assert_eq!(snapshot.age_ticks, tick - child.birth_tick.get());
        assert!(snapshot.age_ticks < tick);
    }
    // An identity the population does not hold: none.
    assert_eq!(world.organism_snapshot(OrganismId::new(1 << 40)), Ok(None));
    // Founders were born at tick 0, so their age is the tick.
    let founder = world
        .population()
        .iter()
        .find(|organism| organism.parent.is_none())
        .expect("a founder");
    let snapshot = world
        .organism_snapshot(founder.id)
        .expect("inspects")
        .expect("live");
    assert_eq!(snapshot.age_ticks, tick);
    assert_eq!(snapshot.traits.affinity().len(), 32);
}

/// The chemistry-only ablation decodes every held output from zero, and so
/// does its snapshot.
#[test]
fn a_chemistry_only_snapshot_decodes_from_zero() {
    let config = small(FIELD_ECOLOGY_V1_CHEMISTRY_ONLY);
    let mut world = found(&config);
    for _ in 0..6 {
        step(&mut world);
    }
    check_every_snapshot(&world, &config, true);
}

/// INV-09: two worlds from one founding, one inspected in full before and
/// after every tick, commit the same digests.
#[test]
fn inspecting_changes_nothing() {
    let config = small(flife_core::model::FIELD_ECOLOGY_V1);
    let mut inspected = found(&config);
    let mut plain = found(&config);
    for _ in 0..20 {
        let before = inspected.digest();
        let ids: Vec<OrganismId> = inspected.population().ids().collect();
        for id in ids {
            assert!(inspected.organism_snapshot(id).expect("inspects").is_some());
        }
        assert_eq!(
            inspected.organism_snapshot(OrganismId::new(1 << 40)),
            Ok(None)
        );
        assert_eq!(inspected.digest(), before);
        step(&mut inspected);
        step(&mut plain);
        assert_eq!(inspected.digest(), plain.digest());
        assert_eq!(inspected.tick(), plain.tick());
    }
}
