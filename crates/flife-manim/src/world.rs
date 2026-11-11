//! The live world as a FrankenManim view: one viewport, one camera, the
//! field, one particle cloud and its density layer, drawn in one [`Style`].

use flife_core::CoreError;
use flife_core::ids::OrganismId;
use flife_core::memory::{bytes_for, capacity_bytes, total};
use flife_render::{Epoch, Gap, Interpolation, Prior, interpolate};
use fmn_core::color::{LinearRgba, PremulRgba, Srgb};
use fmn_render::{
    BLOOM_BAND_ROWS, BLOOM_SCRATCH_BYTES_PER_PIXEL, BLOOM_SIGMA_MAX_PIXELS, Bloom, SurfaceDraw,
    SurfaceMesh, SurfaceVertex, ThreeDCamera, ThreeDDraw, ThreeDJob, ThreeDPreparationLimits,
    Tiling,
};

use crate::cloud::check_min_radius_points;
use crate::mapping::{Look, Particles, Style};
use crate::{
    DensityLayer, FieldLayer, HUD_MAX_TRIANGLES, HUD_MAX_VERTICES, HudText, ParticleCloud,
    SELECTION_RING_SEGMENTS, Viewport, density_bins, dot_radius_at_least, hud_glyphs, hud_plate,
    pick, selection_ring_around,
};

/// The most FrankenManim's preparation of a frame charges for one true dot
/// (`ThreeDJob::preparation_bytes`, FL-043): its draw row, its two raster
/// triangles and its places in the tile bins. Measured on the pinned
/// FrankenManim with 1,024 and 16,384 organisms at 640 x 360 to 3840 x 2160:
/// at most 1,197 bytes. A dot's radius has a ceiling in points, so a zoom
/// does not widen it past a few tiles.
pub const PREPARATION_BYTES_PER_DOT: u64 = 1_536;

/// The most FrankenManim's preparation of a frame charges for one surface
/// triangle, its share of the mesh's vertices and its places in the
/// triangle bins included (FL-043). Measured on FrankenManim at `721b04b9`
/// over vertex grids of 60 x 60 to 360 x 360 cells: at most 476 bytes. The
/// field and density layers are now a quad each (FL-033), so this charges
/// their four triangles and the selection ring's.
pub const PREPARATION_BYTES_PER_TRIANGLE: u64 = 640;

/// FrankenManim's preparation apart from its rows: the job, and the tile
/// tables of its bins.
pub const PREPARATION_BYTES_FIXED: u64 = 1 << 20;

/// The heap FrankenManim's job holds for each draw beyond its own
/// preparation accounting, which `ThreeDJob::preparation_bytes` and the
/// preparation limit leave out (FL-033). Measured on FrankenManim at
/// `8ccb702e`, CPU jobs of 1,024 and 16,384 organisms at 640 x 360 to
/// 3840 x 2160: 865 to 872 bytes a dot, and 1,180 for each of the field's
/// and the density's quads. Until FL-033 the grids' triangle allowance
/// covered it.
pub const JOB_BYTES_PER_DRAW: u64 = 1_024;

/// The bloom's blur width in logical points (FL-019, plan §2.3): the
/// standard deviation of FrankenManim's binomial blur, at the screen's
/// pixel density, up to FrankenManim's widest. A presentation choice: wide
/// enough to read as light around a dot, and the mix is convex, so a
/// boundary keeps at least `1 - 2 x bloom_strength` of its contrast
/// whatever the width.
pub const BLOOM_SIGMA_POINTS: f64 = 1.5;

/// The reach of FrankenManim's widest bloom, in pixels each way: three
/// deviations of `BLOOM_SIGMA_MAX_PIXELS` (32), whatever the pixel density.
pub const BLOOM_WIDEST_REACH_PIXELS: u32 = 96;

/// The bloom of `strength` at `viewport`'s pixel density.
fn bloom_at(strength: f64, viewport: &Viewport) -> Result<Bloom, CoreError> {
    let sigma = (BLOOM_SIGMA_POINTS * viewport.pixels_per_point()).min(BLOOM_SIGMA_MAX_PIXELS);
    Bloom::new(strength, sigma).map_err(|error| CoreError::NumericalInvalid {
        quantity: "presentation bloom",
        detail: error.to_string(),
    })
}

/// The finest edge sampling FrankenManim's 3D route takes per pixel: a
/// 4 x 4 grid.
const MAXIMUM_SAMPLES_PER_PIXEL: u64 = 16;

