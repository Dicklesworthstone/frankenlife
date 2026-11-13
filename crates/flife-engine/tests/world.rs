//! FL-013 evidence (plan §8.1–§8.4, §9.5, §21.2 INV-01–INV-08, §21.3 G1,
//! Appendix A.3 item 1, C.7, C.8): the ordered candidate tick and epoch
//! publication, run as a complete small headless ecology.
//!
//! Every world here is founded from Appendix A with a few values changed,
//! so that births, mutations, emissions and deaths all happen within a few
//! hundred ticks of a 32 × 32 world. The oracles are independent of the
//! engine: they recount what the published state must contain from the
//! state itself, the plan's rules and the run's own reports.

use std::collections::{BTreeMap, BTreeSet};

use flife_core::ids::OrganismId;
use flife_core::ledger::CompensatedSum;
use flife_core::ledger::Flow;
use flife_core::{CoreError, Tick, Transfer, ValidatedConfig};
use flife_engine::{
    Advance, Arrangement, BudgetStage, Capacity, DeathCause, NumericalProfile, Organism,
    STOP_AND_PRESERVE, Stepped, TickReport, World,
};
use flife_field::ConeTolerance;
use flife_genome::{GenomeQuota, TopologyCatalog};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

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
fn config(overrides: &[(&str, &str, &str)]) -> ValidatedConfig {
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

/// A small, fast world: 32 × 32 with four sources. Near them there is
/// enough food for births within a few hundred ticks; away from them there
/// is too little, so organisms die. Founders start above the attempt
/// minimum, so the first births follow their cooldown.
fn ecology(extra: &[(&str, &str, &str)]) -> ValidatedConfig {
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
    ];
    overrides.extend_from_slice(extra);
    config(&overrides)
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn found(config: &ValidatedConfig, arrangement: Arrangement) -> World {
    World::found(
        config,
        arrangement,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("the world founds")
}

#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
fn publish(world: &mut World) -> TickReport {
    match world.step().expect("the tick advances") {
        Stepped::Published(report) => *report,
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    }
}

fn rows(world: &World) -> BTreeMap<OrganismId, Organism> {
    world
        .population()
        .iter()
        .map(|organism| (organism.id, organism))
        .collect()
}

/// `Σ E` in identity order, compensated, as the ledger measures it.
fn organism_holdings(world: &World) -> f64 {
    let mut total = CompensatedSum::ZERO;
    for organism in world.population().iter() {
        total.add(organism.energy);
    }
    total.value()
}

/// Everything committed that a cancelled candidate must leave alone.
fn committed(world: &World) -> (String, Tick, u64, u64, String, String, String) {
    (
        world.digest().to_hex(),
        world.tick(),
        world.epoch().get(),
        world.events(),
        format!("{:?}", world.quota()),
        format!("{:?}", world.identities()),
        format!("{:?}", world.ledger().report()),
    )
}

/// Totals over a run, for the ecology's assertions.
#[derive(Debug, Default)]
struct Census {
    births: usize,
    deaths: usize,
    emitted: usize,
    perturbed: u64,
    descendants_born_of_descendants: usize,
    descendant_deaths: usize,
    stages: BTreeMap<String, usize>,
    motion_paid: f64,
    motion_exact_ticks: usize,
}

/// One tick's checks of INV-01–INV-07 against the published state and its
/// report, given the rows `before` the tick.
#[expect(
    clippy::too_many_lines,
    clippy::expect_used,
    clippy::panic,
    reason = "test helper, one invariant per block in register order: a failure here is a test failure"
)]
fn check_tick(
    world: &World,
    before: &BTreeMap<OrganismId, Organism>,
    report: &TickReport,
    dead: &mut BTreeSet<OrganismId>,
    census: &mut Census,
    motion_cost: f64,
) {
    let dt = world.step_size().seconds_per_tick();
    let after = rows(world);
    assert_eq!(report.tick, report.from.next().expect("next"));
    assert_eq!(world.tick(), report.tick);
    assert_eq!(world.epoch(), report.epoch);
    assert_eq!(world.epoch().get(), report.tick.get(), "one epoch per tick");
    assert_eq!(after.len(), report.live);
    assert_eq!(
        report.events.end - report.events.start,
        (report.births.len() + report.deaths.len()) as u64
    );
    assert_eq!(world.events(), report.events.end);

    // INV-01: finite committed field energy and finite, strictly positive
    // organism energy.
    for organism in after.values() {
        assert!(
            organism.energy.is_finite() && organism.energy > 0.0,
            "{organism:?}"
        );
        let [x, y] = organism.position;
        assert!(
            (0.0..32.0).contains(&x) && (0.0..32.0).contains(&y),
            "{organism:?}"
        );
        assert!(organism.velocity.iter().all(|v| v.is_finite()));
    }
    assert!(
        world
            .field()
            .energy()
            .iter()
            .all(|e| e.is_finite() && *e >= 0.0)
    );
    // INV-02: the published field passes the cone check independently.
    let tolerance = ConeTolerance::new(f64::from(f32::MIN_POSITIVE), 1.0e-5).expect("tolerance");
    let summary = world.field().check_invariants(tolerance).expect("INV-02");
    assert_eq!(summary, report.field);

    // INV-03: the tick closed against the published holdings, measured here
    // independently, and the run's residual is within the gate.
    let closing = report.closure.closing();
    assert_eq!(
        closing.field().to_bits(),
        world.field().energy_total().to_bits()
    );
    assert_eq!(
        closing.organisms().to_bits(),
        organism_holdings(world).to_bits()
    );
    assert!(report.closure.residual().abs() <= report.closure.allowance());
    let normalized = report.ledger.normalized.expect("turnover");
    assert!(normalized.abs() < 1.0e-5, "{:?}", report.ledger);

    // INV-04: nothing was granted beyond what was requested and held.
    assert!(report.uptake.granted <= report.uptake.requested);
    assert!(report.uptake.granted <= report.field_step.energy_after);

    // INV-05: a survivor's committed change is exactly its recorded rate;
    // the ledger closes, so nothing was spent twice.
    for (id, organism) in &after {
        if let Some(previous) = before.get(id) {
            assert_eq!(
                (organism.energy_change_rate * dt).to_bits(),
                (organism.energy - previous.energy).to_bits(),
                "{id}"
            );
        }
    }

    // INV-06: each birth committed identity, genome, slot and endowment.
    let mut endowments = CompensatedSum::ZERO;
    for birth in &report.births {
        let child = after.get(&birth.child).expect("the child is live");
        let parent = before.get(&birth.parent).expect("the parent was live");
        assert_eq!(child.parent, Some(birth.parent));
        assert_eq!(child.genome, birth.genome);
        assert_eq!(
            world
                .population()
                .genome(birth.child)
                .map(flife_genome::Genome::id),
            Some(birth.genome)
        );
        assert_eq!(child.lineage, parent.lineage);
        assert_eq!(child.generation.get(), parent.generation.get() + 1);
        assert_eq!(child.birth_tick, report.tick);
        assert_eq!(child.velocity, [0.0, 0.0]);
        assert_eq!(child.energy_change_rate, 0.0);
        assert!(!child.held.is_valid(), "a newborn's held output is invalid");
        assert!(world.population().slot(birth.child).is_some());
        endowments.add(child.energy);
        census.perturbed += u64::from(birth.record.perturbed);
        if parent.generation.get() > 0 {
            census.descendants_born_of_descendants += 1;
        }
    }
    assert_eq!(
        report.closure.credited(Transfer::ChildEndowment).to_bits(),
        endowments.value().to_bits(),
        "every child's endowment is the transfer that was recorded"
    );
    assert_eq!(
        world.quota().live(),
        after.len() as u64,
        "one genome per organism"
    );

    // INV-07: the dead never return, and new identities are new.
    for death in &report.deaths {
        assert!(before.contains_key(&death.id));
        assert!(!after.contains_key(&death.id));
        assert_eq!(death.tick, report.tick);
        assert_eq!(death.residual, 0.0);
        let DeathCause::Exhausted(stage) = death.cause else {
            panic!("natural deaths are exhaustion: {death:?}");
        };
        // Collected at its resulting position: one that died before motion
        // never moved.
        if stage <= BudgetStage::Maintenance {
            assert_eq!(death.position, before[&death.id].position, "{death:?}");
        }
        *census.stages.entry(stage.name().to_owned()).or_default() += 1;
        if death.generation.get() > 0 {
            census.descendant_deaths += 1;
        }
        dead.insert(death.id);
    }
    let latest_before = before.keys().next_back().copied();
    for birth in &report.births {
        assert!(latest_before.is_none_or(|latest| birth.child > latest));
    }
    assert!(after.keys().all(|id| !dead.contains(id)));

    // Motion is recorded as it is paid. A survivor kept its whole velocity
    // and paid `c_v ||v||² Δt` for it, and a newborn has not moved. Ticks in
    // which an organism died at or after motion paid for velocities no row
    // records, so they only bound the recorded flow from below. The motion
    // here is far below the transport residual, so closure cannot see it.
    let mut motion = CompensatedSum::ZERO;
    for organism in after.values() {
        let [vx, vy] = organism.velocity;
        motion.add(motion_cost * (vx * vx + vy * vy) * dt);
    }
    let recorded = report.closure.flow(Flow::MotionCost);
    let tolerance = 1.0e-12 * motion.value();
    if report.deaths.iter().any(
        |death| matches!(death.cause, DeathCause::Exhausted(stage) if stage >= BudgetStage::Motion),
    ) {
        assert!(
            recorded >= motion.value() - tolerance,
            "{recorded} {}",
            motion.value()
        );
    } else {
        assert!(
            (recorded - motion.value()).abs() <= tolerance,
            "{recorded} {}",
            motion.value()
        );
        census.motion_exact_ticks += 1;
    }
    census.motion_paid += recorded;

    census.births += report.births.len();
    census.deaths += report.deaths.len();
    census.emitted += report.emitted;
}

