//! FL-021 evidence (plan §14.2's Descendants, §21.2 INV-09): a world that
//! traces its ancestry gives each live organism's snapshot its children and
//! descendants, born and alive, and the ancestors its line passes through;
//! tracing changes nothing a run computes; and the index stays in the room
//! reserved for it, which the world's footprint charges.
//!
//! The oracle is not the index: it keeps every organism the run ever had,
//! from the founding population and each published report (births with
//! their parents, deaths, introductions), never pruning, and works out each
//! live organism's counts and the ancestors on its line by walking whole
//! parent chains. An ancestor is on the line when it lives, or when two or
//! more of its children's lines still have live members.
//!
//! The world is Appendix A at 32 × 32 world units with 48 founders, eight
//! rich sources and cheap upkeep, so it breeds for generations; a removal
//! brush every 100 ticks kills whole neighbourhoods, ending lines, and an
//! introduction adds founders of new lineages.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stdout,
    reason = "test helpers: a failure in one is a test failure; figures are printed"
)]

use std::collections::BTreeMap;

use flife_core::ids::{Generation, OrganismId};
use flife_core::model::FIELD_ECOLOGY_V1;
use flife_core::{Tick, ValidatedConfig};
use flife_engine::{
    ANCESTRY_PATH, Ancestor, Arrangement, Descendants, Disposition, Intervention, NumericalProfile,
    ROOM_PER_ORGANISM, Stepped, TickReport, World,
};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// The most organisms the test world holds.
const MAXIMUM: usize = 512;

