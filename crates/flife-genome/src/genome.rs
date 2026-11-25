//! Immutable genomes (plan §6.5, §7.1, §9.2).
//!
//! A genome is one organism's own heritable state: its catalog topology and
//! F32 controller parameters, its unit affinity direction, its 33-term
//! motion response and its sensing scale, with its parent's identity. It is
//! validated once, when it is sealed, and never changes afterwards. A child
//! is always a new genome; nothing is shared between organisms except the
//! shape of a topology.
//!
//! Traits are stored in F64, the precision the organism-phase operators
//! read (`flife_engine::Affinity`, `flife_engine::MotionResponse`), and
//! checked with the same norm bounds. Parameters are behind a shared,
//! immutable buffer, so render, inspection and checkpoint readers can keep
//! a dead genome's parameters alive without copying them (§9.2).

use std::sync::Arc;

use flife_core::checkpoint::{ByteReader, ByteWriter, ChunkWriter};
use flife_core::ids::GenomeId;
use flife_core::numerics::shrink_to_left_fold_norm_limit;
use flife_core::{CoreError, Sha256Stream, V2Feature, ValidatedConfig};
use flife_neural::{ReferenceController, Weights};

use crate::catalog::{TopologyCatalog, TopologyId};
use crate::quota::{ParameterBuffer, ParameterValues};

/// Chemical channels, and so affinity components.
pub const AFFINITY_TERMS: usize = 32;

/// Motion-response terms: energy and every channel.
pub const MOTION_RESPONSE_TERMS: usize = 1 + AFFINITY_TERMS;

/// Components of a decoupled 4-D cadherin adhesion tag (`V2-13`, `C023`).
pub const CADHERIN_TERMS: usize = 4;

/// Additional checkpoint bytes per genome when `V2-08` `pcp-polarity` is
/// enabled: `polarity_strength` (`f64`) and `wedge_curvature` (`f64`).
pub const POLARITY_CHECKPOINT_BYTES: usize = 2 * 8;

/// Hard physical upper bound on `maximum_wedge_curvature` in radians (`V2-08`,
/// `C003`): prevents 3-cell pinwheel traps while allowing hollow cysts.
pub const MAXIMUM_WEDGE_CURVATURE_CAP: f64 = 0.4;

/// Additional checkpoint bytes per genome when `V2-13` `cadherin-4d` is
/// enabled: four `f64` tag components and one `f64` cohesiveness scalar.
pub const CADHERIN_CHECKPOINT_BYTES: usize = (CADHERIN_TERMS + 1) * 8;

/// Fixed-part bytes of one baseline genome checkpoint record before its
/// parameter count: `GenomeId` (8) + topology index (8) + `Option<GenomeId>`
/// parent (9) + `affinity` (32×8) + `motion_response` (33×8) +
/// `sensing_scale` (8).
pub const BASELINE_GENOME_HEADER_BYTES: usize =
    8 + 8 + 9 + 8 * (AFFINITY_TERMS + MOTION_RESPONSE_TERMS) + 8;

/// Extra checkpoint bytes per genome for `V2-08` `pcp-polarity`: `0` when
/// disabled, [`POLARITY_CHECKPOINT_BYTES`] (`16`) when enabled.
#[must_use]
pub const fn polarity_checkpoint_bytes(polarity_enabled: bool) -> usize {
    if polarity_enabled {
        POLARITY_CHECKPOINT_BYTES
    } else {
        0
    }
}

/// Extra checkpoint bytes per genome for `V2-13` `cadherin-4d`: `0` when
/// disabled, [`CADHERIN_CHECKPOINT_BYTES`] (`40`) when enabled.
#[must_use]
pub const fn cadherin_checkpoint_bytes(cadherin_enabled: bool) -> usize {
    if cadherin_enabled {
        CADHERIN_CHECKPOINT_BYTES
    } else {
        0
    }
}

/// Fixed-part bytes of one genome record before its parameter count in a
/// checkpoint stream (`GENOME_HEADER_BYTES`) when only `cadherin-4d` varies
/// (`0` extra bytes when disabled, `+40` bytes when enabled).
#[must_use]
pub const fn genome_header_bytes(cadherin_enabled: bool) -> usize {
    genome_header_bytes_v2(false, cadherin_enabled)
}

/// Fixed-part bytes of one genome record before its parameter count in a
/// checkpoint stream (`GENOME_HEADER_BYTES`), sized dynamically for active
/// v2 trait extensions: `553` baseline, `+16` when `pcp-polarity` is enabled,
/// `+40` when `cadherin-4d` is enabled (`0` extra bytes for any disabled
/// feature).
#[must_use]
pub const fn genome_header_bytes_v2(polarity_enabled: bool, cadherin_enabled: bool) -> usize {
    BASELINE_GENOME_HEADER_BYTES
        + polarity_checkpoint_bytes(polarity_enabled)
        + cadherin_checkpoint_bytes(cadherin_enabled)
}

/// Sentinel stored in [`Traits::polarity_strength`] when `V2-08` `pcp-polarity`
/// is absent. Valid polarity strength always lies in `[0.0, 1.0]`, so `-1.0`
/// is an unambiguous, reflexive (`PartialEq`-safe) absence marker with zero
/// tag padding beyond the 16-byte polarity payload.
const NO_POLARITY_STRENGTH: f64 = -1.0;

/// Sentinel stored in [`Traits::cohesiveness`] when `V2-13` `cadherin-4d` is
/// absent. Valid cohesiveness always lies in `[0.0, 1.0]`, so `-1.0` is an
/// unambiguous, reflexive (`PartialEq`-safe) absence marker with zero tag
/// padding beyond the 40-byte cadherin payload.
const NO_CADHERIN_COHESIVENESS: f64 = -1.0;

/// Heritable apical-basal PCP polarity traits (`V2-08`, `C003`).
///
/// - `polarity_strength`: finite scalar `α_pol ∈ [0.0, 1.0]` (`0.0` = isotropic
///   adhesion, `1.0` = strictly lateral epithelial belt).
/// - `wedge_curvature`: finite scalar `β ∈ [-β_max, β_max]` (spontaneous apical
///   wedge angle in radians, with `β_max <= 0.4` rad).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolarityTraits {
    polarity_strength: f64,
    wedge_curvature: f64,
}

