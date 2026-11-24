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
use flife_core::ledger::{CompensatedSum, Ledger, LedgerLog, sub_down};
use flife_core::memory::{bytes_for, capacity_bytes, total};
use flife_core::model::{
    FIELD_ECOLOGY_V1, FIELD_ECOLOGY_V1_CHEMISTRY_ONLY, FIELD_ECOLOGY_V1_CONTACT,
    FIELD_ECOLOGY_V1_CONTACT_CHEMISTRY_ONLY, FIELD_ECOLOGY_V1_CONTACT_FRESH_OFFSPRING,
    FIELD_ECOLOGY_V1_CONTACT_INHIBITION, FIELD_ECOLOGY_V1_CONTACT_INHIBITION_CHEMISTRY_ONLY,
    FIELD_ECOLOGY_V1_CONTACT_INHIBITION_FRESH_OFFSPRING, FIELD_ECOLOGY_V1_CONTACT_SORTING,
    FIELD_ECOLOGY_V1_CONTACT_SORTING_CHEMISTRY_ONLY,
    FIELD_ECOLOGY_V1_CONTACT_SORTING_FRESH_OFFSPRING, FIELD_ECOLOGY_V1_FRESH_OFFSPRING,
    FIELD_ECOLOGY_V1_UNGATED_MOTION, FIELD_ECOLOGY_V2, FIELD_ECOLOGY_V2_CHEMISTRY_ONLY,
    FIELD_ECOLOGY_V2_FIXED_CONTROLLER, FIELD_ECOLOGY_V2_FRESH_OFFSPRING,
};
use flife_core::numerics::sin_cos_turns;
use flife_core::rng::{Address, Lane, LaneKind, SemanticRng, Stream, half_open_unit};
use flife_core::{
    ConfigIssue, CoreError, EnergyBudget, EnergyLedger, Flow, Holdings, LedgerReport,
    ResidualBudget, Sha256Digest, TickClosure, TickLedger, Transfer, ValidatedConfig,
};
use flife_field::settle::MotorSteeringInputs;
use flife_field::{
    Bins, BufferLimits, CARRIER0_MOMENT_CHANNELS, CARRIER1_FIRST_MOMENT_PLANE,
    CARRIER1_MOMENT_CHANNELS, ConeContractionSummary, ConeTolerance, Demand, DepositSummary,
    Deposits, DualRawSample, ExchangeKernel, ExchangeScratch, FieldBackend, FieldGrid, FieldPair,
    FieldState, FieldStepOptions, FieldStepPhase2, FieldStepReport, FieldSummary, FieldWork,
    FluxEdges, GrantPacket, Grants, MAX_PLANES, PyramidView, RawSample, RequestRule,
    SEEDED_SMOOTH_TORUS_V1, SEEDED_UNIT_VECTORS, SeededPath, SensorBackend, SensorPyramid,
    SensorQuery, SettleBackend, Source, SourceKernel, TransportPlan, UptakeBackend, UptakeSummary,
    UptakeWork, advance_field_on, advance_field_v2_on, advance_field_wave2_on,
    apply_deposit_streams_to_carrier_on, apply_deposits_to_carrier_on, apply_grants_on,
    count_cell_occupancy, demand_with_scratch, demands_on, initial_source,
    reduce_dual_carrier_field_on, reduce_field_on, resolve_uptake_on,
    resolve_uptake_with_on_workers, state::CHANNELS,
};
use flife_genome::{
    AFFINITY_TERMS, CADHERIN_TERMS, CadherinTraits, CandidateRejection, Genome, GenomeQuota,
    MAXIMUM_CANDIDATES, MutationParameters, MutationRecord, Mutator, ParameterBuffer,
    TopologyCatalog, TopologyId, TraitLimits, Traits, founder_topology, founder_traits_v2,
    grow_founder, initial_scale,
};
use flife_neural::actions::EMISSION_SPEND;
use flife_neural::{
    ActionDecoding, ArenaSlot, ControllerBackend, ControllerJob, ControllerScratch, DecodedActions,
    DualObservation, FlatControllerJob, FlatOutputs, Intrinsic, MAX_ACTION_WIDTH, MAX_SENSOR_WIDTH,
    MAXIMUM_RECURRENT_STATE_WIDTH, Observation, RawOutputs, ReferenceController, SENSOR_WIDTH,
    SensorLayout, SensorScales, decode_with_layout, observe_dual_for_controller,
    observe_dual_for_controller_f32, observe_for_controller, observe_for_controller_f32,
    update_recurrent_state, widen_slice,
};

use crate::ancestry::{Ancestry, AncestryHeld, Followed};
use crate::chemistry::{
    Affinity, ChemicalPacket, Chemistry, UptakeSettlement, composition, corpse_affinity_direction,
    effective_affinity, symplectic_waste_direction,
};
use crate::contact::{
    ContactRules, ContactV2Buffers, ContactV2Sweep, ContactWork, CrowdingWork, PcpSweepParams,
    TROPHALLAXIS_QUANTUM, TrophallaxisSweepParams, VelocityAlignmentSweepParams,
};
use crate::costs::{MaintenanceCosts, MaintenanceSettlement};
use crate::emission::{
    CachedFootprint, EmissionContext, EmissionPolicy, EmissionSettlement,
    INITIAL_SECRETORY_READINESS,
};
use crate::inspect::{
    Activations, ControllerWeights, ExchangeCell, LocalChemistry, OrganismSnapshot, RECENT_STEPS,
    ReproductionOutcome, Sensed, Transactions,
};
use crate::intervention::{self, AppliedIntervention, Intervention};
use crate::lifecycle::{BudgetStage, Death, Fate, Recycling, TinyEnergyPolicy, Vitality};
use crate::measure::{RegionMeasurement, measure};
use crate::motion::{Motion, MotionResponse, MotionSettlement, RESPONSE_TERMS, uptake_ratio};
use crate::population::{
    Arrangement, CheckedTick, DEFAULT_POLARITY, Entrant, HeldOutput, Organism, Population,
};
use crate::reproduction::{
    Birth, BirthContext, BirthIntent, BirthRequest, BirthResolution, Births, CapacityPolicy,
    CapacityShortfall, Identities, Offspring, ParentState, PopulationQuota, RefusedBirth,
    RejectedBirth, Reproduction, ReproductionProposal, hippo_attempt_probability,
    hippo_crowding_sensor, resolve_births_with_wave_preparer,
};

mod admission;
mod checkpoint;
mod crop;
mod graft;
mod settle;
mod v2;

pub use checkpoint::{
    WORLD_CHECKPOINT_CHUNKS, WORLD_CHUNK_VERSION, WORLD_RECURRENT_HELD_VERSION,
    WORLD_V2_CHUNK_VERSION,
};
pub use graft::Graft;
use settle::{Settle, settle_with};
pub use v2::{
    BuddingRules, CadherinRules, CarrierSpec, CorpseDirection, CrowdingMode, CrowdingRules,
    DifferentiationRules, LysisRules, NotchDeltaMode, NotchDeltaRules, PolarityRules,
    SenescenceRules, StateInheritanceMode, TrophallaxisRules, V2Rules, VelocityAlignmentRules,
    WasteRules,
};

/// The backend this module implements: the scalar CPU reference.
pub const CPU_REFERENCE: &str = "cpu-reference";

/// The determinism class this module implements: one fixed order for every
/// reduction, no worker-order or clock dependence (§16.1).
pub const QUALIFIED_REFERENCE: &str = "qualified-reference";

/// The numerical fault policy this module implements (§7.6).
pub const STOP_AND_PRESERVE: &str = "stop-and-preserve";

/// The research-mode capacity policy (§7.5); `refuse-excess-births-v1`
/// is the exhibition-mode one ([`CapacityPolicy`]).
pub use crate::reproduction::PAUSE_BEFORE_COMMIT;

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

    /// Plan §11.5 bounded moment-only cone-contraction trigger for this
    /// profile: the same absolute floor as `self.cone` and one-tenth its
    /// relative bound (`(f32::MIN_POSITIVE, 10⁻⁶)` for
    /// [`NumericalProfile::reference_f32`]).
    ///
    /// `self.cone` (`INV-02`, `10⁻⁵`) stays unchanged as the hard Phase 10
    /// failure gate; contraction runs only after `INV-01` and `INV-02` have
    /// already passed and only on cells strictly above this trigger.
    #[must_use]
    pub fn cone_contraction_trigger(&self) -> ConeTolerance {
        let relative = if self.cone.relative() == 1.0e-5 {
            1.0e-6
        } else {
            self.cone.relative() * 0.1
        };
        ConeTolerance::new(self.cone.absolute(), relative).unwrap_or(self.cone)
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
    /// Attempts `refuse-excess-births-v1` refused for capacity, in
    /// identity order; always empty under `pause-before-commit`.
    pub refused: Vec<RefusedBirth>,
    /// The deaths, in identity order.
    pub deaths: Vec<Death>,
    /// The uptake resolution.
    pub uptake: UptakeSummary,
    /// Phase 2's field terms.
    pub field_step: FieldStepReport,
    /// The next field's invariant check (unmasked pre-contraction residual).
    pub field: FieldSummary,
    /// Bounded non-energy-creating moment contraction applied after `INV-01`
    /// and `INV-02` passed (plan §11.5); zero on ticks where no cell exceeded
    /// the contraction trigger.
    pub cone_contraction: ConeContractionSummary,
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
    checked: CheckedTick,
    births: Vec<Entrant>,
    ledger: EnergyLedger,
    quota: GenomeQuota,
    identities: Identities,
    paths: Vec<SeededPath>,
    /// The watched organism's observation in this step, if it observed.
    sensed: Option<Sensed>,
    /// Its pre-evaluation recurrent internal state on this step, if any.
    sensed_state: Option<[f32; MAXIMUM_RECURRENT_STATE_WIDTH]>,
    /// Its optional `V2-15` dual-carrier trail sensor channels on this step.
    sensed_trail: Option<[f64; 4]>,
    /// Its optional Hippo-YAP crowding sensor `c_den` on this step, if active.
    sensed_crowding: Option<f64>,
    /// Its optional Notch-Delta neighbor mean `h̄_0` on this step, if active.
    sensed_notch_delta: Option<f32>,
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
    /// As [`Population::commit_tick`], which the candidate already passed;
    /// [`CoreError::StateConflict`] when the world traces its ancestry and
    /// the report names an organism its index does not hold as it must
    /// (FL-021). On error nothing is published and the committed world is
    /// unchanged.
    pub fn publish(self) -> Result<TickReport, CoreError> {
        let Self { world, staged } = self;
        let mut staged = *staged;
        if let Some(ancestry) = &world.ancestry {
            ancestry.admits(&staged.report)?;
        }
        let mut released_slots = Vec::new();
        let released = world
            .controllers
            .backend
            .is_some()
            .then_some(&mut released_slots);
        world.population.apply_checked_tick(
            staged.checked,
            &world.scratch.survivors,
            &mut staged.births,
            &mut staged.quota,
            released,
        )?;
        if !released_slots.is_empty()
            && let Some(backend) = world.controllers.backend.as_mut()
        {
            backend.release_slots(&released_slots);
        }
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
                world.watch.evaluated_state = staged.sensed_state;
                world.watch.evaluated_trail = staged.sensed_trail;
                world.watch.evaluated_crowding = staged.sensed_crowding;
                world.watch.evaluated_notch_delta = staged.sensed_notch_delta;
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
        if let Some(ancestry) = &mut world.ancestry {
            ancestry.fold(&staged.report);
        }
        Ok(staged.report)
    }
}

/// The run-constant operators a configuration declares.
#[derive(Debug)]
struct Rules {
    step: TimeStep,
    grid: FieldGrid,
    exchange: ExchangeKernel,
    max_exchange_cells: usize,
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
    recurrent_state_width: usize,
    limits: TraitLimits,
    mutation: MutationParameters,
    uptake_capacity: f64,
    half_saturation: f64,
    interval: u32,
    maximum_population: u64,
    /// What a birth the fixed logical quotas cannot cover does (§7.5).
    capacity: CapacityPolicy,
    profile: NumericalProfile,
    variant: ModelVariant,
    /// The contact models' bodies (plan §5.4); none under every other.
    contact: Option<ContactRules>,
    /// Modular `field-ecology-v2*` feature rules and sensor/action layouts.
    v2: V2Rules,
    /// Configured `sources.power_per_source`, reused by `V2-02` (`oases`).
    base_source_power: f64,
    initial_scale: f64,
}

/// Which `field-ecology-v1*` or `field-ecology-v2*` model a world runs: the
/// baseline, one of FL-041's ablation models (plan §15.3, §21.6), a motion
/// variant, a contact variant (plan §5.4), or the modular `field-ecology-v2`
/// ecology and its controls (V2-01).
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
    /// `field-ecology-v1-ungated-motion`: §5.4's response gain `g_u` is 1,
    /// not the tick's uptake ratio.
    UngatedMotion,
    /// `field-ecology-v1-contact`: the baseline with soft-disk contact
    /// ([`crate::contact`]).
    Contact,
    /// `field-ecology-v1-contact-chemistry-only`: contact, with actions
    /// decoded from an all-zero raw output.
    ContactChemistryOnly,
    /// `field-ecology-v1-contact-fresh-offspring`: contact, with a newborn's
    /// genome a founder's drawn at its own identity.
    ContactFreshOffspring,
    /// `field-ecology-v1-contact-inhibition`: contact relaxed in sweeps, and
    /// a body's velocity into the bodies it overlaps removed
    /// ([`crate::contact`]).
    Inhibition,
    /// `field-ecology-v1-contact-inhibition-chemistry-only`: contact
    /// inhibition, with actions decoded from an all-zero raw output.
    InhibitionChemistryOnly,
    /// `field-ecology-v1-contact-inhibition-fresh-offspring`: contact
    /// inhibition, with a newborn's genome a founder's drawn at its own
    /// identity.
    InhibitionFreshOffspring,
    /// `field-ecology-v1-contact-sorting`: contact in which unlike chemistry
    /// repels within adhesion's reach ([`crate::contact`]).
    Sorting,
    /// `field-ecology-v1-contact-sorting-chemistry-only`: sorting, with
    /// actions decoded from an all-zero raw output.
    SortingChemistryOnly,
    /// `field-ecology-v1-contact-sorting-fresh-offspring`: sorting, with a
    /// newborn's genome a founder's drawn at its own identity.
    SortingFreshOffspring,
    /// `field-ecology-v2`: modular emergent ecology (V2-01).
    V2,
    /// `field-ecology-v2-chemistry-only`: v2 with actions decoded from an
    /// all-zero raw output while controllers are still evaluated and charged.
    V2ChemistryOnly,
    /// `field-ecology-v2-fresh-offspring`: v2 with each newborn drawing a
    /// fresh founder genome at its own identity.
    V2FreshOffspring,
    /// `field-ecology-v2-fixed-controller`: v2 with controller weights and
    /// topology frozen across inheritance while traits still mutate.
    V2FixedController,
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
            FIELD_ECOLOGY_V1_UNGATED_MOTION => Ok(Self::UngatedMotion),
            FIELD_ECOLOGY_V1_CONTACT => Ok(Self::Contact),
            FIELD_ECOLOGY_V1_CONTACT_CHEMISTRY_ONLY => Ok(Self::ContactChemistryOnly),
            FIELD_ECOLOGY_V1_CONTACT_FRESH_OFFSPRING => Ok(Self::ContactFreshOffspring),
            FIELD_ECOLOGY_V1_CONTACT_INHIBITION => Ok(Self::Inhibition),
            FIELD_ECOLOGY_V1_CONTACT_INHIBITION_CHEMISTRY_ONLY => Ok(Self::InhibitionChemistryOnly),
            FIELD_ECOLOGY_V1_CONTACT_INHIBITION_FRESH_OFFSPRING => {
                Ok(Self::InhibitionFreshOffspring)
            }
            FIELD_ECOLOGY_V1_CONTACT_SORTING => Ok(Self::Sorting),
            FIELD_ECOLOGY_V1_CONTACT_SORTING_CHEMISTRY_ONLY => Ok(Self::SortingChemistryOnly),
            FIELD_ECOLOGY_V1_CONTACT_SORTING_FRESH_OFFSPRING => Ok(Self::SortingFreshOffspring),
            FIELD_ECOLOGY_V2 => Ok(Self::V2),
            FIELD_ECOLOGY_V2_CHEMISTRY_ONLY => Ok(Self::V2ChemistryOnly),
            FIELD_ECOLOGY_V2_FRESH_OFFSPRING => Ok(Self::V2FreshOffspring),
            FIELD_ECOLOGY_V2_FIXED_CONTROLLER => Ok(Self::V2FixedController),
            other => Err(CoreError::UnknownAlgorithm {
                role: "model",
                name: other.to_owned(),
            }),
        }
    }
}

impl ModelVariant {
    /// Whether this variant belongs to the `field-ecology-v2*` family.
    #[must_use]
    pub const fn is_v2(self) -> bool {
        matches!(
            self,
            Self::V2 | Self::V2ChemistryOnly | Self::V2FreshOffspring | Self::V2FixedController
        )
    }

    /// Whether controller weights and topology stay fixed across inheritance
    /// (`field-ecology-v2-fixed-controller`).
    #[must_use]
    pub const fn fixed_controller(self) -> bool {
        matches!(self, Self::V2FixedController)
    }

    /// Whether actions decode from an all-zero raw output (the
    /// chemistry-only controls).
    #[must_use]
    pub const fn decides_from_zero(self) -> bool {
        matches!(
            self,
            Self::ChemistryOnly
                | Self::ContactChemistryOnly
                | Self::InhibitionChemistryOnly
                | Self::SortingChemistryOnly
                | Self::V2ChemistryOnly
        )
    }

    /// Whether a newborn's genome is drawn fresh (the inheritance controls).
    #[must_use]
    pub const fn fresh_offspring(self) -> bool {
        matches!(
            self,
            Self::FreshOffspring
                | Self::ContactFreshOffspring
                | Self::InhibitionFreshOffspring
                | Self::SortingFreshOffspring
                | Self::V2FreshOffspring
        )
    }

    /// Whether survivors are displaced by soft-disk contact.
    #[must_use]
    pub const fn contact(self) -> bool {
        matches!(
            self,
            Self::Contact
                | Self::ContactChemistryOnly
                | Self::ContactFreshOffspring
                | Self::Inhibition
                | Self::InhibitionChemistryOnly
                | Self::InhibitionFreshOffspring
                | Self::Sorting
                | Self::SortingChemistryOnly
                | Self::SortingFreshOffspring
        )
    }

    /// Whether contact also removes a body's velocity into the bodies it
    /// overlaps.
    #[must_use]
    pub const fn inhibits(self) -> bool {
        matches!(
            self,
            Self::Inhibition | Self::InhibitionChemistryOnly | Self::InhibitionFreshOffspring
        )
    }
}

impl Rules {
    /// §5.4's response gain `g_u` for a tick whose uptake absorbed
    /// `absorbed`: the uptake ratio `U / (Δt U_max)`, or exactly 1 under
    /// `field-ecology-v1-ungated-motion`.
    fn response_gain(&self, absorbed: f64) -> Result<f64, CoreError> {
        let ratio = uptake_ratio(absorbed, self.step, self.uptake_capacity)?;
        Ok(if self.variant == ModelVariant::UngatedMotion {
            1.0
        } else {
            ratio
        })
    }

    /// Effective reproduction attempt probability for `organism`, applying
    /// `V2-07` Hippo-YAP local crowding arrest when enabled and returning the
    /// baseline decoded `attempt_probability` bit-identically when disabled.
    pub(super) fn effective_attempt_probability(
        &self,
        organism: &Working,
    ) -> Result<f64, CoreError> {
        match self.v2.crowding() {
            None => Ok(organism.actions.attempt_probability),
            Some(crowding) => hippo_attempt_probability(
                organism.actions.attempt_rate,
                self.step,
                organism.local_crowding,
                crowding.arrest_threshold,
            ),
        }
    }