/// The overlay's draws: its plate and its glyphs (FL-024).
const OVERLAY_DRAWS: usize = 2;

/// The bytes of a mesh of `vertices` and `triangles`.
fn mesh_bytes((vertices, triangles): (u64, u64)) -> u64 {
    total([
        vertices.saturating_mul(bytes_for::<SurfaceVertex>(1)),
        triangles.saturating_mul(bytes_for::<u32>(3)),
    ])
}

/// A periodic quad's vertices and triangles ([`Viewport::periodic_quad`]).
const QUAD: (u64, u64) = (4, 2);

/// The bytes of a layer texture of `cells`: premultiplied linear `f32`
/// RGBA per cell (FL-033).
fn texel_bytes_of(cells: [u32; 2]) -> u64 {
    let [columns, rows] = cells.map(u64::from);
    columns
        .saturating_mul(rows)
        .saturating_mul(bytes_for::<[f32; 4]>(1))
}

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
///
/// It also keeps the shown epoch's identities and where each is drawn, so
/// a pick reads what is on screen, and at most one selected organism, held
/// by identity: every newly shown epoch revalidates it, and an organism the
/// epoch no longer has is deselected, whatever occupies its old slot.
///
/// The display can lag one committed state behind (FL-020, plan §13.5):
/// [`WorldView::show_between`] draws the organisms part of the way from the
/// prior epoch to the shown one, matched by identity, and fades out the
/// ones that died in the gap ([`Motion`]).
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
    /// The shown epoch's identities, in identity order.
    ids: Vec<u64>,
    /// Where the shown epoch's rows are drawn and whether they are yet,
    /// row for row with `ids`, and the prior rows fading.
    between: Interpolation,
    motion: Option<Motion>,
    selected: Option<OrganismId>,
    ring: Option<SurfaceMesh>,
    /// The texels of the field last shown; none before the first epoch.
    field_texels: [u32; 2],
    /// The configured bloom's strength; none when it is off (FL-019).
    bloom_strength: Option<f64>,
    /// That bloom at the viewport's pixel density.
    bloom: Option<Bloom>,
    /// Whether the view draws text, and so charges for it (FL-024).
    overlay: bool,
    /// The text drawn, with its meshes at the viewport.
    hud: Option<Hud>,
}

/// Text over the world and its meshes at the view's viewport (FL-024).
#[derive(Debug)]
struct Hud {
    text: HudText,
    plate: SurfaceMesh,
    glyphs: Option<SurfaceMesh>,
}

/// `text`'s plate and glyphs at `viewport`.
fn overlay_meshes(
    viewport: &Viewport,
    text: &HudText,
) -> Result<(SurfaceMesh, Option<SurfaceMesh>), CoreError> {
    Ok((hud_plate(viewport, text)?, hud_glyphs(viewport, text)?))
}

