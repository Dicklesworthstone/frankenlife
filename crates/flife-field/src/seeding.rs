//! Seeded sources under `fnp-philox-semantic-v1` (FL-010; plan A.3 items 7
//! and 8, §16.2).
//!
//! FL-004 defined the deterministic arithmetic of `seeded-unit-vectors` and
//! `seeded-smooth-torus-v1` ([`crate::sources`]). This module supplies their
//! draws, each named by the source's stable identity, never by worker or
//! storage order:
//!
//! - **Chemical direction** of source `j`: 32 standard normals, where
//!   Box–Muller pair `p` comes from words 0 and 1 of
//!   `(source chemistry, j, 0, lane (direction, p), 0)` and gives
//!   components `2p` and `2p + 1`, normalized by [`unit_direction`].
//! - **Initial center** of source `j`: `(W u₀, H u₁)` with the half-open
//!   uniforms of words 0 and 1 of `(source placement, j, 0, lane
//!   (position, 0), 0)`, wrapped onto the torus.
//! - **Travel direction** `d_k` of source `j`: `(cos 2πu, sin 2πu)` with `u`
//!   the half-open uniform of word 0 of `(source path, j, k, lane
//!   (direction, 0), 0)`, uniform by angle.
//! - **Path**: segment `k` starts at the seeded center when `k = 0`, and
//!   otherwise at the previous segment's wrapped endpoint, and blends `d_k`
//!   into `d_(k+1)` ([`SeededPath`]).

use flife_core::ids::SourceId;
use flife_core::numerics::sin_cos_turns;
use flife_core::rng::{
    Address, Block, Lane, LaneKind, SemanticRng, Stream, box_muller, half_open_unit,
};
use flife_core::{CoreError, TickSpan, TimeStep};

use crate::grid::FieldGrid;
use crate::sources::{PathSegment, Source, unit_direction};
use crate::state::CHANNELS;

/// The chemical-direction family this module seeds.
pub const SEEDED_UNIT_VECTORS: &str = "seeded-unit-vectors";

/// The path family this module seeds.
pub const SEEDED_SMOOTH_TORUS_V1: &str = "seeded-smooth-torus-v1";

fn block(
    rng: &SemanticRng,
    stream: Stream,
    source: SourceId,
    ordinal: u64,
    kind: LaneKind,
    index: u64,
) -> Result<Block, CoreError> {
    Ok(rng.block(Address {
        stream,
        entity: u64::from(source.get()),
        ordinal,
        lane: Lane::new(0, kind, index)?,
        draw: 0,
    }))
}

/// Source `source`'s fixed chemical direction, `||a||₂ ≤ 1` exactly.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] if the draws have no direction, which
/// does not happen for 32 Box–Muller normals.
pub fn chemical_direction(
    rng: &SemanticRng,
    source: SourceId,
) -> Result<[f64; CHANNELS], CoreError> {
    let mut normals = [0.0; CHANNELS];
    for (pair, slots) in (0_u64..).zip(normals.chunks_mut(2)) {
        let [first, second, ..] = block(
            rng,
            Stream::SourceChemistry,
            source,
            0,
            LaneKind::Direction,
            pair,
        )?;
        for (slot, normal) in slots.iter_mut().zip(box_muller(first, second)?) {
            *slot = normal;
        }
    }
    unit_direction(&normals)
}

/// Source `source`'s initial center, uniform on the torus.
///
/// # Errors
/// As [`FieldGrid::wrap_position`].
pub fn initial_center(
    rng: &SemanticRng,
    grid: &FieldGrid,
    source: SourceId,
) -> Result<[f64; 2], CoreError> {
    let [first, second, ..] = block(
        rng,
        Stream::SourcePlacement,
        source,
        0,
        LaneKind::Position,
        0,
    )?;
    grid.wrap_position([
        grid.world_width() * half_open_unit(first),
        grid.world_height() * half_open_unit(second),
    ])
}

/// Source `source`'s travel direction `d_segment`, uniform by angle.
///
/// # Errors
/// None in practice; [`CoreError::NumericalInvalid`] guards the domain.
pub fn travel_direction(
    rng: &SemanticRng,
    source: SourceId,
    segment: u64,
) -> Result<[f64; 2], CoreError> {
    let [word, ..] = block(
        rng,
        Stream::SourcePath,
        source,
        segment,
        LaneKind::Direction,
        0,
    )?;
    let (sin, cos) =
        sin_cos_turns(half_open_unit(word)).ok_or_else(|| CoreError::NumericalInvalid {
            quantity: "source travel direction",
            detail: format!("word {word:#018x} left the unit interval"),
        })?;
    Ok([cos, sin])
}

