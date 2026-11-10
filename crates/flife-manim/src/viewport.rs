//! The exact mapping from world units to the drawable and back (plan §13.6),
//! with a camera that pans and zooms across the torus's seams (FL-020).
//!
//! Four coordinate spaces meet at the window:
//!
//! - **World**: `[0, width) × [0, height)` in world units, periodic on both
//!   axes, y downward (the field's row order).
//! - **Scene**: FrankenManim's camera space, world units centered on the
//!   view's center with y upward.
//! - **Drawable**: continuous pixels from the drawable's top-left corner.
//!   Pixel `(i, j)` covers `[i, i + 1) × [j, j + 1)`.
//! - **Logical**: window points from the content area's top-left corner,
//!   which the native host reports. A logical point maps to the drawable by
//!   the ratio of drawable pixels to logical points on each axis.
//!
//! A [`Viewport`] shows one image of the torus: the window `[lo, lo + L]`
//! on each axis, where `lo = center − L/2`, centered on the drawable and
//! scaled by `zoom` times the scale that fits the whole torus. Every world
//! point is drawn at its image inside that window, and only there, so the
//! view wraps across the seams without drawing anything twice; an axis on
//! which the window is narrower than the drawable shows letterbox margin
//! beyond it. [`Viewport::fit`] centers the whole torus at zoom 1, where
//! `lo` is zero and every coordinate is the one the fitted viewport had
//! before the camera existed.
//!
//! A point on the window's edge (`lo` or `lo + L`, the same torus point)
//! keeps the image it names when it lies in the closed window, so a grid
//! corner at `L` is drawn at the far edge; every other point is wrapped
//! into the window.

use flife_core::CoreError;
use fmn_core::color::LinearRgba;
use fmn_render::{CameraConfig, CameraFrame, DEFAULT_FOVY, SurfaceVertex, ThreeDCamera};

/// The deepest zoom: 4,096 times the whole-torus fit.
pub const MAXIMUM_ZOOM: f64 = 4096.0;

/// The whole torus, or a panned and zoomed window of it, in a drawable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    extent: [f64; 2],
    drawable: [u32; 2],
    fit_scale: f64,
    center: [f64; 2],
    zoom: f64,
    scale: f64,
    origin: [f64; 2],
}

impl Viewport {
    /// Fit a world of `extent` world units into a drawable of `drawable`
    /// pixels: centered, zoom 1.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for an extent that is not finite and
    /// positive, or an empty drawable.
    pub fn fit(extent: [f64; 2], drawable: [u32; 2]) -> Result<Self, CoreError> {
        if !extent
            .iter()
            .all(|length| length.is_finite() && *length > 0.0)
        {
            return Err(CoreError::NumericalInvalid {
                quantity: "viewport world extent",
                detail: format!("{extent:?} must be finite and positive"),
            });
        }
        if drawable.contains(&0) {
            return Err(CoreError::NumericalInvalid {
                quantity: "viewport drawable size",
                detail: format!("{drawable:?} must be at least one pixel each way"),
            });
        }
        let pixels = drawable.map(f64::from);
        let fit_scale = (pixels[0] / extent[0]).min(pixels[1] / extent[1]);
        Ok(Self::placed(
            extent,
            drawable,
            fit_scale,
            extent.map(|length| length / 2.0),
            1.0,
        ))
    }

    fn placed(
        extent: [f64; 2],
        drawable: [u32; 2],
        fit_scale: f64,
        center: [f64; 2],
        zoom: f64,
    ) -> Self {
        let pixels = drawable.map(f64::from);
        let scale = if zoom == 1.0 {
            fit_scale
        } else {
            fit_scale * zoom
        };
        // The drawable position of the window's start, `lo`: the window is
        // centered, so `center` lands on the drawable's center.
        let origin = [
            (pixels[0] - extent[0] * scale) / 2.0,
            (pixels[1] - extent[1] * scale) / 2.0,
        ];
        Self {
            extent,
            drawable,
            fit_scale,
            center,
            zoom,
            scale,
            origin,
        }
    }

    /// This viewport's world and drawable, viewed at `center` (wrapped into
    /// the world) and `zoom`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a center that is not finite, or a
    /// zoom outside `[1, MAXIMUM_ZOOM]`.
    pub fn with_view(&self, center: [f64; 2], zoom: f64) -> Result<Self, CoreError> {
        if !center.iter().all(|coordinate| coordinate.is_finite()) {
            return Err(CoreError::NumericalInvalid {
                quantity: "viewport center",
                detail: format!("{center:?} must be finite"),
            });
        }
        if !(1.0..=MAXIMUM_ZOOM).contains(&zoom) {
            return Err(CoreError::NumericalInvalid {
                quantity: "viewport zoom",
                detail: format!("{zoom} is outside [1, {MAXIMUM_ZOOM}]"),
            });
        }
        let center = [
            wrap(center[0], self.extent[0]),
            wrap(center[1], self.extent[1]),
        ];
        Ok(Self::placed(
            self.extent,
            self.drawable,
            self.fit_scale,
            center,
            zoom,
        ))
    }

