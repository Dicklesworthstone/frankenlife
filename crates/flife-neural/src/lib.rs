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
//! - [`weights`]: the canonical weight dtypes, F32 and compact F16 (plan
//!   §10.3), and how the evaluator reads each as F32.
//!
//! The density pyramid itself is `flife_field::pyramid`. The FrankenTorch
//! population adapter is FL-025.

#![forbid(unsafe_code)]

pub mod actions;
pub mod arena;
pub mod backend;
pub mod controller;
pub mod sensors;
pub mod weights;

pub use actions::{
    ACTION_WIDTH, ActionDecoding, ActionLayout, BuddingSteering, DecodedActions, EMT_GATE_OUTPUTS,
    Emission, MAX_ACTION_WIDTH, MAX_CONTROLLER_OUTPUT_WIDTH, MotorSteering, SO2_BUDDING_OUTPUTS,
    SO2_MOTOR_OUTPUTS, WAKE_PUMP_OUTPUTS, decode, decode_slice, decode_with_layout,
    decode_with_layout_f32_into,
};
pub use arena::{ArenaSlot, F16_SLOT_ALIGNMENT_ELEMENTS, Placement, ShardGeometry};
pub use backend::{
    BackendBudget, ControllerBackend, ControllerJob, FlatControllerJob, FlatOutputs,
};
pub use controller::{
    ControllerScratch, DenseTopology, INDEPENDENT_DENSE_MLP_V1, INDEPENDENT_RECURRENT_MLP_V1,
    LayerShape, MAXIMUM_RECURRENT_STATE_WIDTH, RawOutputs, RawTrace, RecurrentTrace,
    ReferenceController, Trace, rotate_epigenetic_affinity, update_recurrent_state, widen,
    widen_slice,
};
pub use sensors::{
    DualObservation, Intrinsic, MAX_SENSOR_WIDTH, Observation, SENSOR_WIDTH, SensorLayout,
    SensorScales, observe, observe_dual, observe_dual_for_controller,
    observe_dual_for_controller_f32, observe_dual_for_controller_f32_into,
    observe_dual_gradients_only, observe_dual_gradients_only_into, observe_for_controller,
    observe_for_controller_f32, observe_for_controller_f32_into, observe_gradients_only,
    observe_gradients_only_into,
};
pub use weights::{Weight, WeightDtype, Weights};