impl PolarityTraits {
    /// Validate and construct polarity traits with `polarity_strength ∈ [0, 1]`
    /// and `wedge_curvature ∈ [-maximum_wedge_curvature, maximum_wedge_curvature]`
    /// (`0 <= maximum_wedge_curvature <= 0.4`).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for any non-finite value, a
    /// `maximum_wedge_curvature` outside `[0.0, 0.4]`, a `polarity_strength`
    /// outside `[0.0, 1.0]`, or a `wedge_curvature` outside
    /// `[-maximum_wedge_curvature, maximum_wedge_curvature]`.
    pub fn new(
        polarity_strength: f64,
        wedge_curvature: f64,
        maximum_wedge_curvature: f64,
    ) -> Result<Self, CoreError> {
        check_maximum_wedge_curvature(maximum_wedge_curvature)?;
        check_polarity_strength(polarity_strength)?;
        check_wedge_curvature(wedge_curvature, maximum_wedge_curvature)?;
        Ok(Self {
            polarity_strength,
            wedge_curvature,
        })
    }

    /// Heritable lateral-belt adhesion strength `α_pol ∈ [0.0, 1.0]`.
    #[must_use]
    pub const fn polarity_strength(&self) -> f64 {
        self.polarity_strength
    }

    /// Heritable spontaneous apical wedge curvature `β ∈ [-β_max, β_max]`.
    #[must_use]
    pub const fn wedge_curvature(&self) -> f64 {
        self.wedge_curvature
    }

    /// Feed canonical little-endian bit patterns of `polarity_strength` and
    /// `wedge_curvature` into `hasher` for `Population::digest`.
    pub fn hash_into(&self, hasher: &mut Sha256Stream) {
        hasher.update(&self.polarity_strength.to_bits().to_le_bytes());
        hasher.update(&self.wedge_curvature.to_bits().to_le_bytes());
    }

    /// Encode `polarity_strength` and `wedge_curvature` into a streaming
    /// checkpoint chunk (`16` bytes).
    pub fn write_checkpoint(&self, out: &mut ChunkWriter<'_>) {
        out.f64(self.polarity_strength);
        out.f64(self.wedge_curvature);
    }

    /// Encode `polarity_strength` and `wedge_curvature` into a [`ByteWriter`]
    /// (`16` bytes).
    pub fn write_bytes(&self, out: &mut ByteWriter) {
        out.f64(self.polarity_strength);
        out.f64(self.wedge_curvature);
    }

    /// Decode and validate `polarity_strength` and `wedge_curvature` from a
    /// checkpoint stream (`16` bytes).
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] if truncated or if decoded values
    /// violate polarity strength or wedge curvature bounds.
    pub fn read_checkpoint(
        input: &mut ByteReader<'_>,
        maximum_wedge_curvature: f64,
    ) -> Result<Self, CoreError> {
        let polarity_strength = input.f64()?;
        let wedge_curvature = input.f64()?;
        Self::new(polarity_strength, wedge_curvature, maximum_wedge_curvature).map_err(|error| {
            CoreError::IncompatibleCheckpoint {
                reason: format!("polarity traits: {error}"),
            }
        })
    }
}

/// Decoupled 4-D cadherin adhesion tag and cohesiveness (`V2-13`, `C023`).
///
/// - `tag`: finite 4-D direction with `0 < left_fold_sq(tag) <= 1.0`,
///   strictly guarded against `‖c‖ == 0` and exact left-fold unit-ball safe
///   via [`shrink_to_left_fold_norm_limit`].
/// - `cohesiveness`: finite scalar `s_i ∈ [0.0, 1.0]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CadherinTraits {
    tag: [f64; CADHERIN_TERMS],
    cohesiveness: f64,
}

impl CadherinTraits {
    /// Validate and construct cadherin traits from a unit-ball 4-D `tag`
    /// (`0 < left_fold_sq(tag) <= 1.0`) and `cohesiveness ∈ [0.0, 1.0]`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a non-finite value, a zero-norm
    /// tag (`‖c‖ == 0`), a tag whose left-fold squared norm exceeds `1.0`,
    /// or a cohesiveness outside `[0.0, 1.0]`.
    pub fn new(tag: [f64; CADHERIN_TERMS], cohesiveness: f64) -> Result<Self, CoreError> {
        check_cadherin_tag(&tag)?;
        check_cohesiveness(cohesiveness)?;
        Ok(Self { tag, cohesiveness })
    }

    /// Normalize a non-zero finite 4-D `tag` onto the exact left-fold unit
    /// ball via [`shrink_to_left_fold_norm_limit`] and validate `cohesiveness`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] when `tag` has zero or non-finite norm,
    /// cannot be shrunk into the left-fold unit ball, or `cohesiveness` lies
    /// outside `[0.0, 1.0]`.
    pub fn normalized(
        mut tag: [f64; CADHERIN_TERMS],
        cohesiveness: f64,
    ) -> Result<Self, CoreError> {
        normalize_cadherin_tag(&mut tag)?;
        Self::new(tag, cohesiveness)
    }

    /// The unit 4-D cadherin tag `c_i`.
    #[must_use]
    pub const fn tag(&self) -> &[f64; CADHERIN_TERMS] {
        &self.tag
    }

    /// The cohesiveness scalar `s_i ∈ [0.0, 1.0]`.
    #[must_use]
    pub const fn cohesiveness(&self) -> f64 {
        self.cohesiveness
    }

    /// Symmetric pair adhesion weight `min(s_i, s_j) * max(0, cos(c_i, c_j))²`
    /// (`V2-13`, `C023`), bit-identical under `(self, other) <-> (other, self)`.
    #[must_use]
    pub fn pair_adhesion_weight(&self, other: &Self) -> f64 {
        let dot = (0..CADHERIN_TERMS).fold(0.0_f64, |acc, k| acc + self.tag[k] * other.tag[k]);
        let denom = squared_norm(&self.tag).sqrt() * squared_norm(&other.tag).sqrt();
        if !dot.is_finite() || !denom.is_finite() || denom <= 0.0 {
            return 0.0;
        }
        let cos = (dot / denom).clamp(-1.0, 1.0);
        if cos <= 0.0 {
            return 0.0;
        }
        self.cohesiveness.min(other.cohesiveness) * (cos * cos)
    }

    /// Feed canonical little-endian bit patterns of `tag` and `cohesiveness`
    /// into `hasher` for `Population::digest`.
    pub fn hash_into(&self, hasher: &mut Sha256Stream) {
        for component in self.tag {
            hasher.update(&component.to_bits().to_le_bytes());
        }
        hasher.update(&self.cohesiveness.to_bits().to_le_bytes());
    }

    /// Encode `tag` and `cohesiveness` into a streaming checkpoint chunk (`40` bytes).
    pub fn write_checkpoint(&self, out: &mut ChunkWriter<'_>) {
        for component in self.tag {
            out.f64(component);
        }
        out.f64(self.cohesiveness);
    }

    /// Encode `tag` and `cohesiveness` into a [`ByteWriter`] (`40` bytes).
    pub fn write_bytes(&self, out: &mut ByteWriter) {
        for component in self.tag {
            out.f64(component);
        }
        out.f64(self.cohesiveness);
    }

