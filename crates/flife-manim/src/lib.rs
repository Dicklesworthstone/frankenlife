//! FrankenLife's FrankenManim bulk world adapter (FL-018, FL-019, plan §13).
//!
//! The live ecology enters a FrankenManim scene as a few bulk primitives,
//! never as one scene object per organism (§13.2):
//!
//! - [`Viewport`]: the exact map between world units, FrankenManim's scene,
//!   drawable pixels and logical window points (§13.6), and the 3D camera
//!   whose frame it is; it pans and zooms across the torus's seams (FL-020).
//! - [`pick`]: the organism drawn under the pointer, by stable identity, and
//!   [`selection_ring`], the mark around the selected one.
//! - [`ParticleCloud`]: the `ParticleCloudView`, one bounded cloud lowered
//!   from a render epoch onto FrankenManim's 3D camera route.
//! - [`FieldLayer`]: the energy field under the organisms.
//! - [`DensityLayer`]: organism counts and mean colours per screen bin, for
//!   worlds denser than their pixels (§2.3, §13.3).
//! - [`Style`] and its parts: §13.4's colour mappings ([`ColorMapping`]),
//!   the glow and raw looks ([`Look`]), points or density ([`Particles`]).
//! - [`WorldView`]: all of them, with the per-frame
//!   [`fmn_render::ThreeDJob`], drawn at rest or part of the way from one
//!   committed epoch to the next ([`Motion`], FL-020).
//!
//! Nothing here reads or writes the world: everything comes from an
//! immutable [`flife_render::Epoch`]. The metric overlay and camera track
//! are FL-024 work.

#![forbid(unsafe_code)]

mod cloud;
mod density;
mod field;
mod mapping;
mod selection;
mod viewport;
mod world;

pub use cloud::{
    DOT_RADIUS, DOT_RADIUS_MAX_POINTS, DOT_RADIUS_MIN_POINTS, HALO_ALPHA, HALO_HALF_ENERGY,
    ParticleCloud, dot_radius, dot_radius_at_least, halo_factor,
};
pub use density::{
    DENSITY_BIN_POINTS, DENSITY_BINS_MAX, DensityLayer, density_bins, density_level,
};
pub use field::{FIELD_PALETTE, FieldLayer, SCALE_DECAY, display_value, palette, raw_value};
pub use mapping::{
    AFFINITY_CHROMA, AFFINITY_GAIN, AFFINITY_LIGHTNESS, ColorMapping, ENERGY_HALF, ENERGY_RAMP,
    GENERATION_RAMP, GENERATION_SCALE, LINEAGE_PALETTE, Look, MOTION_STILL, MOTION_WHEEL,
    Particles, Style, TOPOLOGY_PALETTE, Traits, affinity_rgb8, energy_level, generation_level,
    heading, lineage_color, lineage_slot, linear, mapped_color, mapped_rgb8, mix, ramp,
    speed_level, traits, wheel,
};
pub use selection::{
    PICK_RADIUS_PIXELS, SELECTION_RING_COLOR, SELECTION_RING_GAP_POINTS, SELECTION_RING_SEGMENTS,
    SELECTION_RING_WIDTH_POINTS, pick, selection_ring, selection_ring_around,
};
pub use viewport::{MAXIMUM_ZOOM, Viewport};
pub use world::{
    BLOOM_SIGMA_POINTS, BLOOM_WIDEST_REACH_PIXELS, JOB_BYTES_PER_DRAW, Motion,
    PREPARATION_BYTES_FIXED, PREPARATION_BYTES_PER_DOT, PREPARATION_BYTES_PER_TRIANGLE, WorldView,
    hex_rgb,
};
