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
use flife_neural::ReferenceController;

use crate::catalog::{TopologyCatalog, TopologyId};
use crate::quota::ParameterBuffer;

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

fn check_norm(quantity: &'static str, values: &[f64], limit: f64) -> Result<(), CoreError> {
    let mut squared = 0.0;
    for &value in values {
        if !value.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity,
                detail: format!("component {value} is not finite"),
            });
        }
        squared += value * value;
    }
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
pub struct SharedParameters(Arc<Vec<f32>>);

impl SharedParameters {
    /// The parameters.
    #[must_use]
    pub fn as_slice(&self) -> &[f32] {
        &self.0
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

    /// The controller parameters, in the layout of `flife_neural::controller`.
    #[must_use]
    pub fn parameters(&self) -> &[f32] {
        self.parameters.as_slice()
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
        ReferenceController::new(
            topology,
            self.parameters(),
            limits.maximum_absolute_parameter,
        )
    }
}
