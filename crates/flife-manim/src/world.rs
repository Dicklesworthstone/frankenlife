//! The live world as a FrankenManim view: one viewport, one camera, one
//! particle cloud.

use flife_core::CoreError;
use flife_render::{Epoch, Prior};
use fmn_core::color::{LinearRgba, Srgb};
use fmn_render::{ThreeDCamera, ThreeDDraw, ThreeDJob, Tiling};

use crate::{FieldLayer, ParticleCloud, Viewport};

/// A `#rrggbb` colour, as `[presentation] background` spells it.
#[must_use]
pub fn hex_rgb(text: &str) -> Option<[u8; 3]> {
    let hex = text.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

/// The world as FrankenManim draws it.
///
/// A frame is [`WorldView::job`]: one 3D job over the cloud's draws, with
/// the camera fitted to the current drawable. The view holds the epoch it
/// last showed as a [`Prior`], so a host can tell what is on screen and
/// request only newer epochs.
#[derive(Debug)]
pub struct WorldView {
    background: LinearRgba,
    viewport: Viewport,
    camera: ThreeDCamera,
    field: FieldLayer,
    cloud: ParticleCloud,
    shown: Option<Prior>,
}

impl WorldView {
    /// A view of a world of `extent` world units with up to `capacity`
    /// organisms, drawn at `drawable` pixels over the sRGB `background`.
    ///
    /// # Errors
    /// As [`Viewport::fit`], [`Viewport::camera`] and
    /// [`ParticleCloud::with_capacity`].
    pub fn new(
        extent: [f64; 2],
        capacity: usize,
        drawable: [u32; 2],
        background: [u8; 3],
    ) -> Result<Self, CoreError> {
        let viewport = Viewport::fit(extent, drawable)?;
        let [r, g, b] = background;
        let background = Srgb::from_rgb8(r, g, b).to_linear(1.0);
        Ok(Self {
            camera: viewport.camera(background)?,
            background,
            field: FieldLayer::new(&viewport),
            viewport,
            cloud: ParticleCloud::with_capacity(capacity)?,
            shown: None,
        })
    }

    /// A view sized for `epoch`'s world and population ceiling.
    ///
    /// # Errors
    /// As [`WorldView::new`].
    pub fn for_epoch(
        epoch: &Epoch,
        drawable: [u32; 2],
        background: [u8; 3],
    ) -> Result<Self, CoreError> {
        Self::new(epoch.extent(), epoch.capacity(), drawable, background)
    }

    /// Refit the viewport, camera and dot radius to a new drawable size,
    /// keeping the shown epoch. Returns whether the size changed.
    ///
    /// # Errors
    /// As [`Viewport::fit`] and [`Viewport::camera`]; the view is unchanged
    /// on error.
    pub fn resize(&mut self, drawable: [u32; 2]) -> Result<bool, CoreError> {
        if drawable == self.viewport.drawable() {
            return Ok(false);
        }
        let viewport = Viewport::fit(self.viewport.extent(), drawable)?;
        let camera = viewport.camera(self.background)?;
        self.viewport = viewport;
        self.camera = camera;
        self.cloud.refit(&self.viewport);
        Ok(true)
    }

    /// Lower `epoch` into the cloud and remember it as shown.
    ///
    /// # Errors
    /// As [`ParticleCloud::update`]; what is shown is unchanged on error.
    pub fn show(&mut self, epoch: &Epoch) -> Result<(), CoreError> {
        self.cloud.update(epoch, &self.viewport)?;
        self.field.update(epoch.field())?;
        self.shown = Some(Prior {
            id: epoch.id(),
            tick: epoch.tick(),
        });
        Ok(())
    }

    /// The 3D job for the current frame.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] when FrankenManim refuses to compile
    /// the draws.
    pub fn job(&self) -> Result<ThreeDJob<'_>, CoreError> {
        // Painter order: the field, every halo, then every dot.
        let halos = self.cloud.halos();
        let dots = self.cloud.draws();
        let mut draws: Vec<ThreeDDraw<'_>> = Vec::new();
        draws
            .try_reserve_exact(1 + halos.len() + dots.len())
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "world view frame draws",
                bytes: u64::try_from(
                    (1 + halos.len() + dots.len()).saturating_mul(size_of::<ThreeDDraw<'_>>()),
                )
                .unwrap_or(u64::MAX),
            })?;
        draws.extend(self.field.draw());
        draws.extend_from_slice(halos);
        draws.extend_from_slice(dots);
        ThreeDJob::new(self.camera.camera(), &draws, Tiling::default()).map_err(|error| {
            CoreError::NumericalInvalid {
                quantity: "world view frame",
                detail: error.to_string(),
            }
        })
    }

    /// Render the current frame on the CPU with up to `threads` threads, as
    /// tight top-row-first sRGB RGBA8: FrankenManim's reference 3D route,
    /// for offline frames and video.
    ///
    /// # Errors
    /// As [`WorldView::job`]; [`CoreError::NumericalInvalid`] when
    /// FrankenManim cannot render or convert the frame.
    pub fn render_rgba8(&self, threads: usize) -> Result<Vec<u8>, CoreError> {
        let refused = |error: String| CoreError::NumericalInvalid {
            quantity: "world view offline frame",
            detail: error,
        };
        let frame = self
            .job()?
            .render(threads.max(1))
            .map_err(|error| refused(error.to_string()))?;
        let [width, height] = self.viewport.drawable();
        let bytes = usize::try_from(u64::from(width) * u64::from(height) * 4).map_err(|_| {
            CoreError::Overflow {
                quantity: "offline frame bytes",
            }
        })?;
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(bytes)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "offline frame",
                bytes: u64::try_from(bytes).unwrap_or(u64::MAX),
            })?;
        rgba.resize(bytes, 0);
        fmn_frame::convert::rgba16f_to_rgba8_slice(&frame, &mut rgba)
            .map_err(|error| refused(error.to_string()))?;
        Ok(rgba)
    }

    /// The epoch currently shown.
    #[must_use]
    pub const fn shown(&self) -> Option<Prior> {
        self.shown
    }

    /// The viewport.
    #[must_use]
    pub const fn viewport(&self) -> &Viewport {
        &self.viewport
    }

    /// The camera.
    #[must_use]
    pub const fn camera(&self) -> &ThreeDCamera {
        &self.camera
    }

    /// The particle cloud.
    #[must_use]
    pub const fn cloud(&self) -> &ParticleCloud {
        &self.cloud
    }

    /// Scene objects in the view: the field layer and the one particle
    /// cloud, however many organisms it draws.
    #[must_use]
    pub const fn scene_objects(&self) -> usize {
        2
    }

    /// The field layer.
    #[must_use]
    pub const fn field(&self) -> &FieldLayer {
        &self.field
    }
}
