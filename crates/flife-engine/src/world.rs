//! The ordered candidate tick and epoch publication (FL-013; plan §8.1–§8.4,
//! §9.4, §9.5, INV-01–INV-08, Appendix C.8).
//!
//! A [`World`] holds one committed state: the tick `n`, the committed field,
//! the live population with its genomes, the source paths, the identity
//! sequences, the genome quota and the energy ledger. [`World::advance`]
//! runs the ordered phases of §8.1 against that state and returns a
//! [`Candidate`] for tick `n + 1`. It reads only committed state. It writes
//! only the spare field buffer, the world's reusable scratch and the
//! candidate's own copies of the small committed records. So the committed
//! state stays authoritative until [`Candidate::publish`]:
//!
//! 1. Commands: none yet. FL-014 admits them at this boundary.
//! 2. Transport the committed field and apply the sources, sampled at
//!    mid-tick, into the spare buffer. That buffer is the tick's one
//!    observation field. Source power and decay are recorded.
//! 3. Build the sensing pyramid on the observation field.
//! 4. Observe every organism at its sensing scale. Evaluate its controller
//!    when it is due (`(n + 1) mod K` equals its phase) or when its held
//!    output is invalid (a newborn's first advance). Decode the held raw
//!    output for this tick.
//! 5. Request uptake and resolve the cell grants on the observation field.
//! 6. Settle chemistry per packet, then maintenance by flow.
//! 7. Motion, then emission, then the reproduction proposal.
//! 8. Clear tiny reservoirs and decide every fate. Stage the births as one
//!    transaction and settle their parents. Collect the deaths and settle
//!    their residuals.
//! 9. Debit the grants, then reduce the emission deposits, then the
//!    recycling deposits, into the next field.
//! 10. Check the next field (INV-01, INV-02), close the tick's ledger and
//!     the run's (INV-03), and check the population's whole commit (INV-01,
//!     INV-06, INV-07).
//!
//! Every organism spends through its own budget and rechecks exhaustion
//! after every stage (§8.2). Organisms interact only through the field
//! (INV-08). Each one reads the shared observation field and its own state.
//! It writes only its own budget, its grants and its deposits. Organisms are
//! visited in identity order, never storage order.
//!
//! **Cancellation and publication (§8.3, C.8).** Dropping a candidate, or
//! [`Candidate::cancel`], discards it. Nothing it staged becomes visible:
//! the committed field, population, quota, identities, paths and ledger are
//! exactly as before. Its staged genomes are freed with it, and their
//! charges never reached the world's quota. [`Candidate::publish`] first
//! commits the population as one checked transaction. Only then does it
//! commit the field, ledger, quota, identities, paths, tick, epoch and event
//! cursor, none of which can fail. A candidate holds the world mutably, so
//! it is the world's only writer, and nothing can change the base it was
//! prepared against (§9.5).
//!
//! **Pausing (§7.5).** Under `pause-before-commit`, a birth the fixed
//! logical quotas cannot cover pauses the tick. [`Advance::Paused`] reports
//! the shortfall and nothing changes. Retrying the unchanged world prepares
//! the same candidate, with the same proposals, randomness and identities.
//!
//! **Numerical faults (§7.6).** Under `stop-and-preserve`, a numerical
//! fault while preparing a candidate stops the run. The committed state is
//! kept bit for bit, the fault is recorded with its input tick, and every
//! later advance is refused.
//!
//! **Operator splitting (§8.4).** The schedule is first-order splitting at
//! the fixed timestep: transport and sources, then exchange, then each
//! organism's ordered stages. Fixed-step reproducibility is not a claim of
//! continuum accuracy.

use std::num::NonZeroUsize;
use std::ops::Range;

use flife_core::clock::{Tick, TimeStep, age_seconds, stable_phase};
use flife_core::ids::{
    Generation, GenomeId, IdSequence, LineageId, OrganismId, ResourceEpoch, SourceId,
};
use flife_core::ledger::CompensatedSum;
use flife_core::rng::{Address, Lane, LaneKind, SemanticRng, Stream, half_open_unit};
use flife_core::{
    CoreError, EnergyBudget, EnergyLedger, Flow, Holdings, LedgerReport, ResidualBudget,
    Sha256Digest, TickClosure, TickLedger, Transfer, ValidatedConfig,
};
use flife_field::{
    BufferLimits, ConeTolerance, Demand, Deposits, ExchangeStencil, FieldGrid, FieldPair,
    FieldState, FieldStepReport, FieldSummary, GrantPacket, Grants, PyramidView,
    SEEDED_SMOOTH_TORUS_V1, SEEDED_UNIT_VECTORS, SeededPath, SensorPyramid, Source, SourceKernel,
    SourceSupport, TransportPlan, TransportScratch, UptakeSummary, advance_field, apply_deposits,
    apply_grants, initial_source, resolve_uptake, saturating_request,
};
use flife_genome::{
    Genome, GenomeQuota, MutationParameters, MutationRecord, Mutator, TopologyCatalog, TraitLimits,
    Traits, founder, initial_scale,
};
use flife_neural::{
    ActionDecoding, ControllerScratch, DecodedActions, Intrinsic, SensorScales, decode, observe,
    widen,
};

use crate::chemistry::{Affinity, ChemicalPacket, Chemistry, composition};
use crate::costs::MaintenanceCosts;
use crate::emission::EmissionPolicy;
use crate::intervention::{self, AppliedIntervention, Intervention};
use crate::lifecycle::{BudgetStage, Death, Fate, Recycling, TinyEnergyPolicy, Vitality};
use crate::motion::{Motion, MotionResponse, RESPONSE_TERMS, uptake_ratio};
use crate::population::{Arrangement, Entrant, HeldOutput, Organism, Population};
use crate::reproduction::{
    BirthContext, BirthIntent, BirthRequest, BirthResolution, CapacityShortfall, Identities,
    ParentState, PopulationQuota, RejectedBirth, Reproduction, ReproductionProposal,
    resolve_births,
};

/// The backend this module implements: the scalar CPU reference.
pub const CPU_REFERENCE: &str = "cpu-reference";

/// The determinism class this module implements: one fixed order for every
/// reduction, no worker-order or clock dependence (§16.1).
pub const QUALIFIED_REFERENCE: &str = "qualified-reference";

/// The numerical fault policy this module implements (§7.6).
pub const STOP_AND_PRESERVE: &str = "stop-and-preserve";

