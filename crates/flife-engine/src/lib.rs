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
//!
//! Each organism spends through a `flife_core::EnergyBudget` in the order of
//! §8.2: it absorbs its gain, pays toxicity and maintenance, then pays for
//! motion, then emission, then reproduction. Once its budget is exhausted it
//! pays nothing more and absorbs nothing more, and it dies. [`world`] strings
//! these together in identity order.

#![forbid(unsafe_code)]

pub mod chemistry;
pub mod costs;
pub mod emission;
pub mod inspect;
pub mod intervention;
pub mod lifecycle;
pub mod motion;
pub mod population;
pub mod reproduction;
pub mod world;

pub use chemistry::{
    Affinity, ChemicalPacket, Chemistry, UptakeSettlement, composition, settle_efficiencies,
};
pub use costs::{MaintenanceBill, MaintenanceCosts, MaintenanceSettlement};
pub use emission::{EmissionPolicy, EmissionSettlement, NEXT_TICK_VISIBILITY, POST_MOTION};
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
pub use motion::{Motion, MotionResponse, MotionSettlement, RESPONSE_TERMS, uptake_ratio};
pub use population::{
    Arrangement, Collected, Entrant, HeldOutput, Organism, POPULATION_DIGEST_SCHEMA, Population,
};
pub use reproduction::{
    Birth, BirthContext, BirthIntent, BirthRequest, BirthResolution, Births, Capacity,
    CapacityShortfall, Identities, NEXT_TICK_ACTIVATION, Newborn, Offspring, ParentState,
    PopulationQuota, RejectedBirth, Reproduction, ReproductionProposal, ReproductionRules, Split,
    Unviable, Viability, resolve_births,
};
pub use world::{
    Advance, BirthEvent, CPU_REFERENCE, Candidate, ControllerReport, FieldReport, Footprint,
    IntroductionEvent, ModelVariant, NumericalFault, NumericalProfile, PAUSE_BEFORE_COMMIT,
    QUALIFIED_REFERENCE, STOP_AND_PRESERVE, SensorReport, Stepped, TickReport, UNIFORM_POSITIONS,
    UptakeReport, WORLD_CHECKPOINT_CHUNKS, WORLD_CHUNK_VERSION, WORLD_DIGEST_SCHEMA, World,
    ZERO_VELOCITY, founder_position,
};
