//! The live world as a FrankenManim view: one viewport, one camera, the
//! field, one particle cloud and its density layer, drawn in one [`Style`].

use flife_core::CoreError;
use flife_render::{Epoch, Prior};
use fmn_core::color::{LinearRgba, Srgb};
use fmn_render::{ThreeDCamera, ThreeDDraw, ThreeDJob, Tiling};

use crate::mapping::{Look, Particles, Style};
use crate::{DensityLayer, FieldLayer, ParticleCloud, Viewport};

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
    density: DensityLayer,
    style: Style,
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
            density: DensityLayer::new(&viewport),
            viewport,
            cloud: ParticleCloud::with_capacity(capacity)?,
            style: Style::default(),
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
    /// keeping the shown epoch and the camera's center and zoom. Returns
    /// whether the size changed.
    ///
    /// # Errors
    /// As [`Viewport::resized`] and [`Viewport::camera`]; the view is
    /// unchanged on error.
    pub fn resize(&mut self, drawable: [u32; 2]) -> Result<bool, CoreError> {
        if drawable == self.viewport.drawable() {
            return Ok(false);
        }
        let viewport = self.viewport.resized(drawable)?;
        self.adopt(viewport)?;
        Ok(true)
    }

    /// Look at `viewport` (the same world): a pan, a zoom or a reset. Place
    /// the shown epoch again ([`WorldView::place`]) to draw it there.
    /// Returns whether the view changed.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for a viewport of another world or
    /// drawable; as [`Viewport::camera`]. The view is unchanged on error.
    pub fn set_view(&mut self, viewport: Viewport) -> Result<bool, CoreError> {
        if viewport.extent().map(f64::to_bits) != self.viewport.extent().map(f64::to_bits)
            || viewport.drawable() != self.viewport.drawable()
        {
            return Err(CoreError::LayoutMismatch {
                buffer: "world view viewport",
                detail: format!(
                    "a {:?} world at {:?} pixels is not this view's {:?} at {:?}",
                    viewport.extent(),
                    viewport.drawable(),
                    self.viewport.extent(),
                    self.viewport.drawable()
                ),
            });
        }
        if viewport == self.viewport {
            return Ok(false);
        }
        self.adopt(viewport)?;
        Ok(true)
    }

    fn adopt(&mut self, viewport: Viewport) -> Result<(), CoreError> {
        let camera = viewport.camera(self.background)?;
        self.viewport = viewport;
        self.camera = camera;
        self.cloud.refit(&self.viewport);
        self.density.refit(&self.viewport);
        self.field.refit(&self.viewport);
        Ok(())
    }

    /// The style frames are drawn in.
    #[must_use]
    pub const fn style(&self) -> Style {
        self.style
    }

    /// Draw from the next [`WorldView::show`] in `style`. Show an epoch
    /// again to see the change at once.
    pub const fn set_style(&mut self, style: Style) {
        self.style = style;
    }

    /// Lower `epoch` in the current style and remember it as shown. The
    /// field is mapped even while hidden, so its display scale keeps
    /// following the world.
    ///
    /// # Errors
    /// As [`ParticleCloud::update_styled`], [`DensityLayer::update`] and
    /// [`FieldLayer::update_look`].
    pub fn show(&mut self, epoch: &Epoch) -> Result<(), CoreError> {
        let style = self.style;
        self.cloud.update_styled(epoch, &self.viewport, &style)?;
        if style.particles == Particles::Density {
            self.density.update(epoch, &style)?;
        }
        self.field.update_look(epoch.field(), style.look)?;
        self.shown = Some(Prior {
            id: epoch.id(),
            tick: epoch.tick(),
        });
        Ok(())
    }

    /// Draw `epoch` again at the current viewport and style without moving
    /// the field's or density's display scale: what a pan or zoom redraws
    /// for the epoch already shown. The density layer re-places the epoch
    /// it last binned.
    ///
    /// # Errors
    /// As [`WorldView::show`].
    pub fn place(&mut self, epoch: &Epoch) -> Result<(), CoreError> {
        let style = self.style;
        self.cloud.update_styled(epoch, &self.viewport, &style)?;
        if style.particles == Particles::Density {
            self.density.place(&style)?;
        }
        self.field.place(epoch.field(), style.look)?;
        self.shown = Some(Prior {
            id: epoch.id(),
            tick: epoch.tick(),
        });
        Ok(())
    }

    /// The current frame's draws in painter order: the field, then the
    /// density layer, or every halo and then every dot. Raw density is
    /// opaque, so the field is left out under it.
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when the list cannot be reserved.
    pub fn draws(&self) -> Result<Vec<ThreeDDraw<'_>>, CoreError> {
        let style = self.style;
        let density = style.particles == Particles::Density;
        let halos = self.cloud.halos();
        let dots = self.cloud.draws();
        let mut draws: Vec<ThreeDDraw<'_>> = Vec::new();
        draws
            .try_reserve_exact(2 + halos.len() + dots.len())
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "world view frame draws",
                bytes: u64::try_from(
                    (2 + halos.len() + dots.len()).saturating_mul(size_of::<ThreeDDraw<'_>>()),
                )
                .unwrap_or(u64::MAX),
            })?;
        if style.field && !(density && style.look == Look::Raw) {
            draws.extend(self.field.draw());
        }
        if density {
            draws.extend(self.density.draw());
        } else {
            draws.extend_from_slice(halos);
            draws.extend_from_slice(dots);
        }
        Ok(draws)
    }

    /// The 3D job for the current frame, over [`WorldView::draws`].
    ///
    /// # Errors
    /// As [`WorldView::draws`]; [`CoreError::NumericalInvalid`] when
    /// FrankenManim refuses to compile the draws.
    pub fn job(&self) -> Result<ThreeDJob<'_>, CoreError> {
        let draws = self.draws()?;
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

    /// Scene objects in the view: the field layer, the one particle cloud
    /// however many organisms it draws, and its density layer.
    #[must_use]
    pub const fn scene_objects(&self) -> usize {
        3
    }

    /// The density layer.
    #[must_use]
    pub const fn density(&self) -> &DensityLayer {
        &self.density
    }

    /// The field layer.
    #[must_use]
    pub const fn field(&self) -> &FieldLayer {
        &self.field
    }
}