    /// The same view in a drawable of `drawable` pixels.
    ///
    /// # Errors
    /// As [`Viewport::fit`].
    pub fn resized(&self, drawable: [u32; 2]) -> Result<Self, CoreError> {
        let fitted = Self::fit(self.extent, drawable)?;
        Ok(Self::placed(
            self.extent,
            drawable,
            fitted.fit_scale,
            self.center,
            self.zoom,
        ))
    }

    /// The view zoomed by `factor` about the drawable position `anchor`:
    /// the world point under the anchor stays under it. The zoom is clamped
    /// to `[1, MAXIMUM_ZOOM]`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a factor that is not finite and
    /// positive, or an anchor that is not finite.
    pub fn zoomed_about(&self, anchor: [f64; 2], factor: f64) -> Result<Self, CoreError> {
        if !(factor.is_finite() && factor > 0.0) {
            return Err(CoreError::NumericalInvalid {
                quantity: "viewport zoom factor",
                detail: format!("{factor} must be finite and positive"),
            });
        }
        if !anchor.iter().all(|coordinate| coordinate.is_finite()) {
            return Err(CoreError::NumericalInvalid {
                quantity: "viewport zoom anchor",
                detail: format!("{anchor:?} must be finite"),
            });
        }
        let zoom = (self.zoom * factor).clamp(1.0, MAXIMUM_ZOOM);
        let scale = self.fit_scale * zoom;
        let pixels = self.drawable.map(f64::from);
        // The anchor's world offset from the center, before and after.
        let center = [0, 1].map(|axis| {
            let from_middle = anchor[axis] - pixels[axis] / 2.0;
            self.center[axis] + from_middle / self.scale - from_middle / scale
        });
        self.with_view(center, zoom)
    }

    /// The view moved so the world follows a drag of `delta` drawable
    /// pixels.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a delta that is not finite.
    pub fn panned(&self, delta: [f64; 2]) -> Result<Self, CoreError> {
        if !delta.iter().all(|coordinate| coordinate.is_finite()) {
            return Err(CoreError::NumericalInvalid {
                quantity: "viewport pan",
                detail: format!("{delta:?} must be finite"),
            });
        }
        self.with_view(
            [
                self.center[0] - delta[0] / self.scale,
                self.center[1] - delta[1] / self.scale,
            ],
            self.zoom,
        )
    }

    /// World width and height in world units.
    #[must_use]
    pub const fn extent(&self) -> [f64; 2] {
        self.extent
    }

    /// Drawable width and height in pixels.
    #[must_use]
    pub const fn drawable(&self) -> [u32; 2] {
        self.drawable
    }

    /// Drawable pixels per world unit, the same on both axes: the fit scale
    /// times the zoom.
    #[must_use]
    pub const fn scale(&self) -> f64 {
        self.scale
    }

    /// Drawable pixels per world unit at zoom 1, whatever the view.
    #[must_use]
    pub const fn fit_scale(&self) -> f64 {
        self.fit_scale
    }

    /// The world point at the drawable's center.
    #[must_use]
    pub const fn center(&self) -> [f64; 2] {
        self.center
    }

    /// The zoom, in `[1, MAXIMUM_ZOOM]`.
    #[must_use]
    pub const fn zoom(&self) -> f64 {
        self.zoom
    }

    /// The window's start on each axis, `center − L/2`, in world units (it
    /// may be negative).
    #[must_use]
    pub fn window_start(&self) -> [f64; 2] {
        [
            self.center[0] - self.extent[0] / 2.0,
            self.center[1] - self.extent[1] / 2.0,
        ]
    }

    /// The drawable position of the window's start: at fit, the letterbox
    /// margin.
    #[must_use]
    pub const fn origin(&self) -> [f64; 2] {
        self.origin
    }

    /// A world point's offset into the window, in `[0, L]` on each axis.
    #[must_use]
    pub fn image(&self, point: [f64; 2]) -> [f64; 2] {
        let start = self.window_start();
        [0, 1].map(|axis| {
            let offset = point[axis] - start[axis];
            if (0.0..=self.extent[axis]).contains(&offset) {
                offset
            } else {
                wrap(offset, self.extent[axis])
            }
        })
    }

    /// The drawable position of a world point's image.
    #[must_use]
    pub fn world_to_drawable(&self, point: [f64; 2]) -> [f64; 2] {
        let image = self.image(point);
        [
            self.origin[0] + image[0] * self.scale,
            self.origin[1] + image[1] * self.scale,
        ]
    }

