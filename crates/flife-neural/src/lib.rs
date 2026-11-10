//! FrankenLife sensor/action ABI and reference controller (FL-007, FL-008;
//! plan §6.1–§6.5, §10.1, A.1, Appendix C.10, C.11, C.16).
//!
//! - [`sensors`]: `energy-moment-gradient-104-v1` with
//!   `fixed-local-scales-v1` normalization. A raw pyramid sample becomes the
//!   controller's 104 inputs in their fixed order, plus the unsaturated
//!   physical gradients that motion follows.
//! - [`actions`]: the 36 raw outputs of a linear final layer, decoded into a
//!   reproduction attempt probability, a child energy fraction and an
//!   emission proposal. Sigmoid is applied exactly once, here.
//! - [`controller`]: the scalar reference evaluator of
//!   `independent-dense-mlp-v1`, the known-answer oracle between the two:
//!   F32 accumulation in a declared order, `tanh` hidden layers and a
//!   linear output layer, over each organism's own parameters.
//!
//! - [`backend`]: the port through which the engine hands a tick's due
//!   evaluations to an exact batch backend (FL-030's Metal kernel).
//! - [`arena`]: each live genome's arena slot, by which a backend keeps its
//!   packed parameters, and the checked 64-bit geometry of the shards it
//!   packs them into (FL-026).
//!
//! The density pyramid itself is `flife_field::pyramid`. The FrankenTorch
//! population adapter is FL-025.

#![forbid(unsafe_code)]

pub mod actions;
pub mod arena;
pub mod backend;
pub mod controller;
pub mod sensors;

pub use actions::{ACTION_WIDTH, ActionDecoding, DecodedActions, Emission, decode};
pub use arena::{ArenaSlot, Placement, ShardGeometry};
pub use backend::{ControllerBackend, ControllerJob};
pub use controller::{ControllerScratch, DenseTopology, LayerShape, ReferenceController, widen};
pub use sensors::{Intrinsic, Observation, SENSOR_WIDTH, SensorScales, observe};