/// The capacity policy this module implements (§7.5).
pub const PAUSE_BEFORE_COMMIT: &str = "pause-before-commit";

/// The initial positions this module implements (A.3 item 1).
pub const UNIFORM_POSITIONS: &str = "uniform";

/// The initial velocity this module implements (A.3 item 1).
pub const ZERO_VELOCITY: &str = "zero";

/// The world digest's schema line.
pub const WORLD_DIGEST_SCHEMA: &str = "frankenlife.world-state.v1";

/// The numerical profile a world is checked against (§21.4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NumericalProfile {
    /// The ledger's residual budget (INV-03).
    pub residual: ResidualBudget,
    /// The committed field's cone tolerance (INV-02).
    pub cone: ConeTolerance,
}

impl NumericalProfile {
    /// §21.4's initial gate for F32 production: `10⁻⁵` of the scale per tick
    /// and cumulatively, and a cone residual within `10⁻⁵` of the cell's
    /// energy, each above an absolute floor of the smallest normal F32.
    ///
    /// # Errors
    /// None in practice; the constructors validate their constants.
    pub fn reference_f32() -> Result<Self, CoreError> {
        let floor = f64::from(f32::MIN_POSITIVE);
        Ok(Self {
            residual: ResidualBudget::new(floor, 1.0e-5, 1.0e-5)?,
            cone: ConeTolerance::new(floor, 1.0e-5)?,
        })
    }
}

/// A numerical fault that stopped the run under `stop-and-preserve`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumericalFault {
    /// The committed input tick of the advance that faulted.
    pub tick: Tick,
    /// The fault.
    pub error: CoreError,
}

/// One birth a published tick committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BirthEvent {
    /// The child.
    pub child: OrganismId,
    /// Its parent.
    pub parent: OrganismId,
    /// The child's genome.
    pub genome: GenomeId,
    /// How the genome was made.
    pub record: MutationRecord,
}

/// What one candidate tick did. A published tick's report is the tick's
/// state, events and accounting, published together.
#[derive(Debug, Clone, PartialEq)]
pub struct TickReport {
    /// The committed input tick `n`.
    pub from: Tick,
    /// The tick `n + 1` the candidate publishes.
    pub tick: Tick,
    /// The resource epoch the candidate publishes as.
    pub epoch: ResourceEpoch,
    /// The event cursor's range for this tick: deaths in identity order,
    /// then births in identity order.
    pub events: Range<u64>,
    /// Live organisms after the tick.
    pub live: usize,
    /// Controller evaluations, regular and forced.
    pub evaluated: usize,
    /// Organisms that emitted.
    pub emitted: usize,
    /// Viable birth attempts.
    pub attempted: usize,
    /// The births, in identity order.
    pub births: Vec<BirthEvent>,
    /// Attempts whose every mutation candidate was rejected.
    pub rejected: Vec<RejectedBirth>,
    /// The deaths, in identity order.
    pub deaths: Vec<Death>,
    /// The uptake resolution.
    pub uptake: UptakeSummary,
    /// Phase 2's field terms.
    pub field_step: FieldStepReport,
    /// The next field's invariant check.
    pub field: FieldSummary,
    /// The tick's closed ledger.
    pub closure: TickClosure,
    /// The run's ledger after the tick.
    pub ledger: LedgerReport,
    /// The interventions applied at this tick's boundary, in their sealed
    /// order (FL-014).
    pub interventions: Vec<AppliedIntervention>,
}

/// The outcome of [`World::advance`].
#[derive(Debug)]
pub enum Advance<'w> {
    /// A complete, checked candidate.
    Ready(Candidate<'w>),
    /// A birth the fixed logical quotas cannot cover paused the tick. Nothing
    /// changed.
    Paused(CapacityShortfall),
}

/// The outcome of [`World::step`].
#[derive(Debug)]
pub enum Stepped {
    /// The tick was published.
    Published(Box<TickReport>),
    /// The tick paused; nothing changed.
    Paused(CapacityShortfall),
}

/// One prepared candidate tick. It holds its world mutably: until it is
/// published or dropped, nothing else can read or write the world.
#[derive(Debug)]
pub struct Candidate<'w> {
    world: &'w mut World,
    staged: Box<Staged>,
}

#[derive(Debug)]
struct Staged {
    tick: Tick,
    epoch: ResourceEpoch,
    events: u64,
    births: Vec<Entrant>,
    ledger: EnergyLedger,
    quota: GenomeQuota,
    identities: Identities,
    paths: Vec<SeededPath>,
    report: TickReport,
}

enum Prepared {
    Ready(Box<Staged>),
    Paused(CapacityShortfall),
}

impl Candidate<'_> {
    /// What the candidate did and would publish.
    #[must_use]
    pub fn report(&self) -> &TickReport {
        &self.staged.report
    }

    /// The candidate's next field, after grants and deposits.
    #[must_use]
    pub fn field(&self) -> &FieldState {
        self.world.field.next()
    }

    /// The committed tick and epoch it was prepared against.
    #[must_use]
    pub const fn base(&self) -> (Tick, ResourceEpoch) {
        (self.world.tick, self.world.epoch)
    }

    /// Discard the candidate. The committed world stays authoritative.
    pub fn cancel(self) {}

    /// Publish the candidate as the next epoch: the population's whole
    /// commit first, then everything else together.
    ///
    /// # Errors
    /// As [`Population::commit_tick`], which the candidate already passed.
    /// On error nothing is published and the committed world is unchanged.
    pub fn publish(self) -> Result<TickReport, CoreError> {
        let Self { world, staged } = self;
        let mut staged = *staged;
        world.population.commit_tick(
            staged.tick,
            &world.scratch.survivors,
            &world.scratch.deaths,
            &mut staged.births,
            &mut staged.quota,
        )?;
        world.field.commit();
        world.ledger = staged.ledger;
        world.quota = staged.quota;
        world.identities = staged.identities;
        world.paths = staged.paths;
        world.tick = staged.tick;
        world.epoch = staged.epoch;
        world.events = staged.events;
        Ok(staged.report)
    }
}

/// The run-constant operators a configuration declares.
#[derive(Debug)]
struct Rules {
    step: TimeStep,
    grid: FieldGrid,
    rng: SemanticRng,
    plan: TransportPlan,
    kernel: SourceKernel,
    chemistry: Chemistry,
    costs: MaintenanceCosts,
    motion: Motion,
    emission: EmissionPolicy,
    reproduction: Reproduction,
    tiny: TinyEnergyPolicy,
    recycling: Recycling,
    decoding: ActionDecoding,
    scales: SensorScales,
    catalog: TopologyCatalog,
    limits: TraitLimits,
    mutation: MutationParameters,
    uptake_capacity: f64,
    half_saturation: f64,
    interval: u32,
    maximum_population: u64,
    profile: NumericalProfile,
}

