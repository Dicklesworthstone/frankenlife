//! FL-012: death, recycling, compaction and stable identity (plan §4.4,
//! §7.6, §7.7, §8.2, §9.1, INV-01, INV-06, INV-07, A.1, Appendix C.7,
//! C.12).
//!
//! Exhaustion rechecked after every budget stage, deaths committed once,
//! tiny reservoirs dissipated and recorded, recycling through the deposit
//! reduction with the organism's genetic affinity, and live storage whose
//! two slot arrangements hold the same world by identity.

use std::collections::BTreeMap;

use flife_core::clock::TimeStep;
use flife_core::ids::{Generation, GenomeId, IdSequence, LineageId, OrganismId};
use flife_core::ledger::{TickClosure, TickLedger, Transfer};
use flife_core::rng::SemanticRng;
use flife_core::{
    CoreError, EnergyBudget, Flow, Holdings, ResidualBudget, Tick, TickSpan, ValidatedConfig,
};
use flife_engine::{
    Arrangement, BirthContext, BirthRequest, BirthResolution, BudgetStage, Death, DeathCause,
    DeathSettlement, Disposition, EmissionPolicy, Entrant, Fate, HeldOutput, Identities,
    MaintenanceBill, MaintenanceCosts, Motion, Offspring, Organism, ParentState, Population,
    PopulationQuota, Recycling, Reproduction, ReproductionProposal, ReproductionRules,
    TinyEnergyPolicy, Vitality, resolve_births, settle_efficiencies,
};
use flife_field::sources::unit_direction;
use flife_field::state::CHANNELS;
use flife_field::{
    BufferLimits, Demand, Deposits, FieldGrid, FieldState, Grants, apply_deposits, apply_grants,
    resolve_uptake,
};
use flife_genome::{
    Genome, GenomeQuota, MAXIMUM_CANDIDATES, MutationParameters, Mutator, TopologyCatalog,
    TraitLimits, Traits, founder_traits, initial_scale,
};
use flife_neural::{ACTION_WIDTH, DecodedActions, Emission};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");
const SEED: u64 = 0x0f1_0012;
const COOLDOWN: u64 = 128;

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn appendix_a() -> ValidatedConfig {
    let appendix = &PLAN[PLAN.find("## Appendix A").expect("Appendix A heading")..];
    let start = appendix.find("```toml\n").expect("Appendix A TOML block") + "```toml\n".len();
    let length = appendix[start..].find("```").expect("closing fence");
    ValidatedConfig::from_toml(&appendix[start..start + length]).expect("Appendix A is valid")
}

fn appendix_a_toml() -> String {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    appendix[start..start + length].to_owned()
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn holdings(field: f64, organisms: f64) -> Holdings {
    Holdings::new(field, organisms).expect("holdings")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn budget(energy: f64) -> EnergyBudget {
    EnergyBudget::new(energy).expect("budget")
}

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn uniform(state: &mut u64) -> f64 {
    (splitmix(state) >> 11) as f64 / (1_u64 << 53) as f64
}

/// The Appendix A configuration with its catalog and trait limits.
struct World {
    config: ValidatedConfig,
    catalog: TopologyCatalog,
    limits: TraitLimits,
    scale: f64,
}

impl World {
    #[expect(
        clippy::expect_used,
        reason = "test helper: a failure here is a test failure"
    )]
    fn new() -> Self {
        let config = appendix_a();
        let catalog = TopologyCatalog::from_config(&config).expect("catalog");
        let limits = TraitLimits::from_config(&config).expect("limits");
        let scale = initial_scale(&config);
        Self {
            config,
            catalog,
            limits,
            scale,
        }
    }

    #[expect(
        clippy::expect_used,
        reason = "test helper: a failure here is a test failure"
    )]
    fn traits(&self, organism: u64) -> Traits {
        founder_traits(
            &SemanticRng::new(SEED),
            OrganismId::new(organism),
            &self.limits,
            self.scale,
        )
        .expect("traits")
    }

    /// A zero-parameter default-topology genome for `organism`.
    #[expect(
        clippy::expect_used,
        reason = "test helper: a failure here is a test failure"
    )]
    fn genome(&self, organism: u64, quota: &mut GenomeQuota) -> Genome {
        let buffer = quota
            .allocate(&self.catalog, self.catalog.default_topology())
            .expect("room for a genome");
        Genome::seal(
            buffer,
            GenomeId::new(organism),
            &self.catalog,
            self.traits(organism),
            None,
            &self.limits,
        )
        .expect("a genome")
    }

    fn founder(&self, organism: u64, energy: f64, quota: &mut GenomeQuota) -> Entrant {
        Entrant {
            organism: founder_row(organism, energy),
            genome: self.genome(organism, quota),
        }
    }

    fn child(
        &self,
        organism: u64,
        parent: &Organism,
        tick: u64,
        energy: f64,
        quota: &mut GenomeQuota,
    ) -> Entrant {
        Entrant {
            organism: child_row(organism, parent, tick, energy),
            genome: self.genome(organism, quota),
        }
    }
}