/// G1 and the bead's evidence: a complete small headless ecology over three
/// simulated seconds, with every invariant checked on every tick.
#[test]
fn a_small_headless_ecology_lives_breeds_mutates_emits_and_dies() {
    let config = ecology(&[]);
    let mut world = found(&config, Arrangement::FreeList);
    let mut dead = BTreeSet::new();
    let mut census = Census::default();
    let initial = world.ledger().report().initial;
    let motion_cost = config.config().costs.motion_energy_per_speed_squared_second;
    for _ in 0..384 {
        let before = rows(&world);
        let report = publish(&mut world);
        // Every live controller is evaluated every tick at K = 1.
        assert_eq!(report.evaluated, before.len());
        check_tick(
            &world,
            &before,
            &report,
            &mut dead,
            &mut census,
            motion_cost,
        );
    }
    eprintln!("{census:?}; live {}", world.population().len());
    assert!(census.births >= 20, "{census:?}");
    assert!(census.deaths >= 20, "{census:?}");
    // Phase 6's deaths are recorded at the stage that exhausted them, not
    // at a later check of the same empty budget.
    assert!(
        census
            .stages
            .get("maintenance")
            .is_some_and(|&count| count > 0),
        "{census:?}"
    );
    assert!(census.emitted > 1_000, "{census:?}");
    assert!(census.perturbed > 0, "mutation changed the children");
    assert!(census.descendant_deaths > 0, "children die too: {census:?}");
    assert!(
        census.motion_paid > 0.0,
        "organisms move and pay: {census:?}"
    );
    assert!(census.motion_exact_ticks >= 300, "{census:?}");
    assert!(!world.population().is_empty());
    let report = world.ledger().report();
    assert_eq!(report.ticks, 384);
    assert_eq!(report.initial, initial);
    assert!(report.injected > 0.0 && report.dissipated > 0.0);
    // Cross-host guard: pinned on macOS arm64 and checked on Linux x86_64.
    // Every operator is IEEE-754 arithmetic or `flife_core::numerics`.
    assert_eq!(
        world.digest().to_hex(),
        "66826e20027d349788020a6b005742adcff577afde0e3c5d321b8acb275dc538"
    );
}