    /// Decode and validate `tag` and `cohesiveness` from a checkpoint stream (`40` bytes).
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] if truncated or if decoded values
    /// violate cadherin unit-ball / non-zero / cohesiveness invariants.
    pub fn read_checkpoint(input: &mut ByteReader<'_>) -> Result<Self, CoreError> {
        let mut tag = [0.0; CADHERIN_TERMS];
        for slot in &mut tag {
            *slot = input.f64()?;
        }
        let cohesiveness = input.f64()?;
        Self::new(tag, cohesiveness).map_err(|error| CoreError::IncompatibleCheckpoint {
            reason: format!("cadherin traits: {error}"),
        })
    }
}

/// The run's bounds on heritable values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraitLimits {
    affinity_norm: f64,
    motion_response_norm: f64,
    minimum_scale: f64,
    maximum_scale: f64,
    maximum_absolute_parameter: f64,
    polarity_enabled: bool,
    initial_polarity_strength_max: f64,
    maximum_wedge_curvature: f64,
    polarity_mutation_std: f64,
    cadherin_enabled: bool,
    cadherin_mutation_std: f64,
}

impl TraitLimits {
    /// Bounds `||a||₂ ≤ affinity_norm`, `||χ||₂ ≤ motion_response_norm`,
    /// `sensing scale ∈ [minimum_scale, maximum_scale]` and
    /// `|p| ≤ maximum_absolute_parameter`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonpositive or nonfinite bound,
    /// or a scale range out of order.
    pub fn new(
        affinity_norm: f64,
        motion_response_norm: f64,
        scale_range: [f64; 2],
        maximum_absolute_parameter: f64,
    ) -> Result<Self, CoreError> {
        let [minimum_scale, maximum_scale] = scale_range;
        for (quantity, value) in [
            ("chemistry.affinity_norm_limit", affinity_norm),
            ("chemistry.motion_response_norm_limit", motion_response_norm),
            ("sensors.minimum_scale_world_units", minimum_scale),
            ("sensors.maximum_scale_world_units", maximum_scale),
            (
                "controller.maximum_absolute_parameter",
                maximum_absolute_parameter,
            ),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(CoreError::NumericalInvalid {
                    quantity,
                    detail: format!("{value} is not a positive bound"),
                });
            }
        }
        if minimum_scale > maximum_scale {
            return Err(CoreError::NumericalInvalid {
                quantity: "sensors.maximum_scale_world_units",
                detail: format!("{maximum_scale} is below the minimum {minimum_scale}"),
            });
        }
        Ok(Self {
            affinity_norm,
            motion_response_norm,
            minimum_scale,
            maximum_scale,
            maximum_absolute_parameter,
            polarity_enabled: false,
            initial_polarity_strength_max: 0.0,
            maximum_wedge_curvature: 0.0,
            polarity_mutation_std: 0.0,
            cadherin_enabled: false,
            cadherin_mutation_std: 0.0,
        })
    }

    /// Enable `V2-08` `pcp-polarity` with founder strength ceiling
    /// `initial_strength_max ∈ [0.0, 1.0]`, wedge curvature bound
    /// `maximum_wedge_curvature ∈ [0.0, 0.4]`, and per-birth Gaussian
    /// standard deviation `mutation_std >= 0.0`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for any out-of-range or non-finite
    /// parameter.
    pub fn with_polarity(
        mut self,
        initial_strength_max: f64,
        maximum_wedge_curvature: f64,
        mutation_std: f64,
    ) -> Result<Self, CoreError> {
        if !initial_strength_max.is_finite() || !(0.0..=1.0).contains(&initial_strength_max) {
            return Err(CoreError::NumericalInvalid {
                quantity: "v2.polarity.initial_polarity_strength_max",
                detail: format!("{initial_strength_max} is outside [0, 1]"),
            });
        }
        check_maximum_wedge_curvature(maximum_wedge_curvature)?;
        if !mutation_std.is_finite() || mutation_std < 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "v2.polarity.polarity_mutation_std",
                detail: format!("{mutation_std} is not a non-negative finite standard deviation"),
            });
        }
        self.polarity_enabled = true;
        self.initial_polarity_strength_max = initial_strength_max;
        self.maximum_wedge_curvature = maximum_wedge_curvature;
        self.polarity_mutation_std = mutation_std;
        Ok(self)
    }

    /// Enable `V2-13` `cadherin-4d` with per-birth Gaussian standard deviation
    /// `mutation_std` (`>= 0.0`, finite).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a negative or non-finite
    /// `mutation_std`.
    pub fn with_cadherin(mut self, mutation_std: f64) -> Result<Self, CoreError> {
        if !mutation_std.is_finite() || mutation_std < 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "v2.contact.cadherin.mutation_std",
                detail: format!("{mutation_std} is not a non-negative finite standard deviation"),
            });
        }
        self.cadherin_enabled = true;
        self.cadherin_mutation_std = mutation_std;
        Ok(self)
    }

    /// The bounds of a validated configuration, including `V2-08`
    /// `pcp-polarity` and `V2-13` `cadherin-4d` enablement, bounds, and
    /// mutation standard deviations when active.
    ///
    /// # Errors
    /// As [`TraitLimits::new`], [`TraitLimits::with_polarity`], and
    /// [`TraitLimits::with_cadherin`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let model = config.config();
        let mut limits = Self::new(
            model.chemistry.affinity_norm_limit,
            model.chemistry.motion_response_norm_limit,
            [
                model.sensors.minimum_scale_world_units,
                model.sensors.maximum_scale_world_units,
            ],
            model.controller.maximum_absolute_parameter,
        )?;
        if let Some(v2) = model.v2.as_ref() {
            if v2.is_enabled(V2Feature::PcpPolarity) {
                limits = limits.with_polarity(
                    v2.polarity.initial_polarity_strength_max,
                    v2.polarity.maximum_wedge_curvature,
                    v2.polarity.polarity_mutation_std,
                )?;
            }
            if v2.is_enabled(V2Feature::Cadherin4d) {
                limits = limits.with_cadherin(v2.contact.cadherin.mutation_std)?;
            }
        }
        Ok(limits)
    }

    /// `maximum_absolute_parameter`.
    #[must_use]
    pub const fn maximum_absolute_parameter(&self) -> f64 {
        self.maximum_absolute_parameter
    }

    /// The bound on `||a||₂`.
    #[must_use]
    pub const fn affinity_norm(&self) -> f64 {
        self.affinity_norm
    }

    /// The bound on `||χ||₂`.
    #[must_use]
    pub const fn motion_response_norm(&self) -> f64 {
        self.motion_response_norm
    }

    /// The admitted sensing scales `[minimum, maximum]`, in world-length.
    #[must_use]
    pub const fn scale_range(&self) -> [f64; 2] {
        [self.minimum_scale, self.maximum_scale]
    }

    /// Whether `V2-08` `pcp-polarity` is enabled for this run.
    #[must_use]
    pub const fn polarity_enabled(&self) -> bool {
        self.polarity_enabled
    }

    /// The configured `v2.polarity.initial_polarity_strength_max` (`0.0` when
    /// disabled).
    #[must_use]
    pub const fn initial_polarity_strength_max(&self) -> f64 {
        self.initial_polarity_strength_max
    }

    /// The configured `v2.polarity.maximum_wedge_curvature` (`0.0` when
    /// disabled).
    #[must_use]
    pub const fn maximum_wedge_curvature(&self) -> f64 {
        self.maximum_wedge_curvature
    }

    /// The configured `v2.polarity.polarity_mutation_std` (`0.0` when
    /// disabled).
    #[must_use]
    pub const fn polarity_mutation_std(&self) -> f64 {
        self.polarity_mutation_std
    }

    /// Whether `V2-13` `cadherin-4d` is enabled for this run.
    #[must_use]
    pub const fn cadherin_enabled(&self) -> bool {
        self.cadherin_enabled
    }

    /// The configured `v2.contact.cadherin.mutation_std` (`0.0` when disabled).
    #[must_use]
    pub const fn cadherin_mutation_std(&self) -> f64 {
        self.cadherin_mutation_std
    }

    /// Dynamic checkpoint fixed-header byte length for genomes under these
    /// limits ([`genome_header_bytes_v2`]).
    #[must_use]
    pub const fn genome_header_bytes(&self) -> usize {
        genome_header_bytes_v2(self.polarity_enabled, self.cadherin_enabled)
    }

    /// Wedge-curvature validation cap applied when checking/resealing a
    /// [`PolarityTraits`]: the configured bound when `pcp-polarity` is
    /// enabled, or the global physical cap [`MAXIMUM_WEDGE_CURVATURE_CAP`]
    /// (`0.4`) when preserving carried polarity traits on a non-polarity host.
    pub(crate) const fn effective_wedge_curvature_bound(&self) -> f64 {
        if self.polarity_enabled {
            self.maximum_wedge_curvature
        } else {
            MAXIMUM_WEDGE_CURVATURE_CAP
        }
    }
}