impl Rules {
    fn from_config(config: &ValidatedConfig, profile: NumericalProfile) -> Result<Self, CoreError> {
        let model = config.config();
        let named = |role: &'static str, name: &str, implemented: &str| {
            if name == implemented {
                Ok(())
            } else {
                Err(CoreError::UnknownAlgorithm {
                    role,
                    name: name.to_owned(),
                })
            }
        };
        named("execution.backend", &model.execution.backend, CPU_REFERENCE)?;
        named(
            "execution.determinism",
            &model.execution.determinism,
            QUALIFIED_REFERENCE,
        )?;
        named(
            "death.numerical_fault_policy",
            &model.death.numerical_fault_policy,
            STOP_AND_PRESERVE,
        )?;
        named(
            "population.capacity_policy",
            &model.population.capacity_policy,
            PAUSE_BEFORE_COMMIT,
        )?;
        named(
            "population.initial_positions",
            &model.population.initial_positions,
            UNIFORM_POSITIONS,
        )?;
        named(
            "population.initial_velocity",
            &model.population.initial_velocity,
            ZERO_VELOCITY,
        )?;
        named(
            "sources.chemical_directions",
            &model.sources.chemical_directions,
            SEEDED_UNIT_VECTORS,
        )?;
        named("sources.path", &model.sources.path, SEEDED_SMOOTH_TORUS_V1)?;
        if model.population.automatic_reseed {
            return Err(CoreError::MissingCapability {
                capability: "population.automatic_reseed: the baseline never reseeds".into(),
            });
        }
        let chemistry = &model.chemistry;
        Ok(Self {
            step: config.report().timestep,
            grid: FieldGrid::from_config(config)?,
            rng: SemanticRng::from_config(config)?,
            plan: TransportPlan::from_config(config)?,
            kernel: SourceKernel::from_config(config)?,
            chemistry: Chemistry::from_config(config)?,
            costs: MaintenanceCosts::from_config(config)?,
            motion: Motion::from_config(config)?,
            emission: EmissionPolicy::from_config(config)?,
            reproduction: Reproduction::from_config(config)?,
            tiny: TinyEnergyPolicy::from_config(config)?,
            recycling: Recycling::from_config(config)?,
            decoding: ActionDecoding::from_config(config)?,
            scales: SensorScales::from_config(config)?,
            catalog: TopologyCatalog::from_config(config)?,
            limits: TraitLimits::from_config(config)?,
            mutation: MutationParameters::from_config(config)?,
            uptake_capacity: chemistry.uptake_capacity_per_second,
            half_saturation: chemistry.uptake_half_saturation_density,
            interval: model.controller.evaluation_interval_ticks,
            maximum_population: u64::from(model.population.maximum_count),
            profile,
        })
    }
}

/// Reusable per-tick buffers (§9.4). Their contents never outlive the
/// candidate that wrote them, except the survivors and deaths, which a
/// candidate's publication reads.
#[derive(Debug)]
struct Scratch {
    transport: TransportScratch,
    support: SourceSupport,
    pyramid: SensorPyramid,
    /// The committed rows, in identity order, that phases 3 and 4 read.
    rows: Vec<Organism>,
    /// One per CPU worker (`[execution] cpu_workers`).
    lanes: Vec<DecisionLane>,
    sampled: Vec<Source>,
    working: Vec<Working>,
    demands: Vec<Demand>,
    grants: Grants,
    order: Vec<usize>,
    packets: Vec<ChemicalPacket>,
    emissions: Deposits,
    recycling: Deposits,
    survivors: Vec<Organism>,
    deaths: Vec<Death>,
}

/// Phases 3 and 4 for one organism: what it observed, and its held or newly
/// evaluated controller output decoded into actions.
#[derive(Debug, Clone, Copy)]
struct Decision {
    held: HeldOutput,
    evaluated: bool,
    actions: DecodedActions,
    gradients: [[f64; 2]; RESPONSE_TERMS],
}

/// One CPU worker's scratch for phases 3 and 4.
#[derive(Debug, Default)]
struct DecisionLane {
    controller: ControllerScratch,
    decisions: Vec<Decision>,
}

/// The fewest organisms worth handing to another thread.
const MINIMUM_ROWS_PER_LANE: usize = 128;

/// One organism's state during a candidate tick.
#[derive(Debug, Clone, Copy)]
struct Working {
    /// Its committed row at tick `n`.
    committed: Organism,
    budget: EnergyBudget,
    vitality: Vitality,
    held: HeldOutput,
    evaluated: bool,
    actions: DecodedActions,
    gradients: [[f64; 2]; RESPONSE_TERMS],
    absorbed: f64,
    position: [f64; 2],
    velocity: [f64; 2],
    cooldown_deadline: Tick,
    emitted: bool,
    intent: Option<BirthIntent>,
}

/// A committed world and the operators that advance it.
#[derive(Debug)]
pub struct World {
    rules: Rules,
    tick: Tick,
    epoch: ResourceEpoch,
    events: u64,
    field: FieldPair,
    population: Population,
    sources: Vec<Source>,
    paths: Vec<SeededPath>,
    identities: Identities,
    quota: GenomeQuota,
    ledger: EnergyLedger,
    fault: Option<NumericalFault>,
    scratch: Scratch,
}