/// Source `source` at its initial center with its seeded chemical direction
/// and total power `power`.
///
/// # Errors
/// As [`chemical_direction`], [`initial_center`] and [`Source::new`].
pub fn initial_source(
    rng: &SemanticRng,
    grid: &FieldGrid,
    source: SourceId,
    power: f64,
) -> Result<Source, CoreError> {
    Source::new(
        source,
        initial_center(rng, grid, source)?,
        chemical_direction(rng, source)?,
        power,
    )
}

/// One source's `seeded-smooth-torus-v1` path at segment `ordinal`. The
/// stored anchor and ordinal are all a checkpoint needs; within-segment time
/// comes from integer ticks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeededPath {
    source: SourceId,
    ordinal: u64,
    segment: PathSegment,
}

impl SeededPath {
    /// Segment 0 of `source`'s path: from its initial center, blending `d_0`
    /// into `d_1` at `speed` over `ticks` ticks of `step`.
    ///
    /// # Errors
    /// As [`PathSegment::new`] and the draws.
    pub fn start(
        rng: &SemanticRng,
        grid: &FieldGrid,
        source: SourceId,
        speed: f64,
        ticks: TickSpan,
        step: TimeStep,
    ) -> Result<Self, CoreError> {
        Ok(Self {
            source,
            ordinal: 0,
            segment: PathSegment::new(
                grid,
                initial_center(rng, grid, source)?,
                travel_direction(rng, source, 0)?,
                travel_direction(rng, source, 1)?,
                speed,
                ticks,
                step,
            )?,
        })
    }

    /// Segment `ordinal` of `source`'s path, from the wrapped `anchor`:
    /// what [`SeededPath::start`] and `ordinal` calls of
    /// [`SeededPath::next`] give when that segment starts at `anchor`. A
    /// checkpoint stores only the ordinal and the anchor.
    ///
    /// # Errors
    /// [`CoreError::Overflow`] at the last ordinal; otherwise as
    /// [`PathSegment::new`] and the draws.
    #[expect(
        clippy::too_many_arguments,
        reason = "the segment's identity, anchor and the path's three constants"
    )]
    pub fn resume(
        rng: &SemanticRng,
        grid: &FieldGrid,
        source: SourceId,
        ordinal: u64,
        anchor: [f64; 2],
        speed: f64,
        ticks: TickSpan,
        step: TimeStep,
    ) -> Result<Self, CoreError> {
        let following = ordinal.checked_add(1).ok_or(CoreError::Overflow {
            quantity: "source path segment",
        })?;
        Ok(Self {
            source,
            ordinal,
            segment: PathSegment::new(
                grid,
                anchor,
                travel_direction(rng, source, ordinal)?,
                travel_direction(rng, source, following)?,
                speed,
                ticks,
                step,
            )?,
        })
    }

    /// The source.
    #[must_use]
    pub const fn source(&self) -> SourceId {
        self.source
    }

    /// The segment ordinal `k`.
    #[must_use]
    pub const fn ordinal(&self) -> u64 {
        self.ordinal
    }

    /// The current segment.
    #[must_use]
    pub const fn segment(&self) -> &PathSegment {
        &self.segment
    }

    /// Segment `k + 1`: from this segment's wrapped endpoint, blending
    /// `d_(k+1)` into `d_(k+2)`.
    ///
    /// # Errors
    /// [`CoreError::Overflow`] past the last ordinal; otherwise as
    /// [`PathSegment::next`].
    pub fn next(
        &self,
        rng: &SemanticRng,
        grid: &FieldGrid,
        step: TimeStep,
    ) -> Result<Self, CoreError> {
        let ordinal = self.ordinal.checked_add(1).ok_or(CoreError::Overflow {
            quantity: "source path segment",
        })?;
        let following = ordinal.checked_add(1).ok_or(CoreError::Overflow {
            quantity: "source path segment",
        })?;
        Ok(Self {
            source: self.source,
            ordinal,
            segment: self.segment.next(
                grid,
                travel_direction(rng, self.source, following)?,
                step,
            )?,
        })
    }
}
