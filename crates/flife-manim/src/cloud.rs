//! One particle cloud for the whole live population (plan §13.2,
//! `ParticleCloudView`).
//!
//! A [`ParticleCloud`] lowers a render epoch into FrankenManim dot clouds
//! (`DotCloudDraw`, FL-033): one camera-facing dot per organism, in the
//! epoch's stable identity order. A cloud draws exactly the true dots of its
//! dots (`DotCloudDraw::true_dot`), byte for byte on both of FrankenManim's
//! routes, so one true dot per organism remains the reference. The cloud is
//! one scene object however many organisms live. It owns no mobject,
//! animation handle or draw call per organism, and a frame is one 3D job.
//!
//! Its dot buffers are reserved once, at the population ceiling, so lowering
//! never allocates. A dot's colour is the style's [`ColorMapping`] of its
//! row; the glow look adds a halo under every dot, the raw look none.
//!
//! Between two committed epochs (FL-020), [`ParticleCloud::update_between`]
//! draws each organism where an interpolation places it, and fades out the
//! organisms that died in the gap.
//!
//! [`ColorMapping`]: crate::ColorMapping

use flife_core::CoreError;
use flife_render::{Epoch, Interpolation};
use fmn_core::color::LinearRgba;
use fmn_render::{CloudDot, DotCloudDraw};

use crate::Viewport;
use crate::mapping::{EpochColors, Look, Particles, Style};

/// Dot radius in world units.
pub const DOT_RADIUS: f64 = 0.35;

/// The smallest dot radius in points (drawable pixels at one pixel per
/// point) unless one is configured: a world too large for its window
/// still shows every organism. The Studio and `flife render` set
/// `[presentation] point_radius_pixels` in its place
/// ([`ParticleCloud::set_min_radius_points`], FL-019).
pub const DOT_RADIUS_MIN_POINTS: f64 = 1.5;

/// The largest dot radius in points (FL-020): zoomed in past it, dots stop
/// growing on screen, so organisms close together separate, and a dot
/// stays small against the camera's focal distance (FrankenManim's frame is
/// a perspective one, about 1.2 frame heights away), where its projection
/// is the disk the viewport places.
pub const DOT_RADIUS_MAX_POINTS: f64 = 24.0;

/// The largest radius a declared body (a contact model's) is drawn at, as a
/// fraction of the drawable's height, and never less than
/// [`DOT_RADIUS_MAX_POINTS`] points. Up to it a body is drawn at its world
/// size, so bodies that touch in the world touch on screen: under the point
/// cap a confluent world's tissue fell apart into loose dots past about 14x
/// (FL-024's G5 review). The camera stands about 1.2 frame heights away, so
/// at this bound a body's radius is about a tenth of that distance and its
/// largest glow halo (4.5 times) under half of it; a billboard's size error
/// is second order in that ratio.
pub const BODY_RADIUS_MAX_FRAME: f64 = 0.125;

/// The sphere look's `(reflectiveness, gloss, shadow)`: FrankenManim's
/// true-dot lighting, which shades each dot as a sphere facing the camera.
/// Both lifts towards white are kept small and the shadow deep, so a body
/// keeps its colour, with a dark rim and a soft highlight, as the reference
/// screenshots' close-ups show; together they raise the lit pixels' median
/// OKLab lightness by about 0.05.
pub const SPHERE_SHADING: [f64; 3] = [0.10, 0.12, 0.90];

/// A halo's opacity: the soft glow around each organism.
pub const HALO_ALPHA: f64 = 0.42;

/// The energy at which a halo reaches half its extra size.
pub const HALO_HALF_ENERGY: f64 = 2.0;

/// A halo's radius as a multiple of the dot's: from `2` for an organism
/// with no energy towards `4.5` for a very energetic one.
#[must_use]
pub fn halo_factor(energy: f32) -> f64 {
    let energy = if energy.is_finite() {
        f64::from(energy).max(0.0)
    } else {
        0.0
    };
    2.0 + 2.5 * (energy / (energy + HALO_HALF_ENERGY))
}

/// The dot radius in world units for a viewport: [`DOT_RADIUS`], but at
/// least [`DOT_RADIUS_MIN_POINTS`] and at most [`DOT_RADIUS_MAX_POINTS`] on
/// screen, at the viewport's pixels per point.
#[must_use]
pub fn dot_radius(viewport: &Viewport) -> f64 {
    dot_radius_at_least(viewport, DOT_RADIUS_MIN_POINTS)
}

