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
//! A viewport also carries the drawable's pixels per point, 1 unless a host
//! draws at a screen's density (2 on a typical high-DPI screen). It changes
//! no mapping; the sizes that should look the same on any screen, such as
//! the smallest dot and the selection ring, are given in points and scaled
//! by it.
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
use fmn_render::{
    CameraConfig, CameraFrame, DEFAULT_FOVY, SurfaceMesh, SurfaceVertex, ThreeDCamera,
};

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
    pixels_per_point: f64,
}

impl Viewport {
    /// Fit a world of `extent` world units into a drawable of `drawable`
    /// pixels: centered, zoom 1, one pixel per point.
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
            1.0,
        ))
    }

    fn placed(
        extent: [f64; 2],
        drawable: [u32; 2],
        fit_scale: f64,
        center: [f64; 2],
        zoom: f64,
        pixels_per_point: f64,
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
            pixels_per_point,
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
            self.pixels_per_point,
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
            self.pixels_per_point,
        ))
    }

    /// The whole torus again, centered at zoom 1, in this drawable and at
    /// this density.
    #[must_use]
    pub fn refit(&self) -> Self {
        Self::placed(
            self.extent,
            self.drawable,
            self.fit_scale,
            self.extent.map(|length| length / 2.0),
            1.0,
            self.pixels_per_point,
        )
    }

    /// The same view on a drawable of `pixels_per_point` pixels per point.
    /// The mapping is unchanged.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a density that is not finite and
    /// positive.
    pub fn with_pixels_per_point(&self, pixels_per_point: f64) -> Result<Self, CoreError> {
        if !(pixels_per_point.is_finite() && pixels_per_point > 0.0) {
            return Err(CoreError::NumericalInvalid {
                quantity: "viewport pixels per point",
                detail: format!("{pixels_per_point} must be finite and positive"),
            });
        }
        Ok(Self {
            pixels_per_point,
            ..*self
        })
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

    /// Drawable pixels per point on the screen: 1 unless a host draws at the
    /// screen's density.
    #[must_use]
    pub const fn pixels_per_point(&self) -> f64 {
        self.pixels_per_point
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

    /// The quad that draws a periodic texture of `cells` (columns, rows)
    /// over the torus once (FL-033): the window's image, `[lo, lo + L]` on
    /// each axis, with texture coordinates that put texel `k` of an axis on
    /// the torus's corner at `L · k / n`.
    ///
    /// A texel is sampled at its centre: coordinate `u` on an axis of `n`
    /// texels lands on texel `u · n − ½`. The quad's coordinates are
    /// `lo / L + ½ / n` at the window's start and one more at its end, so
    /// the corner at world `L · k / n` lands on texel `k` exactly, and the
    /// sampler interpolates bilinearly between corners. The texture
    /// repeats, so a window that starts inside a cell, or crosses the seam,
    /// shows the cells it cuts without a seam. Texture rows run top to
    /// bottom, the world's y order.
    ///
    /// Returns two triangles over four textured vertices facing the camera,
    /// at full opacity.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for an empty axis, or if FrankenManim
    /// refuses the mesh.
    pub fn periodic_quad(&self, cells: [u32; 2]) -> Result<SurfaceMesh, CoreError> {
        let refused = |detail: String| CoreError::NumericalInvalid {
            quantity: "periodic quad",
            detail,
        };
        if cells.contains(&0) {
            return Err(refused(format!(
                "{cells:?} cells must be at least one each way"
            )));
        }
        let start = self.window_start();
        let [u, v] =
            [0, 1].map(|axis| start[axis] / self.extent[axis] + 0.5 / f64::from(cells[axis]));
        let [x, y] = self.extent.map(|extent| extent / 2.0);
        let vertex = |point: [f64; 2], uv: [f64; 2]| {
            SurfaceVertex::textured([point[0], point[1], 0.0], [0.0, 0.0, 1.0], uv, 1.0)
        };
        let vertices = vec![
            vertex([-x, y], [u, v]),
            vertex([x, y], [u + 1.0, v]),
            vertex([-x, -y], [u, v + 1.0]),
            vertex([x, -y], [u + 1.0, v + 1.0]),
        ];
        SurfaceMesh::new(vertices, vec![0, 1, 3, 0, 3, 2])
            .map_err(|error| refused(error.to_string()))
    }
}

/// `value` wrapped into `[0, length)`.
fn wrap(value: f64, length: f64) -> f64 {
    let wrapped = value.rem_euclid(length);
    // A tiny negative value wraps to `length` itself in floating point.
    if wrapped >= length { 0.0 } else { wrapped }
}
