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
//!
//! Source injection and transport are FL-004; uptake and the ledger are
//! FL-005; sensing and the pyramid are FL-007.

#![forbid(unsafe_code)]

pub mod footprint;
pub mod grid;
pub mod invariants;
pub mod layout;
pub mod state;

pub use footprint::{BufferLimits, FieldFootprint};
pub use grid::{ENERGY_PLANE, FIELD_PLANES, FieldGrid};
pub use invariants::{ConeTolerance, FieldSummary};
pub use layout::{Arrangement, Axis, DeclaredLayout, FieldLayout, Ownership};
pub use state::{Cell, FieldPair, FieldState, FieldView, InitialEnergy, relayout};
