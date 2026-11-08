//! FrankenLife sensor/action ABI (FL-007; plan §6.1, §6.3, A.1, Appendix
//! C.10, C.11).
//!
//! - [`sensors`]: `energy-moment-gradient-104-v1` with
//!   `fixed-local-scales-v1` normalization. A raw pyramid sample becomes the
//!   controller's 104 inputs in their fixed order, plus the unsaturated
//!   physical gradients that motion follows.
//! - [`actions`]: the 36 raw outputs of a linear final layer, decoded into a
//!   reproduction attempt probability, a child energy fraction and an
//!   emission proposal. Sigmoid is applied exactly once, here.
//!
//! The density pyramid itself is `flife_field::pyramid`. The scalar
//! reference evaluator and the FrankenTorch population adapter are FL-008.

#![forbid(unsafe_code)]

pub mod actions;
pub mod sensors;

pub use actions::{ACTION_WIDTH, ActionDecoding, DecodedActions, Emission, decode};
pub use sensors::{Intrinsic, Observation, SENSOR_WIDTH, SensorScales, observe};
