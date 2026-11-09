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
//! [`ColorMapping`]: crate::ColorMapping

use flife_core::CoreError;
use flife_render::Epoch;
use fmn_core::color::LinearRgba;
use fmn_render::{ThreeDDraw, TrueDotDraw};

use crate::Viewport;
use crate::mapping::{Look, Particles, Style, mapped_color, traits};

/// Dot radius in world units.
pub const DOT_RADIUS: f64 = 0.35;

/// The smallest dot radius in drawable pixels: a world too large for its
/// window still shows every organism.
pub const DOT_RADIUS_MIN_PIXELS: f64 = 1.5;

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

/// The dot radius in world units for a viewport.
#[must_use]
pub fn dot_radius(viewport: &Viewport) -> f64 {
    DOT_RADIUS.max(DOT_RADIUS_MIN_PIXELS / viewport.scale())
}

/// The live population as one bounded scene object.
#[derive(Debug)]
pub struct ParticleCloud {
    draws: Vec<ThreeDDraw<'static>>,
    /// One soft glow per organism, drawn under every dot; sized by energy.
    halos: Vec<ThreeDDraw<'static>>,
    radius: f64,
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
        let radius = dot_radius(viewport);
        self.radius = radius;
        self.draws.clear();
        self.halos.clear();
        if style.particles == Particles::Density {
            return Ok(&self.draws);
        }
        let speed = epoch.maximum_speed();
        for (position, traits) in epoch.positions().iter().zip(traits(epoch)) {
            let world = [f64::from(position[0]), f64::from(position[1])];
            let center = viewport.scene_point(world);
            let color = mapped_color(style.mapping, &traits, speed);
            self.draws
                .push(ThreeDDraw::TrueDot(TrueDotDraw::new(center, radius, color)));
            if style.look == Look::Glow {
                self.halos.push(ThreeDDraw::TrueDot(TrueDotDraw::glow(
                    center,
                    radius * halo_factor(traits.energy),
                    LinearRgba {
                        a: HALO_ALPHA,
                        ..color
                    },
                )));
            }
        }
        Ok(&self.draws)
    }

    /// Refit the current draws' radius to a new viewport. Scene points do
    /// not depend on the drawable, so only the pixel floor on the radius
    /// can change.
    pub fn refit(&mut self, viewport: &Viewport) {
        let radius = dot_radius(viewport);
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
