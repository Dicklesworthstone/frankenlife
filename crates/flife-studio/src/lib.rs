//! FrankenLife's interactive Studio shell (FL-018, plan §13.6).
//!
//! The live world in a native window: FrankenManim's Metal 3D route draws
//! one particle cloud ([`flife_manim`]) straight into frankentorch's
//! `CAMetalLayer` drawable, and the window's real AppKit events arrive as
//! typed input. The process main thread owns the window, input, viewport
//! and presentation ([`Studio`]); the FL-014 session owner runs on its own
//! Asupersync thread ([`SessionThread`]) behind a bounded intent lane.
//!
//! Keys: Space runs or pauses, → or `.` steps one tick while paused, and
//! Esc or ⌘W closes. The pointer reports the world point under it.

#![forbid(unsafe_code)]

mod controls;
mod host;
mod session;

pub use controls::{
    Controls, InputTally, Intent, KEY_ESCAPE, KEY_PERIOD, KEY_RIGHT, KEY_SPACE, KEY_W,
};
pub use flife_backend_metal::ControllerChoice;
pub use host::{Frame, FrameTally, STUDIO_SESSION, Studio, StudioClosed, StudioOptions};
pub use session::{End, INTENT_CAPACITY, SessionThread, Status};

/// The schema of the Studio's NDJSON records.
pub const STUDIO_SCHEMA: &str = "frankenlife.studio.v1";