/// The heritable traits other than the controller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Traits {
    affinity: [f64; AFFINITY_TERMS],
    unit_affinity: [f32; AFFINITY_TERMS],
    motion_response: [f64; MOTION_RESPONSE_TERMS],
    sensing_scale: f64,
    polarity_strength: f64,
    wedge_curvature: f64,
    cadherin_tag: [f64; CADHERIN_TERMS],
    cohesiveness: f64,
}

/// Compute the cached F32 unit affinity direction `a / ‖a‖₂` (all zeros when
/// `‖a‖₂ == 0` or nonfinite), bit-identical to `flife_engine::contact::unit_affinity`.
fn compute_unit_affinity(affinity: &[f64; AFFINITY_TERMS]) -> [f32; AFFINITY_TERMS] {
    let norm = affinity
        .iter()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    if norm.is_finite() && norm > 0.0 {
        affinity.map(|value| (value / norm) as f32)
    } else {
        [0.0; AFFINITY_TERMS]
    }
}

impl Traits {
    /// Traits within `limits`, with norms checked in F64 and no optional v2
    /// polarity or cadherin traits attached.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite value, a norm above
    /// its bound, or a sensing scale outside its range.
    pub fn new(
        affinity: [f64; AFFINITY_TERMS],
        motion_response: [f64; MOTION_RESPONSE_TERMS],
        sensing_scale: f64,
        limits: &TraitLimits,
    ) -> Result<Self, CoreError> {
        Self::with_v2_traits(affinity, motion_response, sensing_scale, None, None, limits)
    }

    /// Traits within `limits`, optionally carrying validated `V2-08`
    /// [`PolarityTraits`] (no cadherin traits attached).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite value, a norm above
    /// its bound, a sensing scale outside its range, or invalid polarity traits.
    pub fn with_polarity(
        affinity: [f64; AFFINITY_TERMS],
        motion_response: [f64; MOTION_RESPONSE_TERMS],
        sensing_scale: f64,
        polarity: Option<PolarityTraits>,
        limits: &TraitLimits,
    ) -> Result<Self, CoreError> {
        Self::with_v2_traits(
            affinity,
            motion_response,
            sensing_scale,
            polarity,
            None,
            limits,
        )
    }

    /// Traits within `limits`, optionally carrying validated `V2-13`
    /// [`CadherinTraits`] (no polarity traits attached).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite value, a norm above
    /// its bound, a sensing scale outside its range, or invalid cadherin traits.
    pub fn with_cadherin(
        affinity: [f64; AFFINITY_TERMS],
        motion_response: [f64; MOTION_RESPONSE_TERMS],
        sensing_scale: f64,
        cadherin: Option<CadherinTraits>,
        limits: &TraitLimits,
    ) -> Result<Self, CoreError> {
        Self::with_v2_traits(
            affinity,
            motion_response,
            sensing_scale,
            None,
            cadherin,
            limits,
        )
    }

    /// Traits within `limits`, optionally carrying validated `V2-08`
    /// [`PolarityTraits`] and/or `V2-13` [`CadherinTraits`].
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for any nonfinite value, out-of-range
    /// bound, or invalid polarity/cadherin traits.
    pub fn with_v2_traits(
        affinity: [f64; AFFINITY_TERMS],
        motion_response: [f64; MOTION_RESPONSE_TERMS],
        sensing_scale: f64,
        polarity: Option<PolarityTraits>,
        cadherin: Option<CadherinTraits>,
        limits: &TraitLimits,
    ) -> Result<Self, CoreError> {
        check_norm("affinity direction", &affinity, limits.affinity_norm)?;
        check_norm(
            "motion response",
            &motion_response,
            limits.motion_response_norm,
        )?;
        if !(limits.minimum_scale..=limits.maximum_scale).contains(&sensing_scale) {
            return Err(CoreError::NumericalInvalid {
                quantity: "sensing scale",
                detail: format!(
                    "{sensing_scale} is outside [{}, {}]",
                    limits.minimum_scale, limits.maximum_scale
                ),
            });
        }
        let (polarity_strength, wedge_curvature) = match polarity {
            Some(pol) => {
                let checked = PolarityTraits::new(
                    pol.polarity_strength,
                    pol.wedge_curvature,
                    limits.effective_wedge_curvature_bound(),
                )?;
                (checked.polarity_strength, checked.wedge_curvature)
            }
            None => (NO_POLARITY_STRENGTH, 0.0),
        };
        let (cadherin_tag, cohesiveness) = match cadherin {
            Some(cad) => {
                let checked = CadherinTraits::new(cad.tag, cad.cohesiveness)?;
                (checked.tag, checked.cohesiveness)
            }
            None => ([0.0; CADHERIN_TERMS], NO_CADHERIN_COHESIVENESS),
        };
        let unit_affinity = compute_unit_affinity(&affinity);
        Ok(Self {
            affinity,
            unit_affinity,
            motion_response,
            sensing_scale,
            polarity_strength,
            wedge_curvature,
            cadherin_tag,
            cohesiveness,
        })
    }

