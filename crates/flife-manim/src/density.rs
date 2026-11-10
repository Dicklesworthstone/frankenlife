//! Density rendering (plan §2.3, §13.3 step 5): when organisms crowd below
//! a screen pixel, count them per screen bin instead of drawing dots.
//!
//! The world is cut into bins of about [`DENSITY_BIN_POINTS`] points on
//! screen, aligned to the world so the torus wraps exactly. Each bin holds
//! its organism count and the mean linear colour of the current mapping
//! over them. The bins are drawn like the field: one vertex-coloured grid
//! whose vertices sit on bin corners, each the alpha-weighted mean of the
//! four bins around it, wrapping, so the seam does not show.
//!
//! Bins are several pixels, not one: FrankenManim's Metal 3D route has no
//! textured surfaces, and a vertex grid per pixel would be about four
//! million triangles at 1080p.
//!
//! - Glow: a bin's colour is its mean colour, its opacity
//!   `ln(1 + n) / ln(1 + scale)` over the field.
//! - Raw: opaque linear grey `n / scale`, and the field is not drawn.
//!
//! `scale` is the fullest bin, or the previous scale times
//! [`SCALE_DECAY`](crate::SCALE_DECAY) if larger; it restarts when the bin
//! grid changes. Counts and colours come only from the epoch.

use flife_core::CoreError;
use flife_render::Epoch;
use fmn_core::color::LinearRgba;
use fmn_render::{SurfaceDraw, SurfaceMesh, ThreeDDraw};

use crate::mapping::{Look, Style, mapped_color, traits};
use crate::{SCALE_DECAY, Viewport};

/// The target bin width in points (drawable pixels at one pixel per point).
pub const DENSITY_BIN_POINTS: f64 = 6.0;

/// The most bins per axis.
pub const DENSITY_BINS_MAX: u32 = 512;

/// Bins per axis for `viewport`: the world's drawable span at the
/// whole-torus fit over [`DENSITY_BIN_POINTS`] at the viewport's pixels per
/// point, rounded up, in `1..=DENSITY_BINS_MAX`. The zoom does not change
/// them: zoomed in, a bin is wider than its target on screen.
#[must_use]
pub fn density_bins(viewport: &Viewport) -> [u32; 2] {
    // Exact at one pixel per point, so the bins are as before there.
    let bin = DENSITY_BIN_POINTS * viewport.pixels_per_point();
    viewport.extent().map(|extent| {
        let bins = (extent * viewport.fit_scale() / bin).ceil();
        if bins.is_finite() {
            (bins as u32).clamp(1, DENSITY_BINS_MAX)
        } else {
            1
        }
    })
}

/// A bin's display level in `[0, 1]`: `ln(1 + count) / ln(1 + scale)` for
/// the glow look, `count / scale` for raw; `0` for a non-positive or
/// non-finite scale.
#[must_use]
pub fn density_level(count: u32, scale: f64, look: Look) -> f64 {
    if !scale.is_finite() || scale <= 0.0 {
        return 0.0;
    }
    let count = f64::from(count);
    let level = match look {
        Look::Glow => count.ln_1p() / scale.ln_1p(),
        Look::Raw => count / scale,
    };
    level.clamp(0.0, 1.0)
}

/// Organism counts and mean colours per screen bin, as one grid.
#[derive(Debug)]
pub struct DensityLayer {
    viewport: Viewport,
    bins: [u32; 2],
    counts: Vec<u32>,
    sums: Vec<[f64; 3]>,
    mesh: Option<SurfaceMesh>,
    scale: f64,
}

impl DensityLayer {
    /// A layer over `viewport`'s world, empty until an epoch is binned.
    #[must_use]
    pub const fn new(viewport: &Viewport) -> Self {
        Self {
            viewport: *viewport,
            bins: [0; 2],
            counts: Vec::new(),
            sums: Vec::new(),
            mesh: None,
            scale: 0.0,
        }
    }

    /// Use `viewport` from the next [`DensityLayer::update`]. The current
    /// grid stays valid meanwhile: bins are world-aligned and scene points
    /// do not depend on the drawable.
    pub const fn refit(&mut self, viewport: &Viewport) {
        self.viewport = *viewport;
    }

