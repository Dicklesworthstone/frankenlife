//! FrankenLife genomes (FL-009; plan §6.4, §6.5, §7.1, §7.2, §7.5, §9.2,
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
//!
//! Random words (FL-010's Philox semantic addressing), mutation (FL-010)
//! and packed per-topology arenas (FL-026) are not here.

#![forbid(unsafe_code)]

pub mod catalog;
pub mod genome;
pub mod init;
pub mod quota;
pub mod transition;

pub use catalog::{DENSE_104_36_CATALOG_V1, DENSE_104_36_MEMBERS, TopologyCatalog, TopologyId};
pub use genome::{Genome, SharedParameters, TraitLimits, Traits, Unsealed};
pub use init::{FAN_IN_SCALED_SYMMETRIC_V1, WeightAddress, fan_in_weight, initialize};
pub use quota::{GenomeQuota, ParameterBuffer, allocate_parameters, logical_bytes};
pub use transition::{Transition, map_parameters};