impl World {
    /// Found a world at tick zero as a validated configuration declares it:
    /// the initial field; `population.initial_count` founders with
    /// identities, genomes and lineages `0, 1, …`, uniform positions, zero
    /// velocity and history, the initial energy, birth tick zero, cooldown
    /// deadline `⌈cooldown/Δt⌉`, invalid held outputs and stable phases (A.3
    /// item 1); and the sources at their seeded centers and paths.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for a backend, determinism class,
    /// fault policy, capacity policy, initialization or source family this
    /// module does not implement; [`CoreError::MissingCapability`] for
    /// automatic reseeding; and the errors of the operators' constructors,
    /// the founders' genomes and the population's admission.
    pub fn found(
        config: &ValidatedConfig,
        arrangement: Arrangement,
        profile: NumericalProfile,
    ) -> Result<Self, CoreError> {
        let rules = Rules::from_config(config, profile)?;
        let model = config.config();
        let limits = BufferLimits::host();
        let mut field = FieldPair::zeroed(rules.grid, &limits)?;
        let (initial, _) = FieldState::initial(config, &limits)?;
        field.current_mut().copy_from(&initial)?;
        drop(initial);

        let mut quota = GenomeQuota::from_config(config)?;
        let mut identities = Identities {
            organisms: IdSequence::starting_at(0),
            genomes: IdSequence::starting_at(0),
        };
        let mut population = Population::from_config(arrangement, config)?;
        let count =
            usize::try_from(model.population.initial_count).map_err(|_| CoreError::Overflow {
                quantity: "population.initial_count",
            })?;
        let mut entrants = Vec::new();
        entrants
            .try_reserve_exact(count)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "founders",
                bytes: u64::try_from(count.saturating_mul(size_of::<Entrant>()))
                    .unwrap_or(u64::MAX),
            })?;
        let scale = initial_scale(config);
        let cooldown_deadline = Tick::new(0).checked_add(config.report().cooldown_ticks)?;
        for _ in 0..count {
            let id = identities.organisms.issue()?;
            let genome_id = identities.genomes.issue()?;
            let genome = founder(
                &rules.rng,
                id,
                genome_id,
                &rules.catalog,
                &mut quota,
                &rules.limits,
                scale,
            )?;
            let organism = Organism {
                id,
                genome: genome_id,
                parent: None,
                lineage: LineageId::new(id.get()),
                generation: Generation::FOUNDER,
                birth_tick: Tick::new(0),
                position: founder_position(&rules.rng, &rules.grid, id)?,
                velocity: [0.0, 0.0],
                energy: model.population.initial_energy,
                energy_change_rate: 0.0,
                held: HeldOutput::INVALID,
                phase: stable_phase(id, rules.interval)?,
                cooldown_deadline,
            };
            entrants.push(Entrant { organism, genome });
        }
        population.admit(&mut entrants)?;

        let mut sources = Vec::new();
        let mut paths = Vec::new();
        for index in 0..model.sources.count {
            let id = SourceId::new(index);
            sources.push(initial_source(
                &rules.rng,
                &rules.grid,
                id,
                model.sources.power_per_source,
            )?);
            paths.push(SeededPath::start(
                &rules.rng,
                &rules.grid,
                id,
                model.sources.path_speed,
                config.report().source_segment_ticks,
                rules.step,
            )?);
        }

        let holdings = Holdings::new(field.current().energy_total(), organism_total(&population))?;
        let ledger = EnergyLedger::new(holdings, profile.residual);
        let scratch = Scratch {
            transport: TransportScratch::new(&rules.grid, &limits)?,
            support: SourceSupport::new(),
            pyramid: SensorPyramid::from_config(config, &limits)?,
            rows: Vec::new(),
            lanes: lanes(model.execution.cpu_workers)?,
            sampled: Vec::with_capacity(sources.len()),
            working: Vec::new(),
            demands: Vec::new(),
            grants: Grants::new(),
            order: Vec::new(),
            packets: Vec::new(),
            emissions: Deposits::new(),
            recycling: Deposits::new(),
            survivors: Vec::new(),
            deaths: Vec::new(),
        };
        Ok(Self {
            rules,
            tick: Tick::new(0),
            epoch: ResourceEpoch::new(0),
            events: 0,
            field,
            population,
            sources,
            paths,
            identities,
            quota,
            ledger,
            fault: None,
            scratch,
        })
    }

    /// The committed tick.
    #[must_use]
    pub const fn tick(&self) -> Tick {
        self.tick
    }

    /// The committed resource epoch: zero at founding, one more at every
    /// publication.
    #[must_use]
    pub const fn epoch(&self) -> ResourceEpoch {
        self.epoch
    }

    /// How many threads phases 3 and 4 use: `[execution] cpu_workers`
    /// unless [`World::set_cpu_workers`] changed it.
    #[must_use]
    pub fn cpu_workers(&self) -> usize {
        self.scratch.lanes.len()
    }

    /// Use `workers` threads for phases 3 and 4 from the next tick. No result
    /// depends on the count: each organism's decision reads only committed
    /// state, and the decisions are gathered in identity order.
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] if the lanes cannot be reserved; the
    /// count is then unchanged.
    pub fn set_cpu_workers(&mut self, workers: NonZeroUsize) -> Result<(), CoreError> {
        let lanes = &mut self.scratch.lanes;
        let workers = workers.get();
        if workers > lanes.len() {
            lanes
                .try_reserve_exact(workers - lanes.len())
                .map_err(|_| CoreError::AllocationFailed {
                    buffer: "decision lanes",
                    bytes: u64::try_from(workers.saturating_mul(size_of::<DecisionLane>()))
                        .unwrap_or(u64::MAX),
                })?;
        }
        lanes.resize_with(workers, DecisionLane::default);
        Ok(())
    }

    /// The event cursor: births and deaths published so far.
    #[must_use]
    pub const fn events(&self) -> u64 {
        self.events
    }

    /// The timestep.
    #[must_use]
    pub const fn step_size(&self) -> TimeStep {
        self.rules.step
    }

    /// The speed limit (`chemistry.maximum_speed`): no organism moves
    /// farther than `Δt` times it in one tick (§5.4). Render epochs carry it
    /// so a viewer can tell whether a skipped-epoch gap is unambiguous
    /// (§13.5, C.15).
    #[must_use]
    pub const fn maximum_speed(&self) -> f64 {
        self.rules.motion.maximum_speed()
    }

    /// The committed field.
    #[must_use]
    pub const fn field(&self) -> &FieldState {
        self.field.current()
    }

    /// The live population.
    #[must_use]
    pub const fn population(&self) -> &Population {
        &self.population
    }

    /// The source paths at the committed tick's segments.
    #[must_use]
    pub fn paths(&self) -> &[SeededPath] {
        &self.paths
    }

    /// The identity sequences.
    #[must_use]
    pub const fn identities(&self) -> &Identities {
        &self.identities
    }

    /// The genome quota.
    #[must_use]
    pub const fn quota(&self) -> &GenomeQuota {
        &self.quota
    }

    /// The run's ledger.
    #[must_use]
    pub const fn ledger(&self) -> &EnergyLedger {
        &self.ledger
    }

    /// The fault that stopped the run, if one has.
    #[must_use]
    pub const fn fault(&self) -> Option<&NumericalFault> {
        self.fault.as_ref()
    }

    /// The committed scientific state: tick, field, population, identity
    /// sequences, genome quota, source paths and ledger holdings. Slots,
    /// the epoch and the event cursor are excluded.
    #[must_use]
    pub fn digest(&self) -> Sha256Digest {
        let field = self.field.current().as_slice();
        let mut bytes = Vec::with_capacity(256 + field.len() * 4);
        bytes.extend_from_slice(WORLD_DIGEST_SCHEMA.as_bytes());
        bytes.push(b'\n');
        bytes.extend_from_slice(&self.tick.get().to_le_bytes());
        for value in field {
            bytes.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        bytes.extend_from_slice(self.population.digest().as_bytes());
        for next in [
            self.identities.organisms.next_raw(),
            self.identities.genomes.next_raw(),
        ] {
            match next {
                Some(raw) => {
                    bytes.push(1);
                    bytes.extend_from_slice(&raw.to_le_bytes());
                }
                None => bytes.push(0),
            }
        }
        for value in [
            self.quota.logical_bytes(),
            self.quota.allocated_bytes(),
            self.quota.live(),
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for path in &self.paths {
            bytes.extend_from_slice(&path.source().get().to_le_bytes());
            bytes.extend_from_slice(&path.ordinal().to_le_bytes());
        }
        let holdings = self.ledger.holdings();
        for value in [holdings.field(), holdings.organisms()] {
            bytes.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        Sha256Digest::of(&bytes)
    }

    /// Prepare the candidate for the next tick (see the [module
    /// documentation](self)).
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] once the run has stopped; otherwise any
    /// error of the phases. On error nothing committed has changed. A
    /// [`CoreError::NumericalInvalid`] stops the run under
    /// `stop-and-preserve`.
    pub fn advance(&mut self) -> Result<Advance<'_>, CoreError> {
        self.advance_with(&[])
    }

    /// Prepare the candidate for the next tick with `interventions`, the
    /// sealed, admission-ordered set effective at the committed tick's
    /// boundary (FL-014). They apply in order with phase 2's external
    /// sources (see [`intervention`]).
    ///
    /// # Errors
    /// As [`World::advance`]. An intervention that fails
    /// [`Intervention::check`] is refused before anything is prepared and
    /// does not stop the run.
    pub fn advance_with(
        &mut self,
        interventions: &[Intervention],
    ) -> Result<Advance<'_>, CoreError> {
        if let Some(fault) = &self.fault {
            return Err(CoreError::StateConflict {
                operation: "advance world",
                detail: format!(
                    "the run stopped under {STOP_AND_PRESERVE} advancing tick {}: {}",
                    fault.tick.get(),
                    fault.error
                ),
            });
        }
        let grid = self.field.current().grid();
        for intervention in interventions {
            intervention.check(grid)?;
        }
        match self.prepare(interventions) {
            Ok(Prepared::Ready(staged)) => Ok(Advance::Ready(Candidate {
                world: self,
                staged,
            })),
            Ok(Prepared::Paused(shortfall)) => Ok(Advance::Paused(shortfall)),
            Err(error) => {
                if matches!(error, CoreError::NumericalInvalid { .. }) {
                    self.fault = Some(NumericalFault {
                        tick: self.tick,
                        error: error.clone(),
                    });
                }
                Err(error)
            }
        }
    }

    /// Advance and publish one tick, or report the pause.
    ///
    /// # Errors
    /// As [`World::advance`] and [`Candidate::publish`].
    pub fn step(&mut self) -> Result<Stepped, CoreError> {
        self.step_with(&[])
    }

    /// Advance with `interventions` and publish one tick, or report the
    /// pause.
    ///
    /// # Errors
    /// As [`World::advance_with`] and [`Candidate::publish`].
    pub fn step_with(&mut self, interventions: &[Intervention]) -> Result<Stepped, CoreError> {
        match self.advance_with(interventions)? {
            Advance::Ready(candidate) => Ok(Stepped::Published(Box::new(candidate.publish()?))),
            Advance::Paused(shortfall) => Ok(Stepped::Paused(shortfall)),
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the ten ordered phases of §8.1 read top to bottom; each stage is its own function"
    )]
    fn prepare(&mut self, interventions: &[Intervention]) -> Result<Prepared, CoreError> {
        let Self {
            rules,
            tick,
            epoch,
            events,
            field,
            population,
            sources,
            paths,
            identities,
            quota,
            ledger,
            scratch,
            ..
        } = self;
        let Scratch {
            transport,
            support,
            pyramid,
            rows,
            lanes,
            sampled,
            working,
            demands,
            grants,
            order,
            packets,
            emissions,
            recycling,
            survivors,
            deaths,
        } = scratch;
        let from = *tick;
        let to = from.next()?;
        let next_epoch =
            ResourceEpoch::new(epoch.get().checked_add(1).ok_or(CoreError::Overflow {
                quantity: "resource epoch",
            })?);
        let step = rules.step;
        emissions.clear();
        recycling.clear();

        // Phase 1 sealed `interventions` at this boundary; they apply below,
        // with phase 2's external sources.
        // Phase 2: transport, then the sources sampled at mid-tick, then the
        // interventions in their sealed order.
        let opening = Holdings::new(field.current().energy_total(), organism_total(population))?;
        let mut tick_ledger = ledger.open(from, opening)?;
        let mut next_paths = paths.clone();
        sampled.clear();
        for (path, source) in next_paths.iter_mut().zip(sources.iter()) {
            *path = path_at(*path, from, rules)?;
            let segment = path.segment();
            let start = path
                .ordinal()
                .checked_mul(segment.ticks())
                .ok_or(CoreError::Overflow {
                    quantity: "source path tick",
                })?;
            let u = segment.mid_tick(from.get() - start)?;
            sampled.push(source.moved_to(segment.position(&rules.grid, u)?)?);
        }
        let field_step = advance_field(
            field,
            &rules.plan,
            &rules.kernel,
            sampled,
            step,
            transport,
            support,
        )?;
        tick_ledger.record(Flow::SourcePower, field_step.injected_declared)?;
        tick_ledger.record(Flow::Decay, field_step.transport.decay_removed)?;
        let mut applied = Vec::new();
        {
            let (_, next) = field.stage();
            intervention::apply(from, interventions, next, &mut tick_ledger, &mut applied)?;
        }
        let (_, observed) = field.stage();
        let observed: &FieldState = observed;

        // Phases 3 and 4: the pyramid, then every organism's observation and
        // its due or forced evaluation, in identity order.
        let view = pyramid.build(observed)?;
        rows.clear();
        rows.extend(population.iter());
        decide_all(rules, population, &view, rows, [from, to], lanes)?;
        working.clear();
        let mut evaluated = 0;
        let decisions = lanes.iter().flat_map(|lane| lane.decisions.iter());
        for (&organism, decision) in rows.iter().zip(decisions) {
            evaluated += usize::from(decision.evaluated);
            working.push(Working {
                committed: organism,
                budget: EnergyBudget::new(organism.energy)?,
                vitality: Vitality::new(),
                held: decision.held,
                evaluated: decision.evaluated,
                actions: decision.actions,
                gradients: decision.gradients,
                absorbed: 0.0,
                position: organism.position,
                velocity: organism.velocity,
                cooldown_deadline: organism.cooldown_deadline,
                emitted: false,
                intent: None,
            });
        }
        if working.len() != rows.len() {
            return Err(CoreError::StateConflict {
                operation: "gather decisions",
                detail: format!("{} decisions for {} organisms", working.len(), rows.len()),
            });
        }

        // Phase 5: requests from the observation field, resolved per cell.
        demands.clear();
        for organism in working.iter() {
            let density = ExchangeStencil::at(&rules.grid, organism.position)?.density(observed)?;
            demands.push(Demand {
                organism: organism.committed.id,
                position: organism.position,
                amount: saturating_request(
                    step,
                    rules.uptake_capacity,
                    rules.half_saturation,
                    density,
                )?,
            });
        }
        let uptake = resolve_uptake(observed, demands, grants)?;
        by_demand(grants.packets(), order);

        // Phases 6 and 7: each organism's ordered budget stages.
        let mut cursor = 0;
        for (index, organism) in working.iter_mut().enumerate() {
            packets.clear();
            while let Some(packet) = order
                .get(cursor)
                .and_then(|&packet| grants.packets().get(packet))
                .filter(|packet| packet.demand == index)
            {
                packets.push(ChemicalPacket {
                    granted: packet.granted,
                    composition: composition(observed, packet.cell)?,
                });
                cursor += 1;
            }
            let genome = genome_of(population, organism.committed.id)?;
            let parameters = parameter_count(&rules.catalog, genome)?;
            settle_stages(
                rules,
                organism,
                genome.traits(),
                parameters,
                packets,
                from,
                &mut tick_ledger,
                emissions,
            )?;
        }

        // Phase 8: the last stages of organisms that do not reproduce, then
        // the births as one transaction, then the parents' last stages.
        let mut live = 0_u64;
        for organism in working.iter_mut() {
            if organism.intent.is_some() {
                live += 1;
            } else if organism.vitality.is_alive() {
                organism
                    .vitality
                    .check(BudgetStage::Split, &organism.budget)?;
                clear_tiny(&rules.tiny, organism, &mut tick_ledger)?;
                live += u64::from(organism.vitality.is_alive());
            }
        }
        let mut requests = Vec::new();
        for organism in working.iter() {
            if let Some(intent) = organism.intent {
                requests.push(BirthRequest {
                    intent,
                    genome: genome_of(population, intent.parent)?,
                });
            }
        }
        let mutator = Mutator::new(rules.rng, rules.mutation, &rules.catalog, rules.limits)?;
        let context = BirthContext {
            rng: &rules.rng,
            grid: &rules.grid,
            reproduction: &rules.reproduction,
            mutator: &mutator,
        };
        let mut next_identities = identities.clone();
        let mut next_quota = quota.clone();
        let births = match resolve_births(
            &context,
            &requests,
            PopulationQuota::new(rules.maximum_population, live)?,
            &mut next_identities,
            &mut next_quota,
        )? {
            BirthResolution::Staged(births) => births,
            BirthResolution::Paused(shortfall) => return Ok(Prepared::Paused(shortfall)),
        };
        let kept = births.settle(&mut tick_ledger, |id| {
            find(working, id).map(|organism| organism.budget)
        })?;
        for (birth, (parent, budget)) in births.born().iter().zip(kept) {
            let index = working
                .binary_search_by_key(&parent, |organism| organism.committed.id)
                .map_err(|_| CoreError::StateConflict {
                    operation: "settle birth",
                    detail: format!("{parent} is not in this tick"),
                })?;
            if let Some(organism) = working.get_mut(index) {
                organism.budget = budget;
                organism.cooldown_deadline = birth.parent_cooldown_deadline;
            }
        }
        for organism in working
            .iter_mut()
            .filter(|organism| organism.intent.is_some())
        {
            organism
                .vitality
                .check(BudgetStage::Split, &organism.budget)?;
            clear_tiny(&rules.tiny, organism, &mut tick_ledger)?;
        }

        // Every fate, in identity order; the deaths' residuals are settled.
        survivors.clear();
        deaths.clear();
        let mut organisms = CompensatedSum::ZERO;
        let seconds = step.seconds_per_tick();
        let mut emitted = 0;
        for organism in working.iter() {
            emitted += usize::from(organism.emitted);
            match organism.vitality.fate()? {
                Fate::Survives => {
                    let energy = organism.budget.remaining();
                    organisms.add(energy);
                    survivors.push(Organism {
                        position: organism.position,
                        velocity: organism.velocity,
                        energy,
                        energy_change_rate: (energy - organism.committed.energy) / seconds,
                        held: organism.held,
                        cooldown_deadline: organism.cooldown_deadline,
                        ..organism.committed
                    });
                }
                Fate::Dies(stage) => {
                    let death =
                        Death::exhausted(&organism.committed, to, organism.position, stage)?;
                    let affinity = genome_of(population, death.id)?.traits().affinity();
                    rules
                        .recycling
                        .settle(&rules.grid, &death, affinity, recycling)?
                        .record(&mut tick_ledger)?;
                    deaths.push(death);
                }
            }
        }
        let attempted = requests.len();
        let (born, rejected) = births.into_parts();
        let mut entrants = Vec::with_capacity(born.len());
        let mut born_events = Vec::with_capacity(born.len());
        for birth in born {
            let parent = birth.parent;
            let (entrant, record) = Entrant::born(birth);
            organisms.add(entrant.organism.energy);
            born_events.push(BirthEvent {
                child: entrant.organism.id,
                parent,
                genome: entrant.organism.genome,
                record,
            });
            entrants.push(entrant);
        }

        // Phase 9: the grants' debits, then the paid deposits, then
        // recycling, reduced into the next field.
        let (_, next) = field.stage();
        apply_grants(next, grants)?;
        apply_deposits(next, emissions)?.record(&mut tick_ledger, Transfer::Emission)?;
        apply_deposits(next, recycling)?.record(&mut tick_ledger, Transfer::Recycling)?;

        // Phase 10: invariants, accounting and the population's commit.
        let field_summary = next.check_invariants(rules.profile.cone)?;
        let closing = Holdings::new(next.energy_total(), organisms.value())?;
        let closure = tick_ledger.close(closing)?;
        let mut next_ledger = ledger.clone();
        let cumulative = next_ledger.commit(&closure)?;
        population.check_tick(to, survivors, deaths, &entrants, &next_quota)?;

        let count =
            u64::try_from(deaths.len() + entrants.len()).map_err(|_| CoreError::Overflow {
                quantity: "event cursor",
            })?;
        let end = events.checked_add(count).ok_or(CoreError::Overflow {
            quantity: "event cursor",
        })?;
        let report = TickReport {
            from,
            tick: to,
            epoch: next_epoch,
            events: *events..end,
            live: survivors.len() + entrants.len(),
            evaluated,
            emitted,
            attempted,
            births: born_events,
            rejected,
            deaths: deaths.clone(),
            uptake,
            field_step,
            field: field_summary,
            closure,
            ledger: cumulative,
            interventions: applied,
        };
        Ok(Prepared::Ready(Box::new(Staged {
            tick: to,
            epoch: next_epoch,
            events: end,
            births: entrants,
            ledger: next_ledger,
            quota: next_quota,
            identities: next_identities,
            paths: next_paths,
            report,
        })))
    }
}

