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
//! - [`config`], [`schema`], [`validate`], [`canonical`]: the
//!   `frankenlife.config.v1` configuration, its table of units and bounds,
//!   the A.2 validator and the canonical serialization with its hash.
//! - [`descriptor`]: the model descriptor and its identity hash.
//! - [`model`]: fixed facts of `field-ecology-v1` and checked capacity
//!   arithmetic.

#![forbid(unsafe_code)]

pub mod canonical;
pub mod clock;
pub mod config;
pub mod descriptor;
pub mod error;
pub mod ids;
pub mod model;
pub mod registry;
pub mod schema;
pub mod units;
pub mod validate;

pub use canonical::Sha256Digest;
pub use clock::{Tick, TickSpan, TimeStep};
pub use config::RunConfig;
pub use descriptor::ModelDescriptor;
pub use error::{ConfigIssue, CoreError};
pub use validate::{ValidatedConfig, ValidationReport, validate};
