//! The peak-memory preflight (FL-043; plan §7.5, §19.3, §19.4, INV-12,
//! INV-15).
//!
//! Before a world steps, [`World::admit_memory`] adds up §19.4's terms
//! against its configuration's `[execution] maximum_total_bytes`:
//!
//! - genomes: `maximum_allocated_genome_bytes`, the allocated genome quota.
//!   The genome quota refuses any parameter buffer past it, and the logical
//!   quota (`maximum_logical_genome_bytes`, reported in parameters too)
//!   stays the fixed scientific one: this preflight never changes which
//!   birth is accepted.
//! - state, field and scratch: the world's [`World::footprint`], its
//!   buffers as reserved for `population.maximum_count`, plus the batched
//!   path's lists ([`World::batched_bytes`]) when it will have a controller
//!   backend.
//! - births: the footprint's staging, what one step holds while it runs.
//! - render: what the host's observers hold, which the host works out. A
//!   headless run has none.
//! - checkpoint: the checkpoint admission's peak (FL-038), or nothing
//!   without a run directory.
//! - reserve: [`RESERVE_BYTES`] for the runtime's and libraries' own heap.
//! - backend: what all that leaves, granted to the controller backend as
//!   its budget. It holds no more: a parameter shard that does not fit is
//!   not made, and its jobs are evaluated on the reference, which gives the
//!   same bits. So a small budget slows a run and never changes it.
//!
//! A world whose terms other than the backend's exceed the budget is
//! refused before its host creates anything else.

use flife_core::CoreError;
use flife_core::memory::{MemoryAdmission, RESERVE_BYTES};
use flife_genome::quota::F32_BYTES;

use super::World;

impl World {
    /// Admit this world under its configuration's memory budget (plan
    /// §19.4), with a controller backend when `batched`, a checkpoint
    /// admission's `checkpoint` peak bytes and `render` bytes of observers.
    /// The terms are the allocated genome quota; the [`World::footprint`]'s
    /// state, field, scratch (with [`World::batched_bytes`] when `batched`)
    /// and staging as births; `render`; `checkpoint`; and [`RESERVE_BYTES`].
    /// The backend is granted what they leave of `maximum_total_bytes`, or
    /// nothing when not `batched`. Call it after the world's workers are set
    /// and before its backend is installed, and give the backend
    /// [`MemoryAdmission::backend`] bytes.
    ///
    /// # Errors
    /// [`CoreError::BudgetExhausted`] when the terms other than the
    /// backend's exceed `maximum_total_bytes`.
    pub fn admit_memory(
        &self,
        batched: bool,
        checkpoint: u64,
        render: u64,
    ) -> Result<MemoryAdmission, CoreError> {
        let execution = &self.config().config().execution;
        let footprint = self.footprint();
        let scratch = if batched {
            footprint.scratch.saturating_add(self.batched_bytes())
        } else {
            footprint.scratch
        };
        let mut admission = MemoryAdmission {
            budget_bytes: execution.maximum_total_bytes,
            logical_parameters: execution.maximum_logical_genome_bytes / F32_BYTES,
            logical_genome_bytes: execution.maximum_logical_genome_bytes,
            genomes: execution.maximum_allocated_genome_bytes,
            state: footprint.state,
            field: footprint.field,
            scratch,
            births: footprint.staging,
            render,
            checkpoint,
            reserve: RESERVE_BYTES,
            backend: 0,
            peak_bytes: 0,
        };
        let fixed = admission.fixed_bytes();
        if fixed > admission.budget_bytes {
            return Err(CoreError::BudgetExhausted {
                budget: "peak memory (maximum_total_bytes)",
                required: fixed,
                available: admission.budget_bytes,
            });
        }
        if batched {
            admission.backend = admission.budget_bytes - fixed;
        }
        admission.peak_bytes = fixed.saturating_add(admission.backend);
        Ok(admission)
    }
}
