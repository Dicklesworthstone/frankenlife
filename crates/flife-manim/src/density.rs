//! Density rendering (plan §2.3, §13.3 step 5): when organisms crowd below
//! a screen pixel, count them per screen bin instead of drawing dots.
//!
//! The world is cut into bins of about [`DENSITY_BIN_POINTS`] points on
//! screen, aligned to the world so the torus wraps exactly. Each bin holds
//! its organism count and the mean linear colour of the current mapping
//! over them. The bins are drawn like the field (FL-033): one texture of
//! the bins' corners, each the premultiplied mean of the four bins around
//! it, wrapping, on one [`Viewport::periodic_quad`], so the seam does not
//! show.
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
use fmn_render::{SurfaceDraw, SurfaceMesh, Texture, ThreeDDraw};

use crate::mapping::{ColorMapping, EpochColors, Look, Style};
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
        Look::Raw | Look::Sphere => count / scale,
    };
    level.clamp(0.0, 1.0)
}

/// Look class that affects density texels (`Look::Raw` and `Look::Sphere`
/// produce identical opaque grey texels).
#[inline]
const fn density_look(look: Look) -> Look {
    match look {
        Look::Glow => Look::Glow,
        Look::Raw | Look::Sphere => Look::Raw,
    }
}

/// Key identifying the geometry and UVs of a cached [`Viewport::periodic_quad`].
type QuadKey = ([u64; 2], [u64; 2], [u32; 2]);

#[inline]
fn quad_key(viewport: &Viewport, bins: [u32; 2]) -> QuadKey {
    (
        viewport.window_start().map(f64::to_bits),
        viewport.extent().map(f64::to_bits),
        bins,
    )
}

/// Key identifying the binned `(counts, sums)` buffers so unchanged pan/zoom
/// placements skip re-binning while mapping changes, bin-grid changes, or
/// different epochs re-bin cleanly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BinnedKey {
    mapping: ColorMapping,
    bins: [u32; 2],
    epoch_id: u64,
    resource: u64,
    tick: u64,
    len: usize,
    first_pos: Option<[u32; 2]>,
    last_pos: Option<[u32; 2]>,
    positions_ptr: usize,
}

fn binned_key(epoch: &Epoch, mapping: ColorMapping, bins: [u32; 2]) -> BinnedKey {
    let positions = epoch.positions();
    BinnedKey {
        mapping,
        bins,
        epoch_id: epoch.id().get(),
        resource: epoch.resource().get(),
        tick: epoch.tick().get(),
        len: positions.len(),
        first_pos: positions.first().map(|p| p.map(f32::to_bits)),
        last_pos: positions.last().map(|p| p.map(f32::to_bits)),
        positions_ptr: positions.as_ptr() as usize,
    }
}