    /// Return a copy of `self` with optional `V2-08` [`PolarityTraits`]
    /// attached or cleared, re-validating `polarity` against `limits` when
    /// present.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] if `polarity` violates polarity strength
    /// or wedge curvature bounds.
    pub fn with_polarity_traits(
        mut self,
        polarity: Option<PolarityTraits>,
        limits: &TraitLimits,
    ) -> Result<Self, CoreError> {
        let (polarity_strength, wedge_curvature) = match polarity {
            Some(pol) => {
                let checked = PolarityTraits::new(
                    pol.polarity_strength,
                    pol.wedge_curvature,
                    limits.effective_wedge_curvature_bound(),
                )?;
                (checked.polarity_strength, checked.wedge_curvature)
            }
            None => (NO_POLARITY_STRENGTH, 0.0),
        };
        self.polarity_strength = polarity_strength;
        self.wedge_curvature = wedge_curvature;
        Ok(self)
    }

    /// Return a copy of `self` with optional `V2-13` [`CadherinTraits`]
    /// attached or cleared, re-validating `cadherin` when present.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] if `cadherin` violates cadherin bounds.
    pub fn with_cadherin_traits(
        mut self,
        cadherin: Option<CadherinTraits>,
    ) -> Result<Self, CoreError> {
        let (cadherin_tag, cohesiveness) = match cadherin {
            Some(cad) => {
                let checked = CadherinTraits::new(cad.tag, cad.cohesiveness)?;
                (checked.tag, checked.cohesiveness)
            }
            None => ([0.0; CADHERIN_TERMS], NO_CADHERIN_COHESIVENESS),
        };
        self.cadherin_tag = cadherin_tag;
        self.cohesiveness = cohesiveness;
        Ok(self)
    }

    /// The affinity direction `a`.
    #[must_use]
    pub const fn affinity(&self) -> &[f64; AFFINITY_TERMS] {
        &self.affinity
    }

    /// Cached F32 unit affinity direction `a / ‖a‖₂` (all zeros when `a == 0`),
    /// bit-identical to `flife_engine::contact::unit_affinity(self.affinity())`.
    #[must_use]
    pub const fn unit_affinity(&self) -> &[f32; AFFINITY_TERMS] {
        &self.unit_affinity
    }

    /// The motion response `χ`: energy first, then each channel.
    #[must_use]
    pub const fn motion_response(&self) -> &[f64; MOTION_RESPONSE_TERMS] {
        &self.motion_response
    }

    /// The sensing scale, in world-length.
    #[must_use]
    pub const fn sensing_scale(&self) -> f64 {
        self.sensing_scale
    }

    /// Whether `V2-08` `pcp-polarity` traits are present on this genome.
    #[must_use]
    pub const fn has_polarity(&self) -> bool {
        self.polarity_strength >= 0.0
    }

    /// The optional heritable polarity strength `α_pol ∈ [0, 1]` (`None` when
    /// disabled).
    #[must_use]
    pub const fn polarity_strength(&self) -> Option<f64> {
        if self.has_polarity() {
            Some(self.polarity_strength)
        } else {
            None
        }
    }

    /// The optional heritable apical wedge curvature `β ∈ [-β_max, β_max]`
    /// (`None` when disabled).
    #[must_use]
    pub const fn wedge_curvature(&self) -> Option<f64> {
        if self.has_polarity() {
            Some(self.wedge_curvature)
        } else {
            None
        }
    }

    /// Both `V2-08` polarity fields together (`None` when disabled).
    #[must_use]
    pub const fn polarity_traits(&self) -> Option<PolarityTraits> {
        if self.has_polarity() {
            Some(PolarityTraits {
                polarity_strength: self.polarity_strength,
                wedge_curvature: self.wedge_curvature,
            })
        } else {
            None
        }
    }

    /// Whether `V2-13` `cadherin-4d` traits are present on this genome.
    #[must_use]
    pub const fn has_cadherin(&self) -> bool {
        self.cohesiveness >= 0.0
    }

    /// The optional 4-D cadherin unit tag `c_i` (`None` when disabled).
    #[must_use]
    pub const fn cadherin(&self) -> Option<&[f64; CADHERIN_TERMS]> {
        if self.has_cadherin() {
            Some(&self.cadherin_tag)
        } else {
            None
        }
    }

    /// The optional cadherin cohesiveness `s_i ∈ [0, 1]` (`None` when disabled).
    #[must_use]
    pub const fn cohesiveness(&self) -> Option<f64> {
        if self.has_cadherin() {
            Some(self.cohesiveness)
        } else {
            None
        }
    }

    /// Both `V2-13` cadherin fields together (`None` when disabled).
    #[must_use]
    pub const fn cadherin_traits(&self) -> Option<CadherinTraits> {
        if self.has_cadherin() {
            Some(CadherinTraits {
                tag: self.cadherin_tag,
                cohesiveness: self.cohesiveness,
            })
        } else {
            None
        }
    }

    /// Serialized trait bytes in a checkpoint record (`528` baseline, `+16`
    /// with polarity, `+40` with cadherin, `584` with both).
    #[must_use]
    pub const fn checkpoint_bytes(&self) -> usize {
        8 * (AFFINITY_TERMS + MOTION_RESPONSE_TERMS)
            + 8
            + polarity_checkpoint_bytes(self.has_polarity())
            + cadherin_checkpoint_bytes(self.has_cadherin())
    }

