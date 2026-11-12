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
//!    their residuals. Then add the introduced organisms (see
//!    [`intervention`]).
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

use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::ops::Range;

use flife_core::clock::{Tick, TimeStep, age_seconds, stable_phase};
use flife_core::ids::{
    Generation, GenomeId, IdSequence, LineageId, OrganismId, ResourceEpoch, SourceId,
};
use flife_core::ledger::CompensatedSum;
use flife_core::memory::{bytes_for, capacity_bytes, total};
use flife_core::model::{
    FIELD_ECOLOGY_V1, FIELD_ECOLOGY_V1_CHEMISTRY_ONLY, FIELD_ECOLOGY_V1_FRESH_OFFSPRING,
};
use flife_core::rng::{Address, Lane, LaneKind, SemanticRng, Stream, half_open_unit};
use flife_core::{
    CoreError, EnergyBudget, EnergyLedger, Flow, Holdings, LedgerReport, ResidualBudget,
    Sha256Digest, TickClosure, TickLedger, Transfer, ValidatedConfig,
};
use flife_field::{
    Bins, BufferLimits, ConeTolerance, Demand, Deposits, FieldBackend, FieldGrid, FieldPair,
    FieldState, FieldStepReport, FieldSummary, FieldWork, Grants, PyramidView, RawSample,
    RequestRule, SEEDED_SMOOTH_TORUS_V1, SEEDED_UNIT_VECTORS, SeededPath, SensorBackend,
    SensorPyramid, SensorQuery, Source, SourceKernel, TransportPlan, UptakeBackend, UptakeSummary,
    UptakeWork, advance_field_on, apply_deposits, apply_grants, demands as reference_demands,
    demands_on, initial_source, resolve_uptake, resolve_uptake_on,
};
use flife_genome::{
    CandidateRejection, Genome, GenomeQuota, MAXIMUM_CANDIDATES, MutationParameters,
    MutationRecord, Mutator, TopologyCatalog, TopologyId, TraitLimits, Traits, founder,
    initial_scale,
};
use flife_neural::{
    ACTION_WIDTH, ActionDecoding, ControllerBackend, ControllerJob, ControllerScratch,
    DecodedActions, Intrinsic, Observation, SensorScales, decode, observe, widen,
};

use crate::chemistry::{Affinity, ChemicalPacket, Chemistry, composition};
use crate::costs::MaintenanceCosts;
use crate::emission::EmissionPolicy;
use crate::inspect::{
    Activations, OrganismSnapshot, RECENT_STEPS, ReproductionOutcome, Sensed, Transactions,
};
use crate::intervention::{self, AppliedIntervention, Intervention};
use crate::lifecycle::{BudgetStage, Death, Fate, Recycling, TinyEnergyPolicy, Vitality};
use crate::motion::{Motion, MotionResponse, RESPONSE_TERMS, uptake_ratio};
use crate::population::{Arrangement, Entrant, HeldOutput, Organism, Population};
use crate::reproduction::{
    Birth, BirthContext, BirthIntent, BirthRequest, BirthResolution, Births, CapacityShortfall,
    Identities, Offspring, ParentState, PopulationQuota, RejectedBirth, Reproduction,
    ReproductionProposal, resolve_births,
};

mod admission;
mod checkpoint;

pub use checkpoint::{WORLD_CHECKPOINT_CHUNKS, WORLD_CHUNK_VERSION};

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

/// One organism a published tick introduced (FL-022's seeded
/// introduction): a founder of its own lineage, generation zero, born at the
/// tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntroductionEvent {
    /// The organism, also its lineage.
    pub organism: OrganismId,
    /// Its founder genome.
    pub genome: GenomeId,
    /// Its genome's topology.
    pub topology: TopologyId,
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
    /// then births in identity order, then introductions in identity order.
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
    /// The organisms introduced at the end of the tick, in identity order
    /// (sealed order, then each introduction's own order), after every
    /// birth.
    pub introductions: Vec<IntroductionEvent>,
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
    /// The watched organism's observation in this step, if it observed.
    sensed: Option<Sensed>,
    /// The watched organism's transactions in this step, if it settled.
    transactions: Option<Transactions>,
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
        if let Some(sensed) = staged.sensed {
            if sensed.evaluated {
                world.watch.evaluated = Some(sensed.clone());
            }
            world.watch.observed = Some(sensed);
        }
        if let Some(transactions) = staged.transactions {
            let recent = &mut world.watch.recent;
            if recent.len() >= RECENT_STEPS {
                recent.pop_front();
            }
            recent.push_back(transactions);
        }
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
    variant: ModelVariant,
    initial_scale: f64,
}

/// Which `field-ecology-v1` model a world runs: the baseline, or one of
/// FL-041's ablation models, each changing exactly one rule (plan §15.3,
/// §21.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelVariant {
    /// `field-ecology-v1`.
    Baseline,
    /// `field-ecology-v1-chemistry-only`: controllers are evaluated and
    /// charged as in the baseline, and their actions decoded from an
    /// all-zero raw output.
    ChemistryOnly,
    /// `field-ecology-v1-fresh-offspring`: a newborn's genome is a
    /// founder's genome drawn at its own identity.
    FreshOffspring,
}

impl ModelVariant {
    /// The variant `model` names.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for another model.
    pub fn of(model: &str) -> Result<Self, CoreError> {
        match model {
            FIELD_ECOLOGY_V1 => Ok(Self::Baseline),
            FIELD_ECOLOGY_V1_CHEMISTRY_ONLY => Ok(Self::ChemistryOnly),
            FIELD_ECOLOGY_V1_FRESH_OFFSPRING => Ok(Self::FreshOffspring),
            other => Err(CoreError::UnknownAlgorithm {
                role: "model",
                name: other.to_owned(),
            }),
        }
    }
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
            variant: ModelVariant::of(&model.model)?,
            initial_scale: initial_scale(config),
        })
    }
}

/// Reusable per-tick buffers (§9.4). Their contents never outlive the
/// candidate that wrote them, except the survivors and deaths, which a
/// candidate's publication reads.
#[derive(Debug)]
struct Scratch {
    /// Transport scratch per worker and a support per source.
    field: FieldWork,
    pyramid: SensorPyramid,
    /// The committed rows, in identity order, that phases 3 and 4 read.
    rows: Vec<Organism>,
    /// One per CPU worker (`[execution] cpu_workers`).
    lanes: Vec<DecisionLane>,
    sampled: Vec<Source>,
    working: Vec<Working>,
    demands: Vec<Demand>,
    grants: Grants,
    /// Each organism's grant packets, grouped by demand in cell order.
    by_demand: Bins,
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
    /// The batched path's observations of this lane's run.
    observed: Vec<Observation>,
    decisions: Vec<Decision>,
}

/// What the world's controller backend did, for run records. Not
/// scientific state: no result depends on the backend.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ControllerReport {
    /// The backend serving the world, or `None` for the reference alone.
    pub backend: Option<String>,
    /// Evaluations the backend returned.
    pub batched: u64,
    /// Evaluations it deferred, which the reference did.
    pub deferred: u64,
    /// Ticks whose phases 3 and 4 the reference redid whole, because the
    /// batched path met an error: the reference reproduces it exactly, or
    /// finds none (an allocation that failed only on the batched path).
    pub reference_ticks: u64,
    /// Why the backend stopped serving the world, if it failed.
    pub failure: Option<String>,
    /// The most bytes the backend has held after a batch, host and device
    /// ([`ControllerBackend::resident_bytes`], FL-043).
    pub resident_peak_bytes: u64,
}

/// What a world holds on the host, by plan §19.4's terms (FL-043): each
/// buffer's capacity times its element size, in bytes. Genome parameters
/// are apart from it: the genome quota charges them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Footprint {
    /// The population (its columns, genome arena and each organism's index
    /// entries and genome header), the sources and their paths.
    pub state: u64,
    /// The committed and candidate fields, the field step's per-worker
    /// transport scratch and source supports, and the sensor pyramid.
    pub field: u64,
    /// The tick's scratch: rows, decision lanes, demands, grants, packets,
    /// deposits, survivors and deaths, and the controller and sensor
    /// backends' vectors.
    pub scratch: u64,
    /// What a step holds only while it runs: birth requests, births and
    /// rejected births with their mutation records, newborn entrants,
    /// events, the report's deaths and the controller batch's jobs.
    pub staging: u64,
}