/// C.7: two storage arrangements of the same stable-identity world publish
/// the same scientific state and the same reports on every tick, while
/// their slots differ.
#[test]
fn c7_compact_and_free_list_worlds_publish_the_same_epochs() {
    let config = ecology(&[]);
    let mut compact = found(&config, Arrangement::Compact);
    let mut free = found(&config, Arrangement::FreeList);
    let mut slots_differed = false;
    let mut deaths = 0;
    for _ in 0..288 {
        let a = publish(&mut compact);
        let b = publish(&mut free);
        assert_eq!(a, b);
        assert_eq!(compact.digest(), free.digest());
        assert_eq!(compact.population().digest(), free.population().digest());
        slots_differed |= compact.population().slots() != free.population().slots();
        deaths += a.deaths.len();
    }
    assert!(deaths > 0 && slots_differed);
}

/// C.8: cancelling a candidate before publication leaves the committed epoch
/// authoritative; retrying prepares the same candidate; publishing makes the
/// next epoch authoritative. A world that cancels every candidate once
/// follows the world that never cancels, bit for bit.
#[test]
fn c8_cancellation_before_publication_leaves_the_prior_epoch_authoritative() {
    let config = ecology(&[]);
    let mut reference = found(&config, Arrangement::Compact);
    let mut world = found(&config, Arrangement::Compact);
    let (mut births, mut deaths) = (0, 0);
    for _ in 0..256 {
        let expected = publish(&mut reference);
        let before = committed(&world);

        let Advance::Ready(candidate) = world.advance().expect("advance") else {
            panic!("paused");
        };
        assert_eq!(candidate.base(), (before.1, world_epoch(&before)));
        let first = candidate.report().clone();
        candidate.cancel();
        assert_eq!(committed(&world), before, "cancellation changed the world");

        // Dropping is cancellation too.
        {
            let Advance::Ready(candidate) = world.advance().expect("advance") else {
                panic!("paused");
            };
            assert_eq!(candidate.report(), &first);
        }
        assert_eq!(committed(&world), before);

        let Advance::Ready(candidate) = world.advance().expect("advance") else {
            panic!("paused");
        };
        assert_eq!(
            candidate.report(),
            &first,
            "a retry prepares the same candidate"
        );
        let published = candidate.publish().expect("publish");
        assert_eq!(published, expected);
        assert_eq!(world.digest(), reference.digest());
        assert_eq!(world.epoch().get(), before.2 + 1);
        births += published.births.len();
        deaths += published.deaths.len();
    }
    assert!(births > 0 && deaths > 0, "{births} births, {deaths} deaths");
}

