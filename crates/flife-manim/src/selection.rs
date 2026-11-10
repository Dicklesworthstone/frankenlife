//! Picking an organism under the pointer, and the ring that marks the
//! selected one (plan §13.6, FL-020).
//!
//! A pick reads what is drawn: each organism's stable identity and the
//! position it is drawn at (the epoch's own, or an interpolation's), placed
//! through the [`Viewport`]. It returns an identity, never a storage slot,
//! so a host validates it against the authoritative world before showing
//! anything; an organism that has died since is gone, whatever now occupies
//! its old slot.

use flife_core::CoreError;
use flife_core::ids::OrganismId;
use fmn_core::color::LinearRgba;
use fmn_render::{SurfaceMesh, SurfaceVertex};

use crate::{Viewport, dot_radius};

/// The default pick radius in drawable pixels.
pub const PICK_RADIUS_PIXELS: f64 = 8.0;

/// The selection ring's gap outside the organism's dot, in drawable pixels.
pub const SELECTION_RING_GAP_PIXELS: f64 = 3.0;

/// The selection ring's width, in drawable pixels.
pub const SELECTION_RING_WIDTH_PIXELS: f64 = 2.0;

/// The selection ring's colour: near-white, nearly opaque, linear light.
pub const SELECTION_RING_COLOR: LinearRgba = LinearRgba {
    r: 0.95,
    g: 0.95,
    b: 0.95,
    a: 0.95,
};

/// Segments in the selection ring's polygon.
pub const SELECTION_RING_SEGMENTS: usize = 48;

/// The ring around a selected organism at world `position`: an annulus in
/// the view's scene, centred on the organism's scene point, from
/// [`SELECTION_RING_GAP_PIXELS`] outside its dot to
/// [`SELECTION_RING_WIDTH_PIXELS`] beyond that, whatever the zoom.
///
/// # Errors
/// [`CoreError::AllocationFailed`] when the mesh cannot be reserved;
/// [`CoreError::NumericalInvalid`] when FrankenManim refuses it (a
/// non-finite position).
pub fn selection_ring(viewport: &Viewport, position: [f64; 2]) -> Result<SurfaceMesh, CoreError> {
    let scale = viewport.scale();
    let inner = dot_radius(viewport) + SELECTION_RING_GAP_PIXELS / scale;
    let outer = inner + SELECTION_RING_WIDTH_PIXELS / scale;
    let [x, y, z] = viewport.scene_point(position);
    let segments = SELECTION_RING_SEGMENTS;
    let mut vertices = Vec::new();
    vertices
        .try_reserve_exact(2 * segments)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: "selection ring vertices",
            bytes: u64::try_from(2 * segments * size_of::<SurfaceVertex>()).unwrap_or(u64::MAX),
        })?;
    for k in 0..segments {
        let angle = std::f64::consts::TAU * k as f64 / segments as f64;
        let (sin, cos) = angle.sin_cos();
        for radius in [outer, inner] {
            vertices.push(SurfaceVertex::colored(
                [x + radius * cos, y + radius * sin, z],
                [0.0, 0.0, 1.0],
                SELECTION_RING_COLOR,
            ));
        }
    }
    let mut indices = Vec::new();
    indices
        .try_reserve_exact(6 * segments)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: "selection ring indices",
            bytes: u64::try_from(6 * segments * size_of::<u32>()).unwrap_or(u64::MAX),
        })?;
    let at = |k: usize, ring: usize| u32::try_from(2 * (k % segments) + ring).unwrap_or(u32::MAX);
    for k in 0..segments {
        // Clockwise in the scene, as the field and density grids are.
        let (a, b, c, d) = (at(k, 0), at(k, 1), at(k + 1, 1), at(k + 1, 0));
        indices.extend_from_slice(&[a, b, c, a, c, d]);
    }
    SurfaceMesh::new(vertices, indices).map_err(|error| CoreError::NumericalInvalid {
        quantity: "selection ring",
        detail: error.to_string(),
    })
}

/// The organism drawn nearest the drawable position `at`, within
/// `radius_pixels` of it, from `drawn` (identity, world position) pairs:
/// the nearest by drawable distance, the lowest identity among equals.
/// `None` when nothing is drawn that close, or for a non-finite position or
/// radius.
#[must_use]
pub fn pick(
    viewport: &Viewport,
    drawn: impl IntoIterator<Item = (u64, [f64; 2])>,
    at: [f64; 2],
    radius_pixels: f64,
) -> Option<OrganismId> {
    if !(at.iter().all(|value| value.is_finite()) && radius_pixels.is_finite()) {
        return None;
    }
    let mut best: Option<(f64, u64)> = None;
    let reach = radius_pixels * radius_pixels;
    for (id, position) in drawn {
        let place = viewport.world_to_drawable(position);
        let (dx, dy) = (place[0] - at[0], place[1] - at[1]);
        let distance = dx * dx + dy * dy;
        if distance > reach {
            continue;
        }
        let better = match best {
            None => true,
            Some((nearest, holder)) => distance < nearest || (distance == nearest && id < holder),
        };
        if better {
            best = Some((distance, id));
        }
    }
    best.map(|(_, id)| OrganismId::new(id))
}