/// A founder's initial position, uniform on the torus (A.3 item 1): words 0
/// and 1 of `(founders, organism, 0, lane (0, position, 0), 0)` as half-open
/// uniforms, scaled to the world.
///
/// # Errors
/// As [`FieldGrid::wrap_position`], which does not fail for these draws.
pub fn founder_position(
    rng: &SemanticRng,
    grid: &FieldGrid,
    organism: OrganismId,
) -> Result<[f64; 2], CoreError> {
    let [first, second, ..] = rng.block(Address {
        stream: Stream::Founders,
        entity: organism.get(),
        ordinal: 0,
        lane: Lane::new(0, LaneKind::Position, 0)?,
        draw: 0,
    });
    grid.wrap_position([
        half_open_unit(first) * grid.world_width(),
        half_open_unit(second) * grid.world_height(),
    ])
}

/// `path` advanced to the segment that contains `tick`.
fn path_at(mut path: SeededPath, tick: Tick, rules: &Rules) -> Result<SeededPath, CoreError> {
    loop {
        let end = path
            .ordinal()
            .checked_add(1)
            .and_then(|next| next.checked_mul(path.segment().ticks()))
            .ok_or(CoreError::Overflow {
                quantity: "source path tick",
            })?;
        if tick.get() < end {
            return Ok(path);
        }
        path = path.next(&rules.rng, &rules.grid, rules.step)?;
    }
}

