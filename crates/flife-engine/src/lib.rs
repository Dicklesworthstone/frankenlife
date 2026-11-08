//! FrankenLife organism-phase reference operators (FL-006, FL-011; plan
//! §5.3–§5.6, §7.4, §7.5, §8.1, §8.2).
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
//!
//! Each organism spends through a `flife_core::EnergyBudget` in the order of
//! §8.2: it absorbs its gain, pays toxicity and maintenance, then pays for
//! motion, then emission, then reproduction. Once its budget is exhausted it
//! pays nothing more and absorbs nothing more. Death, recycling and the
//! ordered tick are FL-012 and FL-013.

#![forbid(unsafe_code)]

pub mod chemistry;
pub mod costs;
pub mod emission;
pub mod motion;
pub mod reproduction;

pub use chemistry::{
    Affinity, ChemicalPacket, Chemistry, UptakeSettlement, composition, settle_efficiencies,
};
pub use costs::MaintenanceCosts;
pub use emission::{EmissionPolicy, EmissionSettlement, NEXT_TICK_VISIBILITY, POST_MOTION};
pub use motion::{Motion, MotionResponse, MotionSettlement, RESPONSE_TERMS, uptake_ratio};
pub use reproduction::{
    Birth, BirthContext, BirthIntent, BirthRequest, BirthResolution, Births, Capacity,
    CapacityShortfall, Identities, NEXT_TICK_ACTIVATION, Newborn, ParentState, PopulationQuota,
    RejectedBirth, Reproduction, ReproductionProposal, ReproductionRules, Split, Unviable,
    Viability, resolve_births,
};
