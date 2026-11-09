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
//! never allocates. Sizes and colours here are provisional: one dot radius,
//! and a fixed categorical lineage palette. FL-019 owns the visual mappings.

use flife_core::CoreError;
use flife_render::Epoch;
use fmn_core::color::{LinearRgba, Srgb};
use fmn_render::{ThreeDDraw, TrueDotDraw};

use crate::Viewport;

/// Dot radius in world units.
pub const DOT_RADIUS: f64 = 0.35;

/// The smallest dot radius in drawable pixels: a world too large for its
/// window still shows every organism.
pub const DOT_RADIUS_MIN_PIXELS: f64 = 1.5;

/// The provisional lineage palette (sRGB), indexed by [`lineage_slot`].
pub const LINEAGE_PALETTE: [[u8; 3]; 12] = [
    [0x4E, 0x79, 0xA7],
    [0xF2, 0x8E, 0x2B],
    [0xE1, 0x57, 0x59],
    [0x76, 0xB7, 0xB2],
    [0x59, 0xA1, 0x4F],
    [0xED, 0xC9, 0x48],
    [0xB0, 0x7A, 0xA1],
    [0xFF, 0x9D, 0xA7],
    [0x9C, 0x75, 0x5F],
    [0xBA, 0xB0, 0xAC],
    [0x86, 0xBC, 0xFF],
    [0xC5, 0xE0, 0x6B],
];

/// The palette slot of a lineage: a fixed SplitMix64 mix of its identity,
/// so neighbouring founder identities get unrelated colours and a lineage
/// keeps its colour for the whole run.
#[must_use]
pub fn lineage_slot(lineage: u64) -> usize {
    let mut z = lineage.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    let slots = LINEAGE_PALETTE.len() as u64;
    usize::try_from(z % slots).unwrap_or(0)
}

/// The linear-light colour of a lineage.
#[must_use]
pub fn lineage_color(lineage: u64) -> LinearRgba {
    let [r, g, b] = LINEAGE_PALETTE[lineage_slot(lineage)];
    Srgb::from_rgb8(r, g, b).to_linear(1.0)
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
    capacity: usize,
}

impl ParticleCloud {
    /// A cloud for up to `capacity` organisms, its draw buffer reserved now.
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when the buffer cannot be reserved.
    pub fn with_capacity(capacity: usize) -> Result<Self, CoreError> {
        let mut draws = Vec::new();
        draws
            .try_reserve_exact(capacity)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "particle cloud draws",
                bytes: u64::try_from(capacity.saturating_mul(size_of::<ThreeDDraw<'static>>()))
                    .unwrap_or(u64::MAX),
            })?;
        Ok(Self { draws, capacity })
    }

    /// A cloud sized for `epoch`'s population ceiling.
    ///
    /// # Errors
    /// As [`ParticleCloud::with_capacity`].
    pub fn for_epoch(epoch: &Epoch) -> Result<Self, CoreError> {
        Self::with_capacity(epoch.capacity())
    }

    /// Lower `epoch` through `viewport`: one dot per row, in identity order,
    /// centred on the row's scene point with the lineage colour. Nothing is
    /// allocated.
    ///
    /// # Errors
    /// - [`CoreError::BudgetExhausted`] when the epoch has more rows than
    ///   the cloud's capacity. The previous draws are kept.
    /// - [`CoreError::LayoutMismatch`] when the epoch's world is not the
    ///   viewport's. The previous draws are kept.
    pub fn update(
        &mut self,
        epoch: &Epoch,
        viewport: &Viewport,
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
        self.draws.clear();
        for (position, &lineage) in epoch.positions().iter().zip(epoch.lineages()) {
            let world = [f64::from(position[0]), f64::from(position[1])];
            self.draws.push(ThreeDDraw::TrueDot(TrueDotDraw::new(
                viewport.scene_point(world),
                radius,
                lineage_color(lineage),
            )));
        }
        Ok(&self.draws)
    }

    /// Refit the current draws' radius to a new viewport. Scene points do
    /// not depend on the drawable, so only the pixel floor on the radius
    /// can change.
    pub fn refit(&mut self, viewport: &Viewport) {
        let radius = dot_radius(viewport);
        for draw in &mut self.draws {
            if let ThreeDDraw::TrueDot(dot) = draw {
                dot.radius = radius;
            }
        }
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
            self.draws
                .capacity()
                .saturating_mul(size_of::<ThreeDDraw<'static>>()),
        )
        .unwrap_or(u64::MAX)
    }
}
