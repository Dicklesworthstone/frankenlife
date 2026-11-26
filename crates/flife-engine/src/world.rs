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
    FluxEdges, GrantPacket, Grants, PyramidView, RawSample, RequestRule, SEEDED_SMOOTH_TORUS_V1,
    SEEDED_UNIT_VECTORS, SeededPath, SensorBackend, SensorPyramid, SensorQuery, SettleBackend,
    Source, SourceKernel, TransportPlan, UptakeBackend, UptakeSummary, UptakeWork,
    advance_field_on, advance_field_v2_on, advance_field_wave2_on,
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
    SensorLayout, SensorScales, decode_with_layout, decode_with_layout_f32_into,
    observe_dual_for_controller, observe_dual_for_controller_f32_into,
    observe_dual_gradients_only_into, observe_for_controller, observe_for_controller_f32_into,
    observe_gradients_only_into, update_recurrent_state, widen_slice,
};

use crate::ancestry::{Ancestry, AncestryHeld, Followed};
use crate::chemistry::{
    Affinity, Chemistry, UptakeSettlement, composition, corpse_affinity_direction,
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
        world.population.apply_checked_tick_on(
            staged.checked,
            &world.scratch.survivors,
            &mut staged.births,
            &mut staged.quota,
            released,
            world.scratch.lanes.len(),
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
        decision: &Decision,
    ) -> Result<f64, CoreError> {
        match self.v2.crowding() {
            None => Ok(decision.actions.attempt_probability),
            Some(crowding) => hippo_attempt_probability(
                decision.actions.attempt_rate,
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
        decision: &Decision,
        position: [f64; 2],
        velocity: [f64; 2],
        from: Tick,
    ) -> Result<EmissionContext, CoreError> {
        let outputs = decision
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
            energy_gradient: decision.gradients[0],
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

/// Precomputed Phase-6 chemistry settlement and post-chemistry budget for one organism.
type PrecomputedUptake = (UptakeSettlement, EnergyBudget);

/// Spatial macro-tile order and precomputed Phase-6 chemistry scratch handed to
/// [`settle::settle_with`].
struct SpatialSettle<'a> {
    rows: &'a [u32],
    row_to_spatial: &'a [u32],
    tile_offsets: &'a [usize; SPATIAL_TILES + 1],
    uptake: &'a mut Vec<PrecomputedUptake>,
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
    /// Phases 3 and 4 decisions, shared across dynamic work-stealing tiles so
    /// `lanes` only hold per-worker scratch. Stored in identity order when
    /// `row_to_spatial` is empty, or in spatial macro-tile order indexed by
    /// `row_to_spatial[row_idx]` on large multi-worker flat-F32 ticks.
    decisions: Vec<Decision>,
    /// Row indices partitioned into 8×8 spatial macro-tiles during large
    /// multi-worker flat-F32 Pass 1/Pass 2 ticks (`capacity >= 8,192` and
    /// `cells >= 32,768`; empty / 0 bytes otherwise).
    spatial_rows: Vec<u32>,
    /// Inverse map `row_idx -> spatial_pos` into `decisions` when spatial
    /// Pass 1/Pass 2 ran on the current tick; empty when `decisions` is in
    /// identity order.
    row_to_spatial: Vec<u32>,
    /// Macro-tile prefix offsets into `spatial_rows` from the last spatial
    /// Pass 1 (`[0; SPATIAL_TILES + 1]` when spatial Pass 1 did not run).
    spatial_tile_offsets: [usize; SPATIAL_TILES + 1],
    /// Precomputed Phase-6 chemistry settlements in spatial macro-tile order
    /// (`spatial_pos -> (UptakeSettlement, EnergyBudget)`), reserved alongside
    /// `spatial_rows` on large worlds (`0` bytes otherwise) so Phase-6
    /// chemistry streams `compositions` inside L2-resident 1 MiB tiles.
    spatial_uptake: Vec<(UptakeSettlement, EnergyBudget)>,
    /// One per CPU worker (`[execution] cpu_workers`).
    lanes: Vec<DecisionLane>,
    /// Pre-reserved dynamic work-stealing settle tiles for large multi-worker
    /// `grid-cic-v1` worlds (`capacity ≥ 24,576`), eliminating per-tick
    /// `LedgerLog` / `Deposits` alloc/free churn and the intermediate tile-to-lane
    /// merge copy. Empty (`0` bytes) on smaller or single-worker worlds.
    settle_tiles: Vec<SettleTileBuf>,
    sampled: Vec<Source>,
    working: Vec<Working>,
    demands: Vec<Demand>,
    grants: Grants,
    /// Each organism's grant packets, grouped by demand in cell order.
    by_demand: Bins,
    /// Deduplicated cell compositions (at most `min(cells, packets)` unique
    /// debited cells per tick, phase 6) indexed by `packet_compositions`.
    compositions: Vec<[f64; CHANNELS]>,
    /// Per-packet index into `compositions` (`4 B` per packet vs `256 B`,
    /// eliminating per-packet broadcast copies in phase 6).
    packet_compositions: Vec<u32>,
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
    /// Compact `f64` survivor energies filled by Phase 8 `BuildTile` so the
    /// canonical compensated organism energy sum scans `8 B/org` in L2 instead
    /// of `424 B/org` across `survivors`.
    survivor_energies: Vec<f64>,
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

impl Decision {
    fn empty() -> Self {
        Self {
            held: HeldOutput::INVALID,
            evaluated: false,
            actions: DecodedActions::default(),
            gradients: [[0.0; 2]; RESPONSE_TERMS],
        }
    }
}

/// Queries sampled per spatial pyramid batch on a decision lane. Sized to
/// match [`PyramidView::sample_batch_each`]'s 512-query L1D block.
const SAMPLE_BATCH: usize = 512;

/// One CPU worker's scratch for phases 3 and 4, and 6 and 7.
#[derive(Debug, Default)]
struct DecisionLane {
    controller: ControllerScratch,
    /// Footprint scratch for phase 5 demand and phase 6/7 deposit computation
    /// under fixed-world exchange kernels (FL-053; empty / 0 bytes under
    /// `grid-cic-v1`).
    exchange: ExchangeScratch,
    /// Legacy batched path's observations of this lane's run.
    observed: Vec<Observation>,
    /// Expanded controller input vectors for legacy batched runs whose layout
    /// exceeds baseline `SENSOR_WIDTH` (recurrent or v2 extra sensors); empty
    /// (0 bytes) on baseline 104-input worlds.
    controller_inputs: Vec<[f64; MAX_SENSOR_WIDTH]>,
    /// Start index and count of this lane's due jobs inside a legacy batched tick.
    due_offset: usize,
    due_count: usize,
    /// Phases 6 and 7 of this lane's run: one organism's `(granted, comp_idx)`
    /// packet pairs when an exchange footprint exceeds 4 cells, the run's
    /// ledger flows and staged emissions in identity order (empty when large-
    /// world `settle_tiles` owns staging), and the watched organism's
    /// transactions if the run holds it.
    packets: Vec<(f64, u32)>,
    flows: LedgerLog,
    emissions: Deposits,
    /// Dedicated Carrier-1 trail emission staging (`V2-15`); `None` (0 bytes)
    /// when disabled.
    trail_emissions: Option<Deposits>,
    /// Dedicated symplectic metabolic waste staging (`V2-11`); `None` (0 bytes)
    /// when disabled.
    byproducts: Option<Deposits>,
    transactions: Option<Transactions>,
}

/// Minimum live / capacity organism count for dynamic work-stealing settle
/// tiles (`settle_all`), avoiding heterogeneous P/E-core tail stragglers on
/// large multi-worker worlds while keeping `0` bytes on small reference worlds.
const SETTLE_TILE_THRESHOLD: usize = 24_576;

/// Reusable staging buffers for one dynamic work-stealing settle tile.
#[derive(Debug, Default)]
pub(super) struct SettleTileBuf {
    rows: usize,
    flows: LedgerLog,
    emissions: Deposits,
    trail_emissions: Option<Deposits>,
    byproducts: Option<Deposits>,
    transactions: Option<Transactions>,
}

impl SettleTileBuf {
    fn allocated_bytes(&self) -> u64 {
        total([
            self.flows.allocated_bytes(),
            self.emissions.allocated_bytes(),
            self.trail_emissions
                .as_ref()
                .map_or(0, Deposits::allocated_bytes),
            self.byproducts
                .as_ref()
                .map_or(0, Deposits::allocated_bytes),
        ])
    }

    fn fit(&mut self, rows: usize, has_trail: bool, has_byproducts: bool) -> Result<(), CoreError> {
        self.flows.fit(rows.saturating_mul(FLOWS_PER_ORGANISM))?;
        self.emissions.fit_staging(rows)?;
        if has_trail {
            let trail = self.trail_emissions.get_or_insert_with(Deposits::new);
            trail.fit_staging(rows)?;
        } else {
            self.trail_emissions = None;
        }
        if has_byproducts {
            let byproducts = self.byproducts.get_or_insert_with(Deposits::new);
            byproducts.fit_staging(rows)?;
        } else {
            self.byproducts = None;
        }
        self.transactions = None;
        self.rows = rows;
        Ok(())
    }

    fn clear(&mut self) {
        self.flows.clear();
        self.emissions.clear();
        if let Some(trail) = self.trail_emissions.as_mut() {
            trail.clear();
        }
        if let Some(byproducts) = self.byproducts.as_mut() {
            byproducts.clear();
        }
        self.transactions = None;
    }
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
///
/// Compact committed organism metadata held inside [`Working`], omitting the
/// 280-byte `HeldOutput` (already stored in `Scratch::decisions` / `rows`) and
/// fields copied into `Working`'s mutable state. Shrinks `Working` by >2.2×
/// and removes 280-byte `_platform_memmove` copies from `GatherTile`.
#[derive(Debug, Clone, Copy)]
struct CommittedRow {
    id: OrganismId,
    genome: GenomeId,
    parent: Option<OrganismId>,
    lineage: LineageId,
    generation: Generation,
    birth_tick: Tick,
    position: [f64; 2],
    energy: f64,
    phase: u32,
    polarity: [f64; 2],
}

impl CommittedRow {
    #[inline]
    const fn from_organism(organism: &Organism) -> Self {
        Self {
            id: organism.id,
            genome: organism.genome,
            parent: organism.parent,
            lineage: organism.lineage,
            generation: organism.generation,
            birth_tick: organism.birth_tick,
            position: organism.position,
            energy: organism.energy,
            phase: organism.phase,
            polarity: organism.polarity,
        }
    }
}

/// Read-only Phase-3/4 outputs (`held`, `evaluated`, `actions`, `gradients` —
/// ~1.2 KiB per organism) stay in `Scratch::decisions[decision_index]` instead
/// of being copied into `Working`, shrinking `Working` by ~2.7× and eliminating
/// random 1.2 KiB gathers from `decisions` during `GatherTile`.
#[derive(Debug, Clone, Copy)]
struct Working {
    /// Its committed metadata at tick `n` (without the 280-byte `HeldOutput`).
    committed: CommittedRow,
    budget: EnergyBudget,
    vitality: Vitality,
    /// Index into `Scratch::decisions` for this organism's read-only `Decision`.
    decision_index: u32,
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
    /// Exact emission payment settled by phase 7 for this organism, summed in
    /// canonical ascending identity order when dual-carrier trails are active
    /// so `Transfer::Emission` is independent of worker count and tiling.
    emission_paid: f64,
    /// Residual energy captured by `V2-11` viability-floor lysis in Phase 8
    /// `clear_tiny`, handed to `Recycling` without `Flow::TinyReservoir`.
    lysed_residual: f64,
    intent: Option<BirthIntent>,
}

impl Working {
    /// This organism's read-only Phase-3/4 [`Decision`].
    #[inline]
    fn decision<'a>(&self, decisions: &'a [Decision]) -> &'a Decision {
        &decisions[self.decision_index as usize]
    }
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
        let old_workers = NonZeroUsize::new(self.scratch.lanes.len()).unwrap_or(NonZeroUsize::MIN);
        let target = workers.get();
        let legacy_batched = self
            .controllers
            .backend
            .as_deref()
            .is_some_and(|backend| !backend.supports_flat_f32());
        let capacity = capacity_of(&self.rules)?;
        let old_use_tiles = uses_settle_tiles(
            old_workers.get(),
            &self.rules,
            capacity,
            self.settle.backend.is_some(),
        );
        let use_tiles =
            uses_settle_tiles(target, &self.rules, capacity, self.settle.backend.is_some());
        self.scratch.field.set_workers(workers)?;
        let mut added = Vec::new();
        for _ in self.scratch.lanes.len()..target {
            match lane(&self.rules) {
                Ok(entry) => added.push(entry),
                Err(error) => {
                    let _ = self.scratch.field.set_workers(old_workers);
                    return Err(error);
                }
            }
        }
        // When switching onto `settle_tiles`, reserve tiles BEFORE mutating
        // `lanes` so an allocation failure leaves worker count and lanes intact.
        if use_tiles
            && let Err(error) = fit_settle_tiles(
                &mut self.scratch.settle_tiles,
                target,
                &self.rules,
                capacity,
                self.settle.backend.is_some(),
            )
        {
            let _ = self.scratch.field.set_workers(old_workers);
            let _ = fit_settle_tiles(
                &mut self.scratch.settle_tiles,
                old_workers.get(),
                &self.rules,
                capacity,
                self.settle.backend.is_some(),
            );
            return Err(error);
        }
        let lanes = &mut self.scratch.lanes;
        let old_len = lanes.len();
        if target > old_len && lanes.try_reserve_exact(target - old_len).is_err() {
            let _ = self.scratch.field.set_workers(old_workers);
            let _ = fit_settle_tiles(
                &mut self.scratch.settle_tiles,
                old_workers.get(),
                &self.rules,
                capacity,
                self.settle.backend.is_some(),
            );
            return Err(CoreError::AllocationFailed {
                buffer: "decision lanes",
                bytes: bytes_for::<DecisionLane>(target),
            });
        }
        // Hold any trimmed lanes until `fit_lanes` succeeds so an OOM while
        // resizing per-lane buffers restores `lanes`, `field`, and
        // `settle_tiles` without shrinking `lanes` capacity or losing workers.
        let removed = if target < old_len {
            lanes.split_off(target)
        } else {
            lanes.extend(added);
            Vec::new()
        };
        // A lane's share of the rows follows the count (FL-043). Only legacy
        // non-flat-F32 backends use per-lane `observed` / `controller_inputs`.
        if let Err(error) = fit_lanes(lanes, &self.rules, capacity, legacy_batched, !use_tiles) {
            if target < old_len {
                lanes.extend(removed);
            } else {
                lanes.truncate(old_len);
                lanes.shrink_to(old_len);
            }
            let _ = fit_lanes(lanes, &self.rules, capacity, legacy_batched, !old_use_tiles);
            let _ = self.scratch.field.set_workers(old_workers);
            let _ = fit_settle_tiles(
                &mut self.scratch.settle_tiles,
                old_workers.get(),
                &self.rules,
                capacity,
                self.settle.backend.is_some(),
            );
            return Err(error);
        }
        drop(removed);
        lanes.shrink_to(target);
        if !use_tiles {
            fit_settle_tiles(
                &mut self.scratch.settle_tiles,
                target,
                &self.rules,
                capacity,
                self.settle.backend.is_some(),
            )?;
        }
        Ok(())
    }

    /// Evaluate due controllers in one batch on `backend` from the next
    /// tick, or on the reference alone with `None`. No result depends on
    /// it: a backend returns the reference's outputs bit for bit and defers
    /// what it cannot reproduce, and the reference evaluates those
    /// ([`flife_neural::backend`]). A backend that fails is dropped, and
    /// [`World::controller_report`] says why.
    ///
    /// Installing a backend reserves only the batched path's lists it actually
    /// uses for the population's capacity (FL-043): compact flat-F32 buffers
    /// ([`World::flat_f32_batched_bytes`]) when `backend.supports_flat_f32()`,
    /// or legacy `RawOutputs` and per-lane `Observation` buffers
    /// ([`World::batched_bytes`]) otherwise. If the host refuses them, the
    /// backend is not installed, and the report says why.
    pub fn set_controller_backend(&mut self, backend: Option<Box<dyn ControllerBackend>>) {
        let reserved = match backend.as_deref() {
            Some(device) => {
                let flat_f32 = device.supports_flat_f32();
                capacity_of(&self.rules).and_then(|rows| self.reserve_batched(rows, flat_f32))
            }
            None => {
                self.release_batched();
                Ok(())
            }
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

    /// Reserve the batched path's lists for `rows` rows: either compact
    /// flat-F32 staging/output buffers (`flat_f32 == true`, freeing legacy
    /// `outputs` and per-lane `observed` / `controller_inputs`) or legacy
    /// `RawOutputs` + per-lane observations (`flat_f32 == false`, freeing
    /// flat-F32 buffers). Never disturbs lane settle buffers or `settle_tiles`.
    fn reserve_batched(&mut self, rows: usize, flat_f32: bool) -> Result<(), CoreError> {
        if flat_f32 {
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
            controllers.outputs = Vec::new();
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
            fit_lane_batched(&mut self.scratch.lanes, &self.rules, rows, false)
        } else {
            let controllers = &mut self.controllers;
            controllers.flat_inputs_f32 = Vec::new();
            controllers.flat_inputs_finite = Vec::new();
            controllers.flat_outputs = FlatOutputs::default();
            fit(&mut controllers.outputs, rows, "controller outputs")?;
            fit_lane_batched(&mut self.scratch.lanes, &self.rules, rows, true)
        }
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
        let legacy_batched = self
            .controllers
            .backend
            .as_deref()
            .is_some_and(|ctrl| !ctrl.supports_flat_f32());
        let workers = self.scratch.lanes.len();
        self.settle.report.backend = backend.as_ref().map(|backend| backend.describe());
        self.settle.report.failure = None;
        self.settle.work = settle::SettleWork::default();
        self.settle.backend = None;
        if backend.is_none() {
            if let Ok(capacity) = capacity_of(&self.rules) {
                let use_tiles = uses_settle_tiles(workers, &self.rules, capacity, false);
                if fit_settle_tiles(
                    &mut self.scratch.settle_tiles,
                    workers,
                    &self.rules,
                    capacity,
                    false,
                )
                .is_ok()
                {
                    let _ = fit_lanes(
                        &mut self.scratch.lanes,
                        &self.rules,
                        capacity,
                        legacy_batched,
                        !use_tiles,
                    );
                }
            }
            return;
        }
        let prepared = capacity_of(&self.rules).and_then(|rows| {
            self.settle
                .work
                .reserve(rows, self.rules.v2.needs_motion_v2())?;
            fit_lanes(
                &mut self.scratch.lanes,
                &self.rules,
                rows,
                legacy_batched,
                true,
            )?;
            fit_settle_tiles(
                &mut self.scratch.settle_tiles,
                workers,
                &self.rules,
                rows,
                true,
            )
        });
        if let Err(error) = prepared {
            self.settle.report.failure = Some(error.to_string());
            self.settle.work = settle::SettleWork::default();
            if let Ok(capacity) = capacity_of(&self.rules) {
                let use_tiles = uses_settle_tiles(workers, &self.rules, capacity, false);
                if fit_settle_tiles(
                    &mut self.scratch.settle_tiles,
                    workers,
                    &self.rules,
                    capacity,
                    false,
                )
                .is_ok()
                {
                    let _ = fit_lanes(
                        &mut self.scratch.lanes,
                        &self.rules,
                        capacity,
                        legacy_batched,
                        !use_tiles,
                    );
                }
            }
            return;
        }
        self.settle.backend = backend;
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

    /// What installing a legacy (non-flat-F32) controller backend reserves on
    /// top of [`World::footprint`] (FL-043): its `RawOutputs` list for the
    /// population's capacity, and each lane's `Observation` (plus expanded
    /// input vectors when the controller layout exceeds `SENSOR_WIDTH`) for
    /// the most rows it can take.
    #[must_use]
    pub fn batched_bytes(&self) -> u64 {
        let organisms = self.population.capacity();
        let lanes = self.scratch.lanes.len();
        let rows = lane_rows(organisms, lanes);
        let input_width = self.rules.v2.sensors().input_width();
        let expanded = if input_width > SENSOR_WIDTH {
            bytes_for::<[f64; MAX_SENSOR_WIDTH]>(rows).saturating_mul(lanes as u64)
        } else {
            0
        };
        total([
            bytes_for::<Option<RawOutputs>>(organisms),
            bytes_for::<Observation>(rows).saturating_mul(lanes as u64),
            expanded,
        ])
    }

    /// What installing a flat-F32 controller backend
    /// (`ControllerBackend::supports_flat_f32() == true`, such as `CpuBackend`
    /// or `MetalBackend`) reserves on top of [`World::footprint`] (FL-043):
    /// contiguous F32 inputs, input-finiteness flags, and `FlatOutputs` for
    /// the population's capacity (omitting the unused legacy `RawOutputs` and
    /// per-lane `Observation` buffers).
    #[must_use]
    pub fn flat_f32_batched_bytes(&self) -> u64 {
        let organisms = self.population.capacity();
        let input_width = self.rules.v2.sensors().input_width();
        let raw_output_width = self.rules.v2.actions().raw_output_width();
        total([
            bytes_for::<f32>(organisms.saturating_mul(input_width)),
            bytes_for::<u8>(organisms),
            bytes_for::<f32>(organisms.saturating_mul(raw_output_width)),
            bytes_for::<u8>(organisms),
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
                capacity_bytes(&lane.observed),
                capacity_bytes(&lane.controller_inputs),
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
                capacity_bytes(&scratch.decisions),
                capacity_bytes(&scratch.spatial_rows),
                capacity_bytes(&scratch.row_to_spatial),
                capacity_bytes(&scratch.spatial_uptake),
                capacity_bytes(&scratch.lanes),
                capacity_bytes(&scratch.settle_tiles),
                capacity_bytes(&scratch.sampled),
                capacity_bytes(&scratch.working),
                capacity_bytes(&scratch.demands),
                scratch.grants.allocated_bytes(),
                scratch.by_demand.allocated_bytes(),
                capacity_bytes(&scratch.compositions),
                capacity_bytes(&scratch.packet_compositions),
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
                capacity_bytes(&scratch.survivor_energies),
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
            .chain(lanes)
            .chain(
                scratch
                    .settle_tiles
                    .iter()
                    .map(SettleTileBuf::allocated_bytes),
            ),
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
        let effective_affinity = if self.rules.v2.affinity_rotation_enabled() {
            Some(effective_affinity(
                genome.traits(),
                organism.held.recurrent_state_array()[0],
                &self.rules.v2,
                self.rules.chemistry.affinity_norm_limit(),
            )?)
        } else {
            None
        };
        let secretory_readiness = self
            .rules
            .v2
            .emission()
            .excitable
            .is_some()
            .then(|| f64::from(organism.secretory_readiness));
        let last_neighbor_h0 = self
            .rules
            .v2
            .notch_delta()
            .is_some()
            .then(|| f64::from(organism.last_neighbor_h0));
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
            secretory_readiness,
            effective_affinity,
            last_neighbor_h0,
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
        let mut stream = flife_core::canonical::Sha256Stream::new();
        let mut buf = Vec::with_capacity(4096);
        let flush = |stream: &mut flife_core::canonical::Sha256Stream, buf: &mut Vec<u8>| {
            if !buf.is_empty() {
                stream.update(buf);
                buf.clear();
            }
        };
        buf.extend_from_slice(WORLD_DIGEST_SCHEMA.as_bytes());
        buf.push(b'\n');
        buf.extend_from_slice(&self.tick.get().to_le_bytes());
        for &value in self.field.current().as_slice() {
            buf.extend_from_slice(&value.to_bits().to_le_bytes());
            if buf.len() >= 4096 {
                flush(&mut stream, &mut buf);
            }
        }
        if let Some((gx, gy)) = self.field.current().vascular_edges() {
            for &value in gx.iter().chain(gy.iter()) {
                buf.extend_from_slice(&value.to_bits().to_le_bytes());
                if buf.len() >= 4096 {
                    flush(&mut stream, &mut buf);
                }
            }
        }
        flush(&mut stream, &mut buf);
        buf.extend_from_slice(self.population.digest().as_bytes());
        for next in [
            self.identities.organisms.next_raw(),
            self.identities.genomes.next_raw(),
        ] {
            match next {
                Some(raw) => {
                    buf.push(1);
                    buf.extend_from_slice(&raw.to_le_bytes());
                }
                None => buf.push(0),
            }
        }
        for value in [
            self.quota.logical_bytes(),
            self.quota.allocated_bytes(),
            self.quota.live(),
        ] {
            buf.extend_from_slice(&value.to_le_bytes());
        }
        for path in &self.paths {
            buf.extend_from_slice(&path.source().get().to_le_bytes());
            buf.extend_from_slice(&path.ordinal().to_le_bytes());
            if buf.len() >= 4096 {
                flush(&mut stream, &mut buf);
            }
        }
        if !self.source_kernels.is_empty() {
            buf.extend_from_slice(&(self.source_kernels.len() as u64).to_le_bytes());
            for kernel in &self.source_kernels {
                buf.extend_from_slice(&kernel.sigma().to_bits().to_le_bytes());
                if buf.len() >= 4096 {
                    flush(&mut stream, &mut buf);
                }
            }
        }
        let holdings = self.ledger.holdings();
        for value in [holdings.field(), holdings.organisms()] {
            buf.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        flush(&mut stream, &mut buf);
        stream.finish()
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
            decisions,
            spatial_rows,
            row_to_spatial,
            spatial_tile_offsets,
            spatial_uptake,
            lanes,
            settle_tiles,
            sampled,
            working,
            demands,
            grants,
            by_demand,
            compositions,
            packet_compositions,
            emissions,
            trail_emissions,
            byproducts,
            recycling,
            survivors,
            survivor_energies,
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
        population.copy_rows_into_on(rows, lanes.len())?;
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
        // When an off-CPU controller backend (e.g. Metal GPU) runs on >= 2
        // workers and Phase 5 stays on CPU, overlap Phase 5 uptake (+ Phase 6
        // fused spatial chemistry prepass when Phase 6 stays on CPU) behind
        // the GPU controller dispatch inside the single worker budget.
        let view = pyramid.build_on(observed, field_work.workers())?;
        let (sampled, sensor_failure) = sample_with(population, rows, &view, observed, sensors);
        let mut overlap = (uptake_state.backend.is_none() && lanes.len() >= 2).then(|| {
            OverlapPhase56 {
                observed,
                demands,
                grants,
                by_demand,
                compositions,
                packet_compositions,
                spatial_uptake,
                run_fused_chem: settle_state.backend.is_none(),
                uptake_result: None,
                fused_chem_done: false,
            }
        });
        let evaluated = decide_with(
            rules,
            population,
            &view,
            sampled,
            rows,
            crowding_counts,
            [from, to],
            decisions,
            spatial_rows,
            row_to_spatial,
            spatial_tile_offsets,
            lanes,
            controllers,
            overlap.as_mut(),
        )?;
        let (precomputed_uptake, precomputed_fused_chem) = match overlap {
            Some(ov) => (
                ov.uptake_result,
                ov.fused_chem_done && !row_to_spatial.is_empty(),
            ),
            None => (None, false),
        };
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
        if decisions.len() != rows.len() {
            return Err(CoreError::StateConflict {
                operation: "gather decisions",
                detail: format!("{} decisions for {} organisms", decisions.len(), rows.len()),
            });
        }
        if u32::try_from(decisions.len()).is_err() {
            return Err(CoreError::Overflow {
                quantity: "decision indices",
            });
        }
        let spatial_decisions = !row_to_spatial.is_empty();
        if spatial_decisions && row_to_spatial.len() != rows.len() {
            return Err(CoreError::StateConflict {
                operation: "gather decisions",
                detail: format!(
                    "{} spatial decision indices for {} organisms",
                    row_to_spatial.len(),
                    rows.len()
                ),
            });
        }
        let decision_count = decisions.len();
        if rows.len() >= 512 && lanes.len() > 1 {
            let placeholder = Working {
                committed: CommittedRow::from_organism(&rows[0]),
                budget: EnergyBudget::new(0.0)?,
                vitality: Vitality::new(),
                decision_index: 0,
                basal_multiplier: 1.0,
                local_crowding: 0,
                position: [0.0, 0.0],
                velocity: [0.0, 0.0],
                cooldown_deadline: Tick::new(0),
                secretory_readiness: 0.0,
                emission_cooldown_deadline: Tick::new(0),
                emitted: false,
                emission_paid: 0.0,
                lysed_residual: 0.0,
                intent: None,
            };
            set_len_reuse(working, rows.len(), placeholder);
            let tile_count = lanes
                .len()
                .saturating_mul(4)
                .min(64)
                .min(rows.len().div_ceil(512))
                .max(1);
            let tile_len = rows.len().div_ceil(tile_count);
            struct GatherTile<'a> {
                start: usize,
                rows: &'a [Organism],
                spatial_map: &'a [u32],
                out: &'a mut [Working],
            }
            let mut tiles: Vec<GatherTile<'_>> = rows
                .chunks(tile_len)
                .zip(working.chunks_mut(tile_len))
                .enumerate()
                .map(|(idx, (row_c, out_c))| {
                    let start = idx * tile_len;
                    let end = start + row_c.len();
                    GatherTile {
                        start,
                        rows: row_c,
                        spatial_map: if spatial_decisions {
                            &row_to_spatial[start..end]
                        } else {
                            &[]
                        },
                        out: out_c,
                    }
                })
                .collect();
            flife_core::pool::try_parallel_tasks(
                lanes.len(),
                "gather decisions",
                &mut tiles,
                |_, tile| {
                    let mapped = !tile.spatial_map.is_empty();
                    for (i, (organism, slot)) in
                        tile.rows.iter().zip(tile.out.iter_mut()).enumerate()
                    {
                        let dec_idx = if mapped {
                            tile.spatial_map[i]
                        } else {
                            #[expect(
                                clippy::cast_possible_truncation,
                                reason = "decisions.len() <= u32::MAX checked above"
                            )]
                            {
                                (tile.start + i) as u32
                            }
                        };
                        if (dec_idx as usize) >= decision_count {
                            return Err(CoreError::StateConflict {
                                operation: "gather decisions",
                                detail: format!(
                                    "decision index {dec_idx} out of bounds for {decision_count}"
                                ),
                            });
                        }
                        let basal_multiplier =
                            rules
                                .v2
                                .basal_multiplier(organism.birth_tick, from, rules.step)?;
                        let local_crowding =
                            crowding_counts.get(tile.start + i).copied().unwrap_or(0);
                        slot.committed = CommittedRow::from_organism(organism);
                        slot.budget = EnergyBudget::new(organism.energy)?;
                        slot.vitality = Vitality::new();
                        slot.decision_index = dec_idx;
                        slot.basal_multiplier = basal_multiplier;
                        slot.local_crowding = local_crowding;
                        slot.position = organism.position;
                        slot.velocity = organism.velocity;
                        slot.cooldown_deadline = organism.cooldown_deadline;
                        slot.secretory_readiness = organism.secretory_readiness;
                        slot.emission_cooldown_deadline = organism.emission_cooldown_deadline;
                        slot.emitted = false;
                        slot.emission_paid = 0.0;
                        slot.lysed_residual = 0.0;
                        slot.intent = None;
                    }
                    Ok(())
                },
            )?;
        } else {
            working.clear();
            for (row_index, organism) in rows.iter().enumerate() {
                let dec_idx = if spatial_decisions {
                    row_to_spatial[row_index]
                } else {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "decisions.len() <= u32::MAX checked above"
                    )]
                    {
                        row_index as u32
                    }
                };
                if (dec_idx as usize) >= decision_count {
                    return Err(CoreError::StateConflict {
                        operation: "gather decisions",
                        detail: format!(
                            "decision index {dec_idx} out of bounds for {decision_count}"
                        ),
                    });
                }
                let basal_multiplier =
                    rules
                        .v2
                        .basal_multiplier(organism.birth_tick, from, rules.step)?;
                let local_crowding = crowding_counts.get(row_index).copied().unwrap_or(0);
                working.push(Working {
                    committed: CommittedRow::from_organism(organism),
                    budget: EnergyBudget::new(organism.energy)?,
                    vitality: Vitality::new(),
                    decision_index: dec_idx,
                    basal_multiplier,
                    local_crowding,
                    position: organism.position,
                    velocity: organism.velocity,
                    cooldown_deadline: organism.cooldown_deadline,
                    secretory_readiness: organism.secretory_readiness,
                    emission_cooldown_deadline: organism.emission_cooldown_deadline,
                    emitted: false,
                    emission_paid: 0.0,
                    lysed_residual: 0.0,
                    intent: None,
                });
            }
        }

        // Phase 5: requests from the observation field, resolved per cell,
        // on the uptake backend if the world has one (or already completed
        // during the off-CPU controller overlap window).
        let uptake = match precomputed_uptake {
            Some(result) => result?,
            None => {
                let summary = uptake_with(
                    rules,
                    rows,
                    observed,
                    demands,
                    grants,
                    uptake_state,
                    lanes,
                )?;
                // The packets are in (cell, identity) order, so grouping them by
                // demand keeps each organism's in cell order.
                let granted = grants.packets();
                by_demand.group_on(demands.len(), granted.len(), lanes.len(), |at| {
                    granted.get(at).map_or(usize::MAX, |packet| packet.demand)
                })?;
                summary
            }
        };
        let granted = grants.packets();

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
        let active_settle_tiles = settle_with(
            rules,
            population,
            observed,
            (by_demand, granted, grants.bins()),
            (compositions, packet_compositions),
            SpatialSettle {
                rows: spatial_rows,
                row_to_spatial,
                tile_offsets: spatial_tile_offsets,
                uptake: spatial_uptake,
            },
            precomputed_fused_chem,
            from,
            watch.organism,
            decisions,
            working,
            lanes,
            settle_tiles,
            settle_state,
        )?;
        let mut total_emission_paid = CompensatedSum::ZERO;
        if trail_emissions.is_some() {
            for organism in working.iter() {
                if organism.emission_paid > 0.0 {
                    total_emission_paid.add(organism.emission_paid);
                }
            }
        }
        if active_settle_tiles > 0 || (lanes.len() > 1 && working.len() >= 512) {
            let src_len = if active_settle_tiles > 0 {
                active_settle_tiles
            } else {
                lanes.len()
            };
            let mut lane_emissions: Vec<&mut Deposits> = Vec::with_capacity(src_len);
            let mut lane_trails: Vec<&mut Deposits> = if trail_emissions.is_some() {
                Vec::with_capacity(src_len)
            } else {
                Vec::new()
            };
            let mut lane_byproducts: Vec<&mut Deposits> = if byproducts.is_some() {
                Vec::with_capacity(src_len)
            } else {
                Vec::new()
            };
            let merge_workers = lanes.len();
            let mut lane_flows: Vec<(&mut LedgerLog, Option<Transactions>)> =
                Vec::with_capacity(src_len);
            if active_settle_tiles > 0 {
                for tile in &mut settle_tiles[..active_settle_tiles] {
                    lane_emissions.push(&mut tile.emissions);
                    if let Some(src) = tile.trail_emissions.as_mut() {
                        lane_trails.push(src);
                    }
                    if let Some(src) = tile.byproducts.as_mut() {
                        lane_byproducts.push(src);
                    }
                    lane_flows.push((&mut tile.flows, tile.transactions.take()));
                }
            } else {
                for lane in lanes.iter_mut() {
                    lane_emissions.push(&mut lane.emissions);
                    if let Some(src) = lane.trail_emissions.as_mut() {
                        lane_trails.push(src);
                    }
                    if let Some(src) = lane.byproducts.as_mut() {
                        lane_byproducts.push(src);
                    }
                    lane_flows.push((&mut lane.flows, lane.transactions.take()));
                }
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
                for (flows, tx) in lane_flows {
                    if let Err(err) = flows.replay(&mut tick_ledger) {
                        flow_res = Err(err);
                        break;
                    }
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
                if let Some(record) = lane.transactions.take() {
                    transactions = Some(record);
                }
            }
        }

        // Phase 8: the last stages of organisms that do not reproduce, then
        // the births as one transaction, then the parents' last stages.
        let mut live = 0_u64;
        if working.len() >= 1_024 && lanes.len() > 1 {
            let tiny_threshold = rules.tiny.threshold();
            let tile_count = lanes
                .len()
                .saturating_mul(4)
                .min(64)
                .min(working.len().div_ceil(512))
                .max(1);
            let tile_len = working.len().div_ceil(tile_count);
            struct SplitTile<'a> {
                base: usize,
                chunk: &'a mut [Working],
                live_no_tiny: u64,
                tiny_indices: Vec<usize>,
            }
            let mut tiles: Vec<SplitTile<'_>> = working
                .chunks_mut(tile_len)
                .enumerate()
                .map(|(idx, chunk)| SplitTile {
                    base: idx * tile_len,
                    chunk,
                    live_no_tiny: 0,
                    tiny_indices: Vec::new(),
                })
                .collect();
            flife_core::pool::try_parallel_tasks(
                lanes.len(),
                "phase 8 split check",
                &mut tiles,
                |_, tile| {
                    let mut count = 0_u64;
                    for (offset, organism) in tile.chunk.iter_mut().enumerate() {
                        if organism.intent.is_some() {
                            count += 1;
                        } else if organism.vitality.is_alive() {
                            organism
                                .vitality
                                .check(BudgetStage::Split, &organism.budget)?;
                            let remaining = organism.budget.remaining();
                            if !organism.budget.is_exhausted() && remaining < tiny_threshold {
                                tile.tiny_indices.push(tile.base + offset);
                            } else {
                                organism
                                    .vitality
                                    .check(BudgetStage::TinyReservoir, &organism.budget)?;
                                count += u64::from(organism.vitality.is_alive());
                            }
                        }
                    }
                    tile.live_no_tiny = count;
                    Ok(())
                },
            )?;
            let mut tiny_indices = Vec::new();
            for tile in tiles {
                live += tile.live_no_tiny;
                tiny_indices.extend(tile.tiny_indices);
            }
            for idx in tiny_indices {
                let organism = &mut working[idx];
                let cleared = clear_tiny(&rules.tiny, organism, &mut tick_ledger)?;
                if let Some(record) = transactions
                    .as_mut()
                    .filter(|_| watch.organism == Some(organism.committed.id))
                {
                    record.tiny = cleared;
                }
                live += u64::from(organism.vitality.is_alive());
            }
        } else {
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
        // Preserve `survivors` length on the parallel path so `set_len_reuse`
        // avoids serially zero-filling 42+ MB of `Organism` rows every tick.
        deaths.clear();
        deaths.append(&mut removals);
        let mut organisms = CompensatedSum::ZERO;
        let seconds = step.seconds_per_tick();
        let mut emitted = 0;
        // The contact models displace every survivor from the settled
        // positions, before any moves (crate::contact).
        let contacted = if let Some(bodies) = &rules.contact {
            contact_displacements(
                rules,
                bodies,
                population,
                decisions,
                working,
                contact,
                lanes.len(),
            )?;
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
        if working.len() >= 1_024 && lanes.len() > 1 {
            let tile_count = lanes
                .len()
                .saturating_mul(4)
                .min(64)
                .min(working.len().div_ceil(512))
                .max(1);
            let tile_len = working.len().div_ceil(tile_count);
            struct FateTile<'a> {
                base: usize,
                chunk: &'a [Working],
                emitted: usize,
                survivors: usize,
                dies: Vec<(usize, BudgetStage)>,
            }
            let mut fate_tiles: Vec<FateTile<'_>> = working
                .chunks(tile_len)
                .enumerate()
                .map(|(idx, chunk)| FateTile {
                    base: idx * tile_len,
                    chunk,
                    emitted: 0,
                    survivors: 0,
                    dies: Vec::new(),
                })
                .collect();
            flife_core::pool::try_parallel_tasks(
                lanes.len(),
                "phase 8 fate scan",
                &mut fate_tiles,
                |_, tile| {
                    let mut tile_emitted = 0_usize;
                    let mut tile_survivors = 0_usize;
                    for (offset, organism) in tile.chunk.iter().enumerate() {
                        tile_emitted += usize::from(organism.emitted);
                        match organism.vitality.fate()? {
                            Fate::Survives => tile_survivors += 1,
                            Fate::Dies(stage) => tile.dies.push((tile.base + offset, stage)),
                        }
                    }
                    tile.emitted = tile_emitted;
                    tile.survivors = tile_survivors;
                    Ok(())
                },
            )?;
            let mut total_survivors = 0_usize;
            let mut die_list: Vec<(usize, BudgetStage)> = Vec::new();
            let mut tile_specs: Vec<(usize, usize, usize)> = Vec::with_capacity(fate_tiles.len());
            for tile in fate_tiles {
                emitted += tile.emitted;
                tile_specs.push((tile.base, tile.chunk.len(), tile.survivors));
                total_survivors += tile.survivors;
                die_list.extend(tile.dies);
            }
            if total_survivors == 0 {
                survivors.clear();
            } else {
                set_len_reuse(survivors, total_survivors, rows[0]);
                set_len_reuse(survivor_energies, total_survivors, 0.0);
                struct BuildTile<'a> {
                    base: usize,
                    chunk: &'a [Working],
                    out: &'a mut [Organism],
                    energies: &'a mut [f64],
                }
                let mut build_tiles: Vec<BuildTile<'_>> = Vec::with_capacity(tile_specs.len());
                let mut rem_out = survivors.as_mut_slice();
                let mut rem_e = survivor_energies.as_mut_slice();
                for (base, len, surv_count) in tile_specs {
                    let (out_head, out_tail) = rem_out.split_at_mut(surv_count);
                    rem_out = out_tail;
                    let (e_head, e_tail) = rem_e.split_at_mut(surv_count);
                    rem_e = e_tail;
                    if surv_count > 0 {
                        build_tiles.push(BuildTile {
                            base,
                            chunk: &working[base..base + len],
                            out: out_head,
                            energies: e_head,
                        });
                    }
                }
                let inv_seconds = {
                    let bits = seconds.to_bits();
                    let sign = bits >> 63;
                    let exp = (bits >> 52) & 0x7FF;
                    let frac = bits & 0x000F_FFFF_FFFF_FFFF;
                    if sign == 0 && frac == 0 && (1..=2045).contains(&exp) {
                        Some(1.0 / seconds)
                    } else {
                        None
                    }
                };
                let plain_v1_survivors = trophallaxis_deltas.is_empty()
                    && contacted.is_none()
                    && next_velocities.is_empty()
                    && pressed.is_empty()
                    && next_polarities.is_empty()
                    && next_neighbor_h0.is_empty();
                flife_core::pool::try_parallel_tasks(
                    lanes.len(),
                    "phase 8 build survivors",
                    &mut build_tiles,
                    |_, tile| {
                        let mut out_iter = tile.out.iter_mut().zip(tile.energies.iter_mut());
                        if plain_v1_survivors {
                            if let Some(inv_dt) = inv_seconds {
                                for organism in tile.chunk {
                                    if !organism.vitality.is_alive() {
                                        continue;
                                    }
                                    if let Some((out, out_energy)) = out_iter.next() {
                                        let energy = organism.budget.remaining();
                                        let committed = &organism.committed;
                                        out.id = committed.id;
                                        out.genome = committed.genome;
                                        out.parent = committed.parent;
                                        out.lineage = committed.lineage;
                                        out.generation = committed.generation;
                                        out.birth_tick = committed.birth_tick;
                                        out.position = organism.position;
                                        out.velocity = organism.velocity;
                                        out.energy = energy;
                                        out.energy_change_rate =
                                            (energy - committed.energy) * inv_dt;
                                        out.held
                                            .copy_active_from(&organism.decision(decisions).held);
                                        out.phase = committed.phase;
                                        out.cooldown_deadline = organism.cooldown_deadline;
                                        out.secretory_readiness = organism.secretory_readiness;
                                        out.emission_cooldown_deadline =
                                            organism.emission_cooldown_deadline;
                                        out.polarity = committed.polarity;
                                        out.last_neighbor_h0 = 0.0;
                                        *out_energy = energy;
                                    }
                                }
                            } else {
                                for organism in tile.chunk {
                                    if !organism.vitality.is_alive() {
                                        continue;
                                    }
                                    if let Some((out, out_energy)) = out_iter.next() {
                                        let energy = organism.budget.remaining();
                                        let committed = &organism.committed;
                                        out.id = committed.id;
                                        out.genome = committed.genome;
                                        out.parent = committed.parent;
                                        out.lineage = committed.lineage;
                                        out.generation = committed.generation;
                                        out.birth_tick = committed.birth_tick;
                                        out.position = organism.position;
                                        out.velocity = organism.velocity;
                                        out.energy = energy;
                                        out.energy_change_rate =
                                            (energy - committed.energy) / seconds;
                                        out.held
                                            .copy_active_from(&organism.decision(decisions).held);
                                        out.phase = committed.phase;
                                        out.cooldown_deadline = organism.cooldown_deadline;
                                        out.secretory_readiness = organism.secretory_readiness;
                                        out.emission_cooldown_deadline =
                                            organism.emission_cooldown_deadline;
                                        out.polarity = committed.polarity;
                                        out.last_neighbor_h0 = 0.0;
                                        *out_energy = energy;
                                    }
                                }
                            }
                            return Ok(());
                        }
                        for (offset, organism) in tile.chunk.iter().enumerate() {
                            if !organism.vitality.is_alive() {
                                continue;
                            }
                            let index = tile.base + offset;
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
                            let position = match contacted.and_then(|moves| moves.get(index)) {
                                Some(&[dx, dy]) if dx != 0.0 || dy != 0.0 => {
                                    rules.grid.wrap_position([
                                        organism.position[0] + dx,
                                        organism.position[1] + dy,
                                    ])?
                                }
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
                            let last_neighbor_h0 =
                                next_neighbor_h0.get(index).copied().unwrap_or(0.0);
                            if let Some((out, out_energy)) = out_iter.next() {
                                let committed = &organism.committed;
                                out.id = committed.id;
                                out.genome = committed.genome;
                                out.parent = committed.parent;
                                out.lineage = committed.lineage;
                                out.generation = committed.generation;
                                out.birth_tick = committed.birth_tick;
                                out.position = position;
                                out.velocity = velocity;
                                out.energy = energy;
                                out.energy_change_rate = match inv_seconds {
                                    Some(inv_dt) => (energy - committed.energy) * inv_dt,
                                    None => (energy - committed.energy) / seconds,
                                };
                                out.held
                                    .copy_active_from(&organism.decision(decisions).held);
                                out.phase = committed.phase;
                                out.cooldown_deadline = organism.cooldown_deadline;
                                out.secretory_readiness = organism.secretory_readiness;
                                out.emission_cooldown_deadline =
                                    organism.emission_cooldown_deadline;
                                out.polarity = polarity;
                                out.last_neighbor_h0 = last_neighbor_h0;
                                *out_energy = energy;
                            }
                        }
                        Ok(())
                    },
                )?;
                for &energy in &survivor_energies[..total_survivors] {
                    organisms.add(energy);
                }
                if !trophallaxis_deltas.is_empty()
                    && let (Some(watched_id), Some(record)) =
                        (watch.organism, transactions.as_mut())
                    && let Ok(index) = working.binary_search_by_key(&watched_id, |o| o.committed.id)
                    && working[index].vitality.is_alive()
                {
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "signed per-organism quantum delta is exact in f64 for |delta_k| <= 2^53"
                    )]
                    let trophallaxis_delta = trophallaxis_deltas
                        .get(index)
                        .copied()
                        .map_or(0.0, |k| (k as f64) * TROPHALLAXIS_QUANTUM);
                    if trophallaxis_delta != 0.0 {
                        record.trophallaxis_delta = trophallaxis_delta;
                        record.closing = working[index].budget.remaining() + trophallaxis_delta;
                    }
                }
            }
            for (index, stage) in die_list {
                let organism = &working[index];
                let genome = death_genomes.next(organism.committed.id).0.ok_or_else(|| {
                    CoreError::StateConflict {
                        operation: "read genome",
                        detail: format!("{} has no genome", organism.committed.id),
                    }
                })?;
                let recurrent_h0 = organism.decision(decisions).held.recurrent_state_array()[0];
                let (death, corpse_dir) = if organism.lysed_residual > 0.0 {
                    let death = Death::lysed(
                        &rows[index],
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
                    let death = Death::exhausted(&rows[index], to, organism.position, stage)?;
                    (death, *genome.traits().affinity())
                };
                rules
                    .recycling
                    .settle_with(&rules.grid, rules.exchange, &death, &corpse_dir, recycling)?
                    .record(&mut tick_ledger)?;
                deaths.push(death);
            }
        } else {
            survivors.clear();
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
                            Some(&[dx, dy]) if dx != 0.0 || dy != 0.0 => {
                                rules.grid.wrap_position([
                                    organism.position[0] + dx,
                                    organism.position[1] + dy,
                                ])?
                            }
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
                        let committed = &organism.committed;
                        survivors.push(Organism {
                            id: committed.id,
                            genome: committed.genome,
                            parent: committed.parent,
                            lineage: committed.lineage,
                            generation: committed.generation,
                            birth_tick: committed.birth_tick,
                            position,
                            velocity,
                            energy,
                            energy_change_rate: (energy - committed.energy) / seconds,
                            held: organism.decision(decisions).held,
                            phase: committed.phase,
                            cooldown_deadline: organism.cooldown_deadline,
                            secretory_readiness: organism.secretory_readiness,
                            emission_cooldown_deadline: organism.emission_cooldown_deadline,
                            polarity,
                            last_neighbor_h0,
                        });
                    }
                    Fate::Dies(stage) => {
                        let genome =
                            death_genomes.next(organism.committed.id).0.ok_or_else(|| {
                                CoreError::StateConflict {
                                    operation: "read genome",
                                    detail: format!("{} has no genome", organism.committed.id),
                                }
                            })?;
                        let recurrent_h0 =
                            organism.decision(decisions).held.recurrent_state_array()[0];
                        let (death, corpse_dir) = if organism.lysed_residual > 0.0 {
                            let death = Death::lysed(
                                &rows[index],
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
                                Death::exhausted(&rows[index], to, organism.position, stage)?;
                            (death, *genome.traits().affinity())
                        };
                        rules
                            .recycling
                            .settle_with(
                                &rules.grid,
                                rules.exchange,
                                &death,
                                &corpse_dir,
                                recycling,
                            )?
                            .record(&mut tick_ledger)?;
                        deaths.push(death);
                    }
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
        let mut fused_byproducts: Option<DepositSummary> = None;
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
                    fused_byproducts = Some(bp_summary);
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
                fused_byproducts = Some(bp_summary);
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
        if let Some(byproducts) = byproducts.as_mut() {
            let byproduct_summary = if let Some(summary) = fused_byproducts {
                summary
            } else if byproducts.is_empty() {
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
        let checked = population.check_tick_staged_on(
            to,
            survivors,
            deaths,
            &entrants,
            &next_quota,
            lanes.len(),
        )?;

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
        let worker_count = workers(workers_configured)?.get();
        let use_tiles = uses_settle_tiles(worker_count, rules, rows, false);
        let mut settle_tiles = Vec::new();
        fit_settle_tiles(&mut settle_tiles, worker_count, rules, rows, false)?;
        let packet_cap = usize::try_from(packets).map_err(|_| CoreError::Overflow {
            quantity: "organism packets",
        })?;
        let comp_cap = usize::try_from(rules.grid.cell_count().min(packets)).map_err(|_| {
            CoreError::Overflow {
                quantity: "packet compositions",
            }
        })?;
        let uses_spatial_pass1 =
            rows >= SPATIAL_PASS1_MIN_ROWS && rules.grid.cell_count() >= SPATIAL_PASS1_MIN_CELLS;
        let (spatial_rows, row_to_spatial, spatial_uptake) = if uses_spatial_pass1 {
            (
                reserved(rows, "spatial pass1 rows")?,
                reserved(rows, "spatial pass1 map")?,
                reserved(rows, "spatial phase6 uptake")?,
            )
        } else {
            (Vec::new(), Vec::new(), Vec::new())
        };
        Ok(Self {
            field,
            pyramid: SensorPyramid::from_config(config, &limits)?,
            rows: reserved(rows, "organism rows")?,
            decisions: reserved(rows, "decisions")?,
            spatial_rows,
            row_to_spatial,
            spatial_tile_offsets: [0; SPATIAL_TILES + 1],
            spatial_uptake,
            lanes: lanes(worker_count, rules, !use_tiles)?,
            settle_tiles,
            sampled: Vec::with_capacity(sources),
            working: reserved(rows, "working organisms")?,
            demands: reserved(rows, "organism demands")?,
            grants: Grants::with_capacity_for(&rules.grid, organisms, rules.exchange, &limits)?,
            by_demand: Bins::with_capacity("organism packets", organisms, packets, &limits)?,
            compositions: reserved(comp_cap, "packet compositions")?,
            packet_compositions: reserved(packet_cap, "packet composition indices")?,
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
            survivor_energies: reserved(rows, "survivor energies")?,
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
    population.energy_total()
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
fn lanes(
    workers: usize,
    rules: &Rules,
    lane_settle_buffers: bool,
) -> Result<Vec<DecisionLane>, CoreError> {
    let workers = workers.max(1);
    let mut lanes = reserved(workers, "decision lanes")?;
    for _ in 0..workers {
        lanes.push(lane(rules)?);
    }
    fit_lanes(
        &mut lanes,
        rules,
        capacity_of(rules)?,
        false,
        lane_settle_buffers,
    )?;
    Ok(lanes)
}

/// An empty decision lane with its controller scratch for the catalog's
/// widest member and its exchange-footprint scratch for the world's kernel.
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
        exchange: ExchangeScratch::for_kernel(rules.exchange, &rules.grid)?,
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

/// Reserve or release only each lane's legacy batched observation buffers
/// (`observed` and `controller_inputs`), leaving its settle buffers intact.
fn fit_lane_batched(
    lanes: &mut [DecisionLane],
    rules: &Rules,
    organisms: usize,
    batched: bool,
) -> Result<(), CoreError> {
    let rows = lane_rows(organisms, lanes.len());
    let needs_expanded = rules.v2.sensors().input_width() > SENSOR_WIDTH;
    for lane in lanes {
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

/// Reserve each lane's decisions, ledger flows and staged emissions (unless
/// large-world `settle_tiles` owns staging), and with `batched` its
/// observations, for the most rows it can take of `organisms`; without
/// `batched`, release its observations.
fn fit_lanes(
    lanes: &mut [DecisionLane],
    rules: &Rules,
    organisms: usize,
    batched: bool,
    lane_settle_buffers: bool,
) -> Result<(), CoreError> {
    let rows = lane_rows(organisms, lanes.len());
    let packet_cap = if rules.exchange.is_grid_cic() {
        0
    } else {
        rules.max_exchange_cells
    };
    for lane in lanes.iter_mut() {
        fit(&mut lane.packets, packet_cap, "organism packets")?;
        if lane_settle_buffers {
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
        } else {
            lane.flows = LedgerLog::default();
            lane.emissions = Deposits::new();
            lane.trail_emissions = None;
            lane.byproducts = None;
        }
        lane.transactions = None;
    }
    fit_lane_batched(lanes, rules, organisms, batched)
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
    let mut cursor = 0_usize;
    while cursor < deposits.len() {
        let carrier = deposits[cursor].1;
        let layout = match carrier {
            CarrierSpec::Carrier0 => next.carrier0(),
            CarrierSpec::Carrier1 => next.carrier1().ok_or_else(|| CoreError::LayoutMismatch {
                buffer: "field",
                detail: "Carrier1 deposit offered to a 33-plane single-carrier field".to_owned(),
            })?,
        };
        let mut end = cursor + 1;
        while end < deposits.len() && deposits[end].1 == carrier {
            end += 1;
        }
        let mut group: Vec<&mut Deposits> = deposits[cursor..end]
            .iter_mut()
            .map(|(deposit, _)| &mut **deposit)
            .collect();
        apply_deposit_streams_to_carrier_on(
            next,
            &mut group,
            layout,
            workers,
            &mut out[cursor..end],
        )?;
        cursor = end;
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
    rows: &[Organism],
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
    let requesters = || rows.iter().map(|organism| (organism.id, organism.position));
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
                demands_on_lanes(field, &rule, rules.exchange, rows, demands, lanes, lanes.len())?;
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
    demands_on_lanes(field, &rule, rules.exchange, rows, demands, lanes, lanes.len())?;
    resolve_uptake_with_on_workers(field, rules.exchange, demands, grants, lanes.len())
}

/// Phase 5's demands on the reference, on up to `workers` lanes: `demands`
/// gets each committed organism's identity and position in identity order,
/// then each lane computes a contiguous identity-ordered run of them with
/// [`demand_with_scratch`]. A demand reads only the observed field and
/// committed positions, so the demands are identical at any lane count and
/// independent of Phase 4 decisions, and the error returned is the first in
/// identity order.
fn demands_on_lanes(
    field: &FieldState,
    rule: &RequestRule,
    exchange: ExchangeKernel,
    rows: &[Organism],
    demands: &mut Vec<Demand>,
    lanes: &mut [DecisionLane],
    workers: usize,
) -> Result<(), CoreError> {
    demands.clear();
    if rows.is_empty() {
        return Ok(());
    }
    reserve_for(demands, rows.len(), "organism demands")?;
    let placeholder = Demand {
        organism: rows[0].id,
        position: rows[0].position,
        amount: 0.0,
    };
    set_len_reuse(demands, rows.len(), placeholder);
    let worker_budget = workers.min(lanes.len()).max(1);
    let max_tiles = rows.len().div_ceil(MINIMUM_ROWS_PER_LANE).max(1);
    let tile_count = if worker_budget <= 1 || max_tiles <= 1 {
        1
    } else {
        worker_budget
            .saturating_mul(4)
            .min(64)
            .min(max_tiles)
            .max(worker_budget.min(max_tiles))
    };
    let tile_len = rows.len().div_ceil(tile_count);
    let mut tiles: Vec<(&[Organism], &mut [Demand])> = rows
        .chunks(tile_len)
        .zip(demands.chunks_mut(tile_len))
        .collect();
    flife_core::pool::try_parallel_tasks_with_lanes(
        worker_budget,
        "uptake demands",
        lanes,
        &mut tiles,
        |_, (row_chunk, dem_chunk), lane| {
            for (organism, slot) in row_chunk.iter().zip(dem_chunk.iter_mut()) {
                *slot = demand_with_scratch(
                    field,
                    rule,
                    exchange,
                    organism.id,
                    organism.position,
                    &mut lane.exchange,
                )?;
            }
            Ok(())
        },
    )
}

/// CPU Phase 5 (uptake + demand grouping) and optional Phase 6 fused spatial
/// chemistry prepass state that can overlap behind an off-CPU controller
/// backend (e.g. Metal GPU) because neither depends on Phase 4 decisions.
struct OverlapPhase56<'a> {
    observed: &'a FieldState,
    demands: &'a mut Vec<Demand>,
    grants: &'a mut Grants,
    by_demand: &'a mut Bins,
    compositions: &'a mut Vec<[f64; CHANNELS]>,
    packet_compositions: &'a mut Vec<u32>,
    spatial_uptake: &'a mut Vec<(UptakeSettlement, EnergyBudget)>,
    run_fused_chem: bool,
    uptake_result: Option<Result<UptakeSummary, CoreError>>,
    fused_chem_done: bool,
}

#[expect(
    clippy::too_many_arguments,
    reason = "runs Phase 5 uptake and optional Phase 6 fused chemistry prepass on CPU while off-CPU controllers execute"
)]
fn run_overlapped_phase56(
    rules: &Rules,
    population: &Population,
    rows: &[Organism],
    spatial_rows: &[u32],
    row_to_spatial: &[u32],
    spatial_tile_offsets: &[usize; SPATIAL_TILES + 1],
    lanes: &mut [DecisionLane],
    cpu_workers: usize,
    ov: &mut OverlapPhase56<'_>,
) {
    let rule = RequestRule {
        step: rules.step,
        capacity_per_second: rules.uptake_capacity,
        half_saturation_density: rules.half_saturation,
    };
    let uptake_res = demands_on_lanes(
        ov.observed,
        &rule,
        rules.exchange,
        rows,
        ov.demands,
        lanes,
        cpu_workers,
    )
    .and_then(|()| {
        let summary = resolve_uptake_with_on_workers(
            ov.observed,
            rules.exchange,
            ov.demands,
            ov.grants,
            cpu_workers,
        )?;
        let granted = ov.grants.packets();
        ov.by_demand
            .group_on(ov.demands.len(), granted.len(), cpu_workers, |at| {
                granted.get(at).map_or(usize::MAX, |packet| packet.demand)
            })?;
        Ok(summary)
    });
    let uptake_ok = uptake_res.is_ok();
    ov.uptake_result = Some(uptake_res);
    if uptake_ok && ov.run_fused_chem && !row_to_spatial.is_empty() {
        ov.fused_chem_done = spatial_chemistry_prepass_fused(
            rules,
            population,
            ov.observed,
            (ov.by_demand, ov.grants.packets(), ov.grants.bins()),
            rows.len(),
            cpu_workers,
            spatial_rows,
            row_to_spatial,
            spatial_tile_offsets,
            (ov.compositions, ov.packet_compositions),
            ov.spatial_uptake,
        );
    }
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

/// Direct F32 sensor encoding into `out_inputs_f32` and `out_gradients` with
/// zero intermediate `[f64; 104]` or 528-byte gradient return copies.
#[inline]
fn encode_for_controller_f32_into(
    rules: &Rules,
    organism: &Organism,
    raw: &RawSample,
    from: Tick,
    out_inputs_f32: &mut [f32; SENSOR_WIDTH],
    out_gradients: &mut [[f64; 2]; RESPONSE_TERMS],
) -> Result<(), CoreError> {
    let intrinsic = Intrinsic {
        energy: organism.energy,
        age_seconds: age_seconds(organism.birth_tick, from, rules.step)?,
        velocity: organism.velocity,
        energy_change_per_second: organism.energy_change_rate,
    };
    observe_for_controller_f32_into(
        raw,
        &intrinsic,
        &rules.scales,
        out_inputs_f32,
        out_gradients,
    )
}

/// Direct F32 dual-carrier sensor encoding (`V2-15`) into `out_inputs_f32`,
/// `out_trail_f32`, and `out_gradients`.
#[inline]
fn encode_dual_for_controller_f32_into(
    rules: &Rules,
    organism: &Organism,
    raw: &DualRawSample,
    from: Tick,
    out_inputs_f32: &mut [f32; SENSOR_WIDTH],
    out_trail_f32: &mut [f32; 4],
    out_gradients: &mut [[f64; 2]; RESPONSE_TERMS],
) -> Result<(), CoreError> {
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
    observe_dual_for_controller_f32_into(
        raw,
        &intrinsic,
        &rules.scales,
        dual.trail_energy_density_scale(),
        out_inputs_f32,
        out_trail_f32,
        out_gradients,
    )
}

/// Gradient-only encoding directly into `out_gradients` for an organism whose
/// controller is not due this tick (skips all 69 `tanh` saturations).
#[inline]
fn encode_gradients_only_into(
    rules: &Rules,
    organism: &Organism,
    raw: &RawSample,
    from: Tick,
    out_gradients: &mut [[f64; 2]; RESPONSE_TERMS],
) -> Result<(), CoreError> {
    let intrinsic = Intrinsic {
        energy: organism.energy,
        age_seconds: age_seconds(organism.birth_tick, from, rules.step)?,
        velocity: organism.velocity,
        energy_change_per_second: organism.energy_change_rate,
    };
    observe_gradients_only_into(raw, &intrinsic, &rules.scales, out_gradients)
}

/// Gradient-only dual-carrier encoding (`V2-15`) directly into `out_gradients`
/// for an organism whose controller is not due this tick (skips 71 `tanh`s).
#[inline]
fn encode_dual_gradients_only_into(
    rules: &Rules,
    organism: &Organism,
    raw: &DualRawSample,
    from: Tick,
    out_gradients: &mut [[f64; 2]; RESPONSE_TERMS],
) -> Result<(), CoreError> {
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
    observe_dual_gradients_only_into(
        raw,
        &intrinsic,
        &rules.scales,
        dual.trail_energy_density_scale(),
        out_gradients,
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

/// Decode `held` directly into `out`, avoiding per-organism `f64` widening
/// buffers and 336-byte `DecodedActions` return copies.
#[inline]
fn decode_held_into(
    rules: &Rules,
    organism: &Organism,
    held: &HeldOutput,
    out: &mut DecodedActions,
) -> Result<(), CoreError> {
    const ZERO_OUTPUTS: [f32; MAX_ACTION_WIDTH] = [0.0; MAX_ACTION_WIDTH];
    let outputs = held.outputs().ok_or_else(|| CoreError::StateConflict {
        operation: "decode held output",
        detail: format!("{} holds no valid output", organism.id),
    })?;
    let len = outputs.len();
    if len > MAX_ACTION_WIDTH {
        return Err(CoreError::LayoutMismatch {
            buffer: "held controller output",
            detail: format!("output width {len} exceeds maximum {MAX_ACTION_WIDTH}"),
        });
    }
    let slice = if rules.variant.decides_from_zero() {
        &ZERO_OUTPUTS[..len]
    } else {
        outputs
    };
    decode_with_layout_f32_into(slice, rules.step, &rules.decoding, &rules.v2.actions(), out)
}

/// Decode `held` into [`DecodedActions`] for this tick.
fn decode_held(
    rules: &Rules,
    organism: &Organism,
    held: &HeldOutput,
) -> Result<DecodedActions, CoreError> {
    let mut actions = DecodedActions::default();
    decode_held_into(rules, organism, held, &mut actions)?;
    Ok(actions)
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

/// Write one backend's raw F32 slice directly into `out`, avoiding a 280-byte
/// stack `HeldOutput` temporary and copy in Pass 2 controller decode loops.
#[inline]
fn held_from_raw_slice_into(
    rules: &Rules,
    organism: &Organism,
    to: Tick,
    raw: &[f32],
    out: &mut HeldOutput,
) -> Result<(), CoreError> {
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
        out.write_evaluated_recurrent_slice(to, action_slice, &next_state[..state_width])
    } else {
        out.write_evaluated_slice(to, action_slice)
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
    let mut held = HeldOutput::INVALID;
    held_from_raw_slice_into(rules, organism, to, raw, &mut held)?;
    Ok(held)
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
/// Skips all 69 controller-input `tanh` saturations when the organism is not
/// due this tick while keeping identical gradients and validation errors.
fn decide_from_raw(
    rules: &Rules,
    genome: &Genome,
    organism: &Organism,
    raw: &RawSample,
    local_crowding: Option<u32>,
    [from, to]: [Tick; 2],
    controller: &mut ControllerScratch,
) -> Result<Decision, CoreError> {
    if !due(rules, organism, to) {
        let mut gradients = [[0.0; 2]; RESPONSE_TERMS];
        encode_gradients_only_into(rules, organism, raw, from, &mut gradients)?;
        return finish_from_gradients(rules, organism, organism.held, false, gradients);
    }
    let observation = encode_for_controller(rules, organism, raw, from)?;
    let held = evaluate_reference_held(
        rules,
        genome,
        organism,
        &observation,
        None,
        local_crowding,
        to,
        controller,
    )?;
    finish(rules, organism, held, true, &observation)
}

/// Phases 3 and 4 for one organism on a 34-plane dual-carrier field (`V2-15`).
/// Skips all 71 controller-input `tanh` saturations when not due this tick.
fn decide_from_dual_raw(
    rules: &Rules,
    genome: &Genome,
    organism: &Organism,
    raw: &DualRawSample,
    local_crowding: Option<u32>,
    [from, to]: [Tick; 2],
    controller: &mut ControllerScratch,
) -> Result<Decision, CoreError> {
    if !due(rules, organism, to) {
        let mut gradients = [[0.0; 2]; RESPONSE_TERMS];
        encode_dual_gradients_only_into(rules, organism, raw, from, &mut gradients)?;
        return finish_from_gradients(rules, organism, organism.held, false, gradients);
    }
    let dual_obs = encode_dual_for_controller(rules, organism, raw, from)?;
    let held = evaluate_reference_held(
        rules,
        genome,
        organism,
        &dual_obs.base,
        Some(dual_obs.trail_inputs),
        local_crowding,
        to,
        controller,
    )?;
    finish(rules, organism, held, true, &dual_obs.base)
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

/// Target number of dynamic work-stealing settle tiles for `workers` CPU
/// workers (`2..=4` tiles per worker, targeting ~72 tiles to absorb
/// heterogeneous P/E-core stragglers).
fn settle_tile_target(workers: usize) -> usize {
    72_usize
        .div_ceil(workers.max(1))
        .clamp(2, 4)
        .saturating_mul(workers.max(1))
}

/// Whether `(workers, rules, capacity, has_settle_backend)` uses pre-reserved
/// dynamic work-stealing `settle_tiles` instead of per-lane settle buffers.
const fn uses_settle_tiles(
    workers: usize,
    rules: &Rules,
    capacity: usize,
    has_settle_backend: bool,
) -> bool {
    !has_settle_backend
        && workers > 1
        && capacity >= SETTLE_TILE_THRESHOLD
        && matches!(rules.exchange, ExchangeKernel::GridCicV1)
}

/// Reserve or release [`Scratch::settle_tiles`] for `(workers, capacity)` so
/// large multi-worker `grid-cic-v1` worlds (`capacity ≥ 24,576`, CPU settle)
/// settle with zero per-tick allocations and without duplicating per-lane
/// settle staging buffers (FL-043).
fn fit_settle_tiles(
    tiles: &mut Vec<SettleTileBuf>,
    workers: usize,
    rules: &Rules,
    capacity: usize,
    has_settle_backend: bool,
) -> Result<(), CoreError> {
    if !uses_settle_tiles(workers, rules, capacity, has_settle_backend) {
        tiles.clear();
        tiles.shrink_to_fit();
        return Ok(());
    }
    let max_tiles = settle_tile_target(workers)
        .min(capacity.div_ceil(MINIMUM_ROWS_PER_LANE).max(1))
        .max(1);
    let max_sub = capacity.div_ceil(max_tiles).max(MINIMUM_ROWS_PER_LANE);
    let has_trail = rules.v2.dual_carrier().is_some();
    let has_byproducts = rules.v2.waste().is_some();
    tiles.truncate(max_tiles);
    tiles.shrink_to(max_tiles);
    if tiles.len() < max_tiles {
        tiles
            .try_reserve_exact(max_tiles - tiles.len())
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "settle tiles",
                bytes: bytes_for::<SettleTileBuf>(max_tiles),
            })?;
        while tiles.len() < max_tiles {
            tiles.push(SettleTileBuf::default());
        }
    }
    for tile in tiles.iter_mut() {
        tile.fit(max_sub, has_trail, has_byproducts)?;
    }
    Ok(())
}

/// Write four contiguous moment planes (`ch..ch+4`) for a contiguous cell run
/// into `comps_out`, processing four cells per iteration so LLVM emits 16-byte
/// NEON `ldr q` loads, `fcvtl`/`fdiv.2d`, in-register 4×4 transpose, and
/// 32-byte contiguous `stp q, q` stores (`0`-ULP bit-exact `f64::from(m) / e`).
#[inline]
fn write_four_planes_contig(
    ch: usize,
    s0: &[f32],
    s1: &[f32],
    s2: &[f32],
    s3: &[f32],
    energies: &[f64],
    dst: &mut [[f64; CHANNELS]],
) {
    let (s0_q, s0_r) = s0.as_chunks::<4>();
    let (s1_q, s1_r) = s1.as_chunks::<4>();
    let (s2_q, s2_r) = s2.as_chunks::<4>();
    let (s3_q, s3_r) = s3.as_chunks::<4>();
    let (e_q, e_r) = energies.as_chunks::<4>();
    let (d_q, d_r) = dst.as_chunks_mut::<4>();
    for (((((m0, m1), m2), m3), ek), dk) in s0_q
        .iter()
        .zip(s1_q)
        .zip(s2_q)
        .zip(s3_q)
        .zip(e_q)
        .zip(d_q)
    {
        let q0 = [
            f64::from(m0[0]) / ek[0],
            f64::from(m0[1]) / ek[1],
            f64::from(m0[2]) / ek[2],
            f64::from(m0[3]) / ek[3],
        ];
        let q1 = [
            f64::from(m1[0]) / ek[0],
            f64::from(m1[1]) / ek[1],
            f64::from(m1[2]) / ek[2],
            f64::from(m1[3]) / ek[3],
        ];
        let q2 = [
            f64::from(m2[0]) / ek[0],
            f64::from(m2[1]) / ek[1],
            f64::from(m2[2]) / ek[2],
            f64::from(m2[3]) / ek[3],
        ];
        let q3 = [
            f64::from(m3[0]) / ek[0],
            f64::from(m3[1]) / ek[1],
            f64::from(m3[2]) / ek[2],
            f64::from(m3[3]) / ek[3],
        ];
        for k in 0..4 {
            let out = &mut dk[k][ch..ch + 4];
            out[0] = q0[k];
            out[1] = q1[k];
            out[2] = q2[k];
            out[3] = q3[k];
        }
    }
    for (((((&m0, &m1), &m2), &m3), &e), out_cell) in s0_r
        .iter()
        .zip(s1_r)
        .zip(s2_r)
        .zip(s3_r)
        .zip(e_r)
        .zip(d_r)
    {
        let out = &mut out_cell[ch..ch + 4];
        out[0] = f64::from(m0) / e;
        out[1] = f64::from(m1) / e;
        out[2] = f64::from(m2) / e;
        out[3] = f64::from(m3) / e;
    }
}

/// Compute normalized chemical compositions `m / e` for `new_cells` directly
/// into a worker lane's L2-resident `comps_out` slice, reading four moment
/// planes per pass (matching Apple Silicon 8-way L1D associativity without
/// set-conflict thrashing while cutting `comps_out` store traffic 4×) and
/// breaking batches at row jumps so dense halo rows run through
/// [`write_four_planes_contig`]. Marked `#[inline(never)]` so
/// [`spatial_chemistry_prepass_fused`]'s inner tile closure stays compact.
#[expect(
    clippy::too_many_lines,
    reason = "4-plane contiguous and sparse batched composition kernels"
)]
#[inline(never)]
fn compute_tile_compositions(
    observed: &FieldState,
    new_cells: &[u32],
    comps_out: &mut [[f64; CHANNELS]],
) -> Result<(), CoreError> {
    const COMP_BATCH: usize = 32;

    let energy_plane = observed.energy();
    let total_cells = energy_plane.len();
    let all_data = observed.as_slice();
    let planes = observed.planes() as usize;
    let active_channels = observed.carrier0_moments_count();
    if !total_cells.is_power_of_two()
        || active_channels > CHANNELS
        || planes < 1 + active_channels
        || all_data.len() != planes.saturating_mul(total_cells)
        || comps_out.len() < new_cells.len()
    {
        return Err(CoreError::LayoutMismatch {
            buffer: "field",
            detail: "fused chemistry tile composition layout mismatch".to_owned(),
        });
    }
    let e_mask = total_cells - 1;
    let mut cells = [0_usize; COMP_BATCH];
    let mut energies = [0.0_f64; COMP_BATCH];
    let mut slots = [0_usize; COMP_BATCH];
    let n = new_cells.len();
    let quad_channels = (active_channels / 4) * 4;
    let mut i = 0_usize;
    while i < n {
        let batch_start = i;
        let mut batch_count = 0_usize;
        let mut cells_contig = true;
        while i < n && batch_count < COMP_BATCH {
            let cell = new_cells[i] as usize;
            if batch_count > 0 && cells_contig {
                let prev = cells[batch_count - 1];
                if cell != prev.wrapping_add(1) {
                    // Flush a contiguous run on a row jump or once >= 4 cells
                    // are queued so dense halo rows stay on the contiguous
                    // NEON path while sparse halos still pack full batches.
                    if batch_count >= 4 || cell.wrapping_sub(prev) >= 32 {
                        break;
                    }
                    cells_contig = false;
                }
            }
            let e = f64::from(energy_plane[cell & e_mask]);
            if e > 0.0 {
                cells[batch_count] = cell;
                energies[batch_count] = e;
                slots[batch_count] = i;
                if active_channels < CHANNELS {
                    comps_out[i][active_channels..].fill(0.0);
                }
                batch_count += 1;
            } else {
                comps_out[i] = [0.0; CHANNELS];
            }
            i += 1;
        }
        if batch_count == 0 {
            continue;
        }
        let slots_contig = i - batch_start == batch_count;
        let c0 = cells[0];
        let contig_in_bounds =
            cells_contig && c0.checked_add(batch_count).is_some_and(|end| end <= total_cells);
        let mut ch = 0_usize;
        while ch < quad_channels {
            let base = (ch + 1) * total_cells;
            let p0 = &all_data[base..base + total_cells];
            let p1 = &all_data[base + total_cells..base + 2 * total_cells];
            let p2 = &all_data[base + 2 * total_cells..base + 3 * total_cells];
            let p3 = &all_data[base + 3 * total_cells..base + 4 * total_cells];
            if slots_contig && contig_in_bounds {
                write_four_planes_contig(
                    ch,
                    &p0[c0..c0 + batch_count],
                    &p1[c0..c0 + batch_count],
                    &p2[c0..c0 + batch_count],
                    &p3[c0..c0 + batch_count],
                    &energies[..batch_count],
                    &mut comps_out[batch_start..batch_start + batch_count],
                );
            } else if slots_contig {
                for ((out_cell, &cell), &e) in comps_out[batch_start..batch_start + batch_count]
                    .iter_mut()
                    .zip(&cells[..batch_count])
                    .zip(&energies[..batch_count])
                {
                    let c = cell & e_mask;
                    let out = &mut out_cell[ch..ch + 4];
                    out[0] = f64::from(p0[c]) / e;
                    out[1] = f64::from(p1[c]) / e;
                    out[2] = f64::from(p2[c]) / e;
                    out[3] = f64::from(p3[c]) / e;
                }
            } else {
                for ((&slot, &cell), &e) in slots[..batch_count]
                    .iter()
                    .zip(&cells[..batch_count])
                    .zip(&energies[..batch_count])
                {
                    let c = cell & e_mask;
                    let out = &mut comps_out[slot][ch..ch + 4];
                    out[0] = f64::from(p0[c]) / e;
                    out[1] = f64::from(p1[c]) / e;
                    out[2] = f64::from(p2[c]) / e;
                    out[3] = f64::from(p3[c]) / e;
                }
            }
            ch += 4;
        }
        while ch < active_channels {
            let base = (ch + 1) * total_cells;
            let plane = &all_data[base..base + total_cells];
            if slots_contig && contig_in_bounds {
                for ((out_cell, &m), &e) in comps_out[batch_start..batch_start + batch_count]
                    .iter_mut()
                    .zip(&plane[c0..c0 + batch_count])
                    .zip(&energies[..batch_count])
                {
                    out_cell[ch] = f64::from(m) / e;
                }
            } else if slots_contig {
                for ((out_cell, &cell), &e) in comps_out[batch_start..batch_start + batch_count]
                    .iter_mut()
                    .zip(&cells[..batch_count])
                    .zip(&energies[..batch_count])
                {
                    out_cell[ch] = f64::from(plane[cell & e_mask]) / e;
                }
            } else {
                for ((&slot, &cell), &e) in slots[..batch_count]
                    .iter()
                    .zip(&cells[..batch_count])
                    .zip(&energies[..batch_count])
                {
                    comps_out[slot][ch] = f64::from(plane[cell & e_mask]) / e;
                }
            }
            ch += 1;
        }
    }
    Ok(())
}

/// Fused Phase-6 L2-resident tile-local compositions + chemistry pre-pass:
/// eliminates the separate `compositions_into` barrier and 67 MB global DRAM
/// compositions round-trip on multi-worker `grid-cic-v1` worlds.
///
/// Each spatial macro-tile `(tx, ty)` covers a `(tile_w + 2) × (tile_h + 2)`
/// periodic halo of grid cells (`<= 66 × 66 = 4,356` cells on `512²`). Each
/// worker lane carves a disjoint `<= 1.1 MiB` composition slice from
/// `cell_compositions` and two disjoint `halo_cells` `u32` index slices
/// (`halo_to_local`, `tile_cells`) from `packet_compositions` (zero new heap
/// allocations, zero large stack buffers, preserving `World::footprint`
/// exactly). Occupied halo cells are discovered directly from
/// `grants_bins.offsets()` in ascending row-major field order, composed via
/// [`compute_tile_compositions`] directly into L2, and consumed immediately in
/// one pass over the tile's organisms. Returns `true` if `spatial_uptake` is
/// fully populated (skipping `compositions_into`); returns `false` (leaving
/// `spatial_uptake` empty) to fall back cleanly to the canonical
/// `compositions_into` + `settle_all` path.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "private Phase-6 fused L2-pinned halo-scan compositions and chemistry runner"
)]
fn spatial_chemistry_prepass_fused(
    rules: &Rules,
    population: &Population,
    observed: &FieldState,
    grants: (&Bins, &[GrantPacket], &Bins),
    count: usize,
    workers: usize,
    spatial_rows: &[u32],
    row_to_spatial: &[u32],
    spatial_tile_offsets: &[usize; SPATIAL_TILES + 1],
    compositions: (&mut Vec<[f64; CHANNELS]>, &mut Vec<u32>),
    spatial_uptake: &mut Vec<(UptakeSettlement, EnergyBudget)>,
) -> bool {
    const MAX_HALO_CELLS: usize = 4_608;

    spatial_uptake.clear();
    let ordered_slots = population.ordered_slots();
    let (by_demand, granted, grants_bins) = grants;
    let (cell_compositions, packet_compositions) = compositions;
    if workers <= 1
        || count == 0
        || row_to_spatial.len() != count
        || spatial_rows.len() != count
        || spatial_tile_offsets[SPATIAL_TILES] != count
        || ordered_slots.len() != count
        || spatial_uptake.capacity() < count
        || rules.v2.affinity_rotation_enabled()
        || rules.limits.affinity_norm() > rules.chemistry.affinity_norm_limit()
        || !rules.exchange.is_grid_cic()
    {
        return false;
    }

    let grid = observed.grid();
    let gx = grid.cells_x() as usize;
    let gy = grid.cells_y() as usize;
    let energy_plane = observed.energy();
    let total_cells = energy_plane.len();
    let all_data = observed.as_slice();
    let planes = observed.planes() as usize;
    let active_channels = observed.carrier0_moments_count();
    let grant_offsets = grants_bins.offsets();
    if !gx.is_power_of_two()
        || !gy.is_power_of_two()
        || gx < SPATIAL_NX
        || gy < SPATIAL_NY
        || gx.checked_mul(gy) != Some(total_cells)
        || !total_cells.is_power_of_two()
        || total_cells >= u32::MAX as usize
        || active_channels > CHANNELS
        || all_data.len() != planes.saturating_mul(total_cells)
        || grants_bins.order().len() != granted.len()
        || grant_offsets.len() != total_cells + 1
    {
        return false;
    }

    let tile_w = gx / SPATIAL_NX;
    let tile_h = gy / SPATIAL_NY;
    let halo_w = tile_w + 2;
    let halo_h = tile_h + 2;
    let halo_cells = halo_w.saturating_mul(halo_h);
    if halo_cells == 0 || halo_cells > MAX_HALO_CELLS {
        return false;
    }

    // Carve per-lane L2 buffers strictly inside already-reserved capacities
    // (zero heap growth).
    let Some(total_comp_slots) = workers.checked_mul(halo_cells) else {
        return false;
    };
    let Some(u32_per_lane) = halo_cells.checked_mul(2) else {
        return false;
    };
    let Some(total_u32_slots) = workers.checked_mul(u32_per_lane) else {
        return false;
    };
    if total_comp_slots > cell_compositions.capacity()
        || total_u32_slots > packet_compositions.capacity()
    {
        return false;
    }

    let Ok(zero_budget) = EnergyBudget::new(0.0) else {
        return false;
    };
    let placeholder = (
        UptakeSettlement {
            absorbed: 0.0,
            gain: 0.0,
            toxicity: flife_core::ledger::Payment {
                requested: 0.0,
                paid: 0.0,
            },
        },
        zero_budget,
    );
    set_len_reuse(spatial_uptake, count, placeholder);
    set_len_reuse(cell_compositions, total_comp_slots, [0.0; CHANNELS]);
    set_len_reuse(packet_compositions, total_u32_slots, 0_u32);

    // Keep normal spatial macro-tiles unsplit so each tile's halo cells are
    // composed once, while still splitting extreme hotspot tiles for balance.
    let max_sub_tile = count
        .div_ceil(workers.saturating_mul(2))
        .max(MINIMUM_ROWS_PER_LANE);
    struct FusedChemTile<'a> {
        tx: usize,
        ty: usize,
        row_indices: &'a [u32],
        out: &'a mut [(UptakeSettlement, EnergyBudget)],
    }
    let mut tiles: Vec<FusedChemTile<'_>> = Vec::with_capacity(SPATIAL_TILES);
    let mut rem_out = spatial_uptake.as_mut_slice();
    for t in 0..SPATIAL_TILES {
        let base = spatial_tile_offsets[t];
        let end = spatial_tile_offsets[t + 1];
        let tile_count = end.saturating_sub(base);
        if tile_count == 0 {
            continue;
        }
        let tx = t % SPATIAL_NX;
        let ty = t / SPATIAL_NX;
        let mut off = 0_usize;
        while off < tile_count {
            let sub_len = (tile_count - off).min(max_sub_tile);
            let (out_head, out_tail) = rem_out.split_at_mut(sub_len);
            rem_out = out_tail;
            tiles.push(FusedChemTile {
                tx,
                ty,
                row_indices: &spatial_rows[base + off..base + off + sub_len],
                out: out_head,
            });
            off += sub_len;
        }
    }

    struct ChemLane<'a> {
        comps: &'a mut [[f64; CHANNELS]],
        halo_to_local: &'a mut [u32],
        tile_cells: &'a mut [u32],
    }
    let mut chem_lanes: Vec<ChemLane<'_>> = Vec::with_capacity(workers);
    let mut rem_comps = cell_compositions.as_mut_slice();
    let mut rem_u32 = packet_compositions.as_mut_slice();
    for _ in 0..workers {
        let (comps, next_comps) = rem_comps.split_at_mut(halo_cells);
        rem_comps = next_comps;
        let (lane_u32, next_u32) = rem_u32.split_at_mut(u32_per_lane);
        rem_u32 = next_u32;
        let (halo_to_local, tile_cells) = lane_u32.split_at_mut(halo_cells);
        chem_lanes.push(ChemLane {
            comps,
            halo_to_local,
            tile_cells,
        });
    }

    let x_shift = gx.trailing_zeros();
    let x_mask = gx - 1;
    let y_mask = gy - 1;

    let ok = flife_core::pool::try_parallel_tasks_with_lanes(
        workers,
        "phase 6 fused spatial chemistry",
        &mut chem_lanes,
        &mut tiles,
        |_, tile, lane| {
            let x0 = (tile.tx * tile_w + gx - 1) & x_mask;
            let y0 = (tile.ty * tile_h + gy - 1) & y_mask;

            // Step 1: Discover all occupied halo cells directly from
            // `grants_bins.offsets()` in ascending row-major order.
            let mut tile_distinct = 0_usize;
            let mut halo_idx = 0_usize;
            if x0 + halo_w <= gx {
                for dy in 0..halo_h {
                    let cy = (y0 + dy) & y_mask;
                    let row_start = (cy << x_shift) + x0;
                    let row_offs = &grant_offsets[row_start..row_start + halo_w + 1];
                    for (dx, pair) in row_offs.windows(2).enumerate() {
                        if pair[0] < pair[1] {
                            #[expect(
                                clippy::cast_possible_truncation,
                                reason = "tile_distinct < halo_cells <= MAX_HALO_CELLS <= u32::MAX"
                            )]
                            {
                                lane.halo_to_local[halo_idx] = tile_distinct as u32;
                            }
                            #[expect(
                                clippy::cast_possible_truncation,
                                reason = "row_start + dx < total_cells < u32::MAX"
                            )]
                            {
                                lane.tile_cells[tile_distinct] = (row_start + dx) as u32;
                            }
                            tile_distinct += 1;
                        } else {
                            lane.halo_to_local[halo_idx] = u32::MAX;
                        }
                        halo_idx += 1;
                    }
                }
            } else {
                for dy in 0..halo_h {
                    let cy = (y0 + dy) & y_mask;
                    let row_base = cy << x_shift;
                    for dx in 0..halo_w {
                        let cx = (x0 + dx) & x_mask;
                        let cell = row_base + cx;
                        if grant_offsets[cell] < grant_offsets[cell + 1] {
                            #[expect(
                                clippy::cast_possible_truncation,
                                reason = "tile_distinct < halo_cells <= MAX_HALO_CELLS <= u32::MAX"
                            )]
                            {
                                lane.halo_to_local[halo_idx] = tile_distinct as u32;
                            }
                            #[expect(
                                clippy::cast_possible_truncation,
                                reason = "cell < total_cells < u32::MAX"
                            )]
                            {
                                lane.tile_cells[tile_distinct] = cell as u32;
                            }
                            tile_distinct += 1;
                        } else {
                            lane.halo_to_local[halo_idx] = u32::MAX;
                        }
                        halo_idx += 1;
                    }
                }
            }

            // Step 2: Compute compositions of all occupied halo cells into L2.
            if tile_distinct > 0 {
                compute_tile_compositions(
                    observed,
                    &lane.tile_cells[..tile_distinct],
                    &mut lane.comps[..tile_distinct],
                )?;
            }

            // Step 3: Settle chemistry in pairs of organisms while
            // `lane.comps[..tile_distinct]` stays hot in L2 cache so
            // `dot4_channels_pair` saturates all 4 FP add pipelines.
            let comps_ready = &lane.comps[..tile_distinct];
            let halo_map = &lane.halo_to_local[..halo_cells];
            #[derive(Clone, Copy)]
            struct HaloGeom {
                total_cells: usize,
                x_mask: usize,
                y_mask: usize,
                x_shift: u32,
                x0: usize,
                y0: usize,
                halo_w: usize,
                halo_h: usize,
                tile_distinct: usize,
            }
            struct LoadedOrg<'a> {
                aff: &'a [f64; CHANNELS],
                budget: EnergyBudget,
                pkts: [(f64, u32); PACKETS_PER_ORGANISM],
                n: usize,
            }
            #[inline(always)]
            fn load_org<'a>(
                row_u32: u32,
                ordered_slots: &[u32],
                population: &'a Population,
                by_demand: &Bins,
                granted: &[GrantPacket],
                halo_map: &[u32],
                geom: HaloGeom,
            ) -> Result<LoadedOrg<'a>, CoreError> {
                let row_idx = row_u32 as usize;
                let phys_slot = ordered_slots[row_idx] as usize;
                let Some((energy, genome)) = population.chemistry_slot_read(phys_slot) else {
                    return Err(CoreError::StateConflict {
                        operation: "fused spatial chemistry prepass",
                        detail: format!("slot {phys_slot} missing occupied state"),
                    });
                };
                let bucket = by_demand.bucket(row_idx);
                if bucket.len() > PACKETS_PER_ORGANISM {
                    return Err(CoreError::StateConflict {
                        operation: "fused spatial chemistry prepass",
                        detail: "bucket exceeded grid-cic packet count".to_owned(),
                    });
                }
                let mut pkts = [(0.0_f64, 0_u32); PACKETS_PER_ORGANISM];
                let mut n = 0_usize;
                for &at in bucket {
                    let packet =
                        granted
                            .get(at as usize)
                            .ok_or_else(|| CoreError::StateConflict {
                                operation: "fused spatial chemistry prepass",
                                detail: format!("missing grant packet {at}"),
                            })?;
                    let cell = packet.cell;
                    if cell >= geom.total_cells {
                        return Err(CoreError::LayoutMismatch {
                            buffer: "field",
                            detail: format!("cell {cell} is not in this field"),
                        });
                    }
                    let dx = (cell & geom.x_mask).wrapping_sub(geom.x0) & geom.x_mask;
                    let dy = (cell >> geom.x_shift).wrapping_sub(geom.y0) & geom.y_mask;
                    if dx >= geom.halo_w || dy >= geom.halo_h {
                        return Err(CoreError::StateConflict {
                            operation: "fused spatial chemistry prepass",
                            detail: "grant cell fell outside spatial tile halo".to_owned(),
                        });
                    }
                    let local_idx = halo_map[dy * geom.halo_w + dx];
                    if (local_idx as usize) >= geom.tile_distinct {
                        return Err(CoreError::StateConflict {
                            operation: "fused spatial chemistry prepass",
                            detail: "grant cell missing from halo composition map".to_owned(),
                        });
                    }
                    pkts[n] = (packet.granted, local_idx);
                    n += 1;
                }
                let budget = EnergyBudget::new(energy)?;
                Ok(LoadedOrg {
                    aff: genome.traits().affinity(),
                    budget,
                    pkts,
                    n,
                })
            }
            let geom = HaloGeom {
                total_cells,
                x_mask,
                y_mask,
                x_shift,
                x0,
                y0,
                halo_w,
                halo_h,
                tile_distinct,
            };

            let (row_pairs, row_rem) = tile.row_indices.as_chunks::<2>();
            let (out_pairs, out_rem) = tile.out.as_chunks_mut::<2>();
            for (&[row_a, row_b], [out_a, out_b]) in row_pairs.iter().zip(out_pairs.iter_mut()) {
                let mut a = load_org(
                    row_a,
                    ordered_slots,
                    population,
                    by_demand,
                    granted,
                    halo_map,
                    geom,
                )?;
                let mut b = load_org(
                    row_b,
                    ordered_slots,
                    population,
                    by_demand,
                    granted,
                    halo_map,
                    geom,
                )?;
                if a.n == PACKETS_PER_ORGANISM && b.n == PACKETS_PER_ORGANISM {
                    let (sa, sb) = rules.chemistry.settle_indexed_pair_4(
                        a.aff,
                        a.pkts,
                        &mut a.budget,
                        b.aff,
                        b.pkts,
                        &mut b.budget,
                        comps_ready,
                    )?;
                    *out_a = (sa, a.budget);
                    *out_b = (sb, b.budget);
                } else {
                    let sa = rules.chemistry.settle_indexed_slice(
                        a.aff,
                        &a.pkts[..a.n],
                        comps_ready,
                        &mut a.budget,
                    )?;
                    let sb = rules.chemistry.settle_indexed_slice(
                        b.aff,
                        &b.pkts[..b.n],
                        comps_ready,
                        &mut b.budget,
                    )?;
                    *out_a = (sa, a.budget);
                    *out_b = (sb, b.budget);
                }
            }
            for (&row_u32, out_slot) in row_rem.iter().zip(out_rem.iter_mut()) {
                let mut org = load_org(
                    row_u32,
                    ordered_slots,
                    population,
                    by_demand,
                    granted,
                    halo_map,
                    geom,
                )?;
                let settlement = rules.chemistry.settle_indexed_slice(
                    org.aff,
                    &org.pkts[..org.n],
                    comps_ready,
                    &mut org.budget,
                )?;
                *out_slot = (settlement, org.budget);
            }

            Ok(())
        },
    )
    .is_ok();

    if !ok {
        spatial_uptake.clear();
        return false;
    }
    true
}

