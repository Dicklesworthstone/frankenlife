//! FrankenLife genomes (FL-009, FL-010; plan §6.4, §6.5, §7.1, §7.2, §7.5, §9.2,
//! A.2, A.3 items 2, 5 and 6, Appendix C.16).
//!
//! - [`catalog`]: `dense-104-36-catalog-v1`, the five dense topologies a
//!   controller may take, under the run's parameter cap.
//! - [`genome`]: immutable genomes. Each holds one organism's own F32
//!   controller parameters and its affinity, motion response and sensing
//!   scale, validated once when sealed.
//! - [`quota`]: the fixed logical genome quota (every genome charged at full
//!   size), the allocated quota, and fallible parameter allocation.
//! - [`init`]: the deterministic map of `fan-in-scaled-symmetric-v1` from
//!   raw words to founders' parameters.
//! - [`transition`]: `overlap-plus-zero-output-v1` and
//!   `prefix-and-identity-bridge-v1`, which map a parent's parameters onto
//!   another catalog member.
//! - [`founder`](mod@founder) (FL-010): founders' genomes from `fnp-philox-semantic-v1`.
//! - [`mutation`] (FL-010): bounded, semantically addressed mutation
//!   proposals with `uniform-other-catalog-member` and
//!   `reject-birth-no-charge`.
//!
//! - [`arena`] (FL-026): one bucket of fixed-size slots per catalog member.
//!   Each live genome holds a slot, with a generation that names it alone,
//!   by which backends keep its packed parameters.

#![forbid(unsafe_code)]

pub mod arena;
pub mod catalog;
mod draws;
pub mod founder;
pub mod genome;
pub mod init;
pub mod mutation;
pub mod quota;
pub mod transition;

pub use arena::GenomeArena;
pub use catalog::{DENSE_104_36_CATALOG_V1, DENSE_104_36_MEMBERS, TopologyCatalog, TopologyId};
pub use founder::{founder, founder_traits, initial_scale};
pub use genome::{
    AFFINITY_TERMS, Genome, MOTION_RESPONSE_TERMS, SharedParameters, TraitLimits, Traits, Unsealed,
};
pub use init::{FAN_IN_SCALED_SYMMETRIC_V1, WeightAddress, fan_in_draw, fan_in_weight, initialize};
pub use mutation::{
    BirthAttempt, CandidateRejection, MAXIMUM_CANDIDATES, MutationParameters, MutationRecord,
    Mutator, Proposal, REJECT_BIRTH_NO_CHARGE, RejectionReason, TopologyChange,
    UNIFORM_OTHER_CATALOG_MEMBER, perturb, perturb_weight, reflect_log_scale,
};
pub use quota::{
    ALLOCATED_GENOME_BYTES, GenomeQuota, LOGICAL_GENOME_BYTES, ParameterBuffer, ParameterValues,
    WeightsMut, allocate_parameters, allocate_values, logical_bytes, logical_bytes_in,
};
pub use transition::{Transition, map_parameters};

/// A genome's parameters in its weight dtype, as [`Genome::parameters`]
/// gives them.
pub use flife_neural::{WeightDtype, Weights};
