//! FrankenLife field representation (FL-003; plan §4.2, §9.3, §19.3,
//! INV-15).
//!
//! - [`grid`]: the periodic grid, cell indexing and torus wrapping.
//! - [`layout`]: the semantic layout descriptor. FrankenNumPy supplies and
//!   checks the strides; readers refuse a buffer whose declared layout
//!   differs from the one they need.
//! - [`state`]: the authoritative planar F32 state, its configured initial
//!   value, read-only views, semantic relayout and the current/next
//!   ping-pong pair.
//! - [`footprint`]: checked byte counts and per-buffer limits.
//! - [`invariants`]: INV-01 and INV-02 on a committed field.
//! - [`transport`]: `positive-five-point-v1` with `subcycle` stability and
//!   exponential decay (FL-004).
//! - [`sources`]: the normalized truncated minimum-image Gaussian,
//!   fixed-total-power deposits, fixed chemical directions and the smooth
//!   torus path (FL-004).
//! - [`step`]: phase 2 of the tick, transport and then sources, with its
//!   ledger terms (FL-004).
//! - [`numerics`]: the host-independent elementary functions of
//!   `flife_core::numerics`, used for decay factors and kernel weights.
//! - [`exchange`]: the `grid-cic-v1` exchange footprint and A.1's
//!   saturating request (FL-005).
//! - [`uptake`]: two-pass demand/grant resolution with exact
//!   `cell-quantum-largest-remainder-v1` reconciliation, and the
//!   proportional field debit (FL-005).
//! - [`pyramid`]: the `positive-box-v1` sensor density pyramid and its
//!   `log-linear-v1` sampling at a sensing scale: raw densities and their
//!   physical derivatives (FL-007).
//!
//! The energy ledger itself is `flife_core::ledger`. The sensor encoding
//! and action decoding are in `flife-neural`.

#![forbid(unsafe_code)]

pub mod exchange;
pub mod footprint;
pub mod grid;
pub mod invariants;
pub mod layout;
pub mod numerics;
pub mod pyramid;
pub mod sources;
pub mod state;
pub mod step;
pub mod transport;
pub mod uptake;

pub use exchange::{ExchangeStencil, saturating_request};
pub use footprint::{BufferLimits, FieldFootprint};
pub use grid::{ENERGY_PLANE, FIELD_PLANES, FieldGrid};
pub use invariants::{ConeTolerance, FieldSummary};
pub use layout::{Arrangement, Axis, DeclaredLayout, FieldLayout, Ownership};
pub use pyramid::{LevelGrid, PyramidView, RawSample, SensorPyramid};
pub use sources::{DepositReport, PathSegment, Source, SourceKernel, SourceSupport};
pub use state::{Cell, FieldPair, FieldState, FieldView, InitialEnergy, relayout};
pub use step::{FieldStepReport, advance_field};
pub use transport::{TransportPlan, TransportReport, TransportScratch};
pub use uptake::{
    CellDebit, Demand, GrantPacket, Grants, UptakeSummary, apply_grants, resolve_uptake,
};