/// Precompute Phase-6 chemistry settlements in 8×8 spatial macro-tile order
/// reusing Pass 1's `spatial_rows` and `spatial_tile_offsets`. Each tile's
/// deduplicated `compositions` working set is ~1 MiB (L2-resident) instead of
/// randomly striding across the full 67 MiB table in identity order.
///
/// Any error clears `spatial_uptake` so [`settle_all`] executes chemistry in
/// canonical ascending identity order and reports the exact first error.
#[expect(
    clippy::too_many_arguments,
    reason = "private Phase-6 spatial chemistry pre-pass taking the tick's grants, compositions, and spatial buffers"
)]
fn spatial_chemistry_prepass(
    rules: &Rules,
    population: &Population,
    grants: (&Bins, &[GrantPacket], &[[f64; CHANNELS]], &[u32]),
    count: usize,
    workers: usize,
    spatial_rows: &[u32],
    row_to_spatial: &[u32],
    spatial_tile_offsets: &[usize; SPATIAL_TILES + 1],
    spatial_uptake: &mut Vec<(UptakeSettlement, EnergyBudget)>,
) {
    spatial_uptake.clear();
    let ordered_slots = population.ordered_slots();
    if workers <= 1
        || count == 0
        || row_to_spatial.len() != count
        || spatial_rows.len() != count
        || spatial_tile_offsets[SPATIAL_TILES] != count
        || ordered_slots.len() != count
        || spatial_uptake.capacity() < count
        || rules.v2.affinity_rotation_enabled()
        || rules.limits.affinity_norm() > rules.chemistry.affinity_norm_limit()
        || !rules.exchange.is_grid_cic()
    {
        return;
    }
    let Ok(zero_budget) = EnergyBudget::new(0.0) else {
        return;
    };
    let placeholder = (
        UptakeSettlement {
            absorbed: 0.0,
            gain: 0.0,
            toxicity: flife_core::ledger::Payment {
                requested: 0.0,
                paid: 0.0,
            },
        },
        zero_budget,
    );
    set_len_reuse(spatial_uptake, count, placeholder);

    let (by_demand, granted, compositions, packet_compositions) = grants;
    let max_sub_tile = count
        .div_ceil(workers.saturating_mul(4))
        .max(MINIMUM_ROWS_PER_LANE);
    struct ChemTile<'a> {
        row_indices: &'a [u32],
        out: &'a mut [(UptakeSettlement, EnergyBudget)],
    }
    let mut tiles: Vec<ChemTile<'_>> = Vec::with_capacity(SPATIAL_TILES);
    let mut rem_out = spatial_uptake.as_mut_slice();
    for t in 0..SPATIAL_TILES {
        let base = spatial_tile_offsets[t];
        let end = spatial_tile_offsets[t + 1];
        let tile_count = end.saturating_sub(base);
        if tile_count == 0 {
            continue;
        }
        let mut off = 0_usize;
        while off < tile_count {
            let sub_len = (tile_count - off).min(max_sub_tile);
            let (out_head, out_tail) = rem_out.split_at_mut(sub_len);
            rem_out = out_tail;
            tiles.push(ChemTile {
                row_indices: &spatial_rows[base + off..base + off + sub_len],
                out: out_head,
            });
            off += sub_len;
        }
    }
    let ok = flife_core::pool::try_parallel_tasks(
        workers,
        "phase 6 spatial chemistry",
        &mut tiles,
        |_, tile| {
            let mut stack_packets = [(0.0_f64, 0_u32); PACKETS_PER_ORGANISM];
            for (&row_u32, out_slot) in tile.row_indices.iter().zip(tile.out.iter_mut()) {
                let row_idx = row_u32 as usize;
                let phys_slot = ordered_slots[row_idx] as usize;
                let Some(energy) = population.energy_at_slot(phys_slot) else {
                    return Err(CoreError::StateConflict {
                        operation: "spatial chemistry prepass",
                        detail: format!("slot {phys_slot} missing energy"),
                    });
                };
                if population.id_at_slot(phys_slot).is_none() {
                    return Err(CoreError::StateConflict {
                        operation: "read genome",
                        detail: format!("slot {phys_slot} vacant"),
                    });
                }
                let (genome, _) = population.genome_and_arena_at_slot(phys_slot);
                let genome = genome.ok_or_else(|| CoreError::StateConflict {
                    operation: "read genome",
                    detail: format!("slot {phys_slot} has no genome"),
                })?;
                let bucket = by_demand.bucket(row_idx);
                if bucket.len() > PACKETS_PER_ORGANISM {
                    return Err(CoreError::StateConflict {
                        operation: "spatial chemistry prepass",
                        detail: "bucket exceeded grid-cic packet count".to_owned(),
                    });
                }
                let mut n = 0_usize;
                for &at in bucket {
                    let at = at as usize;
                    if let (Some(packet), Some(&comp_idx)) =
                        (granted.get(at), packet_compositions.get(at))
                    {
                        stack_packets[n] = (packet.granted, comp_idx);
                        n += 1;
                    }
                }
                let mut budget = EnergyBudget::new(energy)?;
                let settlement = rules.chemistry.settle_indexed_slice(
                    genome.traits().affinity(),
                    &stack_packets[..n],
                    compositions,
                    &mut budget,
                )?;
                *out_slot = (settlement, budget);
            }
            Ok(())
        },
    )
    .is_ok();
    if !ok {
        spatial_uptake.clear();
    }
}