    /// Bin `epoch` and build the grid for `style`.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for an epoch of another world than the
    /// viewport's; [`CoreError::AllocationFailed`] when the bins or the
    /// grid cannot be reserved; [`CoreError::NumericalInvalid`] if
    /// FrankenManim refuses the grid. The previous grid is kept on error.
    pub fn update(&mut self, epoch: &Epoch, style: &Style) -> Result<(), CoreError> {
        if epoch.extent().map(f64::to_bits) != self.viewport.extent().map(f64::to_bits) {
            return Err(CoreError::LayoutMismatch {
                buffer: "density layer",
                detail: format!(
                    "epoch world {:?} is not the viewport's {:?}",
                    epoch.extent(),
                    self.viewport.extent()
                ),
            });
        }
        let bins = density_bins(&self.viewport);
        let [columns, rows] = bins.map(|n| n as usize);
        let cells = columns * rows;
        if bins != self.bins {
            self.counts.clear();
            self.sums.clear();
            self.counts
                .try_reserve_exact(cells)
                .map_err(|_| allocation("density bins", cells, size_of::<u32>()))?;
            self.sums
                .try_reserve_exact(cells)
                .map_err(|_| allocation("density colours", cells, size_of::<[f64; 3]>()))?;
            self.counts.resize(cells, 0);
            self.sums.resize(cells, [0.0; 3]);
            self.scale = 0.0;
        } else {
            self.counts.fill(0);
            self.sums.fill([0.0; 3]);
        }

        let [width, height] = self.viewport.extent();
        let bin = |value: f32, extent: f64, count: usize| {
            let at = (f64::from(value) / extent * count as f64).floor();
            if at.is_finite() && at > 0.0 {
                (at as usize).min(count - 1)
            } else {
                0
            }
        };
        let speed = epoch.maximum_speed();
        for (position, traits) in epoch.positions().iter().zip(traits(epoch)) {
            let cell = bin(position[1], height, rows) * columns + bin(position[0], width, columns);
            let color = mapped_color(style.mapping, &traits, speed);
            self.counts[cell] += 1;
            let sum = &mut self.sums[cell];
            sum[0] += color.r;
            sum[1] += color.g;
            sum[2] += color.b;
        }
        let fullest = self.counts.iter().copied().max().unwrap_or(0);
        self.scale = f64::from(fullest).max(self.scale * SCALE_DECAY);
        self.bins = bins;
        self.place_binned(style)
    }

    /// Re-place the last binned epoch in the layer's grid at the current
    /// viewport and display scale, which does not move: what a pan or zoom
    /// redraws. Nothing happens before the first update.
    ///
    /// # Errors
    /// As [`DensityLayer::update`].
    pub fn place(&mut self, style: &Style) -> Result<(), CoreError> {
        if self.bins == [0, 0] {
            return Ok(());
        }
        self.place_binned(style)
    }

    fn place_binned(&mut self, style: &Style) -> Result<(), CoreError> {
        let [columns, rows] = self.bins.map(|n| n as usize);
        // Each bin's straight colour: the mean mapped colour at the glow
        // opacity, or opaque grey for raw.
        let shown = |cell: usize| -> LinearRgba {
            let count = self.counts[cell];
            let level = density_level(count, self.scale, style.look);
            match style.look {
                Look::Raw => LinearRgba {
                    r: level,
                    g: level,
                    b: level,
                    a: 1.0,
                },
                Look::Glow if count == 0 => LinearRgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                },
                Look::Glow => {
                    let [r, g, b] = self.sums[cell].map(|sum| sum / f64::from(count));
                    LinearRgba { r, g, b, a: level }
                }
            }
        };
        let (vertices, indices) = self.viewport.periodic_grid(
            [columns, rows],
            ["density layer vertices", "density layer indices"],
            |[column, row]| {
                // The corner between bins (column - 1, row - 1) and
                // (column, row), wrapping; premultiplied mean, then straight.
                let (left, up) = ((column + columns - 1) % columns, (row + rows - 1) % rows);
                let (right, down) = (column % columns, row % rows);
                let mut sum = [0.0; 4];
                for cell in [
                    up * columns + left,
                    up * columns + right,
                    down * columns + left,
                    down * columns + right,
                ] {
                    let color = shown(cell);
                    sum[0] += color.r * color.a;
                    sum[1] += color.g * color.a;
                    sum[2] += color.b * color.a;
                    sum[3] += color.a;
                }
                if sum[3] > 0.0 {
                    LinearRgba {
                        r: sum[0] / sum[3],
                        g: sum[1] / sum[3],
                        b: sum[2] / sum[3],
                        a: sum[3] / 4.0,
                    }
                } else {
                    LinearRgba {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.0,
                    }
                }
            },
        )?;
        let mesh =
            SurfaceMesh::new(vertices, indices).map_err(|error| CoreError::NumericalInvalid {
                quantity: "density layer grid",
                detail: error.to_string(),
            })?;
        self.mesh = Some(mesh);
        Ok(())
    }

    /// Bins per axis of the current grid; `[0, 0]` before the first update.
    #[must_use]
    pub const fn bins(&self) -> [u32; 2] {
        self.bins
    }

    /// Organisms per bin, rows top to bottom, of the last epoch binned.
    #[must_use]
    pub fn counts(&self) -> &[u32] {
        &self.counts
    }

    /// The count shown at full level.
    #[must_use]
    pub const fn scale(&self) -> f64 {
        self.scale
    }

    /// The layer's draw, once an epoch has been binned: unlit, without a
    /// depth test, in painter order.
    #[must_use]
    pub fn draw(&self) -> Option<ThreeDDraw<'_>> {
        self.mesh.as_ref().map(|mesh| {
            ThreeDDraw::Surface(SurfaceDraw {
                shading: [0.0; 3],
                depth_test: false,
                ..SurfaceDraw::new(mesh)
            })
        })
    }
}

fn allocation(buffer: &'static str, count: usize, each: usize) -> CoreError {
    CoreError::AllocationFailed {
        buffer,
        bytes: u64::try_from(count.saturating_mul(each)).unwrap_or(u64::MAX),
    }
}
