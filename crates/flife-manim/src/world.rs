//! The live world as a FrankenManim view: one viewport, one camera, one
//! particle cloud.

use flife_core::CoreError;
use flife_render::{Epoch, Prior};
use fmn_core::color::{LinearRgba, Srgb};
use fmn_render::{ThreeDCamera, ThreeDJob, Tiling};

use crate::{ParticleCloud, Viewport};

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
        ThreeDJob::new(self.camera.camera(), self.cloud.draws(), Tiling::default()).map_err(
            |error| CoreError::NumericalInvalid {
                quantity: "world view frame",
                detail: error.to_string(),
            },
        )
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

    /// Scene objects in the view: the one particle cloud, however many
    /// organisms it draws.
    #[must_use]
    pub const fn scene_objects(&self) -> usize {
        1
    }
}