/// Phases 6 and 7 for every working organism, on the lanes (or pre-reserved
/// `settle_tiles` for large multi-worker `grid-cic-v1` worlds): each unit
/// settles a contiguous identity-ordered run and keeps, in identity order, its
/// ledger flows, staged emissions, and watched organism's transactions.
/// Returns the number of populated `settle_tiles` (`0` when results live in
/// `lanes`). The error returned is the first in identity order, as a single
/// thread would report it.
#[expect(
    clippy::too_many_arguments,
    reason = "private phase-6/7 runner receiving the tick's working state and reusable tile buffers"
)]
fn settle_all(
    rules: &Rules,
    population: &Population,
    grants: (&Bins, &[GrantPacket], &[[f64; CHANNELS]], &[u32]),
    spatial_precomputed: (&[u32], &[(UptakeSettlement, EnergyBudget)]),
    from: Tick,
    watched: Option<OrganismId>,
    decisions: &[Decision],
    working: &mut [Working],
    lanes: &mut [DecisionLane],
    settle_tiles: &mut Vec<SettleTileBuf>,
) -> Result<usize, CoreError> {
    let (by_demand, granted, compositions, packet_compositions) = grants;
    let (row_to_spatial, spatial_uptake) = spatial_precomputed;
    let use_precomputed = !spatial_uptake.is_empty()
        && spatial_uptake.len() == working.len()
        && row_to_spatial.len() == working.len();
    for lane in lanes.iter_mut() {
        lane.flows.clear();
        lane.emissions.clear();
        if let Some(trail) = lane.trail_emissions.as_mut() {
            trail.clear();
        }
        if let Some(byproducts) = lane.byproducts.as_mut() {
            byproducts.clear();
        }
        lane.transactions = None;
    }
    let worker_budget = lanes.len();
    if worker_budget > 1
        && !working.is_empty()
        && !settle_tiles.is_empty()
        && matches!(rules.exchange, ExchangeKernel::GridCicV1)
    {
        let has_trail = rules.v2.dual_carrier().is_some();
        let has_byproducts = rules.v2.waste().is_some();
        let tile_count = settle_tiles
            .len()
            .min(working.len().div_ceil(MINIMUM_ROWS_PER_LANE).max(1))
            .max(1);
        let sub_len = working.len().div_ceil(tile_count).max(1);
        let active_tiles = working.len().div_ceil(sub_len);
        if settle_tiles.len() < active_tiles {
            settle_tiles
                .try_reserve_exact(active_tiles - settle_tiles.len())
                .map_err(|_| CoreError::AllocationFailed {
                    buffer: "settle tiles",
                    bytes: bytes_for::<SettleTileBuf>(active_tiles),
                })?;
            while settle_tiles.len() < active_tiles {
                settle_tiles.push(SettleTileBuf::default());
            }
        }
        for tile in &mut settle_tiles[..active_tiles] {
            if tile.rows < sub_len
                || tile.trail_emissions.is_some() != has_trail
                || tile.byproducts.is_some() != has_byproducts
            {
                tile.fit(sub_len, has_trail, has_byproducts)?;
            } else {
                tile.clear();
            }
        }
        let mut tasks: Vec<(usize, &mut [Working], &mut SettleTileBuf)> = working
            .chunks_mut(sub_len)
            .zip(settle_tiles[..active_tiles].iter_mut())
            .enumerate()
            .map(|(idx, (sub_run, buf))| (idx * sub_len, sub_run, buf))
            .collect();
        flife_core::pool::try_parallel_tasks_with_lanes(
            worker_budget,
            "settle",
            lanes,
            &mut tasks,
            |_, (start, sub_run, buf), lane| {
                let DecisionLane {
                    exchange, packets, ..
                } = lane;
                let mut live = sub_run
                    .first()
                    .map(|first| population.ascending_from(first.committed.id));
                let mut stack_packets = [(0.0_f64, 0_u32); PACKETS_PER_ORGANISM];
                for (offset, organism) in sub_run.iter_mut().enumerate() {
                    let row_idx = *start + offset;
                    let precomputed_uptake: Option<PrecomputedUptake> =
                        use_precomputed.then(|| spatial_uptake[row_to_spatial[row_idx] as usize]);
                    let packet_slice: &[(f64, u32)] = if precomputed_uptake.is_some() {
                        &[]
                    } else {
                        let bucket = by_demand.bucket(row_idx);
                        if bucket.len() <= PACKETS_PER_ORGANISM {
                            let mut n = 0_usize;
                            for &at in bucket {
                                let at = at as usize;
                                if let (Some(packet), Some(&comp_idx)) =
                                    (granted.get(at), packet_compositions.get(at))
                                {
                                    stack_packets[n] = (packet.granted, comp_idx);
                                    n += 1;
                                }
                            }
                            &stack_packets[..n]
                        } else {
                            packets.clear();
                            for &at in bucket {
                                let at = at as usize;
                                if let (Some(packet), Some(&comp_idx)) =
                                    (granted.get(at), packet_compositions.get(at))
                                {
                                    packets.push((packet.granted, comp_idx));
                                }
                            }
                            packets.as_slice()
                        }
                    };
                    let genome = live
                        .as_mut()
                        .and_then(|walk| walk.next(organism.committed.id).0)
                        .ok_or_else(|| CoreError::StateConflict {
                            operation: "read genome",
                            detail: format!("{} has no genome", organism.committed.id),
                        })?;
                    let decision = organism.decision(decisions);
                    let traits = genome.traits();
                    let mut stages = Reference {
                        traits,
                        decision,
                        parameters: parameter_count(&rules.catalog, genome)?,
                        packets: packet_slice,
                        compositions,
                        precomputed_uptake,
                    };
                    let record =
                        (watched == Some(organism.committed.id)).then_some(&mut buf.transactions);
                    settle_stages(
                        rules,
                        organism,
                        decision,
                        traits,
                        &mut stages,
                        from,
                        &mut buf.flows,
                        &mut buf.emissions,
                        buf.trail_emissions.as_mut(),
                        buf.byproducts.as_mut(),
                        exchange,
                        record,
                    )?
                    .apply(organism);
                }
                Ok(())
            },
        )?;
        return Ok(active_tiles);
    }
    in_runs_mut(working, lanes, "settle", &|start, run, lane| {
        let DecisionLane {
            exchange,
            packets,
            flows,
            emissions,
            trail_emissions,
            byproducts,
            transactions,
            ..
        } = lane;
        let mut live = run
            .first()
            .map(|first| population.ascending_from(first.committed.id));
        let mut stack_packets = [(0.0_f64, 0_u32); PACKETS_PER_ORGANISM];
        for (offset, organism) in run.iter_mut().enumerate() {
            let row_idx = start + offset;
            let precomputed_uptake: Option<PrecomputedUptake> =
                use_precomputed.then(|| spatial_uptake[row_to_spatial[row_idx] as usize]);
            let packet_slice: &[(f64, u32)] = if precomputed_uptake.is_some() {
                &[]
            } else {
                let bucket = by_demand.bucket(row_idx);
                if bucket.len() <= PACKETS_PER_ORGANISM {
                    let mut n = 0_usize;
                    for &at in bucket {
                        let at = at as usize;
                        if let (Some(packet), Some(&comp_idx)) =
                            (granted.get(at), packet_compositions.get(at))
                        {
                            stack_packets[n] = (packet.granted, comp_idx);
                            n += 1;
                        }
                    }
                    &stack_packets[..n]
                } else {
                    packets.clear();
                    for &at in bucket {
                        let at = at as usize;
                        if let (Some(packet), Some(&comp_idx)) =
                            (granted.get(at), packet_compositions.get(at))
                        {
                            packets.push((packet.granted, comp_idx));
                        }
                    }
                    packets.as_slice()
                }
            };
            let genome = live
                .as_mut()
                .and_then(|walk| walk.next(organism.committed.id).0)
                .ok_or_else(|| CoreError::StateConflict {
                    operation: "read genome",
                    detail: format!("{} has no genome", organism.committed.id),
                })?;
            let decision = organism.decision(decisions);
            let traits = genome.traits();
            let mut stages = Reference {
                traits,
                decision,
                parameters: parameter_count(&rules.catalog, genome)?,
                packets: packet_slice,
                compositions,
                precomputed_uptake,
            };
            let record = (watched == Some(organism.committed.id)).then_some(&mut *transactions);
            settle_stages(
                rules,
                organism,
                decision,
                traits,
                &mut stages,
                from,
                flows,
                emissions,
                trail_emissions.as_mut(),
                byproducts.as_mut(),
                exchange,
                record,
            )?
            .apply(organism);
        }
        Ok(())
    })?;
    Ok(0)
}

