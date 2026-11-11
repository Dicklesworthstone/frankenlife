//! Brushes (FL-022, plan §14.3, §12.3): a click of an armed brush becomes a
//! canonical world-space command payload. The session applies it at a tick
//! boundary and reports the tick it applied at and the energy it moved.
//!
//! | Brush | Key | A click makes |
//! |---|---|---|
//! | energy | E | `InjectEnergy` of [`BRUSH_ENERGY`] declared energy into the field cell under the click |
//! | removal | X | `RemoveOrganisms` in a disk [`BRUSH_RADIUS_POINTS`] in radius on screen about the click, `recycle`d as deaths are |
//!
//! The payload is world-space: a cell index on the field grid, or a world
//! centre and a radius in world units. It does not depend on the window
//! once made, so it is what the session records and replays.

use flife_engine::{Disposition, Intervention};
use flife_manim::Viewport;

/// The declared energy one click of the energy brush adds to its cell:
/// 32 founders' endowments (`population.initial_energy = 2`), recorded as
/// `command-injection`.
pub const BRUSH_ENERGY: f64 = 64.0;

/// The removal brush's radius on screen, in logical points, whatever the
/// zoom.
pub const BRUSH_RADIUS_POINTS: f64 = 12.0;

/// A brush the presentation owner can arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Brush {
    /// Adds declared energy to the field cell under a click.
    Energy,
    /// Removes the organisms in a disk about a click.
    Removal,
}

impl Brush {
    /// Its name in the Studio's records.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Energy => "energy",
            Self::Removal => "removal",
        }
    }
}

/// One click of a brush, in world space: what the session is asked to
/// apply.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stroke {
    /// Declared energy into one field cell.
    Energy {
        /// The cell's `[x, y]` index on the field grid.
        cell: [u32; 2],
        /// The energy added.
        amount: f64,
    },
    /// The organisms within `radius` world units of `center` on the torus.
    Removal {
        /// A world position in `[0, L_x) × [0, L_y)`.
        center: [f64; 2],
        /// In world units.
        radius: f64,
    },
}

impl Stroke {
    /// The brush that made it.
    #[must_use]
    pub const fn brush(self) -> Brush {
        match self {
            Self::Energy { .. } => Brush::Energy,
            Self::Removal { .. } => Brush::Removal,
        }
    }

    /// The canonical payload: [`Intervention::InjectEnergy`], or
    /// [`Intervention::RemoveOrganisms`] with [`Disposition::Recycle`], so
    /// a removed organism's energy goes where a death's does.
    #[must_use]
    pub const fn intervention(self) -> Intervention {
        match self {
            Self::Energy { cell, amount } => Intervention::InjectEnergy { cell, amount },
            Self::Removal { center, radius } => Intervention::RemoveOrganisms {
                center,
                radius,
                disposition: Disposition::Recycle,
            },
        }
    }
}

/// The stroke a click of `brush` at the logical window point `at` makes,
/// through `viewport`, with a content area of `logical` points over a
/// field of `cells` cells per axis. `None` in the letterbox margin, while
/// the content area is empty, or for a field with no cells.
///
/// - Energy: the cell whose square holds the world point under the click,
///   `⌊x / (L / cells)⌋` on each axis.
/// - Removal: centred on that world point, [`BRUSH_RADIUS_POINTS`] on
///   screen at the drawable's density and the view's zoom, in world units,
///   and at most half the shorter world side.
#[must_use]
pub fn stroke(
    brush: Brush,
    viewport: &Viewport,
    logical: [f64; 2],
    cells: [u32; 2],
    at: [f64; 2],
) -> Option<Stroke> {
    let world = viewport.logical_to_world(at, logical)?;
    let extent = viewport.extent();
    match brush {
        Brush::Energy => {
            if cells.contains(&0) {
                return None;
            }
            let cell = [0, 1].map(|axis| {
                let width = extent[axis] / f64::from(cells[axis]);
                // The world point is in [0, L), so the index is in
                // [0, cells); rounding at the far edge stays on the grid.
                ((world[axis] / width).floor() as u32).min(cells[axis] - 1)
            });
            Some(Stroke::Energy {
                cell,
                amount: BRUSH_ENERGY,
            })
        }
        Brush::Removal => {
            let [width, height] = viewport.drawable().map(f64::from);
            let per_point = (width / logical[0]).min(height / logical[1]);
            let radius = (BRUSH_RADIUS_POINTS * per_point / viewport.scale())
                .min(extent[0].min(extent[1]) / 2.0);
            (radius.is_finite() && radius > 0.0).then_some(Stroke::Removal {
                center: world,
                radius,
            })
        }
    }
}
