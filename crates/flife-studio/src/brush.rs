//! Brushes (FL-022, plan §14.3, §12.3): a click of an armed brush becomes a
//! canonical world-space command payload. The session applies it at a tick
//! boundary and reports the tick it applied at and the energy it moved.
//!
//! | Brush | Key | A click makes |
//! |---|---|---|
//! | energy | E | `InjectEnergy` of [`BRUSH_ENERGY`] declared energy into the field cell under the click |
//! | removal | X | `RemoveOrganisms` in a disk [`BRUSH_RADIUS_POINTS`] in radius on screen about the click, `recycle`d as deaths are; a drag draws the disk instead ([`drawn`]) |
//! | material | M | `InjectMaterial` of [`BRUSH_ENERGY`] into the cell under the click, its composition the selected organism's affinity direction ([`material_for`]) |
//! | introduction | I | `IntroduceOrganisms`: [`BRUSH_INTRODUCTION`] founder-drawn organisms in the cell under the click, each with a founder's endowment |
//!
//! The payload is world-space: a cell index on the field grid, or a world
//! centre and a radius in world units, with the material's composition and
//! the introduction's endowment fixed when the click is made. It does not
//! depend on the window once made, so it is what the session records and
//! replays.
//!
//! While a brush is armed, its [`preview`] is the stroke a click at the
//! pointer would make there now, by the same mapping, with the world
//! rectangle of its cell: what the overlay says a click will do (plan
//! §14.3). While a removal's disk is being drawn, it is [`drawing`]'s:
//! the disk a release would apply. [`preview_region`] is the region it
//! outlines on the world: the cell, or the removal's disc, coloured by
//! whether the stroke adds energy or removes it.

use flife_engine::{Disposition, Intervention, MATERIAL_CHANNELS};
use flife_field::sources::unit_direction;
use flife_manim::{Region, RegionEffect, RegionShape, Viewport};

/// The declared energy one click of the energy or material brush adds to
/// its cell: 32 founders' endowments (`population.initial_energy = 2`),
/// recorded as `command-injection`.
pub const BRUSH_ENERGY: f64 = 64.0;

/// The removal brush's radius on screen, in logical points, whatever the
/// zoom.
pub const BRUSH_RADIUS_POINTS: f64 = 12.0;

/// The organisms one click of the introduction brush introduces.
pub const BRUSH_INTRODUCTION: u32 = 8;

/// A brush the presentation owner can arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Brush {
    /// Adds declared energy to the field cell under a click.
    Energy,
    /// Removes the organisms in a disk about a click.
    Removal,
    /// Adds declared energy of the selected organism's affinity direction
    /// to the field cell under a click.
    Material,
    /// Introduces founder-drawn organisms in the field cell under a click.
    Introduction,
}

impl Brush {
    /// Its name in the Studio's records.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Energy => "energy",
            Self::Removal => "removal",
            Self::Material => "material",
            Self::Introduction => "introduction",
        }
    }
}

/// One click of a brush, in world space: what the session is asked to
/// apply.
#[derive(Debug, Clone, Copy, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "a material's 32 components keep the stroke `Copy` and unboxed, as `Intervention` does"
)]
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
    /// Declared energy with a chemical composition into one field cell.
    Material {
        /// The cell's `[x, y]` index on the field grid.
        cell: [u32; 2],
        /// The energy added.
        amount: f64,
        /// The composition per unit energy, `‖direction‖₂ ≤ 1`.
        direction: [f64; MATERIAL_CHANNELS],
    },
    /// Founder-drawn organisms into one field cell.
    Introduction {
        /// The cell's `[x, y]` index on the field grid.
        cell: [u32; 2],
        /// How many.
        count: u32,
        /// Each one's endowment.
        energy: f64,
    },
}

impl Stroke {
    /// The field cell it applies to; `None` for a removal, which covers a
    /// disk.
    #[must_use]
    pub const fn cell(self) -> Option<[u32; 2]> {
        match self {
            Self::Energy { cell, .. }
            | Self::Material { cell, .. }
            | Self::Introduction { cell, .. } => Some(cell),
            Self::Removal { .. } => None,
        }
    }

    /// The brush that made it.
    #[must_use]
    pub const fn brush(self) -> Brush {
        match self {
            Self::Energy { .. } => Brush::Energy,
            Self::Removal { .. } => Brush::Removal,
            Self::Material { .. } => Brush::Material,
            Self::Introduction { .. } => Brush::Introduction,
        }
    }

    /// The canonical payload: [`Intervention::InjectEnergy`],
    /// [`Intervention::RemoveOrganisms`] with [`Disposition::Recycle`], so
    /// a removed organism's energy goes where a death's does,
    /// [`Intervention::InjectMaterial`] or
    /// [`Intervention::IntroduceOrganisms`].
    #[must_use]
    pub const fn intervention(self) -> Intervention {
        match self {
            Self::Energy { cell, amount } => Intervention::InjectEnergy { cell, amount },
            Self::Removal { center, radius } => Intervention::RemoveOrganisms {
                center,
                radius,
                disposition: Disposition::Recycle,
            },
            Self::Material {
                cell,
                amount,
                direction,
            } => Intervention::InjectMaterial {
                cell,
                amount,
                direction,
            },
            Self::Introduction {
                cell,
                count,
                energy,
            } => Intervention::IntroduceOrganisms {
                cell,
                count,
                energy,
            },
        }
    }
}

