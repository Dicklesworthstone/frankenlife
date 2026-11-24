//! FrankenLife organism-phase reference operators, lifecycle and the
//! ordered tick (FL-006, FL-011–FL-013; plan §5.3–§5.6, §7.4–§7.7, §8.1–§8.4,
//! §9.1–§9.5).
//!
//! - [`chemistry`]: `bounded-affinity-v1` efficiency evaluated on each
//!   granted cell packet, useful gain, toxicity clipped to what the organism
//!   holds, and the settlement's ledger entries.
//! - [`costs`]: the mandatory maintenance of one tick, from canonical model
//!   work only.
//! - [`motion`]: the chemical response, drag integrated with a stable
//!   `expm1`, speed and acceleration limits, affordability scaling of the
//!   whole velocity, and the wrapped position step.
//! - [`contact`]: the contact models' soft-disk displacement of every
//!   survivor after phase 8, which moves positions only.
//! - [`emission`]: paid emission from the post-motion budget, staged as a
//!   next-tick deposit at the post-motion position (FL-011).
//! - [`reproduction`]: phase 7's cooldown, Bernoulli attempt and viability
//!   on the post-emission budget, and phase 8's birth transaction, with
//!   early fixed logical quotas and all-or-nothing rollback (FL-011).
//! - [`lifecycle`]: exhaustion rechecked after every budget stage,
//!   `dissipate-and-record` tiny-reservoir clearing, deaths collected once
//!   with their cause, and `genetic-affinity` recycling (FL-012).
//! - [`population`]: live organisms as a structure of arrays behind a
//!   stable-identity index, in a compacting or a free-list arrangement, with
//!   transactional commits of deaths and births (FL-012), and the whole
//!   tick's commit (FL-013).
//! - [`world`]: the ordered candidate tick (§8.1 phases 2–10) against the
//!   committed world and its atomic epoch publication, with
//!   `pause-before-commit` and `stop-and-preserve` (FL-013).
//! - [`intervention`]: the canonical scientific interventions a session
//!   seals at a tick boundary, applied with phase 2's external sources and
//!   recorded in the ledger (FL-014).
//! - [`ancestry`]: the live population's ancestry, folded from every
//!   published tick by a world that traces it, for the inspector's
//!   descendants and ancestry path (FL-021).
//! - [`measure`]: what one disk of the committed world holds, read between
//!   ticks for the Studio's temporary measurement region (plan §14.3).
//!
//! Each organism spends through a `flife_core::EnergyBudget` in the order of
//! §8.2: it absorbs its gain, pays toxicity and maintenance, then pays for
//! motion, then emission, then reproduction. Once its budget is exhausted it
//! pays nothing more and absorbs nothing more, and it dies. [`world`] strings
//! these together in identity order.

#![forbid(unsafe_code)]

pub mod ancestry;
pub mod chemistry;
pub mod contact;
pub mod costs;
pub mod emission;
pub mod inspect;
pub mod intervention;
pub mod lifecycle;
pub mod measure;
pub mod motion;
pub mod population;
pub mod reproduction;
pub mod world;

pub use ancestry::{
    ANCESTRY_PATH, Ancestor, AncestryHeld, Descendants, Followed, ROOM_PER_ORGANISM,
};
pub use chemistry::{
    Affinity, ChemicalPacket, Chemistry, UptakeSettlement, composition, corpse_affinity_direction,
    effective_affinity, settle_efficiencies, symplectic_waste_direction,
};
pub use contact::{
    ContactRules, ContactScratch, ContactWork, CrowdingWork, MAX_POLARITY_DTHETA_RAD,
    PcpSweepParams, Sweep1Context, Sweep1Output, pcp_lateral_wedge_weight, pcp_pair_torque_term,
    relax, relax_sweep1_with_context, rotate_unit_polarity, unit_affinity,
};
pub use costs::{MaintenanceBill, MaintenanceCosts, MaintenanceSettlement, senescence_multiplier};
pub use emission::{
    EmissionContext, EmissionOffsetRule, EmissionPlan, EmissionPolicy, EmissionSettlement,
    EmissionV2Rules, ExcitableMode, INITIAL_SECRETORY_READINESS, NEXT_TICK_VISIBILITY, OffsetMode,
    POST_MOTION, PumpDirection, SurplusFloorRule,
};
pub use inspect::{
    Activations, ControllerWeights, ExchangeCell, LocalChemistry, OrganismSnapshot, RECENT_STEPS,
    ReproductionOutcome, Sensed, Transactions,
};
pub use intervention::{
    AppliedIntervention, Intervention, MATERIAL_CHANNELS, MAXIMUM_INTRODUCTION,
};
pub use lifecycle::{
    BudgetStage, DISSIPATE_AND_RECORD, Death, DeathCause, DeathSettlement, Disposition, Fate,
    GENETIC_AFFINITY, Recycling, TinyEnergyPolicy, Vitality,
};
pub use measure::RegionMeasurement;
pub use motion::{
    HEADING_EPS, Motion, MotionResponse, MotionSettlement, RESPONSE_TERMS, uptake_ratio,
};
pub use population::{
    Arrangement, Ascending, Collected, DEFAULT_POLARITY, Entrant, HeldOutput, Organism,
    POPULATION_DIGEST_SCHEMA, Population,
};
pub use reproduction::{
    Birth, BirthContext, BirthIntent, BirthRequest, BirthResolution, Births, Capacity,
    CapacityPolicy, CapacityShortfall, Identities, NEXT_TICK_ACTIVATION, Newborn, Offspring,
    ParentState, PopulationQuota, REFUSE_EXCESS_BIRTHS, RefusedBirth, RejectedBirth, Reproduction,
    ReproductionProposal, ReproductionRules, Split, Unviable, Viability, admission_key,
    hippo_attempt_probability, hippo_crowding_sensor, hippo_gate, resolve_births,
    resolve_births_with, resolve_births_with_workers,
};
pub use world::{
    Advance, BirthEvent, BuddingRules, CPU_REFERENCE, Candidate, CarrierSpec, ControllerReport,
    CorpseDirection, CrowdingMode, CrowdingRules, FieldReport, Footprint, Graft, IntroductionEvent,
    LysisRules, ModelVariant, NumericalFault, NumericalProfile, PAUSE_BEFORE_COMMIT, PolarityRules,
    QUALIFIED_REFERENCE, STOP_AND_PRESERVE, SenescenceRules, SensorReport, SettleReport, Stepped,
    TickReport, UNIFORM_POSITIONS, UptakeReport, V2Rules, WORLD_CHECKPOINT_CHUNKS,
    WORLD_CHUNK_VERSION, WORLD_DIGEST_SCHEMA, WORLD_RECURRENT_HELD_VERSION, WORLD_V2_CHUNK_VERSION,
    WasteRules, World, ZERO_VELOCITY, founder_polarity, founder_position,
};
