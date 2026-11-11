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
//! Esc or ⌘W closes. 1 to 6 colour the organisms by lineage, energy,
//! generation, motion, topology or chemical affinity; R switches between the glow and raw
//! looks, D between points and density, and F shows or hides the field.
//! The pointer reports the world point under it. Scrolling zooms about the
//! pointer, a primary drag pans across the torus's seams, a click selects
//! the organism drawn under it by identity, and 0 shows the whole torus
//! again (FL-020). E arms the energy brush, X the removal brush, M the
//! material brush (the selected organism's affinity direction) and I the
//! introduction brush; with one armed, a click becomes a world-space
//! command the session applies at its next open boundary, and the Studio
//! reports the tick it applied at and the energy it moved ([`brush`],
//! FL-022).
//!
//! The display runs one committed state behind the world: each new epoch's
//! organisms move from where the previous epoch had them, matched by
//! identity, over about the interval at which epochs arrive ([`Pacer`]),
//! and the organisms that died fade out (FL-020, plan §13.5).
//!
//! The clocks are honest (plan §12.2): a [`ClockMeter`] reads the achieved
//! scientific ticks per wall second and the simulation-time to wall-time
//! ratio beside the frames presented, which the binary reports while
//! `[presentation] show_clock_telemetry` is set. The window draws them,
//! what the session is doing and what a click will do as text in its
//! top-left corner ([`overlay_text`], [`Studio::set_overlay`]), and under
//! them the selected organism's last inspection ([`inspector_lines`]),
//! which [`Studio::inspect_selected`] refreshes.

#![forbid(unsafe_code)]

pub mod brush;
mod clock;
mod controls;
mod host;
pub mod memory;
mod overlay;
mod pacing;
mod session;

pub use brush::{
    BRUSH_ENERGY, BRUSH_INTRODUCTION, BRUSH_RADIUS_POINTS, Brush, Stroke, Supplies, material_for,
    stroke,
};
pub use clock::{ClockMeter, ClockReading};
pub use controls::{
    CLICK_SLOP_POINTS, CameraMove, Controls, InputTally, Intent, KEY_D, KEY_DIGITS, KEY_E,
    KEY_ESCAPE, KEY_F, KEY_I, KEY_M, KEY_PERIOD, KEY_R, KEY_RIGHT, KEY_SPACE, KEY_W, KEY_X,
    KEY_ZERO, ZOOM_PER_LINE, ZOOM_PER_POINT, ZOOM_STEP_LIMIT, scroll_zoom,
};
pub use flife_backend_metal::ControllerChoice;
pub use flife_manim::{ColorMapping, Look, Motion, Particles, Style};
pub use host::{
    Frame, FrameTally, PICK_RADIUS_POINTS, STUDIO_SESSION, Studio, StudioClosed, StudioOptions,
};
pub use overlay::{Inspected, OverlayState, inspector_lines, legends, overlay_text};
pub use pacing::{ARRIVAL_SMOOTHING, MOTION_MAX, Pacer};
pub use session::{End, INTENT_CAPACITY, OUTCOME_POLL, SessionThread, Status};

/// The schema of the Studio's NDJSON records.
pub const STUDIO_SCHEMA: &str = "frankenlife.studio.v1";
