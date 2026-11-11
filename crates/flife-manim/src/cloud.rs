//! One particle cloud for the whole live population (plan §13.2,
//! `ParticleCloudView`).
//!
//! A [`ParticleCloud`] lowers a render epoch into FrankenManim's 3D painter
//! sequence: one camera-facing true dot per organism, in the epoch's stable
//! identity order. That is what FrankenManim's own `DotCloud` lowers to on
//! its camera route, so the existing route is the reference. The cloud is
//! one scene object however many organisms live. It owns no mobject,
//! animation handle or draw call per organism, and a frame is one 3D job.
//!
//! Its draw buffer is reserved once, at the population ceiling, so lowering
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
use fmn_render::{ThreeDDraw, TrueDotDraw};

use crate::Viewport;
use crate::mapping::{Look, Particles, Style, Traits, mapped_color, traits};

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
    // Points to pixels first: at one pixel per point the product is exact,
    // so the radius is bit for bit what a pixel bound gives.
    let pixels = viewport.pixels_per_point();
    let scale = viewport.scale();
    DOT_RADIUS
        .max(min_points * pixels / scale)
        .min(DOT_RADIUS_MAX_POINTS * pixels / scale)
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
    draws: Vec<ThreeDDraw<'static>>,
    /// One soft glow per organism, drawn under every dot; sized by energy.
    halos: Vec<ThreeDDraw<'static>>,
    radius: f64,
    /// The smallest radius a dot takes on screen, in points.
    min_points: f64,
    capacity: usize,
}

impl ParticleCloud {
    /// A cloud for up to `capacity` organisms, its draw buffer reserved now.
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when the buffer cannot be reserved.
    pub fn with_capacity(capacity: usize) -> Result<Self, CoreError> {
        let reserve = |buffer| {
            let mut draws = Vec::new();
            draws
                .try_reserve_exact(capacity)
                .map_err(|_| CoreError::AllocationFailed {
                    buffer,
                    bytes: u64::try_from(capacity.saturating_mul(size_of::<ThreeDDraw<'static>>()))
                        .unwrap_or(u64::MAX),
                })?;
            Ok::<_, CoreError>(draws)
        };
        Ok(Self {
            draws: reserve("particle cloud draws")?,
            halos: reserve("particle cloud halos")?,
            radius: DOT_RADIUS,
            min_points: DOT_RADIUS_MIN_POINTS,
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
    ) -> Result<&[ThreeDDraw<'static>], CoreError> {
        self.update_styled(epoch, viewport, &Style::default())
    }

    /// Lower `epoch` through `viewport` in `style`: one dot per row, in
    /// identity order, centred on the row's scene point and coloured by the
    /// style's mapping, with a halo under each for the glow look. The
    /// density style draws no dots (the density layer does). Nothing is
    /// allocated.
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
    ) -> Result<&[ThreeDDraw<'static>], CoreError> {
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
        self.begin(viewport);
        if style.particles == Particles::Density {
            return Ok(&self.draws);
        }
        let speed = epoch.maximum_speed();
        for (position, traits) in epoch.positions().iter().zip(traits(epoch)) {
            let world = [f64::from(position[0]), f64::from(position[1])];
            self.push(viewport.scene_point(world), &traits, speed, style, 1.0);
        }
        Ok(&self.draws)
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
    ) -> Result<&[ThreeDDraw<'static>], CoreError> {
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
        self.begin(viewport);
        if style.particles == Particles::Density {
            return Ok(&self.draws);
        }
        if let Some(prior) = prior.filter(|_| fading > 0) {
            // Fading rows are in identity order, so increasing rows: one
            // pass over the prior epoch's traits finds them all.
            let speed = prior.maximum_speed();
            let mut rows = between.fading.iter().copied().peekable();
            for (row, (position, traits)) in prior.positions().iter().zip(traits(prior)).enumerate()
            {
                if rows.peek() != Some(&row) {
                    continue;
                }
                rows.next();
                let world = [f64::from(position[0]), f64::from(position[1])];
                self.push(viewport.scene_point(world), &traits, speed, style, fade);
            }
        }
        let speed = current.maximum_speed();
        for ((position, &shown), traits) in between
            .positions
            .iter()
            .zip(&between.shown)
            .zip(traits(current))
        {
            if shown {
                self.push(viewport.scene_point(*position), &traits, speed, style, 1.0);
            }
        }
        Ok(&self.draws)
    }

    /// Clear the draws for a new lowering through `viewport`.
    fn begin(&mut self, viewport: &Viewport) {
        self.radius = dot_radius_at_least(viewport, self.min_points);
        self.draws.clear();
        self.halos.clear();
    }

    /// One organism's dot, and its halo for the glow look, at `center`,
    /// with every alpha multiplied by `opacity`.
    fn push(
        &mut self,
        center: [f64; 3],
        traits: &Traits,
        maximum_speed: f64,
        style: &Style,
        opacity: f64,
    ) {
        let color = mapped_color(style.mapping, traits, maximum_speed);
        let radius = self.radius;
        self.draws.push(ThreeDDraw::TrueDot(TrueDotDraw::new(
            center,
            radius,
            LinearRgba {
                a: color.a * opacity,
                ..color
            },
        )));
        if style.look == Look::Glow {
            self.halos.push(ThreeDDraw::TrueDot(TrueDotDraw::glow(
                center,
                radius * halo_factor(traits.energy),
                LinearRgba {
                    a: HALO_ALPHA * opacity,
                    ..color
                },
            )));
        }
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
        let radius = dot_radius_at_least(viewport, self.min_points);
        let ratio = radius / self.radius;
        self.radius = radius;
        for draw in &mut self.draws {
            if let ThreeDDraw::TrueDot(dot) = draw {
                dot.radius = radius;
            }
        }
        for halo in &mut self.halos {
            if let ThreeDDraw::TrueDot(dot) = halo {
                dot.radius *= ratio;
            }
        }
    }

    /// The current halos: for the glow look one per dot and in the same
    /// order, for the raw look none.
    #[must_use]
    pub fn halos(&self) -> &[ThreeDDraw<'static>] {
        &self.halos
    }

    /// The current draws, one per organism of the last epoch lowered.
    #[must_use]
    pub fn draws(&self) -> &[ThreeDDraw<'static>] {
        &self.draws
    }

    /// The most organisms the cloud can hold.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Bytes reserved for draws. Constant for the cloud's life.
    #[must_use]
    pub fn reserved_bytes(&self) -> u64 {
        u64::try_from(
            (self.draws.capacity() + self.halos.capacity())
                .saturating_mul(size_of::<ThreeDDraw<'static>>()),
        )
        .unwrap_or(u64::MAX)
    }
}
