//! FrankenLife's immutable compact render epochs (FL-017; plan §9.5,
//! §12.4, §13.2–§13.5, Appendix B.3, C.7, C.15).
//!
//! - [`epoch`]: an [`Epoch`] is exactly one committed world, projected
//!   into compact views in stable identity order, plus a bounded field
//!   texture and a metric summary. Every view declares its layout.
//! - [`feed`]: the owner's [`Publisher`] fills preallocated slots and
//!   publishes the latest one. Consumers read through typed per-viewer
//!   [`Lease`]s. A leased or displayable slot is never written. When every
//!   slot is busy the boundary is skipped and the last displayable epoch
//!   stays, so a slow consumer can cost freshness but never the world.
//! - [`pairing`]: matching organisms between epochs by identity, and
//!   whether a skipped-epoch [`Gap`] still allows minimum-image
//!   interpolation.
//!
//! The crate reads the world only through `&World`. Turning epochs into
//! pixels is FrankenManim's (FL-018/FL-019).

#![forbid(unsafe_code)]

pub mod epoch;
pub mod feed;
pub mod pairing;

pub use epoch::{
    AFFINITY_AXES, Bounds, Dtype, Epoch, FIELD_TEXELS_MAX, FieldChannel, FieldTexture, Layout,
    Metrics, Prior, Tally, View, affinity_projection,
};
pub use feed::{DISPLAY_PAIR, Feed, Lease, Publication, Publisher, RenderStats, Viewer};
pub use pairing::{
    Gap, Interpolation, Match, Pairs, ROUNDING_MARGIN, interpolate, pairs, wrapped_between,
};