impl Footprint {
    /// Every term.
    #[must_use]
    pub fn total(&self) -> u64 {
        total([self.state, self.field, self.scratch, self.staging])
    }
}

/// [`Footprint::staging`] per organism the population can hold. Every list
/// a step fills is reserved for its count before it is filled, so each is
/// charged once: per organism, at most one birth request, its place in the
/// resolution order and in a pause's proposal; a place in both the births
/// and the rejected births, since either may take every request; one boxed
/// genome and at most [`MAXIMUM_CANDIDATES`] candidate rejections, as a
/// birth's record or a rejected birth's (a list grown by pushing to at most
/// that many holds no more); one newborn entrant, and one more while an
/// introduction's are staged; one birth or introduction event, one death in
/// the report and one controller job. A list's minimum capacity of a few
/// elements is a constant the reserve covers.
const STAGING_PER_ORGANISM: u64 = {
    bytes_for::<BirthRequest<'static>>(1)
        + bytes_for::<usize>(1)
        + bytes_for::<OrganismId>(1)
        + bytes_for::<Birth>(1)
        + bytes_for::<RejectedBirth>(1)
        + bytes_for::<Genome>(1)
        + bytes_for::<CandidateRejection>(MAXIMUM_CANDIDATES as usize)
        + 2 * bytes_for::<Entrant>(1)
        + bytes_for::<BirthEvent>(1)
        + bytes_for::<IntroductionEvent>(1)
        + bytes_for::<Death>(1)
        + bytes_for::<ControllerJob<'static>>(1)
};

/// What the world's field backend did, for run records (FL-031). Not
/// scientific state: no result depends on the backend.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldReport {
    /// The backend serving the world, or `None` for the reference alone.
    pub backend: Option<String>,
    /// Field steps (phase 2) the backend computed.
    pub steps: u64,
    /// Steps the reference redid because the backend met an error: the
    /// reference reproduces that error exactly, or finds none.
    pub reference_steps: u64,
    /// Why the backend stopped serving the world, if it failed.
    pub failure: Option<String>,
}

/// The world's field backend.
#[derive(Debug, Default)]
struct Fields {
    backend: Option<Box<dyn FieldBackend>>,
    report: FieldReport,
}

/// What the world's sensor backend did, for run records (FL-031). Not
/// scientific state: no result depends on the backend.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SensorReport {
    /// The backend serving the world, or `None` for the reference alone.
    pub backend: Option<String>,
    /// Ticks whose phase 3 samples the backend computed.
    pub ticks: u64,
    /// Ticks the reference sampled because the backend met an error: the
    /// reference reproduces that error exactly, or finds none.
    pub reference_ticks: u64,
    /// Why the backend stopped serving the world, if it failed.
    pub failure: Option<String>,
}

/// The world's sensor backend and the tick's queries and samples.
#[derive(Debug, Default)]
struct Sensors {
    backend: Option<Box<dyn SensorBackend>>,
    report: SensorReport,
    queries: Vec<SensorQuery>,
    samples: Vec<RawSample>,
}

/// What the world's uptake backend did, for run records (FL-031). Not
/// scientific state: no result depends on the backend.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UptakeReport {
    /// The backend serving the world, or `None` for the reference alone.
    pub backend: Option<String>,
    /// Ticks whose phase 5 demands and grants the backend computed.
    pub ticks: u64,
    /// Ticks the reference computed because the backend met an error: the
    /// reference reproduces that error exactly, or finds none.
    pub reference_ticks: u64,
    /// Why the backend stopped serving the world, if it failed.
    pub failure: Option<String>,
}

/// The world's uptake backend and the host's lists for it.
#[derive(Debug, Default)]
struct Uptake {
    backend: Option<Box<dyn UptakeBackend>>,
    report: UptakeReport,
    work: UptakeWork,
}

/// The world's controller backend and its scratch.
#[derive(Debug, Default)]
struct Controllers {
    backend: Option<Box<dyn ControllerBackend>>,
    report: ControllerReport,
    /// One per due evaluation, in identity order.
    outputs: Vec<Option<[f32; ACTION_WIDTH]>>,
    /// One per row: the backend's outputs for a due organism it evaluated.
    by_row: Vec<Option<[f32; ACTION_WIDTH]>>,
}

/// The one organism the world records observations and transactions for
/// (FL-021, plan §14.2) and its newest records. It is not scientific
/// state: no digest, checkpoint or report holds it, and every record in it
/// belongs to `organism`.
#[derive(Debug, Default)]
struct Watch {
    organism: Option<OrganismId>,
    /// Its observation in the newest published step it observed in.
    observed: Option<Sensed>,
    /// Its observation in the newest published step it evaluated in.
    evaluated: Option<Sensed>,
    /// Its transactions in its last [`RECENT_STEPS`] published steps,
    /// oldest first. Each step's preparation reserves the room, so
    /// publishing never allocates.
    recent: VecDeque<Transactions>,
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
    config: ValidatedConfig,
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
    controllers: Controllers,
    fields: Fields,
    sensors: Sensors,
    uptake: Uptake,
    watch: Watch,
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

        let sources = initial_sources(config, &rules)?;
        let mut paths = Vec::new();
        for index in 0..model.sources.count {
            let id = SourceId::new(index);
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
        let scratch = Scratch::new(config, &rules, sources.len())?;
        Ok(Self {
            config: config.clone(),
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
            controllers: Controllers::default(),
            fields: Fields::default(),
            sensors: Sensors::default(),
            uptake: Uptake::default(),
            watch: Watch::default(),
        })
    }

    /// The committed tick.
    #[must_use]
    pub const fn tick(&self) -> Tick {
        self.tick
    }

    /// The validated configuration the world was founded from.
    #[must_use]
    pub const fn config(&self) -> &ValidatedConfig {
        &self.config
    }

    /// The numerical profile the world is checked against.
    #[must_use]
    pub const fn profile(&self) -> NumericalProfile {
        self.rules.profile
    }

    /// The committed resource epoch: zero at founding, one more at every
    /// publication.
    #[must_use]
    pub const fn epoch(&self) -> ResourceEpoch {
        self.epoch
    }

    /// How many threads the field step and phases 3 and 4 use: `[execution]
    /// cpu_workers` unless [`World::set_cpu_workers`] changed it.
    #[must_use]
    pub fn cpu_workers(&self) -> usize {
        self.scratch.lanes.len()
    }

