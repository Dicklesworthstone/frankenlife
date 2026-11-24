//! Founders' genomes from semantic randomness (plan §7.2, A.3 items 1–3).
//!
//! A founder takes the catalog's default topology. Each weight is
//! `fan-in-scaled-symmetric-v1` of word 0 of its own address
//! `(founders, organism, 0, weight lane (layer, output, input), 0)`, and
//! every bias is zero. The affinity direction normalizes 32 standard
//! normals, from Box–Muller pairs `j = 0..16` on lanes `(affinity, j)`, to
//! the sphere of radius `min(1, affinity_norm_limit)`. The motion response
//! does the same with the first 33 of 17 pairs on lanes
//! `(motion response, j)`, to radius `motion_response_norm_limit`. Pair `j`
//! gives components `2j` and `2j + 1`. The sensing scale is the configured
//! initial scale.
//!
//! Nothing depends on the order in which founders are made or on which
//! worker makes them: every value is a function of the world seed and the
//! founder's stable organism identity.

use flife_core::ids::{GenomeId, OrganismId};
use flife_core::rng::{Address, Lane, LaneKind, SemanticRng, Stream, half_open_unit};
use flife_core::{CoreError, ValidatedConfig};

use crate::catalog::{TopologyCatalog, TopologyId};
use crate::draws::normals;
use crate::genome::{
    AFFINITY_TERMS, CADHERIN_TERMS, CadherinTraits, Genome, MOTION_RESPONSE_TERMS, PolarityTraits,
    TraitLimits, Traits, Unsealed, rescale,
};
use crate::init::{WeightAddress, initialize};
use crate::quota::{GenomeQuota, ParameterBuffer, WeightsMut};

/// Index on [`LaneKind::Direction`] (`0x0800`) for initializing and mutating
/// a `V2-08` `pcp-polarity` strength `α_pol ∈ [0, 1]`.
pub const POLARITY_STRENGTH_LANE: u64 = 0x0800;

/// Index on [`LaneKind::Direction`] (`0x0801`) for initializing and mutating
/// a `V2-08` `pcp-polarity` spontaneous apical wedge curvature `β ∈ [-β_max, β_max]`.
pub const POLARITY_WEDGE_LANE: u64 = 0x0801;

/// Base index on [`LaneKind::Direction`] for the two Box–Muller pairs
/// (`0x1300`, `0x1301`) that initialize and mutate a `V2-13` 4-D cadherin tag.
pub const CADHERIN_TAG_LANE_BASE: u64 = 0x1300;

/// Index on [`LaneKind::Direction`] (`0x1310`) for initializing and mutating a
/// `V2-13` cadherin cohesiveness scalar `s_i ∈ [0, 1]`.
pub const CADHERIN_COHESIVENESS_LANE: u64 = 0x1310;

/// A founder's genome: its traits ([`founder_traits`]), a buffer for
/// [`founder_topology`] from `quota`, and [`grow_founder`] in it.
///
/// # Errors
/// As [`GenomeQuota::allocate`] when the quota refuses the genome, and
/// [`CoreError::NumericalInvalid`] or [`CoreError::Overflow`] if a draw or
/// lane is out of range. A founder that fails to seal is abandoned and its
/// error returned; nothing stays charged.
pub fn founder(
    rng: &SemanticRng,
    organism: OrganismId,
    id: GenomeId,
    catalog: &TopologyCatalog,
    quota: &mut GenomeQuota,
    limits: &TraitLimits,
    initial_scale: f64,
) -> Result<Genome, CoreError> {
    let traits = founder_traits(rng, organism, limits, initial_scale)?;
    let member = founder_topology(catalog)?;
    let buffer = quota.allocate(catalog, member)?;
    grow_founder(rng, organism, id, catalog, buffer, traits, limits).or_else(|unsealed| {
        quota.abandon(unsealed.buffer)?;
        Err(unsealed.error)
    })
}

/// The member every founder takes: the catalog's default topology.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] if the catalog does not hold it.
pub fn founder_topology(catalog: &TopologyCatalog) -> Result<TopologyId, CoreError> {
    let member = catalog.default_topology();
    catalog
        .topology(member)
        .map(|_| member)
        .ok_or_else(|| CoreError::LayoutMismatch {
            buffer: "genome parameters",
            detail: "the default topology is not in the catalog".into(),
        })
}

