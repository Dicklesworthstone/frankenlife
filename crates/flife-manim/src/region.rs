//! The region a brush would act on, or the measuring tool measures,
//! outlined on the world (FL-022, plan §14.3: "Every intervention preview
//! shows its units, affected region, and whether it adds/removes energy";
//! "temporary measurement regions").
//!
//! A region is a cell's rectangle or a disc, in world units, and its
//! [`RegionEffect`]: whether the stroke on it adds energy to the world or
//! removes it, or whether it is a measurement, which changes nothing. The
//! effect is the outline's colour. The outline lies just outside the
//! region, so what is inside stays visible, and keeps its width on screen
//! at any zoom or pixel density.

use flife_core::CoreError;
use fmn_core::color::LinearRgba;
use fmn_render::{SurfaceMesh, SurfaceVertex};

use crate::Viewport;

/// The outline's width, in points.
pub const REGION_WIDTH_POINTS: f64 = 1.5;

/// Segments in a disc's outline.
pub const REGION_SEGMENTS: usize = 48;

/// The outline of a region a stroke adds energy to: amber, nearly opaque,
/// linear light.
pub const REGION_ADDS_COLOR: LinearRgba = LinearRgba {
    r: 1.0,
    g: 0.62,
    b: 0.12,
    a: 0.9,
};

/// The outline of a region a stroke removes energy from: red, nearly
/// opaque, linear light.
pub const REGION_REMOVES_COLOR: LinearRgba = LinearRgba {
    r: 1.0,
    g: 0.12,
    b: 0.1,
    a: 0.9,
};

/// The outline of a measurement region, which changes nothing: pale cyan,
/// nearly opaque, linear light.
pub const REGION_MEASURES_COLOR: LinearRgba = LinearRgba {
    r: 0.45,
    g: 0.9,
    b: 1.0,
    a: 0.9,
};

/// A corner of an outline's band, as world offsets from the region's
/// centre: the outer point, then the inner.
type Corner = ([f64; 2], [f64; 2]);

/// A region's shape, in world units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RegionShape {
    /// `[[x₀, x₁], [y₀, y₁]]`, with `x₀ < x₁` and `y₀ < y₁`.
    Rectangle([[f64; 2]; 2]),
    /// A disc of `radius` about `center`.
    Disc {
        /// A world position.
        center: [f64; 2],
        /// In world units, positive.
        radius: f64,
    },
}

/// What a region's stroke does to the world's energy, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionEffect {
    /// A stroke that adds energy to the world.
    Adds,
    /// A stroke that removes energy from the world.
    Removes,
    /// No stroke: a measurement region (plan §14.3), which changes nothing.
    Measures,
}

/// The region a stroke acts on and what it does to the world's energy, or
/// a region measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    /// Where.
    pub shape: RegionShape,
    /// What the stroke does, or that it is a measurement.
    pub effect: RegionEffect,
}

impl Region {
    /// The outline's colour: [`REGION_ADDS_COLOR`] for a stroke that adds
    /// energy, [`REGION_REMOVES_COLOR`] for one that removes it, and
    /// [`REGION_MEASURES_COLOR`] for a measurement.
    #[must_use]
    pub const fn color(self) -> LinearRgba {
        match self.effect {
            RegionEffect::Adds => REGION_ADDS_COLOR,
            RegionEffect::Removes => REGION_REMOVES_COLOR,
            RegionEffect::Measures => REGION_MEASURES_COLOR,
        }
    }
}