/// Organism counts and mean colours per screen bin, as one texture.
#[derive(Debug)]
pub struct DensityLayer {
    viewport: Viewport,
    bins: [u32; 2],
    counts: Vec<u32>,
    sums: Vec<[f64; 3]>,
    texture: Option<Texture>,
    /// The normalized look `texture` was built for from the binned counts,
    /// colours and scale; `None` while it does not show them (before the first
    /// build, and from the start of each update until its build succeeds).
    built: Option<Look>,
    /// Provenance of the currently binned `(counts, sums)` and texture.
    binned: Option<BinnedKey>,
    quad: Option<SurfaceMesh>,
    quad_key: Option<QuadKey>,
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
            texture: None,
            built: None,
            binned: None,
            quad: None,
            quad_key: None,
            scale: 0.0,
        }
    }

    /// Use `viewport` from the next [`DensityLayer::update`] or
    /// [`DensityLayer::place_epoch`]. The current texture and quad stay valid
    /// meanwhile: bins are world-aligned and scene points do not depend on the
    /// drawable.
    pub const fn refit(&mut self, viewport: &Viewport) {
        self.viewport = *viewport;
    }

    /// Bin `epoch` and build the texture for `style`, advancing the decaying
    /// display scale.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for an epoch of another world than the
    /// viewport's; [`CoreError::AllocationFailed`] when the bins or the
    /// texels cannot be reserved; [`CoreError::NumericalInvalid`] if
    /// FrankenManim refuses the texture or the quad. The previous texture
    /// and quad are kept on error.
    pub fn update(&mut self, epoch: &Epoch, style: &Style) -> Result<(), CoreError> {
        self.bin_and_place(epoch, style, true)
    }

    /// Place `epoch` in the layer at the current viewport and `style` without
    /// advancing display-scale decay: what a pan, zoom, resize, or style
    /// change redraws.
    ///
    /// When `epoch`, `style.mapping`, and the viewport's bin grid match the
    /// currently binned buffers, reuses those bins directly via
    /// [`DensityLayer::place`] (keeping texels and moving only the quad when
    /// the look is unchanged). When entering density mode from points,
    /// changing colour mapping, resizing the bin grid, or placing another
    /// epoch, re-bins `epoch` while keeping an existing nonzero display scale
    /// unmoved on an unchanged bin grid.
    ///
    /// # Errors
    /// As [`DensityLayer::update`].
    pub fn place_epoch(&mut self, epoch: &Epoch, style: &Style) -> Result<(), CoreError> {
        self.bin_and_place(epoch, style, false)
    }

    fn bin_and_place(
        &mut self,
        epoch: &Epoch,
        style: &Style,
        advance_scale: bool,
    ) -> Result<(), CoreError> {
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
        let key = binned_key(epoch, style.mapping, bins);
        if !advance_scale && self.binned == Some(key) && self.bins == bins {
            return self.place(style);
        }
        // The bins change below: the texture and provenance match again only
        // once rebuilt.
        self.built = None;
        self.binned = None;
        let [columns, rows] = bins.map(|n| n as usize);
        let cells = columns * rows;
        let grid_changed = bins != self.bins;
        let base_scale = if grid_changed {
            // Reset `self.bins` before clearing so an allocation failure never
            // leaves a nonzero grid size pointing at empty buffers.
            self.bins = [0; 2];
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
            0.0
        } else {
            self.counts.fill(0);
            self.sums.fill([0.0; 3]);
            self.scale
        };

        let [width, height] = self.viewport.extent();
        let bin = |value: f32, extent: f64, count: usize| {
            let at = (f64::from(value) / extent * count as f64).floor();
            if at.is_finite() && at > 0.0 {
                (at as usize).min(count - 1)
            } else {
                0
            }
        };
        let positions = epoch.positions();
        let colors = EpochColors::new(style.mapping, epoch);
        colors.for_each(positions.len(), |row, color| {
            let position = positions[row];
            let cell = bin(position[1], height, rows) * columns + bin(position[0], width, columns);
            self.counts[cell] = self.counts[cell].saturating_add(1);
            let sum = &mut self.sums[cell];
            sum[0] += color.r;
            sum[1] += color.g;
            sum[2] += color.b;
        });
        let fullest = self.counts.iter().copied().max().unwrap_or(0);
        let next_scale = if advance_scale {
            f64::from(fullest).max(base_scale * SCALE_DECAY)
        } else if grid_changed || base_scale == 0.0 {
            f64::from(fullest)
        } else {
            base_scale
        };
        self.bins = bins;
        self.place_binned_at(next_scale, style)?;
        self.binned = Some(key);
        Ok(())
    }

    /// Re-place the last binned epoch in the layer at the current viewport
    /// and display scale, which does not move: what a pan or zoom redraws.
    /// The texels depend on the binned counts and colours, the scale and the
    /// look alone, so a texture built for `style`'s look is kept and only
    /// the quad moves. Nothing happens before the first update.
    ///
    /// # Errors
    /// As [`DensityLayer::update`].
    pub fn place(&mut self, style: &Style) -> Result<(), CoreError> {
        if self.bins == [0, 0] {
            return Ok(());
        }
        let want_look = density_look(style.look);
        if self.built == Some(want_look) {
            let want_quad = quad_key(&self.viewport, self.bins);
            if self.quad.is_none() || self.quad_key != Some(want_quad) {
                self.quad = Some(self.viewport.periodic_quad(self.bins)?);
                self.quad_key = Some(want_quad);
            }
            return Ok(());
        }
        self.place_binned_at(self.scale, style)
    }

    fn place_binned_at(&mut self, scale: f64, style: &Style) -> Result<(), CoreError> {
        let [columns, rows] = self.bins.map(|n| n as usize);
        let cells = columns.saturating_mul(rows);
        if columns == 0
            || rows == 0
            || columns > DENSITY_BINS_MAX as usize
            || self.counts.len() != cells
            || self.sums.len() != cells
        {
            return Err(CoreError::LayoutMismatch {
                buffer: "density layer bins",
                detail: format!(
                    "{} counts and {} sums for {columns} x {rows} bins",
                    self.counts.len(),
                    self.sums.len()
                ),
            });
        }
        let normalized_look = density_look(style.look);
        let valid_scale = scale.is_finite() && scale > 0.0;
        let scale_ln1p = if valid_scale { scale.ln_1p() } else { 0.0 };
        // Compute each bin's premultiplied RGBA once per bin into a two-row
        // stack ring (zero heap allocation, zero per-corner `%` division, and
        // 4x fewer `ln_1p` calls), matching the corner blend bit for bit.
        let fill_row = |bin_row: usize, out: &mut [[f64; 4]]| {
            let base = bin_row * columns;
            match normalized_look {
                Look::Raw | Look::Sphere => {
                    for (col, slot) in out.iter_mut().enumerate().take(columns) {
                        let level = if valid_scale {
                            (f64::from(self.counts[base + col]) / scale).clamp(0.0, 1.0)
                        } else {
                            0.0
                        };
                        *slot = [level, level, level, 1.0];
                    }
                }
                Look::Glow => {
                    for (col, slot) in out.iter_mut().enumerate().take(columns) {
                        let cell = base + col;
                        let count = self.counts[cell];
                        if count == 0 || !valid_scale {
                            *slot = [0.0; 4];
                        } else {
                            let c = f64::from(count);
                            let level = (c.ln_1p() / scale_ln1p).clamp(0.0, 1.0);
                            let sum = self.sums[cell];
                            *slot = [
                                (sum[0] / c) * level,
                                (sum[1] / c) * level,
                                (sum[2] / c) * level,
                                level,
                            ];
                        }
                    }
                }
            }
        };
        let mut texels = Vec::new();
        texels
            .try_reserve_exact(cells)
            .map_err(|_| allocation("density layer texels", cells, size_of::<[f32; 4]>()))?;
        let mut ring = [[[0.0_f64; 4]; DENSITY_BINS_MAX as usize]; 2];
        fill_row(rows - 1, &mut ring[0][..columns]);
        let mut prev = 0_usize;
        for row in 0..rows {
            let cur = 1 - prev;
            fill_row(row, &mut ring[cur][..columns]);
            let up_row = &ring[prev][..columns];
            let cur_row = &ring[cur][..columns];
            for column in 0..columns {
                // Texel (column, row) is the corner between bins
                // (column - 1, row - 1) and (column, row), wrapping: their
                // premultiplied mean.
                let left = if column == 0 { columns - 1 } else { column - 1 };
                let a = up_row[left];
                let b = up_row[column];
                let c = cur_row[left];
                let d = cur_row[column];
                texels.push([
                    ((a[0] + b[0] + c[0] + d[0]) / 4.0) as f32,
                    ((a[1] + b[1] + c[1] + d[1]) / 4.0) as f32,
                    ((a[2] + b[2] + c[2] + d[2]) / 4.0) as f32,
                    ((a[3] + b[3] + c[3] + d[3]) / 4.0) as f32,
                ]);
            }
            prev = cur;
        }
        // An identity, not the SHA-256 of every texel (FL-033).
        let texture =
            Texture::from_premultiplied_linear_unhashed(self.bins[0], self.bins[1], texels)
                .map_err(|error| CoreError::NumericalInvalid {
                    quantity: "density layer texture",
                    detail: error.to_string(),
                })?;
        let want_quad = quad_key(&self.viewport, self.bins);
        let new_quad = if self.quad.is_none() || self.quad_key != Some(want_quad) {
            Some(self.viewport.periodic_quad(self.bins)?)
        } else {
            None
        };
        self.texture = Some(texture);
        self.built = Some(normalized_look);
        if let Some(quad) = new_quad {
            self.quad = Some(quad);
            self.quad_key = Some(want_quad);
        }
        self.scale = scale;
        Ok(())
    }

    /// Bins per axis of the current texture; `[0, 0]` before the first
    /// update.
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

    /// The layer's texture once an epoch has been binned: one texel per bin
    /// corner, rows top to bottom, premultiplied linear.
    #[must_use]
    pub const fn texture(&self) -> Option<&Texture> {
        self.texture.as_ref()
    }

    /// The layer's draw, once an epoch has been binned: its texture on its
    /// quad as an image, unlit, repeating, without a depth test, in painter
    /// order.
    #[must_use]
    pub fn draw(&self) -> Option<ThreeDDraw<'_>> {
        let (Some(quad), Some(texture)) = (&self.quad, &self.texture) else {
            return None;
        };
        Some(ThreeDDraw::Surface(SurfaceDraw::image(quad, texture)))
    }
}

fn allocation(buffer: &'static str, count: usize, each: usize) -> CoreError {
    CoreError::AllocationFailed {
        buffer,
        bytes: u64::try_from(count.saturating_mul(each)).unwrap_or(u64::MAX),
    }
}
