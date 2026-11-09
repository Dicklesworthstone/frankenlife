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
use flife_core::rng::{Address, Lane, LaneKind, SemanticRng, Stream};
use flife_core::{CoreError, ValidatedConfig};

use crate::catalog::TopologyCatalog;
use crate::draws::normals;
use crate::genome::{AFFINITY_TERMS, Genome, MOTION_RESPONSE_TERMS, TraitLimits, Traits, rescale};
use crate::init::initialize;
use crate::quota::GenomeQuota;

/// A founder's genome.
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
    let member = catalog.default_topology();
    let topology = catalog
        .topology(member)
        .ok_or_else(|| CoreError::LayoutMismatch {
            buffer: "genome parameters",
            detail: "the default topology is not in the catalog".into(),
        })?;
    let mut buffer = quota.allocate(catalog, member)?;
    let mut lane_error = None;
    let filled = initialize(
        topology,
        buffer.values_mut(),
        |weight| match Lane::neuron_index(weight.layer, weight.output, weight.input)
            .and_then(|index| Lane::new(0, LaneKind::Weight, index))
        {
            Ok(lane) => rng.block(address(organism, lane))[0],
            Err(error) => {
                lane_error.get_or_insert(error);
                0
            }
        },
    );
    if let Some(error) = lane_error.or_else(|| filled.err()) {
        quota.abandon(buffer)?;
        return Err(error);
    }
    Genome::seal(buffer, id, catalog, traits, None, limits).or_else(|unsealed| {
        quota.abandon(unsealed.buffer)?;
        Err(unsealed.error)
    })
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