/// The fewest unique debited cells a lane takes in [`compositions_into`].
const MINIMUM_CELLS_PER_LANE: usize = 256;

/// Phase 6's deduplicated cell compositions: computes one `[f64; CHANNELS]`
/// composition per unique debited cell into `out` and maps every packet `at`
/// in `granted` to its index in `packet_indices[at]` (`4 B` per packet vs
/// `256 B` broadcast), on at most `lanes` threads. A composition depends only
/// on the observed field and the cell, so tile splitting changes no bit.
#[expect(
    clippy::too_many_lines,
    reason = "deduplicated plane-major cell composition builder with dynamic work-stealing tiles and fallback"
)]
fn compositions_into(
    observed: &FieldState,
    granted: &[GrantPacket],
    grants_bins: &Bins,
    lanes: usize,
    out: &mut Vec<[f64; CHANNELS]>,
    packet_indices: &mut Vec<u32>,
) -> Result<(), CoreError> {
    if granted.is_empty() {
        out.clear();
        packet_indices.clear();
        return Ok(());
    }
    if packet_indices.len() < granted.len() {
        reserve_for(
            packet_indices,
            granted.len() - packet_indices.len(),
            "packet composition indices",
        )?;
        packet_indices.resize(granted.len(), 0);
    }
    let pkt_idx_slice =
        packet_indices
            .get_mut(..granted.len())
            .ok_or_else(|| CoreError::StateConflict {
                operation: "compose packets",
                detail: format!("no room for {} packet composition indices", granted.len()),
            })?;
    let energy_plane = observed.energy();
    let total_cells = energy_plane.len();
    let all_data = observed.as_slice();
    let planes = observed.planes() as usize;
    let active_channels = observed.carrier0_moments_count();
    let planar_ok = all_data.len() == planes.saturating_mul(total_cells);
    let occupied = grants_bins.occupied_buckets();
    let offsets = grants_bins.offsets();

    if planar_ok && grants_bins.order().len() == granted.len() && !occupied.is_empty() {
        let distinct = occupied.len();
        if out.len() < distinct {
            reserve_for(out, distinct - out.len(), "packet compositions")?;
            out.resize(distinct, [0.0; CHANNELS]);
        }
        let comp_slice = out
            .get_mut(..distinct)
            .ok_or_else(|| CoreError::StateConflict {
                operation: "compose packets",
                detail: format!("no room for {distinct} compositions"),
            })?;
        let run_cells = |occ_base: usize,
                         occ_chunk: &[u32],
                         comp_chunk: &mut [[f64; CHANNELS]],
                         pkt_base: usize,
                         pkt_chunk: &mut [u32]|
         -> Result<(), CoreError> {
            // Batch up to 32 distinct nonempty cells and read moment planes
            // plane-by-plane so 512² (1 MiB plane stride) avoids 33-way L1D
            // cache-set conflict misses while writing each cell once.
            const COMP_BATCH: usize = 32;
            let mut cells = [0_usize; COMP_BATCH];
            let mut energies = [0.0_f64; COMP_BATCH];
            let mut slots = [0_usize; COMP_BATCH];
            let mut raw = [0.0_f32; COMP_BATCH];
            let mut quotients = [0.0_f64; COMP_BATCH];
            let mut i = 0_usize;
            let n = occ_chunk.len();
            while i < n {
                let batch_start = i;
                let mut count = 0_usize;
                while i < n && count < COMP_BATCH {
                    let cell = occ_chunk[i] as usize;
                    let &e_raw =
                        energy_plane
                            .get(cell)
                            .ok_or_else(|| CoreError::LayoutMismatch {
                                buffer: "field",
                                detail: format!("cell {cell} is not in this field"),
                            })?;
                    let first = offsets[cell] as usize;
                    let end = offsets[cell + 1] as usize;
                    let comp_id = u32::try_from(occ_base + i).map_err(|_| CoreError::Overflow {
                        quantity: "packet compositions",
                    })?;
                    let pkt_chunk_len = pkt_chunk.len();
                    let range_slice = first
                        .checked_sub(pkt_base)
                        .zip(end.checked_sub(pkt_base))
                        .and_then(|(rel_first, rel_end)| pkt_chunk.get_mut(rel_first..rel_end))
                        .ok_or_else(|| CoreError::StateConflict {
                            operation: "compose packets",
                            detail: format!(
                                "cell {cell} bin range {first}..{end} outside tile slice {pkt_base}..{}",
                                pkt_base + pkt_chunk_len
                            ),
                        })?;
                    range_slice.fill(comp_id);
                    let e = f64::from(e_raw);
                    if e > 0.0 {
                        cells[count] = cell;
                        energies[count] = e;
                        slots[count] = i;
                        if active_channels < CHANNELS {
                            comp_chunk[i][active_channels..].fill(0.0);
                        }
                        count += 1;
                    } else {
                        comp_chunk[i] = [0.0; CHANNELS];
                    }
                    i += 1;
                }
                if count > 0 {
                    let contiguous = i - batch_start == count;
                    for (channel, plane) in all_data[total_cells..]
                        .chunks_exact(total_cells)
                        .enumerate()
                        .take(active_channels)
                    {
                        // Separate power-of-two gather (`raw`) from division
                        // (`quotients`) so LLVM emits bounds-check-free NEON
                        // `fdiv.2d` across the full batch.
                        let plane_len = plane.len();
                        if plane_len.is_power_of_two() {
                            let mask = plane_len - 1;
                            for (dst, &cell) in raw[..count].iter_mut().zip(&cells[..count]) {
                                *dst = plane[cell & mask];
                            }
                        } else {
                            for (dst, &cell) in raw[..count].iter_mut().zip(&cells[..count]) {
                                *dst = plane[cell];
                            }
                        }
                        if count == COMP_BATCH {
                            for ((dst, &m), &energy) in
                                quotients.iter_mut().zip(&raw).zip(&energies)
                            {
                                *dst = f64::from(m) / energy;
                            }
                        } else {
                            for ((dst, &m), &energy) in quotients[..count]
                                .iter_mut()
                                .zip(&raw[..count])
                                .zip(&energies[..count])
                            {
                                *dst = f64::from(m) / energy;
                            }
                        }
                        if contiguous {
                            for (comp, &q) in comp_chunk[batch_start..batch_start + count]
                                .iter_mut()
                                .zip(&quotients[..count])
                            {
                                comp[channel] = q;
                            }
                        } else {
                            for (&slot, &q) in slots[..count].iter().zip(&quotients[..count]) {
                                comp_chunk[slot][channel] = q;
                            }
                        }
                    }
                }
            }
            Ok(())
        };

        let base_used = lanes.min(distinct.div_ceil(MINIMUM_CELLS_PER_LANE)).max(1);
        if base_used == 1 {
            let pkt_base = offsets[occupied[0] as usize] as usize;
            return run_cells(0, occupied, comp_slice, pkt_base, pkt_idx_slice);
        }
        let tile_count = lanes
            .saturating_mul(4)
            .min(64)
            .min(distinct.div_ceil(MINIMUM_CELLS_PER_LANE))
            .max(base_used);
        let tile_len = distinct.div_ceil(tile_count);
        struct CompTile<'a> {
            occ_base: usize,
            occ: &'a [u32],
            comps: &'a mut [[f64; CHANNELS]],
            pkt_base: usize,
            pkt_idx: &'a mut [u32],
        }
        let mut tiles = Vec::with_capacity(tile_count);
        let mut rem_comp = comp_slice;
        let mut rem_pkt = pkt_idx_slice;
        let mut pkt_cursor = 0_usize;
        for (t_idx, occ_chunk) in occupied.chunks(tile_len).enumerate() {
            let (comp_head, comp_tail) = rem_comp.split_at_mut(occ_chunk.len());
            rem_comp = comp_tail;
            let last_cell = *occ_chunk.last().unwrap_or(&0) as usize;
            let pkt_end = offsets[last_cell + 1] as usize;
            let pkt_len = pkt_end.saturating_sub(pkt_cursor);
            let (pkt_head, pkt_tail) = rem_pkt.split_at_mut(pkt_len);
            rem_pkt = pkt_tail;
            tiles.push(CompTile {
                occ_base: t_idx * tile_len,
                occ: occ_chunk,
                comps: comp_head,
                pkt_base: pkt_cursor,
                pkt_idx: pkt_head,
            });
            pkt_cursor = pkt_end;
        }
        return flife_core::pool::try_parallel_tasks(
            lanes,
            "compose packets",
            &mut tiles,
            |_, tile| {
                run_cells(
                    tile.occ_base,
                    tile.occ,
                    tile.comps,
                    tile.pkt_base,
                    tile.pkt_idx,
                )
            },
        );
    }

    // Fallback when field is non-planar or `grants_bins` does not match `granted`.
    out.clear();
    let mut previous_cell: Option<usize> = None;
    let mut current_idx = 0_u32;
    for (packet, idx_slot) in granted.iter().zip(pkt_idx_slice.iter_mut()) {
        if previous_cell != Some(packet.cell) {
            let value = composition(observed, packet.cell)?;
            current_idx = u32::try_from(out.len()).map_err(|_| CoreError::Overflow {
                quantity: "packet compositions",
            })?;
            reserve_for(out, 1, "packet compositions")?;
            out.push(value);
            previous_cell = Some(packet.cell);
        }
        *idx_slot = current_idx;
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
    decisions: &mut Vec<Decision>,
    lanes: &mut [DecisionLane],
) -> Result<usize, CoreError> {
    let dual_carrier = rules.v2.dual_carrier().is_some();
    if rows.is_empty() {
        decisions.clear();
        return Ok(0);
    }
    reserve_for(
        decisions,
        rows.len().saturating_sub(decisions.len()),
        "decisions",
    )?;
    set_len_reuse(decisions, rows.len(), Decision::empty());
    let worker_budget = lanes.len().max(1);
    let max_tiles = rows.len().div_ceil(MINIMUM_ROWS_PER_LANE).max(1);
    let tile_count = if worker_budget <= 1 || max_tiles <= 1 {
        1
    } else {
        worker_budget
            .saturating_mul(4)
            .min(64)
            .min(max_tiles)
            .max(worker_budget.min(max_tiles))
    };
    let tile_len = rows.len().div_ceil(tile_count);
    struct DecideTile<'a> {
        start: usize,
        run: &'a [Organism],
        dec_out: &'a mut [Decision],
        evaluated: usize,
    }
    let mut tiles: Vec<DecideTile<'_>> = rows
        .chunks(tile_len)
        .zip(decisions.chunks_mut(tile_len))
        .enumerate()
        .map(|(idx, (run, dec_out))| DecideTile {
            start: idx * tile_len,
            run,
            dec_out,
            evaluated: 0,
        })
        .collect();
    flife_core::pool::try_parallel_tasks_with_lanes(
        worker_budget,
        "decide",
        lanes,
        &mut tiles,
        |_, tile, lane| {
            let start = tile.start;
            let run = tile.run;
            let dec_out = &mut *tile.dec_out;
            let mut local_eval = 0_usize;
            let DecisionLane { controller, .. } = lane;
            let mut live = run.first().map(|first| population.ascending_from(first.id));
            if dual_carrier {
                if let Some(Sampled::Dual(samples)) = sampled {
                    for (index, (organism, slot)) in run.iter().zip(dec_out.iter_mut()).enumerate()
                    {
                        let genome = live
                            .as_mut()
                            .and_then(|walk| walk.next(organism.id).0)
                            .ok_or_else(|| CoreError::StateConflict {
                                operation: "read genome",
                                detail: format!("{} has no genome", organism.id),
                            })?;
                        let raw = match samples.get(start + index) {
                            Some(raw) => *raw,
                            None => view
                                .sample_dual(organism.position, genome.traits().sensing_scale())?,
                        };
                        *slot = decide_from_dual_raw(
                            rules,
                            genome,
                            organism,
                            &raw,
                            crowding_counts.get(start + index).copied(),
                            ticks,
                            controller,
                        )?;
                        local_eval += usize::from(slot.evaluated);
                    }
                    tile.evaluated = local_eval;
                    return Ok(());
                }
                let mut queries = [SensorQuery {
                    position: [0.0, 0.0],
                    scale: 1.0,
                }; SAMPLE_BATCH];
                let mut batch_genomes: [Option<&Genome>; SAMPLE_BATCH] = [None; SAMPLE_BATCH];
                for (chunk_idx, (chunk, dec_chunk)) in run
                    .chunks(SAMPLE_BATCH)
                    .zip(dec_out.chunks_mut(SAMPLE_BATCH))
                    .enumerate()
                {
                    let count = chunk.len();
                    let base = start + chunk_idx * SAMPLE_BATCH;
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
                        queries[i] = SensorQuery {
                            position: organism.position,
                            scale: genome.traits().sensing_scale(),
                        };
                    }
                    view.sample_dual_batch_each(&queries[..count], |i, raw| {
                        let Some(genome) = batch_genomes[i] else {
                            return Ok(());
                        };
                        let dec = decide_from_dual_raw(
                            rules,
                            genome,
                            &chunk[i],
                            raw,
                            crowding_counts.get(base + i).copied(),
                            ticks,
                            controller,
                        )?;
                        local_eval += usize::from(dec.evaluated);
                        dec_chunk[i] = dec;
                        Ok(())
                    })?;
                }
                tile.evaluated = local_eval;
                return Ok(());
            }
            if let Some(Sampled::Single(samples)) = sampled {
                for (index, (organism, slot)) in run.iter().zip(dec_out.iter_mut()).enumerate() {
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
                    *slot = decide_from_raw(
                        rules,
                        genome,
                        organism,
                        &raw,
                        crowding_counts.get(start + index).copied(),
                        ticks,
                        controller,
                    )?;
                    local_eval += usize::from(slot.evaluated);
                }
                tile.evaluated = local_eval;
                return Ok(());
            }
            let mut queries = [SensorQuery {
                position: [0.0, 0.0],
                scale: 1.0,
            }; SAMPLE_BATCH];
            let mut batch_genomes: [Option<&Genome>; SAMPLE_BATCH] = [None; SAMPLE_BATCH];
            for (chunk_idx, (chunk, dec_chunk)) in run
                .chunks(SAMPLE_BATCH)
                .zip(dec_out.chunks_mut(SAMPLE_BATCH))
                .enumerate()
            {
                let count = chunk.len();
                let base = start + chunk_idx * SAMPLE_BATCH;
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
                    queries[i] = SensorQuery {
                        position: organism.position,
                        scale: genome.traits().sensing_scale(),
                    };
                }
                view.sample_batch_each(&queries[..count], |i, raw| {
                    let Some(genome) = batch_genomes[i] else {
                        return Ok(());
                    };
                    let dec = decide_from_raw(
                        rules,
                        genome,
                        &chunk[i],
                        raw,
                        crowding_counts.get(base + i).copied(),
                        ticks,
                        controller,
                    )?;
                    local_eval += usize::from(dec.evaluated);
                    dec_chunk[i] = dec;
                    Ok(())
                })?;
            }
            tile.evaluated = local_eval;
            Ok(())
        },
    )?;
    Ok(tiles.iter().map(|t| t.evaluated).sum())
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