    /// Encode baseline and optional cadherin trait fields into a streaming
    /// checkpoint chunk (`0` extra bytes when `include_cadherin == false`,
    /// `+40` bytes when `true`).
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] when `include_cadherin` is `true`
    /// but `self` carries no cadherin traits.
    pub fn write_checkpoint_fields(
        &self,
        out: &mut ChunkWriter<'_>,
        include_cadherin: bool,
    ) -> Result<(), CoreError> {
        self.write_checkpoint_fields_v2(out, false, include_cadherin)
    }

    /// Encode all active v2 trait fields into a streaming checkpoint chunk:
    /// baseline (`528` B), then optional `V2-08` polarity (`+16` B when
    /// `include_polarity`), then optional `V2-13` cadherin (`+40` B when
    /// `include_cadherin`). Writes `0` extra bytes for any disabled feature.
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] when a requested extension is
    /// absent on `self`.
    pub fn write_checkpoint_fields_v2(
        &self,
        out: &mut ChunkWriter<'_>,
        include_polarity: bool,
        include_cadherin: bool,
    ) -> Result<(), CoreError> {
        for value in self.affinity.iter().chain(&self.motion_response) {
            out.f64(*value);
        }
        out.f64(self.sensing_scale);
        if include_polarity {
            let polarity =
                self.polarity_traits()
                    .ok_or_else(|| CoreError::IncompatibleCheckpoint {
                        reason: "pcp-polarity checkpoint expects polarity traits on every genome"
                            .into(),
                    })?;
            polarity.write_checkpoint(out);
        }
        if include_cadherin {
            let cadherin =
                self.cadherin_traits()
                    .ok_or_else(|| CoreError::IncompatibleCheckpoint {
                        reason: "cadherin-4d checkpoint expects cadherin traits on every genome"
                            .into(),
                    })?;
            cadherin.write_checkpoint(out);
        }
        Ok(())
    }

    /// Encode baseline and optional cadherin trait fields into a [`ByteWriter`]
    /// (`0` extra bytes when `include_cadherin == false`, `+40` bytes when
    /// `true`).
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] when `include_cadherin` is `true`
    /// but `self` carries no cadherin traits.
    pub fn write_bytes(
        &self,
        out: &mut ByteWriter,
        include_cadherin: bool,
    ) -> Result<(), CoreError> {
        self.write_bytes_v2(out, false, include_cadherin)
    }

    /// Encode all active v2 trait fields into a [`ByteWriter`]: baseline
    /// (`528` B), then optional `V2-08` polarity (`+16` B), then optional
    /// `V2-13` cadherin (`+40` B). Writes `0` extra bytes for any disabled
    /// feature.
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] when a requested extension is
    /// absent on `self`.
    pub fn write_bytes_v2(
        &self,
        out: &mut ByteWriter,
        include_polarity: bool,
        include_cadherin: bool,
    ) -> Result<(), CoreError> {
        for value in self.affinity.iter().chain(&self.motion_response) {
            out.f64(*value);
        }
        out.f64(self.sensing_scale);
        if include_polarity {
            let polarity =
                self.polarity_traits()
                    .ok_or_else(|| CoreError::IncompatibleCheckpoint {
                        reason: "pcp-polarity checkpoint expects polarity traits on every genome"
                            .into(),
                    })?;
            polarity.write_bytes(out);
        }
        if include_cadherin {
            let cadherin =
                self.cadherin_traits()
                    .ok_or_else(|| CoreError::IncompatibleCheckpoint {
                        reason: "cadherin-4d checkpoint expects cadherin traits on every genome"
                            .into(),
                    })?;
            cadherin.write_bytes(out);
        }
        Ok(())
    }

    /// Decode and validate baseline and optional cadherin trait fields from a
    /// checkpoint [`ByteReader`]. Reads `0` extra bytes when
    /// `include_cadherin == false`, `+40` bytes when `true`.
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] on truncation or invalid trait values.
    pub fn read_checkpoint_fields(
        input: &mut ByteReader<'_>,
        limits: &TraitLimits,
        include_cadherin: bool,
    ) -> Result<Self, CoreError> {
        Self::read_checkpoint_fields_v2(input, limits, false, include_cadherin)
    }

    /// Decode and validate all active v2 trait fields from a checkpoint
    /// [`ByteReader`]: baseline (`528` B), then optional `V2-08` polarity
    /// (`+16` B), then optional `V2-13` cadherin (`+40` B). Reads `0` extra
    /// bytes for any disabled feature.
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] on truncation or invalid trait values.
    pub fn read_checkpoint_fields_v2(
        input: &mut ByteReader<'_>,
        limits: &TraitLimits,
        include_polarity: bool,
        include_cadherin: bool,
    ) -> Result<Self, CoreError> {
        let mut affinity = [0.0; AFFINITY_TERMS];
        for value in &mut affinity {
            *value = input.f64()?;
        }
        let mut motion_response = [0.0; MOTION_RESPONSE_TERMS];
        for value in &mut motion_response {
            *value = input.f64()?;
        }
        let sensing_scale = input.f64()?;
        let polarity = if include_polarity {
            Some(PolarityTraits::read_checkpoint(
                input,
                limits.effective_wedge_curvature_bound(),
            )?)
        } else {
            None
        };
        let cadherin = if include_cadherin {
            Some(CadherinTraits::read_checkpoint(input)?)
        } else {
            None
        };
        Self::with_v2_traits(
            affinity,
            motion_response,
            sensing_scale,
            polarity,
            cadherin,
            limits,
        )
        .map_err(|error| CoreError::IncompatibleCheckpoint {
            reason: format!("genome traits: {error}"),
        })
    }

    /// Revalidate and copy every heritable trait against `limits`, preserving
    /// all baseline and v2 traits (including `V2-08` polarity strength / wedge
    /// curvature and `V2-13` cadherin tag / cohesiveness) cleanly across crops
    /// and transplants (`V2-01`, `V2-08`, `V2-13`).
    ///
    /// # Errors
    /// As [`Traits::with_v2_traits`].
    pub fn reseal(&self, limits: &TraitLimits) -> Result<Self, CoreError> {
        Self::with_v2_traits(
            self.affinity,
            self.motion_response,
            self.sensing_scale,
            self.polarity_traits(),
            self.cadherin_traits(),
            limits,
        )
    }
}

/// IEEE-754 exact power of two (`2^-530`) used to down-scale components and
/// bounds without mantissa rounding when `Σ vᵢ²` or `limit²` overflows `f64`.
const NORM_DOWN_SCALE: f64 = f64::from_bits((1023_u64 - 530) << 52);

/// `Σ vᵢ²` in index order. Every norm bound in this crate is checked with
/// exactly this sum against `limit²` whenever both squares are finite.
pub(crate) fn squared_norm(values: &[f64]) -> f64 {
    values
        .iter()
        .fold(0.0, |total, &value| total + value * value)
}

/// `Σ (vᵢ · scale)²` in index order.
fn scaled_squared_norm(values: &[f64], scale: f64) -> f64 {
    values.iter().fold(0.0, |total, &value| {
        let scaled = value * scale;
        total + scaled * scaled
    })
}