/// Appendix A at 32 × 32 world units with 48 founders, at most
/// [`MAXIMUM`], eight sources and cheap upkeep: it breeds for generations.
fn config() -> ValidatedConfig {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    let mut lines: Vec<String> = appendix[start..start + length]
        .lines()
        .map(str::to_owned)
        .collect();
    let maximum = MAXIMUM.to_string();
    for (section, key, value) in [
        ("world", "width", "32.0"),
        ("world", "height", "32.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", "48"),
        ("population", "maximum_count", maximum.as_str()),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "8"),
        ("sources", "power_per_source", "240.0"),
        ("sources", "sigma_world_units", "3.0"),
        ("sources", "path_direction_interval_seconds", "2.0"),
        ("chemistry", "uptake_capacity_per_second", "8.0"),
        ("costs", "basal_energy_per_second", "0.2"),
        (
            "reproduction",
            "minimum_parent_energy_before_attempt",
            "1.6",
        ),
        ("reproduction", "maximum_attempt_rate_per_second", "4.0"),
        ("reproduction", "cooldown_seconds", "0.1"),
        ("controller", "evaluation_interval_ticks", "1"),
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
    lines[model_line] = format!("model = \"{FIELD_ECOLOGY_V1}\"");
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

/// The brushes sealed at the boundary of committed tick `tick`: a removal
/// of every organism within five world units of one of four places every
/// 100 ticks, and six founders introduced at tick 150.
fn brushes(tick: u64) -> Vec<Intervention> {
    let mut brushes = Vec::new();
    if tick % 100 == 50 {
        let places = [[8.0, 8.0], [24.0, 8.0], [24.0, 24.0], [8.0, 24.0]];
        brushes.push(Intervention::RemoveOrganisms {
            center: places[(tick / 100 % 4) as usize],
            radius: 5.0,
            disposition: Disposition::Recycle,
        });
    }
    if tick == 150 {
        brushes.push(Intervention::IntroduceOrganisms {
            cell: [15, 17],
            count: 6,
            energy: 2.5,
        });
    }
    brushes
}

fn step(world: &mut World) -> TickReport {
    let brushes = brushes(world.tick().get());
    match world.step_with(&brushes).expect("steps") {
        Stepped::Published(report) => *report,
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    }
}

/// One organism the oracle knows.
#[derive(Debug, Clone, Copy)]
struct Known {
    /// Its parent when the oracle knows it: alive at `since` or later.
    parent: Option<OrganismId>,
    generation: Generation,
    birth_tick: Tick,
    /// Born after `since`.
    counted: bool,
    alive: bool,
}

/// Every organism alive at `since` or born or introduced after it.
struct Genealogy {
    since: Tick,
    known: BTreeMap<OrganismId, Known>,
}

impl Genealogy {
    /// From `world`'s live population: each under its parent when that
    /// parent lives too, none counted.
    fn begin(world: &World) -> Self {
        let population = world.population();
        let known = population
            .iter()
            .map(|organism| {
                (
                    organism.id,
                    Known {
                        parent: organism
                            .parent
                            .filter(|&parent| population.contains(parent)),
                        generation: organism.generation,
                        birth_tick: organism.birth_tick,
                        counted: false,
                        alive: true,
                    },
                )
            })
            .collect();
        Self {
            since: world.tick(),
            known,
        }
    }

    /// Fold a published report, checking each newcomer's row in `world`,
    /// which published it.
    fn fold(&mut self, report: &TickReport, world: &World) {
        for birth in &report.births {
            let parent = self.known[&birth.parent];
            assert!(parent.alive, "{} bore a child dead", birth.parent);
            let row = world.population().get(birth.child).expect("a live child");
            assert_eq!(row.parent, Some(birth.parent));
            assert_eq!(
                row.generation,
                parent.generation.child().expect("a generation")
            );
            assert_eq!(row.birth_tick, report.tick);
            self.known.insert(
                birth.child,
                Known {
                    parent: Some(birth.parent),
                    generation: row.generation,
                    birth_tick: row.birth_tick,
                    counted: true,
                    alive: true,
                },
            );
        }
        for death in &report.deaths {
            self.known
                .get_mut(&death.id)
                .expect("a known organism")
                .alive = false;
        }
        for introduction in &report.introductions {
            let row = world
                .population()
                .get(introduction.organism)
                .expect("a live introduction");
            assert_eq!(row.parent, None);
            self.known.insert(
                introduction.organism,
                Known {
                    parent: None,
                    generation: row.generation,
                    birth_tick: row.birth_tick,
                    counted: true,
                    alive: true,
                },
            );
        }
    }

    /// What a snapshot must say of every live organism.
    fn expected(&self) -> BTreeMap<OrganismId, Descendants> {
        let mut children: BTreeMap<OrganismId, Vec<OrganismId>> = BTreeMap::new();
        for (&id, known) in &self.known {
            if let Some(parent) = known.parent {
                children.entry(parent).or_default().push(id);
            }
        }
        // (born below, living below, lines with live members); a child's
        // identity is above its parent's, so descending order sees children
        // first.
        let mut below: BTreeMap<OrganismId, (u64, u64, u32)> = BTreeMap::new();
        for &id in self.known.keys().rev() {
            let (mut born, mut living, mut lines) = (0, 0, 0);
            for child in children.get(&id).into_iter().flatten() {
                let known = &self.known[child];
                let (child_born, child_living, child_lines) = below[child];
                born += child_born + u64::from(known.counted);
                living += child_living + u64::from(known.alive);
                if known.alive || child_lines > 0 {
                    lines += 1;
                }
            }
            below.insert(id, (born, living, lines));
        }
        let mut expected = BTreeMap::new();
        for (&organism, known) in &self.known {
            if !known.alive {
                continue;
            }
            let mine = children.get(&organism).map_or(&[][..], Vec::as_slice);
            let mut path = Vec::new();
            let mut parent = known.parent;
            while let Some(id) = parent {
                let ancestor = &self.known[&id];
                let (born, living, lines) = below[&id];
                if ancestor.alive || lines >= 2 {
                    path.push(Ancestor {
                        organism: id,
                        generation: ancestor.generation,
                        birth_tick: ancestor.birth_tick,
                        alive: ancestor.alive,
                        born,
                        living,
                    });
                }
                parent = ancestor.parent;
            }
            let beyond = path.len().saturating_sub(ANCESTRY_PATH) as u64;
            path.truncate(ANCESTRY_PATH);
            let (born, living, _) = below[&organism];
            let count = |keep: fn(&Known) -> bool| {
                mine.iter().filter(|child| keep(&self.known[child])).count() as u64
            };
            expected.insert(
                organism,
                Descendants {
                    since: self.since,
                    children_born: count(|known| known.counted),
                    children_living: count(|known| known.alive),
                    born,
                    living,
                    path,
                    beyond,
                },
            );
        }
        expected
    }
}

/// What the checks saw.
#[derive(Debug, Default)]
struct Seen {
    checks: usize,
    organisms: usize,
    most_living: u64,
    longest_path: usize,
    dead_ancestors: usize,
    births: usize,
    deaths: usize,
}

/// Step `world` to committed tick `until`, folding each report into
/// `genealogy`, and every 25 ticks check every live organism's snapshot
/// against it.
fn run(world: &mut World, genealogy: &mut Genealogy, until: u64, seen: &mut Seen) {
    while world.tick().get() < until {
        let report = step(world);
        seen.births += report.births.len();
        seen.deaths += report.deaths.len();
        genealogy.fold(&report, world);
        if !world.tick().get().is_multiple_of(25) {
            continue;
        }
        let expected = genealogy.expected();
        assert_eq!(expected.len(), world.population().len());
        for (&id, expected) in &expected {
            let snapshot = world
                .organism_snapshot(id)
                .expect("inspects")
                .expect("a live organism");
            let descendants = snapshot.descendants.expect("the world traces its ancestry");
            assert_eq!(&descendants, expected, "tick {}: {id}", world.tick().get());
            seen.organisms += 1;
            seen.most_living = seen.most_living.max(descendants.living);
            seen.longest_path = seen.longest_path.max(descendants.path.len());
            seen.dead_ancestors += descendants
                .path
                .iter()
                .filter(|ancestor| !ancestor.alive)
                .count();
        }
        seen.checks += 1;
    }
}

/// A world traced from its founding: every live organism's snapshot, every
/// 25 ticks for 1,200, says what the whole genealogy says. Each birth's
/// child is its parent's next generation, born at the report's tick.
#[test]
fn a_snapshot_says_what_the_whole_genealogy_says() {
    let config = config();
    let mut world = found(&config);
    world.trace_ancestry().expect("traces");
    let mut genealogy = Genealogy::begin(&world);
    assert_eq!(genealogy.since, Tick::ZERO);
    let mut seen = Seen::default();
    run(&mut world, &mut genealogy, 1_200, &mut seen);
    println!("{seen:?}");
    // What the run exercised: births and deaths, a lineage with many live
    // descendants, a long line, and lines meeting at dead ancestors.
    assert_eq!(seen.checks, 48);
    assert!(seen.births > 200 && seen.deaths > 50, "{seen:?}");
    assert!(seen.most_living >= 20, "{seen:?}");
    assert!(seen.longest_path >= 4, "{seen:?}");
    assert!(seen.dead_ancestors > 0, "{seen:?}");
}

/// A world traced from the first tick past 300 at which some live
/// organism's parent has died starts from its live population: births
/// count from then, and that organism starts a line of its own. Before
/// tracing, a snapshot carries no descendants.
#[test]
fn a_world_traced_later_starts_from_its_live_population() {
    let config = config();
    let mut world = found(&config);
    let orphaned = |world: &World| {
        world.population().iter().any(|organism| {
            organism
                .parent
                .is_some_and(|parent| !world.population().contains(parent))
        })
    };
    while world.tick().get() < 300 || !orphaned(&world) {
        assert!(
            world.tick().get() < 1_200,
            "no live organism outlived its parent"
        );
        step(&mut world);
    }
    let since = world.tick();
    let id = world.population().ids().next().expect("an organism");
    let untraced = world
        .organism_snapshot(id)
        .expect("inspects")
        .expect("live");
    assert_eq!(untraced.descendants, None);
    assert_eq!(world.ancestry_held(), None);
    world.trace_ancestry().expect("traces");
    let mut genealogy = Genealogy::begin(&world);
    assert_eq!(genealogy.since, since);
    let mut seen = Seen::default();
    let until = since.get() + 500;
    run(&mut world, &mut genealogy, until, &mut seen);
    println!("traced from {since:?}: {seen:?}");
    assert_eq!(seen.checks as u64, until / 25 - since.get() / 25);
    assert!(seen.longest_path >= 2, "{seen:?}");
    // Tracing again keeps what the index has.
    world.trace_ancestry().expect("traces");
    assert_eq!(world.ancestry_held().expect("traced").since, since);
}

/// Tracing changes nothing a run computes: a traced world and a plain one
/// publish the same reports and digests, tick for tick. The index stays in
/// the room reserved for it, [`ROOM_PER_ORGANISM`] entries an organism the
/// population can hold, which the footprint's state and the memory
/// admission charge; it never holds more than twice the live population
/// less one, and its dead are fewer than its live.
#[test]
fn tracing_changes_nothing_and_stays_in_its_room() {
    let config = config();
    let mut plain = found(&config);
    let mut traced = found(&config);
    traced.trace_ancestry().expect("traces");
    let held = traced.ancestry_held().expect("traced");
    assert_eq!(held.room, MAXIMUM * ROOM_PER_ORGANISM);
    assert_eq!(held.members, 48);
    assert_eq!(
        traced.footprint().state,
        plain.footprint().state + held.bytes
    );
    let plain_admission = plain.admit_memory(false, 0, 0).expect("admits");
    let traced_admission = traced.admit_memory(false, 0, 0).expect("admits");
    assert_eq!(traced_admission.state, plain_admission.state + held.bytes);
    let bytes = held.bytes;
    let mut most = 0;
    while plain.tick().get() < 1_200 {
        let expected = step(&mut plain);
        let report = step(&mut traced);
        assert_eq!(report, expected, "tick {}", report.tick.get());
        assert_eq!(
            traced.digest(),
            plain.digest(),
            "tick {}",
            report.tick.get()
        );
        let held = traced.ancestry_held().expect("traced");
        let live = traced.population().len();
        assert_eq!(held.members - held.dead, live);
        assert!(held.members < 2 * live.max(1), "{held:?} for {live}");
        assert!(held.entries <= held.room);
        assert_eq!(held.room, MAXIMUM * ROOM_PER_ORGANISM);
        assert_eq!(held.bytes, bytes);
        most = most.max(held.dead);
    }
    println!("most dead ancestors held: {most}");
    assert!(most > 0);
}
