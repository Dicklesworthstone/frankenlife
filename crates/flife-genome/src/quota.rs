//! Genome quotas and checked parameter allocation (plan §7.5, §9.2, A.2,
//! INV-15).
//!
//! The logical quota charges every live genome its full canonical size,
//! `P × 4` bytes for F32 and `P × 2` for compact F16 (plan §10.3). Exact
//! content deduplication, if a later store
//! adds it, never lowers that charge. The allocated quota counts the bytes
//! actually reserved for parameter buffers. Both are checked before
//! anything is allocated; a refusal leaves the quota unchanged. Host
//! allocation failure is a separate error, never an abort.

use flife_core::binary16::F16;
use flife_core::{CoreError, ValidatedConfig};
use flife_neural::{DenseTopology, WeightDtype, Weights};

use crate::catalog::{TopologyCatalog, TopologyId};
use crate::genome::Genome;

/// Bytes per canonical F32 parameter.
pub const F32_BYTES: u64 = 4;

/// The budget a [`CoreError::BudgetExhausted`] names when the fixed logical
/// genome quota refuses a genome. Under `pause-before-commit` this is a
/// capacity shortfall that pauses the tick (§7.5).
pub const LOGICAL_GENOME_BYTES: &str = "logical genome bytes";

/// The budget a [`CoreError::BudgetExhausted`] names when the allocated
/// (physical) genome quota refuses a buffer. That is a physical condition
/// that pauses, retries or fails the run; it never selects births (§7.5).
pub const ALLOCATED_GENOME_BYTES: &str = "allocated genome bytes";

/// The logical bytes one F32 genome of `topology` is charged.
///
/// # Errors
/// [`CoreError::Overflow`] past `u64::MAX`.
pub fn logical_bytes(topology: &DenseTopology) -> Result<u64, CoreError> {
    logical_bytes_in(topology, WeightDtype::F32)
}

/// The logical bytes one genome of `topology` is charged in `dtype`.
///
/// # Errors
/// [`CoreError::Overflow`] past `u64::MAX`.
pub fn logical_bytes_in(topology: &DenseTopology, dtype: WeightDtype) -> Result<u64, CoreError> {
    u64::try_from(topology.parameter_count())
        .ok()
        .and_then(|count| count.checked_mul(dtype.bytes()))
        .ok_or(CoreError::Overflow {
            quantity: "logical genome bytes",
        })
}

/// A zeroed buffer of `count` parameters, allocated fallibly.
///
/// # Errors
/// [`CoreError::AllocationFailed`] when the size is unrepresentable or the
/// host refuses it.
pub fn allocate_parameters(count: usize) -> Result<Vec<f32>, CoreError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: "genome parameters",
            bytes: u64::try_from(count)
                .ok()
                .and_then(|count| count.checked_mul(F32_BYTES))
                .unwrap_or(u64::MAX),
        })?;
    values.resize(count, 0.0);
    Ok(values)
}

/// A zeroed buffer of `count` parameters in `dtype`, allocated fallibly.
///
/// # Errors
/// As [`allocate_parameters`].
pub fn allocate_values(count: usize, dtype: WeightDtype) -> Result<ParameterValues, CoreError> {
    match dtype {
        WeightDtype::F32 => allocate_parameters(count).map(ParameterValues::F32),
        WeightDtype::F16 => {
            let mut values = Vec::new();
            values
                .try_reserve_exact(count)
                .map_err(|_| CoreError::AllocationFailed {
                    buffer: "genome parameters",
                    bytes: u64::try_from(count)
                        .ok()
                        .and_then(|count| count.checked_mul(dtype.bytes()))
                        .unwrap_or(u64::MAX),
                })?;
            values.resize(count, F16::ZERO);
            Ok(ParameterValues::F16(values))
        }
    }
}

/// A genome's parameter values in their canonical dtype, owned.
#[derive(Debug, Clone, PartialEq)]
pub enum ParameterValues {
    /// The reference profile's F32 values.
    F32(Vec<f32>),
    /// The compact-F16 profile's binary16 values.
    F16(Vec<F16>),
}

