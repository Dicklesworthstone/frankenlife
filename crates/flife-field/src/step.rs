//! Phase 2 of the tick (plan §8.1): transport the committed field, decay
//! it, then apply this tick's external sources. The result is the next
//! field's observation state. The step writes only the candidate buffer and
//! never commits. The engine validates the candidate (phase 10) and then
//! commits or discards it.

use std::num::NonZeroUsize;

use flife_core::{CoreError, TimeStep};

use crate::footprint::BufferLimits;
use crate::grid::{FIELD_PLANES, FieldGrid};
use crate::layout::overflow;
use crate::sources::{
    DepositReport, Source, SourceKernel, SourceSupport, component, deposit, deposit_energy,
    deposit_moment,
};
use crate::state::FieldPair;
use crate::transport::{
    TransportPlan, TransportReport, TransportScratch, sum, transport, transport_on,
};

/// The field's ledger terms for one tick (§4.4), in F64.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldStepReport {
    /// Transport and decay.
    pub transport: TransportReport,
    /// One record per source, in the order given.
    pub deposits: Vec<DepositReport>,
    /// `Σ_j P_j Δt`: declared external injection `I`.
    pub injected_declared: f64,
    /// Energy the F32 field actually gained from sources.
    pub injected_applied: f64,
    /// `Σ e` of the candidate field.
    pub energy_after: f64,
    /// `ε = (after − before) − (I − D_decay)`: the numerical residual of the
    /// whole step. It is zero in exact arithmetic.
    pub residual: f64,
}

/// Advance the field part of one tick from `pair.current()` into the
/// candidate `pair`'s next buffer.
///
/// # Errors
/// As [`transport`] and [`deposit`]. On error the candidate is incomplete
/// and must be discarded; the committed field is untouched.
pub fn advance_field(
    pair: &mut FieldPair,
    plan: &TransportPlan,
    kernel: &SourceKernel,
    sources: &[Source],
    step: TimeStep,
    scratch: &mut TransportScratch,
    support: &mut SourceSupport,
) -> Result<FieldStepReport, CoreError> {
    let (current, next) = pair.stage();
    let transported = transport(plan, current, next, scratch)?;
    let mut deposits = Vec::with_capacity(sources.len());
    let (mut injected_declared, mut injected_applied) = (0.0, 0.0);
    for source in sources {
        let report = deposit(next, kernel, source, step, support)?;
        injected_declared += report.declared;
        injected_applied += report.applied;
        deposits.push(report);
    }
    let energy_after = sum(next.energy());
    let residual = (energy_after - transported.energy_before)
        - (injected_declared - transported.decay_removed);
    Ok(FieldStepReport {
        transport: transported,
        deposits,
        injected_declared,
        injected_applied,
        energy_after,
        residual,
    })
}

/// Reusable scratch for [`advance_field_on`]: one transport scratch per CPU
/// worker and one support per source.
#[derive(Debug)]
pub struct FieldWork {
    grid: FieldGrid,
    limits: BufferLimits,
    transport: Vec<TransportScratch>,
    supports: Vec<SourceSupport>,
}

impl FieldWork {
    /// Scratch for `workers` threads on `grid`.
    ///
    /// # Errors
    /// As [`TransportScratch::new`].
    pub fn new(
        grid: &FieldGrid,
        limits: &BufferLimits,
        workers: NonZeroUsize,
    ) -> Result<Self, CoreError> {
        let mut work = Self {
            grid: *grid,
            limits: limits.clone(),
            transport: Vec::new(),
            supports: Vec::new(),
        };
        work.set_workers(workers)?;
        Ok(work)
    }

    /// The threads [`advance_field_on`] may use.
    #[must_use]
    pub fn workers(&self) -> usize {
        self.transport.len()
    }

    /// Use `workers` threads from the next step.
    ///
    /// # Errors
    /// As [`TransportScratch::new`]; the count is then unchanged.
    pub fn set_workers(&mut self, workers: NonZeroUsize) -> Result<(), CoreError> {
        let workers = workers.get();
        let mut added = Vec::new();
        for _ in self.transport.len()..workers {
            added.push(TransportScratch::new(&self.grid, &self.limits)?);
        }
        self.transport.extend(added);
        self.transport.truncate(workers);
        Ok(())
    }
}

/// [`advance_field`] on up to [`FieldWork::workers`] threads: this one and
/// scoped workers it joins before returning (plan §3.4's bounded reference
/// adapter). Planes are transported independently ([`transport_on`]). Every
/// source's energy is then deposited in source order on this thread, and
/// the moment planes are split over the workers, each adding the sources
/// in the same order. Every value is computed exactly as [`advance_field`]
/// computes it.
///
/// # Errors
/// As [`advance_field`]; [`CoreError::StateConflict`] for a worker that
/// panicked.
pub fn advance_field_on(
    pair: &mut FieldPair,
    plan: &TransportPlan,
    kernel: &SourceKernel,
    sources: &[Source],
    step: TimeStep,
    work: &mut FieldWork,
) -> Result<FieldStepReport, CoreError> {
    let FieldWork {
        transport: scratches,
        supports,
        ..
    } = work;
    let (current, next) = pair.stage();
    let transported = transport_on(plan, current, next, scratches)?;
    if supports.len() < sources.len() {
        supports.resize_with(sources.len(), SourceSupport::new);
    }
    let supports = &mut supports[..sources.len()];
    let grid = *next.grid();
    let mut deposits = Vec::with_capacity(sources.len());
    let (mut injected_declared, mut injected_applied) = (0.0, 0.0);
    for (source, support) in sources.iter().zip(supports.iter_mut()) {
        kernel.support_into(&grid, source.center(), support)?;
        let report = deposit_energy(next, source, step, support)?;
        injected_declared += report.declared;
        injected_applied += report.applied;
        deposits.push(report);
    }

    let cells = usize::try_from(grid.cell_count()).map_err(|_| overflow())?;
    let channels = usize::try_from(FIELD_PLANES - 1).map_err(|_| overflow())?;
    let supports: &[SourceSupport] = supports;
    let run = |first: usize, planes: &mut [f32]| -> Result<(), CoreError> {
        for (offset, plane) in planes.chunks_exact_mut(cells).enumerate() {
            for (source, support) in sources.iter().zip(supports) {
                deposit_moment(plane, component(source, first + offset), support)?;
            }
        }
        Ok(())
    };
    let moments = next.moments_mut();
    if cells == 0 || moments.len() != cells * channels {
        return Err(crate::layout::mismatch(
            "the candidate field has no moment planes".to_owned(),
        ));
    }
    let used = scratches.len().clamp(1, channels.max(1));
    let per = channels.div_ceil(used);
    let mut runs = moments.chunks_mut(per * cells);
    let head = runs.next().unwrap_or(&mut []);
    let run = &run;
    std::thread::scope(|scope| {
        let workers: Vec<_> = runs
            .enumerate()
            .map(|(index, planes)| scope.spawn(move || run((index + 1) * per, planes)))
            .collect();
        let mut result = run(0, head);
        for worker in workers {
            let joined = worker.join().unwrap_or_else(|_| {
                Err(CoreError::StateConflict {
                    operation: "deposit",
                    detail: "a deposit worker panicked".to_owned(),
                })
            });
            if result.is_ok() {
                result = joined;
            }
        }
        result
    })?;

    let energy_after = sum(next.energy());
    let residual = (energy_after - transported.energy_before)
        - (injected_declared - transported.decay_removed);
    Ok(FieldStepReport {
        transport: transported,
        deposits,
        injected_declared,
        injected_applied,
        energy_after,
        residual,
    })
}