fn world_epoch(
    state: &(String, Tick, u64, u64, String, String, String),
) -> flife_core::ids::ResourceEpoch {
    flife_core::ids::ResourceEpoch::new(state.2)
}

/// §6.6 at `K = 4`: a regular evaluation is due when `(n + 1) mod K` equals
/// the stable phase; a newborn is forced on its first advance; held raw
/// outputs are decoded in between.
#[test]
fn controllers_are_evaluated_when_due_and_newborns_are_forced() {
    let interval = 4_u64;
    let config = ecology(&[("controller", "evaluation_interval_ticks", "4")]);
    let mut world = found(&config, Arrangement::FreeList);
    let mut newborns = 0;
    for _ in 0..192 {
        let before = rows(&world);
        let next = world.tick().get() + 1;
        let due = before
            .values()
            .filter(|o| !o.held.is_valid() || next % interval == u64::from(o.phase))
            .count();
        let report = publish(&mut world);
        assert_eq!(report.evaluated, due);
        newborns += report.births.len();
        let now = world.tick().get();
        for organism in world.population().iter() {
            assert_eq!(u64::from(organism.phase), organism.id.get() % interval);
            if organism.birth_tick.get() == now {
                assert_eq!(organism.held.evaluated_at(), None);
                continue;
            }
            let forced = organism.birth_tick.get() + 1;
            let phase = u64::from(organism.phase);
            // The last tick at or before `now` congruent to the phase.
            let regular = now.checked_sub((now + interval - phase) % interval);
            let expected = regular.filter(|&tick| tick >= forced).unwrap_or(forced);
            assert_eq!(
                organism.held.evaluated_at().map(Tick::get),
                Some(expected),
                "{organism:?} at tick {now}"
            );
        }
    }
    assert!(newborns > 0);
}

/// §7.5 `pause-before-commit`: a birth the population quota cannot cover
/// pauses the tick, reports the proposed births, and changes nothing; a
/// retry pauses the same way.
#[test]
fn a_population_shortfall_pauses_the_tick_and_changes_nothing() {
    let config = ecology(&[("population", "maximum_count", "48")]);
    let mut world = found(&config, Arrangement::Compact);
    let shortfall = loop {
        assert!(world.tick().get() < 128, "no birth was attempted");
        let before = committed(&world);
        match world.advance().expect("advance") {
            Advance::Ready(candidate) => {
                assert!(candidate.report().births.is_empty());
                candidate.publish().expect("publish");
            }
            Advance::Paused(shortfall) => {
                assert_eq!(committed(&world), before, "a paused tick changes nothing");
                break shortfall;
            }
        }
    };
    assert_eq!(shortfall.resource, Capacity::PopulationSlots);
    assert_eq!((shortfall.required, shortfall.available), (1, 0));
    assert!(shortfall.proposed.contains(&shortfall.parent));
    assert_eq!(shortfall.covered, 0);
    let before = committed(&world);
    let Advance::Paused(again) = world.advance().expect("advance") else {
        panic!("the retry did not pause");
    };
    assert_eq!(again, shortfall);
    assert_eq!(committed(&world), before);
}