/// The dot radius in world units for a viewport whose dots are at least
/// `min_points` on screen: [`DOT_RADIUS`], but at least `min_points` and at
/// most [`DOT_RADIUS_MAX_POINTS`] points, at the viewport's pixels per
/// point.
#[must_use]
pub fn dot_radius_at_least(viewport: &Viewport, min_points: f64) -> f64 {
    dot_radius_with(viewport, min_points, None)
}

/// The dot radius in world units for a viewport whose dots are at least
/// `min_points` on screen, at the viewport's pixels per point.
///
/// - Point organisms (`body` is `None`): [`DOT_RADIUS`], at most
///   [`DOT_RADIUS_MAX_POINTS`] points.
/// - Organisms whose bodies are `body` world units in radius: `body`, at
///   most [`BODY_RADIUS_MAX_FRAME`] of the drawable's height (never less
///   than [`DOT_RADIUS_MAX_POINTS`] points).
#[must_use]
pub fn dot_radius_with(viewport: &Viewport, min_points: f64, body: Option<f64>) -> f64 {
    // Points to pixels first: at one pixel per point the product is exact,
    // so the radius is bit for bit what a pixel bound gives.
    let pixels = viewport.pixels_per_point();
    let scale = viewport.scale();
    let point_cap = DOT_RADIUS_MAX_POINTS * pixels;
    let (radius, cap) = match body {
        None => (DOT_RADIUS, point_cap),
        Some(body) => (
            body,
            (f64::from(viewport.drawable()[1]) * BODY_RADIUS_MAX_FRAME).max(point_cap),
        ),
    };
    radius.max(min_points * pixels / scale).min(cap / scale)
}

/// Refuse a smallest dot radius that is not finite, positive and at most
/// [`DOT_RADIUS_MAX_POINTS`] points.
pub(crate) fn check_min_radius_points(points: f64) -> Result<(), CoreError> {
    if points > 0.0 && points <= DOT_RADIUS_MAX_POINTS {
        Ok(())
    } else {
        Err(CoreError::NumericalInvalid {
            quantity: "presentation point radius",
            detail: format!(
                "{points} points: a dot's smallest radius is positive and at most \
                 {DOT_RADIUS_MAX_POINTS}, its largest"
            ),
        })
    }
}

/// The live population as one bounded scene object.
#[derive(Debug)]
pub struct ParticleCloud {
    /// One dot per organism lowered, in identity order.
    dots: Vec<CloudDot>,
    /// One soft glow per organism, drawn under every dot; sized by energy.
    halos: Vec<CloudDot>,
    /// The dots' shading: [`SPHERE_SHADING`] in the sphere look, else none.
    shading: [f64; 3],
    radius: f64,
    /// The smallest radius a dot takes on screen, in points.
    min_points: f64,
    /// An organism's body radius in world units, if the model declares one;
    /// point organisms are drawn at [`DOT_RADIUS`].
    body: Option<f64>,
    capacity: usize,
}

impl ParticleCloud {
    /// A cloud for up to `capacity` organisms, its dot buffers reserved now.
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when a buffer cannot be reserved.
    pub fn with_capacity(capacity: usize) -> Result<Self, CoreError> {
        let reserve = |buffer| {
            let mut dots = Vec::new();
            dots.try_reserve_exact(capacity)
                .map_err(|_| CoreError::AllocationFailed {
                    buffer,
                    bytes: u64::try_from(capacity.saturating_mul(size_of::<CloudDot>()))
                        .unwrap_or(u64::MAX),
                })?;
            Ok::<_, CoreError>(dots)
        };
        Ok(Self {
            dots: reserve("particle cloud draws")?,
            halos: reserve("particle cloud halos")?,
            shading: [0.0; 3],
            radius: DOT_RADIUS,
            min_points: DOT_RADIUS_MIN_POINTS,
            body: None,
            capacity,
        })
    }

    /// A cloud sized for `epoch`'s population ceiling.
    ///
    /// # Errors
    /// As [`ParticleCloud::with_capacity`].
    pub fn for_epoch(epoch: &Epoch) -> Result<Self, CoreError> {
        Self::with_capacity(epoch.capacity())
    }

