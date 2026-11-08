//! FrankenLife organism-phase reference operators (FL-006; plan §5.3, §5.4,
//! §5.6, §8.2).
//!
//! - [`chemistry`]: `bounded-affinity-v1` efficiency evaluated on each
//!   granted cell packet, useful gain, toxicity clipped to what the organism
//!   holds, and the settlement's ledger entries.
//! - [`costs`]: the mandatory maintenance of one tick, from canonical model
//!   work only.
//! - [`motion`]: the chemical response, drag integrated with a stable
//!   `expm1`, speed and acceleration limits, affordability scaling of the
//!   whole velocity, and the wrapped position step.
//!
//! Each organism spends through a `flife_core::EnergyBudget` in the order of
//! §8.2: it absorbs its gain, pays toxicity and maintenance, then pays for
//! motion. Once its budget is exhausted it pays nothing more and absorbs
//! nothing more. The ordered tick, emission, reproduction and death are
//! FL-011 to FL-013.

#![forbid(unsafe_code)]

pub mod chemistry;
pub mod costs;
pub mod motion;

pub use chemistry::{
    Affinity, ChemicalPacket, Chemistry, UptakeSettlement, composition, settle_efficiencies,
};
pub use costs::MaintenanceCosts;
pub use motion::{Motion, MotionResponse, MotionSettlement, RESPONSE_TERMS, uptake_ratio};