impl ParameterValues {
    /// The values, borrowed.
    #[must_use]
    pub fn weights(&self) -> Weights<'_> {
        match self {
            Self::F32(values) => Weights::F32(values),
            Self::F16(values) => Weights::F16(values),
        }
    }

    /// The values, to fill.
    pub fn weights_mut(&mut self) -> WeightsMut<'_> {
        match self {
            Self::F32(values) => WeightsMut::F32(values),
            Self::F16(values) => WeightsMut::F16(values),
        }
    }

    /// The dtype.
    #[must_use]
    pub const fn dtype(&self) -> WeightDtype {
        match self {
            Self::F32(_) => WeightDtype::F32,
            Self::F16(_) => WeightDtype::F16,
        }
    }

    /// Bytes the allocation holds: its capacity times the dtype's size.
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        let capacity = match self {
            Self::F32(values) => values.capacity(),
            Self::F16(values) => values.capacity(),
        };
        u64::try_from(capacity)
            .ok()
            .and_then(|capacity| capacity.checked_mul(self.dtype().bytes()))
            .unwrap_or(u64::MAX)
    }
}

/// A genome's parameter values in their canonical dtype, writable.
#[derive(Debug)]
pub enum WeightsMut<'a> {
    /// The reference profile's F32 values.
    F32(&'a mut [f32]),
    /// The compact-F16 profile's binary16 values.
    F16(&'a mut [F16]),
}

/// The run's fixed logical and allocated genome quotas and what is charged
/// against them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenomeQuota {
    maximum_logical: u64,
    maximum_allocated: u64,
    logical: u64,
    allocated: u64,
    live: u64,
    dtype: WeightDtype,
}

impl GenomeQuota {
    /// Quotas `maximum_logical ≤ maximum_allocated`, both in bytes, for F32
    /// genomes ([`GenomeQuota::with_dtype`] for another dtype).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for quotas out of order.
    pub fn new(maximum_logical: u64, maximum_allocated: u64) -> Result<Self, CoreError> {
        if maximum_logical > maximum_allocated {
            return Err(CoreError::NumericalInvalid {
                quantity: "execution.maximum_logical_genome_bytes",
                detail: format!(
                    "{maximum_logical} exceeds the allocated quota {maximum_allocated}"
                ),
            });
        }
        Ok(Self {
            maximum_logical,
            maximum_allocated,
            logical: 0,
            allocated: 0,
            live: 0,
            dtype: WeightDtype::F32,
        })
    }

    /// The same quotas for genomes of `dtype`.
    #[must_use]
    pub const fn with_dtype(self, dtype: WeightDtype) -> Self {
        Self { dtype, ..self }
    }