    /// Lower `epoch` through `viewport` in the default style: lineage
    /// colours with glow halos.
    ///
    /// # Errors
    /// As [`ParticleCloud::update_styled`].
    pub fn update(
        &mut self,
        epoch: &Epoch,
        viewport: &Viewport,
    ) -> Result<DotCloudDraw<'_>, CoreError> {
        self.update_styled(epoch, viewport, &Style::default())
    }

    /// Lower `epoch` through `viewport` in `style`: one dot per row, in
    /// identity order, centred on the row's scene point and coloured by the
    /// style's mapping, with a halo under each for the glow look. The
    /// density style draws no dots (the density layer does). Nothing is
    /// allocated. Returns the dots ([`ParticleCloud::draws`]).
    ///
    /// # Errors
    /// - [`CoreError::BudgetExhausted`] when the epoch has more rows than
    ///   the cloud's capacity. The previous draws are kept.
    /// - [`CoreError::LayoutMismatch`] when the epoch's world is not the
    ///   viewport's. The previous draws are kept.
    pub fn update_styled(
        &mut self,
        epoch: &Epoch,
        viewport: &Viewport,
        style: &Style,
    ) -> Result<DotCloudDraw<'_>, CoreError> {
        if epoch.len() > self.capacity {
            return Err(CoreError::BudgetExhausted {
                budget: "particle cloud draws",
                required: u64::try_from(epoch.len()).unwrap_or(u64::MAX),
                available: u64::try_from(self.capacity).unwrap_or(u64::MAX),
            });
        }
        if epoch.extent().map(f64::to_bits) != viewport.extent().map(f64::to_bits) {
            return Err(CoreError::LayoutMismatch {
                buffer: "particle cloud",
                detail: format!(
                    "epoch world {:?} is not the viewport's {:?}",
                    epoch.extent(),
                    viewport.extent()
                ),
            });
        }
        self.begin(viewport, style);
        if style.particles == Particles::Density {
            return Ok(self.draws());
        }
        let (start, extent, half) = viewport.scene_transform();
        let positions = epoch.positions();
        let colors = EpochColors::new(style.mapping, epoch);
        let radius = self.radius;
        match style.look {
            Look::Glow => {
                let energies = epoch.energies();
                colors.for_each(positions.len(), |row, color| {
                    let position = positions[row];
                    let center = Viewport::scene_point_with(
                        start,
                        extent,
                        half,
                        [f64::from(position[0]), f64::from(position[1])],
                    );
                    self.dots.push(CloudDot {
                        center,
                        radius,
                        color,
                    });
                    self.halos.push(CloudDot {
                        center,
                        radius: radius * halo_factor(energies[row]),
                        color: LinearRgba {
                            a: HALO_ALPHA,
                            ..color
                        },
                    });
                });
            }
            // The sphere look's shading is the dots cloud's.
            Look::Raw | Look::Sphere => {
                colors.for_each(positions.len(), |row, color| {
                    let position = positions[row];
                    let center = Viewport::scene_point_with(
                        start,
                        extent,
                        half,
                        [f64::from(position[0]), f64::from(position[1])],
                    );
                    self.dots.push(CloudDot {
                        center,
                        radius,
                        color,
                    });
                });
            }
        }
        Ok(self.draws())
    }

    /// Lower the display `fraction` of the way from `prior` to `current`
    /// (FL-020, plan §13.5), as `between` places it ([`interpolate`]):
    /// first each organism that died in the gap, at its `prior` position in
    /// its `prior` colour, its dot and halo faded by `1 − fraction` and gone
    /// at fraction 1; then each of `current`'s rows that `between` shows, in
    /// identity order, at its drawn position in its `current` colour. The
    /// dead fade under the living. Nothing is allocated: while the display
    /// moves, births are not yet shown, so the living and the fading
    /// together are `prior`'s rows.
    ///
    /// # Errors
    /// - [`CoreError::LayoutMismatch`] when `between` does not row for row
    ///   describe `current`, names a fading row `prior` lacks (or fading
    ///   rows with no `prior`), or either epoch's world is not the
    ///   viewport's. The previous draws are kept.
    /// - [`CoreError::BudgetExhausted`] when the dots to draw exceed the
    ///   cloud's capacity. The previous draws are kept.
    /// - [`CoreError::NumericalInvalid`] for a fraction outside `[0, 1]`.
    ///
    /// [`interpolate`]: flife_render::interpolate
    pub fn update_between(
        &mut self,
        prior: Option<&Epoch>,
        current: &Epoch,
        between: &Interpolation,
        fraction: f64,
        viewport: &Viewport,
        style: &Style,
    ) -> Result<DotCloudDraw<'_>, CoreError> {
        if !(0.0..=1.0).contains(&fraction) {
            return Err(CoreError::NumericalInvalid {
                quantity: "particle cloud display fraction",
                detail: format!("{fraction} is outside [0, 1]"),
            });
        }
        let mismatch = |detail: String| CoreError::LayoutMismatch {
            buffer: "particle cloud",
            detail,
        };
        if between.positions.len() != current.len() || between.shown.len() != current.len() {
            return Err(mismatch(format!(
                "{} drawn positions and {} visibilities for an epoch of {} rows",
                between.positions.len(),
                between.shown.len(),
                current.len()
            )));
        }
        let fade = 1.0 - fraction;
        let fading = if fade > 0.0 { between.fading.len() } else { 0 };
        let prior_rows = prior.map_or(0, Epoch::len);
        if between.fading.iter().any(|&row| row >= prior_rows) {
            return Err(mismatch(format!(
                "fading rows beyond the {prior_rows} rows of the prior epoch"
            )));
        }
        for epoch in prior.into_iter().chain([current]) {
            if epoch.extent().map(f64::to_bits) != viewport.extent().map(f64::to_bits) {
                return Err(mismatch(format!(
                    "epoch world {:?} is not the viewport's {:?}",
                    epoch.extent(),
                    viewport.extent()
                )));
            }
        }
        let drawn = between.shown.iter().filter(|&&shown| shown).count() + fading;
        if drawn > self.capacity {
            return Err(CoreError::BudgetExhausted {
                budget: "particle cloud draws",
                required: u64::try_from(drawn).unwrap_or(u64::MAX),
                available: u64::try_from(self.capacity).unwrap_or(u64::MAX),
            });
        }
        self.begin(viewport, style);
        if style.particles == Particles::Density {
            return Ok(self.draws());
        }
        let (start, extent, half) = viewport.scene_transform();
        let radius = self.radius;
        if let Some(prior) = prior.filter(|_| fading > 0) {
            // Direct-index only the fading rows in O(fading) instead of
            // scanning every row of `prior`; every row was validated against
            // `prior.len()` above.
            let prior_positions = prior.positions();
            let prior_colors = EpochColors::new(style.mapping, prior);
            let prior_energies = prior.energies();
            let halo_alpha = HALO_ALPHA * fade;
            for &row in &between.fading {
                let position = prior_positions[row];
                let center = Viewport::scene_point_with(
                    start,
                    extent,
                    half,
                    [f64::from(position[0]), f64::from(position[1])],
                );
                let color = prior_colors.color_at(row);
                self.dots.push(CloudDot {
                    center,
                    radius,
                    color: LinearRgba {
                        a: color.a * fade,
                        ..color
                    },
                });
                if style.look == Look::Glow {
                    self.halos.push(CloudDot {
                        center,
                        radius: radius * halo_factor(prior_energies[row]),
                        color: LinearRgba {
                            a: halo_alpha,
                            ..color
                        },
                    });
                }
            }
        }
        let current_colors = EpochColors::new(style.mapping, current);
        let positions = between.positions.as_slice();
        let shown = between.shown.as_slice();
        match style.look {
            Look::Glow => {
                let energies = current.energies();
                current_colors.for_each(positions.len(), |row, color| {
                    if shown[row] {
                        let center =
                            Viewport::scene_point_with(start, extent, half, positions[row]);
                        self.dots.push(CloudDot {
                            center,
                            radius,
                            color,
                        });
                        self.halos.push(CloudDot {
                            center,
                            radius: radius * halo_factor(energies[row]),
                            color: LinearRgba {
                                a: HALO_ALPHA,
                                ..color
                            },
                        });
                    }
                });
            }
            Look::Raw | Look::Sphere => {
                current_colors.for_each(positions.len(), |row, color| {
                    if shown[row] {
                        let center =
                            Viewport::scene_point_with(start, extent, half, positions[row]);
                        self.dots.push(CloudDot {
                            center,
                            radius,
                            color,
                        });
                    }
                });
            }
        }
        Ok(self.draws())
    }

    /// Clear the dots for a new lowering through `viewport` in `style`.
    fn begin(&mut self, viewport: &Viewport, style: &Style) {
        self.radius = self.radius_on(viewport);
        self.shading = if style.look == Look::Sphere {
            SPHERE_SHADING
        } else {
            [0.0; 3]
        };
        self.dots.clear();
        self.halos.clear();
    }

    /// The radius a dot takes in world units on `viewport`
    /// ([`dot_radius_with`]): the body radius within the on-screen bounds.
    #[must_use]
    pub fn radius_on(&self, viewport: &Viewport) -> f64 {
        dot_radius_with(viewport, self.min_points, self.body)
    }

    /// An organism's radius in world units: its declared body's, or
    /// [`DOT_RADIUS`] for a point organism.
    #[must_use]
    pub const fn body_radius(&self) -> f64 {
        match self.body {
            Some(body) => body,
            None => DOT_RADIUS,
        }
    }

    /// The declared body radius in world units, `None` for point organisms.
    #[must_use]
    pub const fn declared_body(&self) -> Option<f64> {
        self.body
    }

    /// Draw organisms as bodies `body` world units in radius, refitting the
    /// current draws to `viewport`. Non-finite or non-positive radii are
    /// ignored.
    pub fn set_body_radius(&mut self, body: f64, viewport: &Viewport) {
        if !(body.is_finite() && body > 0.0) {
            return;
        }
        self.body = Some(body);
        self.refit(viewport);
    }

    /// The smallest radius a dot takes on screen, in points:
    /// [`DOT_RADIUS_MIN_POINTS`] unless one is set.
    #[must_use]
    pub const fn min_radius_points(&self) -> f64 {
        self.min_points
    }

    /// Draw each dot at least `points` in radius on screen
    /// (`[presentation] point_radius_pixels`, FL-019), refitting the current
    /// draws through `viewport` at once.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless `points` is finite, positive
    /// and at most [`DOT_RADIUS_MAX_POINTS`]; the cloud is unchanged on
    /// error.
    pub fn set_min_radius_points(
        &mut self,
        points: f64,
        viewport: &Viewport,
    ) -> Result<(), CoreError> {
        check_min_radius_points(points)?;
        self.min_points = points;
        self.refit(viewport);
        Ok(())
    }

    /// Refit the current draws' radius to a new viewport. Scene points do
    /// not depend on the drawable, so only the pixel floor on the radius
    /// can change.
    pub fn refit(&mut self, viewport: &Viewport) {
        let radius = self.radius_on(viewport);
        if radius.to_bits() == self.radius.to_bits() {
            return;
        }
        let ratio = if self.radius.is_finite() && self.radius > 0.0 {
            radius / self.radius
        } else {
            1.0
        };
        self.radius = radius;
        for dot in &mut self.dots {
            dot.radius = radius;
        }
        if ratio.is_finite() && ratio > 0.0 {
            for halo in &mut self.halos {
                halo.radius *= ratio;
            }
        }
    }

    /// The current halos as one cloud of glow dots
    /// ([`DotCloudDraw::glow`]): for the glow look one per dot and in the
    /// same order, for the other looks none.
    #[must_use]
    pub fn halos(&self) -> DotCloudDraw<'_> {
        DotCloudDraw::glow(&self.halos)
    }

    /// The current dots as one cloud, one per organism of the last epoch
    /// lowered: unlit dots ([`DotCloudDraw::new`]), shaded by
    /// [`SPHERE_SHADING`] in the sphere look.
    #[must_use]
    pub fn draws(&self) -> DotCloudDraw<'_> {
        DotCloudDraw {
            shading: self.shading,
            ..DotCloudDraw::new(&self.dots)
        }
    }

    /// The most organisms the cloud can hold.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Bytes reserved for the dots and halos. Constant for the cloud's life.
    #[must_use]
    pub fn reserved_bytes(&self) -> u64 {
        u64::try_from(
            (self.dots.capacity() + self.halos.capacity()).saturating_mul(size_of::<CloudDot>()),
        )
        .unwrap_or(u64::MAX)
    }
}
