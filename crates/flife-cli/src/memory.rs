//! The peak-memory preflight (FL-043; plan §7.5, §19.3, §19.4, INV-12,
//! INV-15) for the command line's runs and renders.
//!
//! [`admit`] is [`World::admit_memory`] with a [`ControllerChoice`] and a
//! [`CheckpointAdmission`]: every term but the backend's against
//! `[execution] maximum_total_bytes`, and the backend given what they
//! leave. A run whose other terms exceed the budget is refused before
//! anything is created. The start record carries the admission, and the
//! end record the most the backend held.

use flife_backend_metal::ControllerChoice;
use flife_core::CoreError;
pub use flife_core::memory::{MemoryAdmission, RESERVE_BYTES};
use flife_engine::World;

use crate::checkpointing::CheckpointAdmission;

/// Admit `world` under its configuration's memory budget, with `choice`'s
/// controller backend, `checkpoint`'s admission and `render` bytes of
/// observers ([`World::admit_memory`]). Call it after the world's workers
/// are set and before its backend is installed.
///
/// # Errors
/// [`CoreError::BudgetExhausted`] when the terms other than the backend's
/// exceed `maximum_total_bytes`.
pub fn admit(
    world: &World,
    choice: ControllerChoice,
    checkpoint: Option<&CheckpointAdmission>,
    render: u64,
) -> Result<MemoryAdmission, CoreError> {
    let ckpt = checkpoint.map_or(0, |admission| admission.peak_bytes);
    match choice {
        ControllerChoice::Cpu => world.admit_memory(false, ckpt, render),
        // Every built-in accelerated backend (`CpuBackend`, `MetalBackend`)
        // implements `supports_flat_f32() == true`, reserving only the compact
        // flat-F32 buffers (`flat_f32_batched_bytes`) rather than legacy
        // `RawOutputs` / per-lane `Observation` buffers.
        ControllerChoice::Auto | ControllerChoice::Torch | ControllerChoice::Metal => {
            world.admit_memory_flat_f32(ckpt, render)
        }
    }
}
