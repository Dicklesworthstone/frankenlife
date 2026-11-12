//! The measuring tool (plan §14.3: "Allow selection regions, temporary
//! measurement regions, and camera-follow targets").
//!
//! G arms it, putting any brush away. A click then places a disk on the
//! world: exactly the disk the removal brush's click there would remove,
//! [`BRUSH_RADIUS_POINTS`] on screen about it. A drag draws one, as the
//! removal brush's drag does ([`crate::drawn`]). The session measures the
//! disk at its committed boundary ([`crate::Intent::Measure`],
//! `SessionHandle::measure_region`). It reports the organisms a removal of
//! that disk there would take, their energy and lineages, and the field
//! cells under it and their energy. Nothing is submitted, and the world
//! is unchanged.
//!
//! The disk is in world units, so it stays where it was placed while the
//! camera moves, and the presentation owner outlines it in the
//! measurement colour ([`flife_manim::RegionEffect::Measures`]). It is
//! temporary: G again, or arming a brush, puts it away, and nothing about
//! it is kept in the run. The Studio's timeline records its outline and the
//! overlay's text, as it records everything a frame shows.
//!
//! [`BRUSH_RADIUS_POINTS`]: crate::BRUSH_RADIUS_POINTS

use flife_manim::{Region, RegionEffect, RegionShape, Viewport};

use crate::Stroke;
use crate::brush;

/// A disk on the world: what the measuring tool measures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Disk {
    /// Its centre: a world position.
    pub center: [f64; 2],
    /// Its radius in world units.
    pub radius: f64,
}

impl Disk {
    /// The disk a removal takes; `None` for any other stroke.
    #[must_use]
    pub const fn of(stroke: Stroke) -> Option<Self> {
        match stroke {
            Stroke::Removal { center, radius } => Some(Self { center, radius }),
            _ => None,
        }
    }

    /// Its outline on the world, in the measurement colour.
    #[must_use]
    pub const fn region(self) -> Region {
        Region {
            shape: RegionShape::Disc {
                center: self.center,
                radius: self.radius,
            },
            effect: RegionEffect::Measures,
        }
    }
}

/// The disk a click at the logical window point `at` measures, through
/// `viewport` with a content area of `logical` points: the removal brush's
/// click disk there ([`brush::clicked`]). `None` in the letterbox margin
/// and while the content area is empty.
#[must_use]
pub fn clicked(viewport: &Viewport, logical: [f64; 2], at: [f64; 2]) -> Option<Disk> {
    brush::clicked(viewport, logical, at).and_then(Disk::of)
}

/// The disk a drag from the logical window point `center` out to `edge`
/// measures: the removal brush's drawn disk ([`brush::drawn`]). `None` with
/// `center` in the letterbox margin, while the content area is empty, and
/// for a drag of no length or a nonfinite one.
#[must_use]
pub fn drawn(
    viewport: &Viewport,
    logical: [f64; 2],
    center: [f64; 2],
    edge: [f64; 2],
) -> Option<Disk> {
    brush::drawn(viewport, logical, center, edge).and_then(Disk::of)
}

/// What the measuring tool shows while it is armed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MeasurePreview {
    /// No disk placed: what a click at the pointer would measure; `None`
    /// with the pointer off the world.
    Click(Option<Disk>),
    /// A disk being drawn: what a release with the pointer where it is
    /// would measure; `None` for a press off the world.
    Release(Option<Disk>),
    /// The disk placed.
    Placed(Disk),
}

impl MeasurePreview {
    /// The disk it outlines, if any.
    #[must_use]
    pub const fn disk(self) -> Option<Disk> {
        match self {
            Self::Click(disk) | Self::Release(disk) => disk,
            Self::Placed(disk) => Some(disk),
        }
    }
}

/// What the armed measuring tool shows through `viewport`, with a content
/// area of `logical` points and the pointer at the logical point
/// `pointer`: while a drag pressed at `drawing` draws a disk, what its
/// release there would measure; otherwise the disk `placed`, or with none
/// placed, what a click at the pointer would measure.
#[must_use]
pub fn preview(
    viewport: &Viewport,
    logical: Option<[f64; 2]>,
    pointer: Option<[f64; 2]>,
    drawing: Option<[f64; 2]>,
    placed: Option<Disk>,
) -> MeasurePreview {
    let at = logical.zip(pointer);
    match (drawing, placed) {
        (Some(center), _) => MeasurePreview::Release(
            at.and_then(|(logical, edge)| drawn(viewport, logical, center, edge)),
        ),
        (None, Some(disk)) => MeasurePreview::Placed(disk),
        (None, None) => {
            MeasurePreview::Click(at.and_then(|(logical, at)| clicked(viewport, logical, at)))
        }
    }
}
