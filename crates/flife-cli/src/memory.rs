//! The peak-memory preflight (FL-043; plan §7.5, §19.3, §19.4, INV-12,
//! INV-15).
//!
//! Before a run steps, [`admit`] adds up §19.4's terms against the
//! configuration's `[execution] maximum_total_bytes`:
//!
//! - genomes: `maximum_allocated_genome_bytes`, the allocated genome quota.
//!   The genome quota refuses any parameter buffer past it, and the logical
//!   quota (`maximum_logical_genome_bytes`, reported in parameters too)
//!   stays the fixed scientific one: this preflight never changes which
//!   birth is accepted.
//! - state, field and scratch: the founded world's [`World::footprint`],
//!   its buffers as reserved for `population.maximum_count`, plus the
//!   batched path's lists when the run has a controller backend.
//! - births: the footprint's staging, what one step holds while it runs.
//! - render: what an observer reserves. A headless run has none.
//! - checkpoint: the checkpoint admission's peak (FL-038), or nothing
//!   without a run directory.
//! - reserve: [`RESERVE_BYTES`] for the runtime's and libraries' own heap.
//! - backend: what all that leaves, granted to the controller backend as
//!   its [`BackendBudget`]. It holds no more: a parameter shard that does
//!   not fit is not made, and its jobs are evaluated on the reference,
//!   which gives the same bits. So a small budget slows a run and never
//!   changes it.
//!
//! A run whose terms other than the backend's exceed the budget is refused
//! before anything is created. The start record carries the admission, and
//! the end record the most the backend held.

use flife_backend_metal::{BackendBudget, ControllerChoice};
use flife_core::memory::total;
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::World;
use flife_genome::quota::F32_BYTES;
use serde::Serialize;

use crate::checkpointing::CheckpointAdmission;

/// The heap the runtime, the session, its records, the store's database and
/// the device driver hold apart from the world, its backend and its
/// checkpoints: a declared allowance.
pub const RESERVE_BYTES: u64 = 64 << 20;

/// The preflight's decision, reported in the start record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryAdmission {
    /// `[execution] maximum_total_bytes`.
    pub budget_bytes: u64,
    /// The fixed logical genome quota in F32 parameters:
    /// `maximum_logical_genome_bytes` over four.
    pub logical_parameters: u64,
    /// `maximum_logical_genome_bytes`.
    pub logical_genome_bytes: u64,
    /// The allocated genome quota, `maximum_allocated_genome_bytes`.
    pub genomes: u64,
    /// The population, the sources and their paths.
    pub state: u64,
    /// The fields, the field step's scratch and the sensor pyramid.
    pub field: u64,
    /// The tick's scratch, the batched path's lists included.
    pub scratch: u64,
    /// What a step holds while it runs: births and their staging.
    pub births: u64,
    /// What observers reserve; zero for a headless run.
    pub render: u64,
    /// The checkpoint admission's peak, or zero without a run directory.
    pub checkpoint: u64,
    /// [`RESERVE_BYTES`].
    pub reserve: u64,
    /// The controller backend's budget: what the other terms leave, or zero
    /// for the CPU reference.
    pub backend: u64,
    /// Every term: at most `budget_bytes`.
    pub peak_bytes: u64,
}

impl MemoryAdmission {
    /// The budget the controller backend is given.
    #[must_use]
    pub const fn backend_budget(&self, organisms: u64) -> BackendBudget {
        BackendBudget {
            bytes: self.backend,
            organisms,
        }
    }
}

/// Admit `world` under its configuration's memory budget, with `choice`'s
/// controller backend, `checkpoint`'s admission and `render` bytes of
/// observers (see the [module documentation](self)). Call it after the
/// world's workers are set and before its backend is installed.
///
/// # Errors
/// [`CoreError::BudgetExhausted`] when the terms other than the backend's
/// exceed `maximum_total_bytes`.
pub fn admit(
    config: &ValidatedConfig,
    world: &World,
    choice: ControllerChoice,
    checkpoint: Option<&CheckpointAdmission>,
    render: u64,
) -> Result<MemoryAdmission, CoreError> {
    let execution = &config.config().execution;
    let footprint = world.footprint();
    let batched = choice != ControllerChoice::Cpu;
    let scratch = if batched {
        footprint.scratch.saturating_add(world.batched_bytes())
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
        checkpoint: checkpoint.map_or(0, |admission| admission.peak_bytes),
        reserve: RESERVE_BYTES,
        backend: 0,
        peak_bytes: 0,
    };
    let fixed = total([
        admission.genomes,
        admission.state,
        admission.field,
        admission.scratch,
        admission.births,
        admission.render,
        admission.checkpoint,
        admission.reserve,
    ]);
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