/// Whether `||values||₂ > limit` for finite `values` and finite `limit >= 0`.
///
/// Runs the exact `squared_norm(values) > limit * limit` comparison whenever
/// both squares stay finite, and falls back to exact power-of-two down-scaling
/// when either square overflows `f64`.
pub(crate) fn exceeds_norm(values: &[f64], limit: f64) -> bool {
    let squared = squared_norm(values);
    let bound = limit * limit;
    if squared.is_finite() && bound.is_finite() {
        return squared > bound;
    }
    let scaled_limit = limit * NORM_DOWN_SCALE;
    scaled_squared_norm(values, NORM_DOWN_SCALE) > scaled_limit * scaled_limit
}

/// Scale `values` onto the sphere of `radius`: `vᵢ / ||v||₂ · radius`. While
/// rounding leaves the norm above `radius`, every component is multiplied by
/// `1 − 2^-53`, so the result always passes the bound it is checked against.
/// One pass almost always suffices.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for a nonpositive/nonfinite radius, or a
/// zero, nonfinite or unrecoverable norm.
pub(crate) fn rescale(
    quantity: &'static str,
    values: &mut [f64],
    radius: f64,
) -> Result<(), CoreError> {
    if !radius.is_finite() || radius <= 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity,
            detail: format!("radius {radius:?} must be finite and positive"),
        });
    }
    if let Some(value) = values.iter().find(|value| !value.is_finite()) {
        return Err(CoreError::NumericalInvalid {
            quantity,
            detail: format!("component {value} is not finite"),
        });
    }
    let squared = squared_norm(values);
    if squared.is_finite() {
        let norm = squared.sqrt();
        if !norm.is_finite() || norm <= 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity,
                detail: format!("a vector of norm {norm:?} has no direction"),
            });
        }
        for value in values.iter_mut() {
            *value = *value / norm * radius;
        }
    } else {
        let scaled_norm = scaled_squared_norm(values, NORM_DOWN_SCALE).sqrt();
        if !scaled_norm.is_finite() || scaled_norm <= 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity,
                detail: format!("a vector of norm {scaled_norm:?} has no direction"),
            });
        }
        for value in values.iter_mut() {
            *value = (*value * NORM_DOWN_SCALE) / scaled_norm * radius;
        }
    }
    let shrink = 1.0 - f64::EPSILON / 2.0;
    for _ in 0..64 {
        if !exceeds_norm(values, radius) {
            return Ok(());
        }
        for value in values.iter_mut() {
            *value *= shrink;
        }
    }
    Err(CoreError::NumericalInvalid {
        quantity,
        detail: format!("could not bring the norm within {radius}"),
    })
}

fn check_norm(quantity: &'static str, values: &[f64], limit: f64) -> Result<(), CoreError> {
    if let Some(value) = values.iter().find(|value| !value.is_finite()) {
        return Err(CoreError::NumericalInvalid {
            quantity,
            detail: format!("component {value} is not finite"),
        });
    }
    if exceeds_norm(values, limit) {
        let squared = squared_norm(values);
        return Err(CoreError::NumericalInvalid {
            quantity,
            detail: format!("the squared norm {squared} exceeds {limit}²"),
        });
    }
    Ok(())
}

fn check_cadherin_tag(tag: &[f64; CADHERIN_TERMS]) -> Result<(), CoreError> {
    if let Some(value) = tag.iter().find(|value| !value.is_finite()) {
        return Err(CoreError::NumericalInvalid {
            quantity: "cadherin tag",
            detail: format!("component {value} is not finite"),
        });
    }
    let squared = squared_norm(tag);
    if !squared.is_finite() || squared <= 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "cadherin tag",
            detail: format!("squared norm {squared:?} must be strictly positive (||c|| > 0)"),
        });
    }
    if squared > 1.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "cadherin tag",
            detail: format!("the squared norm {squared} exceeds 1²"),
        });
    }
    Ok(())
}

pub(crate) fn check_maximum_wedge_curvature(maximum_wedge_curvature: f64) -> Result<(), CoreError> {
    if !maximum_wedge_curvature.is_finite()
        || !(0.0..=MAXIMUM_WEDGE_CURVATURE_CAP).contains(&maximum_wedge_curvature)
    {
        return Err(CoreError::NumericalInvalid {
            quantity: "v2.polarity.maximum_wedge_curvature",
            detail: format!(
                "{maximum_wedge_curvature} is outside [0, {MAXIMUM_WEDGE_CURVATURE_CAP}]"
            ),
        });
    }
    Ok(())
}

fn check_polarity_strength(polarity_strength: f64) -> Result<(), CoreError> {
    if !polarity_strength.is_finite() || !(0.0..=1.0).contains(&polarity_strength) {
        return Err(CoreError::NumericalInvalid {
            quantity: "polarity strength",
            detail: format!("{polarity_strength} is outside [0, 1]"),
        });
    }
    Ok(())
}

fn check_wedge_curvature(
    wedge_curvature: f64,
    maximum_wedge_curvature: f64,
) -> Result<(), CoreError> {
    if !wedge_curvature.is_finite()
        || !(-maximum_wedge_curvature..=maximum_wedge_curvature).contains(&wedge_curvature)
    {
        return Err(CoreError::NumericalInvalid {
            quantity: "wedge curvature",
            detail: format!(
                "{wedge_curvature} is outside [{}, {}]",
                -maximum_wedge_curvature, maximum_wedge_curvature
            ),
        });
    }
    Ok(())
}

fn check_cohesiveness(cohesiveness: f64) -> Result<(), CoreError> {
    if !cohesiveness.is_finite() || !(0.0..=1.0).contains(&cohesiveness) {
        return Err(CoreError::NumericalInvalid {
            quantity: "cadherin cohesiveness",
            detail: format!("{cohesiveness} is outside [0, 1]"),
        });
    }
    Ok(())
}

/// Normalize a finite, non-zero 4-D cadherin tag onto the exact left-fold
/// unit ball using [`shrink_to_left_fold_norm_limit`] (`V2-13`, Trap D guard).
pub(crate) fn normalize_cadherin_tag(tag: &mut [f64; CADHERIN_TERMS]) -> Result<(), CoreError> {
    if let Some(value) = tag.iter().find(|value| !value.is_finite()) {
        return Err(CoreError::NumericalInvalid {
            quantity: "cadherin tag",
            detail: format!("component {value} is not finite"),
        });
    }
    let norm = squared_norm(tag).sqrt();
    if !norm.is_finite() || norm <= 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "cadherin tag",
            detail: format!("a vector of norm {norm:?} has no direction"),
        });
    }
    for component in tag.iter_mut() {
        *component /= norm;
    }
    shrink_to_left_fold_norm_limit(tag, 1.0)?;
    if squared_norm(tag) <= 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "cadherin tag",
            detail: "normalized cadherin tag underflowed to zero norm".into(),
        });
    }
    Ok(())
}

/// A genome's parameters, shared read-only with its readers.
#[derive(Debug, Clone, PartialEq)]
pub struct SharedParameters(Arc<ParameterValues>);