    /// The world point at a drawable position, or `None` outside the
    /// window (the letterbox margin). The window is half-open, so its far
    /// edges are margin.
    #[must_use]
    pub fn drawable_to_world(&self, position: [f64; 2]) -> Option<[f64; 2]> {
        let image = [
            (position[0] - self.origin[0]) / self.scale,
            (position[1] - self.origin[1]) / self.scale,
        ];
        let inside = image
            .iter()
            .zip(self.extent)
            .all(|(offset, length)| *offset >= 0.0 && *offset < length);
        if !inside {
            return None;
        }
        let start = self.window_start();
        Some([0, 1].map(|axis| {
            let point = start[axis] + image[axis];
            if (0.0..self.extent[axis]).contains(&point) {
                point
            } else {
                wrap(point, self.extent[axis])
            }
        }))
    }

    /// The world point under a logical window point, given the content
    /// area's logical size; `None` in the margin or while the content area
    /// is empty.
    #[must_use]
    pub fn logical_to_world(&self, point: [f64; 2], logical_size: [f64; 2]) -> Option<[f64; 2]> {
        self.logical_to_drawable(point, logical_size)
            .and_then(|position| self.drawable_to_world(position))
    }

    /// The drawable position of a logical window point, given the content
    /// area's logical size; `None` while the content area is empty.
    #[must_use]
    pub fn logical_to_drawable(&self, point: [f64; 2], logical_size: [f64; 2]) -> Option<[f64; 2]> {
        if !logical_size
            .iter()
            .all(|length| length.is_finite() && *length > 0.0)
        {
            return None;
        }
        Some([
            point[0] * f64::from(self.drawable[0]) / logical_size[0],
            point[1] * f64::from(self.drawable[1]) / logical_size[1],
        ])
    }

    /// The scene point FrankenManim draws for a world point: its image,
    /// centered, y up.
    #[must_use]
    pub fn scene_point(&self, point: [f64; 2]) -> [f64; 3] {
        let image = self.image(point);
        [
            image[0] - self.extent[0] / 2.0,
            self.extent[1] / 2.0 - image[1],
            0.0,
        ]
    }

    /// The camera frame's width and height in scene units: the whole
    /// drawable, margin included.
    #[must_use]
    pub fn frame_shape(&self) -> [f64; 2] {
        [
            f64::from(self.drawable[0]) / self.scale,
            f64::from(self.drawable[1]) / self.scale,
        ]
    }

    /// A FrankenManim 3D camera at the drawable's resolution whose frame is
    /// this viewport, over `background`. It takes no multisampling: true
    /// dots carry their own analytic coverage.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] when FrankenManim refuses the frame
    /// or camera.
    pub fn camera(&self, background: LinearRgba) -> Result<ThreeDCamera, CoreError> {
        let refused = |error: fmn_render::CameraError| CoreError::NumericalInvalid {
            quantity: "viewport camera",
            detail: error.to_string(),
        };
        let frame =
            CameraFrame::new(self.frame_shape(), [0.0; 3], DEFAULT_FOVY, "zxz").map_err(refused)?;
        ThreeDCamera::new(CameraConfig {
            resolution: (self.drawable[0], self.drawable[1]),
            background,
            // Every scene point is an image in the window, within half its
            // diagonal of the center.
            max_allowable_norm: self.extent[0].hypot(self.extent[1]),
            samples: 0,
            frame,
            ..CameraConfig::default()
        })
        .map_err(refused)
    }