/// [`founder`] after its allocation: `organism`'s weights drawn into
/// `buffer`, which a quota allocated for [`founder_topology`], and the
/// buffer sealed as genome `id` with `traits` ([`founder_traits`]). It
/// takes no quota, so founders' buffers can be allocated in one place and
/// filled on several workers.
///
/// # Errors
/// [`Unsealed`], holding the buffer for its quota to abandon, with
/// [`CoreError::NumericalInvalid`] or [`CoreError::Overflow`] if a draw or
/// lane is out of range, or [`Genome::seal`]'s refusal.
pub fn grow_founder(
    rng: &SemanticRng,
    organism: OrganismId,
    id: GenomeId,
    catalog: &TopologyCatalog,
    mut buffer: ParameterBuffer,
    traits: Traits,
    limits: &TraitLimits,
) -> Result<Genome, Unsealed> {
    let Some(topology) = catalog.topology(buffer.topology()) else {
        return Err(Unsealed {
            error: CoreError::LayoutMismatch {
                buffer: "genome parameters",
                detail: "the buffer's topology is not in the catalog".into(),
            },
            buffer,
        });
    };
    let mut lane_error = None;
    let word =
        |weight: WeightAddress| match Lane::neuron_index(weight.layer, weight.output, weight.input)
            .and_then(|index| Lane::new(0, LaneKind::Weight, index))
        {
            Ok(lane) => rng.block(address(organism, lane))[0],
            Err(error) => {
                lane_error.get_or_insert(error);
                0
            }
        };
    let filled = match buffer.values_mut() {
        WeightsMut::F32(values) => initialize(topology, values, word),
        WeightsMut::F16(values) => initialize(topology, values, word),
    };
    if let Some(error) = lane_error.or_else(|| filled.err()) {
        return Err(Unsealed { error, buffer });
    }
    Genome::seal(buffer, id, catalog, traits, None, limits)
}

/// A founder's affinity direction, motion response and sensing scale.
///
/// # Errors
/// As [`founder`].
pub fn founder_traits(
    rng: &SemanticRng,
    organism: OrganismId,
    limits: &TraitLimits,
    initial_scale: f64,
) -> Result<Traits, CoreError> {
    let at = |lane| address(organism, lane);
    let mut affinity = [0.0; AFFINITY_TERMS];
    normals(rng, at, 0, LaneKind::Affinity, &mut affinity)?;
    rescale(
        "affinity direction",
        &mut affinity,
        limits.affinity_norm().min(1.0),
    )?;
    let mut motion_response = [0.0; MOTION_RESPONSE_TERMS];
    normals(rng, at, 0, LaneKind::MotionResponse, &mut motion_response)?;
    rescale(
        "motion response",
        &mut motion_response,
        limits.motion_response_norm(),
    )?;
    Traits::new(affinity, motion_response, initial_scale, limits)
}

/// A founder's genome with `V2-08` polarity and/or `V2-13` cadherin trait
/// initialization governed by `limits` ([`founder_traits_v2`]). Bit-identical
/// to [`founder`] with zero extra Philox draws when neither v2 trait feature
/// is enabled.
///
/// # Errors
/// As [`founder`].
pub fn founder_v2(
    rng: &SemanticRng,
    organism: OrganismId,
    id: GenomeId,
    catalog: &TopologyCatalog,
    quota: &mut GenomeQuota,
    limits: &TraitLimits,
    initial_scale: f64,
) -> Result<Genome, CoreError> {
    let traits = founder_traits_v2(rng, organism, limits, initial_scale)?;
    let member = founder_topology(catalog)?;
    let buffer = quota.allocate(catalog, member)?;
    grow_founder(rng, organism, id, catalog, buffer, traits, limits).or_else(|unsealed| {
        quota.abandon(unsealed.buffer)?;
        Err(unsealed.error)
    })
}

