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

use flife_core::ids::GenomeId;
use flife_core::{CoreError, ValidatedConfig};
use flife_neural::{ReferenceController, Weights};

use crate::catalog::{TopologyCatalog, TopologyId};
use crate::quota::{ParameterBuffer, ParameterValues};

/// Chemical channels, and so affinity components.
pub const AFFINITY_TERMS: usize = 32;

/// Motion-response terms: energy and every channel.
pub const MOTION_RESPONSE_TERMS: usize = 1 + AFFINITY_TERMS;

/// The run's bounds on heritable values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraitLimits {
    affinity_norm: f64,
    motion_response_norm: f64,
    minimum_scale: f64,
    maximum_scale: f64,
    maximum_absolute_parameter: f64,
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
        })
    }

    /// The bounds of a validated configuration.
    ///
    /// # Errors
    /// As [`TraitLimits::new`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let model = config.config();
        Self::new(
            model.chemistry.affinity_norm_limit,
            model.chemistry.motion_response_norm_limit,
            [
                model.sensors.minimum_scale_world_units,
                model.sensors.maximum_scale_world_units,
            ],
            model.controller.maximum_absolute_parameter,
        )
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
}

/// The heritable traits other than the controller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Traits {
    affinity: [f64; AFFINITY_TERMS],
    motion_response: [f64; MOTION_RESPONSE_TERMS],
    sensing_scale: f64,
}

impl Traits {
    /// Traits within `limits`, with norms checked in F64.
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
        Ok(Self {
            affinity,
            motion_response,
            sensing_scale,
        })
    }

    /// The affinity direction `a`.
    #[must_use]
    pub const fn affinity(&self) -> &[f64; AFFINITY_TERMS] {
        &self.affinity
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
}

/// `Σ vᵢ²` in index order. Every norm bound in this crate is checked with
/// exactly this sum against `limit²`.
pub(crate) fn squared_norm(values: &[f64]) -> f64 {
    values
        .iter()
        .fold(0.0, |total, &value| total + value * value)
}

/// Scale `values` onto the sphere of `radius`: `vᵢ / ||v||₂ · radius`. While
/// rounding leaves [`squared_norm`] above `radius²`, every component is
/// multiplied by `1 − 2^-53`, so the result always passes the bound it is
/// checked against. One pass almost always suffices.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for a zero, nonfinite or unrecoverable
/// norm.
pub(crate) fn rescale(
    quantity: &'static str,
    values: &mut [f64],
    radius: f64,
) -> Result<(), CoreError> {
    let norm = squared_norm(values).sqrt();
    if !norm.is_finite() || norm <= 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity,
            detail: format!("a vector of norm {norm:?} has no direction"),
        });
    }
    for value in values.iter_mut() {
        *value = *value / norm * radius;
    }
    let shrink = 1.0 - f64::EPSILON / 2.0;
    for _ in 0..64 {
        if squared_norm(values) <= radius * radius {
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
    let squared = squared_norm(values);
    if squared > limit * limit {
        return Err(CoreError::NumericalInvalid {
            quantity,
            detail: format!("the squared norm {squared} exceeds {limit}²"),
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