/// §7.5: the fixed logical genome quota pauses the tick the same way.
#[test]
fn a_genome_quota_shortfall_pauses_the_tick_and_changes_nothing() {
    let founders = found(&ecology(&[]), Arrangement::Compact);
    let logical = founders.quota().logical_bytes().to_string();
    let config = ecology(&[("execution", "maximum_logical_genome_bytes", &logical)]);
    let mut world = found(&config, Arrangement::Compact);
    let shortfall = loop {
        assert!(world.tick().get() < 128, "no birth was attempted");
        match world.advance().expect("advance") {
            Advance::Ready(candidate) => {
                candidate.publish().expect("publish");
            }
            Advance::Paused(shortfall) => break shortfall,
        }
    };
    assert_eq!(shortfall.resource, Capacity::LogicalGenomeBytes);
    assert_eq!(shortfall.available, 0);
    assert_eq!(
        world.quota().logical_bytes(),
        founders.quota().logical_bytes()
    );
}

/// §7.6 `stop-and-preserve`: a numerical fault while preparing a candidate
/// stops the run with the last committed state kept bit for bit. Here one
/// enormous, narrow source overflows the F32 field.
#[test]
fn a_numerical_fault_stops_the_run_and_preserves_the_committed_state() {
    let config = ecology(&[
        ("sources", "count", "1"),
        ("sources", "power_per_source", "1.0e40"),
        ("sources", "sigma_world_units", "0.5"),
    ]);
    let mut world = found(&config, Arrangement::Compact);
    let (error, preserved) = loop {
        assert!(world.tick().get() < 64, "the field never overflowed");
        let before = committed(&world);
        match world.advance() {
            Ok(Advance::Ready(candidate)) => {
                candidate.publish().expect("publish");
            }
            Ok(Advance::Paused(shortfall)) => panic!("paused: {shortfall:?}"),
            Err(error) => break (error, before),
        }
    };
    assert!(
        matches!(error, CoreError::NumericalInvalid { .. }),
        "{error}"
    );
    assert!(world.tick().get() > 0, "some ticks were published first");
    assert_eq!(committed(&world), preserved);
    let fault = world.fault().expect("the run stopped");
    assert_eq!(fault.tick, world.tick());
    assert_eq!(fault.error, error);
    let refused = world
        .advance()
        .expect_err("a stopped run refuses to advance");
    assert!(
        matches!(&refused, CoreError::StateConflict { detail, .. } if detail.contains(STOP_AND_PRESERVE)),
        "{refused}"
    );
    assert_eq!(committed(&world), preserved);
}