/// `Σ E` over the live organisms in identity order, compensated: the
/// organisms' holdings.
fn organism_total(population: &Population) -> f64 {
    let mut total = CompensatedSum::ZERO;
    for organism in population.iter() {
        total.add(organism.energy);
    }
    total.value()
}

/// `workers` empty decision lanes.
fn lanes(workers: u32) -> Result<Vec<DecisionLane>, CoreError> {
    let workers = usize::try_from(workers).map_err(|_| CoreError::Overflow {
        quantity: "execution.cpu_workers",
    })?;
    let mut lanes = Vec::new();
    lanes
        .try_reserve_exact(workers)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: "decision lanes",
            bytes: u64::try_from(workers.saturating_mul(size_of::<DecisionLane>()))
                .unwrap_or(u64::MAX),
        })?;
    lanes.resize_with(workers.max(1), DecisionLane::default);
    Ok(lanes)
}

/// Phases 3 and 4 for one organism (§8.1): its observation at its sensing
/// scale, then its due or forced evaluation, decoded.
fn decide(
    rules: &Rules,
    population: &Population,
    view: &PyramidView<'_>,
    organism: &Organism,
    [from, to]: [Tick; 2],
    controller: &mut ControllerScratch,
) -> Result<Decision, CoreError> {
    let genome = genome_of(population, organism.id)?;
    let traits = genome.traits();
    let raw = view.sample(organism.position, traits.sensing_scale())?;
    let intrinsic = Intrinsic {
        energy: organism.energy,
        age_seconds: age_seconds(organism.birth_tick, from, rules.step)?,
        velocity: organism.velocity,
        energy_change_per_second: organism.energy_change_rate,
    };
    let observation = observe(&raw, &intrinsic, &rules.scales)?;
    let due = !organism.held.is_valid()
        || to.get() % u64::from(rules.interval) == u64::from(organism.phase);
    let held = if due {
        let outputs = genome
            .controller(&rules.catalog, &rules.limits)?
            .evaluate(&observation.inputs, controller)?;
        HeldOutput::evaluated(to, outputs)?
    } else {
        organism.held
    };
    let outputs = held.outputs().ok_or_else(|| CoreError::StateConflict {
        operation: "decode held output",
        detail: format!("{} holds no valid output", organism.id),
    })?;
    Ok(Decision {
        held,
        evaluated: due,
        actions: decode(&widen(outputs), rules.step, &rules.decoding)?,
        gradients: observation.gradients,
    })
}