    /// The quotas of a validated configuration, for its canonical weight
    /// dtype.
    ///
    /// # Errors
    /// As [`GenomeQuota::new`], and [`WeightDtype::named`]'s refusal.
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let model = config.config();
        let dtype = WeightDtype::named(&model.controller.canonical_weight_dtype)?;
        let execution = &model.execution;
        Self::new(
            execution.maximum_logical_genome_bytes,
            execution.maximum_allocated_genome_bytes,
        )
        .map(|quota| quota.with_dtype(dtype))
    }

    /// The dtype of the genomes it allocates.
    #[must_use]
    pub const fn dtype(&self) -> WeightDtype {
        self.dtype
    }

    /// Logical bytes charged to live genomes.
    #[must_use]
    pub const fn logical_bytes(&self) -> u64 {
        self.logical
    }

    /// Bytes allocated to live genomes' parameters.
    #[must_use]
    pub const fn allocated_bytes(&self) -> u64 {
        self.allocated
    }

    /// Live genomes, and buffers not yet sealed.
    #[must_use]
    pub const fn live(&self) -> u64 {
        self.live
    }

    /// How many more genomes of `topology` the logical quota admits.
    ///
    /// # Errors
    /// As [`logical_bytes`].
    pub fn capacity_for(&self, topology: &DenseTopology) -> Result<u64, CoreError> {
        let each = logical_bytes_in(topology, self.dtype)?;
        Ok((self.maximum_logical - self.logical)
            .checked_div(each)
            .unwrap_or(u64::MAX))
    }

    /// Charge one genome of catalog member `id` and allocate its zeroed
    /// parameter buffer. Refused, with nothing charged or allocated, when
    /// the member is not admitted or either quota cannot cover it.
    ///
    /// # Errors
    /// [`CoreError::BudgetExhausted`] for a member above the parameter cap
    /// or a quota that cannot cover it; [`CoreError::NumericalInvalid`] for
    /// an unknown member; [`CoreError::AllocationFailed`] when the host
    /// refuses the buffer.
    pub fn allocate(
        &mut self,
        catalog: &TopologyCatalog,
        id: TopologyId,
    ) -> Result<ParameterBuffer, CoreError> {
        let topology = catalog
            .topology(id)
            .ok_or_else(|| CoreError::NumericalInvalid {
                quantity: "topology id",
                detail: format!("member {} is not in the catalog", id.index()),
            })?;
        let parameters = u64::try_from(topology.parameter_count()).unwrap_or(u64::MAX);
        if !catalog.admits(id) {
            return Err(CoreError::BudgetExhausted {
                budget: "architecture_mutation.maximum_parameters",
                required: parameters,
                available: catalog.maximum_parameters(),
            });
        }
        let logical = logical_bytes_in(topology, self.dtype)?;
        self.check(
            LOGICAL_GENOME_BYTES,
            logical,
            self.logical,
            self.maximum_logical,
        )?;
        self.check(
            ALLOCATED_GENOME_BYTES,
            logical,
            self.allocated,
            self.maximum_allocated,
        )?;
        let values = allocate_values(topology.parameter_count(), self.dtype)?;
        let allocated = values.allocated_bytes();
        // The host may reserve more than requested; the allocated quota
        // counts what it actually reserved.
        self.check(
            ALLOCATED_GENOME_BYTES,
            allocated,
            self.allocated,
            self.maximum_allocated,
        )?;
        self.logical += logical;
        self.allocated += allocated;
        self.live += 1;
        Ok(ParameterBuffer {
            values,
            topology: id,
            logical,
            allocated,
        })
    }

    /// Return the charge of a buffer that never became a genome.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for a charge this quota does not hold.
    pub fn abandon(&mut self, buffer: ParameterBuffer) -> Result<(), CoreError> {
        self.release(buffer.logical, buffer.allocated)
    }

    /// Return a retired genome's charge. Readers holding its shared
    /// parameters keep them; they count against observer budgets, not this
    /// quota (§7.5).
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for a charge this quota does not hold.
    pub fn retire(&mut self, genome: Genome) -> Result<(), CoreError> {
        self.release(genome.logical_bytes(), genome.allocated_bytes())
    }

    fn check(
        &self,
        budget: &'static str,
        required: u64,
        used: u64,
        maximum: u64,
    ) -> Result<(), CoreError> {
        let available = maximum - used;
        if required > available {
            return Err(CoreError::BudgetExhausted {
                budget,
                required,
                available,
            });
        }
        Ok(())
    }

    fn release(&mut self, logical: u64, allocated: u64) -> Result<(), CoreError> {
        match (
            self.logical.checked_sub(logical),
            self.allocated.checked_sub(allocated),
            self.live.checked_sub(1),
        ) {
            (Some(new_logical), Some(new_allocated), Some(live)) => {
                self.logical = new_logical;
                self.allocated = new_allocated;
                self.live = live;
                Ok(())
            }
            _ => Err(CoreError::StateConflict {
                operation: "release genome charge",
                detail: format!(
                    "{logical} logical and {allocated} allocated bytes exceed what is charged"
                ),
            }),
        }
    }
}

/// A charged, zeroed parameter buffer for one catalog member, to be filled
/// and sealed into a [`Genome`].
#[derive(Debug, PartialEq)]
pub struct ParameterBuffer {
    values: ParameterValues,
    topology: TopologyId,
    logical: u64,
    allocated: u64,
}

impl ParameterBuffer {
    /// The member this buffer is laid out for.
    #[must_use]
    pub const fn topology(&self) -> TopologyId {
        self.topology
    }

    /// The parameters.
    #[must_use]
    pub fn values(&self) -> Weights<'_> {
        self.values.weights()
    }

    /// The parameters, to fill.
    pub fn values_mut(&mut self) -> WeightsMut<'_> {
        self.values.weights_mut()
    }

    /// The parameters of an F32 buffer, to fill; `None` for another dtype.
    pub fn f32_mut(&mut self) -> Option<&mut [f32]> {
        match &mut self.values {
            ParameterValues::F32(values) => Some(values),
            ParameterValues::F16(_) => None,
        }
    }

    pub(crate) fn into_parts(self) -> (ParameterValues, TopologyId, u64, u64) {
        (self.values, self.topology, self.logical, self.allocated)
    }
}