/// A.3 item 1: founders at uniform positions with zero velocity and
/// history, the initial energy, birth tick zero, the cooldown deadline
/// `⌈cooldown/Δt⌉`, invalid held outputs and stable phases, each its own
/// lineage.
#[test]
fn founders_follow_appendix_a3_item_1() {
    let config = ecology(&[("population", "initial_count", "256")]);
    let world = found(&config, Arrangement::Compact);
    let population = world.population();
    assert_eq!(population.len(), 256);
    let mut quadrants = [0_u32; 4];
    for (index, organism) in population.iter().enumerate() {
        let raw = index as u64;
        assert_eq!(organism.id.get(), raw);
        assert_eq!(organism.genome.get(), raw);
        assert_eq!(organism.lineage.get(), raw);
        assert_eq!(organism.generation.get(), 0);
        assert_eq!(organism.parent, None);
        assert_eq!(organism.birth_tick, Tick::new(0));
        assert_eq!(organism.velocity, [0.0, 0.0]);
        assert_eq!(organism.energy, 3.0);
        assert_eq!(organism.energy_change_rate, 0.0);
        assert!(!organism.held.is_valid());
        assert_eq!(organism.phase, 0, "K = 1");
        assert_eq!(organism.cooldown_deadline, Tick::new(32), "⌈0.25 s × 128⌉");
        let [x, y] = organism.position;
        assert!((0.0..32.0).contains(&x) && (0.0..32.0).contains(&y));
        quadrants[usize::from(x >= 16.0) + 2 * usize::from(y >= 16.0)] += 1;
    }
    // Uniform by area: each quadrant holds 64 ± 3.5 standard deviations.
    for count in quadrants {
        assert!((40..=88).contains(&count), "{quadrants:?}");
    }
    // The address is (founders, organism, 0, lane (0, position, 0), 0).
    let first = population.get(OrganismId::new(0)).expect("founder 0");
    eprintln!("founder 0 at {:?}", first.position.map(f64::to_bits));
    assert_eq!(
        first.position.map(f64::to_bits),
        [4_625_357_314_911_663_276, 4_629_538_456_029_050_546],
        "pinned founder position"
    );
    let holdings = world.ledger().holdings();
    assert_eq!(holdings.field(), 0.0);
    assert_eq!(holdings.organisms(), 256.0 * 3.0);
    assert_eq!(world.quota().live(), 256);
    assert_eq!(world.tick(), Tick::new(0));
    assert_eq!(world.epoch().get(), 0);
}

/// INV-08: interaction is exclusively field-mediated. Organism 0 of a world
/// founded alone and organism 0 of a sparse world of three have the same
/// genome and position (both are functions of identity). While no other
/// organism can reach it through the field, it evolves bit for bit the same.
#[test]
fn an_organism_out_of_the_fields_reach_behaves_as_if_alone() {
    // Appendix A's 128 × 128 world, sparsely populated.
    let sparse = [
        ("world", "width", "128.0"),
        ("world", "height", "128.0"),
        ("world", "grid_width", "128"),
        ("world", "grid_height", "128"),
    ];
    let mut alone = found(
        &ecology(&[sparse.as_slice(), &[("population", "initial_count", "1")]].concat()),
        Arrangement::Compact,
    );
    let mut company = found(
        &ecology(&[sparse.as_slice(), &[("population", "initial_count", "3")]].concat()),
        Arrangement::Compact,
    );
    let zero = OrganismId::new(0);
    let origin = company.population().get(zero).expect("organism 0").position;
    let nearest = company
        .population()
        .iter()
        .filter(|organism| organism.id != zero)
        .map(|organism| {
            let dx = (organism.position[0] - origin[0]).abs();
            let dy = (organism.position[1] - origin[1]).abs();
            let (dx, dy) = (dx.min(128.0 - dx), dy.min(128.0 - dy));
            (dx * dx + dy * dy).sqrt()
        })
        .fold(f64::INFINITY, f64::min);
    // Per tick the field carries a difference at most one cell (one
    // transport substep at this resolution); the exchange, deposit and
    // sensing footprints add at most three cells on each side, and motion
    // (at most 4/128 of a cell per tick) a tenth of a cell more.
    let reach = |ticks: f64| 1.1 * ticks + 6.0;
    let mut compared = 0;
    while reach(f64::from(compared + 1)) < nearest {
        publish(&mut alone);
        publish(&mut company);
        assert_eq!(
            alone.population().get(zero),
            company.population().get(zero),
            "tick {}",
            alone.tick().get()
        );
        compared += 1;
    }
    eprintln!("nearest neighbour {nearest} cells; {compared} ticks compared");
    assert!(compared >= 32, "the sparse world is too crowded: {nearest}");
}