/// Phases 3 and 4 for every row, on at most `lanes.len()` threads: this one
/// and scoped workers it joins before returning. That is plan §3.4's bounded
/// reference adapter, not Asupersync-scheduled parallelism. The rows are split
/// into contiguous identity-ordered runs, one per lane, and each lane's
/// decisions follow its run. An organism's decision reads only committed
/// state, so the split cannot change any result, and the error returned is the
/// first in identity order, as a single thread would report it.
fn decide_all(
    rules: &Rules,
    population: &Population,
    view: &PyramidView<'_>,
    rows: &[Organism],
    ticks: [Tick; 2],
    lanes: &mut [DecisionLane],
) -> Result<(), CoreError> {
    for lane in lanes.iter_mut() {
        lane.decisions.clear();
    }
    let used = lanes
        .len()
        .min(rows.len().div_ceil(MINIMUM_ROWS_PER_LANE))
        .max(1);
    let run = |rows: &[Organism], lane: &mut DecisionLane| -> Result<(), CoreError> {
        lane.decisions
            .try_reserve(rows.len())
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "decisions",
                bytes: u64::try_from(rows.len().saturating_mul(size_of::<Decision>()))
                    .unwrap_or(u64::MAX),
            })?;
        for organism in rows {
            let decision = decide(
                rules,
                population,
                view,
                organism,
                ticks,
                &mut lane.controller,
            )?;
            lane.decisions.push(decision);
        }
        Ok(())
    };
    let Some((first, others)) = lanes.split_first_mut() else {
        return Err(CoreError::StateConflict {
            operation: "decide",
            detail: "no decision lane".to_owned(),
        });
    };
    if used == 1 {
        return run(rows, first);
    }
    let length = rows.len().div_ceil(used);
    let mut runs = rows.chunks(length);
    let head = runs.next().unwrap_or(&[]);
    let run = &run;
    std::thread::scope(|scope| {
        let workers: Vec<_> = runs
            .zip(others.iter_mut())
            .map(|(rows, lane)| scope.spawn(move || run(rows, lane)))
            .collect();
        let mut result = run(head, first);
        for worker in workers {
            let joined = worker.join().unwrap_or_else(|_| {
                Err(CoreError::StateConflict {
                    operation: "decide",
                    detail: "a decision worker panicked".to_owned(),
                })
            });
            if result.is_ok() {
                result = joined;
            }
        }
        result
    })
}

