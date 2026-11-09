//! The exact mapping from world units to the drawable and back (plan §13.6).
//!
//! Four coordinate spaces meet at the window:
//!
//! - **World**: `[0, width) × [0, height)` in world units, periodic on both
//!   axes, y downward (the field's row order).
//! - **Scene**: FrankenManim's camera space, world units centered on the
//!   origin with y upward: `(x − width/2, height/2 − y, 0)`.
//! - **Drawable**: continuous pixels from the drawable's top-left corner.
//!   Pixel `(i, j)` covers `[i, i + 1) × [j, j + 1)`.
//! - **Logical**: window points from the content area's top-left corner,
//!   which the native host reports. A logical point maps to the drawable by
//!   the ratio of drawable pixels to logical points on each axis.
//!
//! A [`Viewport`] fits the whole torus into the drawable once, preserving
//! aspect and centering it, so the unused margin is letterbox. The camera it
//! builds has exactly that frame, so FrankenManim projects a scene point at
//! `z = 0` to [`Viewport::world_to_drawable`] of its world point. Pan and
//! zoom, where the view wraps across the seam, are FL-020's.

use flife_core::CoreError;
use fmn_core::color::LinearRgba;
use fmn_render::{CameraConfig, CameraFrame, DEFAULT_FOVY, ThreeDCamera};

/// The whole torus, fitted into a drawable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    extent: [f64; 2],
    drawable: [u32; 2],
    scale: f64,
    origin: [f64; 2],
}

impl Viewport {
    /// Fit a world of `extent` world units into a drawable of `drawable`
    /// pixels.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for an extent that is not finite and
    /// positive, or an empty drawable.
    pub fn fit(extent: [f64; 2], drawable: [u32; 2]) -> Result<Self, CoreError> {
        if !extent.iter().all(|length| length.is_finite() && *length > 0.0) {
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
        let scale = (pixels[0] / extent[0]).min(pixels[1] / extent[1]);
        let origin = [
            (pixels[0] - extent[0] * scale) / 2.0,
            (pixels[1] - extent[1] * scale) / 2.0,
        ];
        Ok(Self {
            extent,
            drawable,
            scale,
            origin,
        })
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

    /// Drawable pixels per world unit, the same on both axes.
    #[must_use]
    pub const fn scale(&self) -> f64 {
        self.scale
    }

    /// The drawable position of the world origin: the letterbox margin.
    #[must_use]
    pub const fn origin(&self) -> [f64; 2] {
        self.origin
    }

    /// The drawable position of a world point.
    #[must_use]
    pub fn world_to_drawable(&self, point: [f64; 2]) -> [f64; 2] {
        [
            self.origin[0] + point[0] * self.scale,
            self.origin[1] + point[1] * self.scale,
        ]
    }

    /// The world point at a drawable position, or `None` in the letterbox
    /// margin. The world is half-open, so its far edges are margin.
    #[must_use]
    pub fn drawable_to_world(&self, position: [f64; 2]) -> Option<[f64; 2]> {
        let world = [
            (position[0] - self.origin[0]) / self.scale,
            (position[1] - self.origin[1]) / self.scale,
        ];
        let inside = world
            .iter()
            .zip(self.extent)
            .all(|(coordinate, length)| *coordinate >= 0.0 && *coordinate < length);
        inside.then_some(world)
    }

    /// The world point under a logical window point, given the content
    /// area's logical size; `None` in the margin or while the content area
    /// is empty.
    #[must_use]
    pub fn logical_to_world(&self, point: [f64; 2], logical_size: [f64; 2]) -> Option<[f64; 2]> {
        if !logical_size
            .iter()
            .all(|length| length.is_finite() && *length > 0.0)
        {
            return None;
        }
        self.drawable_to_world([
            point[0] * f64::from(self.drawable[0]) / logical_size[0],
            point[1] * f64::from(self.drawable[1]) / logical_size[1],
        ])
    }

    /// The scene point FrankenManim draws for a world point.
    #[must_use]
    pub fn scene_point(&self, point: [f64; 2]) -> [f64; 3] {
        [
            point[0] - self.extent[0] / 2.0,
            self.extent[1] / 2.0 - point[1],
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
            // Every scene point of the world is within half its diagonal.
            max_allowable_norm: self.extent[0].hypot(self.extent[1]),
            samples: 0,
            frame,
            ..CameraConfig::default()
        })
        .map_err(refused)
    }
}
