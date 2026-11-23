//! The controller backend port (plan §10.2, §10.7, §15): how the engine
//! hands a tick's due evaluations to an accelerator in one batch. The
//! composition root (the CLI, the Studio) chooses an implementation and
//! gives it to the world; the engine depends only on this trait, and no
//! backend depends on the engine.
//!
//! A backend is an exact replacement for [`ReferenceController::evaluate`],
//! not an approximation: every output it returns is the reference's, bit for
//! bit, and it returns `None` for any evaluation it cannot reproduce. The
//! engine evaluates those on the reference, which also reproduces the
//! reference's refusals. So no result of a run depends on the backend.

use std::fmt::Debug;

use flife_core::CoreError;
use flife_core::ids::GenomeId;

use crate::{ArenaSlot, RawOutputs, ReferenceController};

/// One due organism's controller evaluation.
#[derive(Debug, Clone, Copy)]
pub struct ControllerJob<'a> {
    /// The organism's genome.
    pub genome: GenomeId,
    /// The genome's arena slot. While a backend serves one world, a slot
    /// and generation name one immutable parameter set for as long as that
    /// genome lives, and a later occupant of the slot has a higher
    /// generation. So a backend keeps packed parameters by slot, and reuses
    /// them only for the same generation and genome; anything else it
    /// packs afresh into the same place.
    pub slot: ArenaSlot,
    /// The organism's controller, admitted under the run's limits.
    pub controller: ReferenceController<'a>,
    /// Its controller inputs (`inputs.len() == controller.topology().input_width()`,
    /// up to [`crate::MAX_SENSOR_WIDTH`] = `128`). For recurrent topologies,
    /// the caller appends the current internal state `h[0..S]` at the tail.
    pub inputs: &'a [f64],
}

/// The memory a controller backend may hold for one world (FL-043, plan
/// §19.4): the preflight's backend term, given to a backend when it is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendBudget {
    /// Bytes it may hold at once, host and device: resident parameters,
    /// staging and scratch. A backend makes no parameter shard that would
    /// leave too little of it for its staging of a batch of `organisms`
    /// jobs; it defers that shard's jobs to the reference instead.
    pub bytes: u64,
    /// The most jobs one batch holds: the world's population capacity.
    pub organisms: u64,
}

impl BackendBudget {
    /// No limit but the host's and the device's.
    pub const UNBOUNDED: Self = Self {
        bytes: u64::MAX,
        organisms: u64::MAX,
    };
}

/// A batch evaluator of the reference controller.
pub trait ControllerBackend: Debug + Send {
    /// The backend and its device, for run records.
    fn describe(&self) -> String;

    /// The bytes it holds now, host and device: resident parameters,
    /// staging and scratch (FL-043). Within its [`BackendBudget`]'s bytes.
    fn resident_bytes(&self) -> u64;

    /// Drop everything kept resident; the next batch uploads afresh. The
    /// world calls this when the backend starts serving it, since another
    /// world's slots name other genomes.
    fn forget(&mut self);

    /// Notify the backend that `slots` were retired by deaths at tick commit
    /// (FL-043). Backends holding resident parameter shards mark matching
    /// `(slot, generation)` entries unoccupied and reclaim any shard with no
    /// live occupants remaining.
    fn release_slots(&mut self, _slots: &[ArenaSlot]) {}

    /// The device's time on the commands of the last
    /// [`ControllerBackend::evaluate`], in nanoseconds (FL-029): zero for a
    /// backend that runs on the host, and for a batch that dispatched
    /// nothing.
    fn last_device_nanoseconds(&self) -> u64 {
        0
    }

    /// Evaluate `jobs` in order. `outputs` becomes one entry per job: its
    /// raw linear outputs (length `job.controller.topology().raw_output_width()`,
    /// up to [`crate::MAX_CONTROLLER_OUTPUT_WIDTH`] = `80`), bit-identical to
    /// `job.controller.evaluate_raw(job.inputs, _)`, or `None` where the
    /// backend defers that job to the reference. Every job the reference
    /// refuses must be deferred.
    ///
    /// # Errors
    /// A device or allocation failure. The world then evaluates the tick on
    /// the reference and stops using the backend.
    fn evaluate(
        &mut self,
        jobs: &[ControllerJob<'_>],
        outputs: &mut Vec<Option<RawOutputs>>,
    ) -> Result<(), CoreError>;
}
