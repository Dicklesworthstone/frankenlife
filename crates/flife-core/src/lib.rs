//! FrankenLife core contracts (FL-002; plan §18.1, §18.3).
//!
//! - [`ids`]: stable identities and epoch identifiers. They are never
//!   storage slots, never reused, and overflow is refused.
//! - [`clock`]: integer ticks, the rational timestep, exact seconds-to-ticks
//!   conversion, and stable controller phases.
//! - [`units`]: the unit of every numeric schema quantity.
//! - [`error`]: distinct failure classes.
//! - [`registry`]: every named algorithm, policy and dtype a configuration
//!   may use, with its contract, owner and status.
//! - [`config`], [`schema`], [`validate`](mod@validate), [`canonical`]: the
//!   `frankenlife.config.v1` configuration, its table of units and bounds,
//!   the A.2 validator and the canonical serialization with its hash.
//! - [`checkpoint`]: the versioned, hashed checkpoint container and its
//!   bounded byte codecs (FL-016).
//! - [`descriptor`]: the model descriptor and its identity hash.
//! - [`model`]: fixed facts of `field-ecology-v1` and checked capacity
//!   arithmetic.
//! - [`ledger`]: the global energy ledger (INV-03), its residual budget,
//!   and per-organism energy budgets that never spend twice (INV-05).
//! - [`numerics`]: host-independent `exp`, `exp(−x)`, `1 − exp(−x)`, `tanh`,
//!   `sigmoid`, `log₂`, `ln` and turn-angle sine and cosine for the reference
//!   numerical profile.
//! - [`rng`]: `fnp-philox-semantic-v1`, semantically addressed Philox words
//!   and the uniform, choice and Box–Muller conventions (FL-010).
//! - [`binary16`]: the compact-F16 profile's weights, exact widening and
//!   the declared rounding from F64 (plan §10.3).

#![forbid(unsafe_code)]

pub mod binary16;
pub mod canonical;
pub mod checkpoint;
pub mod clock;
pub mod config;
pub mod descriptor;
pub mod error;
pub mod ids;
pub mod ledger;
pub mod memory;
pub mod model;
pub mod numerics;
pub mod registry;
pub mod rng;
pub mod schema;
pub mod units;
pub mod validate;

pub use canonical::{Sha256Digest, Sha256Stream};
pub use clock::{Tick, TickSpan, TimeStep};
pub use config::RunConfig;
pub use descriptor::ModelDescriptor;
pub use error::{ConfigIssue, CoreError};
pub use ledger::{
    CompensatedSum, EnergyBudget, EnergyLedger, Flow, FlowClass, Holdings, Ledger, LedgerLog,
    LedgerReport, LedgerState, Payment, ResidualBudget, TickClosure, TickLedger, Transfer,
};
pub use validate::{ValidatedConfig, ValidationReport, validate};