    /// Use `workers` threads for the field step and phases 3 and 4 from the
    /// next tick. No result depends on the count: field planes are
    /// transported and deposited independently, each organism's decision
    /// reads only committed state, and decisions are gathered in identity
    /// order.
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] if the lanes cannot be reserved; the
    /// count is then unchanged.
    pub fn set_cpu_workers(&mut self, workers: NonZeroUsize) -> Result<(), CoreError> {
        self.scratch.field.set_workers(workers)?;
        let workers = workers.get();
        let mut added = Vec::new();
        for _ in self.scratch.lanes.len()..workers {
            added.push(lane(&self.rules)?);
        }
        let lanes = &mut self.scratch.lanes;
        if workers > lanes.len() {
            lanes
                .try_reserve_exact(workers - lanes.len())
                .map_err(|_| CoreError::AllocationFailed {
                    buffer: "decision lanes",
                    bytes: bytes_for::<DecisionLane>(workers),
                })?;
        }
        lanes.extend(added);
        lanes.truncate(workers);
        lanes.shrink_to(workers);
        // A lane's share of the rows follows the count (FL-043).
        let batched = self.controllers.backend.is_some();
        fit_lanes(lanes, capacity_of(&self.rules)?, batched)
    }

    /// Evaluate due controllers in one batch on `backend` from the next
    /// tick, or on the reference alone with `None`. No result depends on
    /// it: a backend returns the reference's outputs bit for bit and defers
    /// what it cannot reproduce, and the reference evaluates those
    /// ([`flife_neural::backend`]). A backend that fails is dropped, and
    /// [`World::controller_report`] says why.
    ///
    /// Installing a backend reserves the batched path's lists for the
    /// population's capacity (FL-043): its outputs, a slot per row and each
    /// lane's observations. If the host refuses them, the backend is not
    /// installed, and the report says why.
    pub fn set_controller_backend(&mut self, backend: Option<Box<dyn ControllerBackend>>) {
        let reserved = if backend.is_some() {
            capacity_of(&self.rules).and_then(|rows| self.reserve_batched(rows))
        } else {
            self.release_batched();
            Ok(())
        };
        let controllers = &mut self.controllers;
        controllers.report.backend = backend.as_ref().map(|backend| backend.describe());
        controllers.report.failure = None;
        if let Err(error) = reserved {
            controllers.report.failure = Some(error.to_string());
            controllers.backend = None;
            self.release_batched();
            return;
        }
        controllers.backend = backend.map(|mut backend| {
            backend.forget();
            backend
        });
    }

    /// Reserve the batched path's lists for `rows` rows: the outputs, a slot
    /// per row, and each lane's observations.
    fn reserve_batched(&mut self, rows: usize) -> Result<(), CoreError> {
        let controllers = &mut self.controllers;
        fit(&mut controllers.outputs, rows, "controller outputs")?;
        fit(&mut controllers.by_row, rows, "controller outputs by row")?;
        fit_lanes(&mut self.scratch.lanes, rows, true)
    }

    /// Release the batched path's lists.
    fn release_batched(&mut self) {
        self.controllers.outputs = Vec::new();
        self.controllers.by_row = Vec::new();
        for lane in &mut self.scratch.lanes {
            lane.observed = Vec::new();
        }
    }

    /// What the controller backend has done so far.
    #[must_use]
    pub const fn controller_report(&self) -> &ControllerReport {
        &self.controllers.report
    }

    /// Run phase 2 (transport and the external sources) on `backend` from
    /// the next tick, or on the reference alone with `None` (FL-031). No
    /// result depends on it: a backend writes the reference's candidate and
    /// report bit for bit ([`FieldBackend`]), and any error it meets hands
    /// the step to the reference. A backend that fails where the reference
    /// does not is dropped, and [`World::field_report`] says why.
    pub fn set_field_backend(&mut self, backend: Option<Box<dyn FieldBackend>>) {
        let fields = &mut self.fields;
        fields.report.backend = backend.as_ref().map(|backend| backend.describe());
        fields.report.failure = None;
        fields.backend = backend;
    }

    /// What the field backend has done so far.
    #[must_use]
    pub const fn field_report(&self) -> &FieldReport {
        &self.fields.report
    }

    /// Sample phase 3's sensor pyramid on `backend` from the next tick, or
    /// on the reference alone with `None` (FL-031). No result depends on it:
    /// a backend returns the reference's raw samples bit for bit
    /// ([`SensorBackend`]), and any error it meets hands the tick's sampling
    /// to the reference. A backend that fails where the reference does not
    /// is dropped, and [`World::sensor_report`] says why.
    ///
    /// Installing a backend reserves its queries and samples for the
    /// population's capacity (FL-043). If the host refuses them, the backend
    /// is not installed, and the report says why.
    pub fn set_sensor_backend(&mut self, backend: Option<Box<dyn SensorBackend>>) {
        let rows = if backend.is_some() {
            capacity_of(&self.rules)
        } else {
            Ok(0)
        };
        let sensors = &mut self.sensors;
        sensors.report.backend = backend.as_ref().map(|backend| backend.describe());
        sensors.report.failure = None;
        let reserved = rows.and_then(|rows| {
            fit(&mut sensors.queries, rows, "sensor queries")?;
            fit(&mut sensors.samples, rows, "sensor samples")
        });
        if let Err(error) = reserved {
            sensors.report.failure = Some(error.to_string());
            sensors.queries = Vec::new();
            sensors.samples = Vec::new();
            sensors.backend = None;
            return;
        }
        sensors.backend = backend;
    }

    /// What the sensor backend has done so far.
    #[must_use]
    pub const fn sensor_report(&self) -> &SensorReport {
        &self.sensors.report
    }

    /// Compute phase 5's demands and grants on `backend` from the next
    /// tick, or on the reference alone with `None` (FL-031). No result
    /// depends on it: a backend returns the reference's densities, requests,
    /// cell demands and quanta bit for bit ([`UptakeBackend`]), the host
    /// refuses a cell whose quanta do not sum to its grant, and any error
    /// hands the tick's phase 5 to the reference. A backend that fails where
    /// the reference does not is dropped, and [`World::uptake_report`] says
    /// why.
    ///
    /// Installing a backend reserves its lists for the population's
    /// capacity on this grid (FL-043). If the host refuses them, the backend
    /// is not installed, and the report says why.
    pub fn set_uptake_backend(&mut self, backend: Option<Box<dyn UptakeBackend>>) {
        let uptake = &mut self.uptake;
        uptake.report.backend = backend.as_ref().map(|backend| backend.describe());
        uptake.report.failure = None;
        uptake.work = UptakeWork::new();
        uptake.backend = None;
        if backend.is_none() {
            return;
        }
        let grid = &self.rules.grid;
        let reserved = capacity_of(&self.rules).and_then(|rows| {
            let cells = usize::try_from(grid.cell_count()).map_err(|_| CoreError::Overflow {
                quantity: "field cells",
            })?;
            uptake.work.reserve(rows, cells)
        });
        if let Err(error) = reserved {
            uptake.report.failure = Some(error.to_string());
            uptake.work = UptakeWork::new();
            return;
        }
        uptake.backend = backend;
    }

    /// What the uptake backend has done so far.
    #[must_use]
    pub const fn uptake_report(&self) -> &UptakeReport {
        &self.uptake.report
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

    /// The run's bounds on heritable traits and parameters.
    #[must_use]
    pub const fn trait_limits(&self) -> &TraitLimits {
        &self.rules.limits
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

    /// What the world holds on the host now (FL-043, plan §19.4): see
    /// [`Footprint`]. Its staging is zero; a step holds that only while it
    /// runs.
    #[must_use]
    pub fn allocated(&self) -> Footprint {
        Footprint {
            state: self.state_bytes(self.population.allocated_bytes()),
            field: self.field_bytes(),
            scratch: self.scratch_bytes(),
            staging: 0,
        }
    }

    /// The most the world holds on the host while its population stays
    /// within its capacity (FL-043, plan §19.4). Its buffers count as they
    /// are reserved: the population's columns, and the tick's per-organism
    /// lists, are reserved for `population.maximum_count` when the world is
    /// founded, its workers set or a backend installed, and filling them
    /// never allocates. The genome arena and each organism's index entries
    /// count at their bounds for a full population of the catalog's members,
    /// and a step's staging at [`Footprint::staging`]'s charge for every
    /// organism.
    #[must_use]
    pub fn footprint(&self) -> Footprint {
        let organisms = self.population.capacity();
        Footprint {
            state: self.state_bytes(self.population.bound_bytes(self.rules.catalog.len())),
            field: self.field_bytes(),
            scratch: self.scratch_bytes(),
            staging: (organisms as u64).saturating_mul(STAGING_PER_ORGANISM),
        }
    }

    /// What installing a controller backend reserves on top of
    /// [`World::footprint`] (FL-043): its outputs and a slot per row for the
    /// population's capacity, and each lane's observations for the most
    /// rows it can take.
    #[must_use]
    pub fn batched_bytes(&self) -> u64 {
        let organisms = self.population.capacity();
        let lanes = self.scratch.lanes.len();
        total([
            bytes_for::<Option<[f32; ACTION_WIDTH]>>(organisms).saturating_mul(2),
            bytes_for::<Observation>(lane_rows(organisms, lanes)).saturating_mul(lanes as u64),
        ])
    }

    fn state_bytes(&self, population: u64) -> u64 {
        total([
            population,
            capacity_bytes(&self.sources),
            capacity_bytes(&self.paths),
        ])
    }

    fn field_bytes(&self) -> u64 {
        total([
            self.field.allocated_bytes(),
            self.scratch.field.allocated_bytes(),
            self.scratch.pyramid.allocated_bytes(),
        ])
    }

    fn scratch_bytes(&self) -> u64 {
        let scratch = &self.scratch;
        let lanes = scratch.lanes.iter().map(|lane| {
            total([
                lane.controller.allocated_bytes(),
                capacity_bytes(&lane.observed),
                capacity_bytes(&lane.decisions),
            ])
        });
        total(
            [
                capacity_bytes(&scratch.rows),
                capacity_bytes(&scratch.lanes),
                capacity_bytes(&scratch.sampled),
                capacity_bytes(&scratch.working),
                capacity_bytes(&scratch.demands),
                scratch.grants.allocated_bytes(),
                scratch.by_demand.allocated_bytes(),
                capacity_bytes(&scratch.packets),
                scratch.emissions.allocated_bytes(),
                scratch.recycling.allocated_bytes(),
                capacity_bytes(&scratch.survivors),
                capacity_bytes(&scratch.deaths),
                capacity_bytes(&self.controllers.outputs),
                capacity_bytes(&self.controllers.by_row),
                capacity_bytes(&self.sensors.queries),
                capacity_bytes(&self.sensors.samples),
                self.uptake.work.allocated_bytes(),
            ]
            .into_iter()
            .chain(lanes),
        )
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

    /// Record `organism`'s observation and transactions in every step from
    /// the next one on, or no organism's (FL-021, plan §14.2): what
    /// [`World::organism_snapshot`] reports as [`Sensed`], [`Activations`]
    /// and [`Transactions`]. Watching another organism drops the records of the
    /// one before; watching the same one again keeps them. It changes no
    /// result of a run: no digest, checkpoint or report holds the records,
    /// and a step reads them from nothing.
    pub fn watch(&mut self, organism: Option<OrganismId>) {
        if self.watch.organism != organism {
            self.watch = Watch {
                organism,
                ..Watch::default()
            };
        }
    }

    /// The organism the world records observations for.
    #[must_use]
    pub const fn watched(&self) -> Option<OrganismId> {
        self.watch.organism
    }

    /// The watched organism's transactions in its last [`RECENT_STEPS`]
    /// published steps, oldest first, whether or not it still lives: an
    /// organism that died has the step that exhausted it last. A snapshot
    /// of a live watched organism carries the same records.
    pub fn watched_transactions(
        &self,
    ) -> impl DoubleEndedIterator<Item = &Transactions> + ExactSizeIterator {
        self.watch.recent.iter()
    }

    /// One organism at this committed tick (FL-021, plan §14.2), or `None`
    /// for an identity the population does not hold. It reads the
    /// organism's row and its own genome and changes nothing; see
    /// [`OrganismSnapshot`]. While the world watches it
    /// ([`World::watch`]), the snapshot carries its newest recorded
    /// observation, and the hidden activations of the evaluation its held
    /// output came from when that evaluation's observation was recorded.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for a live organism whose genome or
    /// topology is missing, or whose held output the reference controller
    /// does not reproduce from the inputs recorded for it; as [`decode`]
    /// for its held output and as [`ReferenceController::trace`] for the
    /// activations.
    ///
    /// [`ReferenceController::trace`]: flife_neural::ReferenceController::trace
    pub fn organism_snapshot(&self, id: OrganismId) -> Result<Option<OrganismSnapshot>, CoreError> {
        let Some(organism) = self.population.get(id) else {
            return Ok(None);
        };
        let missing = |what: &str| CoreError::StateConflict {
            operation: "inspect organism",
            detail: format!("{id} has no {what}"),
        };
        let genome = self
            .population
            .genome(id)
            .ok_or_else(|| missing("genome"))?;
        let topology = self
            .rules
            .catalog
            .topology(genome.topology())
            .ok_or_else(|| missing("topology in the catalog"))?;
        let decoded = match organism.held.outputs() {
            Some(outputs) => Some(decode(
                &raw_outputs(&self.rules, outputs),
                self.rules.step,
                &self.rules.decoding,
            )?),
            None => None,
        };
        let watched = self.watch.organism == Some(id);
        let sensed = self.watch.observed.clone().filter(|_| watched);
        let recorded = self.watch.evaluated.as_ref().filter(|_| watched);
        let activations = match (
            recorded,
            organism.held.evaluated_at(),
            organism.held.outputs(),
        ) {
            (Some(recorded), Some(at), Some(held)) if recorded.tick.next().ok() == Some(at) => {
                let trace = genome
                    .controller(&self.rules.catalog, &self.rules.limits)?
                    .trace(
                        &recorded.observation.inputs,
                        &mut ControllerScratch::default(),
                    )?;
                if trace.outputs.map(f32::to_bits) != held.map(f32::to_bits) {
                    return Err(CoreError::StateConflict {
                        operation: "inspect organism",
                        detail: format!(
                            "{id}'s output held from tick {} is not the reference's on the inputs \
                             recorded for it",
                            at.get()
                        ),
                    });
                }
                Some(Activations {
                    tick: at,
                    hidden: trace.hidden,
                })
            }
            _ => None,
        };
        let transactions = if watched {
            self.watch.recent.iter().copied().collect()
        } else {
            Vec::new()
        };
        Ok(Some(OrganismSnapshot {
            tick: self.tick,
            organism,
            age_ticks: self.tick.get().saturating_sub(organism.birth_tick.get()),
            topology: genome.topology().index(),
            hidden_widths: topology.hidden_widths().to_vec(),
            parameters: genome.parameters().len(),
            traits: *genome.traits(),
            decoded,
            sensed,
            activations,
            transactions,
        }))
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
            controllers,
            fields,
            sensors,
            uptake: uptake_state,
            watch,
            ..
        } = self;
        let Scratch {
            field: field_work,
            pyramid,
            rows,
            lanes,
            sampled,
            working,
            demands,
            grants,
            by_demand,
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

        // Phase 1 sealed `interventions` at this boundary. The removals take
        // their organisms out here, before anything senses or acts; the
        // injections apply below, with phase 2's external sources.
        let opening = Holdings::new(field.current().energy_total(), organism_total(population))?;
        let mut tick_ledger = ledger.open(from, opening)?;
        rows.clear();
        rows.extend(population.iter());
        let mut applied = Vec::new();
        let mut removals = Vec::new();
        intervention::remove(
            from,
            interventions,
            &rules.grid,
            population,
            &rules.recycling,
            rows,
            &mut removals,
            recycling,
            &mut tick_ledger,
            &mut applied,
        )?;

        // Phase 2: transport, then the sources sampled at mid-tick, then the
        // injections in their sealed order.
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
        let field_step = advance_field_with(field, rules, sampled, step, field_work, fields)?;
        tick_ledger.record(Flow::SourcePower, field_step.injected_declared)?;
        tick_ledger.record(Flow::Decay, field_step.transport.decay_removed)?;
        {
            let (_, next) = field.stage();
            intervention::apply(from, interventions, next, &mut tick_ledger, &mut applied)?;
        }
        // The report lists every intervention in its sealed position.
        applied.sort_by_key(|intervention| intervention.position);
        let (_, observed) = field.stage();
        let observed: &FieldState = observed;

        // Phases 3 and 4: the pyramid, then every remaining organism's
        // observation and its due or forced evaluation, in identity order.
        let view = pyramid.build(observed)?;
        let (sampled, sensor_failure) = sample_with(population, rows, &view, observed, sensors);
        decide_with(
            rules,
            population,
            &view,
            sampled,
            rows,
            [from, to],
            lanes,
            controllers,
        )?;
        let sensed = match watch.organism {
            Some(id) => watched(rules, population, &view, sampled, rows, id, [from, to])?,
            None => None,
        };
        // The reference sampled the tick without an error, so a backend that
        // met one failed where the reference does not.
        if let Some(failure) = sensor_failure {
            sensors.report.failure = Some(failure);
            sensors.backend = None;
        }
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

        // Phase 5: requests from the observation field, resolved per cell,
        // on the uptake backend if the world has one.
        let uptake = uptake_with(rules, working, observed, demands, grants, uptake_state)?;
        // The packets are in (cell, identity) order, so grouping them by
        // demand keeps each organism's in cell order.
        let granted = grants.packets();
        by_demand.group(demands.len(), granted.len(), |at| {
            granted.get(at).map_or(usize::MAX, |packet| packet.demand)
        })?;

        // Room for the watched organism's transactions, so that publishing
        // them never allocates.
        if watch.organism.is_some() && watch.recent.capacity() < RECENT_STEPS {
            let more = RECENT_STEPS.saturating_sub(watch.recent.len());
            watch
                .recent
                .try_reserve_exact(more)
                .map_err(|_| CoreError::AllocationFailed {
                    buffer: "watched transactions",
                    bytes: u64::try_from(more.saturating_mul(size_of::<Transactions>()))
                        .unwrap_or(u64::MAX),
                })?;
        }
        let mut transactions = None;

        // Phases 6 and 7: each organism's ordered budget stages.
        for (index, organism) in working.iter_mut().enumerate() {
            packets.clear();
            for packet in by_demand
                .bucket(index)
                .iter()
                .filter_map(|&at| granted.get(at as usize))
            {
                packets.push(ChemicalPacket {
                    granted: packet.granted,
                    composition: composition(observed, packet.cell)?,
                });
            }
            let genome = genome_of(population, organism.committed.id)?;
            let parameters = parameter_count(&rules.catalog, genome)?;
            let record =
                (watch.organism == Some(organism.committed.id)).then_some(&mut transactions);
            settle_stages(
                rules,
                organism,
                genome.traits(),
                parameters,
                packets,
                from,
                &mut tick_ledger,
                emissions,
                record,
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
                let cleared = clear_tiny(&rules.tiny, organism, &mut tick_ledger)?;
                if let Some(record) = transactions
                    .as_mut()
                    .filter(|_| watch.organism == Some(organism.committed.id))
                {
                    record.tiny = cleared;
                }
                live += u64::from(organism.vitality.is_alive());
            }
        }
        let mut requests = Vec::new();
        reserve_for(
            &mut requests,
            working
                .iter()
                .filter(|organism| organism.intent.is_some())
                .count(),
            "birth requests",
        )?;
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
            offspring: match rules.variant {
                ModelVariant::FreshOffspring => Offspring::Fresh {
                    catalog: &rules.catalog,
                    limits: &rules.limits,
                    initial_scale: rules.initial_scale,
                },
                ModelVariant::Baseline | ModelVariant::ChemistryOnly => Offspring::Inherited,
            },
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
        if let (Some(record), Some(intent)) = (
            transactions.as_mut(),
            watch
                .organism
                .and_then(|id| find(working, id))
                .and_then(|organism| organism.intent),
        ) {
            record.reproduction = Some(attempted(&births, &intent)?);
        }
        for organism in working
            .iter_mut()
            .filter(|organism| organism.intent.is_some())
        {
            organism
                .vitality
                .check(BudgetStage::Split, &organism.budget)?;
            let cleared = clear_tiny(&rules.tiny, organism, &mut tick_ledger)?;
            if let Some(record) = transactions
                .as_mut()
                .filter(|_| watch.organism == Some(organism.committed.id))
            {
                record.tiny = cleared;
            }
        }
        if let Some(record) = transactions.as_mut() {
            record.closing = watch
                .organism
                .and_then(|id| find(working, id))
                .map_or(0.0, |organism| organism.budget.remaining());
        }

        // Every fate, in identity order; the deaths' residuals are settled.
        // The deaths are listed as they happened: phase 1's removals first.
        survivors.clear();
        deaths.clear();
        deaths.append(&mut removals);
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
        // Room for the births and every introduction the population's
        // bound admits, so that appending introductions never grows it
        // (FL-043).
        let mut entrants = Vec::new();
        reserve_for(
            &mut entrants,
            capacity_of(rules)?
                .saturating_sub(survivors.len())
                .max(born.len()),
            "entrants",
        )?;
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
        // The introductions enter after the births, in sealed order, each
        // all or none.
        let first_introduced = entrants.len();
        intervention::introduce(
            from,
            interventions,
            &intervention::Founding {
                rng: &rules.rng,
                grid: &rules.grid,
                catalog: &rules.catalog,
                limits: &rules.limits,
                initial_scale: rules.initial_scale,
                interval: rules.interval,
                cooldown: rules.reproduction.rules().cooldown,
                maximum_population: rules.maximum_population,
            },
            survivors.len(),
            &mut next_identities,
            &mut next_quota,
            &mut entrants,
            &mut tick_ledger,
            &mut applied,
        )?;
        let introduced = entrants.get(first_introduced..).unwrap_or(&[]);
        let mut introductions = Vec::with_capacity(introduced.len());
        for entrant in introduced {
            organisms.add(entrant.organism.energy);
            introductions.push(IntroductionEvent {
                organism: entrant.organism.id,
                genome: entrant.organism.genome,
                topology: entrant.genome.topology(),
            });
        }
        applied.sort_by_key(|intervention| intervention.position);

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
            introductions,
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
            sensed,
            transactions,
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
    let [u, v] = founder_draws(rng, organism)?;
    grid.wrap_position([u * grid.world_width(), v * grid.world_height()])
}

/// The two half-open uniforms behind a founder's position: words 0 and 1 of
/// `(founders, organism, 0, lane (0, position, 0), 0)`. An introduced
/// organism scales them into its cell.
///
/// # Errors
/// As [`Lane::new`], which does not fail for this lane.
pub(crate) fn founder_draws(
    rng: &SemanticRng,
    organism: OrganismId,
) -> Result<[f64; 2], CoreError> {
    let [first, second, ..] = rng.block(Address {
        stream: Stream::Founders,
        entity: organism.get(),
        ordinal: 0,
        lane: Lane::new(0, LaneKind::Position, 0)?,
        draw: 0,
    });
    Ok([half_open_unit(first), half_open_unit(second)])
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
/// The configuration's sources at their seeded centers: fixed for the run.
fn initial_sources(config: &ValidatedConfig, rules: &Rules) -> Result<Vec<Source>, CoreError> {
    let model = config.config();
    (0..model.sources.count)
        .map(|index| {
            initial_source(
                &rules.rng,
                &rules.grid,
                SourceId::new(index),
                model.sources.power_per_source,
            )
        })
        .collect()
}

impl Scratch {
    /// Empty scratch for `sources` sources at the configuration's worker
    /// count.
    fn new(config: &ValidatedConfig, rules: &Rules, sources: usize) -> Result<Self, CoreError> {
        let limits = BufferLimits::host();
        let workers_configured = config.config().execution.cpu_workers;
        // Contribution scratch for the largest live population (§9.4): every
        // organism demands, emits and dies at most once per tick, and each
        // touches at most four cells.
        let organisms = rules.maximum_population;
        let packets = organisms.checked_mul(4).ok_or(CoreError::Overflow {
            quantity: "organism packets",
        })?;
        // Every per-organism list is reserved for the largest population too
        // (FL-043), so the footprint taken now bounds every later tick.
        let rows = capacity_of(rules)?;
        let mut field = FieldWork::new(&rules.grid, &limits, workers(workers_configured)?)?;
        field.reserve_supports(sources, &rules.kernel)?;
        Ok(Self {
            field,
            pyramid: SensorPyramid::from_config(config, &limits)?,
            rows: reserved(rows, "organism rows")?,
            lanes: lanes(workers(workers_configured)?.get(), rules)?,
            sampled: Vec::with_capacity(sources),
            working: reserved(rows, "working organisms")?,
            demands: reserved(rows, "organism demands")?,
            grants: Grants::with_capacity(&rules.grid, organisms, &limits)?,
            by_demand: Bins::with_capacity("organism packets", organisms, packets, &limits)?,
            packets: reserved(4, "organism packets")?,
            emissions: Deposits::with_capacity(&rules.grid, organisms, &limits)?,
            recycling: Deposits::with_capacity(&rules.grid, organisms, &limits)?,
            survivors: reserved(rows, "survivors")?,
            deaths: reserved(rows, "deaths")?,
        })
    }
}

fn organism_total(population: &Population) -> f64 {
    let mut total = CompensatedSum::ZERO;
    for organism in population.iter() {
        total.add(organism.energy);
    }
    total.value()
}

/// `[execution] cpu_workers` as a thread count.
fn workers(workers: u32) -> Result<NonZeroUsize, CoreError> {
    usize::try_from(workers)
        .ok()
        .and_then(NonZeroUsize::new)
        .ok_or_else(|| CoreError::NumericalInvalid {
            quantity: "execution.cpu_workers",
            detail: "not a positive thread count".to_owned(),
        })
}

/// `workers` decision lanes, each with its decisions reserved for the most
/// rows it can take and its controller scratch for the catalog's widest
/// member (FL-043).
fn lanes(workers: usize, rules: &Rules) -> Result<Vec<DecisionLane>, CoreError> {
    let workers = workers.max(1);
    let mut lanes = reserved(workers, "decision lanes")?;
    for _ in 0..workers {
        lanes.push(lane(rules)?);
    }
    fit_lanes(&mut lanes, capacity_of(rules)?, false)?;
    Ok(lanes)
}

/// An empty decision lane with its controller scratch for the catalog's
/// widest member.
fn lane(rules: &Rules) -> Result<DecisionLane, CoreError> {
    let widest = rules
        .catalog
        .ids()
        .filter_map(|id| rules.catalog.topology(id))
        .max_by_key(|topology| topology.hidden_widths().iter().copied().max().unwrap_or(0));
    Ok(DecisionLane {
        controller: match widest {
            Some(topology) => ControllerScratch::new(topology)?,
            None => ControllerScratch::default(),
        },
        observed: Vec::new(),
        decisions: Vec::new(),
    })
}

/// The population's capacity, as a row count.
fn capacity_of(rules: &Rules) -> Result<usize, CoreError> {
    usize::try_from(rules.maximum_population).map_err(|_| CoreError::Overflow {
        quantity: "population.maximum_count",
    })
}

/// The most rows one of `lanes` lanes takes of `organisms` (see
/// [`in_runs`]): an even share, or fewer than [`MINIMUM_ROWS_PER_LANE`]
/// rows on fewer lanes.
fn lane_rows(organisms: usize, lanes: usize) -> usize {
    organisms
        .div_ceil(lanes.max(1))
        .max(MINIMUM_ROWS_PER_LANE)
        .min(organisms)
}

/// Reserve each lane's decisions, and with `batched` its observations, for
/// the most rows it can take of `organisms`; without `batched`, release its
/// observations.
fn fit_lanes(lanes: &mut [DecisionLane], organisms: usize, batched: bool) -> Result<(), CoreError> {
    let rows = lane_rows(organisms, lanes.len());
    for lane in lanes {
        fit(&mut lane.decisions, rows, "decisions")?;
        if batched {
            fit(&mut lane.observed, rows, "observations")?;
        } else {
            lane.observed = Vec::new();
        }
    }
    Ok(())
}

/// An empty list with room for exactly `count` values.
fn reserved<T>(count: usize, buffer: &'static str) -> Result<Vec<T>, CoreError> {
    let mut vector = Vec::new();
    fit(&mut vector, count, buffer)?;
    Ok(vector)
}

/// Empty `vector` and give it room for `count` values, and no more than it
/// needs: filling it to `count` then never allocates.
fn fit<T>(vector: &mut Vec<T>, count: usize, buffer: &'static str) -> Result<(), CoreError> {
    vector.clear();
    if vector.capacity() > count {
        vector.shrink_to(count);
    }
    vector
        .try_reserve_exact(count)
        .map_err(|_| CoreError::AllocationFailed {
            buffer,
            bytes: bytes_for::<T>(count),
        })
}

/// Phase 2 on the world's field backend if it has one. Any error there
/// hands the step to [`advance_field_on`], which reproduces the reference's
/// own error or finds none; with none, the backend failed where the
/// reference does not, and the world stops using it. Either way the step is
/// the reference's.
fn advance_field_with(
    pair: &mut FieldPair,
    rules: &Rules,
    sources: &[Source],
    step: TimeStep,
    work: &mut FieldWork,
    fields: &mut Fields,
) -> Result<FieldStepReport, CoreError> {
    let Fields { backend, report } = fields;
    if let Some(device) = backend.as_deref_mut() {
        match device.advance(pair, &rules.plan, &rules.kernel, sources, step) {
            Ok(stepped) => {
                report.steps = report.steps.saturating_add(1);
                return Ok(stepped);
            }
            Err(error) => {
                report.reference_steps = report.reference_steps.saturating_add(1);
                let stepped =
                    advance_field_on(pair, &rules.plan, &rules.kernel, sources, step, work)?;
                report.failure = Some(error.to_string());
                *backend = None;
                return Ok(stepped);
            }
        }
    }
    advance_field_on(pair, &rules.plan, &rules.kernel, sources, step, work)
}

/// Phase 3's raw samples on the world's sensor backend, if it has one and
/// it succeeds; `None` leaves each organism's sampling to the reference, in
/// its observation. The second value is the backend's error, if it met one:
/// the caller drops the backend once the reference has sampled the tick
/// without an error.
fn sample_with<'a>(
    population: &Population,
    rows: &[Organism],
    view: &PyramidView<'_>,
    field: &FieldState,
    sensors: &'a mut Sensors,
) -> (Option<&'a [RawSample]>, Option<String>) {
    let Sensors {
        backend,
        report,
        queries,
        samples,
    } = sensors;
    let Some(device) = backend.as_deref_mut() else {
        return (None, None);
    };
    match sample_on(device, population, rows, view, field, queries, samples) {
        Ok(()) => {
            report.ticks = report.ticks.saturating_add(1);
            (Some(samples.as_slice()), None)
        }
        Err(error) => {
            report.reference_ticks = report.reference_ticks.saturating_add(1);
            (None, Some(error.to_string()))
        }
    }
}

/// Every row's query, then the device's samples, one per row.
fn sample_on(
    device: &mut dyn SensorBackend,
    population: &Population,
    rows: &[Organism],
    view: &PyramidView<'_>,
    field: &FieldState,
    queries: &mut Vec<SensorQuery>,
    samples: &mut Vec<RawSample>,
) -> Result<(), CoreError> {
    queries.clear();
    reserve_for(queries, rows.len(), "sensor queries")?;
    for organism in rows {
        queries.push(SensorQuery {
            position: organism.position,
            scale: genome_of(population, organism.id)?.traits().sensing_scale(),
        });
    }
    device.sample(view.pyramid(), field, queries, samples)?;
    if samples.len() != rows.len() {
        return Err(CoreError::StateConflict {
            operation: "sensor samples",
            detail: format!("{} samples for {} organisms", samples.len(), rows.len()),
        });
    }
    Ok(())
}

/// Phase 5 on the world's uptake backend if it has one: every organism's
/// demand at its footprint's density, then the grants. Any error there
/// hands the tick's phase 5 to the reference, which reproduces the
/// reference's own error or finds none; with none, the backend failed where
/// the reference does not, and the world stops using it. Either way the
/// demands and grants are the reference's.
fn uptake_with(
    rules: &Rules,
    working: &[Working],
    field: &FieldState,
    demands: &mut Vec<Demand>,
    grants: &mut Grants,
    uptake: &mut Uptake,
) -> Result<UptakeSummary, CoreError> {
    let rule = RequestRule {
        step: rules.step,
        capacity_per_second: rules.uptake_capacity,
        half_saturation_density: rules.half_saturation,
    };
    let requesters = || {
        working
            .iter()
            .map(|organism| (organism.committed.id, organism.position))
    };
    let Uptake {
        backend,
        report,
        work,
    } = uptake;
    if let Some(device) = backend.as_deref_mut() {
        let served = demands_on(device, field, &rule, requesters(), work, demands)
            .and_then(|()| resolve_uptake_on(device, field, demands, work, grants));
        match served {
            Ok(summary) => {
                report.ticks = report.ticks.saturating_add(1);
                return Ok(summary);
            }
            Err(error) => {
                report.reference_ticks = report.reference_ticks.saturating_add(1);
                reference_demands(field, &rule, requesters(), demands)?;
                let summary = resolve_uptake(field, demands, grants)?;
                report.failure = Some(error.to_string());
                *backend = None;
                return Ok(summary);
            }
        }
    }
    reference_demands(field, &rule, requesters(), demands)?;
    resolve_uptake(field, demands, grants)
}

/// Phase 3 for one organism (§8.1): its observation at its sensing scale,
/// from the backend's `raw` sample if there is one.
fn observation(
    rules: &Rules,
    population: &Population,
    view: &PyramidView<'_>,
    organism: &Organism,
    raw: Option<&RawSample>,
    from: Tick,
) -> Result<Observation, CoreError> {
    let raw = blended(population, view, organism, raw)?;
    encode(rules, organism, &raw, from)
}

/// The watched organism's observation in the step from `from` to `to`,
/// exactly as its decision read it: from the backend's sample for its row
/// when the decisions used the backend's samples, else from the reference
/// pyramid. `None` when it is not among `rows`, which are in identity order.
fn watched(
    rules: &Rules,
    population: &Population,
    view: &PyramidView<'_>,
    sampled: Option<&[RawSample]>,
    rows: &[Organism],
    id: OrganismId,
    [from, to]: [Tick; 2],
) -> Result<Option<Sensed>, CoreError> {
    let Some((row, organism)) = rows
        .binary_search_by_key(&id, |organism| organism.id)
        .ok()
        .and_then(|row| rows.get(row).map(|organism| (row, organism)))
    else {
        return Ok(None);
    };
    let raw = blended(
        population,
        view,
        organism,
        sampled.and_then(|samples| samples.get(row)),
    )?;
    Ok(Some(Sensed {
        tick: from,
        scale: genome_of(population, id)?.traits().sensing_scale(),
        observation: encode(rules, organism, &raw, from)?,
        raw,
        evaluated: due(rules, organism, to),
    }))
}

/// The organism's raw sample at its sensing scale: the backend's `raw` if
/// there is one, else the reference pyramid's.
fn blended(
    population: &Population,
    view: &PyramidView<'_>,
    organism: &Organism,
    raw: Option<&RawSample>,
) -> Result<RawSample, CoreError> {
    let genome = genome_of(population, organism.id)?;
    match raw {
        Some(raw) => Ok(*raw),
        None => view.sample(organism.position, genome.traits().sensing_scale()),
    }
}

/// The observation encoded from the organism's raw sample and its own
/// committed state at `from`.
fn encode(
    rules: &Rules,
    organism: &Organism,
    raw: &RawSample,
    from: Tick,
) -> Result<Observation, CoreError> {
    let intrinsic = Intrinsic {
        energy: organism.energy,
        age_seconds: age_seconds(organism.birth_tick, from, rules.step)?,
        velocity: organism.velocity,
        energy_change_per_second: organism.energy_change_rate,
    };
    observe(raw, &intrinsic, &rules.scales)
}

/// Whether the organism evaluates its controller for tick `to`: due on its
/// phase, or forced because it holds no valid output.
fn due(rules: &Rules, organism: &Organism, to: Tick) -> bool {
    !organism.held.is_valid() || to.get() % u64::from(rules.interval) == u64::from(organism.phase)
}

/// The raw outputs a held output decodes from: the controller's, widened,
/// or none under the chemistry-only variant.
fn raw_outputs(rules: &Rules, outputs: &[f32; ACTION_WIDTH]) -> [f64; ACTION_WIDTH] {
    match rules.variant {
        ModelVariant::ChemistryOnly => [0.0; ACTION_WIDTH],
        ModelVariant::Baseline | ModelVariant::FreshOffspring => widen(outputs),
    }
}

/// The rest of phase 4: the held output, newly evaluated or not, decoded.
fn finish(
    rules: &Rules,
    organism: &Organism,
    held: HeldOutput,
    evaluated: bool,
    observation: &Observation,
) -> Result<Decision, CoreError> {
    let outputs = held.outputs().ok_or_else(|| CoreError::StateConflict {
        operation: "decode held output",
        detail: format!("{} holds no valid output", organism.id),
    })?;
    Ok(Decision {
        held,
        evaluated,
        actions: decode(&raw_outputs(rules, outputs), rules.step, &rules.decoding)?,
        gradients: observation.gradients,
    })
}

/// Phases 3 and 4 for one organism (§8.1): its observation at its sensing
/// scale, then its due or forced evaluation, decoded.
fn decide(
    rules: &Rules,
    population: &Population,
    view: &PyramidView<'_>,
    (organism, raw): (&Organism, Option<&RawSample>),
    [from, to]: [Tick; 2],
    controller: &mut ControllerScratch,
) -> Result<Decision, CoreError> {
    let observation = observation(rules, population, view, organism, raw, from)?;
    let due = due(rules, organism, to);
    let held = if due {
        let outputs = genome_of(population, organism.id)?
            .controller(&rules.catalog, &rules.limits)?
            .evaluate(&observation.inputs, controller)?;
        HeldOutput::evaluated(to, outputs)?
    } else {
        organism.held
    };
    finish(rules, organism, held, due, &observation)
}

/// Run `work` over `rows` on at most `lanes.len()` threads: this one and
/// scoped workers it joins before returning. That is plan §3.4's bounded
/// reference adapter, not Asupersync-scheduled parallelism. The rows are
/// split into contiguous identity-ordered runs, the same split for the same
/// row and lane counts, one per lane; `work` gets the index of its run's
/// first row. The error returned is the first in identity order, as a
/// single thread would report it.
fn in_runs<F>(rows: &[Organism], lanes: &mut [DecisionLane], work: &F) -> Result<(), CoreError>
where
    F: Fn(usize, &[Organism], &mut DecisionLane) -> Result<(), CoreError> + Sync,
{
    let used = lanes
        .len()
        .min(rows.len().div_ceil(MINIMUM_ROWS_PER_LANE))
        .max(1);
    let Some((first, others)) = lanes.split_first_mut() else {
        return Err(CoreError::StateConflict {
            operation: "decide",
            detail: "no decision lane".to_owned(),
        });
    };
    if used == 1 {
        return work(0, rows, first);
    }
    let length = rows.len().div_ceil(used);
    let mut runs = rows.chunks(length);
    let head = runs.next().unwrap_or(&[]);
    std::thread::scope(|scope| {
        let workers: Vec<_> = runs
            .zip(others.iter_mut())
            .enumerate()
            .map(|(index, (run, lane))| scope.spawn(move || work((index + 1) * length, run, lane)))
            .collect();
        let mut result = work(0, head, first);
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

fn reserve_for<T>(
    vector: &mut Vec<T>,
    count: usize,
    buffer: &'static str,
) -> Result<(), CoreError> {
    vector
        .try_reserve(count)
        .map_err(|_| CoreError::AllocationFailed {
            buffer,
            bytes: u64::try_from(count.saturating_mul(size_of::<T>())).unwrap_or(u64::MAX),
        })
}

/// Phases 3 and 4 for every row on the reference, each lane's decisions
/// following its run (see [`in_runs`]). An organism's decision reads only
/// committed state, so the split cannot change any result.
fn decide_all(
    rules: &Rules,
    population: &Population,
    view: &PyramidView<'_>,
    sampled: Option<&[RawSample]>,
    rows: &[Organism],
    ticks: [Tick; 2],
    lanes: &mut [DecisionLane],
) -> Result<(), CoreError> {
    for lane in lanes.iter_mut() {
        lane.decisions.clear();
    }
    in_runs(rows, lanes, &|start, run, lane| {
        reserve_for(&mut lane.decisions, run.len(), "decisions")?;
        for (index, organism) in run.iter().enumerate() {
            let raw = sampled.and_then(|samples| samples.get(start + index));
            let decision = decide(
                rules,
                population,
                view,
                (organism, raw),
                ticks,
                &mut lane.controller,
            )?;
            lane.decisions.push(decision);
        }
        Ok(())
    })
}

/// How the batched path ended.
enum Batched {
    /// Every decision is made: `batched` evaluations came from the backend
    /// and `deferred` from the reference.
    Done { batched: u64, deferred: u64 },
    /// The backend failed.
    Device(CoreError),
}

/// Phases 3 and 4 for every row, with the due evaluations batched on the
/// world's backend if it has one. Any error on the batched path hands the
/// tick to [`decide_all`], which reproduces the reference's own error (or
/// finds none), so the result is always the reference's.
#[expect(
    clippy::too_many_arguments,
    reason = "phases 3 and 4's inputs, the backend's samples and the controllers"
)]
fn decide_with(
    rules: &Rules,
    population: &Population,
    view: &PyramidView<'_>,
    sampled: Option<&[RawSample]>,
    rows: &[Organism],
    ticks: [Tick; 2],
    lanes: &mut [DecisionLane],
    controllers: &mut Controllers,
) -> Result<(), CoreError> {
    let Controllers {
        backend,
        report,
        outputs,
        by_row,
    } = controllers;
    let Some(device) = backend.as_deref_mut() else {
        return decide_all(rules, population, view, sampled, rows, ticks, lanes);
    };
    let decided = decide_batched(
        rules, population, view, sampled, rows, ticks, lanes, device, outputs, by_row,
    );
    report.resident_peak_bytes = report.resident_peak_bytes.max(device.resident_bytes());
    match decided {
        Ok(Batched::Done { batched, deferred }) => {
            report.batched = report.batched.saturating_add(batched);
            report.deferred = report.deferred.saturating_add(deferred);
            Ok(())
        }
        Ok(Batched::Device(error)) => {
            report.failure = Some(error.to_string());
            *backend = None;
            report.reference_ticks = report.reference_ticks.saturating_add(1);
            decide_all(rules, population, view, sampled, rows, ticks, lanes)
        }
        Err(_) => {
            report.reference_ticks = report.reference_ticks.saturating_add(1);
            decide_all(rules, population, view, sampled, rows, ticks, lanes)
        }
    }
}

/// The batched path: every observation on the lanes, the due evaluations
/// in one call to `device`, then each lane's decisions, evaluating what the
/// device deferred on the reference.
#[expect(
    clippy::too_many_arguments,
    reason = "phase 3 and 4's inputs and the backend's two scratch vectors"
)]
fn decide_batched(
    rules: &Rules,
    population: &Population,
    view: &PyramidView<'_>,
    sampled: Option<&[RawSample]>,
    rows: &[Organism],
    [from, to]: [Tick; 2],
    lanes: &mut [DecisionLane],
    device: &mut dyn ControllerBackend,
    outputs: &mut Vec<Option<[f32; ACTION_WIDTH]>>,
    by_row: &mut Vec<Option<[f32; ACTION_WIDTH]>>,
) -> Result<Batched, CoreError> {
    for lane in lanes.iter_mut() {
        lane.observed.clear();
        lane.decisions.clear();
    }
    in_runs(rows, lanes, &|start, run, lane| {
        reserve_for(&mut lane.observed, run.len(), "observations")?;
        for (index, organism) in run.iter().enumerate() {
            let raw = sampled.and_then(|samples| samples.get(start + index));
            let observed = observation(rules, population, view, organism, raw, from)?;
            lane.observed.push(observed);
        }
        Ok(())
    })?;

    let observed = lanes.iter().flat_map(|lane| lane.observed.iter());
    let mut jobs = Vec::new();
    reserve_for(&mut jobs, rows.len(), "controller jobs")?;
    let mut seen = 0;
    for (organism, observation) in rows.iter().zip(observed) {
        seen += 1;
        if due(rules, organism, to) {
            jobs.push(ControllerJob {
                genome: organism.genome,
                slot: population.arena_slot(organism.id).ok_or_else(|| {
                    CoreError::StateConflict {
                        operation: "controller batch",
                        detail: format!("{} holds no arena slot", organism.id),
                    }
                })?,
                controller: genome_of(population, organism.id)?
                    .controller(&rules.catalog, &rules.limits)?,
                inputs: &observation.inputs,
            });
        }
    }
    if seen != rows.len() {
        return Err(CoreError::StateConflict {
            operation: "batch observations",
            detail: format!("{seen} observations for {} organisms", rows.len()),
        });
    }
    outputs.clear();
    if !jobs.is_empty() {
        if let Err(error) = device.evaluate(&jobs, outputs) {
            return Ok(Batched::Device(error));
        }
        if outputs.len() != jobs.len() {
            return Ok(Batched::Device(CoreError::StateConflict {
                operation: "batch controllers",
                detail: format!("{} outputs for {} jobs", outputs.len(), jobs.len()),
            }));
        }
    }
    drop(jobs);

    by_row.clear();
    reserve_for(by_row, rows.len(), "batched outputs")?;
    let mut next = outputs.iter();
    for organism in rows {
        let output = if due(rules, organism, to) {
            next.next().copied().flatten()
        } else {
            None
        };
        by_row.push(output);
    }
    let by_row: &[Option<[f32; ACTION_WIDTH]>] = by_row;
    in_runs(rows, lanes, &|start, run, lane| {
        let DecisionLane {
            controller,
            observed,
            decisions,
        } = lane;
        reserve_for(decisions, run.len(), "decisions")?;
        for (index, (organism, observation)) in run.iter().zip(observed.iter()).enumerate() {
            let evaluated = due(rules, organism, to);
            let held = if evaluated {
                let outputs = match by_row.get(start + index).copied().flatten() {
                    Some(outputs) => outputs,
                    None => genome_of(population, organism.id)?
                        .controller(&rules.catalog, &rules.limits)?
                        .evaluate(&observation.inputs, controller)?,
                };
                HeldOutput::evaluated(to, outputs)?
            } else {
                organism.held
            };
            decisions.push(finish(rules, organism, held, evaluated, observation)?);
        }
        Ok(())
    })?;
    let deferred = outputs.iter().filter(|output| output.is_none()).count();
    let count = |value: usize| u64::try_from(value).unwrap_or(u64::MAX);
    Ok(Batched::Done {
        batched: count(outputs.len() - deferred),
        deferred: count(deferred),
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
fn find(working: &[Working], id: OrganismId) -> Option<&Working> {
    working
        .binary_search_by_key(&id, |organism| organism.committed.id)
        .ok()
        .and_then(|index| working.get(index))
}

/// The tiny-reservoir stage, the last of §8.2. Returns what it took.
fn clear_tiny(
    tiny: &TinyEnergyPolicy,
    organism: &mut Working,
    ledger: &mut TickLedger,
) -> Result<f64, CoreError> {
    let cleared = tiny.clear(&mut organism.budget)?;
    if cleared > 0.0 {
        ledger.record(Flow::TinyReservoir, cleared)?;
    }
    organism
        .vitality
        .check(BudgetStage::TinyReservoir, &organism.budget)?;
    Ok(cleared)
}

/// What a viable attempt by the watched organism came to in phase 8.
///
/// # Errors
/// [`CoreError::StateConflict`] if its birth was neither staged nor
/// rejected.
fn attempted(births: &Births, intent: &BirthIntent) -> Result<ReproductionOutcome, CoreError> {
    if let Some(birth) = births
        .born()
        .iter()
        .find(|birth| birth.parent == intent.parent)
    {
        return Ok(ReproductionOutcome::Born {
            child: birth.newborn.id,
            split: birth.split,
        });
    }
    births
        .rejected()
        .iter()
        .find(|rejected| rejected.parent == intent.parent)
        .map(|rejected| ReproductionOutcome::Rejected {
            split: intent.split,
            candidates: rejected.candidates.len(),
        })
        .ok_or_else(|| CoreError::StateConflict {
            operation: "record transactions",
            detail: format!("{}'s birth was neither staged nor rejected", intent.parent),
        })
}

/// Phases 6 and 7 for one organism: chemistry, maintenance, motion,
/// emission and the reproduction proposal, rechecking exhaustion after
/// each. Once a stage exhausts the organism nothing later pays or acts.
/// For the watched organism, `record` is filled with each stage's
/// settlement as the ledger is given it.
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
    record: Option<&mut Option<Transactions>>,
) -> Result<(), CoreError> {
    let affinity = Affinity::new(*traits.affinity(), &rules.chemistry)?;
    let opening = organism.budget.remaining();
    let uptake = rules
        .chemistry
        .settle(&affinity, packets, &mut organism.budget)?;
    uptake.record(ledger)?;
    organism.absorbed = uptake.absorbed;
    let mut record = record.map(|slot| {
        slot.insert(Transactions {
            tick: from,
            opening,
            uptake,
            maintenance: None,
            motion: None,
            emission: None,
            reproduction: None,
            tiny: 0.0,
            closing: 0.0,
        })
    });
    if !organism
        .vitality
        .check(BudgetStage::Chemistry, &organism.budget)?
    {
        return Ok(());
    }
    let maintenance = rules
        .costs
        .bill(
            rules.step,
            traits.sensing_scale(),
            parameters,
            organism.evaluated,
        )?
        .settle(&mut organism.budget)?;
    maintenance.record(ledger)?;
    if let Some(record) = record.as_deref_mut() {
        record.maintenance = Some(maintenance);
    }
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
    if let Some(record) = record.as_deref_mut() {
        record.motion = Some(moved);
    }
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
    if let Some(record) = record.as_deref_mut() {
        record.emission = Some(emission);
    }
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
    let proposal = rules.reproduction.propose(
        &rules.rng,
        &parent,
        from,
        &organism.actions,
        &organism.budget,
    )?;
    if let Some(record) = record {
        // A viable attempt's outcome is phase 8's.
        record.reproduction = match proposal {
            ReproductionProposal::CoolingDown { deadline } => {
                Some(ReproductionOutcome::CoolingDown { deadline })
            }
            ReproductionProposal::NoAttempt => Some(ReproductionOutcome::NoAttempt),
            ReproductionProposal::Unviable(why) => Some(ReproductionOutcome::Unviable(why)),
            ReproductionProposal::Exhausted | ReproductionProposal::Intent(_) => None,
        };
    }
    if let ReproductionProposal::Intent(intent) = proposal {
        organism.intent = Some(intent);
    }
    Ok(())
}