    /// A periodic grid of `cells` (columns, rows) covering the torus once,
    /// placed in the window as a smoothly shaded mesh: vertices on cell
    /// corners, each coloured by `corner([column, row])` for corner indices
    /// in `[0, columns) × [0, rows)` (corner `columns` is corner 0).
    ///
    /// Where the window starts inside a cell, the grid is cut there: the
    /// window's two edges get vertices whose colours interpolate the cut
    /// cell's corners linearly, as the shading along that edge would. At
    /// [`Viewport::fit`] no cell is cut, and the grid is the corner lattice
    /// at `L · k / n`.
    ///
    /// Returns the vertices, rows top to bottom, and two triangles per cell.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for an empty grid;
    /// [`CoreError::AllocationFailed`] naming `buffers` (vertices, indices)
    /// when either cannot be reserved.
    pub fn periodic_grid(
        &self,
        cells: [usize; 2],
        buffers: [&'static str; 2],
        mut corner: impl FnMut([usize; 2]) -> LinearRgba,
    ) -> Result<(Vec<SurfaceVertex>, Vec<u32>), CoreError> {
        if cells.contains(&0) {
            return Err(CoreError::NumericalInvalid {
                quantity: "periodic grid",
                detail: format!("{cells:?} cells must be at least one each way"),
            });
        }
        let start = self.window_start();
        let columns = grid_lines(self.extent[0], start[0], cells[0]);
        let rows = grid_lines(self.extent[1], start[1], cells[1]);
        let count = columns.len() * rows.len();
        let mut vertices = Vec::new();
        vertices
            .try_reserve_exact(count)
            .map_err(|_| allocation(buffers[0], count, size_of::<SurfaceVertex>()))?;
        for row in &rows {
            for column in &columns {
                let color = blend(column, row, &mut corner);
                vertices.push(SurfaceVertex::colored(
                    [
                        column.offset - self.extent[0] / 2.0,
                        self.extent[1] / 2.0 - row.offset,
                        0.0,
                    ],
                    [0.0, 0.0, 1.0],
                    color,
                ));
            }
        }
        let quads = (columns.len() - 1) * (rows.len() - 1);
        let mut indices = Vec::new();
        indices
            .try_reserve_exact(quads * 6)
            .map_err(|_| allocation(buffers[1], quads * 6, size_of::<u32>()))?;
        let stride = columns.len();
        for row in 0..rows.len() - 1 {
            for column in 0..stride - 1 {
                let at = |c: usize, r: usize| u32::try_from(r * stride + c).unwrap_or(u32::MAX);
                let (a, b, c, d) = (
                    at(column, row),
                    at(column + 1, row),
                    at(column + 1, row + 1),
                    at(column, row + 1),
                );
                indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
        Ok((vertices, indices))
    }
}

/// `value` wrapped into `[0, length)`.
fn wrap(value: f64, length: f64) -> f64 {
    let wrapped = value.rem_euclid(length);
    // A tiny negative value wraps to `length` itself in floating point.
    if wrapped >= length { 0.0 } else { wrapped }
}

/// One vertex line of a periodic grid along an axis: its offset into the
/// window and the corner colour it takes, `(1 − weight) · from + weight ·
/// to`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Line {
    offset: f64,
    from: usize,
    to: usize,
    weight: f64,
}

/// The vertex lines of `count` cells of a periodic axis of `length` in the
/// window from `start`.
fn grid_lines(length: f64, start: f64, count: usize) -> Vec<Line> {
    let n = count as f64;
    let boundary = |k: i64| length * k as f64 / n;
    let index = |k: i64| {
        // `count` is a cell count far below `i64::MAX`.
        usize::try_from(k.rem_euclid(count as i64)).unwrap_or(0)
    };
    // The cell the window starts in.
    let first = (start / length * n).floor();
    let first = if first.is_finite() { first as i64 } else { 0 };
    let mut lines = Vec::with_capacity(count + 2);
    if boundary(first) == start {
        // Aligned: the corner lattice itself, `count + 1` lines.
        for k in first..=first + count as i64 {
            let corner = index(k);
            lines.push(Line {
                offset: boundary(k) - start,
                from: corner,
                to: corner,
                weight: 0.0,
            });
        }
        return lines;
    }
    // Cut: the window's edges fall inside cell `first`.
    let low = boundary(first);
    let weight = ((start - low) / (boundary(first + 1) - low)).clamp(0.0, 1.0);
    let edge = |offset: f64| Line {
        offset,
        from: index(first),
        to: index(first + 1),
        weight,
    };
    lines.push(edge(0.0));
    for k in first + 1..=first + count as i64 {
        let corner = index(k);
        lines.push(Line {
            offset: (boundary(k) - start).clamp(0.0, length),
            from: corner,
            to: corner,
            weight: 0.0,
        });
    }
    lines.push(edge(length));
    lines
}

/// The colour at a vertex on `column` and `row`: the corner itself when
/// neither line is cut, otherwise the bilinear blend of the cut cell's
/// corners.
fn blend(
    column: &Line,
    row: &Line,
    corner: &mut impl FnMut([usize; 2]) -> LinearRgba,
) -> LinearRgba {
    if column.weight == 0.0 && row.weight == 0.0 {
        return corner([column.from, row.from]);
    }
    let mut sum = [0.0; 4];
    for (x, wx) in [
        (column.from, 1.0 - column.weight),
        (column.to, column.weight),
    ] {
        for (y, wy) in [(row.from, 1.0 - row.weight), (row.to, row.weight)] {
            let weight = wx * wy;
            if weight == 0.0 {
                continue;
            }
            let color = corner([x, y]);
            sum[0] += weight * color.r;
            sum[1] += weight * color.g;
            sum[2] += weight * color.b;
            sum[3] += weight * color.a;
        }
    }
    LinearRgba {
        r: sum[0],
        g: sum[1],
        b: sum[2],
        a: sum[3],
    }
}

fn allocation(buffer: &'static str, count: usize, each: usize) -> CoreError {
    CoreError::AllocationFailed {
        buffer,
        bytes: u64::try_from(count.saturating_mul(each)).unwrap_or(u64::MAX),
    }
}