/// Minimum live organism count and grid cell count to partition flat-F32
/// Pass 1 and Pass 2 into 8×8 spatial macro-tiles so each worker keeps its
/// macro-tile slice in L2 and its 2-row fine strips in L1D instead of
/// randomly thrashing the full multi-plane field.
const SPATIAL_PASS1_MIN_ROWS: usize = 8_192;
const SPATIAL_PASS1_MIN_CELLS: u64 = 32_768;
const SPATIAL_NX: usize = 8;
const SPATIAL_NY: usize = 8;
const SPATIAL_TILES: usize = SPATIAL_NX * SPATIAL_NY;

/// Phases 3 and 4 for every row, with the due evaluations batched on the
/// world's backend if it has one. Any error on the batched path hands the
/// tick to [`decide_all`], which reproduces the reference's own error (or
/// finds none), so the result is always the reference's.
#[expect(
    clippy::too_many_arguments,
    reason = "phases 3 and 4's inputs, the backend's samples, post-Phase-1 crowding, spatial staging, and the controllers"
)]
fn decide_with(
    rules: &Rules,
    population: &Population,
    view: &PyramidView<'_>,
    sampled: Option<Sampled<'_>>,
    rows: &[Organism],
    crowding_counts: &[u32],
    ticks: [Tick; 2],
    decisions: &mut Vec<Decision>,
    spatial_rows: &mut Vec<u32>,
    row_to_spatial: &mut Vec<u32>,
    spatial_tile_offsets: &mut [usize; SPATIAL_TILES + 1],
    lanes: &mut [DecisionLane],
    controllers: &mut Controllers,
    overlap: Option<&mut OverlapPhase56<'_>>,
) -> Result<usize, CoreError> {
    let Controllers {
        backend,
        report,
        outputs,
        flat_inputs_f32,
        flat_inputs_finite,
        flat_outputs,
    } = controllers;
    let Some(device) = backend.as_deref_mut() else {
        row_to_spatial.clear();
        *spatial_tile_offsets = [0; SPATIAL_TILES + 1];
        return decide_all(
            rules,
            population,
            view,
            sampled,
            rows,
            crowding_counts,
            ticks,
            decisions,
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
            decisions,
            spatial_rows,
            row_to_spatial,
            spatial_tile_offsets,
            lanes,
            device,
            flat_inputs_f32,
            flat_inputs_finite,
            flat_outputs,
            overlap,
        )
    } else {
        row_to_spatial.clear();
        *spatial_tile_offsets = [0; SPATIAL_TILES + 1];
        decide_batched(
            rules,
            population,
            view,
            sampled,
            rows,
            crowding_counts,
            ticks,
            decisions,
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
            usize::try_from(batched.saturating_add(deferred)).map_err(|_| CoreError::Overflow {
                quantity: "evaluated organisms",
            })
        }
        Ok(Batched::Device(error)) => {
            report.failure = Some(error.to_string());
            *backend = None;
            report.reference_ticks = report.reference_ticks.saturating_add(1);
            row_to_spatial.clear();
            *spatial_tile_offsets = [0; SPATIAL_TILES + 1];
            decide_all(
                rules,
                population,
                view,
                sampled,
                rows,
                crowding_counts,
                ticks,
                decisions,
                lanes,
            )
        }
        Err(_) => {
            report.reference_ticks = report.reference_ticks.saturating_add(1);
            row_to_spatial.clear();
            *spatial_tile_offsets = [0; SPATIAL_TILES + 1];
            decide_all(
                rules,
                population,
                view,
                sampled,
                rows,
                crowding_counts,
                ticks,
                decisions,
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
    decisions_out: &mut [Decision],
    in_f32: &mut [f32],
    in_finite: &mut [u8],
    jobs_out: &mut Vec<FlatControllerJob<'p>>,
) -> Result<(), CoreError> {
    const FLAT_SAMPLE_BATCH: usize = 512;
    const NOT_DUE: u32 = u32::MAX;

    let sensors = rules.v2.sensors();
    let input_width = sensors.input_width();
    let obs_width = sensors.observation_width();
    let state_width = rules.recurrent_state_width;
    let needs_expanded = input_width > SENSOR_WIDTH;
    let dual_carrier = rules.v2.dual_carrier().is_some();

    let mut live = run.first().map(|first| population.ascending_from(first.id));
    let mut local_due = 0_usize;

    let finish_due_row = |dst: &mut [f32],
                          finite_slot: &mut u8,
                          row_idx: usize,
                          organism: &Organism,
                          trail_f32: Option<[f32; 4]>| {
        if needs_expanded {
            dst[SENSOR_WIDTH..].fill(0.0);
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
        *finite_slot = u8::from(dst.iter().all(|v| v.is_finite()));
    };

    let build_job = |organism: &Organism,
                     genome: &'p Genome,
                     slot: Option<ArenaSlot>|
     -> Result<FlatControllerJob<'p>, CoreError> {
        let arena_slot = slot.ok_or_else(|| CoreError::StateConflict {
            operation: "controller batch",
            detail: format!("{} holds no arena slot", organism.id),
        })?;
        Ok(FlatControllerJob {
            genome: organism.genome,
            slot: arena_slot,
            controller: genome.controller(&rules.catalog, &rules.limits)?,
        })
    };

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
                let dec = &mut decisions_out[index];
                if due(rules, organism, to) {
                    let base_off = local_due * input_width;
                    let dst = in_f32
                        .get_mut(base_off..base_off + input_width)
                        .ok_or_else(|| CoreError::StateConflict {
                            operation: "controller batch",
                            detail: "flat input buffer shorter than due count".to_owned(),
                        })?;
                    let base_arr: &mut [f32; SENSOR_WIDTH] = (&mut dst[..SENSOR_WIDTH])
                        .try_into()
                        .map_err(|_| CoreError::StateConflict {
                            operation: "controller batch",
                            detail: "flat input slice narrower than SENSOR_WIDTH".to_owned(),
                        })?;
                    let mut trail_f32 = [0.0_f32; 4];
                    encode_dual_for_controller_f32_into(
                        rules,
                        organism,
                        &raw,
                        from,
                        base_arr,
                        &mut trail_f32,
                        &mut dec.gradients,
                    )?;
                    let finite_slot =
                        in_finite
                            .get_mut(local_due)
                            .ok_or_else(|| CoreError::StateConflict {
                                operation: "controller batch",
                                detail: "flat finite flag buffer shorter than due count".to_owned(),
                            })?;
                    finish_due_row(dst, finite_slot, start + index, organism, Some(trail_f32));
                    jobs_out.push(build_job(organism, genome, slot)?);
                    local_due += 1;
                    // Pass 2 overwrites `dec.held` for every evaluated organism.
                    dec.evaluated = true;
                } else {
                    encode_dual_gradients_only_into(
                        rules,
                        organism,
                        &raw,
                        from,
                        &mut dec.gradients,
                    )?;
                    dec.held.copy_active_from(&organism.held);
                    dec.evaluated = false;
                    decode_held_into(rules, organism, &organism.held, &mut dec.actions)?;
                }
            }
            return Ok(());
        }
        let mut queries = [SensorQuery {
            position: [0.0, 0.0],
            scale: 1.0,
        }; FLAT_SAMPLE_BATCH];
        let mut batch_meta: [(Option<&'p Genome>, Option<ArenaSlot>, u32); FLAT_SAMPLE_BATCH] =
            [(None, None, NOT_DUE); FLAT_SAMPLE_BATCH];
        let mut chunk_jobs: [Option<FlatControllerJob<'p>>; FLAT_SAMPLE_BATCH] =
            std::array::from_fn(|_| None);
        for (chunk_idx, chunk) in run.chunks(FLAT_SAMPLE_BATCH).enumerate() {
            let count = chunk.len();
            let local_base = chunk_idx * FLAT_SAMPLE_BATCH;
            let base = start + local_base;
            let mut chunk_due = 0_usize;
            for i in 0..count {
                let organism = &chunk[i];
                let (genome, slot) = live
                    .as_mut()
                    .map_or((None, None), |walk| walk.next(organism.id));
                let genome = genome.ok_or_else(|| CoreError::StateConflict {
                    operation: "read genome",
                    detail: format!("{} has no genome", organism.id),
                })?;
                let due_slot = if due(rules, organism, to) {
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "chunk_due < FLAT_SAMPLE_BATCH (512) < u32::MAX"
                    )]
                    let d = chunk_due as u32;
                    chunk_due += 1;
                    d
                } else {
                    NOT_DUE
                };
                batch_meta[i] = (Some(genome), slot, due_slot);
                queries[i] = SensorQuery {
                    position: organism.position,
                    scale: genome.traits().sensing_scale(),
                };
            }
            let chunk_in_f32 = in_f32
                .get_mut(local_due * input_width..(local_due + chunk_due) * input_width)
                .ok_or_else(|| CoreError::StateConflict {
                    operation: "controller batch",
                    detail: "flat input buffer shorter than due count".to_owned(),
                })?;
            let chunk_in_fin = in_finite
                .get_mut(local_due..local_due + chunk_due)
                .ok_or_else(|| CoreError::StateConflict {
                    operation: "controller batch",
                    detail: "flat finite flag buffer shorter than due count".to_owned(),
                })?;
            let chunk_dec = decisions_out
                .get_mut(local_base..local_base + count)
                .ok_or_else(|| CoreError::StateConflict {
                    operation: "controller batch",
                    detail: "decision buffer shorter than organism chunk".to_owned(),
                })?;
            view.sample_dual_batch_each(&queries[..count], |i, raw| {
                let (Some(genome), slot, due_slot) = batch_meta[i] else {
                    return Ok(());
                };
                let organism = &chunk[i];
                let dec = &mut chunk_dec[i];
                if due_slot != NOT_DUE {
                    let d = due_slot as usize;
                    let dst = &mut chunk_in_f32[d * input_width..(d + 1) * input_width];
                    let base_arr: &mut [f32; SENSOR_WIDTH] = (&mut dst[..SENSOR_WIDTH])
                        .try_into()
                        .map_err(|_| CoreError::StateConflict {
                            operation: "controller batch",
                            detail: "flat input slice narrower than SENSOR_WIDTH".to_owned(),
                        })?;
                    let mut trail_f32 = [0.0_f32; 4];
                    encode_dual_for_controller_f32_into(
                        rules,
                        organism,
                        raw,
                        from,
                        base_arr,
                        &mut trail_f32,
                        &mut dec.gradients,
                    )?;
                    finish_due_row(
                        dst,
                        &mut chunk_in_fin[d],
                        base + i,
                        organism,
                        Some(trail_f32),
                    );
                    chunk_jobs[d] = Some(build_job(organism, genome, slot)?);
                    // Pass 2 overwrites `dec.held` for every evaluated organism.
                    dec.evaluated = true;
                } else {
                    encode_dual_gradients_only_into(
                        rules,
                        organism,
                        raw,
                        from,
                        &mut dec.gradients,
                    )?;
                    dec.held.copy_active_from(&organism.held);
                    dec.evaluated = false;
                    decode_held_into(rules, organism, &organism.held, &mut dec.actions)?;
                }
                Ok(())
            })?;
            for job in chunk_jobs[..chunk_due].iter_mut() {
                if let Some(job) = job.take() {
                    jobs_out.push(job);
                }
            }
            local_due += chunk_due;
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
            let dec = &mut decisions_out[index];
            if due(rules, organism, to) {
                let base_off = local_due * input_width;
                let dst = in_f32
                    .get_mut(base_off..base_off + input_width)
                    .ok_or_else(|| CoreError::StateConflict {
                        operation: "controller batch",
                        detail: "flat input buffer shorter than due count".to_owned(),
                    })?;
                let base_arr: &mut [f32; SENSOR_WIDTH] = (&mut dst[..SENSOR_WIDTH])
                    .try_into()
                    .map_err(|_| CoreError::StateConflict {
                        operation: "controller batch",
                        detail: "flat input slice narrower than SENSOR_WIDTH".to_owned(),
                    })?;
                encode_for_controller_f32_into(
                    rules,
                    organism,
                    &raw,
                    from,
                    base_arr,
                    &mut dec.gradients,
                )?;
                let finite_slot =
                    in_finite
                        .get_mut(local_due)
                        .ok_or_else(|| CoreError::StateConflict {
                            operation: "controller batch",
                            detail: "flat finite flag buffer shorter than due count".to_owned(),
                        })?;
                finish_due_row(dst, finite_slot, start + index, organism, None);
                jobs_out.push(build_job(organism, genome, slot)?);
                local_due += 1;
                // Pass 2 overwrites `dec.held` for every evaluated organism.
                dec.evaluated = true;
            } else {
                encode_gradients_only_into(rules, organism, &raw, from, &mut dec.gradients)?;
                dec.held = organism.held;
                dec.evaluated = false;
                decode_held_into(rules, organism, &organism.held, &mut dec.actions)?;
            }
        }
        return Ok(());
    }

    let mut queries = [SensorQuery {
        position: [0.0, 0.0],
        scale: 1.0,
    }; FLAT_SAMPLE_BATCH];
    let mut batch_meta: [(Option<&'p Genome>, Option<ArenaSlot>, u32); FLAT_SAMPLE_BATCH] =
        [(None, None, NOT_DUE); FLAT_SAMPLE_BATCH];
    let mut chunk_jobs: [Option<FlatControllerJob<'p>>; FLAT_SAMPLE_BATCH] =
        std::array::from_fn(|_| None);
    for (chunk_idx, chunk) in run.chunks(FLAT_SAMPLE_BATCH).enumerate() {
        let count = chunk.len();
        let local_base = chunk_idx * FLAT_SAMPLE_BATCH;
        let base = start + local_base;
        let mut chunk_due = 0_usize;
        for i in 0..count {
            let organism = &chunk[i];
            let (genome, slot) = live
                .as_mut()
                .map_or((None, None), |walk| walk.next(organism.id));
            let genome = genome.ok_or_else(|| CoreError::StateConflict {
                operation: "read genome",
                detail: format!("{} has no genome", organism.id),
            })?;
            let due_slot = if due(rules, organism, to) {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "chunk_due < FLAT_SAMPLE_BATCH (512) < u32::MAX"
                )]
                let d = chunk_due as u32;
                chunk_due += 1;
                d
            } else {
                NOT_DUE
            };
            batch_meta[i] = (Some(genome), slot, due_slot);
            queries[i] = SensorQuery {
                position: organism.position,
                scale: genome.traits().sensing_scale(),
            };
        }
        let chunk_in_f32 = in_f32
            .get_mut(local_due * input_width..(local_due + chunk_due) * input_width)
            .ok_or_else(|| CoreError::StateConflict {
                operation: "controller batch",
                detail: "flat input buffer shorter than due count".to_owned(),
            })?;
        let chunk_in_fin = in_finite
            .get_mut(local_due..local_due + chunk_due)
            .ok_or_else(|| CoreError::StateConflict {
                operation: "controller batch",
                detail: "flat finite flag buffer shorter than due count".to_owned(),
            })?;
        let chunk_dec = decisions_out
            .get_mut(local_base..local_base + count)
            .ok_or_else(|| CoreError::StateConflict {
                operation: "controller batch",
                detail: "decision buffer shorter than organism chunk".to_owned(),
            })?;
        view.sample_batch_each(&queries[..count], |i, raw| {
            let (Some(genome), slot, due_slot) = batch_meta[i] else {
                return Ok(());
            };
            let organism = &chunk[i];
            let dec = &mut chunk_dec[i];
            if due_slot != NOT_DUE {
                let d = due_slot as usize;
                let dst = &mut chunk_in_f32[d * input_width..(d + 1) * input_width];
                let base_arr: &mut [f32; SENSOR_WIDTH] = (&mut dst[..SENSOR_WIDTH])
                    .try_into()
                    .map_err(|_| CoreError::StateConflict {
                        operation: "controller batch",
                        detail: "flat input slice narrower than SENSOR_WIDTH".to_owned(),
                    })?;
                encode_for_controller_f32_into(
                    rules,
                    organism,
                    raw,
                    from,
                    base_arr,
                    &mut dec.gradients,
                )?;
                finish_due_row(dst, &mut chunk_in_fin[d], base + i, organism, None);
                chunk_jobs[d] = Some(build_job(organism, genome, slot)?);
                // Pass 2 overwrites `dec.held` for every evaluated organism.
                dec.evaluated = true;
            } else {
                encode_gradients_only_into(rules, organism, raw, from, &mut dec.gradients)?;
                dec.held = organism.held;
                dec.evaluated = false;
                decode_held_into(rules, organism, &organism.held, &mut dec.actions)?;
            }
            Ok(())
        })?;
        for job in chunk_jobs[..chunk_due].iter_mut() {
            if let Some(job) = job.take() {
                jobs_out.push(job);
            }
        }
        local_due += chunk_due;
    }
    Ok(())
}

/// Spatial-tile counterpart to [`pass1_flat_f32`]: processes organisms belonging
/// to one 8×8 spatial macro-tile (via `row_indices`) with O(1) physical-slot
/// lookup through `population.ordered_slots()`, keeping each 512-query pyramid
/// chunk inside one coarse macro-tile so [`PyramidView::sample_batch_each`]
/// activates fine 2-row strip sorting in L1D/L2.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "single-pass spatial macro-tile worker for L1D/L2-local sampling, F32 encoding, and job construction"
)]
fn pass1_flat_f32_spatial<'p>(
    rules: &'p Rules,
    population: &'p Population,
    ordered_slots: &[u32],
    view: &PyramidView<'_>,
    rows: &[Organism],
    crowding_counts: &[u32],
    [from, to]: [Tick; 2],
    row_indices: &[u32],
    decisions_out: &mut [Decision],
    in_f32: &mut [f32],
    in_finite: &mut [u8],
    jobs_out: &mut Vec<FlatControllerJob<'p>>,
) -> Result<(), CoreError> {
    const FLAT_SAMPLE_BATCH: usize = 512;
    const NOT_DUE: u32 = u32::MAX;

    let sensors = rules.v2.sensors();
    let input_width = sensors.input_width();
    let obs_width = sensors.observation_width();
    let state_width = rules.recurrent_state_width;
    let needs_expanded = input_width > SENSOR_WIDTH;
    let dual_carrier = rules.v2.dual_carrier().is_some();

    let mut local_due = 0_usize;

    let finish_due_row = |dst: &mut [f32],
                          finite_slot: &mut u8,
                          row_idx: usize,
                          organism: &Organism,
                          trail_f32: Option<[f32; 4]>| {
        if needs_expanded {
            dst[SENSOR_WIDTH..].fill(0.0);
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
        *finite_slot = u8::from(dst.iter().all(|v| v.is_finite()));
    };

    let build_job = |organism: &Organism,
                     genome: &'p Genome,
                     slot: Option<ArenaSlot>|
     -> Result<FlatControllerJob<'p>, CoreError> {
        let arena_slot = slot.ok_or_else(|| CoreError::StateConflict {
            operation: "controller batch",
            detail: format!("{} holds no arena slot", organism.id),
        })?;
        Ok(FlatControllerJob {
            genome: organism.genome,
            slot: arena_slot,
            controller: genome.controller(&rules.catalog, &rules.limits)?,
        })
    };

    let mut queries = [SensorQuery {
        position: [0.0, 0.0],
        scale: 1.0,
    }; FLAT_SAMPLE_BATCH];
    let mut batch_meta: [(Option<&'p Genome>, Option<ArenaSlot>, u32); FLAT_SAMPLE_BATCH] =
        [(None, None, NOT_DUE); FLAT_SAMPLE_BATCH];
    let mut chunk_jobs: [Option<FlatControllerJob<'p>>; FLAT_SAMPLE_BATCH] =
        std::array::from_fn(|_| None);

    for (chunk_idx, idx_chunk) in row_indices.chunks(FLAT_SAMPLE_BATCH).enumerate() {
        let count = idx_chunk.len();
        let local_base = chunk_idx * FLAT_SAMPLE_BATCH;
        let mut chunk_due = 0_usize;
        for i in 0..count {
            let row_idx = idx_chunk[i] as usize;
            let organism = &rows[row_idx];
            let phys_slot = ordered_slots[row_idx] as usize;
            if population.id_at_slot(phys_slot) != Some(organism.id) {
                return Err(CoreError::StateConflict {
                    operation: "read genome",
                    detail: format!("{} slot mismatch", organism.id),
                });
            }
            let (genome, slot) = population.genome_and_arena_at_slot(phys_slot);
            let genome = genome.ok_or_else(|| CoreError::StateConflict {
                operation: "read genome",
                detail: format!("{} has no genome", organism.id),
            })?;
            let due_slot = if due(rules, organism, to) {
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "chunk_due < FLAT_SAMPLE_BATCH (512) < u32::MAX"
                )]
                let d = chunk_due as u32;
                chunk_due += 1;
                d
            } else {
                NOT_DUE
            };
            batch_meta[i] = (Some(genome), slot, due_slot);
            queries[i] = SensorQuery {
                position: organism.position,
                scale: genome.traits().sensing_scale(),
            };
        }
        let chunk_in_f32 = in_f32
            .get_mut(local_due * input_width..(local_due + chunk_due) * input_width)
            .ok_or_else(|| CoreError::StateConflict {
                operation: "controller batch",
                detail: "flat input buffer shorter than due count".to_owned(),
            })?;
        let chunk_in_fin = in_finite
            .get_mut(local_due..local_due + chunk_due)
            .ok_or_else(|| CoreError::StateConflict {
                operation: "controller batch",
                detail: "flat finite flag buffer shorter than due count".to_owned(),
            })?;
        let chunk_dec = decisions_out
            .get_mut(local_base..local_base + count)
            .ok_or_else(|| CoreError::StateConflict {
                operation: "controller batch",
                detail: "decision buffer shorter than organism chunk".to_owned(),
            })?;
        if dual_carrier {
            view.sample_dual_batch_each(&queries[..count], |i, raw| {
                let (Some(genome), slot, due_slot) = batch_meta[i] else {
                    return Ok(());
                };
                let row_idx = idx_chunk[i] as usize;
                let organism = &rows[row_idx];
                let dec = &mut chunk_dec[i];
                if due_slot != NOT_DUE {
                    let d = due_slot as usize;
                    let dst = &mut chunk_in_f32[d * input_width..(d + 1) * input_width];
                    let base_arr: &mut [f32; SENSOR_WIDTH] = (&mut dst[..SENSOR_WIDTH])
                        .try_into()
                        .map_err(|_| CoreError::StateConflict {
                            operation: "controller batch",
                            detail: "flat input slice narrower than SENSOR_WIDTH".to_owned(),
                        })?;
                    let mut trail_f32 = [0.0_f32; 4];
                    encode_dual_for_controller_f32_into(
                        rules,
                        organism,
                        raw,
                        from,
                        base_arr,
                        &mut trail_f32,
                        &mut dec.gradients,
                    )?;
                    finish_due_row(
                        dst,
                        &mut chunk_in_fin[d],
                        row_idx,
                        organism,
                        Some(trail_f32),
                    );
                    chunk_jobs[d] = Some(build_job(organism, genome, slot)?);
                    // Pass 2 overwrites `dec.held` for every evaluated organism.
                    dec.evaluated = true;
                } else {
                    encode_dual_gradients_only_into(
                        rules,
                        organism,
                        raw,
                        from,
                        &mut dec.gradients,
                    )?;
                    dec.held.copy_active_from(&organism.held);
                    dec.evaluated = false;
                    decode_held_into(rules, organism, &organism.held, &mut dec.actions)?;
                }
                Ok(())
            })?;
        } else {
            view.sample_batch_each(&queries[..count], |i, raw| {
                let (Some(genome), slot, due_slot) = batch_meta[i] else {
                    return Ok(());
                };
                let row_idx = idx_chunk[i] as usize;
                let organism = &rows[row_idx];
                let dec = &mut chunk_dec[i];
                if due_slot != NOT_DUE {
                    let d = due_slot as usize;
                    let dst = &mut chunk_in_f32[d * input_width..(d + 1) * input_width];
                    let base_arr: &mut [f32; SENSOR_WIDTH] = (&mut dst[..SENSOR_WIDTH])
                        .try_into()
                        .map_err(|_| CoreError::StateConflict {
                            operation: "controller batch",
                            detail: "flat input slice narrower than SENSOR_WIDTH".to_owned(),
                        })?;
                    encode_for_controller_f32_into(
                        rules,
                        organism,
                        raw,
                        from,
                        base_arr,
                        &mut dec.gradients,
                    )?;
                    finish_due_row(dst, &mut chunk_in_fin[d], row_idx, organism, None);
                    chunk_jobs[d] = Some(build_job(organism, genome, slot)?);
                    // Pass 2 overwrites `dec.held` for every evaluated organism.
                    dec.evaluated = true;
                } else {
                    encode_gradients_only_into(rules, organism, raw, from, &mut dec.gradients)?;
                    dec.held.copy_active_from(&organism.held);
                    dec.evaluated = false;
                    decode_held_into(rules, organism, &organism.held, &mut dec.actions)?;
                }
                Ok(())
            })?;
        }
        for job in chunk_jobs[..chunk_due].iter_mut() {
            if let Some(job) = job.take() {
                jobs_out.push(job);
            }
        }
        local_due += chunk_due;
    }
    Ok(())
}

/// The zero-copy flat-F32 batched path for backends with
/// [`ControllerBackend::supports_flat_f32`]: eliminates intermediate `[f64; 104]`
/// / `[f64; 128]` staging, `Vec<ControllerJob>`, the second `population.ascending`
/// walk, and the `by_row` unpack copy, using dynamic work-stealing tiles across
/// `lanes` to eliminate P-core/E-core tail wait. On large multi-worker worlds
/// (`rows >= 8,192` and `cells >= 32,768` with CPU pyramid sampling), partitions
/// Pass 1 and Pass 2 into 8×8 spatial macro-tiles so each core streams one
/// L2-resident macro-tile and 2-row L1D strips without duplicating `decisions`.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "phases 3 and 4 flat-F32 fast path across spatial/linear Pass 1, evaluate_flat_f32, and Pass 2"
)]
fn decide_batched_flat_f32(
    rules: &Rules,
    population: &Population,
    view: &PyramidView<'_>,
    sampled: Option<Sampled<'_>>,
    rows: &[Organism],
    crowding_counts: &[u32],
    [from, to]: [Tick; 2],
    decisions: &mut Vec<Decision>,
    spatial_rows: &mut Vec<u32>,
    row_to_spatial: &mut Vec<u32>,
    spatial_tile_offsets: &mut [usize; SPATIAL_TILES + 1],
    lanes: &mut [DecisionLane],
    device: &mut dyn ControllerBackend,
    flat_inputs_f32: &mut Vec<f32>,
    flat_inputs_finite: &mut Vec<u8>,
    flat_outputs: &mut FlatOutputs,
    overlap: Option<&mut OverlapPhase56<'_>>,
) -> Result<Batched, CoreError> {
    row_to_spatial.clear();
    *spatial_tile_offsets = [0; SPATIAL_TILES + 1];
    for lane in lanes.iter_mut() {
        lane.due_offset = 0;
        lane.due_count = 0;
    }
    if rows.is_empty() {
        decisions.clear();
        flat_inputs_f32.clear();
        flat_inputs_finite.clear();
        flat_outputs.clear();
        return Ok(Batched::Done {
            batched: 0,
            deferred: 0,
            device_nanoseconds: 0,
        });
    }
    reserve_for(
        decisions,
        rows.len().saturating_sub(decisions.len()),
        "decisions",
    )?;
    set_len_reuse(decisions, rows.len(), Decision::empty());
    let sensors = rules.v2.sensors();
    let actions = rules.v2.actions();
    let input_width = sensors.input_width();
    let raw_output_width = actions.raw_output_width();

    let worker_budget = lanes.len().max(1);
    let ordered_slots = population.ordered_slots();
    let try_spatial = sampled.is_none()
        && worker_budget > 1
        && rows.len() >= SPATIAL_PASS1_MIN_ROWS
        && rules.grid.cell_count() >= SPATIAL_PASS1_MIN_CELLS
        && ordered_slots.len() == rows.len()
        && spatial_rows.capacity() >= rows.len()
        && row_to_spatial.capacity() >= rows.len()
        && u32::try_from(rows.len()).is_ok();

    if try_spatial {
        // Parallel Pass A: classify each row into an 8×8 spatial macro-tile
        // in one read of `rows`, storing the tile ID temporarily in
        // `row_to_spatial` so Pass B scatters purely inside 800 KiB of L2.
        let width = rules.grid.world_width();
        let height = rules.grid.world_height();
        let inv_w = (SPATIAL_NX as f64) / width;
        let inv_h = (SPATIAL_NY as f64) / height;
        set_len_reuse(row_to_spatial, rows.len(), 0);
        let bin_chunks = worker_budget.min(rows.len().div_ceil(1_024)).max(1);
        let bin_len = rows.len().div_ceil(bin_chunks);
        struct BinTask<'a> {
            rows: &'a [Organism],
            tile_ids: &'a mut [u32],
            counts: [usize; SPATIAL_TILES],
            due: [usize; SPATIAL_TILES],
            ok: bool,
        }
        let mut bin_tasks: Vec<BinTask<'_>> = rows
            .chunks(bin_len)
            .zip(row_to_spatial.chunks_mut(bin_len))
            .map(|(r, t)| BinTask {
                rows: r,
                tile_ids: t,
                counts: [0; SPATIAL_TILES],
                due: [0; SPATIAL_TILES],
                ok: true,
            })
            .collect();
        flife_core::pool::parallel_tasks(
            worker_budget,
            "spatial pass1 bin",
            &mut bin_tasks,
            |_, task| {
                for (organism, tile_out) in task.rows.iter().zip(task.tile_ids.iter_mut()) {
                    let [x, y] = organism.position;
                    if !x.is_finite()
                        || !y.is_finite()
                        || x < 0.0
                        || x >= width
                        || y < 0.0
                        || y >= height
                    {
                        task.ok = false;
                        return;
                    }
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "0 <= x < width and 0 <= y < height checked above, clamped to 0..8"
                    )]
                    let tx = ((x * inv_w) as usize).min(SPATIAL_NX - 1);
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "0 <= x < width and 0 <= y < height checked above, clamped to 0..8"
                    )]
                    let ty = ((y * inv_h) as usize).min(SPATIAL_NY - 1);
                    let tile = ty * SPATIAL_NX + tx;
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "tile < SPATIAL_TILES (64) <= u32::MAX"
                    )]
                    {
                        *tile_out = tile as u32;
                    }
                    task.counts[tile] += 1;
                    task.due[tile] += usize::from(due(rules, organism, to));
                }
            },
        )?;
        let spatial_ok = bin_tasks.iter().all(|task| task.ok);
        if spatial_ok {
            let mut tile_counts = [0_usize; SPATIAL_TILES];
            let mut tile_due = [0_usize; SPATIAL_TILES];
            for task in &bin_tasks {
                for t in 0..SPATIAL_TILES {
                    tile_counts[t] += task.counts[t];
                    tile_due[t] += task.due[t];
                }
            }
            let mut tile_offsets = [0_usize; SPATIAL_TILES + 1];
            let mut total_due = 0_usize;
            for t in 0..SPATIAL_TILES {
                tile_offsets[t + 1] = tile_offsets[t] + tile_counts[t];
                total_due = total_due
                    .checked_add(tile_due[t])
                    .ok_or(CoreError::Overflow {
                        quantity: "due controller jobs",
                    })?;
            }
            *spatial_tile_offsets = tile_offsets;
            set_len_reuse(spatial_rows, rows.len(), 0);
            let mut cursor = [0_usize; SPATIAL_TILES];
            cursor.copy_from_slice(&tile_offsets[..SPATIAL_TILES]);
            for (row_idx, slot) in row_to_spatial.iter_mut().enumerate() {
                let tile = *slot as usize;
                let pos = cursor[tile];
                cursor[tile] = pos + 1;
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "rows.len() <= u32::MAX checked in try_spatial"
                )]
                {
                    spatial_rows[pos] = row_idx as u32;
                    *slot = pos as u32;
                }
            }

            let total_input_elems =
                total_due
                    .checked_mul(input_width)
                    .ok_or(CoreError::Overflow {
                        quantity: "flat controller inputs",
                    })?;
            reserve_for(
                flat_inputs_f32,
                total_input_elems.saturating_sub(flat_inputs_f32.len()),
                "flat controller inputs",
            )?;
            set_len_reuse(flat_inputs_f32, total_input_elems, 0.0_f32);
            reserve_for(
                flat_inputs_finite,
                total_due.saturating_sub(flat_inputs_finite.len()),
                "flat controller input flags",
            )?;
            set_len_reuse(flat_inputs_finite, total_due, 0_u8);

            // Subdivide heavily clustered spatial tiles so all workers stay
            // load-balanced even if every organism congregates in one corner.
            let max_sub_tile = rows
                .len()
                .div_ceil(worker_budget.saturating_mul(4))
                .max(MINIMUM_ROWS_PER_LANE);
            struct SpatialSubSpec {
                start: usize,
                len: usize,
                due_offset: usize,
                due_count: usize,
            }
            let mut sub_specs: Vec<SpatialSubSpec> = Vec::with_capacity(SPATIAL_TILES);
            let mut due_cursor = 0_usize;
            for t in 0..SPATIAL_TILES {
                let count = tile_counts[t];
                if count == 0 {
                    continue;
                }
                let base = tile_offsets[t];
                if count <= max_sub_tile {
                    sub_specs.push(SpatialSubSpec {
                        start: base,
                        len: count,
                        due_offset: due_cursor,
                        due_count: tile_due[t],
                    });
                    due_cursor += tile_due[t];
                } else {
                    let mut off = 0_usize;
                    while off < count {
                        let sub_len = (count - off).min(max_sub_tile);
                        let sub_due = spatial_rows[base + off..base + off + sub_len]
                            .iter()
                            .filter(|&&r| due(rules, &rows[r as usize], to))
                            .count();
                        sub_specs.push(SpatialSubSpec {
                            start: base + off,
                            len: sub_len,
                            due_offset: due_cursor,
                            due_count: sub_due,
                        });
                        due_cursor += sub_due;
                        off += sub_len;
                    }
                }
            }

            struct Pass1SpatialTile<'a, 'p> {
                row_indices: &'a [u32],
                decisions: &'a mut [Decision],
                in_f32: &'a mut [f32],
                in_fin: &'a mut [u8],
                jobs: Vec<FlatControllerJob<'p>>,
            }
            let mut tiles: Vec<Pass1SpatialTile<'_, '_>> = Vec::with_capacity(sub_specs.len());
            let mut rem_dec = decisions.as_mut_slice();
            let mut rem_f32 = flat_inputs_f32.as_mut_slice();
            let mut rem_fin = flat_inputs_finite.as_mut_slice();
            for spec in &sub_specs {
                let (my_dec, next_dec) = rem_dec.split_at_mut(spec.len);
                rem_dec = next_dec;
                let (my_f32, next_f32) = rem_f32.split_at_mut(spec.due_count * input_width);
                rem_f32 = next_f32;
                let (my_fin, next_fin) = rem_fin.split_at_mut(spec.due_count);
                rem_fin = next_fin;
                let mut jobs = Vec::new();
                reserve_for(&mut jobs, spec.due_count, "controller jobs")?;
                tiles.push(Pass1SpatialTile {
                    row_indices: &spatial_rows[spec.start..spec.start + spec.len],
                    decisions: my_dec,
                    in_f32: my_f32,
                    in_fin: my_fin,
                    jobs,
                });
            }
            flife_core::pool::try_parallel_tasks(
                worker_budget,
                "decide spatial",
                &mut tiles,
                |_, tile| {
                    pass1_flat_f32_spatial(
                        rules,
                        population,
                        ordered_slots,
                        view,
                        rows,
                        crowding_counts,
                        [from, to],
                        tile.row_indices,
                        tile.decisions,
                        tile.in_f32,
                        tile.in_fin,
                        &mut tile.jobs,
                    )
                },
            )?;
            let mut flat_jobs = Vec::new();
            reserve_for(&mut flat_jobs, total_due, "controller jobs")?;
            for mut tile in tiles {
                flat_jobs.append(&mut tile.jobs);
            }

            let mut device_nanoseconds = 0_u64;
            if flat_jobs.is_empty() {
                flat_outputs.clear();
            } else {
                let eval_result = if device.prefers_cpu_overlap()
                    && worker_budget >= 2
                    && let Some(ov) = overlap
                {
                    let flat_jobs_ref: &[FlatControllerJob<'_>] = &flat_jobs;
                    let flat_in_f32_ref: &[f32] = flat_inputs_f32;
                    let flat_in_fin_ref: &[u8] = flat_inputs_finite;
                    let (eval_res, ()) = flife_core::pool::overlap_off_cpu(
                        worker_budget,
                        "overlap controllers and phase 5-6",
                        || {
                            device.evaluate_flat_f32(
                                flat_jobs_ref,
                                input_width,
                                flat_in_f32_ref,
                                flat_in_fin_ref,
                                flat_outputs,
                            )
                        },
                        |cpu_workers| {
                            run_overlapped_phase56(
                                rules,
                                population,
                                rows,
                                spatial_rows,
                                row_to_spatial,
                                spatial_tile_offsets,
                                lanes,
                                cpu_workers,
                                ov,
                            );
                        },
                    )?;
                    eval_res
                } else {
                    device.evaluate_flat_f32(
                        &flat_jobs,
                        input_width,
                        flat_inputs_f32,
                        flat_inputs_finite,
                        flat_outputs,
                    )
                };
                if let Err(error) = eval_result {
                    flat_outputs.clear();
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
                struct Pass2SpatialTile<'a> {
                    row_indices: &'a [u32],
                    decisions: &'a mut [Decision],
                    due_offset: usize,
                    due_count: usize,
                }
                let mut pass2_tiles: Vec<Pass2SpatialTile<'_>> =
                    Vec::with_capacity(sub_specs.len());
                let mut rem_dec = decisions.as_mut_slice();
                for spec in &sub_specs {
                    let (my_dec, next_dec) = rem_dec.split_at_mut(spec.len);
                    rem_dec = next_dec;
                    pass2_tiles.push(Pass2SpatialTile {
                        row_indices: &spatial_rows[spec.start..spec.start + spec.len],
                        decisions: my_dec,
                        due_offset: spec.due_offset,
                        due_count: spec.due_count,
                    });
                }
                flife_core::pool::try_parallel_tasks_with_lanes(
                    worker_budget,
                    "decode controllers spatial",
                    lanes,
                    &mut pass2_tiles,
                    |_, tile, lane| {
                        if tile.due_count == 0 {
                            return Ok(());
                        }
                        let mut job_idx = tile.due_offset;
                        for (&row_u32, decision) in
                            tile.row_indices.iter().zip(tile.decisions.iter_mut())
                        {
                            if !decision.evaluated {
                                continue;
                            }
                            let organism = &rows[row_u32 as usize];
                            match flat_out_ref.row(job_idx) {
                                Some(raw_slice) => {
                                    held_from_raw_slice_into(
                                        rules,
                                        organism,
                                        to,
                                        raw_slice,
                                        &mut decision.held,
                                    )?;
                                }
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
                                            detail: format!(
                                                "missing flat inputs for deferred job {job_idx}"
                                            ),
                                        })?;
                                    decision.held = evaluate_reference_held_from_f32(
                                        rules,
                                        job.controller,
                                        organism,
                                        in_row,
                                        to,
                                        &mut lane.controller,
                                    )?;
                                }
                            }
                            decode_held_into(
                                rules,
                                organism,
                                &decision.held,
                                &mut decision.actions,
                            )?;
                            job_idx += 1;
                        }
                        Ok(())
                    },
                )?;
            }
            let deferred = flat_outputs.valid.iter().filter(|&&v| v == 0).count();
            let count = |value: usize| u64::try_from(value).unwrap_or(u64::MAX);
            return Ok(Batched::Done {
                batched: count(flat_jobs.len().saturating_sub(deferred)),
                deferred: count(deferred),
                device_nanoseconds,
            });
        }
        row_to_spatial.clear();
    }

    let max_tiles = rows.len().div_ceil(MINIMUM_ROWS_PER_LANE).max(1);
    let tile_count = if worker_budget <= 1 || max_tiles <= 1 {
        1
    } else {
        worker_budget
            .saturating_mul(4)
            .min(64)
            .min(max_tiles)
            .max(worker_budget.min(max_tiles))
    };
    let tile_len = rows.len().div_ceil(tile_count);

    let mut tile_due_info: Vec<(usize, usize)> = Vec::with_capacity(tile_count);
    let mut total_due = 0_usize;
    for run in rows.chunks(tile_len) {
        let due_count = run
            .iter()
            .filter(|organism| due(rules, organism, to))
            .count();
        tile_due_info.push((total_due, due_count));
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
    reserve_for(
        flat_inputs_f32,
        total_input_elems.saturating_sub(flat_inputs_f32.len()),
        "flat controller inputs",
    )?;
    set_len_reuse(flat_inputs_f32, total_input_elems, 0.0_f32);
    reserve_for(
        flat_inputs_finite,
        total_due.saturating_sub(flat_inputs_finite.len()),
        "flat controller input flags",
    )?;
    set_len_reuse(flat_inputs_finite, total_due, 0_u8);

    let flat_jobs = if tile_due_info.len() == 1 {
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
            decisions.as_mut_slice(),
            flat_inputs_f32.as_mut_slice(),
            flat_inputs_finite.as_mut_slice(),
            &mut jobs,
        )?;
        jobs
    } else {
        struct Pass1Tile<'a, 'p> {
            start: usize,
            run: &'a [Organism],
            decisions: &'a mut [Decision],
            in_f32: &'a mut [f32],
            in_fin: &'a mut [u8],
            jobs: Vec<FlatControllerJob<'p>>,
        }
        let mut tiles: Vec<Pass1Tile<'_, '_>> = Vec::with_capacity(tile_due_info.len());
        let mut rem_f32 = flat_inputs_f32.as_mut_slice();
        let mut rem_fin = flat_inputs_finite.as_mut_slice();
        for (idx, ((run, dec_slice), &(_, due_count))) in rows
            .chunks(tile_len)
            .zip(decisions.chunks_mut(tile_len))
            .zip(tile_due_info.iter())
            .enumerate()
        {
            let (my_f32, next_f32) = rem_f32.split_at_mut(due_count * input_width);
            rem_f32 = next_f32;
            let (my_fin, next_fin) = rem_fin.split_at_mut(due_count);
            rem_fin = next_fin;
            let mut jobs = Vec::new();
            reserve_for(&mut jobs, due_count, "controller jobs")?;
            tiles.push(Pass1Tile {
                start: idx * tile_len,
                run,
                decisions: dec_slice,
                in_f32: my_f32,
                in_fin: my_fin,
                jobs,
            });
        }
        flife_core::pool::try_parallel_tasks(worker_budget, "decide", &mut tiles, |_, tile| {
            pass1_flat_f32(
                rules,
                population,
                view,
                sampled,
                crowding_counts,
                [from, to],
                tile.start,
                tile.run,
                tile.decisions,
                tile.in_f32,
                tile.in_fin,
                &mut tile.jobs,
            )
        })?;
        let mut merged = Vec::new();
        reserve_for(&mut merged, total_due, "controller jobs")?;
        for mut tile in tiles {
            merged.append(&mut tile.jobs);
        }
        merged
    };

    let mut device_nanoseconds = 0_u64;
    if flat_jobs.is_empty() {
        flat_outputs.clear();
    } else {
        let eval_result = if device.prefers_cpu_overlap()
            && worker_budget >= 2
            && let Some(ov) = overlap
        {
            let flat_jobs_ref: &[FlatControllerJob<'_>] = &flat_jobs;
            let flat_in_f32_ref: &[f32] = flat_inputs_f32;
            let flat_in_fin_ref: &[u8] = flat_inputs_finite;
            let (eval_res, ()) = flife_core::pool::overlap_off_cpu(
                worker_budget,
                "overlap controllers and phase 5",
                || {
                    device.evaluate_flat_f32(
                        flat_jobs_ref,
                        input_width,
                        flat_in_f32_ref,
                        flat_in_fin_ref,
                        flat_outputs,
                    )
                },
                |cpu_workers| {
                    run_overlapped_phase56(
                        rules,
                        population,
                        rows,
                        spatial_rows,
                        row_to_spatial,
                        spatial_tile_offsets,
                        lanes,
                        cpu_workers,
                        ov,
                    );
                },
            )?;
            eval_res
        } else {
            device.evaluate_flat_f32(
                &flat_jobs,
                input_width,
                flat_inputs_f32,
                flat_inputs_finite,
                flat_outputs,
            )
        };
        if let Err(error) = eval_result {
            flat_outputs.clear();
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
        struct Pass2Tile<'a> {
            run: &'a [Organism],
            decisions: &'a mut [Decision],
            due_offset: usize,
            due_count: usize,
        }
        let mut pass2_tiles: Vec<Pass2Tile<'_>> = rows
            .chunks(tile_len)
            .zip(decisions.chunks_mut(tile_len))
            .zip(tile_due_info.iter())
            .map(|((run, dec_slice), &(due_offset, due_count))| Pass2Tile {
                run,
                decisions: dec_slice,
                due_offset,
                due_count,
            })
            .collect();
        flife_core::pool::try_parallel_tasks_with_lanes(
            worker_budget,
            "decode controllers",
            lanes,
            &mut pass2_tiles,
            |_, tile, lane| {
                if tile.due_count == 0 {
                    return Ok(());
                }
                let mut job_idx = tile.due_offset;
                for (organism, decision) in tile.run.iter().zip(tile.decisions.iter_mut()) {
                    if !decision.evaluated {
                        continue;
                    }
                    match flat_out_ref.row(job_idx) {
                        Some(raw_slice) => {
                            held_from_raw_slice_into(
                                rules,
                                organism,
                                to,
                                raw_slice,
                                &mut decision.held,
                            )?;
                        }
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
                                    detail: format!(
                                        "missing flat inputs for deferred job {job_idx}"
                                    ),
                                })?;
                            decision.held = evaluate_reference_held_from_f32(
                                rules,
                                job.controller,
                                organism,
                                in_row,
                                to,
                                &mut lane.controller,
                            )?;
                        }
                    }
                    decode_held_into(rules, organism, &decision.held, &mut decision.actions)?;
                    job_idx += 1;
                }
                Ok(())
            },
        )?;
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
    decisions: &mut Vec<Decision>,
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
        lane.due_offset = 0;
        lane.due_count = 0;
    }
    in_runs(rows, lanes, &|start, run, lane| {
        const ZERO_OBS: Observation = Observation {
            inputs: [0.0; SENSOR_WIDTH],
            gradients: [[0.0; 2]; RESPONSE_TERMS],
        };
        let DecisionLane {
            observed,
            controller_inputs,
            ..
        } = lane;
        reserve_for(observed, run.len(), "observations")?;
        if needs_expanded {
            reserve_for(controller_inputs, run.len(), "controller inputs")?;
        }
        let mut live = run.first().map(|first| population.ascending_from(first.id));
        let build_full = |organism: &Organism,
                          obs: &Observation,
                          trail_inputs: Option<[f64; 4]>,
                          local_crowding: Option<u32>| {
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
            full
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
                    if needs_expanded {
                        controller_inputs.push(build_full(
                            organism,
                            &dual_obs.base,
                            Some(dual_obs.trail_inputs),
                            crowding_counts.get(start + index).copied(),
                        ));
                    }
                    observed.push(dual_obs.base);
                }
                return Ok(());
            }
            let mut queries = [SensorQuery {
                position: [0.0, 0.0],
                scale: 1.0,
            }; SAMPLE_BATCH];
            for (chunk_idx, chunk) in run.chunks(SAMPLE_BATCH).enumerate() {
                let count = chunk.len();
                let base = start + chunk_idx * SAMPLE_BATCH;
                for (query, organism) in queries[..count].iter_mut().zip(chunk) {
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
                let obs_base = observed.len();
                observed.resize(obs_base + count, ZERO_OBS);
                let obs_slice = &mut observed[obs_base..obs_base + count];
                if needs_expanded {
                    let in_base = controller_inputs.len();
                    controller_inputs.resize(in_base + count, [0.0_f64; MAX_SENSOR_WIDTH]);
                    let in_slice = &mut controller_inputs[in_base..in_base + count];
                    view.sample_dual_batch_each(&queries[..count], |i, raw| {
                        let organism = &chunk[i];
                        let dual_obs = encode_dual_for_controller(rules, organism, raw, from)?;
                        in_slice[i] = build_full(
                            organism,
                            &dual_obs.base,
                            Some(dual_obs.trail_inputs),
                            crowding_counts.get(base + i).copied(),
                        );
                        obs_slice[i] = dual_obs.base;
                        Ok(())
                    })?;
                } else {
                    view.sample_dual_batch_each(&queries[..count], |i, raw| {
                        let dual_obs = encode_dual_for_controller(rules, &chunk[i], raw, from)?;
                        obs_slice[i] = dual_obs.base;
                        Ok(())
                    })?;
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
                if needs_expanded {
                    controller_inputs.push(build_full(
                        organism,
                        &obs,
                        None,
                        crowding_counts.get(start + index).copied(),
                    ));
                }
                observed.push(obs);
            }
            return Ok(());
        }
        let mut queries = [SensorQuery {
            position: [0.0, 0.0],
            scale: 1.0,
        }; SAMPLE_BATCH];
        for (chunk_idx, chunk) in run.chunks(SAMPLE_BATCH).enumerate() {
            let count = chunk.len();
            let base = start + chunk_idx * SAMPLE_BATCH;
            for (query, organism) in queries[..count].iter_mut().zip(chunk) {
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
            let obs_base = observed.len();
            observed.resize(obs_base + count, ZERO_OBS);
            let obs_slice = &mut observed[obs_base..obs_base + count];
            if needs_expanded {
                let in_base = controller_inputs.len();
                controller_inputs.resize(in_base + count, [0.0_f64; MAX_SENSOR_WIDTH]);
                let in_slice = &mut controller_inputs[in_base..in_base + count];
                view.sample_batch_each(&queries[..count], |i, raw| {
                    let organism = &chunk[i];
                    let obs = encode_for_controller(rules, organism, raw, from)?;
                    in_slice[i] =
                        build_full(organism, &obs, None, crowding_counts.get(base + i).copied());
                    obs_slice[i] = obs;
                    Ok(())
                })?;
            } else {
                view.sample_batch_each(&queries[..count], |i, raw| {
                    obs_slice[i] = encode_for_controller(rules, &chunk[i], raw, from)?;
                    Ok(())
                })?;
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

    if rows.is_empty() {
        decisions.clear();
    } else {
        reserve_for(
            decisions,
            rows.len().saturating_sub(decisions.len()),
            "decisions",
        )?;
        set_len_reuse(decisions, rows.len(), Decision::empty());
    }
    let outputs_slice: &[Option<RawOutputs>] = outputs;
    let worker_budget = lanes.len();
    let used = worker_budget
        .min(rows.len().div_ceil(MINIMUM_ROWS_PER_LANE))
        .max(1);
    let run_len = if used == 1 {
        rows.len().max(1)
    } else {
        rows.len().div_ceil(used)
    };
    let mut legacy_tiles: Vec<(usize, &[Organism], &mut [Decision], &mut DecisionLane)> = rows
        .chunks(run_len)
        .zip(decisions.chunks_mut(run_len))
        .zip(lanes.iter_mut())
        .enumerate()
        .map(|(idx, ((run, dec_out), lane))| (idx * run_len, run, dec_out, lane))
        .collect();
    flife_core::pool::try_parallel_tasks(
        worker_budget,
        "decide",
        &mut legacy_tiles,
        |_, (start, run, dec_out, lane)| {
            let start = *start;
            let run = *run;
            let DecisionLane {
                controller,
                observed,
                controller_inputs,
                due_offset,
                ..
            } = *lane;
            let mut live = run.first().map(|first| population.ascending_from(first.id));
            let mut job_idx = *due_offset;
            for (index, ((organism, observation), slot)) in run
                .iter()
                .zip(observed.iter())
                .zip(dec_out.iter_mut())
                .enumerate()
            {
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
                *slot = finish(rules, organism, held, evaluated, observation)?;
            }
            Ok(())
        },
    )?;
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
    emission_paid: f64,
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
            emission_paid: organism.emission_paid,
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
        organism.emission_paid = self.emission_paid;
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
/// controller of `parameters` weights, and its granted `(amount, comp_idx)`
/// packet pairs referencing the deduplicated per-cell `compositions` table.
struct Reference<'a> {
    traits: &'a Traits,
    decision: &'a Decision,
    parameters: u32,
    packets: &'a [(f64, u32)],
    compositions: &'a [[f64; CHANNELS]],
    precomputed_uptake: Option<(UptakeSettlement, EnergyBudget)>,
}

impl Stages for Reference<'_> {
    #[inline]
    fn uptake(
        &mut self,
        rules: &Rules,
        _organism: &Working,
        budget: &mut EnergyBudget,
    ) -> Result<UptakeSettlement, CoreError> {
        if let Some((settlement, next_budget)) = self.precomputed_uptake {
            *budget = next_budget;
            return Ok(settlement);
        }
        let h0 = self.decision.held.recurrent_state_array()[0];
        if (!rules.v2.affinity_rotation_enabled() || h0 == 0.0)
            && rules.limits.affinity_norm() <= rules.chemistry.affinity_norm_limit()
        {
            return rules.chemistry.settle_indexed_slice(
                self.traits.affinity(),
                self.packets,
                self.compositions,
                budget,
            );
        }
        let a_eff = effective_affinity(
            self.traits,
            h0,
            &rules.v2,
            rules.chemistry.affinity_norm_limit(),
        )?;
        let affinity = if rules.limits.affinity_norm() <= rules.chemistry.affinity_norm_limit() {
            Affinity::trusted(a_eff)
        } else {
            Affinity::new(a_eff, &rules.chemistry)?
        };
        rules
            .chemistry
            .settle_indexed(&affinity, self.packets, self.compositions, budget)
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
                self.decision.evaluated,
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
        let trusted = rules.limits.motion_response_norm() <= rules.motion.response_norm_limit();
        let acceleration = match (rules.v2.motor(), self.decision.actions.motor) {
            (Some(constants), Some(motor)) => {
                let response = if trusted {
                    MotionResponse::trusted(*self.traits.motion_response())
                } else {
                    MotionResponse::new(*self.traits.motion_response(), &rules.motion)?
                };
                rules.motion.acceleration_v2(
                    ratio,
                    &response,
                    &self.decision.gradients,
                    organism.velocity,
                    budget.remaining(),
                    &constants,
                    MotorSteeringInputs::from(motor),
                )?
            }
            _ if trusted => rules.motion.acceleration_slice(
                ratio,
                self.traits.motion_response(),
                &self.decision.gradients,
            )?,
            _ => {
                let response = MotionResponse::new(*self.traits.motion_response(), &rules.motion)?;
                rules
                    .motion
                    .acceleration(ratio, &response, &self.decision.gradients)?
            }
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
                    self.decision.actions.emission.as_ref(),
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
            let ctx = rules.emission_context(organism, self.decision, position, velocity, from)?;
            let (settlement, plan) = rules.emission.emit_v2_dual_reusing_footprint(
                &rules.grid,
                rules.exchange,
                rules.step,
                rules.v2.emission(),
                recovery_alpha,
                organism.committed.id,
                self.decision.actions.emission.as_ref(),
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
                self.decision.actions.emission.as_ref(),
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
        let ctx = rules.emission_context(organism, self.decision, position, velocity, from)?;
        let (settlement, plan) = rules.emission.emit_v2_reusing_footprint(
            &rules.grid,
            rules.exchange,
            rules.step,
            rules.v2.emission(),
            recovery_alpha,
            organism.committed.id,
            self.decision.actions.emission.as_ref(),
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
        let attempt_probability = rules.effective_attempt_probability(organism, self.decision)?;
        rules.reproduction.propose_with_probability(
            &rules.rng,
            parent,
            tick,
            attempt_probability,
            self.decision.actions.child_fraction,
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

/// Combined EMT adhesion multiplier for `decision` (`V2-08` `2 σ(o_adh)`
/// composed multiplicatively with `V2-12` post-Phase-4
/// `m(h_1) = clamp(1 + h_1, 0, 1)`).
fn effective_emt_factor(rules: &Rules, decision: &Decision) -> f64 {
    let base = if rules.v2.emt_gate() {
        2.0 * decision.actions.emt_gate.unwrap_or(0.5)
    } else {
        1.0
    };
    if rules.v2.emt_h1_adhesion_enabled() {
        let h1 = decision.held.recurrent_state_array()[1];
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
    decisions: &[Decision],
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
                work.energy_gradients
                    .push(organism.decision(decisions).gradients[0]);
            }
            if needs_emt_factors {
                work.emt_factors
                    .push(effective_emt_factor(rules, organism.decision(decisions)));
            }
            if notch_delta {
                work.cell_states_h0
                    .push(organism.decision(decisions).held.recurrent_state_array()[0]);
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
                    out.grad[i] = organism.decision(decisions).gradients[0];
                }
                if needs_emt_factors {
                    out.emt[i] = effective_emt_factor(rules, organism.decision(decisions));
                }
                if notch_delta {
                    out.h0[i] = organism.decision(decisions).held.recurrent_state_array()[0];
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
    reason = "ordered phases 6 and 7 stage runner including optional V2-11 byproducts and V2-15 trail deposits"
)]
fn settle_stages(
    rules: &Rules,
    organism: &Working,
    decision: &Decision,
    traits: &Traits,
    stages: &mut impl Stages,
    from: Tick,
    ledger: &mut impl Ledger,
    emissions: &mut Deposits,
    trail_emissions: Option<&mut Deposits>,
    byproducts: Option<&mut Deposits>,
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
            let recurrent_h0 = decision.held.recurrent_state_array()[0];
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
    settled.emission_paid = emission.cost.paid;
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
            let parent_state = decision.held.recurrent_state_array();
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
                let emt_eff = effective_emt_factor(rules, decision);
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
                    decision.actions.budding,
                    decision.gradients[0],
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

#[cfg(test)]
mod tests {
    use super::compute_tile_compositions;
    use flife_field::state::CHANNELS;
    use flife_field::{BufferLimits, FieldGrid, FieldState};

    #[test]
    fn compute_tile_compositions_matches_scalar_across_contig_row_jumps_sparse_and_zero_energy() {
        let grid = FieldGrid::new(64, 64, 64.0, 64.0).expect("grid");
        let limits = BufferLimits::host();

        // Test single-carrier (32 active channels) and dual-carrier (24 active channels).
        for &(planes, expected_active) in &[(33_u32, 32_usize), (34_u32, 24_usize)] {
            let mut field = FieldState::zeroed_with_planes(grid, planes, &limits).expect("field");
            let total_cells = field.energy().len();
            let active = field.carrier0_moments_count();
            assert_eq!(active, expected_active, "planes={planes}");

            // Populate field with varied positive and zero energies and non-trivial moments.
            let mut lcg = 0x1234_5678_9ABC_DEF0_u64;
            let mut next_f32 = || {
                lcg = lcg
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                ((lcg >> 40) as f32) / 16_777_216.0
            };
            for c in 0..total_cells {
                let e = if c % 37 == 0 { 0.0 } else { 0.01 + next_f32() * 5.0 };
                field.as_mut_slice()[c] = e;
            }
            for p in 1..planes as usize {
                let base = p * total_cells;
                for c in 0..total_cells {
                    let sign = if (p + c) % 2 == 0 { 1.0 } else { -1.0 };
                    field.as_mut_slice()[base + c] = sign * (next_f32() * 0.25);
                }
            }

            // Build a mixed index sequence exercising:
            //  1) long contiguous rows (34 cells -> splits into 32 + 2 at row jump),
            //  2) short contiguous runs (7 cells) followed by row jump (+64),
            //  3) sparse non-contiguous cells with small gaps (2..5 apart),
            //  4) zero-energy cells (c % 37 == 0) interleaved inside contiguous and sparse runs,
            //  5) tail cells near total_cells - 1.
            let mut cells: Vec<u32> = Vec::new();
            // Dense 34-cell row without zeros (avoiding 37*k by starting at 38..72)
            for c in 38..72 {
                cells.push(c);
            }
            // Row jump to row 2 (130..164)
            for c in 130..164 {
                cells.push(c);
            }
            // 7-cell contiguous run then row jump (+64)
            for c in 200..207 {
                cells.push(c);
            }
            // Sparse gaps 2..5 apart (includes multiples of 37 -> zero-energy!)
            let mut cur = 290_u32;
            for step in 0..45_u32 {
                cells.push(cur);
                cur += 2 + (step % 4);
            }
            // Contiguous run containing a zero-energy cell (74 = 2*37)
            for c in 70..82 {
                cells.push(c);
            }
            // End-of-grid boundary cells
            for c in (total_cells as u32 - 5)..(total_cells as u32) {
                cells.push(c);
            }

            // Pre-poison output buffer to ensure active and inactive channels are written cleanly.
            let mut got = vec![[f64::NAN; CHANNELS]; cells.len()];
            compute_tile_compositions(&field, &cells, &mut got).expect("compute");

            let all = field.as_slice();
            for (idx, &cell_u32) in cells.iter().enumerate() {
                let c = cell_u32 as usize;
                let e = f64::from(all[c]);
                let mut expected = [0.0_f64; CHANNELS];
                if e > 0.0 {
                    for ch in 0..active {
                        expected[ch] = f64::from(all[(ch + 1) * total_cells + c]) / e;
                    }
                }
                for ch in 0..CHANNELS {
                    assert_eq!(
                        got[idx][ch].to_bits(),
                        expected[ch].to_bits(),
                        "planes={planes} idx={idx} cell={c} ch={ch}: got={} expected={}",
                        got[idx][ch],
                        expected[ch]
                    );
                }
            }
        }
    }
}