/// A founder's `V2-08` apical-basal polarity strength `α_pol ∈ [0, initial_strength_max]`
/// and wedge curvature `β ∈ [-maximum_wedge_curvature, maximum_wedge_curvature]`
/// from semantic Philox lanes `(Stream::Founders, organism, 0, Direction, 0x0800)`
/// and `(Stream::Founders, organism, 0, Direction, 0x0801)`.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] or [`CoreError::Overflow`] if a bound, draw,
/// or lane is out of range.
pub fn founder_polarity_traits(
    rng: &SemanticRng,
    organism: OrganismId,
    initial_strength_max: f64,
    maximum_wedge_curvature: f64,
) -> Result<PolarityTraits, CoreError> {
    if !initial_strength_max.is_finite() || !(0.0..=1.0).contains(&initial_strength_max) {
        return Err(CoreError::NumericalInvalid {
            quantity: "v2.polarity.initial_polarity_strength_max",
            detail: format!("{initial_strength_max} is outside [0, 1]"),
        });
    }
    let strength_lane = Lane::new(0, LaneKind::Direction, POLARITY_STRENGTH_LANE)?;
    let u_strength = half_open_unit(rng.block(address(organism, strength_lane))[0]);
    let polarity_strength = (u_strength * initial_strength_max).clamp(0.0, initial_strength_max);

    let wedge_lane = Lane::new(0, LaneKind::Direction, POLARITY_WEDGE_LANE)?;
    let u_wedge = half_open_unit(rng.block(address(organism, wedge_lane))[0]);
    let wedge_curvature = if maximum_wedge_curvature == 0.0 {
        0.0
    } else {
        ((2.0 * u_wedge - 1.0) * maximum_wedge_curvature)
            .clamp(-maximum_wedge_curvature, maximum_wedge_curvature)
    };
    PolarityTraits::new(polarity_strength, wedge_curvature, maximum_wedge_curvature)
}

/// A founder's `V2-13` 4-D cadherin unit tag and cohesiveness from semantic
/// Philox lanes `(Stream::Founders, organism, 0, Direction, 0x1300..0x1301)`
/// and `(Stream::Founders, organism, 0, Direction, 0x1310)`.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] or [`CoreError::Overflow`] if a draw or
/// lane is out of range.
pub fn founder_cadherin_traits(
    rng: &SemanticRng,
    organism: OrganismId,
) -> Result<CadherinTraits, CoreError> {
    let mut tag = [0.0; CADHERIN_TERMS];
    for (pair, slots) in (0_u64..).zip(tag.chunks_mut(2)) {
        let lane = Lane::new(0, LaneKind::Direction, CADHERIN_TAG_LANE_BASE + pair)?;
        let [first, second, ..] = rng.block(address(organism, lane));
        for (slot, normal) in slots
            .iter_mut()
            .zip(flife_core::rng::box_muller(first, second)?)
        {
            *slot = normal;
        }
    }
    let lane = Lane::new(0, LaneKind::Direction, CADHERIN_COHESIVENESS_LANE)?;
    let cohesiveness = half_open_unit(rng.block(address(organism, lane))[0]);
    CadherinTraits::normalized(tag, cohesiveness)
}

/// A founder's traits including `V2-08` polarity when
/// `limits.polarity_enabled()` and/or `V2-13` cadherin when
/// `limits.cadherin_enabled()`. Bit-identical to [`founder_traits`] with zero
/// extra Philox draws for any disabled feature.
///
/// # Errors
/// As [`founder_traits`], [`founder_polarity_traits`], and
/// [`founder_cadherin_traits`].
pub fn founder_traits_v2(
    rng: &SemanticRng,
    organism: OrganismId,
    limits: &TraitLimits,
    initial_scale: f64,
) -> Result<Traits, CoreError> {
    let mut traits = founder_traits(rng, organism, limits, initial_scale)?;
    if limits.polarity_enabled() {
        let polarity = founder_polarity_traits(
            rng,
            organism,
            limits.initial_polarity_strength_max(),
            limits.maximum_wedge_curvature(),
        )?;
        traits = traits.with_polarity_traits(Some(polarity), limits)?;
    }
    if limits.cadherin_enabled() {
        let cadherin = founder_cadherin_traits(rng, organism)?;
        traits = traits.with_cadherin_traits(Some(cadherin))?;
    }
    Ok(traits)
}

/// The configured initial sensing scale (`sensors.initial_scale_world_units`).
#[must_use]
pub fn initial_scale(config: &ValidatedConfig) -> f64 {
    config.config().sensors.initial_scale_world_units
}

fn address(organism: OrganismId, lane: Lane) -> Address {
    Address {
        stream: Stream::Founders,
        entity: organism.get(),
        ordinal: 0,
        lane,
        draw: 0,
    }
}