fn genome_of(population: &Population, id: OrganismId) -> Result<&Genome, CoreError> {
    population
        .genome(id)
        .ok_or_else(|| CoreError::StateConflict {
            operation: "read genome",
            detail: format!("{id} has no genome"),
        })
}

fn parameter_count(catalog: &TopologyCatalog, genome: &Genome) -> Result<u32, CoreError> {
    let topology =
        catalog
            .topology(genome.topology())
            .ok_or_else(|| CoreError::LayoutMismatch {
                buffer: "genome parameters",
                detail: format!("member {} is not in the catalog", genome.topology().index()),
            })?;
    u32::try_from(topology.parameter_count()).map_err(|_| CoreError::Overflow {
        quantity: "controller parameter count",
    })
}

/// Packet indices grouped by demand, each group in cell order.
fn by_demand(packets: &[GrantPacket], order: &mut Vec<usize>) {
    order.clear();
    order.extend(0..packets.len());
    order.sort_unstable_by_key(|&index| {
        packets
            .get(index)
            .map_or((usize::MAX, usize::MAX), |packet| {
                (packet.demand, packet.cell)
            })
    });
}

fn find(working: &[Working], id: OrganismId) -> Option<&Working> {
    working
        .binary_search_by_key(&id, |organism| organism.committed.id)
        .ok()
        .and_then(|index| working.get(index))
}

/// The tiny-reservoir stage: the last of §8.2.
fn clear_tiny(
    tiny: &TinyEnergyPolicy,
    organism: &mut Working,
    ledger: &mut TickLedger,
) -> Result<(), CoreError> {
    let cleared = tiny.clear(&mut organism.budget)?;
    if cleared > 0.0 {
        ledger.record(Flow::TinyReservoir, cleared)?;
    }
    organism
        .vitality
        .check(BudgetStage::TinyReservoir, &organism.budget)?;
    Ok(())
}

/// Phases 6 and 7 for one organism: chemistry, maintenance, motion,
/// emission and the reproduction proposal, rechecking exhaustion after
/// each. Once a stage exhausts the organism nothing later pays or acts.
#[expect(
    clippy::too_many_arguments,
    reason = "private stage runner of World::prepare; the arguments are the tick's working state"
)]
fn settle_stages(
    rules: &Rules,
    organism: &mut Working,
    traits: &Traits,
    parameters: u32,
    packets: &[ChemicalPacket],
    from: Tick,
    ledger: &mut TickLedger,
    emissions: &mut Deposits,
) -> Result<(), CoreError> {
    let affinity = Affinity::new(*traits.affinity(), &rules.chemistry)?;
    let uptake = rules
        .chemistry
        .settle(&affinity, packets, &mut organism.budget)?;
    uptake.record(ledger)?;
    organism.absorbed = uptake.absorbed;
    if !organism
        .vitality
        .check(BudgetStage::Chemistry, &organism.budget)?
    {
        return Ok(());
    }
    rules
        .costs
        .bill(
            rules.step,
            traits.sensing_scale(),
            parameters,
            organism.evaluated,
        )?
        .settle(&mut organism.budget)?
        .record(ledger)?;
    if !organism
        .vitality
        .check(BudgetStage::Maintenance, &organism.budget)?
    {
        return Ok(());
    }

    let ratio = uptake_ratio(organism.absorbed, rules.step, rules.uptake_capacity)?;
    let response = MotionResponse::new(*traits.motion_response(), &rules.motion)?;
    let acceleration = rules
        .motion
        .acceleration(ratio, &response, &organism.gradients)?;
    let proposed = rules.motion.propose(organism.velocity, acceleration)?;
    let moved = rules.motion.settle(proposed, &mut organism.budget)?;
    ledger.record(Flow::MotionCost, moved.cost.paid)?;
    organism.velocity = moved.velocity;
    organism.position = rules
        .motion
        .displace(&rules.grid, organism.position, moved.velocity)?;
    if !organism
        .vitality
        .check(BudgetStage::Motion, &organism.budget)?
    {
        return Ok(());
    }

    let emission = rules.emission.emit(
        &rules.grid,
        organism.committed.id,
        organism.position,
        organism.actions.emission.as_ref(),
        &mut organism.budget,
        emissions,
    )?;
    organism.emitted = emission.emitted();
    if !organism
        .vitality
        .check(BudgetStage::Emission, &organism.budget)?
    {
        return Ok(());
    }

    let parent = ParentState {
        id: organism.committed.id,
        lineage: organism.committed.lineage,
        generation: organism.committed.generation,
        position: organism.position,
        cooldown_deadline: organism.cooldown_deadline,
    };
    if let ReproductionProposal::Intent(intent) = rules.reproduction.propose(
        &rules.rng,
        &parent,
        from,
        &organism.actions,
        &organism.budget,
    )? {
        organism.intent = Some(intent);
    }
    Ok(())
}