/// The whole-tick commit accounts for every live organism exactly once:
/// each survives with its new state, in identity order, or dies; survivors
/// pass INV-01 and keep their lifelong fields; and the quota holds the
/// dying genomes' charges. `commit_tick` runs this same check first.
#[test]
fn a_whole_tick_commit_accounts_for_every_organism_once() {
    let world = found(&ecology(&[]), Arrangement::FreeList);
    let population = world.population();
    let quota = world.quota();
    let tick = Tick::new(1);
    let rows: Vec<Organism> = population
        .iter()
        .map(|organism| Organism {
            energy: organism.energy * 0.5,
            ..organism
        })
        .collect();
    let check = |survivors: &[Organism], deaths: &[flife_engine::Death], quota| {
        population.check_tick(tick, survivors, deaths, &[], quota)
    };
    assert!(check(&rows, &[], quota).is_ok());
    // A live organism that neither survives nor dies.
    assert!(check(&rows[1..], &[], quota).is_err());
    // Twice, or out of order.
    let mut twice = rows.clone();
    twice.insert(1, rows[0]);
    assert!(check(&twice, &[], quota).is_err());
    let mut reversed = rows.clone();
    reversed.reverse();
    assert!(check(&reversed, &[], quota).is_err());
    // Both surviving and dying; dying alone is fine.
    let death =
        flife_engine::Death::exhausted(&rows[0], tick, rows[0].position, BudgetStage::Maintenance)
            .expect("death");
    assert!(check(&rows, &[death], quota).is_err());
    assert!(check(&rows[1..], &[death], quota).is_ok());
    // INV-01 for every survivor, by identity.
    let mut starved = rows.clone();
    starved[3].energy = 0.0;
    let error = check(&starved, &[], quota).expect_err("zero energy");
    assert!(
        matches!(&error, CoreError::NumericalInvalid { detail, .. } if detail.contains("organism:3")),
        "{error}"
    );
    // Fields fixed for life.
    let mut renamed = rows.clone();
    renamed[2].lineage = flife_core::ids::LineageId::new(99);
    assert!(check(&renamed, &[], quota).is_err());
    // The quota must hold the charges the deaths return.
    let empty = GenomeQuota::new(quota.logical_bytes(), quota.allocated_bytes()).expect("quota");
    assert!(check(&rows[1..], &[death], &empty).is_err());
    assert!(check(&rows, &[], &empty).is_ok());
    // Its bytes and its genome count are each checked on their own. One
    // 128-96 genome holds more bytes than two 64-32 founders but is one
    // genome; two 8-wide genomes are two genomes but hold fewer bytes than
    // one founder.
    let catalog = TopologyCatalog::from_config(&ecology(&[])).expect("catalog");
    let charged = |hidden: &[u32], count| {
        let member = catalog.find(hidden).expect("a catalog member");
        let mut charged = GenomeQuota::new(1 << 30, 1 << 30).expect("quota");
        for _ in 0..count {
            let _buffer = charged.allocate(&catalog, member).expect("charged");
        }
        charged
    };
    let second =
        flife_engine::Death::exhausted(&rows[1], tick, rows[1].position, BudgetStage::Maintenance)
            .expect("death");
    let one_large = charged(&[128, 96], 1);
    assert!(
        one_large.logical_bytes()
            > 2 * population
                .genome(rows[0].id)
                .expect("genome")
                .logical_bytes()
    );
    assert!(check(&rows[2..], &[death, second], quota).is_ok());
    assert!(check(&rows[2..], &[death, second], &one_large).is_err());
    let two_small = charged(&[8], 2);
    assert!(check(&rows[1..], &[death], &two_small).is_err());
}

/// Step `world` once, rendering the outcome for comparison: the report of a
/// published tick, or the shortfall of a paused one.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn outcome(world: &mut World) -> String {
    match world.step().expect("the tick runs") {
        Stepped::Published(report) => format!("{report:?}"),
        Stepped::Paused(shortfall) => format!("paused {shortfall:?}"),
    }
}

#[test]
fn the_cpu_worker_count_changes_no_result() {
    // Enough organisms that every lane gets a run of its own, and enough
    // ticks that births, deaths and held outputs all happen.
    let config = ecology(&[("population", "initial_count", "720")]);
    let ticks = 96;
    let mut serial = found(&config, Arrangement::FreeList);
    assert_eq!(serial.cpu_workers(), 1, "Appendix A asks for one worker");
    let reference: Vec<(String, String)> = (0..ticks)
        .map(|_| (outcome(&mut serial), serial.digest().to_hex()))
        .collect();
    assert!(
        reference
            .iter()
            .any(|(report, _)| report.contains("births: [B")),
        "the reference run has births"
    );
    for workers in [2, 3, 6, 64] {
        let mut world = found(&config, Arrangement::FreeList);
        world
            .set_cpu_workers(std::num::NonZeroUsize::new(workers).expect("nonzero"))
            .expect("lanes");
        assert_eq!(world.cpu_workers(), workers);
        for (tick, expected) in reference.iter().enumerate() {
            let got = (outcome(&mut world), world.digest().to_hex());
            assert_eq!(&got, expected, "{workers} workers diverge at tick {tick}");
        }
    }
}
