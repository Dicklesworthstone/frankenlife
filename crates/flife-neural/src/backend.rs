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

use crate::{ACTION_WIDTH, ReferenceController, SENSOR_WIDTH};

/// One due organism's controller evaluation.
#[derive(Debug, Clone, Copy)]
pub struct ControllerJob<'a> {
    /// The organism's genome. While a backend serves one world, one genome
    /// names one parameter set, so a backend may keep parameters resident
    /// by it. It must still check that a resident set is the same memory
    /// (pointer and length of [`ReferenceController::parameters`]) before
    /// reusing it.
    pub genome: GenomeId,
    /// The organism's controller, admitted under the run's limits.
    pub controller: ReferenceController<'a>,
    /// Its observation's 104 inputs.
    pub inputs: &'a [f64; SENSOR_WIDTH],
}

/// A batch evaluator of the reference controller.
pub trait ControllerBackend: Debug + Send {
    /// The backend and its device, for run records.
    fn describe(&self) -> String;

    /// Drop everything kept resident; the next batch uploads afresh. The
    /// world calls this when the backend starts serving it.
    fn forget(&mut self);

    /// Evaluate `jobs` in order. `outputs` becomes one entry per job: its
    /// 36 raw outputs, bit-identical to
    /// `job.controller.evaluate(job.inputs, _)`, or `None` where the
    /// backend defers that job to the reference. Every job the reference
    /// refuses must be deferred.
    ///
    /// # Errors
    /// A device or allocation failure. The world then evaluates the tick on
    /// the reference and stops using the backend.
    fn evaluate(
        &mut self,
        jobs: &[ControllerJob<'_>],
        outputs: &mut Vec<Option<[f32; ACTION_WIDTH]>>,
    ) -> Result<(), CoreError>;
}
