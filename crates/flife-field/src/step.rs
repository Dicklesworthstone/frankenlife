//! Phase 2 of the tick (plan §8.1): transport the committed field, decay
//! it, then apply this tick's external sources. The result is the next
//! field's observation state. The step writes only the candidate buffer and
//! never commits. The engine validates the candidate (phase 10) and then
//! commits or discards it.

use std::num::NonZeroUsize;

use flife_core::memory::{capacity_bytes, total};
use flife_core::{CoreError, TimeStep};

use crate::footprint::BufferLimits;
use crate::grid::{FIELD_PLANES, FieldGrid};
use crate::layout::overflow;
use crate::sources::{
    DepositReport, Source, SourceKernel, SourceSupport, component, deposit, deposit_energy,
    deposit_moment,
};
use crate::state::{FieldPair, FieldState};
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
    for source in sources {
        deposits.push(deposit(next, kernel, source, step, support)?);
    }
    Ok(step_report(transported, deposits, next))
}

/// The report of a step whose transport and deposits are done, with `next`
/// the candidate field: the injected totals in source order, the
/// candidate's energy and the step's residual. An accelerator that runs the
/// step itself reports through this, so its report is the reference's.
#[must_use]
pub fn step_report(
    transport: TransportReport,
    deposits: Vec<DepositReport>,
    next: &FieldState,
) -> FieldStepReport {
    let (mut injected_declared, mut injected_applied) = (0.0, 0.0);
    for report in &deposits {
        injected_declared += report.declared;
        injected_applied += report.applied;
    }
    let energy_after = sum(next.energy());
    let residual =
        (energy_after - transport.energy_before) - (injected_declared - transport.decay_removed);
    FieldStepReport {
        transport,
        deposits,
        injected_declared,
        injected_applied,
        energy_after,
        residual,
    }
}

/// The field backend port (plan §10.7, §11.1): phase 2 of the tick on an
/// accelerator. The composition root (the CLI, the Studio) chooses an
/// implementation and gives it to the world; the engine depends only on
/// this trait, and no backend depends on the engine.
///
/// A backend is an exact replacement for [`advance_field_on`], not an
/// approximation: the candidate it writes and the report it returns are the
/// reference's, bit for bit. Any error hands the step back to the
/// reference, which reproduces the reference's own error or finds none. So
/// no result of a run depends on the backend.
pub trait FieldBackend: std::fmt::Debug + Send {
    /// The backend and its device, for run records.
    fn describe(&self) -> String;

    /// [`advance_field_on`]'s step: transport `pair`'s committed field into
    /// its candidate, then deposit `sources`.
    ///
    /// # Errors
    /// Any; the candidate is then incomplete. The world redoes the step on
    /// the reference, and stops using the backend if the reference finds
    /// no error.
    fn advance(
        &mut self,
        pair: &mut FieldPair,
        plan: &TransportPlan,
        kernel: &SourceKernel,
        sources: &[Source],
        step: TimeStep,
    ) -> Result<FieldStepReport, CoreError>;
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
            limits: *limits,
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

    /// Give it a support for each of `sources` sources, each with room for
    /// any of `kernel`'s on its grid, so that no step allocates one (FL-043).
    ///
    /// # Errors
    /// As [`SourceSupport::for_kernel`]; nothing changes then.
    pub fn reserve_supports(
        &mut self,
        sources: usize,
        kernel: &SourceKernel,
    ) -> Result<(), CoreError> {
        let mut supports = Vec::new();
        supports
            .try_reserve_exact(sources)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "source supports",
                bytes: flife_core::memory::bytes_for::<SourceSupport>(sources),
            })?;
        for _ in 0..sources {
            supports.push(SourceSupport::for_kernel(kernel, &self.grid)?);
        }
        self.supports = supports;
        Ok(())
    }

    /// Bytes its transport scratch, one per worker, and its source supports
    /// hold (FL-043).
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        total(
            [
                capacity_bytes(&self.transport),
                capacity_bytes(&self.supports),
            ]
            .into_iter()
            .chain(self.transport.iter().map(TransportScratch::allocated_bytes))
            .chain(self.supports.iter().map(SourceSupport::allocated_bytes)),
        )
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
    for (source, support) in sources.iter().zip(supports.iter_mut()) {
        kernel.support_into(&grid, source.center(), support)?;
        deposits.push(deposit_energy(next, source, step, support)?);
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
    Ok(step_report(transported, deposits, next))
}