    /// Build the Phase 7 `V2-04` [`EmissionContext`] from `organism`'s held
    /// controller outputs, post-motion kinematics, and excitable state.
    pub(super) fn emission_context(
        &self,
        organism: &Working,
        position: [f64; 2],
        velocity: [f64; 2],
        from: Tick,
    ) -> Result<EmissionContext, CoreError> {
        let outputs = organism
            .held
            .outputs()
            .ok_or_else(|| CoreError::StateConflict {
                operation: "resolve v2 emission context",
                detail: format!("{} holds no valid output", organism.committed.id),
            })?;
        let raw_spend_logit =
            if self.variant.decides_from_zero() {
                0.0
            } else {
                f64::from(*outputs.get(EMISSION_SPEND).ok_or_else(|| {
                    CoreError::LayoutMismatch {
                        buffer: "held controller output",
                        detail: format!(
                            "output width {} misses emission spend slot {EMISSION_SPEND}",
                            outputs.len()
                        ),
                    }
                })?)
            };
        let raw_pump_logit = self
            .v2
            .actions()
            .wake_pump()
            .map(|slot| {
                if self.variant.decides_from_zero() {
                    Ok(0.0)
                } else {
                    outputs
                        .get(slot)
                        .map(|&value| f64::from(value))
                        .ok_or_else(|| CoreError::LayoutMismatch {
                            buffer: "held controller output",
                            detail: format!(
                                "output width {} misses wake pump slot {slot}",
                                outputs.len()
                            ),
                        })
                }
            })
            .transpose()?;
        Ok(EmissionContext {
            position,
            velocity,
            energy_gradient: organism.gradients[0],
            polarity: self.v2.polarity().map(|_| organism.committed.polarity),
            raw_spend_logit,
            raw_pump_logit,
            tick: from,
            secretory_readiness: organism.secretory_readiness,
            emission_cooldown_deadline: organism.emission_cooldown_deadline,
        })
    }

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
        let grid = FieldGrid::from_config(config)?;
        let exchange = ExchangeKernel::from_config(config)?;
        let max_exchange_cells = exchange.max_cells(&grid);
        let catalog = TopologyCatalog::from_config(config)?;
        let v2 = V2Rules::from_config(config)?;
        let recurrent_state_width = v2.sensors().recurrent_state_width();
        let step = config.report().timestep;
        let plan = match v2.vascular() {
            Some(vascular) => vascular.transport_plan(&grid, step, model.world.decay_per_second)?,
            None => TransportPlan::from_config(config)?,
        };
        Ok(Self {
            step,
            grid,
            exchange,
            max_exchange_cells,
            rng: SemanticRng::from_config(config)?,
            plan,
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
            catalog,
            recurrent_state_width,
            limits: TraitLimits::from_config(config)?,
            mutation: MutationParameters::from_config(config)?,
            uptake_capacity: chemistry.uptake_capacity_per_second,
            half_saturation: chemistry.uptake_half_saturation_density,
            interval: model.controller.evaluation_interval_ticks,
            maximum_population: u64::from(model.population.maximum_count),
            capacity: CapacityPolicy::from_name(&model.population.capacity_policy)?,
            profile,
            variant: ModelVariant::of(&model.model)?,
            contact: contact_rules(config)?,
            v2,
            base_source_power: model.sources.power_per_source,
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
    /// Each grant packet's composition, in packet order (phase 6): room for
    /// four a row, and as long as the most packets a tick has had.
    compositions: Vec<[f64; CHANNELS]>,
    emissions: Deposits,
    /// Carrier-1 trail emissions (`V2-15` `dual-carrier-trails`); `None` (0 bytes)
    /// when disabled.
    trail_emissions: Option<Deposits>,
    /// Obligate symplectic metabolic waste deposits (`V2-11` `symplectic-waste`);
    /// `None` (0 bytes) when disabled. Dedicated stream prevents Trap C
    /// double-deposit conflicts with `emissions`.
    byproducts: Option<Deposits>,
    recycling: Deposits,
    survivors: Vec<Organism>,
    deaths: Vec<Death>,
    /// The contact models' buffers; empty under every other model.
    contact: ContactWork,
    /// Post-Phase-1 crowding buffers (`V2-07`); empty (0 bytes) when disabled.
    crowding: CrowdingWork,
    /// Scratch for `V2-02` (`oases`) modulated source powers; empty (0 bytes)
    /// when disabled.
    oasis_powers: Vec<f64>,
    /// Scratch positions for `V2-09` (`tight-junctions`) cell occupancy; empty
    /// (0 bytes) when disabled.
    barrier_positions: Vec<[f64; 2]>,
    /// Scratch per-cell occupancy for `V2-09` (`tight-junctions`); empty
    /// (0 bytes) when disabled.
    barrier_occupancy: Vec<u32>,
    /// Precomputed symmetric face conductances for `V2-09` (`tight-junctions`)
    /// and/or `V2-14` (`vascular-field`); `None` (0 bytes) when disabled.
    flux_edges: Option<FluxEdges>,
    /// Precomputed Carrier-1 tight-junction face conductances (`V2-15` + `V2-09`);
    /// `None` (0 bytes) when disabled.
    trail_flux_edges: Option<FluxEdges>,
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

/// Queries sampled per spatial pyramid batch on a decision lane. Sized to
/// match [`PyramidView::sample_batch`]'s 256-query 8x8 tile-sorted L1D block.
const SAMPLE_BATCH: usize = 256;

/// One CPU worker's scratch for phases 3 and 4, and 6 and 7.
#[derive(Debug, Default)]
struct DecisionLane {
    controller: ControllerScratch,
    /// Footprint scratch for phase 5 demand and phase 6/7 deposit computation
    /// under fixed-world exchange kernels (FL-053; empty / 0 bytes under
    /// `grid-cic-v1`).
    exchange: ExchangeScratch,
    /// Reusable 256-query batch buffers for spatially-grouped pyramid sampling
    /// (pre-reserved once per lane; unused single/dual buffer stays 0 bytes).
    sample_queries: Vec<SensorQuery>,
    raw_samples: Vec<RawSample>,
    dual_raw_samples: Vec<DualRawSample>,
    /// Legacy batched path's observations of this lane's run.
    observed: Vec<Observation>,
    /// Expanded controller input vectors for legacy batched runs whose layout
    /// exceeds baseline `SENSOR_WIDTH` (recurrent or v2 extra sensors); empty
    /// (0 bytes) on baseline 104-input worlds.
    controller_inputs: Vec<[f64; MAX_SENSOR_WIDTH]>,
    /// Start index and count of this lane's due jobs inside a batched tick.
    due_offset: usize,
    due_count: usize,
    decisions: Vec<Decision>,
    /// Phases 6 and 7 of this lane's run: one organism's packets at a
    /// time, the run's ledger flows and staged emissions in identity order,
    /// which the tick takes in lane order, and the watched organism's
    /// transactions if the run holds it.
    packets: Vec<ChemicalPacket>,
    flows: LedgerLog,
    emissions: Deposits,
    /// Dedicated Carrier-1 trail emission staging (`V2-15`); `None` (0 bytes)
    /// when disabled.
    trail_emissions: Option<Deposits>,
    /// Dedicated symplectic metabolic waste staging (`V2-11`); `None` (0 bytes)
    /// when disabled.
    byproducts: Option<Deposits>,
    /// Compensated sum of exact emission payments `Σ paid` settled by this
    /// lane, so combined dual-carrier `Transfer::Emission` debits the exact
    /// energy deducted from organisms.
    emission_paid: CompensatedSum,
    transactions: Option<Transactions>,
}

/// The most packets one organism's demand is granted: one a footprint
/// cell.
const PACKETS_PER_ORGANISM: usize = 4;

/// The most ledger entries phases 6 and 7 record for one organism: the
/// uptake transfer and toxicity, three maintenance terms and motion.
const FLOWS_PER_ORGANISM: usize = 6;

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
    /// The device's time on the backend's commands, summed over the
    /// batches it returned, in nanoseconds
    /// ([`ControllerBackend::last_device_nanoseconds`], FL-029): zero for
    /// the reference alone and for a backend that runs on the host.
    pub device_nanoseconds: u64,
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
    /// Phase 9 reductions the backend computed (FL-032).
    pub reductions: u64,
    /// Reductions the reference computed because the backend met an error,
    /// with the field, grants and deposits as they were.
    pub reference_reductions: u64,
    /// Phase 10 field checks the backend scanned (FL-032).
    pub checks: u64,
    /// Checks the reference made because the backend met an error, gave a
    /// scan that is not the field's, or failed a cell the reference passes.
    pub reference_checks: u64,
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
    /// 34-plane dual-carrier samples (`V2-15`); empty (`0 B`) on 33-plane runs.
    dual_samples: Vec<DualRawSample>,
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

/// What the world's settle backend did, for run records (FL-032). Not
/// scientific state: no result depends on the backend.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettleReport {
    /// The backend serving the world, or `None` for the reference alone.
    pub backend: Option<String>,
    /// Ticks whose phases 6 and 7 the backend computed.
    pub ticks: u64,
    /// Ticks whose phases 6 and 7, or whose births, the reference computed
    /// because the backend met an error, or gave a settlement the host's
    /// replay refused: the reference reproduces any error exactly, or finds
    /// none.
    pub reference_ticks: u64,
    /// Births whose first candidate's parameters the backend mutated.
    pub mutated: u64,
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
    /// Legacy path: one per due evaluation, in identity order.
    outputs: Vec<Option<RawOutputs>>,
    /// Flat F32 path (`supports_flat_f32`): contiguous row-major F32 inputs.
    flat_inputs_f32: Vec<f32>,
    /// Flat F32 path: per-due-job finiteness flags (`1` = finite, `0` = defer).
    flat_inputs_finite: Vec<u8>,
    /// Flat F32 path: contiguous row-major F32 outputs and validity flags.
    flat_outputs: FlatOutputs,
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
    /// Its pre-evaluation recurrent internal state on the newest published
    /// step it evaluated in (`independent-recurrent-mlp-v1`, FL-054).
    evaluated_state: Option<[f32; MAXIMUM_RECURRENT_STATE_WIDTH]>,
    /// Its `V2-15` dual-carrier trail sensor channels on the newest published
    /// step it evaluated in, when `dual-carrier-trails` is active.
    evaluated_trail: Option<[f64; 4]>,
    /// Its Hippo-YAP crowding sensor `c_den` on the newest published step it
    /// evaluated in (`V2-07`, when `expose_sensor` is active).
    evaluated_crowding: Option<f64>,
    /// Its Notch-Delta neighbor mean `h̄_0` on the newest published step it
    /// evaluated in (`V2-12`, when `notch-delta` is active).
    evaluated_notch_delta: Option<f32>,
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
    /// `V2-03` basal maintenance multiplier (`1.0` when `senescence` is off).
    basal_multiplier: f64,
    /// `V2-07` post-Phase-1 local crowding count `N_loc` (`0` when off).
    local_crowding: u32,
    position: [f64; 2],
    velocity: [f64; 2],
    cooldown_deadline: Tick,
    secretory_readiness: f32,
    emission_cooldown_deadline: Tick,
    emitted: bool,
    /// Residual energy captured by `V2-11` viability-floor lysis in Phase 8
    /// `clear_tiny`, handed to `Recycling` without `Flow::TinyReservoir`.
    lysed_residual: f64,
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
    /// Per-source heterogeneous Gaussian kernels (`V2-02`, `oases`); empty
    /// (0 bytes) on `v1` or when `oases` is disabled.
    source_kernels: Vec<SourceKernel>,
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
    settle: Settle,
    watch: Watch,
    /// The live population's ancestry while the world traces it (FL-021):
    /// not scientific state, like the watch.
    ancestry: Option<Ancestry>,
    /// What the world's restore held beside it (FL-043): the checkpoint
    /// bytes its reader held, and the window its genome stream read through.
    /// Its admission's `restore` phase. Zero for a world founded, cropped or
    /// otherwise built. Not scientific state, like the watch.
    restored: u64,
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
        let workers = workers(config.config().execution.cpu_workers)?;
        Self::found_on(config, arrangement, profile, workers)
    }

    /// [`World::found`] with the founders made on `workers` threads, the
    /// world then using that many as [`World::set_cpu_workers`] leaves it
    /// (FL-034: a hundred thousand founders take seconds on one). No result
    /// depends on the count: identities and genome buffers are issued in
    /// identity order on this thread, every other founder value is a
    /// function of the world seed and the founder's stable identity
    /// ([`flife_genome::founder()`]), the founders are admitted in identity
    /// order, and the error returned is the first in identity order, as a
    /// single thread would report it.
    ///
    /// # Errors
    /// As [`World::found`], and [`World::set_cpu_workers`]'s.
    pub fn found_on(
        config: &ValidatedConfig,
        arrangement: Arrangement,
        profile: NumericalProfile,
        workers: NonZeroUsize,
    ) -> Result<Self, CoreError> {
        let rules = Rules::from_config(config, profile)?;
        let model = config.config();
        let limits = BufferLimits::host();
        let (initial, _) = FieldState::initial(config, &limits)?;
        let mut next = FieldState::zeroed_with_planes(rules.grid, initial.planes(), &limits)?;
        if let Some(vascular) = rules.v2.vascular() {
            next.enable_vascular(vascular.initial_conductance(), &limits)?;
        }
        let field = FieldPair::from_states(initial, next)?;

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
        let founding = Founding {
            scale: initial_scale(config),
            energy: model.population.initial_energy,
            cooldown_deadline: Tick::new(0).checked_add(config.report().cooldown_ticks)?,
        };
        let seeds = seeds(&rules, &mut identities, &mut quota, count)?;
        grow_founders(&rules, &founding, seeds, workers, &mut entrants)?;
        population.admit(&mut entrants)?;

        let sources = initial_sources(config, &rules)?;
        let source_kernels = initial_source_kernels(&rules, &sources)?;
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
        let scratch = Scratch::new(config, &rules, sources.len(), &source_kernels)?;
        let mut world = Self {
            config: config.clone(),
            rules,
            tick: Tick::new(0),
            epoch: ResourceEpoch::new(0),
            events: 0,
            field,
            population,
            sources,
            source_kernels,
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
            settle: Settle::default(),
            watch: Watch::default(),
            ancestry: None,
            restored: 0,
        };
        if world.cpu_workers() != workers.get() {
            world.set_cpu_workers(workers)?;
        }
        Ok(world)
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
        fit_lanes(lanes, &self.rules, capacity_of(&self.rules)?, batched)
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

    /// Reserve the batched path's lists for `rows` rows: legacy outputs, flat
    /// F32 staging/outputs, and each lane's observations.
    fn reserve_batched(&mut self, rows: usize) -> Result<(), CoreError> {
        let input_width = self.rules.v2.sensors().input_width();
        let raw_output_width = self.rules.v2.actions().raw_output_width();
        let flat_in = rows.checked_mul(input_width).ok_or(CoreError::Overflow {
            quantity: "flat controller inputs",
        })?;
        let flat_out = rows
            .checked_mul(raw_output_width)
            .ok_or(CoreError::Overflow {
                quantity: "flat controller outputs",
            })?;
        let controllers = &mut self.controllers;
        fit(&mut controllers.outputs, rows, "controller outputs")?;
        fit(
            &mut controllers.flat_inputs_f32,
            flat_in,
            "flat controller inputs",
        )?;
        fit(
            &mut controllers.flat_inputs_finite,
            rows,
            "flat controller input flags",
        )?;
        fit(
            &mut controllers.flat_outputs.values,
            flat_out,
            "flat controller outputs",
        )?;
        fit(
            &mut controllers.flat_outputs.valid,
            rows,
            "flat controller valid flags",
        )?;
        controllers.flat_outputs.output_width = 0;
        fit_lanes(&mut self.scratch.lanes, &self.rules, rows, true)
    }

    /// Release the batched path's lists.
    fn release_batched(&mut self) {
        self.controllers.outputs = Vec::new();
        self.controllers.flat_inputs_f32 = Vec::new();
        self.controllers.flat_inputs_finite = Vec::new();
        self.controllers.flat_outputs = FlatOutputs::default();
        for lane in &mut self.scratch.lanes {
            lane.observed = Vec::new();
            lane.controller_inputs = Vec::new();
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
        let dual_carrier = self.rules.v2.dual_carrier().is_some();
        let sensors = &mut self.sensors;
        sensors.report.backend = backend.as_ref().map(|backend| backend.describe());
        sensors.report.failure = None;
        let reserved = rows.and_then(|rows| {
            fit(&mut sensors.queries, rows, "sensor queries")?;
            if dual_carrier {
                sensors.samples = Vec::new();
                fit(
                    &mut sensors.dual_samples,
                    rows,
                    "dual-carrier sensor samples",
                )
            } else {
                sensors.dual_samples = Vec::new();
                fit(&mut sensors.samples, rows, "sensor samples")
            }
        });
        if let Err(error) = reserved {
            sensors.report.failure = Some(error.to_string());
            sensors.queries = Vec::new();
            sensors.samples = Vec::new();
            sensors.dual_samples = Vec::new();
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

    /// Compute phase 6's chemistry, maintenance, motion and emission on
    /// `backend` from the next tick, or on the reference alone with `None`
    /// (FL-032). No result depends on it: the host composes the backend's
    /// settlements with the reference's own stage order, exhaustion checks,
    /// ledger flows and reproduction proposal, replays every payment on its
    /// own budgets and refuses a settlement that differs from them by a
    /// bit, and stages each deposit's split through its checks
    /// ([`SettleBackend`]). Any error leaves every organism as it was and
    /// hands the tick's phase 6 to the reference. A backend that fails
    /// where the reference does not is dropped, and
    /// [`World::settle_report`] says why.
    ///
    /// Installing a backend reserves the host's lists for the population's
    /// capacity (FL-043). If the host refuses them, the backend is not
    /// installed, and the report says why.
    pub fn set_settle_backend(&mut self, backend: Option<Box<dyn SettleBackend>>) {
        let settle = &mut self.settle;
        settle.report.backend = backend.as_ref().map(|backend| backend.describe());
        settle.report.failure = None;
        settle.work = settle::SettleWork::default();
        settle.backend = None;
        if backend.is_none() {
            return;
        }
        if let Err(error) = capacity_of(&self.rules)
            .and_then(|rows| settle.work.reserve(rows, self.rules.v2.needs_motion_v2()))
        {
            settle.report.failure = Some(error.to_string());
            settle.work = settle::SettleWork::default();
            return;
        }
        settle.backend = backend;
    }

    /// What the settle backend has done so far.
    #[must_use]
    pub const fn settle_report(&self) -> &SettleReport {
        &self.settle.report
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

    /// The least energy an organism attempts a birth with
    /// (`reproduction.minimum_parent_energy_before_attempt`). Render epochs
    /// carry it as the scale of the lineage-energy colour mapping (FL-019).
    #[must_use]
    pub const fn minimum_attempt_energy(&self) -> f64 {
        self.rules.reproduction.rules().minimum_attempt_energy
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

    /// The run's organism-field exchange kernel (`grid-cic-v1` or
    /// `fixed-world-gaussian-v1`, FL-053).
    #[must_use]
    pub const fn exchange_kernel(&self) -> ExchangeKernel {
        self.rules.exchange
    }

    /// Recurrent internal state width `S` (`0` for `independent-dense-mlp-v1`,
    /// `1..=16` for `independent-recurrent-mlp-v1` / `epigenetic-recurrence`,
    /// FL-054, V2-01).
    #[must_use]
    pub const fn recurrent_state_width(&self) -> usize {
        self.rules.recurrent_state_width
    }

    /// Resolved `field-ecology-v2` rules and sensor/action ABI layouts (V2-01).
    #[must_use]
    pub const fn v2_rules(&self) -> &V2Rules {
        &self.rules.v2
    }

    /// The live population.
    #[must_use]
    pub const fn population(&self) -> &Population {
        &self.population
    }

    /// Whether `V2-08` (`pcp-polarity`) is active on this world.
    #[must_use]
    pub const fn tracks_polarity(&self) -> bool {
        self.rules.v2.polarity().is_some()
    }

    /// Whether `V2-13` (`cadherin-4d`) is active on this world.
    #[must_use]
    pub const fn tracks_cadherin(&self) -> bool {
        self.rules.v2.cadherin().is_some()
    }

    /// One live organism's decoupled 4-D cadherin traits (`V2-13`), or `None`
    /// when `cadherin-4d` is disabled or `id` is not alive (`INV-09` pure
    /// read-only seam for `flife inspect`).
    #[must_use]
    pub fn cadherin_traits(&self, id: OrganismId) -> Option<CadherinTraits> {
        if !self.tracks_cadherin() {
            return None;
        }
        self.population
            .genome(id)
            .and_then(|genome| genome.traits().cadherin_traits())
    }

    /// Live unit apical-basal polarity vectors `p_i ∈ S^1` in ascending
    /// identity order when `V2-08` (`pcp-polarity`) is active; `None` when
    /// disabled (`INV-09` pure read-only seam for `V2-16` / `Epoch::polarities`).
    #[must_use]
    pub fn polarities(&self) -> Option<Vec<[f64; 2]>> {
        self.rules.v2.polarity().map(|_| {
            self.population
                .iter()
                .map(|organism| organism.polarity)
                .collect()
        })
    }

    /// Live primary recurrent cell states `h_0 ∈ [-1, 1]` in ascending
    /// identity order when recurrent state (`epigenetic-recurrence` or
    /// `independent-recurrent-mlp-v1`) is active (`recurrent_state_width > 0`);
    /// `None` for stateless worlds (`INV-09` pure read-only seam for `V2-16` /
    /// `Epoch::cell_states`).
    #[must_use]
    pub fn cell_states(&self) -> Option<Vec<f32>> {
        (self.rules.recurrent_state_width > 0).then(|| {
            self.population
                .iter()
                .map(|organism| {
                    organism
                        .held
                        .recurrent_state()
                        .first()
                        .copied()
                        .unwrap_or(0.0)
                })
                .collect()
        })
    }

    /// Effective metabolic affinity `a_eff(h_0)` of live organism `id` at the
    /// committed tick (`INV-09` pure read-only seam for `LocalChemistry`,
    /// `flife inspect`, and `ColorMapping::Affinity`), or `None` when `id` is
    /// not alive.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] if `id`'s genome is missing; otherwise as
    /// [`effective_affinity`].
    pub fn effective_affinity(&self, id: OrganismId) -> Result<Option<[f64; CHANNELS]>, CoreError> {
        let Some(organism) = self.population.get(id) else {
            return Ok(None);
        };
        let genome = self
            .population
            .genome(id)
            .ok_or_else(|| CoreError::StateConflict {
                operation: "effective affinity",
                detail: format!("{id} has no genome"),
            })?;
        let h0 = organism.held.recurrent_state_array()[0];
        effective_affinity(
            genome.traits(),
            h0,
            &self.rules.v2,
            self.rules.chemistry.affinity_norm_limit(),
        )
        .map(Some)
    }

    /// How many controller topologies the run's catalog defines.
    #[must_use]
    pub fn topology_count(&self) -> usize {
        self.rules.catalog.len()
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

    /// What the world holds on the host now with its genomes' parameters:
    /// [`World::allocated`]'s terms and what its genome quota has allocated
    /// (FL-042, FL-043). A world that never steps, such as a transplant's
    /// restored package, holds no more; it is that world's term in
    /// [`World::admit_memory_beside`].
    #[must_use]
    pub fn held_bytes(&self) -> u64 {
        self.allocated()
            .total()
            .saturating_add(self.quota.allocated_bytes())
    }

    /// What this world's restore ([`World::read_checkpoint`]) held beside
    /// it: the bytes its checkpoint reader held (the whole container, or all
    /// but a streamed genome chunk) and the genome stream's window. Zero for
    /// a world founded, cropped or otherwise built (FL-043).
    #[must_use]
    pub const fn restored_bytes(&self) -> u64 {
        self.restored
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
    /// population's capacity, and each lane's observations (plus expanded
    /// input vectors when the controller layout exceeds `SENSOR_WIDTH`) for
    /// the most rows it can take.
    #[must_use]
    pub fn batched_bytes(&self) -> u64 {
        let organisms = self.population.capacity();
        let lanes = self.scratch.lanes.len();
        let rows = lane_rows(organisms, lanes);
        let input_width = self.rules.v2.sensors().input_width();
        let raw_output_width = self.rules.v2.actions().raw_output_width();
        let expanded = if input_width > SENSOR_WIDTH {
            bytes_for::<[f64; MAX_SENSOR_WIDTH]>(rows).saturating_mul(lanes as u64)
        } else {
            0
        };
        total([
            bytes_for::<Option<RawOutputs>>(organisms),
            bytes_for::<f32>(organisms.saturating_mul(input_width)),
            bytes_for::<u8>(organisms),
            bytes_for::<f32>(organisms.saturating_mul(raw_output_width)),
            bytes_for::<u8>(organisms),
            bytes_for::<Observation>(rows).saturating_mul(lanes as u64),
            expanded,
        ])
    }

    fn state_bytes(&self, population: u64) -> u64 {
        total([
            population,
            capacity_bytes(&self.sources),
            capacity_bytes(&self.source_kernels),
            capacity_bytes(&self.paths),
            // Reserved whole when tracing begins (FL-021).
            self.ancestry.as_ref().map_or(0, Ancestry::bytes),
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
                lane.exchange.allocated_bytes(),
                capacity_bytes(&lane.sample_queries),
                capacity_bytes(&lane.raw_samples),
                capacity_bytes(&lane.dual_raw_samples),
                capacity_bytes(&lane.observed),
                capacity_bytes(&lane.controller_inputs),
                capacity_bytes(&lane.decisions),
                capacity_bytes(&lane.packets),
                lane.flows.allocated_bytes(),
                lane.emissions.allocated_bytes(),
                lane.trail_emissions
                    .as_ref()
                    .map_or(0, Deposits::allocated_bytes),
                lane.byproducts
                    .as_ref()
                    .map_or(0, Deposits::allocated_bytes),
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
                capacity_bytes(&scratch.compositions),
                scratch.emissions.allocated_bytes(),
                scratch
                    .trail_emissions
                    .as_ref()
                    .map_or(0, Deposits::allocated_bytes),
                scratch
                    .byproducts
                    .as_ref()
                    .map_or(0, Deposits::allocated_bytes),
                scratch.recycling.allocated_bytes(),
                capacity_bytes(&scratch.survivors),
                capacity_bytes(&scratch.deaths),
                scratch.contact.allocated_bytes(),
                scratch.crowding.allocated_bytes(),
                capacity_bytes(&scratch.oasis_powers),
                capacity_bytes(&scratch.barrier_positions),
                capacity_bytes(&scratch.barrier_occupancy),
                scratch
                    .flux_edges
                    .as_ref()
                    .map_or(0, FluxEdges::allocated_bytes),
                scratch
                    .trail_flux_edges
                    .as_ref()
                    .map_or(0, FluxEdges::allocated_bytes),
                capacity_bytes(&self.controllers.outputs),
                capacity_bytes(&self.controllers.flat_inputs_f32),
                capacity_bytes(&self.controllers.flat_inputs_finite),
                capacity_bytes(&self.controllers.flat_outputs.values),
                capacity_bytes(&self.controllers.flat_outputs.valid),
                capacity_bytes(&self.sensors.queries),
                capacity_bytes(&self.sensors.samples),
                capacity_bytes(&self.sensors.dual_samples),
                self.uptake.work.allocated_bytes(),
                self.settle.work.allocated_bytes(),
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

    /// Per-source heterogeneous kernels (`V2-02` `oases`), empty when disabled.
    #[must_use]
    pub fn source_kernels(&self) -> &[SourceKernel] {
        &self.source_kernels
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

    /// Trace the live population's ancestry from this committed tick on
    /// (FL-021, plan §14.2's Descendants), so that
    /// [`World::organism_snapshot`] carries [`Descendants`]: see
    /// [`crate::ancestry`]. A world traced from its founding counts every
    /// birth; one traced later starts from its live population. Tracing a
    /// world that already traces its ancestry keeps what it has. It changes
    /// no result of a run: no digest, checkpoint or report holds the index,
    /// and a step reads nothing from it. Its room, reserved now, is part of
    /// [`World::footprint`]'s state, so call it before
    /// [`World::admit_memory`].
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when the index's room cannot be
    /// reserved; the world then traces nothing.
    ///
    /// [`Descendants`]: crate::Descendants
    pub fn trace_ancestry(&mut self) -> Result<(), CoreError> {
        if self.ancestry.is_none() {
            self.ancestry = Some(Ancestry::begin(&self.population, self.tick)?);
        }
        Ok(())
    }

    /// How much of its room the ancestry index holds, while the world
    /// traces its ancestry.
    #[must_use]
    pub fn ancestry_held(&self) -> Option<AncestryHeld> {
        self.ancestry.as_ref().map(Ancestry::held)
    }

    /// Follow live `organism`'s line of descent from this committed tick
    /// on, or none (plan §2.2, "Follow descendants of a selected
    /// organism"): when the organism that has the line dies, the line
    /// passes to its heir, the descendant alive once that tick's deaths are
    /// all folded, fewest generations below it and then the eldest; with
    /// none alive the line ends (see [`crate::ancestry`]). Following an
    /// organism begins a line from it, even the one that has the line now.
    /// It needs the ancestry index ([`World::trace_ancestry`]). Like the
    /// index, the line is not scientific state: no digest, checkpoint or
    /// report holds it, and a step reads nothing from it.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when the world does not trace its
    /// ancestry or its index does not hold `organism` alive; the line
    /// followed before is kept. Following none never fails.
    pub fn follow(&mut self, organism: Option<OrganismId>) -> Result<Option<Followed>, CoreError> {
        match (&mut self.ancestry, organism) {
            (Some(ancestry), _) => ancestry.follow(organism, self.tick),
            (None, None) => Ok(None),
            (None, Some(organism)) => Err(CoreError::StateConflict {
                operation: "follow a line",
                detail: format!(
                    "the world does not trace its ancestry, so {organism}'s line has no heirs"
                ),
            }),
        }
    }

    /// The line of descent the world follows, if any ([`World::follow`]).
    #[must_use]
    pub fn followed(&self) -> Option<Followed> {
        self.ancestry.as_ref().and_then(Ancestry::followed)
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
            Some(outputs) => {
                let mut raw = [0.0_f64; MAX_ACTION_WIDTH];
                let slice = raw_outputs(&self.rules, outputs, &mut raw)?;
                Some(decode_with_layout(
                    slice,
                    self.rules.step,
                    &self.rules.decoding,
                    &self.rules.v2.actions(),
                )?)
            }
            None => None,
        };
        let local = local_chemistry(
            &self.rules,
            self.field.current(),
            organism.position,
            genome.traits(),
            organism.held.recurrent_state_array()[0],
        )?;
        let watched = self.watch.organism == Some(id);
        let sensed = self.watch.observed.clone().filter(|_| watched);
        let recorded = self.watch.evaluated.as_ref().filter(|_| watched);
        let activations = match (
            recorded,
            organism.held.evaluated_at(),
            organism.held.outputs(),
        ) {
            (Some(recorded), Some(at), Some(held)) if recorded.tick.next().ok() == Some(at) => {
                let controller = genome.controller(&self.rules.catalog, &self.rules.limits)?;
                let sensors = self.rules.v2.sensors();
                let actions = self.rules.v2.actions();
                let state_width = self.rules.recurrent_state_width;
                let obs_width = sensors.observation_width();
                let input_width = sensors.input_width();
                let mut full_inputs = [0.0_f64; MAX_SENSOR_WIDTH];
                full_inputs[..SENSOR_WIDTH].copy_from_slice(&recorded.observation.inputs);
                if let (Some(start), Some(trail)) =
                    (sensors.dual_carrier_trails(), self.watch.evaluated_trail)
                {
                    full_inputs[start..start + trail.len()].copy_from_slice(&trail);
                }
                if let (Some(slot), Some(c_den)) =
                    (sensors.hippo_crowding(), self.watch.evaluated_crowding)
                {
                    full_inputs[slot] = c_den;
                }
                if let (Some(slot), Some(h_bar)) =
                    (sensors.notch_delta(), self.watch.evaluated_notch_delta)
                {
                    full_inputs[slot] = f64::from(h_bar);
                }
                let prev_state = self
                    .watch
                    .evaluated_state
                    .unwrap_or([0.0_f32; MAXIMUM_RECURRENT_STATE_WIDTH]);
                if state_width > 0 {
                    for (slot, &h) in full_inputs[obs_width..input_width]
                        .iter_mut()
                        .zip(&prev_state[..state_width])
                    {
                        *slot = f64::from(h);
                    }
                }
                let trace = controller.trace_raw(
                    &full_inputs[..input_width],
                    &mut ControllerScratch::default(),
                )?;
                let action_width = actions.action_width();
                let trace_actions = trace.outputs.action_slice(action_width).ok_or_else(|| {
                    CoreError::StateConflict {
                        operation: "inspect organism",
                        detail: format!(
                            "{id}'s reference trace returned {} outputs, expected at least \
                             {action_width}",
                            trace.outputs.len()
                        ),
                    }
                })?;
                let outputs_match = trace_actions
                    .iter()
                    .map(|value| value.to_bits())
                    .eq(held.iter().map(|value| value.to_bits()));
                let state_matches = if state_width > 0 {
                    let tail = trace.outputs.recurrent_tail(action_width).ok_or_else(|| {
                        CoreError::StateConflict {
                            operation: "inspect organism",
                            detail: format!("{id}'s reference trace missed recurrent tail"),
                        }
                    })?;
                    let mut next_state = [0.0_f32; MAXIMUM_RECURRENT_STATE_WIDTH];
                    update_recurrent_state(
                        &prev_state[..state_width],
                        tail,
                        self.rules.v2.epigenetic_gate_bias(),
                        &mut next_state[..state_width],
                    )?;
                    apply_notch_delta_inhibition(
                        &self.rules.v2,
                        self.watch.evaluated_notch_delta.unwrap_or(0.0),
                        &mut next_state[..state_width],
                    );
                    next_state[..state_width]
                        .iter()
                        .map(|value| value.to_bits())
                        .eq(organism
                            .held
                            .recurrent_state()
                            .iter()
                            .map(|value| value.to_bits()))
                } else {
                    true
                };
                if !outputs_match || !state_matches {
                    return Err(CoreError::StateConflict {
                        operation: "inspect organism",
                        detail: format!(
                            "{id}'s output held from tick {} is not the reference's on the \
                             inputs recorded for it",
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
            local,
            decoded,
            sensed,
            activations,
            transactions,
            descendants: self
                .ancestry
                .as_ref()
                .and_then(|ancestry| ancestry.descendants(id)),
        }))
    }

    /// One organism's whole controller at this committed tick (FL-021, plan
    /// §14.2's on-demand weight dump), or `None` for an identity the
    /// population does not hold: its own genome's parameters, copied whole,
    /// and its topology's layer shapes. It copies nothing of any other
    /// organism or genome and changes nothing; see [`ControllerWeights`].
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for a live organism whose genome or
    /// topology is missing, or whose parameters are not its topology's
    /// count; [`CoreError::AllocationFailed`] when the copy's room cannot be
    /// reserved.
    pub fn organism_weights(&self, id: OrganismId) -> Result<Option<ControllerWeights>, CoreError> {
        let Some(organism) = self.population.get(id) else {
            return Ok(None);
        };
        let conflict = |detail: String| CoreError::StateConflict {
            operation: "dump organism weights",
            detail,
        };
        let genome = self
            .population
            .genome(id)
            .ok_or_else(|| conflict(format!("{id} has no genome")))?;
        let topology = self
            .rules
            .catalog
            .topology(genome.topology())
            .ok_or_else(|| conflict(format!("{id} has no topology in the catalog")))?;
        let parameters = genome.parameters();
        if parameters.len() != topology.parameter_count() {
            return Err(conflict(format!(
                "{id} holds {} parameters for a topology of {}",
                parameters.len(),
                topology.parameter_count()
            )));
        }
        let mut copy = Vec::new();
        copy.try_reserve_exact(parameters.len())
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "dumped controller weights",
                bytes: bytes_for::<f32>(parameters.len()),
            })?;
        // Widening a binary16 to F32 is exact, so the copy is the stored
        // values whatever the dtype.
        copy.extend(parameters.iter());
        Ok(Some(ControllerWeights {
            tick: self.tick,
            organism: id,
            genome: organism.genome,
            topology: genome.topology().index(),
            layers: topology.layers().to_vec(),
            dtype: parameters.dtype(),
            parameters: copy,
        }))
    }

    /// What the disk of `radius` world units about `center` holds in the
    /// committed world (plan §14.3's temporary measurement region): the
    /// organisms a removal of that disk at this boundary would take, their
    /// energy and lineages, and the field cells whose centres lie in it and
    /// their energy. See [`crate::measure`]. It changes nothing.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a centre or radius a removal
    /// would refuse (`measurement centre`, `measurement radius`);
    /// [`CoreError::AllocationFailed`] when the lineages' room cannot be
    /// reserved.
    pub fn measure_region(
        &self,
        center: [f64; 2],
        radius: f64,
    ) -> Result<RegionMeasurement, CoreError> {
        measure(
            self.tick,
            &self.rules.grid,
            &self.population,
            self.field.current(),
            center,
            radius,
        )
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
        if let Some((gx, gy)) = self.field.current().vascular_edges() {
            for &value in gx.iter().chain(gy.iter()) {
                bytes.extend_from_slice(&value.to_bits().to_le_bytes());
            }
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
        if !self.source_kernels.is_empty() {
            bytes.extend_from_slice(&(self.source_kernels.len() as u64).to_le_bytes());
            for kernel in &self.source_kernels {
                bytes.extend_from_slice(&kernel.sigma().to_bits().to_le_bytes());
            }
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
            source_kernels,
            paths,
            identities,
            quota,
            ledger,
            scratch,
            controllers,
            fields,
            sensors,
            uptake: uptake_state,
            settle: settle_state,
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
            compositions,
            emissions,
            trail_emissions,
            byproducts,
            recycling,
            survivors,
            deaths,
            contact,
            crowding,
            oasis_powers,
            barrier_positions,
            barrier_occupancy,
            flux_edges,
            trail_flux_edges,
        } = scratch;
        let from = *tick;
        let to = from.next()?;
        let next_epoch =
            ResourceEpoch::new(epoch.get().checked_add(1).ok_or(CoreError::Overflow {
                quantity: "resource epoch",
            })?);
        let step = rules.step;
        emissions.clear();
        if let Some(trail) = trail_emissions.as_mut() {
            trail.clear();
        }
        if let Some(waste) = byproducts.as_mut() {
            waste.clear();
        }
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
            rules.exchange,
            population,
            &rules.recycling,
            rows,
            &mut removals,
            recycling,
            &mut tick_ledger,
            &mut applied,
        )?;

        // V2-07: compute local crowding strictly from post-Phase-1 `rows`
        // (after `Intervention::RemoveOrganisms`), so wound-rim cells release
        // on the exact tick of the intervention and `expose_sensor` feeds
        // Phases 3–4 with zero cross-tick state.
        let crowding_counts: &[u32] = if let Some(crowding_rules) = rules.v2.crowding() {
            let positions = crowding.positions_mut();
            positions.clear();
            positions.extend(rows.iter().map(|organism| organism.position));
            match crowding_rules.mode {
                CrowdingMode::LocalNeighbors { radius_world_units } => {
                    let _ = crowding.count_buffered_local_neighbors(
                        &rules.grid,
                        radius_world_units,
                        lanes.len(),
                    )?;
                }
                CrowdingMode::CellOccupancy => {
                    let _ = crowding.count_buffered_cell_occupancy(&rules.grid)?;
                }
            }
            crowding.counts()
        } else {
            crowding.clear();
            &[]
        };

        // V2-09 / V2-14 / V2-15: compute symmetric face conductances strictly
        // from post-Phase-1 `rows` (after `Intervention::RemoveOrganisms`) and
        // committed vascular conductances, before Phase 2 transport.
        let tight_junctions = rules.v2.tight_junctions();
        let vascular = rules.v2.vascular();
        if tight_junctions.is_some() {
            let pos_slice: &[[f64; 2]] = if rules.v2.crowding().is_some() {
                crowding.positions()
            } else {
                barrier_positions.clear();
                barrier_positions.extend(rows.iter().map(|organism| organism.position));
                barrier_positions.as_slice()
            };
            count_cell_occupancy(&rules.grid, pos_slice, barrier_occupancy)?;
        }
        if let (Some(vasc), Some(edges)) = (vascular.as_ref(), flux_edges.as_mut()) {
            let (gx, gy) =
                field
                    .current()
                    .vascular_edges()
                    .ok_or_else(|| CoreError::StateConflict {
                        operation: "prepare vascular edges",
                        detail: "vascular field enabled without edge buffers on FieldState"
                            .to_owned(),
                    })?;
            let barrier_pair = tight_junctions
                .as_ref()
                .map(|tj| (tj, barrier_occupancy.as_slice()));
            vasc.fill_edges(&rules.plan, gx, gy, barrier_pair, edges)?;
        } else if let (Some(barrier), Some(edges)) = (tight_junctions.as_ref(), flux_edges.as_mut())
        {
            barrier.fill_edges(&rules.plan, barrier_occupancy, edges)?;
        }
        if let (Some(barrier), Some(trail_plan), Some(t_edges)) = (
            tight_junctions.as_ref(),
            rules.v2.trail_plan(),
            trail_flux_edges.as_mut(),
        ) {
            barrier.fill_edges(trail_plan, barrier_occupancy, t_edges)?;
        }

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
        if let Some(oases) = rules.v2.oases() {
            oases.apply_powers(from, step, rules.base_source_power, sampled, oasis_powers)?;
        }
        let field_phase2 = FieldStepPhase2 {
            source_kernels: (!source_kernels.is_empty()).then_some(source_kernels.as_slice()),
            carrier0_edges: flux_edges.as_ref(),
            vascular: vascular.as_ref(),
            trail_plan: rules.v2.trail_plan(),
            trail_edges: trail_flux_edges.as_ref(),
        };
        let field_step = advance_field_with(
            field,
            rules,
            sampled,
            step,
            field_phase2,
            field_work,
            fields,
        )?;
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
        let view = pyramid.build_on(observed, field_work.workers())?;
        let (sampled, sensor_failure) = sample_with(population, rows, &view, observed, sensors);
        decide_with(
            rules,
            population,
            &view,
            sampled,
            rows,
            crowding_counts,
            [from, to],
            lanes,
            controllers,
        )?;
        let (sensed, sensed_state, sensed_trail, sensed_crowding, sensed_notch_delta) =
            match watch.organism {
                Some(id) => watched(
                    rules,
                    population,
                    &view,
                    sampled,
                    rows,
                    crowding_counts,
                    id,
                    [from, to],
                )?,
                None => (None, None, None, None, None),
            };
        // The reference sampled the tick without an error, so a backend that
        // met one failed where the reference does not.
        if let Some(failure) = sensor_failure {
            sensors.report.failure = Some(failure);
            sensors.backend = None;
        }
        let total_decisions: usize = lanes.iter().map(|lane| lane.decisions.len()).sum();
        if total_decisions != rows.len() {
            return Err(CoreError::StateConflict {
                operation: "gather decisions",
                detail: format!("{total_decisions} decisions for {} organisms", rows.len()),
            });
        }
        let mut evaluated = 0_usize;
        if rows.len() >= 512 && lanes.len() > 1 {
            let placeholder = Working {
                committed: rows[0],
                budget: EnergyBudget::new(0.0)?,
                vitality: Vitality::new(),
                held: rows[0].held,
                evaluated: false,
                actions: DecodedActions::default(),
                gradients: [[0.0; 2]; RESPONSE_TERMS],
                basal_multiplier: 1.0,
                local_crowding: 0,
                position: [0.0, 0.0],
                velocity: [0.0, 0.0],
                cooldown_deadline: Tick::new(0),
                secretory_readiness: 0.0,
                emission_cooldown_deadline: Tick::new(0),
                emitted: false,
                lysed_residual: 0.0,
                intent: None,
            };
            set_len_reuse(working, rows.len(), placeholder);
            let active_lanes: Vec<(usize, &[Decision])> = {
                let mut v = Vec::with_capacity(lanes.len());
                let mut offset = 0_usize;
                for lane in lanes.iter() {
                    let len = lane.decisions.len();
                    if len > 0 {
                        v.push((offset, lane.decisions.as_slice()));
                        offset += len;
                    }
                }
                v
            };
            let fill_working = |start: usize,
                                row_c: &[Organism],
                                dec_c: &[Decision],
                                out_c: &mut [Working]|
             -> Result<usize, CoreError> {
                let mut local_eval = 0_usize;
                for (i, ((&organism, decision), slot)) in
                    row_c.iter().zip(dec_c).zip(out_c.iter_mut()).enumerate()
                {
                    local_eval += usize::from(decision.evaluated);
                    let basal_multiplier =
                        rules
                            .v2
                            .basal_multiplier(organism.birth_tick, from, rules.step)?;
                    let local_crowding = crowding_counts.get(start + i).copied().unwrap_or(0);
                    *slot = Working {
                        committed: organism,
                        budget: EnergyBudget::new(organism.energy)?,
                        vitality: Vitality::new(),
                        held: decision.held,
                        evaluated: decision.evaluated,
                        actions: decision.actions,
                        gradients: decision.gradients,
                        basal_multiplier,
                        local_crowding,
                        position: organism.position,
                        velocity: organism.velocity,
                        cooldown_deadline: organism.cooldown_deadline,
                        secretory_readiness: organism.secretory_readiness,
                        emission_cooldown_deadline: organism.emission_cooldown_deadline,
                        emitted: false,
                        lysed_residual: 0.0,
                        intent: None,
                    };
                }
                Ok(local_eval)
            };
            if let Some((&(head_start, head_decs), tail_decs)) = active_lanes.split_first() {
                let (head_out, mut rest_out) = working.as_mut_slice().split_at_mut(head_decs.len());
                let mut tail_results: Vec<Result<usize, CoreError>> =
                    (0..tail_decs.len()).map(|_| Ok(0)).collect();
                let mut head_result = Ok(0_usize);
                let fill_working = &fill_working;
                flife_core::pool::scope_workers(lanes.len(), "gather decisions", |scope| {
                    for (&(start, decs), res_slot) in tail_decs.iter().zip(tail_results.iter_mut())
                    {
                        let (out_c, next_out) =
                            core::mem::take(&mut rest_out).split_at_mut(decs.len());
                        rest_out = next_out;
                        let row_c = &rows[start..start + decs.len()];
                        scope.spawn(move || {
                            *res_slot = fill_working(start, row_c, decs, out_c);
                        });
                    }
                    let head_rows = &rows[head_start..head_start + head_decs.len()];
                    head_result = fill_working(head_start, head_rows, head_decs, head_out);
                })?;
                evaluated += head_result?;
                for res in tail_results {
                    evaluated += res?;
                }
            }
        } else {
            working.clear();
            let decisions = lanes.iter().flat_map(|lane| lane.decisions.iter());
            for (row_index, (&organism, decision)) in rows.iter().zip(decisions).enumerate() {
                evaluated += usize::from(decision.evaluated);
                let basal_multiplier =
                    rules
                        .v2
                        .basal_multiplier(organism.birth_tick, from, rules.step)?;
                let local_crowding = crowding_counts.get(row_index).copied().unwrap_or(0);
                working.push(Working {
                    committed: organism,
                    budget: EnergyBudget::new(organism.energy)?,
                    vitality: Vitality::new(),
                    held: decision.held,
                    evaluated: decision.evaluated,
                    actions: decision.actions,
                    gradients: decision.gradients,
                    basal_multiplier,
                    local_crowding,
                    position: organism.position,
                    velocity: organism.velocity,
                    cooldown_deadline: organism.cooldown_deadline,
                    secretory_readiness: organism.secretory_readiness,
                    emission_cooldown_deadline: organism.emission_cooldown_deadline,
                    emitted: false,
                    lysed_residual: 0.0,
                    intent: None,
                });
            }
        }

        // Phase 5: requests from the observation field, resolved per cell,
        // on the uptake backend if the world has one.
        let uptake = uptake_with(
            rules,
            working,
            observed,
            demands,
            grants,
            uptake_state,
            lanes,
        )?;
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

        // Phases 6 and 7: each organism's ordered budget stages, on the
        // lanes, with phase 6 on the settle backend if the world has one.
        // The tick then takes each lane's ledger flows and staged emissions
        // in lane order: what one thread settling every organism in identity
        // order would have recorded, in its order.
        settle_with(
            rules,
            population,
            observed,
            (by_demand, granted),
            compositions,
            from,
            watch.organism,
            working,
            lanes,
            settle_state,
        )?;
        let mut total_emission_paid = CompensatedSum::ZERO;
        if lanes.len() > 1 && working.len() >= 512 {
            let mut lane_emissions: Vec<&mut Deposits> = Vec::with_capacity(lanes.len());
            let mut lane_trails: Vec<&mut Deposits> = if trail_emissions.is_some() {
                Vec::with_capacity(lanes.len())
            } else {
                Vec::new()
            };
            let mut lane_byproducts: Vec<&mut Deposits> = if byproducts.is_some() {
                Vec::with_capacity(lanes.len())
            } else {
                Vec::new()
            };
            let merge_workers = lanes.len();
            let mut lane_flows: Vec<(&mut LedgerLog, f64, Option<Transactions>)> =
                Vec::with_capacity(merge_workers);
            for lane in lanes.iter_mut() {
                lane_emissions.push(&mut lane.emissions);
                if let Some(src) = lane.trail_emissions.as_mut() {
                    lane_trails.push(src);
                }
                if let Some(src) = lane.byproducts.as_mut() {
                    lane_byproducts.push(src);
                }
                lane_flows.push((
                    &mut lane.flows,
                    lane.emission_paid.value(),
                    lane.transactions.take(),
                ));
            }
            let mut em_res: Result<(), CoreError> = Ok(());
            let mut tr_res: Result<(), CoreError> = Ok(());
            let mut bp_res: Result<(), CoreError> = Ok(());
            let mut flow_res: Result<(), CoreError> = Ok(());
            let em_dst: &mut Deposits = emissions;
            let mut trail_dst = trail_emissions.as_mut();
            let mut bp_dst = byproducts.as_mut();
            flife_core::pool::scope_workers(merge_workers, "merge settle lanes", |scope| {
                let em_res_ref = &mut em_res;
                scope.spawn(move || {
                    for src in lane_emissions {
                        if let Err(err) = em_dst.append(src) {
                            *em_res_ref = Err(err);
                            break;
                        }
                    }
                });
                if let Some(dst) = trail_dst.take() {
                    let tr_res_ref = &mut tr_res;
                    scope.spawn(move || {
                        for src in lane_trails {
                            if let Err(err) = dst.append(src) {
                                *tr_res_ref = Err(err);
                                break;
                            }
                        }
                    });
                }
                if let Some(dst) = bp_dst.take() {
                    let bp_res_ref = &mut bp_res;
                    scope.spawn(move || {
                        for src in lane_byproducts {
                            if let Err(err) = dst.append(src) {
                                *bp_res_ref = Err(err);
                                break;
                            }
                        }
                    });
                }
                for (flows, paid, tx) in lane_flows {
                    if let Err(err) = flows.replay(&mut tick_ledger) {
                        flow_res = Err(err);
                        break;
                    }
                    total_emission_paid.add(paid);
                    if let Some(record) = tx {
                        transactions = Some(record);
                    }
                }
            })?;
            flow_res?;
            em_res?;
            tr_res?;
            bp_res?;
        } else {
            for lane in lanes.iter_mut() {
                lane.flows.replay(&mut tick_ledger)?;
                emissions.append(&mut lane.emissions)?;
                if let (Some(dst), Some(src)) =
                    (trail_emissions.as_mut(), lane.trail_emissions.as_mut())
                {
                    dst.append(src)?;
                }
                if let (Some(dst), Some(src)) = (byproducts.as_mut(), lane.byproducts.as_mut()) {
                    dst.append(src)?;
                }
                total_emission_paid.add(lane.emission_paid.value());
                if let Some(record) = lane.transactions.take() {
                    transactions = Some(record);
                }
            }
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
        let mut live_genomes = population.ascending();
        for organism in working.iter() {
            if let Some(intent) = organism.intent {
                let genome =
                    live_genomes
                        .next(intent.parent)
                        .0
                        .ok_or_else(|| CoreError::StateConflict {
                            operation: "read genome",
                            detail: format!("{} has no genome", intent.parent),
                        })?;
                requests.push(BirthRequest { intent, genome });
            }
        }
        let mutator = Mutator::new(rules.rng, rules.mutation, &rules.catalog, rules.limits)?;
        let context = BirthContext {
            rng: &rules.rng,
            grid: &rules.grid,
            reproduction: &rules.reproduction,
            mutator: &mutator,
            offspring: if rules.variant.fresh_offspring() {
                Offspring::Fresh {
                    catalog: &rules.catalog,
                    limits: &rules.limits,
                    initial_scale: rules.initial_scale,
                }
            } else {
                Offspring::Inherited
            },
            capacity: rules.capacity,
        };
        // FL-032 / FL-043: each inherited birth's first candidate mutated on
        // the settle backend in bounded waves (at most 2 MiB of parameter
        // staging per wave), if the world has one.
        let mut wave_mutator = settle::WaveMutator::new(
            settle_state,
            rules,
            &mutator,
            quota.dtype(),
            matches!(context.offspring, Offspring::Inherited),
        );
        let mut next_identities = identities.clone();
        let mut next_quota = quota.clone();
        let births = match resolve_births_with_wave_preparer(
            &context,
            &mut wave_mutator,
            &requests,
            PopulationQuota::new(rules.maximum_population, live)?,
            &mut next_identities,
            &mut next_quota,
            lanes.len(),
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
        // The contact models displace every survivor from the settled
        // positions, before any moves (crate::contact).
        let contacted = if let Some(bodies) = &rules.contact {
            contact_displacements(rules, bodies, population, working, contact, lanes.len())?;
            if rules.v2.trophallaxis().is_some()
                && contact.trophallaxis_summary.total_out_quanta > 0
            {
                tick_ledger.transfer(
                    Transfer::Trophallaxis,
                    contact.trophallaxis_summary.debited(),
                    contact.trophallaxis_summary.credited(),
                )?;
            }
            Some(&contact.displacements)
        } else {
            None
        };
        // Under contact inhibition, each survivor's push from the bodies it
        // overlapped; empty otherwise.
        let pressed = contact.pressed.as_slice();
        let next_polarities = if rules.v2.polarity().is_some() && rules.contact.is_some() {
            contact.next_polarities.as_slice()
        } else {
            &[]
        };
        let next_neighbor_h0 = if rules.v2.notch_delta().is_some() && rules.contact.is_some() {
            contact.neighbor_h0.as_slice()
        } else {
            &[]
        };
        let trophallaxis_deltas = if rules.v2.trophallaxis().is_some() && rules.contact.is_some() {
            contact.trophallaxis_delta_quanta.as_slice()
        } else {
            &[]
        };
        let next_velocities = if rules.v2.velocity_alignment().is_some() && rules.contact.is_some()
        {
            contact.next_velocities.as_slice()
        } else {
            &[]
        };
        let mut death_genomes = population.ascending();
        for (index, organism) in working.iter().enumerate() {
            emitted += usize::from(organism.emitted);
            match organism.vitality.fate()? {
                Fate::Survives => {
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "signed per-organism quantum delta is exact in f64 for |delta_k| <= 2^53"
                    )]
                    let trophallaxis_delta = trophallaxis_deltas
                        .get(index)
                        .copied()
                        .map_or(0.0, |k| (k as f64) * TROPHALLAXIS_QUANTUM);
                    let energy = if trophallaxis_delta != 0.0 {
                        let updated = organism.budget.remaining() + trophallaxis_delta;
                        if !updated.is_finite() || updated <= 0.0 {
                            return Err(CoreError::NumericalInvalid {
                                quantity: "trophallaxis survivor energy",
                                detail: format!(
                                    "{} post-trophallaxis energy {updated:?} is not finite and positive",
                                    organism.committed.id
                                ),
                            });
                        }
                        updated
                    } else {
                        organism.budget.remaining()
                    };
                    if trophallaxis_delta != 0.0
                        && let Some(record) = transactions
                            .as_mut()
                            .filter(|_| watch.organism == Some(organism.committed.id))
                    {
                        record.trophallaxis_delta = trophallaxis_delta;
                        record.closing = energy;
                    }
                    organisms.add(energy);
                    let position = match contacted.and_then(|moves| moves.get(index)) {
                        Some(&[dx, dy]) if dx != 0.0 || dy != 0.0 => rules.grid.wrap_position([
                            organism.position[0] + dx,
                            organism.position[1] + dy,
                        ])?,
                        _ => organism.position,
                    };
                    let base_velocity = next_velocities
                        .get(index)
                        .copied()
                        .unwrap_or(organism.velocity);
                    let velocity = match (pressed.get(index), &rules.contact) {
                        (Some(&push), Some(bodies)) => {
                            inhibited(base_velocity, push, bodies.inhibition)
                        }
                        _ => base_velocity,
                    };
                    let polarity = next_polarities
                        .get(index)
                        .copied()
                        .unwrap_or(organism.committed.polarity);
                    let last_neighbor_h0 = next_neighbor_h0.get(index).copied().unwrap_or(0.0);
                    survivors.push(Organism {
                        position,
                        velocity,
                        energy,
                        energy_change_rate: (energy - organism.committed.energy) / seconds,
                        held: organism.held,
                        cooldown_deadline: organism.cooldown_deadline,
                        secretory_readiness: organism.secretory_readiness,
                        emission_cooldown_deadline: organism.emission_cooldown_deadline,
                        polarity,
                        last_neighbor_h0,
                        ..organism.committed
                    });
                }
                Fate::Dies(stage) => {
                    let genome = death_genomes.next(organism.committed.id).0.ok_or_else(|| {
                        CoreError::StateConflict {
                            operation: "read genome",
                            detail: format!("{} has no genome", organism.committed.id),
                        }
                    })?;
                    let recurrent_h0 = organism.held.recurrent_state_array()[0];
                    let (death, corpse_dir) = if organism.lysed_residual > 0.0 {
                        let death = Death::lysed(
                            &organism.committed,
                            to,
                            organism.position,
                            stage,
                            organism.lysed_residual,
                        )?;
                        let direction = rules
                            .v2
                            .lysis()
                            .map_or(CorpseDirection::GeneticAffinity, |lysis| {
                                lysis.corpse_direction
                            });
                        let a_eff = effective_affinity(
                            genome.traits(),
                            recurrent_h0,
                            &rules.v2,
                            rules.chemistry.affinity_norm_limit(),
                        )?;
                        let corpse_dir = corpse_affinity_direction(
                            &a_eff,
                            direction,
                            rules.v2.active_carrier0_channels(),
                            rules.chemistry.affinity_norm_limit(),
                        )?;
                        (death, corpse_dir)
                    } else {
                        let death =
                            Death::exhausted(&organism.committed, to, organism.position, stage)?;
                        (death, *genome.traits().affinity())
                    };
                    rules
                        .recycling
                        .settle_with(&rules.grid, rules.exchange, &death, &corpse_dir, recycling)?
                        .record(&mut tick_ledger)?;
                    deaths.push(death);
                }
            }
        }
        let attempted = requests.len();
        let (born, rejected, refused) = births.into_parts();
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
                pcp_polarity: rules.v2.polarity().is_some(),
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

        // Phase 9: the grants' debits, then the paid deposits, recycling, and
        // any non-empty V2-11 byproducts, reduced into the next field. On the
        // CPU path all Carrier-0 deposit streams fuse into one worker-pool
        // dispatch via `apply_deposit_streams_to_carrier_on` (bit-identical
        // sequential per-cell order).
        let (_, next) = field.stage();
        let workers = field_work.workers();
        let mut byproducts_recorded = false;
        if let Some(trail_deposits) = trail_emissions.as_mut().filter(|_| next.is_dual_carrier()) {
            let (emitted_c0, recycled_summary, emitted_c1) = if fields.backend.is_none() {
                apply_grants_on(next, grants, workers)?;
                let (emitted_c0, recycled_summary) = if let Some(bp) = byproducts.as_mut()
                    && !bp.is_empty()
                {
                    let mut summaries = [None; 3];
                    apply_deposit_streams_to_carrier_on(
                        next,
                        &mut [emissions, recycling, bp],
                        next.carrier0(),
                        workers,
                        &mut summaries,
                    )?;
                    let [Some(em), Some(rec), Some(bp_summary)] = summaries else {
                        return Err(CoreError::StateConflict {
                            operation: "reduce field deposits",
                            detail: "missing deposit summaries for fused dual-carrier reduction"
                                .to_owned(),
                        });
                    };
                    bp_summary.record(&mut tick_ledger, Transfer::MetabolicByproduct)?;
                    byproducts_recorded = true;
                    (em, rec)
                } else {
                    let mut summaries = [None; 2];
                    apply_deposit_streams_to_carrier_on(
                        next,
                        &mut [emissions, recycling],
                        next.carrier0(),
                        workers,
                        &mut summaries,
                    )?;
                    let [Some(em), Some(rec)] = summaries else {
                        return Err(CoreError::StateConflict {
                            operation: "reduce field deposits",
                            detail: "missing deposit summaries for Carrier 0 reduction".to_owned(),
                        });
                    };
                    (em, rec)
                };
                let c1 = next.carrier1().ok_or_else(|| CoreError::LayoutMismatch {
                    buffer: "field",
                    detail: "Carrier 1 missing on dual-carrier field".to_owned(),
                })?;
                let emitted_c1 = apply_deposits_to_carrier_on(next, trail_deposits, c1, workers)?;
                (emitted_c0, recycled_summary, emitted_c1)
            } else {
                let ([emitted_c0, recycled_summary], emitted_c1) = reduce_dual_carrier_with(
                    next,
                    grants,
                    [emissions, recycling],
                    trail_deposits,
                    workers,
                    fields,
                )?;
                (emitted_c0, recycled_summary, emitted_c1)
            };
            let debited = total_emission_paid
                .value()
                .max(emitted_c0.paid + emitted_c1.paid);
            let credited = (emitted_c0.applied + emitted_c1.applied).min(debited);
            tick_ledger.transfer(Transfer::Emission, debited, credited)?;
            recycled_summary.record(&mut tick_ledger, Transfer::Recycling)?;
        } else if fields.backend.is_none() {
            apply_grants_on(next, grants, workers)?;
            let (emitted_summary, recycled_summary) = if let Some(bp) = byproducts.as_mut()
                && !bp.is_empty()
            {
                let mut summaries = [None; 3];
                apply_deposit_streams_to_carrier_on(
                    next,
                    &mut [emissions, recycling, bp],
                    next.carrier0(),
                    workers,
                    &mut summaries,
                )?;
                let [Some(em), Some(rec), Some(bp_summary)] = summaries else {
                    return Err(CoreError::StateConflict {
                        operation: "reduce field deposits",
                        detail: "missing deposit summaries for fused reduction".to_owned(),
                    });
                };
                bp_summary.record(&mut tick_ledger, Transfer::MetabolicByproduct)?;
                byproducts_recorded = true;
                (em, rec)
            } else {
                let mut summaries = [None; 2];
                apply_deposit_streams_to_carrier_on(
                    next,
                    &mut [emissions, recycling],
                    next.carrier0(),
                    workers,
                    &mut summaries,
                )?;
                let [Some(em), Some(rec)] = summaries else {
                    return Err(CoreError::StateConflict {
                        operation: "reduce field deposits",
                        detail: "missing deposit summaries for emissions and recycling".to_owned(),
                    });
                };
                (em, rec)
            };
            emitted_summary.record(&mut tick_ledger, Transfer::Emission)?;
            recycled_summary.record(&mut tick_ledger, Transfer::Recycling)?;
        } else {
            let mut summaries = [None; 2];
            reduce_field_with(
                next,
                grants,
                &mut [
                    (emissions, CarrierSpec::Carrier0),
                    (recycling, CarrierSpec::Carrier0),
                ],
                &mut summaries,
                workers,
                fields,
            )?;
            let [Some(emitted_summary), Some(recycled_summary)] = summaries else {
                return Err(CoreError::StateConflict {
                    operation: "reduce field deposits",
                    detail: "missing deposit reduction summaries for emissions and recycling"
                        .to_owned(),
                });
            };
            emitted_summary.record(&mut tick_ledger, Transfer::Emission)?;
            recycled_summary.record(&mut tick_ledger, Transfer::Recycling)?;
        }
        if !byproducts_recorded && let Some(byproducts) = byproducts.as_mut() {
            let byproduct_summary = if byproducts.is_empty() {
                DepositSummary::default()
            } else {
                apply_deposits_to_carrier_on(next, byproducts, next.carrier0(), workers)?
            };
            byproduct_summary.record(&mut tick_ledger, Transfer::MetabolicByproduct)?;
        }

        // Phase 10: invariants, accounting and the population's commit. The
        // field's check runs on the field backend if the world has one
        // (FL-032), strictly read-only before any plan §11.5 moment-only
        // contraction so genuine INV-02 violations (> 1e-5) always fail.
        // `field_summary.energy_total` sums `e_0` (+ `e_1`) in exact ascending
        // cell order and plan §11.5 cone contraction never touches energy, so
        // `field_summary.energy_total` is bit-identical to `next.energy_total()`.
        let (field_summary, cone_contraction) = check_field_with(
            next,
            rules.profile.cone,
            rules.profile.cone_contraction_trigger(),
            workers,
            fields,
        )?;
        let closing = Holdings::new(field_summary.energy_total, organisms.value())?;
        let closure = tick_ledger.close(closing)?;
        let mut next_ledger = ledger.clone();
        let cumulative = next_ledger.commit(&closure)?;
        let checked =
            population.check_tick_staged(to, survivors, deaths, &entrants, &next_quota)?;

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
            refused,
            deaths: deaths.clone(),
            uptake,
            field_step,
            field: field_summary,
            cone_contraction,
            closure,
            ledger: cumulative,
            interventions: applied,
        };
        Ok(Prepared::Ready(Box::new(Staged {
            tick: to,
            epoch: next_epoch,
            events: end,
            checked,
            births: entrants,
            ledger: next_ledger,
            quota: next_quota,
            identities: next_identities,
            paths: next_paths,
            sensed,
            sensed_state,
            sensed_trail,
            sensed_crowding,
            sensed_notch_delta,
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

/// A founder's initial unit apical-basal polarity vector (`V2-08`,
/// `pcp-polarity`): uniform angle on `[0, 2π)` from word 0 of
/// `(founders, organism, 0, lane (0, direction, 0x0802), 0)` via
/// host-independent [`sin_cos_turns`].
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for a degenerate draw; [`CoreError::Overflow`]
/// for lane construction.
pub fn founder_polarity(rng: &SemanticRng, organism: OrganismId) -> Result<[f64; 2], CoreError> {
    let [word, ..] = rng.block(Address {
        stream: Stream::Founders,
        entity: organism.get(),
        ordinal: 0,
        lane: Lane::new(0, LaneKind::Direction, 0x0802)?,
        draw: 0,
    });
    let (sin, cos) =
        sin_cos_turns(half_open_unit(word)).ok_or_else(|| CoreError::NumericalInvalid {
            quantity: "founder polarity angle",
            detail: format!("word {word:#018x} left the unit interval"),
        })?;
    let norm = cos.hypot(sin);
    if !norm.is_finite() || norm <= 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "founder polarity",
            detail: format!("polarity [{cos:?}, {sin:?}] has nonpositive norm"),
        });
    }
    Ok([cos / norm, sin / norm])
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

/// Per-source heterogeneous Gaussian kernels when `V2-02` (`oases`) is active;
/// empty (0 bytes) on `v1` or when `oases` is disabled.
fn initial_source_kernels(
    rules: &Rules,
    sources: &[Source],
) -> Result<Vec<SourceKernel>, CoreError> {
    match rules.v2.oases() {
        Some(oases) => oases.seed_kernels(&rules.rng, &rules.kernel, sources),
        None => Ok(Vec::new()),
    }
}

impl Scratch {
    /// Empty scratch for `sources` sources at the configuration's worker
    /// count.
    fn new(
        config: &ValidatedConfig,
        rules: &Rules,
        sources: usize,
        source_kernels: &[SourceKernel],
    ) -> Result<Self, CoreError> {
        let limits = BufferLimits::host();
        let workers_configured = config.config().execution.cpu_workers;
        // Contribution scratch for the largest live population (§9.4): every
        // organism demands, emits and dies at most once per tick, and each
        // touches at most `rules.max_exchange_cells` cells (4 under
        // `grid-cic-v1`, up to 4,096 under `fixed-world-gaussian-v1`).
        let organisms = rules.maximum_population;
        let per_organism =
            u64::try_from(rules.max_exchange_cells).map_err(|_| CoreError::Overflow {
                quantity: "organism packets",
            })?;
        let packets = organisms
            .checked_mul(per_organism)
            .ok_or(CoreError::Overflow {
                quantity: "organism packets",
            })?;
        // Every per-organism list is reserved for the largest population too
        // (FL-043), so the footprint taken now bounds every later tick.
        let rows = capacity_of(rules)?;
        let mut field = FieldWork::new(&rules.grid, &limits, workers(workers_configured)?)?;
        if source_kernels.is_empty() {
            field.reserve_supports(sources, &rules.kernel)?;
        } else {
            field.reserve_supports_for_kernels(source_kernels)?;
        }
        let oasis_powers = if rules.v2.oases().is_some() {
            reserved(sources, "oasis source powers")?
        } else {
            Vec::new()
        };
        let (barrier_positions, barrier_occupancy) = if rules.v2.tight_junctions().is_some() {
            let cells =
                usize::try_from(rules.grid.cell_count()).map_err(|_| CoreError::Overflow {
                    quantity: "grid cells",
                })?;
            let occ_bytes = rules
                .grid
                .cell_count()
                .checked_mul(4)
                .ok_or(CoreError::Overflow {
                    quantity: "cell occupancy scratch bytes",
                })?;
            limits.admit("cell occupancy scratch bytes", occ_bytes)?;
            let mut occupancy = reserved(cells, "cell occupancy scratch")?;
            occupancy.resize(cells, 0);
            let positions = reserved(rows, "tight-junction positions")?;
            (positions, occupancy)
        } else {
            (Vec::new(), Vec::new())
        };
        let flux_edges = if rules.v2.tight_junctions().is_some() || rules.v2.vascular().is_some() {
            Some(FluxEdges::new(&rules.grid, &limits)?)
        } else {
            None
        };
        let trail_flux_edges =
            if rules.v2.dual_carrier().is_some() && rules.v2.tight_junctions().is_some() {
                Some(FluxEdges::new(&rules.grid, &limits)?)
            } else {
                None
            };
        Ok(Self {
            field,
            pyramid: SensorPyramid::from_config(config, &limits)?,
            rows: reserved(rows, "organism rows")?,
            lanes: lanes(workers(workers_configured)?.get(), rules)?,
            sampled: Vec::with_capacity(sources),
            working: reserved(rows, "working organisms")?,
            demands: reserved(rows, "organism demands")?,
            grants: Grants::with_capacity_for(&rules.grid, organisms, rules.exchange, &limits)?,
            by_demand: Bins::with_capacity("organism packets", organisms, packets, &limits)?,
            compositions: reserved(
                usize::try_from(packets).map_err(|_| CoreError::Overflow {
                    quantity: "organism packets",
                })?,
                "packet compositions",
            )?,
            emissions: Deposits::with_capacity_for(
                &rules.grid,
                organisms,
                rules.exchange,
                &limits,
            )?,
            trail_emissions: if rules.v2.dual_carrier().is_some() {
                Some(Deposits::with_capacity_for(
                    &rules.grid,
                    organisms,
                    rules.exchange,
                    &limits,
                )?)
            } else {
                None
            },
            byproducts: if rules.v2.waste().is_some() {
                Some(Deposits::with_capacity_for(
                    &rules.grid,
                    organisms,
                    rules.exchange,
                    &limits,
                )?)
            } else {
                None
            },
            recycling: Deposits::with_capacity_for(
                &rules.grid,
                organisms,
                rules.exchange,
                &limits,
            )?,
            survivors: reserved(rows, "survivors")?,
            deaths: reserved(rows, "deaths")?,
            contact: match &rules.contact {
                Some(bodies) => ContactWork::with_capacity_v2_features(
                    &rules.grid,
                    bodies,
                    rows,
                    ContactV2Buffers {
                        pcp_polarity: rules.v2.polarity().is_some(),
                        emt_gate: rules.v2.emt_gate() || rules.v2.emt_h1_adhesion_enabled(),
                        notch_delta: rules.v2.notch_delta().is_some(),
                        cadherin: rules.v2.cadherin().is_some(),
                        trophallaxis: rules.v2.trophallaxis().is_some(),
                        velocity_alignment: rules.v2.velocity_alignment().is_some(),
                    },
                )?,
                None => ContactWork::default(),
            },
            crowding: match rules.v2.crowding() {
                Some(crowding) => match crowding.mode {
                    CrowdingMode::LocalNeighbors { radius_world_units } => {
                        CrowdingWork::with_capacity_local_neighbors(
                            &rules.grid,
                            radius_world_units,
                            rows,
                        )?
                    }
                    CrowdingMode::CellOccupancy => {
                        CrowdingWork::with_capacity_cell_occupancy(&rules.grid, rows)?
                    }
                },
                None => CrowdingWork::default(),
            },
            oasis_powers,
            barrier_positions,
            barrier_occupancy,
            flux_edges,
            trail_flux_edges,
        })
    }
}

/// What every founder shares: the initial sensing scale and energy and the
/// cooldown deadline.
struct Founding {
    scale: f64,
    energy: f64,
    cooldown_deadline: Tick,
}

/// One founder before its genome grows ([`seeds`]).
enum Seed {
    /// Its identities and the quota's buffer for its genome.
    Ready {
        organism: OrganismId,
        genome: GenomeId,
        buffer: ParameterBuffer,
    },
    /// An identity could not be issued: the founder fails before it draws
    /// anything.
    Unissued(CoreError),
    /// The catalog has no default topology or the quota refused the buffer:
    /// the founder fails after drawing its traits, as
    /// [`flife_genome::founder()`] does.
    Unallocated {
        organism: OrganismId,
        error: CoreError,
    },
}

/// `count` founders' identities and genome buffers, issued from
/// `identities` and `quota` in identity order on this thread, as
/// [`flife_genome::founder()`] would one by one. The first founder that
/// cannot be issued ends the list.
///
/// # Errors
/// [`CoreError::AllocationFailed`] if the list cannot be reserved.
fn seeds(
    rules: &Rules,
    identities: &mut Identities,
    quota: &mut GenomeQuota,
    count: usize,
) -> Result<Vec<Seed>, CoreError> {
    let member = founder_topology(&rules.catalog);
    let mut seeds = reserved(count, "founder buffers")?;
    for _ in 0..count {
        let issued = identities
            .organisms
            .issue()
            .and_then(|organism| identities.genomes.issue().map(|genome| (organism, genome)));
        let (organism, genome) = match issued {
            Ok(issued) => issued,
            Err(error) => {
                seeds.push(Seed::Unissued(error));
                break;
            }
        };
        match member
            .clone()
            .and_then(|member| quota.allocate(&rules.catalog, member))
        {
            Ok(buffer) => seeds.push(Seed::Ready {
                organism,
                genome,
                buffer,
            }),
            Err(error) => {
                seeds.push(Seed::Unallocated { organism, error });
                break;
            }
        }
    }
    Ok(seeds)
}

/// Grow `seeds` into founders ([`grown`]) on at most `workers` threads,
/// appending them to `entrants` in identity order: this thread and scoped
/// workers it joins before returning, each over a contiguous
/// identity-ordered run, with no more runs than workers or started groups
/// of [`MINIMUM_ROWS_PER_LANE`] seeds (plan §3.4's bounded reference
/// adapter, split as [`in_runs`] splits rows). A run stops at its first
/// failure, and the error returned is the first in identity order. A
/// founding that fails drops its buffers with its quota.
fn grow_founders(
    rules: &Rules,
    founding: &Founding,
    mut seeds: Vec<Seed>,
    workers: NonZeroUsize,
    entrants: &mut Vec<Entrant>,
) -> Result<(), CoreError> {
    let used = workers
        .get()
        .min(seeds.len().div_ceil(MINIMUM_ROWS_PER_LANE))
        .max(1);
    let length = seeds.len().div_ceil(used).max(1);
    let mut runs = reserved(used, "founder runs")?;
    while seeds.len() > length {
        let last = (seeds.len() - 1) / length * length;
        let mut run = reserved(seeds.len() - last, "founder buffers")?;
        run.extend(seeds.drain(last..));
        runs.push(run);
    }
    runs.push(seeds);
    runs.reverse();
    let grow = |run: Vec<Seed>| -> Result<Vec<Entrant>, CoreError> {
        let mut founders = reserved(run.len(), "founders")?;
        for seed in run {
            founders.push(grown(rules, founding, seed)?);
        }
        Ok(founders)
    };
    let grow = &grow;
    let mut runs = runs.into_iter();
    let head = runs.next().unwrap_or_default();
    if used <= 1 {
        entrants.extend(grow(head)?);
        return Ok(());
    }
    let tail_count = runs.len();
    let mut tail_results: Vec<Result<Vec<Entrant>, CoreError>> =
        (0..tail_count).map(|_| Ok(Vec::new())).collect();
    let mut head_result = Ok(Vec::new());
    flife_core::pool::scope_workers(workers.get(), "found", |scope| {
        for (run, slot) in runs.zip(tail_results.iter_mut()) {
            scope.spawn(move || {
                *slot = grow(run);
            });
        }
        head_result = grow(head);
    })?;
    entrants.extend(head_result?);
    for part in tail_results {
        entrants.extend(part?);
    }
    Ok(())
}

/// `seed` grown into a founder: [`flife_genome::founder()`]'s traits and
/// genome in the seed's buffer, then the organism at its seeded position
/// and stable phase, failing where a single thread would.
fn grown(rules: &Rules, founding: &Founding, seed: Seed) -> Result<Entrant, CoreError> {
    let (id, genome_id, buffer) = match seed {
        Seed::Ready {
            organism,
            genome,
            buffer,
        } => (organism, genome, buffer),
        Seed::Unissued(error) => return Err(error),
        Seed::Unallocated { organism, error } => {
            founder_traits_v2(&rules.rng, organism, &rules.limits, founding.scale)?;
            return Err(error);
        }
    };
    let traits = founder_traits_v2(&rules.rng, id, &rules.limits, founding.scale)?;
    let genome = grow_founder(
        &rules.rng,
        id,
        genome_id,
        &rules.catalog,
        buffer,
        traits,
        &rules.limits,
    )
    .map_err(|unsealed| unsealed.error)?;
    let polarity = if rules.v2.polarity().is_some() {
        founder_polarity(&rules.rng, id)?
    } else {
        DEFAULT_POLARITY
    };
    let organism = Organism {
        id,
        genome: genome_id,
        parent: None,
        lineage: LineageId::new(id.get()),
        generation: Generation::FOUNDER,
        birth_tick: Tick::new(0),
        position: founder_position(&rules.rng, &rules.grid, id)?,
        velocity: [0.0, 0.0],
        energy: founding.energy,
        energy_change_rate: 0.0,
        held: HeldOutput::INVALID,
        phase: stable_phase(id, rules.interval)?,
        cooldown_deadline: founding.cooldown_deadline,
        secretory_readiness: INITIAL_SECRETORY_READINESS,
        emission_cooldown_deadline: Tick::new(0),
        polarity,
        last_neighbor_h0: 0.0,
    };
    Ok(Entrant { organism, genome })
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
    fit_lanes(&mut lanes, rules, capacity_of(rules)?, false)?;
    Ok(lanes)
}

/// An empty decision lane with its controller scratch for the catalog's
/// widest member, its exchange-footprint scratch for the world's kernel,
/// and its pre-reserved 256-query sampling batch buffers.
fn lane(rules: &Rules) -> Result<DecisionLane, CoreError> {
    let widest = rules
        .catalog
        .ids()
        .filter_map(|id| rules.catalog.topology(id))
        .max_by_key(|topology| topology.hidden_widths().iter().copied().max().unwrap_or(0));
    let mut sample_queries = reserved(SAMPLE_BATCH, "sample queries")?;
    sample_queries.resize(
        SAMPLE_BATCH,
        SensorQuery {
            position: [0.0, 0.0],
            scale: 1.0,
        },
    );
    let (raw_samples, dual_raw_samples) = if rules.v2.dual_carrier().is_some() {
        let mut dual = reserved(SAMPLE_BATCH, "dual raw samples")?;
        dual.resize(
            SAMPLE_BATCH,
            DualRawSample {
                values: [0.0; MAX_PLANES],
                dx: [0.0; MAX_PLANES],
                dy: [0.0; MAX_PLANES],
            },
        );
        (Vec::new(), dual)
    } else {
        let mut single = reserved(SAMPLE_BATCH, "raw samples")?;
        single.resize(
            SAMPLE_BATCH,
            RawSample {
                values: [0.0; RESPONSE_TERMS],
                dx: [0.0; RESPONSE_TERMS],
                dy: [0.0; RESPONSE_TERMS],
            },
        );
        (single, Vec::new())
    };
    Ok(DecisionLane {
        controller: match widest {
            Some(topology) => ControllerScratch::new(topology)?,
            None => ControllerScratch::default(),
        },
        exchange: ExchangeScratch::for_kernel(rules.exchange, &rules.grid)?,
        sample_queries,
        raw_samples,
        dual_raw_samples,
        ..DecisionLane::default()
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

/// Reserve each lane's decisions, ledger flows and staged emissions, and
/// with `batched` its observations, for the most rows it can take of
/// `organisms`, and its packets for one organism; without `batched`,
/// release its observations.
fn fit_lanes(
    lanes: &mut [DecisionLane],
    rules: &Rules,
    organisms: usize,
    batched: bool,
) -> Result<(), CoreError> {
    let rows = lane_rows(organisms, lanes.len());
    let needs_expanded = rules.v2.sensors().input_width() > SENSOR_WIDTH;
    for lane in lanes {
        fit(&mut lane.decisions, rows, "decisions")?;
        fit(
            &mut lane.packets,
            rules.max_exchange_cells,
            "organism packets",
        )?;
        lane.flows.fit(rows.saturating_mul(FLOWS_PER_ORGANISM))?;
        lane.emissions
            .fit_staging_for_kernel(&rules.grid, rows, rules.exchange)?;
        if rules.v2.dual_carrier().is_some() {
            let trail = lane.trail_emissions.get_or_insert_with(Deposits::new);
            trail.fit_staging_for_kernel(&rules.grid, rows, rules.exchange)?;
        } else {
            lane.trail_emissions = None;
        }
        if rules.v2.waste().is_some() {
            let byproducts = lane.byproducts.get_or_insert_with(Deposits::new);
            byproducts.fit_staging_for_kernel(&rules.grid, rows, rules.exchange)?;
        } else {
            lane.byproducts = None;
        }
        lane.emission_paid = CompensatedSum::ZERO;
        lane.transactions = None;
        if batched {
            fit(&mut lane.observed, rows, "observations")?;
            if needs_expanded {
                fit(&mut lane.controller_inputs, rows, "controller inputs")?;
            } else {
                lane.controller_inputs = Vec::new();
            }
        } else {
            lane.observed = Vec::new();
            lane.controller_inputs = Vec::new();
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
/// hands the step to [`advance_field_on`] / [`advance_field_v2_on`] /
/// [`advance_field_wave2_on`], which reproduces the reference's own error or
/// finds none; with none, the backend failed where the reference does not, and
/// the world stops using it. Either way the step is the reference's.
fn advance_field_with(
    pair: &mut FieldPair,
    rules: &Rules,
    sources: &[Source],
    step: TimeStep,
    phase2: FieldStepPhase2<'_>,
    work: &mut FieldWork,
    fields: &mut Fields,
) -> Result<FieldStepReport, CoreError> {
    let wave1_only = phase2.is_wave1_only()
        && !pair.current().is_dual_carrier()
        && !pair.current().has_vascular();
    let options = FieldStepOptions {
        source_kernels: phase2.source_kernels,
        flux_edges: phase2.carrier0_edges,
    };
    let baseline = wave1_only && options.is_baseline() && rules.v2.oases().is_none();
    let Fields { backend, report } = fields;
    if let Some(device) = backend.as_deref_mut() {
        let attempted = if baseline {
            device.advance(pair, &rules.plan, &rules.kernel, sources, step)
        } else if wave1_only {
            device.advance_v2(pair, &rules.plan, &rules.kernel, sources, step, options)
        } else {
            device.advance_wave2(pair, &rules.plan, &rules.kernel, sources, step, phase2)
        };
        match attempted {
            Ok(stepped) => {
                report.steps = report.steps.saturating_add(1);
                return Ok(stepped);
            }
            Err(error) => {
                report.reference_steps = report.reference_steps.saturating_add(1);
                let stepped = if baseline {
                    advance_field_on(pair, &rules.plan, &rules.kernel, sources, step, work)?
                } else if wave1_only {
                    advance_field_v2_on(
                        pair,
                        &rules.plan,
                        &rules.kernel,
                        sources,
                        step,
                        options,
                        work,
                    )?
                } else {
                    advance_field_wave2_on(
                        pair,
                        &rules.plan,
                        &rules.kernel,
                        sources,
                        step,
                        phase2,
                        work,
                    )?
                };
                report.failure = Some(error.to_string());
                *backend = None;
                return Ok(stepped);
            }
        }
    }
    if baseline {
        advance_field_on(pair, &rules.plan, &rules.kernel, sources, step, work)
    } else if wave1_only {
        advance_field_v2_on(
            pair,
            &rules.plan,
            &rules.kernel,
            sources,
            step,
            options,
            work,
        )
    } else {
        advance_field_wave2_on(
            pair,
            &rules.plan,
            &rules.kernel,
            sources,
            step,
            phase2,
            work,
        )
    }
}

/// Phase 9 on the world's field backend if it has one (FL-032), generalized to
/// arbitrary multi-deposit slices for `field-ecology-v2` (V2-01). When given
/// the baseline 2-stream `Carrier0` pair (`emissions`, `recycling`), it takes
/// the `FieldBackend::reduce` fast path; any error there falls back cleanly to
/// the reference. Multi-stream slices reduce grants once then each deposit
/// stream in fixed slice order.
fn reduce_field_with(
    next: &mut FieldState,
    grants: &mut Grants,
    deposits: &mut [(&mut Deposits, CarrierSpec)],
    out: &mut [Option<DepositSummary>],
    workers: usize,
    fields: &mut Fields,
) -> Result<(), CoreError> {
    if deposits.len() != out.len() {
        return Err(CoreError::LayoutMismatch {
            buffer: "deposit summaries",
            detail: format!(
                "{} deposit streams for {} summary slots",
                deposits.len(),
                out.len()
            ),
        });
    }
    if deposits.len() == 2
        && deposits[0].1 == CarrierSpec::Carrier0
        && deposits[1].1 == CarrierSpec::Carrier0
    {
        let (first, rest) = deposits.split_at_mut(1);
        let emissions = &mut *first[0].0;
        let recycling = &mut *rest[0].0;
        let Fields { backend, report } = fields;
        if let Some(device) = backend.as_deref_mut() {
            match device.reduce(next, grants, [&mut *emissions, &mut *recycling]) {
                Ok([emitted, recycled]) => {
                    report.reductions = report.reductions.saturating_add(1);
                    out[0] = Some(emitted);
                    out[1] = Some(recycled);
                    return Ok(());
                }
                Err(error) => {
                    report.reference_reductions = report.reference_reductions.saturating_add(1);
                    let [emitted, recycled] =
                        reduce_field_on(next, grants, [emissions, recycling], workers)?;
                    out[0] = Some(emitted);
                    out[1] = Some(recycled);
                    report.failure = Some(error.to_string());
                    *backend = None;
                    return Ok(());
                }
            }
        }
        let [emitted, recycled] = reduce_field_on(next, grants, [emissions, recycling], workers)?;
        out[0] = Some(emitted);
        out[1] = Some(recycled);
        return Ok(());
    }
    apply_grants_on(next, grants, workers)?;
    for ((deposit, carrier), slot) in deposits.iter_mut().zip(out.iter_mut()) {
        let layout = match carrier {
            CarrierSpec::Carrier0 => next.carrier0(),
            CarrierSpec::Carrier1 => next.carrier1().ok_or_else(|| CoreError::LayoutMismatch {
                buffer: "field",
                detail: "Carrier1 deposit offered to a 33-plane single-carrier field".to_owned(),
            })?,
        };
        *slot = Some(apply_deposits_to_carrier_on(
            next, deposit, layout, workers,
        )?);
    }
    Ok(())
}

/// Phase 9 for a 34-plane dual-carrier field (`V2-15`): debits `grants`
/// strictly from Carrier 0, reduces `carrier0_deposits` (`[emissions_0,
/// recycling]`) into Carrier 0 `(0, 1, 24)`, and reduces `trail_deposits` into
/// Carrier 1 `(25, 26, 8)` on the field backend if present, falling back to
/// [`reduce_dual_carrier_field_on`] cleanly on error.
fn reduce_dual_carrier_with(
    next: &mut FieldState,
    grants: &mut Grants,
    carrier0_deposits: [&mut Deposits; 2],
    trail_deposits: &mut Deposits,
    workers: usize,
    fields: &mut Fields,
) -> Result<([DepositSummary; 2], DepositSummary), CoreError> {
    let [emissions, recycling] = carrier0_deposits;
    let Fields { backend, report } = fields;
    if let Some(device) = backend.as_deref_mut() {
        match device.reduce_dual_carrier(
            next,
            grants,
            [&mut *emissions, &mut *recycling],
            &mut *trail_deposits,
        ) {
            Ok(summaries) => {
                report.reductions = report.reductions.saturating_add(1);
                return Ok(summaries);
            }
            Err(error) => {
                report.reference_reductions = report.reference_reductions.saturating_add(1);
                let summaries = reduce_dual_carrier_field_on(
                    next,
                    grants,
                    [emissions, recycling],
                    trail_deposits,
                    workers,
                )?;
                report.failure = Some(error.to_string());
                *backend = None;
                return Ok(summaries);
            }
        }
    }
    reduce_dual_carrier_field_on(
        next,
        grants,
        [emissions, recycling],
        trail_deposits,
        workers,
    )
}

/// Phase 10's check of the next field (INV-01, INV-02 at `tolerance`), on
/// the world's field backend if it has one (FL-032), else on the field's
/// workers, followed by plan §11.5 bounded moment-only cone contraction on
/// any cell strictly exceeding `trigger`.
///
/// The INV-01/INV-02 check ALWAYS runs strictly read-only first so no
/// operator bug beyond `tolerance` can ever be masked, and `FieldSummary`
/// always reports the unmasked pre-contraction `max_cone_residual`. On CPU,
/// `any_exceeds_trigger` is tracked in the same pass at zero extra memory
/// reads so contraction is skipped completely on ticks where no cell exceeds
/// `trigger`.
fn check_field_with(
    next: &mut FieldState,
    tolerance: ConeTolerance,
    trigger: ConeTolerance,
    workers: usize,
    fields: &mut Fields,
) -> Result<(FieldSummary, ConeContractionSummary), CoreError> {
    let Fields { backend, report } = fields;
    if let Some(device) = backend.as_deref_mut() {
        let failure = match device
            .check(next, tolerance)
            .and_then(|scan| next.summarize_scan(tolerance, &scan))
        {
            Ok(Some(summary)) => {
                report.checks = report.checks.saturating_add(1);
                let contraction = if summary.max_cone_residual > 0.0 {
                    next.contract_cone_on(trigger, workers)?
                } else {
                    ConeContractionSummary::default()
                };
                return Ok((summary, contraction));
            }
            Ok(None) => match next.check_invariants_on(tolerance, workers) {
                Err(error) => {
                    report.checks = report.checks.saturating_add(1);
                    return Err(error);
                }
                Ok(_) => "the backend's check failed a cell the reference passes".to_owned(),
            },
            Err(error) => error.to_string(),
        };
        report.reference_checks = report.reference_checks.saturating_add(1);
        let (summary, any_exceeds_trigger) =
            next.check_invariants_with_trigger_on(tolerance, trigger, workers)?;
        report.failure = Some(failure);
        *backend = None;
        let contraction = if any_exceeds_trigger {
            next.contract_cone_on(trigger, workers)?
        } else {
            ConeContractionSummary::default()
        };
        return Ok((summary, contraction));
    }
    let (summary, any_exceeds_trigger) =
        next.check_invariants_with_trigger_on(tolerance, trigger, workers)?;
    let contraction = if any_exceeds_trigger {
        next.contract_cone_on(trigger, workers)?
    } else {
        ConeContractionSummary::default()
    };
    Ok((summary, contraction))
}

/// Phase 3 raw samples returned by a [`SensorBackend`], matching the field's
/// carrier layout (`33` single-carrier or `34` dual-carrier).
#[derive(Clone, Copy)]
enum Sampled<'a> {
    Single(&'a [RawSample]),
    Dual(&'a [DualRawSample]),
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
) -> (Option<Sampled<'a>>, Option<String>) {
    let Sensors {
        backend,
        report,
        queries,
        samples,
        dual_samples,
    } = sensors;
    let Some(device) = backend.as_deref_mut() else {
        return (None, None);
    };
    if field.is_dual_carrier() {
        match sample_dual_on(device, population, rows, view, field, queries, dual_samples) {
            Ok(()) => {
                report.ticks = report.ticks.saturating_add(1);
                (Some(Sampled::Dual(dual_samples.as_slice())), None)
            }
            Err(error) => {
                report.reference_ticks = report.reference_ticks.saturating_add(1);
                (None, Some(error.to_string()))
            }
        }
    } else {
        match sample_on(device, population, rows, view, field, queries, samples) {
            Ok(()) => {
                report.ticks = report.ticks.saturating_add(1);
                (Some(Sampled::Single(samples.as_slice())), None)
            }
            Err(error) => {
                report.reference_ticks = report.reference_ticks.saturating_add(1);
                (None, Some(error.to_string()))
            }
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

/// Every row's query, then the device's 34-plane dual-carrier samples, one per row.
fn sample_dual_on(
    device: &mut dyn SensorBackend,
    population: &Population,
    rows: &[Organism],
    view: &PyramidView<'_>,
    field: &FieldState,
    queries: &mut Vec<SensorQuery>,
    samples: &mut Vec<DualRawSample>,
) -> Result<(), CoreError> {
    queries.clear();
    reserve_for(queries, rows.len(), "sensor queries")?;
    for organism in rows {
        queries.push(SensorQuery {
            position: organism.position,
            scale: genome_of(population, organism.id)?.traits().sensing_scale(),
        });
    }
    device.sample_dual(view.pyramid(), field, queries, samples)?;
    if samples.len() != rows.len() {
        return Err(CoreError::StateConflict {
            operation: "dual-carrier sensor samples",
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
/// demands and grants are the reference's. The reference's demands are
/// computed on the lanes ([`demands_on_lanes`]).
fn uptake_with(
    rules: &Rules,
    working: &[Working],
    field: &FieldState,
    demands: &mut Vec<Demand>,
    grants: &mut Grants,
    uptake: &mut Uptake,
    lanes: &mut [DecisionLane],
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
        let served = if rules.exchange.is_grid_cic() {
            demands_on(device, field, &rule, requesters(), work, demands)
                .and_then(|()| resolve_uptake_on(device, field, demands, work, grants))
        } else {
            Err(CoreError::MissingCapability {
                capability: format!(
                    "an uptake backend under exchange kernel `{}`",
                    rules.exchange.name()
                ),
            })
        };
        match served {
            Ok(summary) => {
                report.ticks = report.ticks.saturating_add(1);
                return Ok(summary);
            }
            Err(error) => {
                report.reference_ticks = report.reference_ticks.saturating_add(1);
                demands_on_lanes(field, &rule, rules.exchange, working, demands, lanes)?;
                let summary = resolve_uptake_with_on_workers(
                    field,
                    rules.exchange,
                    demands,
                    grants,
                    lanes.len(),
                )?;
                report.failure = Some(error.to_string());
                *backend = None;
                return Ok(summary);
            }
        }
    }
    demands_on_lanes(field, &rule, rules.exchange, working, demands, lanes)?;
    resolve_uptake_with_on_workers(field, rules.exchange, demands, grants, lanes.len())
}

/// Phase 5's demands on the reference, on the lanes: `demands` gets each
/// working organism's identity and position in identity order, then each
/// lane computes a contiguous identity-ordered run of them ([`in_runs_mut`])
/// with [`demand_with_scratch`]. A demand reads only the observed field, so
/// the demands are the same at any lane count, and the error returned is the
/// first in identity order, as one thread would report it.
fn demands_on_lanes(
    field: &FieldState,
    rule: &RequestRule,
    exchange: ExchangeKernel,
    working: &[Working],
    demands: &mut Vec<Demand>,
    lanes: &mut [DecisionLane],
) -> Result<(), CoreError> {
    demands.clear();
    reserve_for(demands, working.len(), "organism demands")?;
    demands.extend(working.iter().map(|organism| Demand {
        organism: organism.committed.id,
        position: organism.position,
        amount: 0.0,
    }));
    in_runs_mut(demands, lanes, "uptake demands", &|_, run, lane| {
        for slot in run.iter_mut() {
            *slot = demand_with_scratch(
                field,
                rule,
                exchange,
                slot.organism,
                slot.position,
                &mut lane.exchange,
            )?;
        }
        Ok(())
    })
}

/// The watched organism's observation in the step from `from` to `to`,
/// exactly as its decision read it: from the backend's sample for its row
/// when the decisions used the backend's samples, else from the reference
/// pyramid. `None` when it is not among `rows`, which are in identity order.
type WatchedObservation = (
    Option<Sensed>,
    Option<[f32; MAXIMUM_RECURRENT_STATE_WIDTH]>,
    Option<[f64; 4]>,
    Option<f64>,
    Option<f32>,
);

#[expect(
    clippy::too_many_arguments,
    reason = "watched observation mirrors decide_with inputs plus the watched id"
)]
fn watched(
    rules: &Rules,
    population: &Population,
    view: &PyramidView<'_>,
    sampled: Option<Sampled<'_>>,
    rows: &[Organism],
    crowding_counts: &[u32],
    id: OrganismId,
    [from, to]: [Tick; 2],
) -> Result<WatchedObservation, CoreError> {
    let Some((row, organism)) = rows
        .binary_search_by_key(&id, |organism| organism.id)
        .ok()
        .and_then(|row| rows.get(row).map(|organism| (row, organism)))
    else {
        return Ok((None, None, None, None, None));
    };
    let scale = genome_of(population, id)?.traits().sensing_scale();
    let prev_state =
        (rules.recurrent_state_width > 0).then(|| organism.held.recurrent_state_array());
    let c_den = rules
        .v2
        .sensors()
        .hippo_crowding()
        .and_then(|_| crowding_counts.get(row).copied())
        .map(hippo_crowding_sensor);
    let notch_h0 = rules.v2.notch_delta().map(|_| organism.last_neighbor_h0);
    if rules.v2.dual_carrier().is_some() {
        let dual_raw = match sampled {
            Some(Sampled::Dual(samples)) => match samples.get(row) {
                Some(&sample) => sample,
                None => view.sample_dual(organism.position, scale)?,
            },
            _ => view.sample_dual(organism.position, scale)?,
        };
        let dual_obs = encode_dual_for_controller(rules, organism, &dual_raw, from)?;
        return Ok((
            Some(Sensed {
                tick: from,
                scale,
                raw: dual_to_raw_sample(&dual_raw),
                observation: dual_obs.base,
                evaluated: due(rules, organism, to),
            }),
            prev_state,
            Some(dual_obs.trail_inputs),
            c_den,
            notch_h0,
        ));
    }
    let single = match sampled {
        Some(Sampled::Single(samples)) => samples.get(row),
        _ => None,
    };
    let raw = blended(population, view, organism, single)?;
    Ok((
        Some(Sensed {
            tick: from,
            scale,
            // The controller's encoding (2482af0), so the record is the job's
            // inputs bit for bit, not only after their narrowing to F32.
            observation: encode_for_controller(rules, organism, &raw, from)?,
            raw,
            evaluated: due(rules, organism, to),
        }),
        prev_state,
        None,
        c_den,
        notch_h0,
    ))
}

/// Project a 34-plane [`DualRawSample`] into a 33-plane [`RawSample`] for the
/// watched inspector summary (`[R_0, M^(0)_{0..23}, M^(1)_{0..7}]`).
fn dual_to_raw_sample(dual: &DualRawSample) -> RawSample {
    let mut values = [0.0; RESPONSE_TERMS];
    let mut dx = [0.0; RESPONSE_TERMS];
    let mut dy = [0.0; RESPONSE_TERMS];
    let c0_planes = 1 + CARRIER0_MOMENT_CHANNELS as usize;
    let c1_first = CARRIER1_FIRST_MOMENT_PLANE as usize;
    let c1_count = CARRIER1_MOMENT_CHANNELS as usize;
    values[..c0_planes].copy_from_slice(&dual.values[..c0_planes]);
    dx[..c0_planes].copy_from_slice(&dual.dx[..c0_planes]);
    dy[..c0_planes].copy_from_slice(&dual.dy[..c0_planes]);
    values[c0_planes..c0_planes + c1_count]
        .copy_from_slice(&dual.values[c1_first..c1_first + c1_count]);
    dx[c0_planes..c0_planes + c1_count].copy_from_slice(&dual.dx[c1_first..c1_first + c1_count]);
    dy[c0_planes..c0_planes + c1_count].copy_from_slice(&dual.dy[c1_first..c1_first + c1_count]);
    RawSample { values, dx, dy }
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
/// committed state at `from`, as its controller reads it:
/// [`observe_for_controller`], so the 69 `tanh`-saturated inputs avoid
/// 13-division F64 `expm1_neg` while keeping every bit after the
/// controller's F32 input narrowing (and the F64 gradients identical).
fn encode_for_controller(
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
    observe_for_controller(raw, &intrinsic, &rules.scales)
}

/// Encode a 34-plane [`DualRawSample`] into a [`DualObservation`] (`V2-15`).
fn encode_dual_for_controller(
    rules: &Rules,
    organism: &Organism,
    raw: &DualRawSample,
    from: Tick,
) -> Result<DualObservation, CoreError> {
    let dual = rules
        .v2
        .dual_carrier()
        .ok_or_else(|| CoreError::StateConflict {
            operation: "encode dual-carrier sensors",
            detail: "dual-carrier encoding invoked without v2.carriers enabled".to_owned(),
        })?;
    let intrinsic = Intrinsic {
        energy: organism.energy,
        age_seconds: age_seconds(organism.birth_tick, from, rules.step)?,
        velocity: organism.velocity,
        energy_change_per_second: organism.energy_change_rate,
    };
    observe_dual_for_controller(
        raw,
        &intrinsic,
        &rules.scales,
        dual.trail_energy_density_scale(),
    )
}

/// Direct F32 sensor encoding for flat-F32 controller backends, returning exact
/// F64 motion gradients without an intermediate `[f64; 104]` buffer.
fn encode_for_controller_f32(
    rules: &Rules,
    organism: &Organism,
    raw: &RawSample,
    from: Tick,
    out_inputs_f32: &mut [f32; SENSOR_WIDTH],
) -> Result<[[f64; 2]; RESPONSE_TERMS], CoreError> {
    let intrinsic = Intrinsic {
        energy: organism.energy,
        age_seconds: age_seconds(organism.birth_tick, from, rules.step)?,
        velocity: organism.velocity,
        energy_change_per_second: organism.energy_change_rate,
    };
    observe_for_controller_f32(raw, &intrinsic, &rules.scales, out_inputs_f32)
}

/// Direct F32 dual-carrier sensor encoding (`V2-15`) for flat-F32 controller
/// backends, returning exact F64 motion gradients.
fn encode_dual_for_controller_f32(
    rules: &Rules,
    organism: &Organism,
    raw: &DualRawSample,
    from: Tick,
    out_inputs_f32: &mut [f32; SENSOR_WIDTH],
    out_trail_f32: &mut [f32; 4],
) -> Result<[[f64; 2]; RESPONSE_TERMS], CoreError> {
    let dual = rules
        .v2
        .dual_carrier()
        .ok_or_else(|| CoreError::StateConflict {
            operation: "encode dual-carrier sensors",
            detail: "dual-carrier encoding invoked without v2.carriers enabled".to_owned(),
        })?;
    let intrinsic = Intrinsic {
        energy: organism.energy,
        age_seconds: age_seconds(organism.birth_tick, from, rules.step)?,
        velocity: organism.velocity,
        energy_change_per_second: organism.energy_change_rate,
    };
    observe_dual_for_controller_f32(
        raw,
        &intrinsic,
        &rules.scales,
        dual.trail_energy_density_scale(),
        out_inputs_f32,
        out_trail_f32,
    )
}

/// Whether the organism evaluates its controller for tick `to`: due on its
/// phase, or forced because it holds no valid output.
fn due(rules: &Rules, organism: &Organism, to: Tick) -> bool {
    !organism.held.is_valid() || to.get() % u64::from(rules.interval) == u64::from(organism.phase)
}

/// The raw outputs a held output decodes from: the controller's, widened,
/// or zeroed under the chemistry-only variants.
fn raw_outputs<'a>(
    rules: &Rules,
    outputs: &[f32],
    buffer: &'a mut [f64; MAX_ACTION_WIDTH],
) -> Result<&'a [f64], CoreError> {
    let len = outputs.len();
    let dst = buffer
        .get_mut(..len)
        .ok_or_else(|| CoreError::LayoutMismatch {
            buffer: "held controller output",
            detail: format!("output width {len} exceeds maximum {MAX_ACTION_WIDTH}"),
        })?;
    if rules.variant.decides_from_zero() {
        dst.fill(0.0);
    } else {
        widen_slice(outputs, dst)?;
    }
    Ok(dst)
}

/// Decode `held` into [`DecodedActions`] for this tick.
fn decode_held(
    rules: &Rules,
    organism: &Organism,
    held: &HeldOutput,
) -> Result<DecodedActions, CoreError> {
    let outputs = held.outputs().ok_or_else(|| CoreError::StateConflict {
        operation: "decode held output",
        detail: format!("{} holds no valid output", organism.id),
    })?;
    let mut raw = [0.0_f64; MAX_ACTION_WIDTH];
    let slice = raw_outputs(rules, outputs, &mut raw)?;
    decode_with_layout(slice, rules.step, &rules.decoding, &rules.v2.actions())
}

/// The rest of phase 4 from `gradients`: the held output, newly evaluated or
/// not, decoded.
fn finish_from_gradients(
    rules: &Rules,
    organism: &Organism,
    held: HeldOutput,
    evaluated: bool,
    gradients: [[f64; 2]; RESPONSE_TERMS],
) -> Result<Decision, CoreError> {
    Ok(Decision {
        held,
        evaluated,
        actions: decode_held(rules, organism, &held)?,
        gradients,
    })
}

/// The rest of phase 4: the held output, newly evaluated or not, decoded.
fn finish(
    rules: &Rules,
    organism: &Organism,
    held: HeldOutput,
    evaluated: bool,
    observation: &Observation,
) -> Result<Decision, CoreError> {
    finish_from_gradients(rules, organism, held, evaluated, observation.gradients)
}

/// Populate optional v2 non-recurrent sensor slots (`dual-carrier-trails`,
/// `hippo-crowding` `c_den`, `notch-delta` `h̄_0`) into `expanded` beyond the
/// 104 baseline sensor entries.
fn fill_v2_extra_sensors(
    sensors: SensorLayout,
    trail_inputs: Option<[f64; 4]>,
    local_crowding: Option<u32>,
    notch_delta_h0: Option<f32>,
    expanded: &mut [f64; MAX_SENSOR_WIDTH],
) {
    if let (Some(start), Some(trail)) = (sensors.dual_carrier_trails(), trail_inputs) {
        expanded[start..start + trail.len()].copy_from_slice(&trail);
    }
    if let (Some(slot), Some(n_loc)) = (sensors.hippo_crowding(), local_crowding) {
        expanded[slot] = hippo_crowding_sensor(n_loc);
    }
    if let (Some(slot), Some(h_bar)) = (sensors.notch_delta(), notch_delta_h0) {
        expanded[slot] = f64::from(h_bar);
    }
}

/// Apply `V2-12` (`notch-delta`, `direct-lateral-inhibition-v1`) directly onto
/// post-Phase-4 `next_state[0]` right after `update_recurrent_state`:
/// `h''_0 = clamp(h'_0 - η_ND * last_neighbor_h0, -1, 1)`.
fn apply_notch_delta_inhibition(v2: &V2Rules, last_neighbor_h0: f32, next_state: &mut [f32]) {
    if let Some(nd) = v2.notch_delta()
        && nd.mode == NotchDeltaMode::DirectLateralInhibition
        && nd.inhibition_gain > 0.0
        && let Some(h0) = next_state.first_mut()
    {
        *h0 = (*h0 - (nd.inhibition_gain as f32) * last_neighbor_h0).clamp(-1.0_f32, 1.0_f32);
    }
}

/// Evaluate one due organism's controller on the scalar reference, handling
/// dynamic sensor/action layouts and optional gated recurrent state.
#[expect(
    clippy::too_many_arguments,
    reason = "reference controller evaluation inputs including optional v2 trail and crowding sensors"
)]
fn evaluate_reference_held(
    rules: &Rules,
    genome: &Genome,
    organism: &Organism,
    observation: &Observation,
    trail_inputs: Option<[f64; 4]>,
    local_crowding: Option<u32>,
    to: Tick,
    controller: &mut ControllerScratch,
) -> Result<HeldOutput, CoreError> {
    let eval = genome.controller(&rules.catalog, &rules.limits)?;
    let sensors = rules.v2.sensors();
    let obs_width = sensors.observation_width();
    let state_width = rules.recurrent_state_width;
    let mut expanded_obs = [0.0_f64; MAX_SENSOR_WIDTH];
    let sensor_slice: &[f64] = if obs_width == SENSOR_WIDTH {
        &observation.inputs
    } else {
        expanded_obs[..SENSOR_WIDTH].copy_from_slice(&observation.inputs);
        fill_v2_extra_sensors(
            sensors,
            trail_inputs,
            local_crowding,
            Some(organism.last_neighbor_h0),
            &mut expanded_obs,
        );
        &expanded_obs[..obs_width]
    };
    if state_width > 0 {
        let prev_state = organism.held.recurrent_state_array();
        let mut next_state = [0.0_f32; MAXIMUM_RECURRENT_STATE_WIDTH];
        let raw = eval.evaluate_recurrent_slice(
            sensor_slice,
            &prev_state[..state_width],
            rules.v2.epigenetic_gate_bias(),
            controller,
            &mut next_state[..state_width],
        )?;
        apply_notch_delta_inhibition(
            &rules.v2,
            organism.last_neighbor_h0,
            &mut next_state[..state_width],
        );
        HeldOutput::evaluated_recurrent_slice(to, raw.as_slice(), &next_state[..state_width])
    } else {
        let raw = eval.evaluate_raw(sensor_slice, controller)?;
        HeldOutput::evaluated_slice(to, raw.as_slice())
    }
}

/// Evaluate one deferred flat-F32 controller job on the scalar reference,
/// widening its already-narrowed F32 sensor slice without repeating a
/// `genome_of` lookup.
fn evaluate_reference_held_from_f32(
    rules: &Rules,
    eval: ReferenceController<'_>,
    organism: &Organism,
    inputs_f32: &[f32],
    to: Tick,
    controller: &mut ControllerScratch,
) -> Result<HeldOutput, CoreError> {
    let sensors = rules.v2.sensors();
    let obs_width = sensors.observation_width();
    let state_width = rules.recurrent_state_width;
    let mut wide_obs = [0.0_f64; MAX_SENSOR_WIDTH];
    for (dst, &src) in wide_obs[..obs_width].iter_mut().zip(inputs_f32) {
        *dst = f64::from(src);
    }
    if state_width > 0 {
        let prev_state = organism.held.recurrent_state_array();
        let mut next_state = [0.0_f32; MAXIMUM_RECURRENT_STATE_WIDTH];
        let raw = eval.evaluate_recurrent_slice(
            &wide_obs[..obs_width],
            &prev_state[..state_width],
            rules.v2.epigenetic_gate_bias(),
            controller,
            &mut next_state[..state_width],
        )?;
        apply_notch_delta_inhibition(
            &rules.v2,
            organism.last_neighbor_h0,
            &mut next_state[..state_width],
        );
        HeldOutput::evaluated_recurrent_slice(to, raw.as_slice(), &next_state[..state_width])
    } else {
        let raw = eval.evaluate_raw(&wide_obs[..obs_width], controller)?;
        HeldOutput::evaluated_slice(to, raw.as_slice())
    }
}

/// Build `HeldOutput` from one backend's raw F32 slice, applying the canonical
/// host `update_recurrent_state` when `state_width > 0`.
fn held_from_raw_slice(
    rules: &Rules,
    organism: &Organism,
    to: Tick,
    raw: &[f32],
) -> Result<HeldOutput, CoreError> {
    let actions = rules.v2.actions();
    let action_width = actions.action_width();
    let state_width = rules.recurrent_state_width;
    let action_slice = raw
        .get(..action_width)
        .ok_or_else(|| CoreError::LayoutMismatch {
            buffer: "controller outputs",
            detail: format!(
                "backend returned {} outputs, expected at least {action_width}",
                raw.len()
            ),
        })?;
    if state_width > 0 {
        let tail = raw
            .get(action_width..)
            .ok_or_else(|| CoreError::LayoutMismatch {
                buffer: "controller outputs",
                detail: "backend output missed recurrent tail".to_owned(),
            })?;
        let prev_state = organism.held.recurrent_state_array();
        let mut next_state = [0.0_f32; MAXIMUM_RECURRENT_STATE_WIDTH];
        update_recurrent_state(
            &prev_state[..state_width],
            tail,
            rules.v2.epigenetic_gate_bias(),
            &mut next_state[..state_width],
        )?;
        apply_notch_delta_inhibition(
            &rules.v2,
            organism.last_neighbor_h0,
            &mut next_state[..state_width],
        );
        HeldOutput::evaluated_recurrent_slice(to, action_slice, &next_state[..state_width])
    } else {
        HeldOutput::evaluated_slice(to, action_slice)
    }
}

/// Build `HeldOutput` from one backend's full `RawOutputs`, applying the
/// canonical host `update_recurrent_state` when `state_width > 0`.
fn held_from_raw(
    rules: &Rules,
    organism: &Organism,
    to: Tick,
    raw: &RawOutputs,
) -> Result<HeldOutput, CoreError> {
    held_from_raw_slice(rules, organism, to, raw.as_slice())
}

/// Phases 3 and 4 for one organism (§8.1): its already-sampled raw observation,
/// encoded for the controller, then its due or forced evaluation, decoded.
fn decide_from_raw(
    rules: &Rules,
    genome: &Genome,
    organism: &Organism,
    raw: &RawSample,
    local_crowding: Option<u32>,
    [from, to]: [Tick; 2],
    controller: &mut ControllerScratch,
) -> Result<Decision, CoreError> {
    let observation = encode_for_controller(rules, organism, raw, from)?;
    let due = due(rules, organism, to);
    let held = if due {
        evaluate_reference_held(
            rules,
            genome,
            organism,
            &observation,
            None,
            local_crowding,
            to,
            controller,
        )?
    } else {
        organism.held
    };
    finish(rules, organism, held, due, &observation)
}

/// Phases 3 and 4 for one organism on a 34-plane dual-carrier field (`V2-15`).
fn decide_from_dual_raw(
    rules: &Rules,
    genome: &Genome,
    organism: &Organism,
    raw: &DualRawSample,
    local_crowding: Option<u32>,
    [from, to]: [Tick; 2],
    controller: &mut ControllerScratch,
) -> Result<Decision, CoreError> {
    let dual_obs = encode_dual_for_controller(rules, organism, raw, from)?;
    let due = due(rules, organism, to);
    let held = if due {
        evaluate_reference_held(
            rules,
            genome,
            organism,
            &dual_obs.base,
            Some(dual_obs.trail_inputs),
            local_crowding,
            to,
            controller,
        )?
    } else {
        organism.held
    };
    finish(rules, organism, held, due, &dual_obs.base)
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
    let worker_budget = lanes.len();
    let used = worker_budget
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
    let tail_count = runs.len();
    let mut tail_results: Vec<Result<(), CoreError>> = (0..tail_count).map(|_| Ok(())).collect();
    let mut head_result = Ok(());
    flife_core::pool::scope_workers(worker_budget, "decide", |scope| {
        for (index, ((run, lane), slot)) in runs
            .zip(others.iter_mut())
            .zip(tail_results.iter_mut())
            .enumerate()
        {
            scope.spawn(move || {
                *slot = work((index + 1) * length, run, lane);
            });
        }
        head_result = work(0, head, first);
    })?;
    head_result?;
    for result in tail_results {
        result?;
    }
    Ok(())
}

/// [`in_runs`] over rows that `work` changes: the same split into
/// contiguous identity-ordered runs, one per lane, each with the index of
/// its first row, and the first error in identity order. `operation` names
/// the phase in its own refusals.
fn in_runs_mut<T, F>(
    rows: &mut [T],
    lanes: &mut [DecisionLane],
    operation: &'static str,
    work: &F,
) -> Result<(), CoreError>
where
    T: Send,
    F: Fn(usize, &mut [T], &mut DecisionLane) -> Result<(), CoreError> + Sync,
{
    let worker_budget = lanes.len();
    let used = worker_budget
        .min(rows.len().div_ceil(MINIMUM_ROWS_PER_LANE))
        .max(1);
    let Some((first, others)) = lanes.split_first_mut() else {
        return Err(CoreError::StateConflict {
            operation,
            detail: "no lane".to_owned(),
        });
    };
    if used == 1 {
        return work(0, rows, first);
    }
    let length = rows.len().div_ceil(used);
    let mut runs = rows.chunks_mut(length);
    let head = runs.next().unwrap_or_default();
    let tail_count = runs.len();
    let mut tail_results: Vec<Result<(), CoreError>> = (0..tail_count).map(|_| Ok(())).collect();
    let mut head_result = Ok(());
    flife_core::pool::scope_workers(worker_budget, operation, |scope| {
        for (index, ((run, lane), slot)) in runs
            .zip(others.iter_mut())
            .zip(tail_results.iter_mut())
            .enumerate()
        {
            scope.spawn(move || {
                *slot = work((index + 1) * length, run, lane);
            });
        }
        head_result = work(0, head, first);
    })?;
    head_result?;
    for result in tail_results {
        result?;
    }
    Ok(())
}

/// Phases 6 and 7 for every working organism, on the lanes: each lane
/// settles a contiguous identity-ordered run ([`in_runs_mut`]) and keeps,
/// in identity order, the run's ledger flows, its staged emissions and the
/// watched organism's transactions, for the tick to take in lane order.
/// `grants` are the grant packets grouped by demand, the packets and their
/// compositions ([`compositions_into`]). The error returned is the first in
/// identity order, as a single thread would report it.
fn settle_all(
    rules: &Rules,
    population: &Population,
    grants: (&Bins, &[GrantPacket], &[[f64; CHANNELS]]),
    from: Tick,
    watched: Option<OrganismId>,
    working: &mut [Working],
    lanes: &mut [DecisionLane],
) -> Result<(), CoreError> {
    let (by_demand, granted, compositions) = grants;
    for lane in lanes.iter_mut() {
        lane.flows.clear();
        lane.emissions.clear();
        if let Some(trail) = lane.trail_emissions.as_mut() {
            trail.clear();
        }
        if let Some(byproducts) = lane.byproducts.as_mut() {
            byproducts.clear();
        }
        lane.emission_paid = CompensatedSum::ZERO;
        lane.transactions = None;
    }
    in_runs_mut(working, lanes, "settle", &|start, run, lane| {
        let DecisionLane {
            exchange,
            packets,
            flows,
            emissions,
            trail_emissions,
            byproducts,
            emission_paid,
            transactions,
            ..
        } = lane;
        let mut live = run
            .first()
            .map(|first| population.ascending_from(first.committed.id));
        for (offset, organism) in run.iter_mut().enumerate() {
            packets.clear();
            for (packet, composition) in by_demand.bucket(start + offset).iter().filter_map(|&at| {
                let at = at as usize;
                granted.get(at).zip(compositions.get(at))
            }) {
                packets.push(ChemicalPacket {
                    granted: packet.granted,
                    composition: *composition,
                });
            }
            let genome = live
                .as_mut()
                .and_then(|walk| walk.next(organism.committed.id).0)
                .ok_or_else(|| CoreError::StateConflict {
                    operation: "read genome",
                    detail: format!("{} has no genome", organism.committed.id),
                })?;
            let traits = genome.traits();
            let mut stages = Reference {
                traits,
                parameters: parameter_count(&rules.catalog, genome)?,
                packets,
            };
            let record = (watched == Some(organism.committed.id)).then_some(&mut *transactions);
            settle_stages(
                rules,
                organism,
                traits,
                &mut stages,
                from,
                flows,
                emissions,
                trail_emissions.as_mut(),
                byproducts.as_mut(),
                emission_paid,
                exchange,
                record,
            )?
            .apply(organism);
        }
        Ok(())
    })
}

/// The fewest packets a lane takes in [`compositions_into`].
const MINIMUM_PACKETS_PER_LANE: usize = 1_024;

/// Phase 6's compositions: `out[at]` becomes [`composition`] of
/// `granted[at]`'s cell in `observed`, on at most `lanes` threads (this one
/// and scoped workers joined before returning), each taking a contiguous
/// run of packets. A composition is a function of the observed field and
/// the cell alone, so the split changes no value. The packets are in cell
/// order, so within a run a packet whose cell is the previous packet's
/// copies its composition. `out` keeps the length of the most packets a tick
/// has had, within the room reserved for four a row, so a tick writes only
/// the compositions it reads. The error returned is the first in packet
/// order.
fn compositions_into(
    observed: &FieldState,
    granted: &[GrantPacket],
    lanes: usize,
    out: &mut Vec<[f64; CHANNELS]>,
) -> Result<(), CoreError> {
    if out.len() < granted.len() {
        reserve_for(out, granted.len() - out.len(), "packet compositions")?;
        out.resize(granted.len(), [0.0; CHANNELS]);
    }
    let out = out
        .get_mut(..granted.len())
        .ok_or_else(|| CoreError::StateConflict {
            operation: "compose packets",
            detail: format!("no room for {} compositions", granted.len()),
        })?;
    let energy_plane = observed.energy();
    let total_cells = energy_plane.len();
    let all_data = observed.as_slice();
    let planes = observed.planes() as usize;
    let active_channels = observed.carrier0_moments_count();
    let planar_ok = all_data.len() == planes.saturating_mul(total_cells);
    let run = |packets: &[GrantPacket], slots: &mut [[f64; CHANNELS]]| -> Result<(), CoreError> {
        if !planar_ok {
            let mut previous: Option<(usize, [f64; CHANNELS])> = None;
            for (packet, slot) in packets.iter().zip(slots) {
                let value = match previous {
                    Some((cell, value)) if cell == packet.cell => value,
                    _ => composition(observed, packet.cell)?,
                };
                *slot = value;
                previous = Some((packet.cell, value));
            }
            return Ok(());
        }
        // Batch up to 32 distinct nonempty cells and read moment planes
        // plane-by-plane so 512² (1 MiB plane stride) avoids 33-way L1D
        // cache-set conflict misses while computing the exact same bits.
        const COMP_BATCH: usize = 32;
        let mut cells = [0_usize; COMP_BATCH];
        let mut energies = [0.0_f64; COMP_BATCH];
        let mut ranges = [(0_usize, 0_usize); COMP_BATCH];
        let mut batch = [[0.0_f64; CHANNELS]; COMP_BATCH];
        let mut i = 0_usize;
        let n = packets.len();
        while i < n {
            let mut count = 0_usize;
            while i < n && count < COMP_BATCH {
                let cell = packets[i].cell;
                let &e_raw = energy_plane
                    .get(cell)
                    .ok_or_else(|| CoreError::LayoutMismatch {
                        buffer: "field",
                        detail: format!("cell {cell} is not in this field"),
                    })?;
                let start = i;
                i += 1;
                while i < n && packets[i].cell == cell {
                    i += 1;
                }
                let e = f64::from(e_raw);
                if e > 0.0 {
                    cells[count] = cell;
                    energies[count] = e;
                    ranges[count] = (start, i);
                    count += 1;
                } else {
                    for slot in &mut slots[start..i] {
                        *slot = [0.0; CHANNELS];
                    }
                }
            }
            if count > 0 {
                for (channel, plane) in all_data[total_cells..]
                    .chunks_exact(total_cells)
                    .enumerate()
                    .take(active_channels)
                {
                    for b in 0..count {
                        batch[b][channel] = f64::from(plane[cells[b]]) / energies[b];
                    }
                }
                for b in 0..count {
                    let (start, end) = ranges[b];
                    let z = batch[b];
                    for slot in &mut slots[start..end] {
                        *slot = z;
                    }
                }
            }
        }
        Ok(())
    };
    let used = lanes
        .min(granted.len().div_ceil(MINIMUM_PACKETS_PER_LANE))
        .max(1);
    if used == 1 {
        return run(granted, out);
    }
    let length = granted.len().div_ceil(used);
    let mut runs = granted.chunks(length).zip(out.chunks_mut(length));
    let Some((head, head_slots)) = runs.next() else {
        return Ok(());
    };
    let run = &run;
    let tail_count = used.saturating_sub(1);
    let mut tail_results: Vec<Result<(), CoreError>> = (0..tail_count).map(|_| Ok(())).collect();
    let mut head_result = Ok(());
    flife_core::pool::scope_workers(lanes, "compose packets", |scope| {
        for ((packets, slots), result_slot) in runs.zip(tail_results.iter_mut()) {
            scope.spawn(move || {
                *result_slot = run(packets, slots);
            });
        }
        head_result = run(head, head_slots);
    })?;
    head_result?;
    for result in tail_results {
        result?;
    }
    Ok(())
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
#[expect(
    clippy::too_many_arguments,
    reason = "phases 3 and 4 reference runner inputs including post-Phase-1 crowding"
)]
fn decide_all(
    rules: &Rules,
    population: &Population,
    view: &PyramidView<'_>,
    sampled: Option<Sampled<'_>>,
    rows: &[Organism],
    crowding_counts: &[u32],
    ticks: [Tick; 2],
    lanes: &mut [DecisionLane],
) -> Result<(), CoreError> {
    let dual_carrier = rules.v2.dual_carrier().is_some();
    for lane in lanes.iter_mut() {
        lane.decisions.clear();
    }
    in_runs(rows, lanes, &|start, run, lane| {
        let DecisionLane {
            controller,
            sample_queries,
            raw_samples,
            dual_raw_samples,
            decisions,
            ..
        } = lane;
        reserve_for(decisions, run.len(), "decisions")?;
        let mut live = run.first().map(|first| population.ascending_from(first.id));
        if dual_carrier {
            if let Some(Sampled::Dual(samples)) = sampled {
                for (index, organism) in run.iter().enumerate() {
                    let genome = live
                        .as_mut()
                        .and_then(|walk| walk.next(organism.id).0)
                        .ok_or_else(|| CoreError::StateConflict {
                            operation: "read genome",
                            detail: format!("{} has no genome", organism.id),
                        })?;
                    let raw = match samples.get(start + index) {
                        Some(raw) => *raw,
                        None => {
                            view.sample_dual(organism.position, genome.traits().sensing_scale())?
                        }
                    };
                    let decision = decide_from_dual_raw(
                        rules,
                        genome,
                        organism,
                        &raw,
                        crowding_counts.get(start + index).copied(),
                        ticks,
                        controller,
                    )?;
                    decisions.push(decision);
                }
                return Ok(());
            }
            let batch_cap = SAMPLE_BATCH
                .min(sample_queries.len())
                .min(dual_raw_samples.len())
                .max(1);
            let mut batch_genomes: [Option<&Genome>; SAMPLE_BATCH] = [None; SAMPLE_BATCH];
            for (chunk_idx, chunk) in run.chunks(batch_cap).enumerate() {
                let count = chunk.len();
                let base = start + chunk_idx * batch_cap;
                for i in 0..count {
                    let organism = &chunk[i];
                    let genome = live
                        .as_mut()
                        .and_then(|walk| walk.next(organism.id).0)
                        .ok_or_else(|| CoreError::StateConflict {
                            operation: "read genome",
                            detail: format!("{} has no genome", organism.id),
                        })?;
                    batch_genomes[i] = Some(genome);
                    sample_queries[i] = SensorQuery {
                        position: organism.position,
                        scale: genome.traits().sensing_scale(),
                    };
                }
                view.sample_dual_batch(&sample_queries[..count], &mut dual_raw_samples[..count])?;
                for i in 0..count {
                    let Some(genome) = batch_genomes[i] else {
                        continue;
                    };
                    let decision = decide_from_dual_raw(
                        rules,
                        genome,
                        &chunk[i],
                        &dual_raw_samples[i],
                        crowding_counts.get(base + i).copied(),
                        ticks,
                        controller,
                    )?;
                    decisions.push(decision);
                }
            }
            return Ok(());
        }
        if let Some(Sampled::Single(samples)) = sampled {
            for (index, organism) in run.iter().enumerate() {
                let genome = live
                    .as_mut()
                    .and_then(|walk| walk.next(organism.id).0)
                    .ok_or_else(|| CoreError::StateConflict {
                        operation: "read genome",
                        detail: format!("{} has no genome", organism.id),
                    })?;
                let raw = match samples.get(start + index) {
                    Some(raw) => *raw,
                    None => view.sample(organism.position, genome.traits().sensing_scale())?,
                };
                let decision = decide_from_raw(
                    rules,
                    genome,
                    organism,
                    &raw,
                    crowding_counts.get(start + index).copied(),
                    ticks,
                    controller,
                )?;
                decisions.push(decision);
            }
            return Ok(());
        }
        let batch_cap = SAMPLE_BATCH
            .min(sample_queries.len())
            .min(raw_samples.len())
            .max(1);
        let mut batch_genomes: [Option<&Genome>; SAMPLE_BATCH] = [None; SAMPLE_BATCH];
        for (chunk_idx, chunk) in run.chunks(batch_cap).enumerate() {
            let count = chunk.len();
            let base = start + chunk_idx * batch_cap;
            for i in 0..count {
                let organism = &chunk[i];
                let genome = live
                    .as_mut()
                    .and_then(|walk| walk.next(organism.id).0)
                    .ok_or_else(|| CoreError::StateConflict {
                        operation: "read genome",
                        detail: format!("{} has no genome", organism.id),
                    })?;
                batch_genomes[i] = Some(genome);
                sample_queries[i] = SensorQuery {
                    position: organism.position,
                    scale: genome.traits().sensing_scale(),
                };
            }
            view.sample_batch(&sample_queries[..count], &mut raw_samples[..count])?;
            for i in 0..count {
                let Some(genome) = batch_genomes[i] else {
                    continue;
                };
                let decision = decide_from_raw(
                    rules,
                    genome,
                    &chunk[i],
                    &raw_samples[i],
                    crowding_counts.get(base + i).copied(),
                    ticks,
                    controller,
                )?;
                decisions.push(decision);
            }
        }
        Ok(())
    })
}

/// How the batched path ended.
enum Batched {
    /// Every decision is made: `batched` evaluations came from the backend
    /// and `deferred` from the reference, and the backend's device spent
    /// `device_nanoseconds` on them (zero when nothing was due).
    Done {
        batched: u64,
        deferred: u64,
        device_nanoseconds: u64,
    },
    /// The backend failed.
    Device(CoreError),
}

/// Phases 3 and 4 for every row, with the due evaluations batched on the
/// world's backend if it has one. Any error on the batched path hands the
/// tick to [`decide_all`], which reproduces the reference's own error (or
/// finds none), so the result is always the reference's.
#[expect(
    clippy::too_many_arguments,
    reason = "phases 3 and 4's inputs, the backend's samples, post-Phase-1 crowding, and the controllers"
)]
fn decide_with(
    rules: &Rules,
    population: &Population,
    view: &PyramidView<'_>,
    sampled: Option<Sampled<'_>>,
    rows: &[Organism],
    crowding_counts: &[u32],
    ticks: [Tick; 2],
    lanes: &mut [DecisionLane],
    controllers: &mut Controllers,
) -> Result<(), CoreError> {
    let Controllers {
        backend,
        report,
        outputs,
        flat_inputs_f32,
        flat_inputs_finite,
        flat_outputs,
    } = controllers;
    let Some(device) = backend.as_deref_mut() else {
        return decide_all(
            rules,
            population,
            view,
            sampled,
            rows,
            crowding_counts,
            ticks,
            lanes,
        );
    };
    let decided = if device.supports_flat_f32() {
        decide_batched_flat_f32(
            rules,
            population,
            view,
            sampled,
            rows,
            crowding_counts,
            ticks,
            lanes,
            device,
            flat_inputs_f32,
            flat_inputs_finite,
            flat_outputs,
        )
    } else {
        decide_batched(
            rules,
            population,
            view,
            sampled,
            rows,
            crowding_counts,
            ticks,
            lanes,
            device,
            outputs,
        )
    };
    report.resident_peak_bytes = report.resident_peak_bytes.max(device.resident_bytes());
    match decided {
        Ok(Batched::Done {
            batched,
            deferred,
            device_nanoseconds,
        }) => {
            report.batched = report.batched.saturating_add(batched);
            report.deferred = report.deferred.saturating_add(deferred);
            report.device_nanoseconds =
                report.device_nanoseconds.saturating_add(device_nanoseconds);
            Ok(())
        }
        Ok(Batched::Device(error)) => {
            report.failure = Some(error.to_string());
            *backend = None;
            report.reference_ticks = report.reference_ticks.saturating_add(1);
            decide_all(
                rules,
                population,
                view,
                sampled,
                rows,
                crowding_counts,
                ticks,
                lanes,
            )
        }
        Err(_) => {
            report.reference_ticks = report.reference_ticks.saturating_add(1);
            decide_all(
                rules,
                population,
                view,
                sampled,
                rows,
                crowding_counts,
                ticks,
                lanes,
            )
        }
    }
}

/// One lane's Pass-1 work on the flat-F32 controller path: sample 256-query
/// spatially-grouped batches, encode directly to F32 into the lane's disjoint
/// slice of `in_f32`, build `FlatControllerJob`s in the same ascending walk,
/// and immediately finish non-due organisms' `Decision`s.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "single-pass lane worker for spatially-grouped sampling, direct F32 sensor encoding, and job construction"
)]
fn pass1_flat_f32<'p>(
    rules: &'p Rules,
    population: &'p Population,
    view: &PyramidView<'_>,
    sampled: Option<Sampled<'_>>,
    crowding_counts: &[u32],
    [from, to]: [Tick; 2],
    start: usize,
    run: &[Organism],
    lane: &mut DecisionLane,
    in_f32: &mut [f32],
    in_finite: &mut [u8],
    jobs_out: &mut Vec<FlatControllerJob<'p>>,
) -> Result<(), CoreError> {
    let sensors = rules.v2.sensors();
    let input_width = sensors.input_width();
    let obs_width = sensors.observation_width();
    let state_width = rules.recurrent_state_width;
    let needs_expanded = input_width > SENSOR_WIDTH;
    let dual_carrier = rules.v2.dual_carrier().is_some();

    let DecisionLane {
        sample_queries,
        raw_samples,
        dual_raw_samples,
        decisions,
        ..
    } = lane;
    reserve_for(decisions, run.len(), "decisions")?;
    let mut live = run.first().map(|first| population.ascending_from(first.id));
    let mut local_due = 0_usize;

    let mut emit_one = |row_idx: usize,
                        organism: &Organism,
                        genome: &'p Genome,
                        slot: Option<ArenaSlot>,
                        gradients: [[f64; 2]; RESPONSE_TERMS],
                        base_f32: &[f32; SENSOR_WIDTH],
                        trail_f32: Option<[f32; 4]>|
     -> Result<(), CoreError> {
        if due(rules, organism, to) {
            let base_off = local_due * input_width;
            let dst = in_f32
                .get_mut(base_off..base_off + input_width)
                .ok_or_else(|| CoreError::StateConflict {
                    operation: "controller batch",
                    detail: "flat input buffer shorter than due count".to_owned(),
                })?;
            dst[..SENSOR_WIDTH].copy_from_slice(base_f32);
            if needs_expanded {
                if let (Some(slot_idx), Some(trail)) = (sensors.dual_carrier_trails(), trail_f32) {
                    dst[slot_idx..slot_idx + trail.len()].copy_from_slice(&trail);
                }
                if let (Some(slot_idx), Some(n_loc)) = (
                    sensors.hippo_crowding(),
                    crowding_counts.get(row_idx).copied(),
                ) {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "matches ReferenceController F32 input narrowing"
                    )]
                    {
                        dst[slot_idx] = hippo_crowding_sensor(n_loc) as f32;
                    }
                }
                if let Some(slot_idx) = sensors.notch_delta() {
                    dst[slot_idx] = organism.last_neighbor_h0;
                }
                if state_width > 0 {
                    let prev = organism.held.recurrent_state_array();
                    dst[obs_width..input_width].copy_from_slice(&prev[..state_width]);
                }
            }
            let finite_slot =
                in_finite
                    .get_mut(local_due)
                    .ok_or_else(|| CoreError::StateConflict {
                        operation: "controller batch",
                        detail: "flat finite flag buffer shorter than due count".to_owned(),
                    })?;
            *finite_slot = u8::from(dst.iter().all(|v| v.is_finite()));
            let arena_slot = slot.ok_or_else(|| CoreError::StateConflict {
                operation: "controller batch",
                detail: format!("{} holds no arena slot", organism.id),
            })?;
            jobs_out.push(FlatControllerJob {
                genome: organism.genome,
                slot: arena_slot,
                controller: genome.controller(&rules.catalog, &rules.limits)?,
            });
            local_due += 1;
            decisions.push(Decision {
                held: organism.held,
                evaluated: true,
                actions: DecodedActions::default(),
                gradients,
            });
        } else {
            decisions.push(finish_from_gradients(
                rules,
                organism,
                organism.held,
                false,
                gradients,
            )?);
        }
        Ok(())
    };

    let mut base_f32 = [0.0_f32; SENSOR_WIDTH];
    let mut trail_f32 = [0.0_f32; 4];

    if dual_carrier {
        if let Some(Sampled::Dual(samples)) = sampled {
            for (index, organism) in run.iter().enumerate() {
                let (genome, slot) = live
                    .as_mut()
                    .map_or((None, None), |walk| walk.next(organism.id));
                let genome = genome.ok_or_else(|| CoreError::StateConflict {
                    operation: "read genome",
                    detail: format!("{} has no genome", organism.id),
                })?;
                let raw = match samples.get(start + index) {
                    Some(raw) => *raw,
                    None => view.sample_dual(organism.position, genome.traits().sensing_scale())?,
                };
                let gradients = encode_dual_for_controller_f32(
                    rules,
                    organism,
                    &raw,
                    from,
                    &mut base_f32,
                    &mut trail_f32,
                )?;
                emit_one(
                    start + index,
                    organism,
                    genome,
                    slot,
                    gradients,
                    &base_f32,
                    Some(trail_f32),
                )?;
            }
            return Ok(());
        }
        let batch_cap = SAMPLE_BATCH
            .min(sample_queries.len())
            .min(dual_raw_samples.len())
            .max(1);
        let mut batch_meta: [(Option<&'p Genome>, Option<ArenaSlot>); SAMPLE_BATCH] =
            [(None, None); SAMPLE_BATCH];
        for (chunk_idx, chunk) in run.chunks(batch_cap).enumerate() {
            let count = chunk.len();
            let base = start + chunk_idx * batch_cap;
            for i in 0..count {
                let organism = &chunk[i];
                let (genome, slot) = live
                    .as_mut()
                    .map_or((None, None), |walk| walk.next(organism.id));
                let genome = genome.ok_or_else(|| CoreError::StateConflict {
                    operation: "read genome",
                    detail: format!("{} has no genome", organism.id),
                })?;
                batch_meta[i] = (Some(genome), slot);
                sample_queries[i] = SensorQuery {
                    position: organism.position,
                    scale: genome.traits().sensing_scale(),
                };
            }
            view.sample_dual_batch(&sample_queries[..count], &mut dual_raw_samples[..count])?;
            for i in 0..count {
                let (Some(genome), slot) = batch_meta[i] else {
                    continue;
                };
                let gradients = encode_dual_for_controller_f32(
                    rules,
                    &chunk[i],
                    &dual_raw_samples[i],
                    from,
                    &mut base_f32,
                    &mut trail_f32,
                )?;
                emit_one(
                    base + i,
                    &chunk[i],
                    genome,
                    slot,
                    gradients,
                    &base_f32,
                    Some(trail_f32),
                )?;
            }
        }
        return Ok(());
    }

    if let Some(Sampled::Single(samples)) = sampled {
        for (index, organism) in run.iter().enumerate() {
            let (genome, slot) = live
                .as_mut()
                .map_or((None, None), |walk| walk.next(organism.id));
            let genome = genome.ok_or_else(|| CoreError::StateConflict {
                operation: "read genome",
                detail: format!("{} has no genome", organism.id),
            })?;
            let raw = match samples.get(start + index) {
                Some(raw) => *raw,
                None => view.sample(organism.position, genome.traits().sensing_scale())?,
            };
            let gradients = encode_for_controller_f32(rules, organism, &raw, from, &mut base_f32)?;
            emit_one(
                start + index,
                organism,
                genome,
                slot,
                gradients,
                &base_f32,
                None,
            )?;
        }
        return Ok(());
    }

    let batch_cap = SAMPLE_BATCH
        .min(sample_queries.len())
        .min(raw_samples.len())
        .max(1);
    let mut batch_meta: [(Option<&'p Genome>, Option<ArenaSlot>); SAMPLE_BATCH] =
        [(None, None); SAMPLE_BATCH];
    for (chunk_idx, chunk) in run.chunks(batch_cap).enumerate() {
        let count = chunk.len();
        let base = start + chunk_idx * batch_cap;
        for i in 0..count {
            let organism = &chunk[i];
            let (genome, slot) = live
                .as_mut()
                .map_or((None, None), |walk| walk.next(organism.id));
            let genome = genome.ok_or_else(|| CoreError::StateConflict {
                operation: "read genome",
                detail: format!("{} has no genome", organism.id),
            })?;
            batch_meta[i] = (Some(genome), slot);
            sample_queries[i] = SensorQuery {
                position: organism.position,
                scale: genome.traits().sensing_scale(),
            };
        }
        view.sample_batch(&sample_queries[..count], &mut raw_samples[..count])?;
        for i in 0..count {
            let (Some(genome), slot) = batch_meta[i] else {
                continue;
            };
            let gradients =
                encode_for_controller_f32(rules, &chunk[i], &raw_samples[i], from, &mut base_f32)?;
            emit_one(
                base + i,
                &chunk[i],
                genome,
                slot,
                gradients,
                &base_f32,
                None,
            )?;
        }
    }
    Ok(())
}

/// The zero-copy flat-F32 batched path for backends with
/// [`ControllerBackend::supports_flat_f32`]: eliminates intermediate `[f64; 104]`
/// / `[f64; 128]` staging, `Vec<ControllerJob>`, the second `population.ascending`
/// walk, and the `by_row` unpack copy.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "phases 3 and 4 flat-F32 fast path across Pass 1, evaluate_flat_f32, and Pass 2"
)]
fn decide_batched_flat_f32(
    rules: &Rules,
    population: &Population,
    view: &PyramidView<'_>,
    sampled: Option<Sampled<'_>>,
    rows: &[Organism],
    crowding_counts: &[u32],
    [from, to]: [Tick; 2],
    lanes: &mut [DecisionLane],
    device: &mut dyn ControllerBackend,
    flat_inputs_f32: &mut Vec<f32>,
    flat_inputs_finite: &mut Vec<u8>,
    flat_outputs: &mut FlatOutputs,
) -> Result<Batched, CoreError> {
    for lane in lanes.iter_mut() {
        lane.decisions.clear();
        lane.due_offset = 0;
        lane.due_count = 0;
    }
    if rows.is_empty() {
        flat_inputs_f32.clear();
        flat_inputs_finite.clear();
        flat_outputs.clear();
        return Ok(Batched::Done {
            batched: 0,
            deferred: 0,
            device_nanoseconds: 0,
        });
    }
    let sensors = rules.v2.sensors();
    let actions = rules.v2.actions();
    let input_width = sensors.input_width();
    let raw_output_width = actions.raw_output_width();

    let worker_budget = lanes.len();
    let used = worker_budget
        .min(rows.len().div_ceil(MINIMUM_ROWS_PER_LANE))
        .max(1);
    let chunk_len = if used == 1 {
        rows.len()
    } else {
        rows.len().div_ceil(used)
    };
    let active_lanes = rows.chunks(chunk_len).len();
    if active_lanes > lanes.len() {
        return Err(CoreError::StateConflict {
            operation: "decide",
            detail: "no decision lane".to_owned(),
        });
    }

    let mut total_due = 0_usize;
    for (run, lane) in rows.chunks(chunk_len).zip(lanes.iter_mut()) {
        let due_count = run
            .iter()
            .filter(|organism| due(rules, organism, to))
            .count();
        lane.due_offset = total_due;
        lane.due_count = due_count;
        total_due = total_due
            .checked_add(due_count)
            .ok_or(CoreError::Overflow {
                quantity: "due controller jobs",
            })?;
    }

    let total_input_elems = total_due
        .checked_mul(input_width)
        .ok_or(CoreError::Overflow {
            quantity: "flat controller inputs",
        })?;
    flat_inputs_f32.clear();
    reserve_for(flat_inputs_f32, total_input_elems, "flat controller inputs")?;
    flat_inputs_f32.resize(total_input_elems, 0.0_f32);
    flat_inputs_finite.clear();
    reserve_for(flat_inputs_finite, total_due, "flat controller input flags")?;
    flat_inputs_finite.resize(total_due, 0_u8);

    let flat_jobs = if active_lanes == 1 {
        let mut jobs = Vec::new();
        reserve_for(&mut jobs, total_due, "controller jobs")?;
        pass1_flat_f32(
            rules,
            population,
            view,
            sampled,
            crowding_counts,
            [from, to],
            0,
            rows,
            &mut lanes[0],
            flat_inputs_f32.as_mut_slice(),
            flat_inputs_finite.as_mut_slice(),
            &mut jobs,
        )?;
        jobs
    } else {
        let mut lane_jobs: Vec<Vec<FlatControllerJob<'_>>> = Vec::with_capacity(active_lanes);
        for lane in lanes.iter().take(active_lanes) {
            let mut part = Vec::new();
            reserve_for(&mut part, lane.due_count, "controller jobs")?;
            lane_jobs.push(part);
        }
        let mut chunks = rows.chunks(chunk_len);
        let head_run = chunks.next().unwrap_or(&[]);
        let (head_lane, tail_lanes) =
            lanes
                .split_first_mut()
                .ok_or_else(|| CoreError::StateConflict {
                    operation: "decide",
                    detail: "no decision lane".to_owned(),
                })?;
        let (head_jobs, tail_jobs) =
            lane_jobs
                .split_first_mut()
                .ok_or_else(|| CoreError::StateConflict {
                    operation: "decide",
                    detail: "no decision lane".to_owned(),
                })?;
        let head_due = head_lane.due_count;
        let (head_f32, mut rest_f32) = flat_inputs_f32
            .as_mut_slice()
            .split_at_mut(head_due * input_width);
        let (head_fin, mut rest_fin) = flat_inputs_finite.as_mut_slice().split_at_mut(head_due);
        let tail_count = active_lanes.saturating_sub(1);
        let mut tail_results: Vec<Result<(), CoreError>> =
            (0..tail_count).map(|_| Ok(())).collect();
        let mut head_result = Ok(());
        flife_core::pool::scope_workers(worker_budget, "decide", |scope| {
            for (index, (((run, lane), jobs_vec), result_slot)) in chunks
                .zip(tail_lanes.iter_mut())
                .zip(tail_jobs.iter_mut())
                .zip(tail_results.iter_mut())
                .enumerate()
            {
                let due_count = lane.due_count;
                let (my_f32, next_f32) =
                    core::mem::take(&mut rest_f32).split_at_mut(due_count * input_width);
                rest_f32 = next_f32;
                let (my_fin, next_fin) = core::mem::take(&mut rest_fin).split_at_mut(due_count);
                rest_fin = next_fin;
                let start = (index + 1) * chunk_len;
                scope.spawn(move || {
                    *result_slot = pass1_flat_f32(
                        rules,
                        population,
                        view,
                        sampled,
                        crowding_counts,
                        [from, to],
                        start,
                        run,
                        lane,
                        my_f32,
                        my_fin,
                        jobs_vec,
                    );
                });
            }
            head_result = pass1_flat_f32(
                rules,
                population,
                view,
                sampled,
                crowding_counts,
                [from, to],
                0,
                head_run,
                head_lane,
                head_f32,
                head_fin,
                head_jobs,
            );
        })?;
        head_result?;
        for result in tail_results {
            result?;
        }
        let mut merged = Vec::new();
        reserve_for(&mut merged, total_due, "controller jobs")?;
        for mut part in lane_jobs {
            merged.append(&mut part);
        }
        merged
    };

    flat_outputs.clear();
    let mut device_nanoseconds = 0_u64;
    if !flat_jobs.is_empty() {
        if let Err(error) = device.evaluate_flat_f32(
            &flat_jobs,
            input_width,
            flat_inputs_f32,
            flat_inputs_finite,
            flat_outputs,
        ) {
            return Ok(Batched::Device(error));
        }
        device_nanoseconds = device.last_device_nanoseconds();
        if flat_outputs.valid.len() != flat_jobs.len()
            || flat_outputs.output_width != raw_output_width
            || flat_outputs.values.len() != flat_jobs.len().saturating_mul(raw_output_width)
        {
            return Ok(Batched::Device(CoreError::StateConflict {
                operation: "batch controllers",
                detail: format!(
                    "backend returned {} valid flags and {} values (width {}), expected {} x {raw_output_width}",
                    flat_outputs.valid.len(),
                    flat_outputs.values.len(),
                    flat_outputs.output_width,
                    flat_jobs.len(),
                ),
            }));
        }

        let flat_jobs_slice: &[FlatControllerJob<'_>] = &flat_jobs;
        let flat_in_slice: &[f32] = flat_inputs_f32;
        let flat_out_ref: &FlatOutputs = flat_outputs;
        in_runs(rows, lanes, &|_start, run, lane| {
            if lane.due_count == 0 {
                return Ok(());
            }
            let mut job_idx = lane.due_offset;
            for (organism, decision) in run.iter().zip(lane.decisions.iter_mut()) {
                if !decision.evaluated {
                    continue;
                }
                let held = match flat_out_ref.row(job_idx) {
                    Some(raw_slice) => held_from_raw_slice(rules, organism, to, raw_slice)?,
                    None => {
                        let job = flat_jobs_slice.get(job_idx).ok_or_else(|| {
                            CoreError::StateConflict {
                                operation: "batch controllers",
                                detail: format!("missing deferred job {job_idx}"),
                            }
                        })?;
                        let start_in = job_idx * input_width;
                        let in_row = flat_in_slice
                            .get(start_in..start_in + input_width)
                            .ok_or_else(|| CoreError::StateConflict {
                                operation: "batch controllers",
                                detail: format!("missing flat inputs for deferred job {job_idx}"),
                            })?;
                        evaluate_reference_held_from_f32(
                            rules,
                            job.controller,
                            organism,
                            in_row,
                            to,
                            &mut lane.controller,
                        )?
                    }
                };
                decision.actions = decode_held(rules, organism, &held)?;
                decision.held = held;
                job_idx += 1;
            }
            Ok(())
        })?;
    }
    let deferred = flat_outputs.valid.iter().filter(|&&v| v == 0).count();
    let count = |value: usize| u64::try_from(value).unwrap_or(u64::MAX);
    Ok(Batched::Done {
        batched: count(flat_jobs.len().saturating_sub(deferred)),
        deferred: count(deferred),
        device_nanoseconds,
    })
}

/// The legacy batched path for backends that do not implement
/// [`ControllerBackend::supports_flat_f32`]: every observation on the lanes,
/// the due evaluations in one call to `device`, then each lane's decisions,
/// indexing `outputs` directly by `lane.due_offset` (no `by_row` copy).
#[expect(
    clippy::too_many_arguments,
    reason = "phase 3 and 4's inputs, crowding counts, and the backend's scratch vector"
)]
fn decide_batched(
    rules: &Rules,
    population: &Population,
    view: &PyramidView<'_>,
    sampled: Option<Sampled<'_>>,
    rows: &[Organism],
    crowding_counts: &[u32],
    [from, to]: [Tick; 2],
    lanes: &mut [DecisionLane],
    device: &mut dyn ControllerBackend,
    outputs: &mut Vec<Option<RawOutputs>>,
) -> Result<Batched, CoreError> {
    let sensors = rules.v2.sensors();
    let actions = rules.v2.actions();
    let input_width = sensors.input_width();
    let obs_width = sensors.observation_width();
    let state_width = rules.recurrent_state_width;
    let raw_output_width = actions.raw_output_width();
    let needs_expanded = input_width > SENSOR_WIDTH;
    let dual_carrier = rules.v2.dual_carrier().is_some();

    for lane in lanes.iter_mut() {
        lane.observed.clear();
        lane.controller_inputs.clear();
        lane.decisions.clear();
        lane.due_offset = 0;
        lane.due_count = 0;
    }
    in_runs(rows, lanes, &|start, run, lane| {
        let DecisionLane {
            sample_queries,
            raw_samples,
            dual_raw_samples,
            observed,
            controller_inputs,
            ..
        } = lane;
        reserve_for(observed, run.len(), "observations")?;
        if needs_expanded {
            reserve_for(controller_inputs, run.len(), "controller inputs")?;
        }
        let mut live = run.first().map(|first| population.ascending_from(first.id));
        let mut push_obs = |organism: &Organism,
                            obs: Observation,
                            trail_inputs: Option<[f64; 4]>,
                            local_crowding: Option<u32>| {
            if needs_expanded {
                let mut full = [0.0_f64; MAX_SENSOR_WIDTH];
                full[..SENSOR_WIDTH].copy_from_slice(&obs.inputs);
                fill_v2_extra_sensors(
                    sensors,
                    trail_inputs,
                    local_crowding,
                    Some(organism.last_neighbor_h0),
                    &mut full,
                );
                if state_width > 0 {
                    let prev = organism.held.recurrent_state_array();
                    for (slot, &h) in full[obs_width..input_width]
                        .iter_mut()
                        .zip(&prev[..state_width])
                    {
                        *slot = f64::from(h);
                    }
                }
                controller_inputs.push(full);
            }
            observed.push(obs);
        };
        if dual_carrier {
            if let Some(Sampled::Dual(samples)) = sampled {
                for (index, organism) in run.iter().enumerate() {
                    let genome = live
                        .as_mut()
                        .and_then(|walk| walk.next(organism.id).0)
                        .ok_or_else(|| CoreError::StateConflict {
                            operation: "read genome",
                            detail: format!("{} has no genome", organism.id),
                        })?;
                    let raw = match samples.get(start + index) {
                        Some(raw) => *raw,
                        None => {
                            view.sample_dual(organism.position, genome.traits().sensing_scale())?
                        }
                    };
                    let dual_obs = encode_dual_for_controller(rules, organism, &raw, from)?;
                    push_obs(
                        organism,
                        dual_obs.base,
                        Some(dual_obs.trail_inputs),
                        crowding_counts.get(start + index).copied(),
                    );
                }
                return Ok(());
            }
            let batch_cap = SAMPLE_BATCH
                .min(sample_queries.len())
                .min(dual_raw_samples.len())
                .max(1);
            for (chunk_idx, chunk) in run.chunks(batch_cap).enumerate() {
                let count = chunk.len();
                let base = start + chunk_idx * batch_cap;
                for (query, organism) in sample_queries[..count].iter_mut().zip(chunk) {
                    let genome = live
                        .as_mut()
                        .and_then(|walk| walk.next(organism.id).0)
                        .ok_or_else(|| CoreError::StateConflict {
                            operation: "read genome",
                            detail: format!("{} has no genome", organism.id),
                        })?;
                    *query = SensorQuery {
                        position: organism.position,
                        scale: genome.traits().sensing_scale(),
                    };
                }
                view.sample_dual_batch(&sample_queries[..count], &mut dual_raw_samples[..count])?;
                for (i, (organism, raw)) in chunk.iter().zip(&dual_raw_samples[..count]).enumerate()
                {
                    let dual_obs = encode_dual_for_controller(rules, organism, raw, from)?;
                    push_obs(
                        organism,
                        dual_obs.base,
                        Some(dual_obs.trail_inputs),
                        crowding_counts.get(base + i).copied(),
                    );
                }
            }
            return Ok(());
        }
        if let Some(Sampled::Single(samples)) = sampled {
            for (index, organism) in run.iter().enumerate() {
                let genome = live
                    .as_mut()
                    .and_then(|walk| walk.next(organism.id).0)
                    .ok_or_else(|| CoreError::StateConflict {
                        operation: "read genome",
                        detail: format!("{} has no genome", organism.id),
                    })?;
                let raw = match samples.get(start + index) {
                    Some(raw) => *raw,
                    None => view.sample(organism.position, genome.traits().sensing_scale())?,
                };
                let obs = encode_for_controller(rules, organism, &raw, from)?;
                push_obs(
                    organism,
                    obs,
                    None,
                    crowding_counts.get(start + index).copied(),
                );
            }
            return Ok(());
        }
        let batch_cap = SAMPLE_BATCH
            .min(sample_queries.len())
            .min(raw_samples.len())
            .max(1);
        for (chunk_idx, chunk) in run.chunks(batch_cap).enumerate() {
            let count = chunk.len();
            let base = start + chunk_idx * batch_cap;
            for (query, organism) in sample_queries[..count].iter_mut().zip(chunk) {
                let genome = live
                    .as_mut()
                    .and_then(|walk| walk.next(organism.id).0)
                    .ok_or_else(|| CoreError::StateConflict {
                        operation: "read genome",
                        detail: format!("{} has no genome", organism.id),
                    })?;
                *query = SensorQuery {
                    position: organism.position,
                    scale: genome.traits().sensing_scale(),
                };
            }
            view.sample_batch(&sample_queries[..count], &mut raw_samples[..count])?;
            for (i, (organism, raw)) in chunk.iter().zip(&raw_samples[..count]).enumerate() {
                let obs = encode_for_controller(rules, organism, raw, from)?;
                push_obs(organism, obs, None, crowding_counts.get(base + i).copied());
            }
        }
        Ok(())
    })?;

    let mut cursor = 0_usize;
    let mut due_total = 0_usize;
    for lane in lanes.iter_mut() {
        let len = if needs_expanded {
            lane.controller_inputs.len()
        } else {
            lane.observed.len()
        };
        let slice = rows.get(cursor..cursor + len).unwrap_or(&[]);
        let count = slice
            .iter()
            .filter(|organism| due(rules, organism, to))
            .count();
        lane.due_offset = due_total;
        lane.due_count = count;
        due_total += count;
        cursor += len;
    }

    let mut jobs = Vec::new();
    reserve_for(&mut jobs, rows.len(), "controller jobs")?;
    let mut seen = 0;
    let mut live = population.ascending();
    if needs_expanded {
        let expanded = lanes.iter().flat_map(|lane| lane.controller_inputs.iter());
        for (organism, full) in rows.iter().zip(expanded) {
            seen += 1;
            if due(rules, organism, to) {
                let (genome, slot) = live.next(organism.id);
                jobs.push(ControllerJob {
                    genome: organism.genome,
                    slot: slot.ok_or_else(|| CoreError::StateConflict {
                        operation: "controller batch",
                        detail: format!("{} holds no arena slot", organism.id),
                    })?,
                    controller: genome
                        .ok_or_else(|| CoreError::StateConflict {
                            operation: "read genome",
                            detail: format!("{} has no genome", organism.id),
                        })?
                        .controller(&rules.catalog, &rules.limits)?,
                    inputs: &full[..input_width],
                });
            }
        }
    } else {
        let observed = lanes.iter().flat_map(|lane| lane.observed.iter());
        for (organism, observation) in rows.iter().zip(observed) {
            seen += 1;
            if due(rules, organism, to) {
                let (genome, slot) = live.next(organism.id);
                jobs.push(ControllerJob {
                    genome: organism.genome,
                    slot: slot.ok_or_else(|| CoreError::StateConflict {
                        operation: "controller batch",
                        detail: format!("{} holds no arena slot", organism.id),
                    })?,
                    controller: genome
                        .ok_or_else(|| CoreError::StateConflict {
                            operation: "read genome",
                            detail: format!("{} has no genome", organism.id),
                        })?
                        .controller(&rules.catalog, &rules.limits)?,
                    inputs: &observation.inputs,
                });
            }
        }
    }
    if seen != rows.len() {
        return Err(CoreError::StateConflict {
            operation: "batch observations",
            detail: format!("{seen} observations for {} organisms", rows.len()),
        });
    }
    outputs.clear();
    let mut device_nanoseconds = 0;
    if !jobs.is_empty() {
        if let Err(error) = device.evaluate(&jobs, outputs) {
            return Ok(Batched::Device(error));
        }
        device_nanoseconds = device.last_device_nanoseconds();
        if outputs.len() != jobs.len() {
            return Ok(Batched::Device(CoreError::StateConflict {
                operation: "batch controllers",
                detail: format!("{} outputs for {} jobs", outputs.len(), jobs.len()),
            }));
        }
        if let Some(bad) = outputs
            .iter()
            .flatten()
            .find(|raw| raw.len() != raw_output_width)
        {
            return Ok(Batched::Device(CoreError::StateConflict {
                operation: "batch controllers",
                detail: format!(
                    "backend returned {} outputs, expected {raw_output_width}",
                    bad.len()
                ),
            }));
        }
    }
    drop(jobs);

    let outputs_slice: &[Option<RawOutputs>] = outputs;
    in_runs(rows, lanes, &|start, run, lane| {
        let DecisionLane {
            controller,
            observed,
            controller_inputs,
            due_offset,
            decisions,
            ..
        } = lane;
        reserve_for(decisions, run.len(), "decisions")?;
        let mut live = run.first().map(|first| population.ascending_from(first.id));
        let mut job_idx = *due_offset;
        for (index, (organism, observation)) in run.iter().zip(observed.iter()).enumerate() {
            let evaluated = due(rules, organism, to);
            let held = if evaluated {
                let raw = outputs_slice.get(job_idx).copied().flatten();
                job_idx += 1;
                match raw {
                    Some(raw) => held_from_raw(rules, organism, to, &raw)?,
                    None => {
                        let genome = live
                            .as_mut()
                            .and_then(|walk| walk.next(organism.id).0)
                            .ok_or_else(|| CoreError::StateConflict {
                                operation: "read genome",
                                detail: format!("{} has no genome", organism.id),
                            })?;
                        let trail_inputs = sensors.dual_carrier_trails().and_then(|slot| {
                            controller_inputs.get(index).map(|full| {
                                [full[slot], full[slot + 1], full[slot + 2], full[slot + 3]]
                            })
                        });
                        evaluate_reference_held(
                            rules,
                            genome,
                            organism,
                            observation,
                            trail_inputs,
                            crowding_counts.get(start + index).copied(),
                            to,
                            controller,
                        )?
                    }
                }
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
        device_nanoseconds,
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

/// The tiny-reservoir / viability-floor lysis stage, the last of §8.2.
/// Returns what went to [`Flow::TinyReservoir`] (always `0.0` when `lysis-recycling`
/// diverts the residual into `organism.lysed_residual` for `Transfer::Recycling`).
fn clear_tiny(
    tiny: &TinyEnergyPolicy,
    organism: &mut Working,
    ledger: &mut TickLedger,
) -> Result<f64, CoreError> {
    let cleared = tiny.clear(&mut organism.budget)?;
    let tiny_flow = if tiny.is_lysis() {
        organism.lysed_residual = cleared;
        0.0
    } else {
        if cleared > 0.0 {
            ledger.record(Flow::TinyReservoir, cleared)?;
        }
        cleared
    };
    organism
        .vitality
        .check(BudgetStage::TinyReservoir, &organism.budget)?;
    Ok(tiny_flow)
}

/// [`OrganismSnapshot::local`]: the exchange footprint at `position` on
/// `field`, each footprint cell's efficiency for `traits`' affinity, and
/// A.1's request at the footprint's density, by the functions phase 5 and
/// chemistry use.
///
/// # Errors
/// As [`ExchangeKernel::footprint_into`], [`RequestRule::request`],
/// [`Affinity::new`], [`composition`] and [`Chemistry::efficiency`].
fn local_chemistry(
    rules: &Rules,
    field: &FieldState,
    position: [f64; 2],
    traits: &Traits,
    recurrent_h0: f32,
) -> Result<LocalChemistry, CoreError> {
    let a_eff = effective_affinity(
        traits,
        recurrent_h0,
        &rules.v2,
        rules.chemistry.affinity_norm_limit(),
    )?;
    let affinity = Affinity::new(a_eff, &rules.chemistry)?;
    let mut scratch = ExchangeScratch::new();
    rules
        .exchange
        .footprint_into(field.grid(), position, &mut scratch)?;
    let mut energy_amount = 0.0;
    let mut cells = Vec::with_capacity(scratch.cells().len());
    let (mut gain, mut harm) = (0.0, 0.0);
    for &(cell, weight) in scratch.cells() {
        let efficiency = rules
            .chemistry
            .efficiency(&affinity, &composition(field, cell)?)?;
        let energy =
            field
                .energy()
                .get(cell)
                .copied()
                .ok_or_else(|| CoreError::LayoutMismatch {
                    buffer: "field",
                    detail: format!("exchange cell {cell} is not in this field"),
                })?;
        energy_amount += weight * f64::from(energy);
        gain += weight * efficiency.max(0.0);
        harm += weight * (-efficiency).max(0.0);
        cells.push(ExchangeCell {
            cell,
            weight,
            energy,
            efficiency,
        });
    }
    let density = energy_amount / field.grid().cell_area();
    let rule = RequestRule {
        step: rules.step,
        capacity_per_second: rules.uptake_capacity,
        half_saturation_density: rules.half_saturation,
    };
    let request = rule.request(density)?;
    Ok(LocalChemistry {
        cells,
        density,
        request,
        gain,
        toxicity: rules.chemistry.toxicity_sensitivity() * harm,
    })
}

/// What a viable attempt by the watched organism came to in phase 8.
///
/// # Errors
/// [`CoreError::StateConflict`] if its birth was neither staged, refused
/// nor rejected.
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
    if let Some(refused) = births
        .refused()
        .iter()
        .find(|refused| refused.parent == intent.parent)
    {
        return Ok(ReproductionOutcome::Refused {
            split: intent.split,
            resource: refused.resource,
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
            detail: format!(
                "{}'s birth was neither staged, refused nor rejected",
                intent.parent
            ),
        })
}

/// What phases 6 and 7 change of one working organism: its budget and
/// vitality, its velocity and position, whether it emitted, and its birth
/// intent. [`settle_stages`] computes it without
/// writing the row, so a backend's phase 6 writes no row until every
/// organism has settled (FL-032).
#[derive(Debug, Clone, Copy)]
struct Settled {
    budget: EnergyBudget,
    vitality: Vitality,
    velocity: [f64; 2],
    position: [f64; 2],
    secretory_readiness: f32,
    emission_cooldown_deadline: Tick,
    emitted: bool,
    intent: Option<BirthIntent>,
}

impl Settled {
    /// `organism`'s values before phase 6.
    const fn of(organism: &Working) -> Self {
        Self {
            budget: organism.budget,
            vitality: organism.vitality,
            velocity: organism.velocity,
            position: organism.position,
            secretory_readiness: organism.secretory_readiness,
            emission_cooldown_deadline: organism.emission_cooldown_deadline,
            emitted: organism.emitted,
            intent: organism.intent,
        }
    }

    /// Write these values to `organism`.
    const fn apply(self, organism: &mut Working) {
        organism.budget = self.budget;
        organism.vitality = self.vitality;
        organism.velocity = self.velocity;
        organism.position = self.position;
        organism.secretory_readiness = self.secretory_readiness;
        organism.emission_cooldown_deadline = self.emission_cooldown_deadline;
        organism.emitted = self.emitted;
        organism.intent = self.intent;
    }
}

/// Where phase 6's settlements and phase 7's proposal come from: the
/// reference's operators ([`Reference`]), or a backend's outcomes replayed on
/// the host's budget (`settle::Device`, FL-032). Each stage settles against
/// `budget`, the organism's balance as the earlier stages left it.
trait Stages {
    /// Chemistry: the gain credited and the toxicity paid.
    fn uptake(
        &mut self,
        rules: &Rules,
        organism: &Working,
        budget: &mut EnergyBudget,
    ) -> Result<UptakeSettlement, CoreError>;

    /// Maintenance: the bill paid.
    fn maintenance(
        &mut self,
        rules: &Rules,
        organism: &Working,
        budget: &mut EnergyBudget,
    ) -> Result<MaintenanceSettlement, CoreError>;

    /// Motion: the proposal from the uptake `absorbed`, and its payment.
    fn movement(
        &mut self,
        rules: &Rules,
        organism: &Working,
        absorbed: f64,
        budget: &mut EnergyBudget,
    ) -> Result<MotionSettlement, CoreError>;

    /// The position after `organism` moves at `velocity`.
    fn displace(
        &mut self,
        rules: &Rules,
        organism: &Working,
        velocity: [f64; 2],
    ) -> Result<[f64; 2], CoreError>;

    /// Emission at the post-motion `position` and `velocity`: the payment,
    /// deposit staged in `emissions` (plus `trail_emissions` when `V2-15`
    /// dual-carrier is active), and updated excitable state
    /// `(next_readiness, next_cooldown_deadline)`.
    #[expect(
        clippy::too_many_arguments,
        reason = "Phase 7 emission stage inputs including V2-04 post-motion velocity, tick, and V2-15 trail deposits"
    )]
    fn emission(
        &mut self,
        rules: &Rules,
        organism: &Working,
        position: [f64; 2],
        velocity: [f64; 2],
        from: Tick,
        budget: &mut EnergyBudget,
        emissions: &mut Deposits,
        trail_emissions: Option<&mut Deposits>,
        exchange_scratch: &mut ExchangeScratch,
        footprint: &mut CachedFootprint,
    ) -> Result<(EmissionSettlement, f32, Tick), CoreError>;

    /// Phase 7: `parent`'s reproduction proposal at attempt tick `tick`,
    /// from its post-emission `budget`.
    fn propose(
        &mut self,
        rules: &Rules,
        organism: &Working,
        parent: &ParentState,
        tick: Tick,
        budget: &EnergyBudget,
    ) -> Result<ReproductionProposal, CoreError>;
}

/// The reference's stages for one organism with genome `traits`, a
/// controller of `parameters` weights, and its granted `packets`.
struct Reference<'a> {
    traits: &'a Traits,
    parameters: u32,
    packets: &'a [ChemicalPacket],
}

impl Stages for Reference<'_> {
    fn uptake(
        &mut self,
        rules: &Rules,
        organism: &Working,
        budget: &mut EnergyBudget,
    ) -> Result<UptakeSettlement, CoreError> {
        let a_eff = effective_affinity(
            self.traits,
            organism.held.recurrent_state_array()[0],
            &rules.v2,
            rules.chemistry.affinity_norm_limit(),
        )?;
        let affinity = if rules.limits.affinity_norm() <= rules.chemistry.affinity_norm_limit() {
            Affinity::trusted(a_eff)
        } else {
            Affinity::new(a_eff, &rules.chemistry)?
        };
        rules.chemistry.settle(&affinity, self.packets, budget)
    }

    fn maintenance(
        &mut self,
        rules: &Rules,
        organism: &Working,
        budget: &mut EnergyBudget,
    ) -> Result<MaintenanceSettlement, CoreError> {
        rules
            .costs
            .bill_with_basal_multiplier(
                rules.step,
                self.traits.sensing_scale(),
                self.parameters,
                organism.evaluated,
                organism.basal_multiplier,
            )?
            .settle(budget)
    }

    fn movement(
        &mut self,
        rules: &Rules,
        organism: &Working,
        absorbed: f64,
        budget: &mut EnergyBudget,
    ) -> Result<MotionSettlement, CoreError> {
        let ratio = rules.response_gain(absorbed)?;
        let response = if rules.limits.motion_response_norm() <= rules.motion.response_norm_limit()
        {
            MotionResponse::trusted(*self.traits.motion_response())
        } else {
            MotionResponse::new(*self.traits.motion_response(), &rules.motion)?
        };
        let acceleration = match (rules.v2.motor(), organism.actions.motor) {
            (Some(constants), Some(motor)) => rules.motion.acceleration_v2(
                ratio,
                &response,
                &organism.gradients,
                organism.velocity,
                budget.remaining(),
                &constants,
                MotorSteeringInputs::from(motor),
            )?,
            _ => rules
                .motion
                .acceleration(ratio, &response, &organism.gradients)?,
        };
        let proposed = rules.motion.propose(organism.velocity, acceleration)?;
        rules.motion.settle(proposed, budget)
    }

    fn displace(
        &mut self,
        rules: &Rules,
        organism: &Working,
        velocity: [f64; 2],
    ) -> Result<[f64; 2], CoreError> {
        rules
            .motion
            .displace(&rules.grid, organism.position, velocity)
    }

    fn emission(
        &mut self,
        rules: &Rules,
        organism: &Working,
        position: [f64; 2],
        velocity: [f64; 2],
        from: Tick,
        budget: &mut EnergyBudget,
        emissions: &mut Deposits,
        trail_emissions: Option<&mut Deposits>,
        exchange_scratch: &mut ExchangeScratch,
        footprint: &mut CachedFootprint,
    ) -> Result<(EmissionSettlement, f32, Tick), CoreError> {
        let recovery_alpha = rules.v2.emission_recovery_alpha();
        if let (Some(dc), Some(trail_deposits)) = (rules.v2.dual_carrier(), trail_emissions) {
            let trail_share = dc.trail_emission_share();
            if !rules.v2.emission().any_enabled() {
                let settlement = rules.emission.emit_dual_with_floor_reusing_footprint(
                    &rules.grid,
                    rules.exchange,
                    organism.committed.id,
                    position,
                    organism.actions.emission.as_ref(),
                    0.0,
                    trail_share,
                    budget,
                    emissions,
                    trail_deposits,
                    exchange_scratch,
                    footprint,
                )?;
                return Ok((
                    settlement,
                    organism.secretory_readiness,
                    organism.emission_cooldown_deadline,
                ));
            }
            let ctx = rules.emission_context(organism, position, velocity, from)?;
            let (settlement, plan) = rules.emission.emit_v2_dual_reusing_footprint(
                &rules.grid,
                rules.exchange,
                rules.step,
                rules.v2.emission(),
                recovery_alpha,
                organism.committed.id,
                organism.actions.emission.as_ref(),
                &ctx,
                trail_share,
                budget,
                emissions,
                trail_deposits,
                exchange_scratch,
                footprint,
            )?;
            return Ok((settlement, plan.next_readiness, plan.next_cooldown_deadline));
        }
        if !rules.v2.emission().any_enabled() {
            let settlement = rules.emission.emit_with_floor_reusing_footprint(
                &rules.grid,
                rules.exchange,
                organism.committed.id,
                position,
                organism.actions.emission.as_ref(),
                0.0,
                budget,
                emissions,
                exchange_scratch,
                footprint,
            )?;
            return Ok((
                settlement,
                organism.secretory_readiness,
                organism.emission_cooldown_deadline,
            ));
        }
        let ctx = rules.emission_context(organism, position, velocity, from)?;
        let (settlement, plan) = rules.emission.emit_v2_reusing_footprint(
            &rules.grid,
            rules.exchange,
            rules.step,
            rules.v2.emission(),
            recovery_alpha,
            organism.committed.id,
            organism.actions.emission.as_ref(),
            &ctx,
            budget,
            emissions,
            exchange_scratch,
            footprint,
        )?;
        Ok((settlement, plan.next_readiness, plan.next_cooldown_deadline))
    }

    fn propose(
        &mut self,
        rules: &Rules,
        organism: &Working,
        parent: &ParentState,
        tick: Tick,
        budget: &EnergyBudget,
    ) -> Result<ReproductionProposal, CoreError> {
        let attempt_probability = rules.effective_attempt_probability(organism)?;
        rules.reproduction.propose_with_probability(
            &rules.rng,
            parent,
            tick,
            attempt_probability,
            organism.actions.child_fraction,
            budget,
        )
    }
}

/// The contact models' `[contact]` table, which `field-ecology-v1-contact*`
/// requires, every other v1 model refuses, and `field-ecology-v2*` admits
/// optionally (validation checks the same rules; this is the engine's own
/// guard).
fn contact_rules(config: &ValidatedConfig) -> Result<Option<ContactRules>, CoreError> {
    let model = &config.config().model;
    let variant = ModelVariant::of(model)?;
    let rules = ContactRules::from_config(config)?;
    if variant.is_v2() {
        return Ok(rules);
    }
    match (variant.contact(), rules) {
        (true, Some(rules)) => Ok(Some(rules)),
        (false, None) => Ok(None),
        (needs, _) => Err(CoreError::InvalidConfig(vec![ConfigIssue {
            path: "contact".into(),
            problem: if needs {
                format!("{model} needs a [contact] table")
            } else {
                format!("[contact] belongs to a contact model, not {model}")
            },
        }])),
    }
}

/// `velocity` without `share` of its component against `push`, the push of
/// the bodies a survivor overlapped (contact inhibition of locomotion,
/// [`crate::contact`]): unchanged for no push or a velocity along it.
fn inhibited(velocity: [f64; 2], push: [f64; 2], share: f64) -> [f64; 2] {
    let length = push[0].hypot(push[1]);
    if !length.is_finite() || length <= 0.0 {
        return velocity;
    }
    let normal = [push[0] / length, push[1] / length];
    let along = velocity[0] * normal[0] + velocity[1] * normal[1];
    if along < 0.0 && share > 0.0 {
        let removed = share * along;
        [
            velocity[0] - removed * normal[0],
            velocity[1] - removed * normal[1],
        ]
    } else {
        velocity
    }
}

/// Combined EMT adhesion multiplier for `organism` (`V2-08` `2 σ(o_adh)`
/// composed multiplicatively with `V2-12` post-Phase-4
/// `m(h_1) = clamp(1 + h_1, 0, 1)`).
fn effective_emt_factor(rules: &Rules, organism: &Working) -> f64 {
    let base = if rules.v2.emt_gate() {
        2.0 * organism.actions.emt_gate.unwrap_or(0.5)
    } else {
        1.0
    };
    if rules.v2.emt_h1_adhesion_enabled() {
        let h1 = organism.held.recurrent_state_array()[1];
        base * (1.0 + f64::from(h1)).clamp(0.0, 1.0)
    } else {
        base
    }
}

/// Resize `vec` to `len` without zeroing existing elements when shrinking,
/// avoiding redundant single-threaded memsets before workers overwrite `0..len`.
#[inline]
fn set_len_reuse<T: Copy>(vec: &mut Vec<T>, len: usize, fill: T) {
    if vec.len() >= len {
        vec.truncate(len);
    } else {
        vec.resize(len, fill);
    }
}

/// Carve up to `take` elements from the front of a mutable slice without
/// extending the borrow on the slice binding itself (safe for parallel fanout).
#[inline]
fn split_prefix<'a, T>(slice: &mut &'a mut [T], take: usize) -> &'a mut [T] {
    let n = take.min(slice.len());
    let (head, tail) = core::mem::take(slice).split_at_mut(n);
    *slice = tail;
    head
}

/// Disjoint per-worker output slices for parallel [`contact_displacements`]
/// input staging into [`ContactWork`].
struct ContactPrepChunk<'a> {
    pos: &'a mut [[f64; 2]],
    ids: &'a mut [u64],
    alive: &'a mut [bool],
    aff: &'a mut [[f32; AFFINITY_TERMS]],
    pol: &'a mut [[f64; 2]],
    pol_s: &'a mut [f64],
    wedge: &'a mut [f64],
    grad: &'a mut [[f64; 2]],
    emt: &'a mut [f64],
    h0: &'a mut [f32],
    cad_tag: &'a mut [[f64; CADHERIN_TERMS]],
    cad_coh: &'a mut [f64],
    energies: &'a mut [f64],
    vels: &'a mut [[f64; 2]],
}

/// Every working organism's contact displacement into `work`, from the
/// settled positions of those that survive the tick, together with any active
/// `field-ecology-v2` Sweep-1 channels (`V2-08` PCP/EMT, `V2-12` Notch-Delta,
/// `V2-13` cadherin-4d, passive exact-quantum trophallaxis, and dissipative
/// velocity alignment).
#[expect(
    clippy::too_many_lines,
    reason = "parallel contiguous ContactWork input staging across all optional V2 Sweep-1 channels"
)]
fn contact_displacements(
    rules: &Rules,
    bodies: &ContactRules,
    population: &Population,
    working: &[Working],
    work: &mut ContactWork,
    workers: usize,
) -> Result<(), CoreError> {
    let count = working.len();
    let pcp = rules.v2.polarity();
    let needs_emt_factors = rules.v2.emt_gate() || rules.v2.emt_h1_adhesion_enabled();
    let notch_delta = rules.v2.notch_delta().is_some();
    let cadherin = rules.v2.cadherin().is_some();
    let trophallaxis = rules.v2.trophallaxis();
    let velocity_alignment = rules.v2.velocity_alignment();
    let needs_affinities = (bodies.adheres() && !cadherin)
        || (pcp.is_some() && !cadherin)
        || notch_delta
        || trophallaxis.is_some()
        || (velocity_alignment.is_some() && !cadherin);
    let needs_genome = needs_affinities || pcp.is_some() || cadherin;

    work.next_polarities.clear();
    work.neighbor_h0.clear();
    work.trophallaxis_delta_quanta.clear();
    work.next_velocities.clear();

    let prep_used = if count >= 512 && workers > 1 {
        workers.min(count.div_ceil(256)).max(1)
    } else {
        1
    };

    if prep_used == 1 {
        work.positions.clear();
        work.ids.clear();
        work.alive.clear();
        work.affinities.clear();
        work.polarities.clear();
        work.polarity_strengths.clear();
        work.wedge_curvatures.clear();
        work.energy_gradients.clear();
        work.emt_factors.clear();
        work.cell_states_h0.clear();
        work.cadherin_tags.clear();
        work.cadherin_cohesiveness.clear();
        work.energies.clear();
        work.velocities.clear();
        let mut live_genomes = needs_genome.then(|| population.ascending());
        for organism in working {
            work.positions.push(organism.position);
            work.ids.push(organism.committed.id.get());
            work.alive
                .push(matches!(organism.vitality.fate()?, Fate::Survives));
            let genome = if let Some(walk) = live_genomes.as_mut() {
                Some(walk.next(organism.committed.id).0.ok_or_else(|| {
                    CoreError::StateConflict {
                        operation: "read genome",
                        detail: format!("{} has no genome", organism.committed.id),
                    }
                })?)
            } else {
                None
            };
            if needs_affinities && let Some(genome) = genome {
                // CRITICAL V2-12 / V2-13 clonal kin rule: contact kin weighting
                // (and ALWAYS trophallaxis & Notch-Delta) MUST use GENETIC
                // `traits.affinity()`, NEVER rotated `a_eff(h_0)`, so
                // differentiated clonal sisters retain kin signaling and freely
                // pipe metabolic energy inward. `Traits::unit_affinity()` caches
                // the bit-identical normalized F32 vector at genome creation.
                work.affinities.push(*genome.traits().unit_affinity());
            }
            if pcp.is_some() {
                let traits = genome.map(Genome::traits);
                work.polarities.push(organism.committed.polarity);
                work.polarity_strengths
                    .push(traits.and_then(Traits::polarity_strength).unwrap_or(0.0));
                work.wedge_curvatures
                    .push(traits.and_then(Traits::wedge_curvature).unwrap_or(0.0));
                work.energy_gradients.push(organism.gradients[0]);
            }
            if needs_emt_factors {
                work.emt_factors.push(effective_emt_factor(rules, organism));
            }
            if notch_delta {
                work.cell_states_h0
                    .push(organism.held.recurrent_state_array()[0]);
            }
            if cadherin {
                let cad = genome
                    .and_then(|g| g.traits().cadherin_traits())
                    .ok_or_else(|| CoreError::StateConflict {
                        operation: "contact",
                        detail: format!(
                            "{} is missing cadherin traits while cadherin-4d is enabled",
                            organism.committed.id
                        ),
                    })?;
                work.cadherin_tags.push(*cad.tag());
                work.cadherin_cohesiveness.push(cad.cohesiveness());
            }
            if trophallaxis.is_some() {
                work.energies.push(organism.budget.remaining());
            }
            if velocity_alignment.is_some() {
                work.velocities.push(organism.velocity);
            }
        }
    } else {
        set_len_reuse(&mut work.positions, count, [0.0, 0.0]);
        set_len_reuse(&mut work.ids, count, 0_u64);
        set_len_reuse(&mut work.alive, count, false);
        if needs_affinities {
            set_len_reuse(&mut work.affinities, count, [0.0_f32; AFFINITY_TERMS]);
        } else {
            work.affinities.clear();
        }
        if pcp.is_some() {
            set_len_reuse(&mut work.polarities, count, [1.0, 0.0]);
            set_len_reuse(&mut work.polarity_strengths, count, 0.0);
            set_len_reuse(&mut work.wedge_curvatures, count, 0.0);
            set_len_reuse(&mut work.energy_gradients, count, [0.0, 0.0]);
        } else {
            work.polarities.clear();
            work.polarity_strengths.clear();
            work.wedge_curvatures.clear();
            work.energy_gradients.clear();
        }
        if needs_emt_factors {
            set_len_reuse(&mut work.emt_factors, count, 1.0);
        } else {
            work.emt_factors.clear();
        }
        if notch_delta {
            set_len_reuse(&mut work.cell_states_h0, count, 0.0_f32);
        } else {
            work.cell_states_h0.clear();
        }
        if cadherin {
            set_len_reuse(&mut work.cadherin_tags, count, [0.0_f64; CADHERIN_TERMS]);
            set_len_reuse(&mut work.cadherin_cohesiveness, count, 0.0);
        } else {
            work.cadherin_tags.clear();
            work.cadherin_cohesiveness.clear();
        }
        if trophallaxis.is_some() {
            set_len_reuse(&mut work.energies, count, 0.0);
        } else {
            work.energies.clear();
        }
        if velocity_alignment.is_some() {
            set_len_reuse(&mut work.velocities, count, [0.0, 0.0]);
        } else {
            work.velocities.clear();
        }

        let fill_chunk = |chunk: &[Working], out: ContactPrepChunk<'_>| -> Result<(), CoreError> {
            let mut live_genomes = if needs_genome {
                chunk
                    .first()
                    .map(|first| population.ascending_from(first.committed.id))
            } else {
                None
            };
            for (i, organism) in chunk.iter().enumerate() {
                out.pos[i] = organism.position;
                out.ids[i] = organism.committed.id.get();
                out.alive[i] = matches!(organism.vitality.fate()?, Fate::Survives);
                let genome = if let Some(walk) = live_genomes.as_mut() {
                    Some(walk.next(organism.committed.id).0.ok_or_else(|| {
                        CoreError::StateConflict {
                            operation: "read genome",
                            detail: format!("{} has no genome", organism.committed.id),
                        }
                    })?)
                } else {
                    None
                };
                if needs_affinities && let Some(genome) = genome {
                    out.aff[i] = *genome.traits().unit_affinity();
                }
                if pcp.is_some() {
                    let traits = genome.map(Genome::traits);
                    out.pol[i] = organism.committed.polarity;
                    out.pol_s[i] = traits.and_then(Traits::polarity_strength).unwrap_or(0.0);
                    out.wedge[i] = traits.and_then(Traits::wedge_curvature).unwrap_or(0.0);
                    out.grad[i] = organism.gradients[0];
                }
                if needs_emt_factors {
                    out.emt[i] = effective_emt_factor(rules, organism);
                }
                if notch_delta {
                    out.h0[i] = organism.held.recurrent_state_array()[0];
                }
                if cadherin {
                    let cad = genome
                        .and_then(|g| g.traits().cadherin_traits())
                        .ok_or_else(|| CoreError::StateConflict {
                            operation: "contact",
                            detail: format!(
                                "{} is missing cadherin traits while cadherin-4d is enabled",
                                organism.committed.id
                            ),
                        })?;
                    out.cad_tag[i] = *cad.tag();
                    out.cad_coh[i] = cad.cohesiveness();
                }
                if trophallaxis.is_some() {
                    out.energies[i] = organism.budget.remaining();
                }
                if velocity_alignment.is_some() {
                    out.vels[i] = organism.velocity;
                }
            }
            Ok(())
        };

        let chunk_len = count.div_ceil(prep_used);
        let mut r_pos = work.positions.as_mut_slice();
        let mut r_ids = work.ids.as_mut_slice();
        let mut r_alive = work.alive.as_mut_slice();
        let mut r_aff = work.affinities.as_mut_slice();
        let mut r_pol = work.polarities.as_mut_slice();
        let mut r_pol_s = work.polarity_strengths.as_mut_slice();
        let mut r_wedge = work.wedge_curvatures.as_mut_slice();
        let mut r_grad = work.energy_gradients.as_mut_slice();
        let mut r_emt = work.emt_factors.as_mut_slice();
        let mut r_h0 = work.cell_states_h0.as_mut_slice();
        let mut r_cad_tag = work.cadherin_tags.as_mut_slice();
        let mut r_cad_coh = work.cadherin_cohesiveness.as_mut_slice();
        let mut r_energies = work.energies.as_mut_slice();
        let mut r_vels = work.velocities.as_mut_slice();

        let mut chunks_in = working.chunks(chunk_len);
        let head_in = chunks_in.next().unwrap_or(&[]);
        let take_head = head_in.len();
        let head_out = ContactPrepChunk {
            pos: split_prefix(&mut r_pos, take_head),
            ids: split_prefix(&mut r_ids, take_head),
            alive: split_prefix(&mut r_alive, take_head),
            aff: split_prefix(&mut r_aff, take_head),
            pol: split_prefix(&mut r_pol, take_head),
            pol_s: split_prefix(&mut r_pol_s, take_head),
            wedge: split_prefix(&mut r_wedge, take_head),
            grad: split_prefix(&mut r_grad, take_head),
            emt: split_prefix(&mut r_emt, take_head),
            h0: split_prefix(&mut r_h0, take_head),
            cad_tag: split_prefix(&mut r_cad_tag, take_head),
            cad_coh: split_prefix(&mut r_cad_coh, take_head),
            energies: split_prefix(&mut r_energies, take_head),
            vels: split_prefix(&mut r_vels, take_head),
        };

        let tail_count = chunks_in.len();
        let mut tail_results: Vec<Result<(), CoreError>> =
            (0..tail_count).map(|_| Ok(())).collect();
        let mut head_result = Ok(());
        let fill_chunk = &fill_chunk;
        flife_core::pool::scope_workers(workers, "contact prep", |scope| {
            for (chunk, res_slot) in chunks_in.zip(tail_results.iter_mut()) {
                let take = chunk.len();
                let out = ContactPrepChunk {
                    pos: split_prefix(&mut r_pos, take),
                    ids: split_prefix(&mut r_ids, take),
                    alive: split_prefix(&mut r_alive, take),
                    aff: split_prefix(&mut r_aff, take),
                    pol: split_prefix(&mut r_pol, take),
                    pol_s: split_prefix(&mut r_pol_s, take),
                    wedge: split_prefix(&mut r_wedge, take),
                    grad: split_prefix(&mut r_grad, take),
                    emt: split_prefix(&mut r_emt, take),
                    h0: split_prefix(&mut r_h0, take),
                    cad_tag: split_prefix(&mut r_cad_tag, take),
                    cad_coh: split_prefix(&mut r_cad_coh, take),
                    energies: split_prefix(&mut r_energies, take),
                    vels: split_prefix(&mut r_vels, take),
                };
                scope.spawn(move || {
                    *res_slot = fill_chunk(chunk, out);
                });
            }
            head_result = fill_chunk(head_in, head_out);
        })?;
        head_result?;
        for result in tail_results {
            result?;
        }
    }
    let dt = rules.step.seconds_per_tick();
    let pcp_params = pcp.map(|pol| PcpSweepParams {
        dt,
        torque_mobility: pol.torque_mobility,
        gradient_cue_strength: pol.gradient_cue_strength,
    });
    let trophallaxis_params = trophallaxis.map(|tr| TrophallaxisSweepParams {
        dt,
        conductance_per_second: tr.conductance_per_second,
        transfer_efficiency: tr.transfer_efficiency,
        reserve_energy: tr.reserve_energy,
        maximum_export_fraction: tr.maximum_export_fraction,
        kin_cosine_exponent: tr.kin_cosine_exponent,
        kin_threshold: tr.kin_threshold,
    });
    let velocity_alignment_params = velocity_alignment.map(|va| VelocityAlignmentSweepParams {
        strength: va.strength,
    });
    work.relax_v2_features(
        &rules.grid,
        bodies,
        ContactV2Sweep {
            pcp: pcp_params,
            emt_gate: needs_emt_factors,
            notch_delta,
            cadherin,
            trophallaxis: trophallaxis_params,
            velocity_alignment: velocity_alignment_params,
        },
        workers,
    )
}

/// Phases 6 and 7 for one organism: chemistry, maintenance, motion,
/// emission and the reproduction proposal, rechecking exhaustion after
/// each, with each stage's settlement from `stages`. Once a stage exhausts
/// the organism nothing later pays or acts. For the watched organism,
/// `record` is filled with each stage's settlement as the ledger is given
/// it. The organism's row is read, not written: the caller applies the
/// returned [`Settled`].
#[expect(
    clippy::too_many_arguments,
    reason = "ordered phases 6 and 7 stage runner including optional V2-11 byproducts, V2-15 trail deposits and paid accumulator"
)]
fn settle_stages(
    rules: &Rules,
    organism: &Working,
    traits: &Traits,
    stages: &mut impl Stages,
    from: Tick,
    ledger: &mut impl Ledger,
    emissions: &mut Deposits,
    trail_emissions: Option<&mut Deposits>,
    byproducts: Option<&mut Deposits>,
    emission_paid: &mut CompensatedSum,
    exchange_scratch: &mut ExchangeScratch,
    record: Option<&mut Option<Transactions>>,
) -> Result<Settled, CoreError> {
    let mut settled = Settled::of(organism);
    let mut footprint = CachedFootprint::new();
    let opening = settled.budget.remaining();
    let uptake = stages.uptake(rules, organism, &mut settled.budget)?;
    let mut byproduct_staged = 0.0;
    if let (Some(waste_rule), Some(byproducts)) = (rules.v2.waste(), byproducts) {
        let unconverted = sub_down(uptake.absorbed, uptake.gain).max(0.0);
        let candidate = (waste_rule.byproduct_recycle_fraction * unconverted).min(unconverted);
        let retained_uptake = uptake.absorbed - candidate;
        if candidate > 0.0 && retained_uptake >= uptake.gain {
            let recurrent_h0 = organism.held.recurrent_state_array()[0];
            let a_eff = effective_affinity(
                traits,
                recurrent_h0,
                &rules.v2,
                rules.chemistry.affinity_norm_limit(),
            )?;
            let waste_dir = symplectic_waste_direction(
                &a_eff,
                rules.v2.active_carrier0_channels(),
                rules.chemistry.affinity_norm_limit(),
            )?;
            footprint.push_deposit(
                byproducts,
                &rules.grid,
                rules.exchange,
                organism.committed.id,
                organism.committed.position,
                candidate,
                candidate,
                &waste_dir,
                exchange_scratch,
            )?;
            ledger.transfer(Transfer::Uptake, retained_uptake, uptake.gain)?;
            ledger.record(Flow::Toxicity, uptake.toxicity.paid)?;
            byproduct_staged = candidate;
        } else {
            uptake.record(ledger)?;
        }
    } else {
        uptake.record(ledger)?;
    }
    let absorbed = uptake.absorbed;
    let mut record = record.map(|slot| {
        slot.insert(Transactions {
            tick: from,
            opening,
            uptake,
            byproduct_staged,
            maintenance: None,
            motion: None,
            emission: None,
            reproduction: None,
            tiny: 0.0,
            trophallaxis_delta: 0.0,
            closing: 0.0,
        })
    });
    if !settled
        .vitality
        .check(BudgetStage::Chemistry, &settled.budget)?
    {
        return Ok(settled);
    }
    let maintenance = stages.maintenance(rules, organism, &mut settled.budget)?;
    maintenance.record(ledger)?;
    if let Some(record) = record.as_deref_mut() {
        record.maintenance = Some(maintenance);
    }
    if !settled
        .vitality
        .check(BudgetStage::Maintenance, &settled.budget)?
    {
        return Ok(settled);
    }

    let moved = stages.movement(rules, organism, absorbed, &mut settled.budget)?;
    ledger.record(Flow::MotionCost, moved.cost.paid)?;
    if let Some(record) = record.as_deref_mut() {
        record.motion = Some(moved);
    }
    settled.velocity = moved.velocity;
    settled.position = stages.displace(rules, organism, moved.velocity)?;
    if !settled
        .vitality
        .check(BudgetStage::Motion, &settled.budget)?
    {
        return Ok(settled);
    }

    let (emission, next_readiness, next_cooldown_deadline) = stages.emission(
        rules,
        organism,
        settled.position,
        settled.velocity,
        from,
        &mut settled.budget,
        emissions,
        trail_emissions,
        exchange_scratch,
        &mut footprint,
    )?;
    if emission.cost.paid > 0.0 {
        emission_paid.add(emission.cost.paid);
    }
    settled.emitted = emission.emitted();
    settled.secretory_readiness = next_readiness;
    settled.emission_cooldown_deadline = next_cooldown_deadline;
    if let Some(record) = record.as_deref_mut() {
        record.emission = Some(emission);
    }
    if !settled
        .vitality
        .check(BudgetStage::Emission, &settled.budget)?
    {
        return Ok(settled);
    }

    let parent = ParentState {
        id: organism.committed.id,
        lineage: organism.committed.lineage,
        generation: organism.committed.generation,
        position: settled.position,
        cooldown_deadline: organism.cooldown_deadline,
    };
    let proposal = stages.propose(rules, organism, &parent, from, &settled.budget)?;
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
    if let ReproductionProposal::Intent(mut intent) = proposal {
        if let Some(diff) = rules.v2.differentiation() {
            let parent_state = organism.held.recurrent_state_array();
            intent.newborn_state = Reproduction::inherit_epigenetic_state(
                &rules.rng,
                intent.parent,
                intent.tick,
                &parent_state[..diff.state_width],
                diff,
            )?;
        }
        if rules.v2.polarity().is_some() {
            intent.newborn_polarity = Some(Reproduction::inherit_polarity(
                &rules.rng,
                intent.parent,
                intent.tick,
                organism.committed.polarity,
            )?);
        }
        let lateral_diameter = rules
            .v2
            .polarity()
            .filter(|pol| pol.oriented_lateral_mitosis)
            .and_then(|_| rules.contact.map(|bodies| bodies.diameter));
        let local_radius = rules.v2.budding().map(|b| b.local_offset_radius);
        if local_radius.is_some() || lateral_diameter.is_some() {
            let (lambda_epi, lateral_offset) = if let Some(diameter) = lateral_diameter {
                let alpha_pol = traits.polarity_strength().unwrap_or(0.0);
                let emt_eff = effective_emt_factor(rules, organism);
                let lambda = (alpha_pol * emt_eff.min(1.0)).clamp(0.0, 1.0);
                if lambda > 0.0 {
                    let offset = Reproduction::lateral_mitosis_offset(
                        &rules.rng,
                        intent.parent,
                        intent.tick,
                        organism.committed.polarity,
                        diameter,
                        alpha_pol,
                    )?;
                    (lambda, offset)
                } else {
                    (0.0, [0.0, 0.0])
                }
            } else {
                (0.0, [0.0, 0.0])
            };
            if local_radius.is_some() || lambda_epi > 0.0 {
                intent.budding_offset = Some(rules.reproduction.budding_child_offset(
                    &rules.rng,
                    intent.parent,
                    intent.tick,
                    local_radius,
                    organism.actions.budding,
                    organism.gradients[0],
                    settled.velocity,
                    lambda_epi,
                    lateral_offset,
                )?);
            }
        }
        settled.intent = Some(intent);
    }
    Ok(settled)
}