/// The outline of `region` in `viewport`'s scene: a band
/// [`REGION_WIDTH_POINTS`] wide just outside it, in its colour. A
/// rectangle's band has mitred corners (4 quads, 8 triangles); a disc's is
/// an annulus of [`REGION_SEGMENTS`] quads, from the disc's edge outward.
/// The region is placed where the viewport draws its centre.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for a non-finite region, an empty
/// rectangle or a radius that is not positive, or when FrankenManim
/// refuses the mesh; [`CoreError::AllocationFailed`] when the mesh cannot
/// be reserved.
pub fn region_outline(viewport: &Viewport, region: Region) -> Result<SurfaceMesh, CoreError> {
    let refused = |detail: String| CoreError::NumericalInvalid {
        quantity: "brush region",
        detail,
    };
    let width = REGION_WIDTH_POINTS * viewport.pixels_per_point() / viewport.scale();
    let color = region.color();
    // Each corner of the band as a world offset from the centre, outer
    // then inner, counter-clockwise on screen (the world's y runs down).
    let (center, offsets): ([f64; 2], Vec<Corner>) = match region.shape {
        RegionShape::Rectangle([[x0, x1], [y0, y1]]) => {
            if !([x0, x1, y0, y1].iter().all(|value| value.is_finite()) && x0 < x1 && y0 < y1) {
                return Err(refused(format!("the rectangle {x0}..{x1} × {y0}..{y1}")));
            }
            let half = [(x1 - x0) / 2.0, (y1 - y0) / 2.0];
            let corners = [[-1.0, 1.0], [1.0, 1.0], [1.0, -1.0], [-1.0, -1.0]];
            let mut offsets = Vec::new();
            reserve(&mut offsets, corners.len())?;
            for [sx, sy] in corners {
                let inner = [sx * half[0], sy * half[1]];
                offsets.push(([inner[0] + sx * width, inner[1] + sy * width], inner));
            }
            ([(x0 + x1) / 2.0, (y0 + y1) / 2.0], offsets)
        }
        RegionShape::Disc { center, radius } => {
            if !(center.iter().all(|value| value.is_finite()) && radius.is_finite() && radius > 0.0)
            {
                return Err(refused(format!("a disc of radius {radius} at {center:?}")));
            }
            let mut offsets = Vec::new();
            reserve(&mut offsets, REGION_SEGMENTS)?;
            for k in 0..REGION_SEGMENTS {
                let angle = std::f64::consts::TAU * k as f64 / REGION_SEGMENTS as f64;
                let (sin, cos) = angle.sin_cos();
                // Counter-clockwise on screen: y negated into the world.
                let at = |radius: f64| [radius * cos, -radius * sin];
                offsets.push((at(radius + width), at(radius)));
            }
            (center, offsets)
        }
    };
    let [x, y, z] = viewport.scene_point(center);
    let segments = offsets.len();
    let mut vertices = Vec::new();
    vertices
        .try_reserve_exact(2 * segments)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: "brush region vertices",
            bytes: u64::try_from(2 * segments * size_of::<SurfaceVertex>()).unwrap_or(u64::MAX),
        })?;
    for (outer, inner) in &offsets {
        for [dx, dy] in [outer, inner] {
            // The scene's y runs up.
            vertices.push(SurfaceVertex::colored(
                [x + dx, y - dy, z],
                [0.0, 0.0, 1.0],
                color,
            ));
        }
    }
    let mut indices = Vec::new();
    indices
        .try_reserve_exact(6 * segments)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: "brush region indices",
            bytes: u64::try_from(6 * segments * size_of::<u32>()).unwrap_or(u64::MAX),
        })?;
    let at = |k: usize, ring: usize| u32::try_from(2 * (k % segments) + ring).unwrap_or(u32::MAX);
    for k in 0..segments {
        // Clockwise in the scene, as the selection ring is.
        let (a, b, c, d) = (at(k, 0), at(k, 1), at(k + 1, 1), at(k + 1, 0));
        indices.extend_from_slice(&[a, b, c, a, c, d]);
    }
    SurfaceMesh::new(vertices, indices).map_err(|error| refused(error.to_string()))
}

/// Reserve `count` corners.
fn reserve(offsets: &mut Vec<Corner>, count: usize) -> Result<(), CoreError> {
    offsets
        .try_reserve_exact(count)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: "brush region corners",
            bytes: u64::try_from(count * size_of::<Corner>()).unwrap_or(u64::MAX),
        })
}