/// What the brushes draw on besides the click.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Supplies {
    /// The material brush's composition per unit energy: the selected
    /// organism's [`material_for`], or `None` while no organism is
    /// selected or its affinity is not yet known.
    pub material: Option<[f64; MATERIAL_CHANNELS]>,
    /// Each introduced organism's endowment: `population.initial_energy`.
    pub founder_energy: f64,
}

/// The material an organism of affinity `a` converts best: the unit
/// composition `a / ‖a‖₂`, shrunk as source directions are until
/// `‖·‖₂ ≤ 1` holds in floating point (`flife_field`'s `unit_direction`).
/// Under `bounded-affinity-v1` a packet of composition `z` converts with
/// efficiency `η_max tanh(k a·z − b)`, which over `‖z‖₂ ≤ 1` is largest
/// along `a`. `None` for a zero or nonfinite affinity, which has no
/// direction.
#[must_use]
pub fn material_for(affinity: &[f64; MATERIAL_CHANNELS]) -> Option<[f64; MATERIAL_CHANNELS]> {
    unit_direction(affinity).ok()
}

/// The stroke a click of `brush` at the logical window point `at` makes,
/// through `viewport`, with a content area of `logical` points over a
/// field of `cells` cells per axis, drawing on `supplies`. `None` in the
/// letterbox margin, while the content area is empty, for a field with no
/// cells, and for the material brush without a material.
///
/// - Energy, material and introduction: the cell whose square holds the
///   world point under the click, `⌊x / (L / cells)⌋` on each axis.
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
    supplies: &Supplies,
) -> Option<Stroke> {
    let world = viewport.logical_to_world(at, logical)?;
    let extent = viewport.extent();
    let cell = || {
        if cells.contains(&0) {
            return None;
        }
        Some([0, 1].map(|axis| {
            let width = extent[axis] / f64::from(cells[axis]);
            // The world point is in [0, L), so the index is in [0, cells);
            // rounding at the far edge stays on the grid.
            ((world[axis] / width).floor() as u32).min(cells[axis] - 1)
        }))
    };
    match brush {
        Brush::Energy => Some(Stroke::Energy {
            cell: cell()?,
            amount: BRUSH_ENERGY,
        }),
        Brush::Material => Some(Stroke::Material {
            cell: cell()?,
            amount: BRUSH_ENERGY,
            direction: supplies.material?,
        }),
        Brush::Introduction => Some(Stroke::Introduction {
            cell: cell()?,
            count: BRUSH_INTRODUCTION,
            energy: supplies.founder_energy,
        }),
        Brush::Removal => removal(viewport, logical, world, BRUSH_RADIUS_POINTS),
    }
}

/// The removal a click of the removal brush at the logical window point
/// `at` makes, through `viewport` with a content area of `logical` points:
/// exactly [`stroke`]'s for [`Brush::Removal`], whatever the field's cells
/// or the supplies. `None` in the letterbox margin and while the content
/// area is empty. The measuring tool's click measures this disk
/// ([`crate::measure::clicked`]).
#[must_use]
pub fn clicked(viewport: &Viewport, logical: [f64; 2], at: [f64; 2]) -> Option<Stroke> {
    removal(
        viewport,
        logical,
        viewport.logical_to_world(at, logical)?,
        BRUSH_RADIUS_POINTS,
    )
}

/// The removal a drag of the removal brush draws from the logical window
/// point `center` out to `edge`, through `viewport`, with a content area of
/// `logical` points (plan §2.2's "draw an intervention region"): centred
/// on the world point under `center`, its radius the drag's length on
/// screen at the drawable's density and the view's zoom, in world units,
/// and at most half the shorter world side, as a click's
/// [`BRUSH_RADIUS_POINTS`] is. `None` with `center` in the letterbox
/// margin, while the content area is empty, and for a drag of no length or
/// a nonfinite one; `edge` may be anywhere.
#[must_use]
pub fn drawn(
    viewport: &Viewport,
    logical: [f64; 2],
    center: [f64; 2],
    edge: [f64; 2],
) -> Option<Stroke> {
    let world = viewport.logical_to_world(center, logical)?;
    removal(
        viewport,
        logical,
        world,
        (edge[0] - center[0]).hypot(edge[1] - center[1]),
    )
}

