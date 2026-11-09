//! FrankenLife's FrankenManim bulk world adapter (FL-018, plan §13).
//!
//! The live ecology enters a FrankenManim scene as a few bulk primitives,
//! never as one scene object per organism (§13.2). FL-018 provides:
//!
//! - [`Viewport`]: the exact map between world units, FrankenManim's scene,
//!   drawable pixels and logical window points (§13.6), and the 3D camera
//!   whose frame it is.
//! - [`ParticleCloud`]: the `ParticleCloudView`, one bounded cloud lowered
//!   from a render epoch onto FrankenManim's 3D camera route.
//! - [`WorldView`]: both together, with the per-frame [`fmn_render::ThreeDJob`].
//!
//! Nothing here reads or writes the world: everything comes from an
//! immutable [`flife_render::Epoch`]. The field surface, selection overlay,
//! metric overlay and camera track are FL-019, FL-020 and FL-024 work.

#![forbid(unsafe_code)]

mod cloud;
mod field;
mod viewport;
mod world;

pub use cloud::{
    DOT_RADIUS, DOT_RADIUS_MIN_PIXELS, HALO_ALPHA, HALO_HALF_ENERGY, LINEAGE_PALETTE,
    ParticleCloud, dot_radius, halo_factor, lineage_color, lineage_slot,
};
pub use field::{FIELD_PALETTE, FieldLayer, SCALE_DECAY, display_value, palette};
pub use viewport::Viewport;
pub use world::{WorldView, hex_rgb};