impl SharedParameters {
    /// The parameters, in their canonical dtype.
    #[must_use]
    pub fn weights(&self) -> Weights<'_> {
        self.0.weights()
    }

    /// Whether `self` and `other` are the same buffer, not merely equal.
    #[must_use]
    pub fn same_buffer(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// A buffer that could not be sealed, returned with the reason so that its
/// charge can be abandoned.
#[derive(Debug)]
pub struct Unsealed {
    /// Why the buffer was refused.
    pub error: CoreError,
    /// The buffer, unchanged.
    pub buffer: ParameterBuffer,
}

/// One organism's immutable genome. It is not `Clone`: it is the live
/// record whose charge [`crate::GenomeQuota::retire`] returns once.
#[derive(Debug, PartialEq)]
pub struct Genome {
    id: GenomeId,
    topology: TopologyId,
    parameters: SharedParameters,
    traits: Traits,
    parent: Option<GenomeId>,
    logical: u64,
    allocated: u64,
    /// The `maximum_absolute_parameter` every parameter was checked against
    /// when the genome was sealed. The parameters never change afterwards.
    sealed_bound: f64,
}

impl Genome {
    /// Seal a filled buffer: every parameter finite and within the bound.
    ///
    /// # Errors
    /// [`Unsealed`] with [`CoreError::NumericalInvalid`] for a parameter
    /// outside the bound, or [`CoreError::LayoutMismatch`] for a buffer the
    /// catalog member does not match.
    pub fn seal(
        buffer: ParameterBuffer,
        id: GenomeId,
        catalog: &TopologyCatalog,
        traits: Traits,
        parent: Option<GenomeId>,
        limits: &TraitLimits,
    ) -> Result<Self, Unsealed> {
        let checked = match catalog.topology(buffer.topology()) {
            Some(topology) => ReferenceController::new(
                topology,
                buffer.values(),
                limits.maximum_absolute_parameter,
            )
            .map(|_| ()),
            None => Err(CoreError::LayoutMismatch {
                buffer: "genome parameters",
                detail: format!("member {} is not in the catalog", buffer.topology().index()),
            }),
        };
        if let Err(error) = checked {
            return Err(Unsealed { error, buffer });
        }
        let (values, topology, logical, allocated) = buffer.into_parts();
        Ok(Self {
            id,
            topology,
            parameters: SharedParameters(Arc::new(values)),
            traits,
            parent,
            logical,
            allocated,
            sealed_bound: limits.maximum_absolute_parameter,
        })
    }

    /// The genome's identity.
    #[must_use]
    pub const fn id(&self) -> GenomeId {
        self.id
    }

    /// The catalog member its controller takes.
    #[must_use]
    pub const fn topology(&self) -> TopologyId {
        self.topology
    }

    /// The controller parameters, in the layout of `flife_neural::controller`
    /// and their canonical dtype.
    #[must_use]
    pub fn parameters(&self) -> Weights<'_> {
        self.parameters.weights()
    }

    /// A read-only handle to the parameters that outlives the genome.
    #[must_use]
    pub fn shared_parameters(&self) -> SharedParameters {
        self.parameters.clone()
    }

    /// The other heritable traits.
    #[must_use]
    pub const fn traits(&self) -> &Traits {
        &self.traits
    }

    /// The parent's genome, or `None` for a founder.
    #[must_use]
    pub const fn parent(&self) -> Option<GenomeId> {
        self.parent
    }

    /// Logical bytes charged for this genome.
    #[must_use]
    pub const fn logical_bytes(&self) -> u64 {
        self.logical
    }

    /// Bytes allocated for this genome's parameters.
    #[must_use]
    pub const fn allocated_bytes(&self) -> u64 {
        self.allocated
    }

    /// This genome's controller, read through its catalog member.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for a catalog without this member;
    /// otherwise as [`ReferenceController::new`].
    pub fn controller<'a>(
        &'a self,
        catalog: &'a TopologyCatalog,
        limits: &TraitLimits,
    ) -> Result<ReferenceController<'a>, CoreError> {
        let topology =
            catalog
                .topology(self.topology)
                .ok_or_else(|| CoreError::LayoutMismatch {
                    buffer: "genome parameters",
                    detail: format!("member {} is not in the catalog", self.topology.index()),
                })?;
        // Sealing checked every parameter against `sealed_bound`, and they
        // are immutable, so under the same or a looser bound only the layout
        // needs checking again. A tighter bound rescans them.
        if limits.maximum_absolute_parameter >= self.sealed_bound {
            ReferenceController::sealed(topology, self.parameters())
        } else {
            ReferenceController::new(
                topology,
                self.parameters(),
                limits.maximum_absolute_parameter,
            )
        }
    }
}

#[cfg(test)]
mod unit_affinity_tests {
    use super::*;

    #[test]
    fn cached_unit_affinity_matches_exact_f32_normalization() {
        let limits = TraitLimits::new(1.0, 10.0, [0.25, 4.0], 4.0).expect("limits");
        let mut affinity = [0.0_f64; AFFINITY_TERMS];
        affinity[0] = 0.3;
        affinity[5] = -0.4;
        affinity[17] = 0.25;
        affinity[31] = -0.15;

        let traits = Traits::new(affinity, [0.0; MOTION_RESPONSE_TERMS], 1.0, &limits)
            .expect("valid traits");
        let norm = affinity.iter().map(|v| v * v).sum::<f64>().sqrt();
        let expected = affinity.map(|v| (v / norm) as f32);
        for (got, exp) in traits.unit_affinity().iter().zip(&expected) {
            assert_eq!(got.to_bits(), exp.to_bits());
        }

        // Zero affinity caches exact +0.0 across all channels.
        let zero_traits = Traits::new(
            [0.0; AFFINITY_TERMS],
            [0.0; MOTION_RESPONSE_TERMS],
            1.0,
            &limits,
        )
        .expect("zero affinity");
        for &v in zero_traits.unit_affinity() {
            assert_eq!(v.to_bits(), 0.0_f32.to_bits());
        }

        // Preserved across v2 polarity/cadherin builders.
        let with_v2 = traits
            .with_polarity_traits(
                Some(
                    PolarityTraits::new(0.5, 0.1, limits.effective_wedge_curvature_bound())
                        .expect("pol"),
                ),
                &limits,
            )
            .expect("polarity")
            .with_cadherin_traits(Some(
                CadherinTraits::new([1.0, 0.0, 0.0, 0.0], 0.8).expect("cad"),
            ))
            .expect("cadherin");
        for (got, exp) in with_v2.unit_affinity().iter().zip(&expected) {
            assert_eq!(got.to_bits(), exp.to_bits());
        }
    }
}