fn founder_row(organism: u64, energy: f64) -> Organism {
    Organism {
        id: OrganismId::new(organism),
        genome: GenomeId::new(organism),
        parent: None,
        lineage: LineageId::new(organism),
        generation: Generation::new(0),
        birth_tick: Tick::new(0),
        position: [(organism % 7) as f64 + 0.5, (organism % 5) as f64 + 0.5],
        velocity: [0.0, 0.0],
        energy,
        energy_change_rate: 0.0,
        held: HeldOutput::INVALID,
        phase: (organism % 4) as u32,
        cooldown_deadline: Tick::new(COOLDOWN),
    }
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn child_row(organism: u64, parent: &Organism, tick: u64, energy: f64) -> Organism {
    Organism {
        id: OrganismId::new(organism),
        genome: GenomeId::new(organism),
        parent: Some(parent.id),
        lineage: parent.lineage,
        generation: parent.generation.child().expect("generation"),
        birth_tick: Tick::new(tick),
        position: parent.position,
        velocity: [0.0, 0.0],
        energy,
        energy_change_rate: 0.0,
        held: HeldOutput::INVALID,
        phase: (organism % 4) as u32,
        cooldown_deadline: Tick::new(tick + COOLDOWN),
    }
}

/// Distinct, finite outputs for `organism` at `tick`.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn held(organism: u64, tick: u64) -> HeldOutput {
    let mut outputs = [0.0_f32; ACTION_WIDTH];
    for (index, output) in outputs.iter_mut().enumerate() {
        *output = (organism * 100 + index as u64) as f32 + 0.25;
    }
    HeldOutput::evaluated(Tick::new(tick), outputs).expect("finite outputs")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn population(arrangement: Arrangement, capacity: usize) -> Population {
    Population::new(arrangement, capacity).expect("population")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn quota() -> GenomeQuota {
    GenomeQuota::new(1 << 32, 1 << 32).expect("quota")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn alive(population: &Population, organism: u64) -> Organism {
    population
        .get(OrganismId::new(organism))
        .expect("a live organism")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn exhausted(organism: &Organism, tick: u64) -> Death {
    Death::exhausted(
        organism,
        Tick::new(tick),
        organism.position,
        BudgetStage::Maintenance,
    )
    .expect("a death")
}

fn organisms_total(population: &Population) -> f64 {
    population.iter().map(|organism| organism.energy).sum()
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn direction(components: &[f64]) -> [f64; CHANNELS] {
    let mut normals = [0.0; CHANNELS];
    normals[..components.len()].copy_from_slice(components);
    unit_direction(&normals).expect("a direction")
}

fn moment(field: &FieldState, channel: u32, cell: usize) -> f32 {
    field
        .moment(channel)
        .and_then(|plane| plane.get(cell))
        .copied()
        .unwrap_or(f32::NAN)
}

/// An `n × n` torus of unit cells, every cell holding `energy`.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn field(n: u32, energy: f32) -> FieldState {
    let grid = FieldGrid::new(n, n, f64::from(n), f64::from(n)).expect("grid");
    let mut field = FieldState::zeroed(grid, &BufferLimits::host()).expect("field");
    field.energy_mut().fill(energy);
    field
}

fn rules() -> ReproductionRules {
    ReproductionRules {
        birth_cost: 0.5,
        minimum_attempt_energy: 4.0,
        minimum_child_energy: 0.3,
        minimum_parent_energy: 0.3,
        cooldown: TickSpan::new(COOLDOWN),
        offset_radius: 0.25,
        controller_interval: 4,
    }
}

fn sure_attempt(child_fraction: f64) -> DecodedActions {
    DecodedActions {
        attempt_rate: 1.0,
        attempt_probability: 1.0,
        child_fraction,
        emission_gate: 0.0,
        emission: None,
    }
}

fn parent_state(organism: &Organism, position: [f64; 2]) -> ParentState {
    ParentState {
        id: organism.id,
        lineage: organism.lineage,
        generation: organism.generation,
        position,
        cooldown_deadline: organism.cooldown_deadline,
    }
}

/// One organism's tick through every §8.2 stage, with the given stage made
/// to exhaust it.
struct StageRun {
    vitality: Vitality,
    budget: EnergyBudget,
    paid: BTreeMap<BudgetStage, f64>,
    emitted: bool,
    proposal: ReproductionProposal,
    position: [f64; 2],
    closure: TickClosure,
    start: f64,
    granted: f64,
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn through_stages(target: Option<BudgetStage>) -> StageRun {
    let tiny = target == Some(BudgetStage::TinyReservoir);
    // Dyadic amounts, so the ledger closes exactly.
    let start = if tiny { 2.0_f64.powi(-40) } else { 1.0 };
    let row = founder_row(3, start);
    let mut world = field(8, 0.0);
    let grid = *world.grid();
    let position = [2.5, 2.5]; // one cell's center: the deposit is one packet
    // Chemistry's grant comes from a cell outside this field's accounting.
    let granted = if target == Some(BudgetStage::Chemistry) {
        0.5
    } else {
        0.0
    };
    let mut ledger = TickLedger::open(
        Tick::new(0),
        holdings(world.energy_total() + granted, start),
        ResidualBudget::EXACT,
    );
    let mut organism = budget(start);
    let mut vitality = Vitality::new();
    let mut paid = BTreeMap::new();

    // Chemistry: a toxic grant takes everything it can.
    let packets: &[(f64, f64)] = if granted > 0.0 {
        &[(granted, -1.0)]
    } else {
        &[]
    };
    let chemistry = settle_efficiencies(10.0, packets, &mut organism).expect("chemistry");
    chemistry.record(&mut ledger).expect("record");
    paid.insert(BudgetStage::Chemistry, chemistry.toxicity.paid);
    vitality
        .check(BudgetStage::Chemistry, &organism)
        .expect("check");

    // Maintenance.
    let maintenance = if target == Some(BudgetStage::Maintenance) {
        2.0 * start
    } else {
        start / 8.0
    };
    let maintenance = organism.pay(maintenance).expect("maintenance");
    ledger
        .record(Flow::Maintenance, maintenance.paid)
        .expect("record");
    paid.insert(BudgetStage::Maintenance, maintenance.paid);
    vitality
        .check(BudgetStage::Maintenance, &organism)
        .expect("check");

    // Motion: Δt = 1, c_v = 1, speed 1 costs 1, more than is left.
    let motion =
        Motion::new(TimeStep::new(1, 1).expect("Δt"), 0.0, 4.0, 4.0, 1.0, 1.0).expect("motion");
    let velocity = if target == Some(BudgetStage::Motion) {
        [1.0, 0.0]
    } else {
        [0.0, 0.0]
    };
    let moved = motion.settle(velocity, &mut organism).expect("motion");
    ledger
        .record(Flow::MotionCost, moved.cost.paid)
        .expect("record");
    paid.insert(BudgetStage::Motion, moved.cost.paid);
    let position = motion
        .displace(&grid, position, moved.velocity)
        .expect("displace");
    vitality
        .check(BudgetStage::Motion, &organism)
        .expect("check");

    // Emission: a quarter, or everything.
    let fraction = if target == Some(BudgetStage::Emission) {
        1.0
    } else {
        0.25
    };
    let mut deposits = Deposits::new();
    let emission = EmissionPolicy::new(0.75)
        .expect("policy")
        .emit(
            &grid,
            row.id,
            position,
            Some(&Emission {
                direction: direction(&[1.0]),
                spend_fraction: fraction,
            }),
            &mut organism,
            &mut deposits,
        )
        .expect("emission");
    paid.insert(BudgetStage::Emission, emission.cost.paid);
    vitality
        .check(BudgetStage::Emission, &organism)
        .expect("check");

    // The split: below the attempt minimum alive, refused outright dead.
    let reproduction = Reproduction::new(rules()).expect("rules");
    let proposal = reproduction
        .propose(
            &SemanticRng::new(SEED),
            &parent_state(&row, position),
            Tick::new(COOLDOWN),
            &sure_attempt(0.5),
            &organism,
        )
        .expect("propose");
    paid.insert(BudgetStage::Split, 0.0);
    vitality
        .check(BudgetStage::Split, &organism)
        .expect("check");

    // Tiny-reservoir clearing, last.
    let cleared = TinyEnergyPolicy::new(1.0e-12)
        .expect("policy")
        .clear(&mut organism)
        .expect("clear");
    if cleared > 0.0 {
        ledger.record(Flow::TinyReservoir, cleared).expect("record");
    }
    paid.insert(BudgetStage::TinyReservoir, cleared);
    vitality
        .check(BudgetStage::TinyReservoir, &organism)
        .expect("check");

    let emitted = !deposits.is_empty();
    if emitted {
        apply_deposits(&mut world, &mut deposits)
            .expect("deposits")
            .record(&mut ledger, Transfer::Emission)
            .expect("record");
    }
    let closure = ledger
        .close(holdings(world.energy_total(), organism.remaining()))
        .expect("the tick closes exactly");
    StageRun {
        vitality,
        budget: organism,
        paid,
        emitted,
        proposal,
        position,
        closure,
        start,
        granted,
    }
}

#[test]
fn c12_exhaustion_after_motion_dies_once_and_never_returns() {
    let world = World::new();
    for arrangement in [Arrangement::Compact, Arrangement::FreeList] {
        let mut genomes = quota();
        let mut population = population(arrangement, 8);
        population
            .admit(&mut vec![world.founder(7, 1.0, &mut genomes)])
            .expect("the founder");
        let row = alive(&population, 7);

        // One step, Δt = 1, no sources or other costs, final speed 1, c_v = 1,
        // and cells wide enough for the displacement.
        let grid = FieldGrid::new(4, 4, 64.0, 64.0).expect("grid");
        let motion =
            Motion::new(TimeStep::new(1, 1).expect("Δt"), 0.0, 4.0, 4.0, 1.0, 1.0).expect("motion");
        let mut ledger = TickLedger::open(
            Tick::new(0),
            holdings(0.0, organisms_total(&population)),
            ResidualBudget::EXACT,
        );
        let mut organism = budget(row.energy);
        let mut vitality = Vitality::new();
        assert!(
            vitality
                .check(BudgetStage::Chemistry, &organism)
                .expect("check")
        );
        assert!(
            vitality
                .check(BudgetStage::Maintenance, &organism)
                .expect("check")
        );
        let proposed = motion.propose([1.0, 0.0], [0.0, 0.0]).expect("propose");
        let moved = motion.settle(proposed, &mut organism).expect("motion");
        assert_eq!(moved.cost.paid, 1.0, "motion spends the remaining unit");
        ledger
            .record(Flow::MotionCost, moved.cost.paid)
            .expect("record");
        assert!(
            !vitality
                .check(BudgetStage::Motion, &organism)
                .expect("check"),
            "it dies at motion"
        );
        let position = motion
            .displace(&grid, row.position, moved.velocity)
            .expect("displace");
        assert_eq!(position, [row.position[0] + 1.0, row.position[1]]);

        // No later emission or split.
        let mut deposits = Deposits::new();
        let emission = EmissionPolicy::new(0.75)
            .expect("policy")
            .emit(
                &grid,
                row.id,
                position,
                Some(&Emission {
                    direction: direction(&[1.0]),
                    spend_fraction: 0.5,
                }),
                &mut organism,
                &mut deposits,
            )
            .expect("emission");
        assert!(!emission.emitted() && deposits.is_empty());
        assert!(
            !vitality
                .check(BudgetStage::Emission, &organism)
                .expect("check")
        );
        let proposal = Reproduction::new(rules())
            .expect("rules")
            .propose(
                &SemanticRng::new(SEED),
                &parent_state(&row, position),
                Tick::new(COOLDOWN),
                &sure_attempt(0.5),
                &organism,
            )
            .expect("propose");
        assert_eq!(proposal, ReproductionProposal::Exhausted);
        assert!(
            !vitality
                .check(BudgetStage::Split, &organism)
                .expect("check")
        );
        let tiny = TinyEnergyPolicy::from_config(&world.config).expect("policy");
        assert_eq!(tiny.clear(&mut organism).expect("clear"), 0.0);
        assert!(
            !vitality
                .check(BudgetStage::TinyReservoir, &organism)
                .expect("check")
        );
        assert_eq!(
            vitality.fate().expect("fate"),
            Fate::Dies(BudgetStage::Motion)
        );

        // Collected once, at its resulting position, with nothing to recycle.
        let death =
            Death::exhausted(&row, Tick::new(1), position, BudgetStage::Motion).expect("death");
        assert_eq!(death.position, position);
        assert_eq!(death.residual, 0.0);
        let settlement = Recycling::from_config(&world.config)
            .expect("recycling")
            .settle(&grid, &death, world.traits(7).affinity(), &mut deposits)
            .expect("settle");
        assert_eq!(
            settlement,
            DeathSettlement {
                residual: 0.0,
                removed: 0.0,
                dissipated: 0.0,
                recycled: 0.0,
                offered: 0.0,
            }
        );
        assert!(deposits.is_empty());
        settlement.record(&mut ledger).expect("record");
        let collected = population
            .commit(Tick::new(1), &[death], &mut Vec::new())
            .expect("commit");
        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].death, death);
        assert_eq!(collected[0].organism, row);
        assert_eq!(collected[0].genome.id(), row.genome);
        assert!(population.is_empty(), "absent from the live population");

        // The ledger records exactly one dissipated unit.
        let closure = ledger
            .close(holdings(0.0, organisms_total(&population)))
            .expect("closes exactly");
        assert_eq!(closure.dissipated(), 1.0);
        assert_eq!(closure.flow(Flow::MotionCost), 1.0);
        assert_eq!(closure.residual(), 0.0);
        for entry in collected {
            genomes.retire(entry.genome).expect("retire");
        }
        assert_eq!(genomes.live(), 0, "its genome charge is returned");

        // Resume with field energy: the dead identity neither reappears nor
        // absorbs.
        let mut resumed = field(8, 2.0);
        let before = resumed.energy_total();
        let demands: Vec<Demand> = population
            .iter()
            .map(|organism| Demand {
                organism: organism.id,
                position: organism.position,
                amount: 0.5,
            })
            .collect();
        assert!(demands.is_empty());
        let mut grants = Grants::new();
        resolve_uptake(&resumed, &demands, &mut grants).expect("resolve");
        assert_eq!(apply_grants(&mut resumed, &mut grants).expect("apply"), 0.0);
        assert_eq!(resumed.energy_total(), before);
        assert!(population.get(row.id).is_none());
        let digest = population.digest();
        assert!(matches!(
            population.admit(&mut vec![world.founder(7, 1.0, &mut genomes)]),
            Err(CoreError::StateConflict { .. })
        ));
        assert!(matches!(
            population.update(&row),
            Err(CoreError::StateConflict { .. })
        ));
        assert!(matches!(
            population.commit(Tick::new(1), &[death], &mut Vec::new()),
            Err(CoreError::StateConflict { .. })
        ));
        assert_eq!(population.digest(), digest);
    }
}

#[test]
fn exhaustion_is_rechecked_after_every_stage() {
    let order = [
        BudgetStage::Chemistry,
        BudgetStage::Maintenance,
        BudgetStage::Motion,
        BudgetStage::Emission,
        BudgetStage::Split,
        BudgetStage::TinyReservoir,
    ];
    assert_eq!(BudgetStage::ALL, order);

    // A survivor pays each stage and is published above the threshold.
    let survivor = through_stages(None);
    assert_eq!(survivor.vitality.fate().expect("fate"), Fate::Survives);
    assert_eq!(survivor.budget.remaining(), 0.65625);
    assert!(survivor.emitted);
    assert!(matches!(
        survivor.proposal,
        ReproductionProposal::Unviable(_)
    ));
    assert_eq!(survivor.closure.flow(Flow::Maintenance), 0.125);
    assert_eq!(survivor.closure.credited(Transfer::Emission), 0.1640625);
    assert_eq!(survivor.closure.flow(Flow::EmissionLoss), 0.0546875);
    assert_eq!(survivor.position, [2.5, 2.5]);

    for stage in [
        BudgetStage::Chemistry,
        BudgetStage::Maintenance,
        BudgetStage::Motion,
        BudgetStage::Emission,
        BudgetStage::TinyReservoir,
    ] {
        let run = through_stages(Some(stage));
        assert_eq!(run.vitality.died_at(), Some(stage), "{stage:?}");
        assert!(!run.vitality.is_alive());
        assert_eq!(run.vitality.fate().expect("fate"), Fate::Dies(stage));
        assert!(run.budget.is_exhausted());
        assert_eq!(run.budget.remaining(), 0.0);
        // Nothing after the exhausting stage pays, and none of it acts.
        for (&later, &amount) in &run.paid {
            if later > stage {
                assert_eq!(amount, 0.0, "{later:?} after death at {stage:?}");
            }
        }
        // Stages before its death acted normally: a tiny-reservoir death
        // emitted and proposed while still alive.
        if stage < BudgetStage::Split {
            assert_eq!(run.proposal, ReproductionProposal::Exhausted, "{stage:?}");
        } else {
            assert!(matches!(run.proposal, ReproductionProposal::Unviable(_)));
        }
        assert_eq!(run.emitted, stage >= BudgetStage::Emission, "{stage:?}");
        let mut dead = run.budget;
        assert!(matches!(
            dead.credit(0.25),
            Err(CoreError::StateConflict { .. })
        ));
        // Already paid motion and deposits keep their effect: the ledger
        // closes exactly, and everything the organism held, plus what it
        // absorbed, is either dissipated or in the field.
        assert_eq!(run.closure.residual(), 0.0, "{stage:?}");
        let field_gain = run.closure.credited(Transfer::Emission);
        assert_eq!(
            run.closure.dissipated() + field_gain,
            run.start + run.granted,
            "{stage:?}"
        );
        if stage == BudgetStage::Emission {
            assert_eq!(field_gain, 0.75 * 0.875);
        }
        if stage == BudgetStage::TinyReservoir {
            assert_eq!(
                run.closure.flow(Flow::TinyReservoir),
                21.0 * 2.0_f64.powi(-45)
            );
        }
    }

    // The split cannot exhaust a parent: its kept share has a positive
    // minimum, and a rule without one is refused.
    assert!(matches!(
        Reproduction::new(ReproductionRules {
            minimum_parent_energy: 0.0,
            ..rules()
        }),
        Err(CoreError::NumericalInvalid { .. })
    ));
}

#[test]
fn stages_are_checked_once_each_in_order() {
    let mut vitality = Vitality::new();
    let organism = budget(1.0);
    assert!(
        vitality
            .check(BudgetStage::Motion, &organism)
            .expect("motion")
    );
    for stage in [BudgetStage::Chemistry, BudgetStage::Motion] {
        assert!(matches!(
            vitality.check(stage, &organism),
            Err(CoreError::StateConflict { .. })
        ));
    }
    assert!(matches!(
        vitality.fate(),
        Err(CoreError::StateConflict { .. })
    ));

    // The first exhausting stage is the one recorded.
    let mut dead = budget(1.0);
    assert_eq!(dead.pay(1.0).expect("pay").paid, 1.0);
    assert!(
        !vitality
            .check(BudgetStage::Emission, &dead)
            .expect("emission")
    );
    assert!(!vitality.check(BudgetStage::Split, &dead).expect("split"));
    assert_eq!(vitality.died_at(), Some(BudgetStage::Emission));
    assert_eq!(
        vitality.fate().expect("fate"),
        Fate::Dies(BudgetStage::Emission)
    );
    assert_eq!(Vitality::default(), Vitality::new());
    assert_eq!(BudgetStage::TinyReservoir.name(), "tiny-reservoir");
}

#[test]
fn tiny_reservoirs_below_the_threshold_are_dissipated_and_recorded() {
    let policy = TinyEnergyPolicy::from_config(&appendix_a()).expect("Appendix A");
    assert_eq!(policy.threshold(), 1.0e-12);

    let mut organism = budget(5.0e-13);
    let mut ledger = TickLedger::open(Tick::new(0), holdings(0.0, 5.0e-13), ResidualBudget::EXACT);
    let cleared = policy.clear(&mut organism).expect("clear");
    assert_eq!(cleared, 5.0e-13);
    assert!(organism.is_exhausted());
    ledger.record(Flow::TinyReservoir, cleared).expect("record");
    let closure = ledger.close(holdings(0.0, 0.0)).expect("exact");
    assert_eq!(closure.flow(Flow::TinyReservoir), 5.0e-13);
    assert_eq!(closure.dissipated(), 5.0e-13);

    // Strictly below: the threshold itself and anything above survive.
    for energy in [1.0e-12, 1.0e-12_f64.next_up(), 2.0, f64::MAX] {
        let mut organism = budget(energy);
        assert_eq!(policy.clear(&mut organism).expect("clear"), 0.0);
        assert_eq!(organism.remaining(), energy);
        assert!(!organism.is_exhausted());
    }
    let mut boundary = budget(1.0e-12_f64.next_down());
    assert_eq!(
        policy.clear(&mut boundary).expect("clear"),
        1.0e-12_f64.next_down()
    );
    // An exhausted budget has nothing to clear.
    let mut dead = budget(0.0);
    assert_eq!(policy.clear(&mut dead).expect("clear"), 0.0);
    // A zero threshold clears nothing, not even the smallest subnormal.
    let mut least = budget(f64::from_bits(1));
    assert_eq!(
        TinyEnergyPolicy::new(0.0)
            .expect("zero")
            .clear(&mut least)
            .expect("clear"),
        0.0
    );

    for threshold in [-1.0e-12, f64::NAN, f64::INFINITY] {
        assert!(matches!(
            TinyEnergyPolicy::new(threshold),
            Err(CoreError::NumericalInvalid { .. })
        ));
    }
    // An unregistered policy never reaches the operator: A.2 refuses it.
    let renamed = appendix_a_toml().replace(
        "tiny_energy_policy = \"dissipate-and-record\"",
        "tiny_energy_policy = \"keep\"",
    );
    assert!(renamed.contains("\"keep\""));
    assert!(ValidatedConfig::from_toml(&renamed).is_err());
}

#[test]
fn recycling_returns_the_configured_fraction_with_the_genetic_affinity() {
    let world = World::new();
    let recycling = Recycling::from_config(&world.config).expect("Appendix A");
    assert_eq!(recycling.fraction(), 0.5);
    let row = founder_row(9, 4.0);
    let affinity = *world.traits(9).affinity();
    let at = [2.5, 2.5]; // the center of cell (2, 2)
    let cell = 2 + 2 * 8;

    // Recycle: half the residual reaches the field, carrying a.
    let mut ground = field(8, 0.0);
    let grid = *ground.grid();
    let mut deposits = Deposits::new();
    let death = Death::removed(&row, Tick::new(5), at, 4.0, Disposition::Recycle).expect("death");
    assert_eq!(death.cause, DeathCause::Removed(Disposition::Recycle));
    let settlement = recycling
        .settle(&grid, &death, &affinity, &mut deposits)
        .expect("settle");
    assert_eq!(
        settlement,
        DeathSettlement {
            residual: 4.0,
            removed: 0.0,
            dissipated: 0.0,
            recycled: 4.0,
            offered: 2.0,
        }
    );
    let mut ledger = TickLedger::open(Tick::new(4), holdings(0.0, 4.0), ResidualBudget::EXACT);
    settlement.record(&mut ledger).expect("record");
    let summary = apply_deposits(&mut ground, &mut deposits).expect("reduce");
    summary
        .record(&mut ledger, Transfer::Recycling)
        .expect("record");
    let closure = ledger
        .close(holdings(ground.energy_total(), 0.0))
        .expect("exact");
    assert_eq!(closure.debited(Transfer::Recycling), 4.0);
    assert_eq!(closure.credited(Transfer::Recycling), 2.0);
    assert_eq!(closure.flow(Flow::UnrecycledDeath), 2.0);
    assert_eq!(ground.energy()[cell], 2.0);
    for (channel, &component) in affinity.iter().enumerate() {
        let channel = u32::try_from(channel).expect("channel");
        assert_eq!(
            moment(&ground, channel, cell),
            (2.0 * component) as f32,
            "the corpse carries its genetic affinity"
        );
    }

    // Remove and dissipate send the whole residual to O and to D.
    for (disposition, flow) in [
        (Disposition::Remove, Flow::CommandRemoval),
        (Disposition::Dissipate, Flow::UnrecycledDeath),
    ] {
        let death = Death::removed(&row, Tick::new(5), at, 4.0, disposition).expect("death");
        let mut deposits = Deposits::new();
        let settlement = recycling
            .settle(&grid, &death, &affinity, &mut deposits)
            .expect("settle");
        assert!(deposits.is_empty());
        let mut ledger = TickLedger::open(Tick::new(4), holdings(0.0, 4.0), ResidualBudget::EXACT);
        settlement.record(&mut ledger).expect("record");
        let closure = ledger.close(holdings(0.0, 0.0)).expect("exact");
        assert_eq!(closure.flow(flow), 4.0, "{disposition:?}");
    }
    let removed = Death::removed(&row, Tick::new(5), at, 4.0, Disposition::Remove).expect("death");
    let removal = recycling
        .settle(&grid, &removed, &affinity, &mut Deposits::new())
        .expect("settle");
    let mut ledger = TickLedger::open(Tick::new(4), holdings(0.0, 4.0), ResidualBudget::EXACT);
    removal.record(&mut ledger).expect("record");
    assert_eq!(
        ledger.close(holdings(0.0, 0.0)).expect("exact").removed(),
        4.0
    );

    // A zero fraction offers nothing; everything is unrecycled.
    let mut ground = field(8, 0.0);
    let mut deposits = Deposits::new();
    Recycling::new(0.0)
        .expect("zero")
        .settle(&grid, &death, &affinity, &mut deposits)
        .expect("settle");
    let mut ledger = TickLedger::open(Tick::new(4), holdings(0.0, 4.0), ResidualBudget::EXACT);
    apply_deposits(&mut ground, &mut deposits)
        .expect("reduce")
        .record(&mut ledger, Transfer::Recycling)
        .expect("record");
    let closure = ledger.close(holdings(0.0, 0.0)).expect("exact");
    assert_eq!(closure.flow(Flow::UnrecycledDeath), 4.0);

    // Nothing to recycle stages nothing; an exhaustion death holds nothing.
    let mut deposits = Deposits::new();
    let empty = Death::removed(&row, Tick::new(5), at, 0.0, Disposition::Recycle).expect("death");
    recycling
        .settle(&grid, &empty, &affinity, &mut deposits)
        .expect("settle");
    assert!(deposits.is_empty());
    let contradictory = Death {
        residual: 1.0,
        ..Death::exhausted(&row, Tick::new(5), at, BudgetStage::Maintenance).expect("death")
    };
    assert!(matches!(
        recycling.settle(&grid, &contradictory, &affinity, &mut deposits),
        Err(CoreError::StateConflict { .. })
    ));
    assert!(matches!(
        recycling.settle(&grid, &death, &[1.0; CHANNELS], &mut deposits),
        Err(CoreError::NumericalInvalid { .. })
    ));
    assert!(deposits.is_empty(), "a refused recycling stages nothing");

    for fraction in [-0.1, 1.5, f64::NAN] {
        assert!(matches!(
            Recycling::new(fraction),
            Err(CoreError::NumericalInvalid { .. })
        ));
    }
    let renamed = appendix_a_toml().replace(
        "corpse_direction = \"genetic-affinity\"",
        "corpse_direction = \"neutral\"",
    );
    assert!(renamed.contains("\"neutral\""));
    assert!(ValidatedConfig::from_toml(&renamed).is_err());
}

#[test]
fn deaths_are_committed_once_and_only_for_the_living() {
    let world = World::new();
    for arrangement in [Arrangement::Compact, Arrangement::FreeList] {
        let mut genomes = quota();
        let mut population = population(arrangement, 8);
        let mut founders: Vec<Entrant> = (1..=3)
            .map(|id| world.founder(id, 2.0, &mut genomes))
            .collect();
        population.admit(&mut founders).expect("founders");
        assert!(founders.is_empty());
        let two = alive(&population, 2);
        let death = exhausted(&two, 1);
        let digest = population.digest();
        let mut births = vec![world.child(4, &two, 1, 0.5, &mut genomes)];

        let refused = [
            vec![death, death],
            vec![Death {
                tick: Tick::new(2),
                ..death
            }],
            vec![Death {
                id: OrganismId::new(9),
                ..death
            }],
            vec![Death {
                genome: GenomeId::new(3),
                ..death
            }],
            vec![Death {
                lineage: LineageId::new(1),
                ..death
            }],
            vec![Death {
                generation: Generation::new(1),
                ..death
            }],
        ];
        for deaths in refused {
            assert!(
                matches!(
                    population.commit(Tick::new(1), &deaths, &mut births),
                    Err(CoreError::StateConflict { .. })
                ),
                "{deaths:?}"
            );
            assert_eq!(
                population.digest(),
                digest,
                "a refused commit changes nothing"
            );
            assert_eq!(births.len(), 1, "a refused commit hands the births back");
        }

        let collected = population
            .commit(Tick::new(1), &[death], &mut births)
            .expect("commit");
        assert!(births.is_empty());
        assert_eq!(collected.len(), 1);
        assert_eq!(
            population.ids().map(OrganismId::get).collect::<Vec<_>>(),
            [1, 3, 4]
        );
        // Dead now: a second death is refused, in any later tick.
        let digest = population.digest();
        assert!(matches!(
            population.commit(
                Tick::new(2),
                &[Death {
                    tick: Tick::new(2),
                    ..death
                }],
                &mut Vec::new()
            ),
            Err(CoreError::StateConflict { .. })
        ));
        assert_eq!(population.digest(), digest);
        for entry in collected {
            genomes.retire(entry.genome).expect("retire");
        }
        assert_eq!(genomes.live(), population.len() as u64);
    }
}

/// The whole-tick commit finds each survivor by identity, whatever slot it
/// occupies and across the gaps the dead leave, and refuses a dead identity
/// between live ones as not alive. Organisms read with their genomes are
/// the organisms and genomes read apart.
#[test]
fn a_whole_tick_commit_finds_survivors_by_identity_across_gaps() {
    let world = World::new();
    for arrangement in [Arrangement::Compact, Arrangement::FreeList] {
        let mut genomes = quota();
        let mut population = population(arrangement, 8);
        let mut founders: Vec<Entrant> = [1, 3, 5, 6]
            .into_iter()
            .map(|id| world.founder(id, 2.0, &mut genomes))
            .collect();
        population.admit(&mut founders).expect("founders");
        // 3 dies and 7 is born: the live identities are 1, 5, 6 and 7, and
        // a slot is reused or moved.
        let three = alive(&population, 3);
        let mut births = vec![world.child(7, &alive(&population, 1), 1, 0.5, &mut genomes)];
        let collected = population
            .commit(Tick::new(1), &[exhausted(&three, 1)], &mut births)
            .expect("commit");
        for entry in collected {
            genomes.retire(entry.genome).expect("retire");
        }
        let rows: Vec<Organism> = population
            .iter()
            .map(|organism| Organism {
                energy: organism.energy * 0.5,
                ..organism
            })
            .collect();
        assert_eq!(
            rows.iter().map(|row| row.id.get()).collect::<Vec<_>>(),
            [1, 5, 6, 7]
        );

        // Read together, as read apart.
        let together: Vec<_> = population.iter_with_genomes().collect();
        assert_eq!(
            together
                .iter()
                .map(|&(organism, _)| organism)
                .collect::<Vec<_>>(),
            population.iter().collect::<Vec<_>>()
        );
        for (organism, genome) in together {
            let apart = population.genome(organism.id).expect("a genome");
            assert!(
                genome.is_some_and(|genome| std::ptr::eq(genome, apart)),
                "{}",
                organism.id
            );
        }

        // The dead 3, between live identities, is not alive; nothing changes.
        let digest = population.digest();
        let mut stray = rows.clone();
        stray.insert(1, three);
        let refused = |error: CoreError| {
            assert!(
                matches!(&error, CoreError::StateConflict { detail, .. }
                    if detail.contains("organism:3 is not alive")),
                "{error}"
            );
        };
        refused(
            population
                .check_tick(Tick::new(2), &stray, &[], &[], &genomes)
                .expect_err("a dead survivor"),
        );
        refused(
            population
                .commit_tick(Tick::new(2), &stray, &[], &mut Vec::new(), &mut genomes)
                .expect_err("a dead survivor"),
        );
        assert_eq!(population.digest(), digest);

        // Each survivor's new state lands on its own identity.
        population
            .commit_tick(Tick::new(2), &rows, &[], &mut Vec::new(), &mut genomes)
            .expect("commit tick");
        for row in &rows {
            assert_eq!(alive(&population, row.id.get()), *row);
        }
    }
}

#[test]
fn dead_identities_are_never_readmitted() {
    let world = World::new();
    for arrangement in [Arrangement::Compact, Arrangement::FreeList] {
        let mut genomes = quota();
        let mut population = population(arrangement, 5);
        let mut founders: Vec<Entrant> = (1..=3)
            .map(|id| world.founder(id, 2.0, &mut genomes))
            .collect();
        population.admit(&mut founders).expect("founders");
        let two = alive(&population, 2);
        let one = alive(&population, 1);
        let collected = population
            .commit(Tick::new(1), &[exhausted(&two, 1)], &mut Vec::new())
            .expect("commit");
        for entry in collected {
            genomes.retire(entry.genome).expect("retire");
        }
        assert_eq!(population.latest(), Some(OrganismId::new(3)));
        let digest = population.digest();

        // The dead identity, a live one, and any identity at or below the
        // latest are refused, whatever slot is free.
        for id in [2, 3, 1] {
            let mut entrants = vec![world.child(id, &one, 2, 0.5, &mut genomes)];
            assert!(matches!(
                population.commit(Tick::new(2), &[], &mut entrants),
                Err(CoreError::StateConflict { .. })
            ));
            assert_eq!(entrants.len(), 1);
            for entrant in entrants {
                genomes.retire(entrant.genome).expect("return the charge");
            }
        }
        // Births out of order, born at another tick, or bringing another
        // identity's genome.
        let mut out_of_order = vec![
            world.child(5, &one, 2, 0.5, &mut genomes),
            world.child(4, &one, 2, 0.5, &mut genomes),
        ];
        let mut wrong_tick = vec![world.child(4, &one, 3, 0.5, &mut genomes)];
        let mut wrong_genome = vec![Entrant {
            organism: child_row(4, &one, 2, 0.5),
            genome: world.genome(5, &mut genomes),
        }];
        for entrants in [&mut out_of_order, &mut wrong_tick, &mut wrong_genome] {
            assert!(matches!(
                population.commit(Tick::new(2), &[], entrants),
                Err(CoreError::StateConflict { .. })
            ));
            assert!(!entrants.is_empty());
        }
        // A newborn without strictly positive energy (INV-01).
        let mut empty = vec![Entrant {
            organism: Organism {
                energy: 0.0,
                ..child_row(4, &one, 2, 0.5)
            },
            genome: world.genome(4, &mut genomes),
        }];
        assert!(matches!(
            population.commit(Tick::new(2), &[], &mut empty),
            Err(CoreError::NumericalInvalid { .. })
        ));
        // More than the capacity.
        let mut crowd: Vec<Entrant> = (4..=7)
            .map(|id| world.child(id, &one, 2, 0.5, &mut genomes))
            .collect();
        assert!(matches!(
            population.commit(Tick::new(2), &[], &mut crowd),
            Err(CoreError::BudgetExhausted {
                budget: "live population",
                required: 6,
                available: 5,
            })
        ));
        assert_eq!(population.digest(), digest, "every refusal changes nothing");
        for entrants in [out_of_order, wrong_tick, wrong_genome, empty, crowd] {
            for entrant in entrants {
                genomes.retire(entrant.genome).expect("return the charge");
            }
        }
        assert_eq!(genomes.live(), population.len() as u64);

        // New identities in order are admitted.
        let mut births = vec![
            world.child(4, &one, 2, 0.5, &mut genomes),
            world.child(5, &one, 2, 0.5, &mut genomes),
        ];
        population
            .commit(Tick::new(2), &[], &mut births)
            .expect("commit");
        assert_eq!(population.latest(), Some(OrganismId::new(5)));
        assert_eq!(
            population.ids().map(OrganismId::get).collect::<Vec<_>>(),
            [1, 3, 4, 5]
        );
    }
}

#[test]
fn reused_slots_carry_no_stale_controller_state() {
    let world = World::new();

    // Free list: a newborn takes the dead organism's slot, stale contents and
    // all, and none of them reach it.
    let mut genomes = quota();
    let mut free = population(Arrangement::FreeList, 8);
    let mut founders: Vec<Entrant> = (1..=3)
        .map(|id| world.founder(id, 2.0, &mut genomes))
        .collect();
    free.admit(&mut founders).expect("founders");
    for id in 1..=3 {
        let row = Organism {
            held: held(id, 4),
            velocity: [0.5, -0.25],
            energy_change_rate: 0.125,
            ..alive(&free, id)
        };
        free.update(&row).expect("update");
    }
    let two = alive(&free, 2);
    assert_eq!(free.slot(two.id), Some(1));
    assert!(two.held.is_valid());
    let one = alive(&free, 1);
    let mut births = vec![world.child(10, &one, 5, 0.5, &mut genomes)];
    free.commit(Tick::new(5), &[exhausted(&two, 5)], &mut births)
        .expect("commit");
    assert_eq!(
        free.slot(OrganismId::new(10)),
        Some(1),
        "the slot is reused"
    );
    let newborn = alive(&free, 10);
    assert_eq!(newborn, child_row(10, &one, 5, 0.5));
    assert!(!newborn.held.is_valid());
    assert_eq!(newborn.held.outputs(), None, "no stale output is readable");
    assert_eq!(newborn.held.evaluated_at(), None);
    assert_eq!(newborn.held, HeldOutput::INVALID);
    assert_eq!(newborn.velocity, [0.0, 0.0]);
    assert_eq!(newborn.energy_change_rate, 0.0);
    assert_eq!(
        free.genome(newborn.id).map(Genome::id),
        Some(GenomeId::new(10))
    );
    assert!(free.get(two.id).is_none());

    // Compact: survivors move down with their own state.
    let mut genomes = quota();
    let mut compact = population(Arrangement::Compact, 8);
    let mut founders: Vec<Entrant> = (1..=3)
        .map(|id| world.founder(id, 2.0, &mut genomes))
        .collect();
    compact.admit(&mut founders).expect("founders");
    for id in 1..=3 {
        let row = Organism {
            held: held(id, 4),
            velocity: [0.5, -0.25],
            energy_change_rate: 0.125,
            ..alive(&compact, id)
        };
        compact.update(&row).expect("update");
    }
    let before: Vec<Organism> = compact.iter().collect();
    let two = alive(&compact, 2);
    let one = alive(&compact, 1);
    let mut births = vec![world.child(10, &one, 5, 0.5, &mut genomes)];
    compact
        .commit(Tick::new(5), &[exhausted(&two, 5)], &mut births)
        .expect("commit");
    assert_eq!(compact.slot(OrganismId::new(3)), Some(1), "3 moves down");
    assert_eq!(compact.slot(OrganismId::new(10)), Some(2));
    assert_eq!(alive(&compact, 3), before[2], "and keeps its own state");
    assert_eq!(alive(&compact, 3).held.outputs(), held(3, 4).outputs());
    assert_eq!(alive(&compact, 10).held, HeldOutput::INVALID);
    assert_eq!(
        compact.genome(OrganismId::new(3)).map(Genome::id),
        Some(GenomeId::new(3))
    );

    // Both arrangements now hold the same world in different slots.
    assert_eq!(compact.digest(), free.digest());
    assert_ne!(compact.slots(), free.slots());
}

/// FL-026: every live genome holds an arena slot of its topology. A death
/// frees it and a newborn in the same commit takes it at the next
/// generation, so the old name is stale; a survivor keeps its slot when the
/// compact arrangement moves it; the slots are the same under both
/// arrangements.
#[test]
fn genomes_hold_arena_slots_that_newborns_reuse_at_the_next_generation() {
    let world = World::new();
    let topology = world.catalog.default_topology();
    let mut held_by = Vec::new();
    for arrangement in [Arrangement::FreeList, Arrangement::Compact] {
        let mut genomes = quota();
        let mut population = population(arrangement, 8);
        let mut founders: Vec<Entrant> = (1..=3)
            .map(|id| world.founder(id, 2.0, &mut genomes))
            .collect();
        population.admit(&mut founders).expect("founders");
        let slot = |population: &Population, id: u64| {
            population
                .arena_slot(OrganismId::new(id))
                .expect("a live genome's slot")
        };
        for (id, index) in [(1, 0), (2, 1), (3, 2)] {
            let held = slot(&population, id);
            assert_eq!(
                (held.topology() as usize, held.slot(), held.generation()),
                (topology.index(), index, 1),
                "{arrangement:?}: founder {id}"
            );
        }
        let two = alive(&population, 2);
        let old = slot(&population, 2);
        let three = slot(&population, 3);
        let one = alive(&population, 1);
        let mut births = vec![world.child(10, &one, 5, 0.5, &mut genomes)];
        population
            .commit(Tick::new(5), &[exhausted(&two, 5)], &mut births)
            .expect("commit");
        let newborn = slot(&population, 10);
        assert_eq!(
            (newborn.slot(), newborn.generation()),
            (1, 2),
            "{arrangement:?}"
        );
        assert!(
            !population.arena().is_live(old),
            "the dead genome's name is stale"
        );
        assert!(population.arena().is_live(newborn));
        assert_eq!(population.arena_slot(two.id), None);
        assert_eq!(
            slot(&population, 3),
            three,
            "{arrangement:?}: 3 keeps its slot"
        );
        assert_eq!(
            (
                population.arena().live(topology),
                population.arena().slots(topology)
            ),
            (3, 3)
        );
        // A death with no birth frees a slot; the next newborn takes the
        // lowest free one.
        let ten = alive(&population, 10);
        population
            .commit(Tick::new(6), &[exhausted(&ten, 6)], &mut Vec::new())
            .expect("commit");
        assert_eq!(population.arena().live(topology), 2);
        let mut births = vec![world.child(11, &one, 7, 0.5, &mut genomes)];
        population
            .commit(Tick::new(7), &[], &mut births)
            .expect("commit");
        let eleven = slot(&population, 11);
        assert_eq!(
            (eleven.slot(), eleven.generation()),
            (1, 3),
            "{arrangement:?}"
        );
        held_by.push(
            population
                .ids()
                .map(|id| population.arena_slot(id))
                .collect::<Vec<_>>(),
        );
    }
    assert_eq!(
        held_by[0], held_by[1],
        "the arrangements assign the same slots"
    );
}

#[test]
fn c7_two_storage_arrangements_hold_the_same_world() {
    let world = World::new();
    let mut compact = population(Arrangement::Compact, 64);
    let mut free = population(Arrangement::FreeList, 64);
    let (mut compact_genomes, mut free_genomes) = (quota(), quota());
    let mut founders: Vec<Entrant> = (0..24)
        .map(|id| world.founder(id, 1.0 + id as f64, &mut compact_genomes))
        .collect();
    compact.admit(&mut founders).expect("founders");
    let mut founders: Vec<Entrant> = (0..24)
        .map(|id| world.founder(id, 1.0 + id as f64, &mut free_genomes))
        .collect();
    free.admit(&mut founders).expect("founders");

    let mut state = 0xc7_u64;
    let mut next = 24;
    let mut slots_differed = false;
    let mut births_total = 0;
    let mut deaths_total = 0;
    for tick in 1..=48_u64 {
        let live: Vec<Organism> = compact.iter().collect();
        assert_eq!(live, free.iter().collect::<Vec<_>>());

        // Survivors change state; some evaluate their controllers.
        let mut deaths = Vec::new();
        let mut survivors = Vec::new();
        for organism in &live {
            if live.len() > 4 && uniform(&mut state) < 0.2 {
                deaths.push(exhausted(organism, tick));
            } else {
                survivors.push(Organism {
                    position: [
                        (organism.position[0] + uniform(&mut state)) % 8.0,
                        (organism.position[1] + 0.5) % 8.0,
                    ],
                    velocity: [uniform(&mut state) - 0.5, 0.0],
                    energy: organism.energy * (0.75 + uniform(&mut state) / 2.0),
                    energy_change_rate: uniform(&mut state) - 0.5,
                    held: if uniform(&mut state) < 0.5 {
                        held(organism.id.get(), tick)
                    } else {
                        organism.held
                    },
                    ..*organism
                });
            }
        }
        let parents: Vec<Organism> = survivors.clone();
        let mut compact_births = Vec::new();
        let mut free_births = Vec::new();
        let births = splitmix(&mut state) % 4;
        for _ in 0..births {
            if parents.is_empty() || compact.len() - deaths.len() + compact_births.len() >= 64 {
                break;
            }
            let pick = (splitmix(&mut state) % parents.len() as u64) as usize;
            let parent = &parents[pick];
            compact_births.push(world.child(next, parent, tick, 0.25, &mut compact_genomes));
            free_births.push(world.child(next, parent, tick, 0.25, &mut free_genomes));
            next += 1;
        }
        births_total += compact_births.len();
        deaths_total += deaths.len();

        for (population, genomes, births) in [
            (&mut compact, &mut compact_genomes, &mut compact_births),
            (&mut free, &mut free_genomes, &mut free_births),
        ] {
            for survivor in &survivors {
                population.update(survivor).expect("update");
            }
            let collected = population
                .commit(Tick::new(tick), &deaths, births)
                .expect("commit");
            assert_eq!(collected.len(), deaths.len());
            for (entry, death) in collected.into_iter().zip(&deaths) {
                assert_eq!(entry.death, *death, "collected in identity order");
                assert_eq!(entry.genome.id(), death.genome);
                genomes.retire(entry.genome).expect("retire");
            }
            assert_eq!(genomes.live(), population.len() as u64);
            // The index and the slots agree.
            for id in population.ids() {
                let slot = population.slot(id).expect("indexed");
                assert_eq!(population.slots()[slot], Some(id));
                assert_eq!(
                    population.genome(id).map(Genome::id),
                    population.get(id).map(|o| o.genome)
                );
            }
            let occupied = population
                .slots()
                .iter()
                .filter(|slot| slot.is_some())
                .count();
            assert_eq!(occupied, population.len());
        }

        // The same scientific state by identity, whatever the slots.
        assert_eq!(compact.digest(), free.digest(), "tick {tick}");
        assert_eq!(
            compact.iter().collect::<Vec<_>>(),
            free.iter().collect::<Vec<_>>()
        );
        assert!(
            compact.slots().iter().all(Option::is_some),
            "compact has no holes"
        );
        slots_differed |= compact.slots() != free.slots();
    }
    assert!(slots_differed, "the arrangements really differ physically");
    assert!(
        births_total > 20 && deaths_total > 20,
        "{births_total} {deaths_total}"
    );
}

#[test]
fn numerical_faults_are_refused_by_identity_and_never_collected() {
    let world = World::new();
    let mut genomes = quota();
    let mut population = population(Arrangement::FreeList, 4);
    population
        .admit(&mut vec![world.founder(1, 2.0, &mut genomes)])
        .expect("founder");
    let one = alive(&population, 1);
    let digest = population.digest();
    let faults = [
        Organism {
            energy: f64::NAN,
            ..one
        },
        Organism {
            energy: f64::INFINITY,
            ..one
        },
        Organism { energy: 0.0, ..one },
        Organism {
            energy: -1.0,
            ..one
        },
        Organism {
            position: [f64::NAN, 0.5],
            ..one
        },
        Organism {
            velocity: [0.0, f64::INFINITY],
            ..one
        },
        Organism {
            energy_change_rate: f64::NAN,
            ..one
        },
    ];
    for fault in faults {
        match population.update(&fault) {
            Err(CoreError::NumericalInvalid { detail, .. }) => {
                assert!(detail.contains("organism:1"), "{detail}");
            }
            other => panic!("{fault:?} gave {other:?}"),
        }
        assert_eq!(
            population.digest(),
            digest,
            "the committed state is preserved"
        );
    }
    let mut outputs = [0.0_f32; ACTION_WIDTH];
    outputs[7] = f32::NAN;
    assert!(matches!(
        HeldOutput::evaluated(Tick::new(1), outputs),
        Err(CoreError::NumericalInvalid { .. })
    ));
    // A fault cannot be collected as a death either.
    assert!(matches!(
        Death::exhausted(&one, Tick::new(1), [f64::NAN, 0.0], BudgetStage::Motion),
        Err(CoreError::NumericalInvalid { .. })
    ));
    for residual in [f64::NAN, -1.0, f64::INFINITY] {
        assert!(matches!(
            Death::removed(
                &one,
                Tick::new(1),
                one.position,
                residual,
                Disposition::Recycle
            ),
            Err(CoreError::NumericalInvalid { .. })
        ));
    }
    assert!(matches!(
        EnergyBudget::new(f64::NAN),
        Err(CoreError::NumericalInvalid { .. })
    ));
}

#[test]
fn updates_keep_lifelong_fields_fixed() {
    let world = World::new();
    let mut genomes = quota();
    let mut population = population(Arrangement::Compact, 4);
    population
        .admit(&mut vec![
            world.founder(1, 2.0, &mut genomes),
            world.founder(2, 2.0, &mut genomes),
        ])
        .expect("founders");
    let one = alive(&population, 1);
    let digest = population.digest();
    for changed in [
        Organism {
            genome: GenomeId::new(2),
            ..one
        },
        Organism {
            parent: Some(OrganismId::new(2)),
            ..one
        },
        Organism {
            lineage: LineageId::new(2),
            ..one
        },
        Organism {
            generation: Generation::new(1),
            ..one
        },
        Organism {
            birth_tick: Tick::new(1),
            ..one
        },
        Organism { phase: 3, ..one },
        Organism {
            id: OrganismId::new(9),
            ..one
        },
    ] {
        assert!(matches!(
            population.update(&changed),
            Err(CoreError::StateConflict { .. })
        ));
    }
    assert_eq!(population.digest(), digest);

    let moved = Organism {
        position: [3.25, 1.75],
        velocity: [0.5, 0.5],
        energy: 1.5,
        energy_change_rate: -64.0,
        held: held(1, 8),
        cooldown_deadline: Tick::new(300),
        ..one
    };
    // The digest sees every changing field on its own.
    let singles = [
        Organism {
            position: moved.position,
            ..one
        },
        Organism {
            velocity: moved.velocity,
            ..one
        },
        Organism {
            energy: moved.energy,
            ..one
        },
        Organism {
            energy_change_rate: moved.energy_change_rate,
            ..one
        },
        Organism {
            held: moved.held,
            ..one
        },
        Organism {
            held: held(1, 9),
            ..one
        },
        Organism {
            cooldown_deadline: moved.cooldown_deadline,
            ..one
        },
    ];
    let mut seen = vec![digest];
    for single in singles {
        population.update(&single).expect("update");
        let changed = population.digest();
        assert!(!seen.contains(&changed), "{single:?}");
        seen.push(changed);
    }
    population.update(&one).expect("restore");
    assert_eq!(
        population.digest(),
        digest,
        "the digest is a function of state"
    );
    population.update(&moved).expect("update");
    assert_eq!(alive(&population, 1), moved);
    assert_ne!(population.digest(), digest);
    assert_eq!(alive(&population, 2), founder_row(2, 2.0));
}

#[test]
fn births_and_deaths_commit_together_with_the_birth_transaction() {
    let world = World::new();
    let mut digests = Vec::new();
    for arrangement in [Arrangement::Compact, Arrangement::FreeList] {
        let mut genomes = quota();
        let mut population = population(arrangement, 8);
        population
            .admit(&mut vec![
                world.founder(1, 10.0, &mut genomes),
                world.founder(2, 6.0, &mut genomes),
                world.founder(3, 6.0, &mut genomes),
            ])
            .expect("founders");
        let tick = Tick::new(COOLDOWN);

        // 1 reproduces; 2 is exhausted at maintenance.
        let reproduction = Reproduction::new(rules()).expect("rules");
        let rng = SemanticRng::new(SEED);
        let one = alive(&population, 1);
        let ReproductionProposal::Intent(intent) = reproduction
            .propose(
                &rng,
                &parent_state(&one, one.position),
                tick,
                &sure_attempt(0.25),
                &budget(one.energy),
            )
            .expect("propose")
        else {
            panic!("a sure, viable attempt");
        };
        let mutator = Mutator::new(
            SemanticRng::new(SEED),
            MutationParameters {
                topology_change_probability: 0.0,
                weight_probability: 0.05,
                weight_standard_deviation: 0.02,
                replacement_probability: 0.0,
                chemistry_standard_deviation: 0.0,
                sensing_log_scale_standard_deviation: 0.0,
                reject_effective_noop: true,
                maximum_candidates: MAXIMUM_CANDIDATES,
            },
            &world.catalog,
            world.limits,
        )
        .expect("mutator");
        let grid = FieldGrid::from_config(&world.config).expect("grid");
        let context = BirthContext {
            rng: &rng,
            grid: &grid,
            reproduction: &reproduction,
            mutator: &mutator,
            offspring: Offspring::Inherited,
        };
        let parent_genome = population.genome(one.id).expect("genome");
        let mut identities = Identities {
            organisms: IdSequence::starting_at(4),
            genomes: IdSequence::starting_at(4),
        };
        let resolution = resolve_births(
            &context,
            &[BirthRequest {
                intent,
                genome: parent_genome,
            }],
            PopulationQuota::new(8, population.len() as u64).expect("quota"),
            &mut identities,
            &mut genomes,
        )
        .expect("resolve");
        let BirthResolution::Staged(births) = resolution else {
            panic!("room for the birth");
        };
        let mut ledger = TickLedger::open(
            tick,
            holdings(0.0, organisms_total(&population)),
            ResidualBudget::new(0.0, 1.0e-15, 1.0e-15).expect("budget"),
        );
        let kept = births
            .settle(&mut ledger, |id| {
                population.get(id).map(|organism| budget(organism.energy))
            })
            .expect("settle");
        let two = alive(&population, 2);
        let mut dying = budget(two.energy);
        let maintenance = dying.pay(8.0).expect("pay");
        ledger
            .record(Flow::Maintenance, maintenance.paid)
            .expect("record");
        let death = Death::exhausted(
            &two,
            tick.next().expect("tick"),
            two.position,
            BudgetStage::Maintenance,
        )
        .expect("death");

        // Phase 8 commits the parent's kept share and deadline, the newborn,
        // and the death together.
        let (born, _) = births.into_parts();
        let mut entrants = Vec::new();
        for (birth, (parent, kept)) in born.into_iter().zip(kept) {
            let row = alive(&population, parent.get());
            population
                .update(&Organism {
                    energy: kept.remaining(),
                    cooldown_deadline: birth.parent_cooldown_deadline,
                    ..row
                })
                .expect("the parent keeps its share");
            let (entrant, record) = Entrant::born(birth);
            assert!(record.perturbed > 0, "a real mutation");
            assert_eq!(entrant.genome.parent(), Some(one.genome));
            entrants.push(entrant);
        }
        let collected = population
            .commit(tick.next().expect("tick"), &[death], &mut entrants)
            .expect("commit");
        for entry in collected {
            genomes.retire(entry.genome).expect("retire");
        }
        assert_eq!(
            population.ids().map(OrganismId::get).collect::<Vec<_>>(),
            [1, 3, 4]
        );
        assert_eq!(genomes.live(), 3, "one genome per live organism");
        let child = alive(&population, 4);
        assert_eq!(child.parent, Some(one.id));
        assert_eq!(child.lineage, one.lineage);
        assert_eq!(child.generation, Generation::new(1));
        assert_eq!(child.birth_tick, tick.next().expect("tick"));
        assert_eq!(child.held, HeldOutput::INVALID);
        assert_eq!(child.energy, intent.split.child);
        assert_eq!(
            population.genome(child.id).map(Genome::id),
            Some(GenomeId::new(4))
        );
        let parent = alive(&population, 1);
        assert_eq!(parent.energy, intent.split.parent);
        assert_eq!(child.cooldown_deadline, parent.cooldown_deadline);
        assert_eq!(parent.cooldown_deadline, Tick::new(COOLDOWN + 1 + COOLDOWN));

        let closure = ledger
            .close(holdings(0.0, organisms_total(&population)))
            .expect("the tick closes");
        assert!((closure.flow(Flow::ReproductionLoss) - 0.5).abs() < 1.0e-15);
        assert_eq!(closure.flow(Flow::Maintenance), 6.0);
        digests.push(population.digest());
    }
    assert_eq!(digests[0], digests[1], "both arrangements agree");
}

#[test]
fn population_capacity_comes_from_the_configuration() {
    let population = Population::from_config(Arrangement::FreeList, &appendix_a()).expect("pop");
    assert_eq!(population.capacity(), 16_384);
    assert_eq!(population.arrangement(), Arrangement::FreeList);
    assert!(population.is_empty());
    assert_eq!(population.latest(), None);
    assert!(population.slots().is_empty());
    let empty = Population::new(Arrangement::Compact, 0).expect("pop");
    assert_eq!(
        empty.digest(),
        Population::new(Arrangement::FreeList, 9)
            .expect("pop")
            .digest()
    );
}

#[test]
fn maintenance_is_billed_by_flow_and_a_partial_payment_covers_basal_first() {
    // Appendix A: the bill's total is `mandatory` bit for bit, because
    // Δt = 1/128 scales exactly.
    let config = appendix_a();
    let dt = config.report().timestep;
    let costs = MaintenanceCosts::from_config(&config).expect("costs");
    let parameters = u32::try_from(config.report().default_parameter_count).expect("count");
    for (scale, evaluated) in [(1.0, true), (1.0, false), (3.0, true), (0.75, false)] {
        let bill = costs.bill(dt, scale, parameters, evaluated).expect("bill");
        assert_eq!(
            bill.total().to_bits(),
            costs
                .mandatory(dt, scale, parameters, evaluated)
                .expect("mandatory")
                .to_bits()
        );
        assert_eq!(bill.basal, 0.005 / 128.0);
        assert_eq!(bill.sensor, 0.001 * scale / 128.0);
        assert_eq!(
            bill.controller,
            if evaluated {
                2.0e-9 * f64::from(parameters)
            } else {
                0.0
            }
        );
    }

    // Dyadic coefficients, so every attribution is exact.
    let costs = MaintenanceCosts::new(0.5, 0.25, 1.0, 1.0, 0.125).expect("costs");
    let step = TimeStep::new(1, 128).expect("Δt");
    let bill = costs.bill(step, 1.0, 4, true).expect("bill");
    assert_eq!(
        bill,
        MaintenanceBill {
            basal: 0.00390625,
            sensor: 0.001953125,
            controller: 0.5,
        }
    );
    for (energy, paid, survives) in [
        // Paid in full.
        (
            1.0,
            MaintenanceBill {
                basal: 0.00390625,
                sensor: 0.001953125,
                controller: 0.5,
            },
            true,
        ),
        // Short of the controller term.
        (
            0.5,
            MaintenanceBill {
                basal: 0.00390625,
                sensor: 0.001953125,
                controller: 0.494140625,
            },
            false,
        ),
        // Short of the sensor term: basal first, then what is left.
        (
            0.0048828125,
            MaintenanceBill {
                basal: 0.00390625,
                sensor: 0.0009765625,
                controller: 0.0,
            },
            false,
        ),
        // Short of the basal term itself.
        (
            0.001953125,
            MaintenanceBill {
                basal: 0.001953125,
                sensor: 0.0,
                controller: 0.0,
            },
            false,
        ),
    ] {
        let mut organism = budget(energy);
        let settlement = bill.settle(&mut organism).expect("settle");
        assert_eq!(settlement.cost.requested, bill.total());
        assert_eq!(settlement.cost.paid, energy.min(bill.total()));
        assert_eq!(settlement.paid, paid, "{energy}");
        let mut vitality = Vitality::new();
        assert_eq!(
            vitality
                .check(BudgetStage::Maintenance, &organism)
                .expect("check"),
            survives
        );
        let mut ledger =
            TickLedger::open(Tick::new(0), holdings(0.0, energy), ResidualBudget::EXACT);
        settlement.record(&mut ledger).expect("record");
        let closure = ledger
            .close(holdings(0.0, organism.remaining()))
            .expect("exact");
        assert_eq!(closure.flow(Flow::Maintenance), paid.basal);
        assert_eq!(closure.flow(Flow::SensorCost), paid.sensor);
        assert_eq!(closure.flow(Flow::ControllerCost), paid.controller);
        // Unpaid maintenance is never dissipation.
        assert_eq!(closure.dissipated(), settlement.cost.paid);
    }

    // A dead organism pays nothing and records nothing.
    let mut dead = budget(0.0);
    let settlement = bill.settle(&mut dead).expect("settle");
    assert_eq!(settlement.cost.paid, 0.0);
    assert_eq!(
        settlement.paid,
        MaintenanceBill {
            basal: 0.0,
            sensor: 0.0,
            controller: 0.0,
        }
    );
    for broken in [
        MaintenanceBill {
            basal: f64::NAN,
            ..bill
        },
        MaintenanceBill {
            sensor: -1.0,
            ..bill
        },
        MaintenanceBill {
            controller: f64::INFINITY,
            ..bill
        },
    ] {
        let mut organism = budget(1.0);
        assert!(matches!(
            broken.settle(&mut organism),
            Err(CoreError::NumericalInvalid { .. })
        ));
        assert_eq!(organism.remaining(), 1.0, "a refused bill pays nothing");
    }
}