/// Where the display is between two committed epochs (FL-020, plan §13.5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    /// The epoch the display moves from.
    pub prior: Prior,
    /// How far it has come, in `[0, 1)`: 0 is `prior`, 1 the shown epoch
    /// (where it no longer moves, so there is no motion).
    pub fraction: f64,
    /// The gap between the two. An ambiguous one snapped every continuing
    /// organism to the shown epoch.
    pub gap: Gap,
    /// The organisms of `prior` that died in the gap, drawn fading.
    pub fading: usize,
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
        let refused = |buffer, each: usize| CoreError::AllocationFailed {
            buffer,
            bytes: u64::try_from(capacity.saturating_mul(each)).unwrap_or(u64::MAX),
        };
        let mut ids = Vec::new();
        ids.try_reserve_exact(capacity)
            .map_err(|_| refused("world view identities", size_of::<u64>()))?;
        let mut between = Interpolation::default();
        between
            .positions
            .try_reserve_exact(capacity)
            .map_err(|_| refused("world view drawn positions", size_of::<[f64; 2]>()))?;
        between
            .shown
            .try_reserve_exact(capacity)
            .map_err(|_| refused("world view visibilities", size_of::<bool>()))?;
        between
            .fading
            .try_reserve_exact(capacity)
            .map_err(|_| refused("world view fading rows", size_of::<usize>()))?;
        Ok(Self {
            camera: viewport.camera(background)?,
            background,
            field: FieldLayer::new(&viewport),
            density: DensityLayer::new(&viewport),
            viewport,
            cloud: ParticleCloud::with_capacity(capacity)?,
            style: Style::default(),
            shown: None,
            ids,
            between,
            motion: None,
            selected: None,
            ring: None,
            field_texels: [0; 2],
            bloom_strength: None,
            bloom: None,
            overlay: false,
            hud: None,
        })
    }

    /// A view sized for `epoch`'s world, field and population ceiling.
    ///
    /// # Errors
    /// As [`WorldView::new`].
    pub fn for_epoch(
        epoch: &Epoch,
        drawable: [u32; 2],
        background: [u8; 3],
    ) -> Result<Self, CoreError> {
        let mut view = Self::new(epoch.extent(), epoch.capacity(), drawable, background)?;
        view.field_texels = epoch.field().texels();
        Ok(view)
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

    /// Draw for a screen of `pixels_per_point` drawable pixels per point,
    /// keeping the view: the dot's smallest and largest radius and the
    /// selection ring keep their size on screen at once, and the density
    /// bins from the next shown epoch. Returns whether the density changed.
    ///
    /// # Errors
    /// As [`Viewport::with_pixels_per_point`] and [`Viewport::camera`]; the
    /// view is unchanged on error.
    pub fn set_pixels_per_point(&mut self, pixels_per_point: f64) -> Result<bool, CoreError> {
        if pixels_per_point.to_bits() == self.viewport.pixels_per_point().to_bits() {
            return Ok(false);
        }
        let viewport = self.viewport.with_pixels_per_point(pixels_per_point)?;
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
        let bloom = self
            .bloom_strength
            .map(|strength| bloom_at(strength, &viewport))
            .transpose()?;
        let ring = match self.selected_position() {
            Some(position) => Some(selection_ring_around(
                &viewport,
                position,
                dot_radius_at_least(&viewport, self.cloud.min_radius_points()),
            )?),
            None => None,
        };
        let hud = match &self.hud {
            Some(hud) => Some(overlay_meshes(&viewport, &hud.text)?),
            None => None,
        };
        self.viewport = viewport;
        self.camera = camera;
        self.ring = ring;
        if let (Some(hud), Some((plate, glyphs))) = (self.hud.as_mut(), hud) {
            hud.plate = plate;
            hud.glyphs = glyphs;
        }
        self.bloom = bloom;
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

    /// Bloom frames with `strength` (`[presentation] bloom_enabled` and
    /// `bloom_strength`, FL-019), or not at all for `None`: FrankenManim's
    /// controlled bloom, [`BLOOM_SIGMA_POINTS`] wide at the viewport's pixel
    /// density. Only glowing frames bloom: the raw look is the diagnostic
    /// view, with no bloom (plan §2.3).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a strength outside `[0, 1]`;
    /// the view is unchanged on error.
    pub fn set_bloom(&mut self, strength: Option<f64>) -> Result<(), CoreError> {
        self.bloom = strength
            .map(|strength| bloom_at(strength, &self.viewport))
            .transpose()?;
        self.bloom_strength = strength;
        Ok(())
    }

    /// Draw each dot at least `points` in radius on screen, in points like
    /// the window's size (`[presentation] point_radius_pixels`, FL-019): the
    /// fine points of plan §2.3 where a world is too large for its window,
    /// in place of [`DOT_RADIUS_MIN_POINTS`](crate::DOT_RADIUS_MIN_POINTS).
    /// Zoomed in, a dot grows to its world radius and stops at
    /// [`DOT_RADIUS_MAX_POINTS`](crate::DOT_RADIUS_MAX_POINTS), as before. The
    /// shown dots, their halos and the selection ring change at once.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless `points` is finite, positive
    /// and at most [`DOT_RADIUS_MAX_POINTS`](crate::DOT_RADIUS_MAX_POINTS); as
    /// [`selection_ring_around`]. The view is unchanged on error.
    pub fn set_point_radius(&mut self, points: f64) -> Result<(), CoreError> {
        check_min_radius_points(points)?;
        let ring = match self.selected_position() {
            Some(position) => Some(selection_ring_around(
                &self.viewport,
                position,
                dot_radius_at_least(&self.viewport, points),
            )?),
            None => None,
        };
        self.cloud.set_min_radius_points(points, &self.viewport)?;
        self.ring = ring;
        Ok(())
    }

    /// The smallest radius a dot takes on screen, in points.
    #[must_use]
    pub const fn point_radius(&self) -> f64 {
        self.cloud.min_radius_points()
    }

    /// The configured bloom at the viewport's pixel density, whatever the
    /// look.
    #[must_use]
    pub const fn bloom(&self) -> Option<&Bloom> {
        self.bloom.as_ref()
    }

    /// The bloom the current frame applies: the configured one, in the
    /// glow look.
    #[must_use]
    pub const fn frame_bloom(&self) -> Option<&Bloom> {
        match self.style.look {
            Look::Glow => self.bloom.as_ref(),
            Look::Raw => None,
        }
    }

    /// Lower `epoch` in the current style and remember it as shown. The
    /// field is mapped even while hidden, so its display scale keeps
    /// following the world.
    ///
    /// # Errors
    /// As [`ParticleCloud::update_styled`], [`DensityLayer::update`] and
    /// [`FieldLayer::update_look`].
    pub fn show(&mut self, epoch: &Epoch) -> Result<(), CoreError> {
        self.show_between(None, epoch, 1.0).map(|_| ())
    }

    /// Show `current` with the display `fraction` of the way from `prior`
    /// to it (FL-020, plan §13.5): as [`WorldView::show`], but each organism
    /// alive in both is drawn where [`interpolate`] places it, by identity
    /// (by the minimum wrapped displacement when the gap is unambiguous,
    /// snapped to `current` when it is not), an organism born in the gap
    /// appears only at fraction 1, and one that died fades out at its
    /// `prior` position. The field and density are `current`'s.
    ///
    /// Only two committed states of one world, the later after the earlier,
    /// are drawn between. Without a `prior`, at fraction 1, or for a
    /// `prior` that is not an earlier state of `current`'s world (a branch,
    /// a restored checkpoint, another world), `current` is shown at rest.
    /// Returns the motion on screen, if any.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a fraction outside `[0, 1]`;
    /// [`CoreError::BudgetExhausted`] for an epoch above the view's ceiling;
    /// [`CoreError::LayoutMismatch`] for an epoch of another world than the
    /// viewport's; and as [`WorldView::show`]. The organisms are unchanged
    /// on the first three.
    pub fn show_between(
        &mut self,
        prior: Option<&Epoch>,
        current: &Epoch,
        fraction: f64,
    ) -> Result<Option<Motion>, CoreError> {
        let style = self.style;
        let motion = self.lower(prior, current, fraction)?;
        if style.particles == Particles::Density {
            self.density.update(current, &style)?;
        }
        self.field.update_look(current.field(), style.look)?;
        Ok(motion)
    }

    /// Place `current` again with the display `fraction` of the way from
    /// `prior`, as [`WorldView::show_between`] draws it, without moving the
    /// field's or density's display scale: a pan or zoom while the display
    /// moves.
    ///
    /// # Errors
    /// As [`WorldView::show_between`].
    pub fn place_between(
        &mut self,
        prior: Option<&Epoch>,
        current: &Epoch,
        fraction: f64,
    ) -> Result<Option<Motion>, CoreError> {
        let style = self.style;
        let motion = self.lower(prior, current, fraction)?;
        if style.particles == Particles::Density {
            self.density.place(&style)?;
        }
        self.field.place(current.field(), style.look)?;
        Ok(motion)
    }

    /// Move the display to `fraction` of the way from `prior` to the shown
    /// epoch `current`, redrawing only the organisms and the selection
    /// ring: the field and density stay as they are. What a frame redraws
    /// while the display catches up with the shown epoch.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when `current` is not the shown epoch;
    /// as [`WorldView::show_between`].
    pub fn advance(
        &mut self,
        prior: Option<&Epoch>,
        current: &Epoch,
        fraction: f64,
    ) -> Result<Option<Motion>, CoreError> {
        if self.shown.map(|shown| shown.id) != Some(current.id()) {
            return Err(CoreError::StateConflict {
                operation: "advance the world view",
                detail: format!(
                    "epoch {} is not the shown epoch {:?}",
                    current.id(),
                    self.shown.map(|shown| shown.id)
                ),
            });
        }
        self.lower(prior, current, fraction)
    }

    /// Lower the organisms of the display `fraction` of the way from
    /// `prior` to `current`, and remember `current` as shown.
    fn lower(
        &mut self,
        prior: Option<&Epoch>,
        current: &Epoch,
        fraction: f64,
    ) -> Result<Option<Motion>, CoreError> {
        if !(0.0..=1.0).contains(&fraction) {
            return Err(CoreError::NumericalInvalid {
                quantity: "world view display fraction",
                detail: format!("{fraction} is outside [0, 1]"),
            });
        }
        let capacity = self.cloud.capacity();
        for epoch in prior.into_iter().chain([current]) {
            if epoch.len() > capacity {
                return Err(CoreError::BudgetExhausted {
                    budget: "world view rows",
                    required: u64::try_from(epoch.len()).unwrap_or(u64::MAX),
                    available: u64::try_from(capacity).unwrap_or(u64::MAX),
                });
            }
            if epoch.extent().map(f64::to_bits) != self.viewport.extent().map(f64::to_bits) {
                return Err(CoreError::LayoutMismatch {
                    buffer: "world view",
                    detail: format!(
                        "epoch world {:?} is not the viewport's {:?}",
                        epoch.extent(),
                        self.viewport.extent()
                    ),
                });
            }
        }
        // Only an earlier state of the same world is drawn between; the
        // gap check refuses anything else, which is then shown at rest.
        let pair = prior
            .filter(|_| fraction < 1.0)
            .and_then(|prior| Gap::between(prior, current).ok().map(|gap| (prior, gap)));
        // Every buffer was reserved at the ceiling both epochs are within,
        // so neither path allocates.
        match pair {
            Some((prior, _)) => {
                interpolate(prior, current, fraction, &mut self.between)?;
            }
            None => {
                self.between.positions.clear();
                self.between
                    .positions
                    .extend(current.positions().iter().map(|p| p.map(f64::from)));
                self.between.shown.clear();
                self.between.shown.resize(current.len(), true);
                self.between.fading.clear();
                self.between.snapped = false;
            }
        }
        let shown_fraction = if pair.is_some() { fraction } else { 1.0 };
        self.cloud.update_between(
            pair.map(|(prior, _)| prior),
            current,
            &self.between,
            shown_fraction,
            &self.viewport,
            &self.style,
        )?;
        self.motion = pair.map(|(prior, gap)| Motion {
            prior: Prior {
                id: prior.id(),
                tick: prior.tick(),
            },
            fraction,
            gap,
            fading: self.between.fading.len(),
        });
        self.remember(current)?;
        Ok(self.motion)
    }

    /// Draw `epoch` again at the current viewport and style without moving
    /// the field's or density's display scale: what a pan or zoom redraws
    /// for the epoch already shown. The density layer re-places the epoch
    /// it last binned.
    ///
    /// # Errors
    /// As [`WorldView::show`].
    pub fn place(&mut self, epoch: &Epoch) -> Result<(), CoreError> {
        self.place_between(None, epoch, 1.0).map(|_| ())
    }

    /// Keep `epoch` as the shown one, with its identities, and the
    /// selection only if the epoch still has that identity drawn.
    fn remember(&mut self, epoch: &Epoch) -> Result<(), CoreError> {
        self.shown = Some(Prior {
            id: epoch.id(),
            tick: epoch.tick(),
        });
        // `lower` has refused an epoch above the ceiling this was reserved
        // for, so it does not grow.
        self.ids.clear();
        self.ids.extend_from_slice(epoch.ids());
        self.field_texels = epoch.field().texels();
        self.reselect(self.selected)
    }

    /// The row of `organism` in the shown epoch, if it is drawn.
    fn drawn_row(&self, organism: OrganismId) -> Option<usize> {
        let row = self.ids.binary_search(&organism.get()).ok()?;
        self.between.shown[row].then_some(row)
    }

    fn reselect(&mut self, organism: Option<OrganismId>) -> Result<(), CoreError> {
        match organism.and_then(|id| self.drawn_row(id)) {
            Some(row) => {
                let position = self.between.positions[row];
                self.ring = Some(selection_ring_around(
                    &self.viewport,
                    position,
                    dot_radius_at_least(&self.viewport, self.cloud.min_radius_points()),
                )?);
                self.selected = organism;
            }
            None => {
                self.ring = None;
                self.selected = None;
            }
        }
        Ok(())
    }

    /// The organism drawn nearest the drawable position `at`, within
    /// `radius_pixels`, among the shown epoch's drawn rows, where they are
    /// drawn, placed through `viewport` (this view's, or one a host is about
    /// to set): see [`pick`]. A fading organism is dead and is never picked.
    #[must_use]
    pub fn pick_in(
        &self,
        viewport: &Viewport,
        at: [f64; 2],
        radius_pixels: f64,
    ) -> Option<OrganismId> {
        pick(viewport, self.shown_rows(), at, radius_pixels)
    }

    /// The shown epoch's drawn rows: each drawn organism's identity and the
    /// world position it is drawn at, in identity order. At rest these are
    /// all the epoch's rows at their positions.
    pub fn shown_rows(&self) -> impl Iterator<Item = (u64, [f64; 2])> + '_ {
        self.ids
            .iter()
            .zip(&self.between.positions)
            .zip(&self.between.shown)
            .filter(|(_, shown)| **shown)
            .map(|((&id, &position), _)| (id, position))
    }

    /// Select `organism`, or nothing. An identity the shown epoch does not
    /// have, or has not drawn yet, selects nothing. Returns the selection
    /// now held.
    ///
    /// # Errors
    /// As [`selection_ring_around`]; the selection is unchanged on error.
    pub fn select(
        &mut self,
        organism: Option<OrganismId>,
    ) -> Result<Option<OrganismId>, CoreError> {
        self.reselect(organism)?;
        Ok(self.selected)
    }

    /// The selected organism, which the shown epoch has.
    #[must_use]
    pub const fn selected(&self) -> Option<OrganismId> {
        self.selected
    }

    /// The world position the selected organism is drawn at.
    #[must_use]
    pub fn selected_position(&self) -> Option<[f64; 2]> {
        let row = self.drawn_row(self.selected?)?;
        Some(self.between.positions[row])
    }

    /// The selection ring's draw, while an organism is selected: unlit,
    /// over everything else.
    #[must_use]
    pub fn selection_draw(&self) -> Option<ThreeDDraw<'_>> {
        self.ring.as_ref().map(|mesh| {
            ThreeDDraw::Surface(SurfaceDraw {
                shading: [0.0; 3],
                depth_test: false,
                ..SurfaceDraw::new(mesh)
            })
        })
    }

    /// Draw text over the world from now on (FL-024): the view charges a
    /// text's most in [`WorldView::reserved_bytes`],
    /// [`WorldView::preparation_limit`] and [`WorldView::frame_bytes`]
    /// whether it shows one or not, so a host enables it before it admits
    /// the view.
    pub fn enable_overlay(&mut self) {
        self.overlay = true;
    }

    /// Whether the view draws text.
    #[must_use]
    pub const fn has_overlay(&self) -> bool {
        self.overlay
    }

    /// Show `text` over the world from the next frame, or none for `None`
    /// or an empty text. Its plate and glyphs follow the viewport.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when the view does not draw text
    /// ([`WorldView::enable_overlay`]); as [`hud_plate`]. The view is
    /// unchanged on error.
    pub fn set_overlay(&mut self, text: Option<HudText>) -> Result<(), CoreError> {
        if !self.overlay {
            return Err(CoreError::StateConflict {
                operation: "world view overlay",
                detail: "the view does not draw text".to_owned(),
            });
        }
        self.hud = match text {
            Some(text) if !text.is_empty() => {
                let (plate, glyphs) = overlay_meshes(&self.viewport, &text)?;
                Some(Hud {
                    text,
                    plate,
                    glyphs,
                })
            }
            _ => None,
        };
        Ok(())
    }

    /// The text shown over the world.
    #[must_use]
    pub fn overlay(&self) -> Option<&HudText> {
        self.hud.as_ref().map(|hud| &hud.text)
    }

    /// The overlay's draws, while it shows text: the plate, then the
    /// glyphs, unlit and over everything else.
    fn overlay_draws(&self) -> impl Iterator<Item = ThreeDDraw<'_>> + '_ {
        let unlit = |mesh| {
            ThreeDDraw::Surface(SurfaceDraw {
                shading: [0.0; 3],
                depth_test: false,
                ..SurfaceDraw::new(mesh)
            })
        };
        self.hud.iter().flat_map(move |hud| {
            std::iter::once(unlit(&hud.plate)).chain(hud.glyphs.iter().map(unlit))
        })
    }

    /// The current frame's draws in painter order: the field, then the
    /// density layer, or every halo and then every dot, then the selection
    /// ring, then the overlay's plate and text. Raw density is opaque, so
    /// the field is left out under it.
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when the list cannot be reserved.
    pub fn draws(&self) -> Result<Vec<ThreeDDraw<'_>>, CoreError> {
        let style = self.style;
        let density = style.particles == Particles::Density;
        let halos = self.cloud.halos();
        let dots = self.cloud.draws();
        let mut draws: Vec<ThreeDDraw<'_>> = Vec::new();
        let fixed = 3 + OVERLAY_DRAWS;
        draws
            .try_reserve_exact(fixed + halos.len() + dots.len())
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "world view frame draws",
                bytes: u64::try_from(
                    (fixed + halos.len() + dots.len()).saturating_mul(size_of::<ThreeDDraw<'_>>()),
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
        draws.extend(self.selection_draw());
        draws.extend(self.overlay_draws());
        Ok(draws)
    }

    /// The 3D job for the current frame, over [`WorldView::draws`],
    /// compiled within [`WorldView::preparation_limit`] (FL-043), with
    /// [`WorldView::frame_bloom`] (FL-019).
    /// FrankenManim leaves out a tile bin that would pass it, which only
    /// slows the frame; rows that would pass it refuse the frame.
    ///
    /// # Errors
    /// As [`WorldView::draws`]; [`CoreError::NumericalInvalid`] when
    /// FrankenManim refuses to compile the draws.
    pub fn job(&self) -> Result<ThreeDJob<'_>, CoreError> {
        let draws = self.draws()?;
        let limits = ThreeDPreparationLimits {
            max_working_bytes: self.preparation_limit(),
            ..ThreeDPreparationLimits::default()
        };
        let job =
            ThreeDJob::new_with_limits(self.camera.camera(), &draws, Tiling::default(), limits)
                .map_err(|error| CoreError::NumericalInvalid {
                    quantity: "world view frame",
                    detail: error.to_string(),
                })?;
        Ok(match self.frame_bloom() {
            Some(bloom) => job.with_bloom(bloom.clone()),
            None => job,
        })
    }

    /// The most blooming holds for a frame of `drawable` pixels (FL-019,
    /// FL-043), while a bloom is configured, whatever the look:
    /// FrankenManim's row sums, [`BLOOM_SCRATCH_BYTES_PER_PIXEL`] a pixel,
    /// for a band of `BLOOM_BAND_ROWS` and the widest blur's reach above
    /// and below it ([`BLOOM_WIDEST_REACH_PIXELS`]), or the whole frame
    /// when that is shorter: what `Bloom::scratch_bytes` gives at any
    /// pixel density, so a new density needs no new admission. The CPU
    /// route allocates them for each frame it blooms; the Metal route
    /// keeps them on the device once it has bloomed.
    #[must_use]
    pub fn bloom_bytes(&self, drawable: [u32; 2]) -> u64 {
        let [width, height] = drawable;
        let rows = height.min(BLOOM_BAND_ROWS + 2 * BLOOM_WIDEST_REACH_PIXELS);
        match self.bloom {
            Some(_) => u64::from(width)
                .saturating_mul(u64::from(rows))
                .saturating_mul(BLOOM_SCRATCH_BYTES_PER_PIXEL),
            None => 0,
        }
    }

    /// The most bytes the view's own buffers hold (FL-043): the shown
    /// rows' identities and drawn positions and the cloud's draws and
    /// halos, each reserved for the population ceiling; the density bins
    /// at the viewport's count; and the field's and the density's textures
    /// ([`WorldView::texel_bytes`]) and quads and the selection ring's
    /// mesh, each twice, since one is built before the one it replaces is
    /// dropped; and once, the copy of the field's values its texture was
    /// built from, one `f32` a texel (FL-033). The field's texture and
    /// values count once the view knows the field: from
    /// [`WorldView::for_epoch`] or the first shown epoch. A resize or a new
    /// pixel density changes the density's bins and so these bytes.
    #[must_use]
    pub fn reserved_bytes(&self) -> u64 {
        let [columns, rows] = density_bins(&self.viewport).map(u64::from);
        let bins = columns.saturating_mul(rows);
        let ring = SELECTION_RING_SEGMENTS as u64;
        let [columns, rows] = self.field_texels.map(u64::from);
        let field_values = columns
            .saturating_mul(rows)
            .saturating_mul(bytes_for::<f32>(1));
        let grids = total([
            self.texel_bytes(),
            mesh_bytes(QUAD).saturating_mul(2),
            mesh_bytes((2 * ring, 2 * ring)),
        ]);
        // A text's buffers and meshes, twice: one is built before the one it
        // replaces is dropped.
        let overlay = if self.overlay {
            total([
                HudText::reserved_bytes(),
                mesh_bytes(QUAD),
                mesh_bytes((HUD_MAX_VERTICES as u64, HUD_MAX_TRIANGLES as u64)),
            ])
            .saturating_mul(2)
        } else {
            0
        };
        total([
            overlay,
            capacity_bytes(&self.ids),
            capacity_bytes(&self.between.positions),
            capacity_bytes(&self.between.shown),
            capacity_bytes(&self.between.fading),
            self.cloud.reserved_bytes(),
            bins.saturating_mul(bytes_for::<u32>(1) + bytes_for::<[f64; 3]>(1)),
            grids.saturating_mul(2),
            field_values,
        ])
    }

    /// The preparation bytes a frame's job is compiled within (FL-043):
    /// [`PREPARATION_BYTES_PER_DOT`] for a dot and a halo per organism the
    /// view can hold, [`PREPARATION_BYTES_PER_TRIANGLE`] for every triangle
    /// of the field's and the density's quads and the ring, and
    /// [`PREPARATION_BYTES_FIXED`], whatever the style, which a host may
    /// change from frame to frame. The layers' texels are not preparation:
    /// a job samples them in place ([`WorldView::texel_bytes`]).
    #[must_use]
    pub fn preparation_limit(&self) -> u64 {
        let dots = (self.cloud.capacity() as u64).saturating_mul(2);
        let ring = 2 * SELECTION_RING_SEGMENTS as u64;
        let overlay = if self.overlay {
            QUAD.1 + HUD_MAX_TRIANGLES as u64
        } else {
            0
        };
        let triangles = total([QUAD.1, QUAD.1, ring, overlay]);
        total([
            dots.saturating_mul(PREPARATION_BYTES_PER_DOT),
            triangles.saturating_mul(PREPARATION_BYTES_PER_TRIANGLE),
            PREPARATION_BYTES_FIXED,
        ])
    }

    /// The texels a frame's draws can sample (FL-033): the field's texture,
    /// once the view knows the field, and the density's at the viewport's
    /// bins, whatever the style, as premultiplied linear `f32` RGBA.
    /// FrankenManim's CPU route samples them in place; its Metal route
    /// uploads the ones a frame draws with the frame.
    #[must_use]
    pub fn texel_bytes(&self) -> u64 {
        total([
            texel_bytes_of(self.field_texels),
            texel_bytes_of(density_bins(&self.viewport)),
        ])
    }

    /// The most one frame's [`WorldView::job`] holds (FL-043): its draw
    /// list, a field, a density and a ring draw and a dot and a halo per
    /// organism the view can hold, [`JOB_BYTES_PER_DRAW`] for each of those
    /// draws, and FrankenManim's preparation, within
    /// [`WorldView::preparation_limit`].
    #[must_use]
    pub fn frame_bytes(&self) -> u64 {
        let overlay = if self.overlay { OVERLAY_DRAWS } else { 0 };
        let draws = self
            .cloud
            .capacity()
            .saturating_mul(2)
            .saturating_add(3 + overlay);
        total([
            bytes_for::<ThreeDDraw<'static>>(draws),
            (draws as u64).saturating_mul(JOB_BYTES_PER_DRAW),
            self.preparation_limit(),
        ])
    }

    /// What [`WorldView::render_rgba8`] holds on `threads` threads on top
    /// of [`WorldView::frame_bytes`] (FL-043): the RGBA16F frame
    /// FrankenManim renders, the RGBA8 frame returned, and each thread's
    /// tile scratch, a premultiplied colour and a depth for every sample of
    /// a tile at the finest edge sampling and a flag for every pixel, and
    /// the bloom's row sums ([`WorldView::bloom_bytes`]).
    #[must_use]
    pub fn offline_bytes(&self, threads: usize) -> u64 {
        let [width, height] = self.viewport.drawable().map(u64::from);
        let pixels = width.saturating_mul(height);
        let tile = u64::from(Tiling::default().fine_tile).pow(2);
        let scratch = tile.saturating_mul(
            MAXIMUM_SAMPLES_PER_PIXEL
                .saturating_mul(bytes_for::<PremulRgba>(1) + bytes_for::<f32>(1))
                .saturating_add(1),
        );
        total([
            pixels.saturating_mul(8 + 4),
            scratch.saturating_mul(threads.max(1) as u64),
            self.bloom_bytes(self.viewport.drawable()),
        ])
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

    /// The epoch currently shown: the newest lowered, which the display
    /// reaches at the end of its [`Motion`].
    #[must_use]
    pub const fn shown(&self) -> Option<Prior> {
        self.shown
    }

    /// The display's motion towards the shown epoch, or `None` at rest.
    #[must_use]
    pub const fn motion(&self) -> Option<Motion> {
        self.motion
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
    /// however many organisms it draws, its density layer, and the
    /// selection ring.
    #[must_use]
    pub const fn scene_objects(&self) -> usize {
        4
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