/// The removal centred on the world point `center`, `points` logical
/// points in radius on screen.
fn removal(
    viewport: &Viewport,
    logical: [f64; 2],
    center: [f64; 2],
    points: f64,
) -> Option<Stroke> {
    let extent = viewport.extent();
    let [width, height] = viewport.drawable().map(f64::from);
    let per_point = (width / logical[0]).min(height / logical[1]);
    let radius = (points * per_point / viewport.scale()).min(extent[0].min(extent[1]) / 2.0);
    // `min` would take a NaN radius to the cap.
    (points.is_finite() && radius.is_finite() && radius > 0.0)
        .then_some(Stroke::Removal { center, radius })
}

/// What a click of an armed brush at the pointer would do, or a release
/// of the removal disk being drawn (plan §14.3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Preview {
    /// The stroke a click at the pointer makes, and for a brush that
    /// applies to a cell, that cell's world rectangle ([`cell_region`]).
    Stroke {
        /// Exactly what [`stroke`] makes for a click there.
        stroke: Stroke,
        /// `[[x₀, x₁], [y₀, y₁]]` in world units; `None` for a removal.
        region: Option<[[f64; 2]; 2]>,
    },
    /// The pointer is not over the world: it is outside the window or in
    /// the letterbox margin, or the content area has no size yet.
    OffWorld,
    /// The material brush over the world with no material: no organism is
    /// selected, or its affinity has not been read yet.
    NoMaterial,
    /// The removal brush's disk being drawn by a drag: what a release with
    /// the pointer where it is applies.
    Drawn {
        /// Exactly what [`drawn`] makes for that release; `None` when the
        /// drag pressed off the world.
        stroke: Option<Stroke>,
    },
}

/// The preview of `brush` with the pointer at the logical window point
/// `pointer`: [`Preview::Stroke`] with the stroke [`stroke`] makes for a
/// click there through `viewport`, with the same `logical`, `cells` and
/// `supplies`; otherwise why there is none.
#[must_use]
pub fn preview(
    brush: Brush,
    viewport: &Viewport,
    logical: Option<[f64; 2]>,
    cells: [u32; 2],
    pointer: Option<[f64; 2]>,
    supplies: &Supplies,
) -> Preview {
    let (Some(logical), Some(at)) = (logical, pointer) else {
        return Preview::OffWorld;
    };
    match stroke(brush, viewport, logical, cells, at, supplies) {
        Some(made) => Preview::Stroke {
            stroke: made,
            region: made
                .cell()
                .map(|cell| cell_region(viewport.extent(), cells, cell)),
        },
        None if brush == Brush::Material
            && supplies.material.is_none()
            && viewport.logical_to_world(at, logical).is_some() =>
        {
            Preview::NoMaterial
        }
        None => Preview::OffWorld,
    }
}

/// The preview of the removal brush's disk being drawn by a drag that
/// pressed at the logical window point `center`, with the pointer at
/// `pointer`: [`Preview::Drawn`] with what [`drawn`] makes for a release
/// there through `viewport`, with a content area of `logical` points.
#[must_use]
pub fn drawing(
    viewport: &Viewport,
    logical: Option<[f64; 2]>,
    center: [f64; 2],
    pointer: Option<[f64; 2]>,
) -> Preview {
    Preview::Drawn {
        stroke: logical
            .zip(pointer)
            .and_then(|(logical, edge)| drawn(viewport, logical, center, edge)),
    }
}

/// The region `preview` outlines on the world (plan §14.3: the preview
/// shows its "affected region, and whether it adds/removes energy"): its
/// cell's rectangle for a brush that applies to a cell, the disc a removal
/// takes, clicked or drawn, and whether the stroke removes energy from the
/// world (a removal does: the share of the removed organisms' energy not
/// recycled to the field is dissipated). `None` without a stroke.
#[must_use]
pub fn preview_region(preview: &Preview) -> Option<Region> {
    let (stroke, region) = match preview {
        Preview::Stroke { stroke, region } => (stroke, region),
        Preview::Drawn {
            stroke: Some(stroke),
        } => (stroke, &None),
        _ => return None,
    };
    let shape = match (stroke, region) {
        (_, Some(bounds)) => RegionShape::Rectangle(*bounds),
        (Stroke::Removal { center, radius }, None) => RegionShape::Disc {
            center: *center,
            radius: *radius,
        },
        _ => return None,
    };
    Some(Region {
        shape,
        effect: if matches!(stroke, Stroke::Removal { .. }) {
            RegionEffect::Removes
        } else {
            RegionEffect::Adds
        },
    })
}

/// The world rectangle of field cell `cell` on a grid of `cells` over a
/// world of `extent`, `[[x₀, x₁], [y₀, y₁]]`: the square whose points
/// [`stroke`]'s `⌊x / (L / cells)⌋` maps to `cell`.
#[must_use]
pub fn cell_region(extent: [f64; 2], cells: [u32; 2], cell: [u32; 2]) -> [[f64; 2]; 2] {
    [0, 1].map(|axis| {
        let width = extent[axis] / f64::from(cells[axis]);
        [
            f64::from(cell[axis]) * width,
            (f64::from(cell[axis]) + 1.0) * width,
        ]
    })
}
