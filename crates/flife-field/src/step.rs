//! Phase 2 of the tick (plan §8.1): transport the committed field, decay
//! it, then apply this tick's external sources. The result is the next
//! field's observation state. The step writes only the candidate buffer and
//! never commits. The engine validates the candidate (phase 10) and then
//! commits or discards it.

use std::num::NonZeroUsize;

use flife_core::memory::{capacity_bytes, total};
use flife_core::{CoreError, TimeStep};

use crate::deposit::{DepositSummary, Deposits, apply_deposits_to_carrier_on};
use crate::footprint::BufferLimits;
use crate::grid::{CARRIER1_ENERGY_PLANE, ENERGY_PLANE, FIELD_PLANES, FieldGrid};
use crate::invariants::{ConeTolerance, FieldScan};
use crate::layout::overflow;
use crate::sources::{
    DepositReport, Source, SourceKernel, SourceSupport, component, deposit, deposit_energy,
    deposit_energy_fused, deposit_moment, deposit_moment_band_group, deposit_moments_fused,
};
use crate::state::{FieldPair, FieldState};
use crate::transport::{
    CarrierTransport, FluxEdges, TightJunctions, TransportPlan, TransportReport, TransportScratch,
    VascularField, count_cell_occupancy, transport, transport_dual_carrier_on, transport_flux_on,
    transport_on,
};
use crate::uptake::{Grants, apply_grants_on};

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
    if !kernel.is_fused() {
        let mut deposits = Vec::with_capacity(sources.len());
        for source in sources {
            deposits.push(deposit(next, kernel, source, step, support)?);
        }
        return Ok(step_report(transported, deposits, next));
    }
    let grid = *next.grid();
    let mut supports = vec![SourceSupport::new(); sources.len()];
    for (source, slot) in sources.iter().zip(&mut supports) {
        if source.power() == 0.0 {
            slot.clear_for_zero_power();
        } else {
            kernel.support_into(&grid, source.center(), slot)?;
        }
    }
    let mut accum = Vec::new();
    let deposits = deposit_energy_fused(next, sources, step, &mut supports, &mut accum)?;
    let channels = next.carrier0_moments_count();
    let cells = usize::try_from(grid.cell_count()).map_err(|_| overflow())?;
    let moments = next.moments_mut();
    if cells == 0 || moments.len() != cells * channels {
        return Err(crate::layout::mismatch(
            "the candidate field has no moment planes".to_owned(),
        ));
    }
    for (channel, plane) in moments.chunks_exact_mut(cells).enumerate() {
        deposit_moments_fused(plane, sources, channel, &supports, &mut accum)?;
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
    let energy_after = next.energy_total();
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

/// Optional `field-ecology-v2` Phase 2 overrides (`V2-02` heterogeneous
/// per-source oasis kernels and `V2-09` passive symmetric tight-junction face
/// conductances). When both are `None`, Phase 2 executes the exact `v1` path
/// bit for bit.
#[derive(Debug, Clone, Copy, Default)]
pub struct FieldStepOptions<'a> {
    /// Per-source [`SourceKernel`]s (`V2-02` heterogeneous `σ_j`). Must match
    /// `sources.len()` when `Some`.
    pub source_kernels: Option<&'a [SourceKernel]>,
    /// Precomputed symmetric face conductances (`V2-09` tight junctions /
    /// `V2-14` vascular field).
    pub flux_edges: Option<&'a FluxEdges>,
}

impl FieldStepOptions<'_> {
    /// Exact `v1` baseline options (uniform kernel and uniform stencil).
    pub const BASELINE: Self = Self {
        source_kernels: None,
        flux_edges: None,
    };

    /// Whether no v2 Phase 2 override is active.
    #[must_use]
    pub const fn is_baseline(&self) -> bool {
        self.source_kernels.is_none() && self.flux_edges.is_none()
    }
}

/// Extended Wave-2 Phase 2 plan composing `V2-02` (`oases`), `V2-09`
/// (`tight-junctions`), `V2-14` (`vascular-field`), and `V2-15`
/// (`dual-carrier-trails`) without breaking [`FieldStepOptions`].
#[derive(Debug, Clone, Copy, Default)]
pub struct FieldStepPhase2<'a> {
    /// Per-source [`SourceKernel`]s (`V2-02` heterogeneous `σ_j`).
    pub source_kernels: Option<&'a [SourceKernel]>,
    /// Precomputed Carrier-0 symmetric face conductances (`V2-09` tight
    /// junctions and/or `V2-14` vascular arteries).
    pub carrier0_edges: Option<&'a FluxEdges>,
    /// Murray / *Physarum* vascular update rule (`V2-14`). When `Some`, updates
    /// candidate `edge_x_cond, edge_y_cond` from committed Carrier-0 `e_0` and
    /// `carrier0_edges`.
    pub vascular: Option<&'a VascularField>,
    /// Carrier-1 slow trail transport plan (`V2-15`, required when `pair` is
    /// 34-plane dual-carrier).
    pub trail_plan: Option<&'a TransportPlan>,
    /// Carrier-1 tight-junction-only face conductances (`V2-15` + `V2-09`;
    /// never accelerated by `V2-14` vascular arteries).
    pub trail_edges: Option<&'a FluxEdges>,
}

impl<'a> FieldStepPhase2<'a> {
    /// Exact `v1` baseline plan.
    pub const BASELINE: Self = Self {
        source_kernels: None,
        carrier0_edges: None,
        vascular: None,
        trail_plan: None,
        trail_edges: None,
    };

    /// Lift Wave-1 [`FieldStepOptions`] into [`FieldStepPhase2`].
    #[must_use]
    pub const fn from_options(options: FieldStepOptions<'a>) -> Self {
        Self {
            source_kernels: options.source_kernels,
            carrier0_edges: options.flux_edges,
            vascular: None,
            trail_plan: None,
            trail_edges: None,
        }
    }

    /// Whether only Wave-1 options (33-plane `V2-02` / `V2-09`) are set.
    #[must_use]
    pub const fn is_wave1_only(&self) -> bool {
        self.vascular.is_none() && self.trail_plan.is_none() && self.trail_edges.is_none()
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

    /// [`advance_field_v2_on`]'s step with `field-ecology-v2` Phase 2 options
    /// (`V2-02` heterogeneous/zero-power oases and `V2-09` flux-form tight
    /// junctions).
    ///
    /// Backends that do not override this delegate directly to
    /// [`FieldBackend::advance`] when `options.is_baseline()` and every source
    /// has positive power, and otherwise return [`CoreError::MissingCapability`]
    /// so the world falls back cleanly to the reference without silent
    /// divergence.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] when unimplemented for active v2
    /// options, or as [`FieldBackend::advance`].
    fn advance_v2(
        &mut self,
        pair: &mut FieldPair,
        plan: &TransportPlan,
        kernel: &SourceKernel,
        sources: &[Source],
        step: TimeStep,
        options: FieldStepOptions<'_>,
    ) -> Result<FieldStepReport, CoreError> {
        if !pair.current().is_dual_carrier()
            && !pair.current().has_vascular()
            && options.is_baseline()
            && sources.iter().all(|source| source.power() > 0.0)
        {
            return self.advance(pair, plan, kernel, sources, step);
        }
        Err(CoreError::MissingCapability {
            capability: "v2 field advance (oases / tight-junctions)".into(),
        })
    }

    /// [`advance_field_wave2_on`]'s step composing `V2-02`, `V2-09`, `V2-14`
    /// (`vascular-field`), and `V2-15` (`dual-carrier-trails`).
    ///
    /// Delegates to [`FieldBackend::advance_v2`] when no Wave-2 feature
    /// (`vascular` / `dual-carrier`) is active, and otherwise returns
    /// [`CoreError::MissingCapability`] until overridden by a device backend.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] when unimplemented for active Wave-2
    /// features, or as [`FieldBackend::advance_v2`].
    fn advance_wave2(
        &mut self,
        pair: &mut FieldPair,
        plan: &TransportPlan,
        kernel: &SourceKernel,
        sources: &[Source],
        step: TimeStep,
        phase2: FieldStepPhase2<'_>,
    ) -> Result<FieldStepReport, CoreError> {
        if !pair.current().is_dual_carrier()
            && !pair.current().has_vascular()
            && phase2.is_wave1_only()
        {
            return self.advance_v2(
                pair,
                plan,
                kernel,
                sources,
                step,
                FieldStepOptions {
                    source_kernels: phase2.source_kernels,
                    flux_edges: phase2.carrier0_edges,
                },
            );
        }
        Err(CoreError::MissingCapability {
            capability: "v2 wave-2 field advance (vascular-field / dual-carrier-trails)".into(),
        })
    }

    /// [`reduce_field_on`]'s phase 9 (FL-032): `grants`' debit, then each
    /// of `deposits` reduced in turn, into `field`, the tick's candidate,
    /// bit for bit, with each reduction's summary.
    ///
    /// # Errors
    /// Any. The field, the grants and the deposits are then as they were:
    /// a backend writes the field once every part has succeeded, and only
    /// then marks the grants and deposits applied. The world reduces the
    /// tick on the reference, which reproduces the error or finds none,
    /// and stops using a backend that failed where the reference did not.
    fn reduce(
        &mut self,
        field: &mut FieldState,
        grants: &mut Grants,
        deposits: [&mut Deposits; 2],
    ) -> Result<[DepositSummary; 2], CoreError>;

    /// Dual-carrier (`34`-plane, `V2-15`) phase 9 reduction: debits `grants`
    /// strictly from Carrier 0, reduces `carrier0_deposits` (`[emissions_0,
    /// recycling]`) into Carrier 0 `(0, 1, 24)`, and reduces `trail_deposits`
    /// into Carrier 1 `(25, 26, 8)`.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] by default until overridden, or as
    /// [`reduce_dual_carrier_field_on`].
    fn reduce_dual_carrier(
        &mut self,
        _field: &mut FieldState,
        _grants: &mut Grants,
        _carrier0_deposits: [&mut Deposits; 2],
        _trail_deposits: &mut Deposits,
    ) -> Result<([DepositSummary; 2], DepositSummary), CoreError> {
        Err(CoreError::MissingCapability {
            capability: "34-plane dual-carrier phase 9 reduction".into(),
        })
    }

    /// Phase 10's scan of INV-01 and INV-02 on `field`, the tick's
    /// candidate, at `tolerance` (FL-032): [`FieldState::scan_invariants`],
    /// bit for bit. The world builds the tick's summary from it with
    /// [`FieldState::summarize_scan`], and has the reference name the error
    /// when the scan finds a failing cell.
    ///
    /// # Errors
    /// Any. The world then checks the field on the reference and stops
    /// using the backend.
    fn check(
        &mut self,
        field: &FieldState,
        tolerance: ConeTolerance,
    ) -> Result<FieldScan, CoreError>;
}

/// Phase 9 (plan §8.1): `grants`' debit ([`apply_grants_on`]), then each of
/// `deposits` reduced in turn ([`apply_deposits_on`]: the emissions, then
/// the recycling), into `field`, the tick's candidate, on up to `workers`
/// threads. Returns each reduction's summary.
///
/// # Errors
/// As [`apply_grants_on`] and [`apply_deposits_on`], in that order.
pub fn reduce_field_on(
    field: &mut FieldState,
    grants: &mut Grants,
    deposits: [&mut Deposits; 2],
    workers: usize,
) -> Result<[DepositSummary; 2], CoreError> {
    apply_grants_on(field, grants, workers)?;
    let [first, second] = deposits;
    let carrier = field.carrier0();
    let mut out = [None; 2];
    crate::deposit::apply_deposit_streams_to_carrier_on(
        field,
        &mut [first, second],
        carrier,
        workers,
        &mut out,
    )?;
    match out {
        [Some(a), Some(b)] => Ok([a, b]),
        _ => Err(CoreError::StateConflict {
            operation: "reduce field deposits",
            detail: "missing deposit reduction summaries".to_owned(),
        }),
    }
}

/// Phase 9 for a 34-plane dual-carrier field (`V2-15`): debits `grants`
/// strictly from Carrier 0 (leaving Carrier 1 bit-for-bit untouched), reduces
/// `carrier0_deposits` (`[emissions_0, recycling]`) onto Carrier 0 `(0, 1, 24)`,
/// and reduces `trail_deposits` onto Carrier 1 `(25, 26, 8)`.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] if `field` is not dual-carrier, or as
/// [`apply_grants_on`] and [`apply_deposits_to_carrier_on`].
pub fn reduce_dual_carrier_field_on(
    field: &mut FieldState,
    grants: &mut Grants,
    carrier0_deposits: [&mut Deposits; 2],
    trail_deposits: &mut Deposits,
    workers: usize,
) -> Result<([DepositSummary; 2], DepositSummary), CoreError> {
    let Some(carrier1) = field.carrier1() else {
        return Err(crate::layout::mismatch(
            "reduce_dual_carrier_field_on requires a 34-plane dual-carrier field".into(),
        ));
    };
    let carrier0 = field.carrier0();
    apply_grants_on(field, grants, workers)?;
    let [emissions_0, recycling] = carrier0_deposits;
    let mut c0_out = [None; 2];
    crate::deposit::apply_deposit_streams_to_carrier_on(
        field,
        &mut [emissions_0, recycling],
        carrier0,
        workers,
        &mut c0_out,
    )?;
    let c0_summaries = match c0_out {
        [Some(a), Some(b)] => [a, b],
        _ => {
            return Err(CoreError::StateConflict {
                operation: "reduce dual-carrier field deposits",
                detail: "missing Carrier 0 deposit reduction summaries".to_owned(),
            });
        }
    };
    let c1_summary = apply_deposits_to_carrier_on(field, trail_deposits, carrier1, workers)?;
    Ok((c0_summaries, c1_summary))
}

/// Reusable scratch for [`advance_field_on`]: one transport scratch per CPU
/// worker and one support per source.
#[derive(Debug)]
pub struct FieldWork {
    grid: FieldGrid,
    limits: BufferLimits,
    transport: Vec<TransportScratch>,
    supports: Vec<SourceSupport>,
    /// Per-worker F64 cell accumulators, reserved only for fused source kernels
    /// (`flife-ze4`). Empty (0 bytes) for `normalized-truncated-minimum-image-gaussian-v1`.
    fused_accum: Vec<Vec<f64>>,
    /// Whether the world's source kernel uses fused per-tick F64 accumulation.
    fused: bool,
    /// Reusable per-cell organism occupancy scratch (`V2-09` tight junctions).
    /// Empty (0 bytes) when disabled.
    cell_occupancy: Vec<u32>,
    /// Reusable precomputed Carrier-0 symmetric face conductances (`V2-09`
    /// tight junctions and/or `V2-14` vascular field). `None` (0 bytes) when
    /// disabled.
    flux_edges: Option<FluxEdges>,
    /// Reusable precomputed Carrier-1 tight-junction-only face conductances
    /// (`V2-15` + `V2-09`). `None` (0 bytes) when disabled.
    trail_flux_edges: Option<FluxEdges>,
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
            fused_accum: Vec::new(),
            fused: false,
            cell_occupancy: Vec::new(),
            flux_edges: None,
            trail_flux_edges: None,
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
        let fused = kernel.is_fused();
        let fused_accum = if fused {
            let cells = usize::try_from(self.grid.cell_count()).map_err(|_| overflow())?;
            let lanes = fused_lane_count(self.transport.len())?;
            allocate_fused_accumulators(cells, lanes)?
        } else {
            Vec::new()
        };
        self.supports = supports;
        self.fused_accum = fused_accum;
        self.fused = fused;
        Ok(())
    }

    /// Give it a support sized individually for each per-source kernel in
    /// `kernels` (`V2-02` heterogeneous oasis `σ_j`), so steady-state steps
    /// allocate zero bytes (FL-043).
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] if `kernels` mixes fused and non-fused
    /// modes, or as [`SourceSupport::for_kernel`]; nothing changes on error.
    pub fn reserve_supports_for_kernels(
        &mut self,
        kernels: &[SourceKernel],
    ) -> Result<(), CoreError> {
        let fused = kernels.first().is_some_and(SourceKernel::is_fused);
        if kernels.iter().any(|kernel| kernel.is_fused() != fused) {
            return Err(CoreError::StateConflict {
                operation: "reserve per-source supports",
                detail: "all per-source kernels must share one fused mode".into(),
            });
        }
        let mut supports = Vec::new();
        supports
            .try_reserve_exact(kernels.len())
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "source supports",
                bytes: flife_core::memory::bytes_for::<SourceSupport>(kernels.len()),
            })?;
        for kernel in kernels {
            supports.push(SourceSupport::for_kernel(kernel, &self.grid)?);
        }
        let fused_accum = if fused {
            let cells = usize::try_from(self.grid.cell_count()).map_err(|_| overflow())?;
            let lanes = fused_lane_count(self.transport.len())?;
            allocate_fused_accumulators(cells, lanes)?
        } else {
            Vec::new()
        };
        self.supports = supports;
        self.fused_accum = fused_accum;
        self.fused = fused;
        Ok(())
    }

    /// Reserve reusable `cell_occupancy` and `FluxEdges` scratch for `V2-09`
    /// tight-junction diffusion, admitted against `self.limits` (FL-043 /
    /// INV-15). Leaves 0 bytes allocated unless called.
    ///
    /// # Errors
    /// [`CoreError::BudgetExhausted`], [`CoreError::AllocationFailed`], or
    /// [`CoreError::Overflow`]; nothing changes on error.
    pub fn reserve_flux(&mut self) -> Result<(), CoreError> {
        let cells = usize::try_from(self.grid.cell_count()).map_err(|_| overflow())?;
        let occ_bytes = self.grid.cell_count().checked_mul(4).ok_or_else(overflow)?;
        self.limits
            .admit("cell occupancy scratch bytes", occ_bytes)?;
        let mut occupancy = Vec::new();
        occupancy
            .try_reserve_exact(cells)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "cell occupancy scratch",
                bytes: occ_bytes,
            })?;
        occupancy.resize(cells, 0);
        let edges = FluxEdges::new(&self.grid, &self.limits)?;
        self.cell_occupancy = occupancy;
        self.flux_edges = Some(edges);
        Ok(())
    }

    /// Recompute `cell_occupancy` and `flux_edges` from post-Phase-1 organism
    /// `positions` for `barrier` and `plan`, allocating scratch first if
    /// [`FieldWork::reserve_flux`] has not been called yet.
    ///
    /// # Errors
    /// As [`FieldWork::reserve_flux`], [`count_cell_occupancy`], and
    /// [`TightJunctions::fill_edges`].
    pub fn prepare_tight_junctions(
        &mut self,
        barrier: &TightJunctions,
        plan: &TransportPlan,
        positions: &[[f64; 2]],
    ) -> Result<&FluxEdges, CoreError> {
        if self.flux_edges.is_none() {
            self.reserve_flux()?;
        }
        count_cell_occupancy(&self.grid, positions, &mut self.cell_occupancy)?;
        let Some(edges) = self.flux_edges.as_mut() else {
            return Err(CoreError::StateConflict {
                operation: "prepare tight junctions",
                detail: "flux edge scratch was not initialized".into(),
            });
        };
        barrier.fill_edges(plan, &self.cell_occupancy, edges)?;
        Ok(&*edges)
    }

    /// The last computed cell occupancy counts, if `V2-09` scratch is active.
    #[must_use]
    pub fn cell_occupancy(&self) -> &[u32] {
        &self.cell_occupancy
    }

    /// The last computed Carrier-0 flux edges, if `V2-09` / `V2-14` scratch is active.
    #[must_use]
    pub fn flux_edges(&self) -> Option<&FluxEdges> {
        self.flux_edges.as_ref()
    }

    /// Reserve reusable Carrier-1 `FluxEdges` scratch for `V2-15` + `V2-09`
    /// tight-junction trail transport (`0 B` unless called).
    ///
    /// # Errors
    /// As [`FluxEdges::new`].
    pub fn reserve_trail_flux(&mut self) -> Result<(), CoreError> {
        self.trail_flux_edges = Some(FluxEdges::new(&self.grid, &self.limits)?);
        Ok(())
    }

    /// Recompute Carrier-0 `flux_edges` from `vascular` (`V2-14`), committed
    /// `(edge_x_cond, edge_y_cond)`, and optional `V2-09` `(barrier, positions)`.
    ///
    /// # Errors
    /// As [`FieldWork::reserve_flux`], [`count_cell_occupancy`], and
    /// [`VascularField::fill_edges`].
    pub fn prepare_vascular_edges(
        &mut self,
        vascular: &VascularField,
        plan: &TransportPlan,
        edge_x_cond: &[f32],
        edge_y_cond: &[f32],
        barrier: Option<(&TightJunctions, &[[f64; 2]])>,
    ) -> Result<&FluxEdges, CoreError> {
        if self.flux_edges.is_none() {
            self.reserve_flux()?;
        }
        let barrier_occ = if let Some((tj, positions)) = barrier {
            count_cell_occupancy(&self.grid, positions, &mut self.cell_occupancy)?;
            Some((tj, self.cell_occupancy.as_slice()))
        } else {
            None
        };
        let Some(edges) = self.flux_edges.as_mut() else {
            return Err(CoreError::StateConflict {
                operation: "prepare vascular edges",
                detail: "flux edge scratch was not initialized".into(),
            });
        };
        vascular.fill_edges(plan, edge_x_cond, edge_y_cond, barrier_occ, edges)?;
        Ok(&*edges)
    }

    /// Recompute Carrier-1 tight-junction-only `trail_flux_edges` from already
    /// computed `cell_occupancy` (`V2-15` + `V2-09`, never accelerated by
    /// `V2-14` vascular arteries).
    ///
    /// # Errors
    /// As [`FieldWork::reserve_trail_flux`] and [`TightJunctions::fill_edges`].
    pub fn prepare_trail_tight_junctions(
        &mut self,
        barrier: &TightJunctions,
        trail_plan: &TransportPlan,
    ) -> Result<&FluxEdges, CoreError> {
        if self.trail_flux_edges.is_none() {
            self.reserve_trail_flux()?;
        }
        let Some(edges) = self.trail_flux_edges.as_mut() else {
            return Err(CoreError::StateConflict {
                operation: "prepare trail tight junctions",
                detail: "trail flux edge scratch was not initialized".into(),
            });
        };
        barrier.fill_edges(trail_plan, &self.cell_occupancy, edges)?;
        Ok(&*edges)
    }

    /// The last computed Carrier-1 trail flux edges, if active.
    #[must_use]
    pub fn trail_flux_edges(&self) -> Option<&FluxEdges> {
        self.trail_flux_edges.as_ref()
    }

    /// Bytes its transport scratch, source supports, and optional `V2-09` /
    /// `V2-14` / `V2-15` flux scratch hold (FL-043).
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        total(
            [
                capacity_bytes(&self.transport),
                capacity_bytes(&self.supports),
                capacity_bytes(&self.fused_accum),
                capacity_bytes(&self.cell_occupancy),
                self.flux_edges
                    .as_ref()
                    .map_or(0, FluxEdges::allocated_bytes),
                self.trail_flux_edges
                    .as_ref()
                    .map_or(0, FluxEdges::allocated_bytes),
            ]
            .into_iter()
            .chain(self.transport.iter().map(TransportScratch::allocated_bytes))
            .chain(self.supports.iter().map(SourceSupport::allocated_bytes))
            .chain(self.fused_accum.iter().map(capacity_bytes)),
        )
    }

    /// Use `workers` threads from the next step.
    ///
    /// # Errors
    /// As [`TransportScratch::new`]; the count is then unchanged.
    pub fn set_workers(&mut self, workers: NonZeroUsize) -> Result<(), CoreError> {
        let workers = workers.get();
        let mut added_transport = Vec::new();
        for _ in self.transport.len()..workers {
            added_transport.push(TransportScratch::new(&self.grid, &self.limits)?);
        }
        let target_lanes = if self.fused {
            fused_lane_count(workers)?
        } else {
            0
        };
        let added_fused = if target_lanes > self.fused_accum.len() {
            let cells = usize::try_from(self.grid.cell_count()).map_err(|_| overflow())?;
            allocate_fused_accumulators(cells, target_lanes - self.fused_accum.len())?
        } else {
            Vec::new()
        };
        if workers > self.transport.len() {
            self.transport
                .try_reserve_exact(workers - self.transport.len())
                .map_err(|_| CoreError::AllocationFailed {
                    buffer: "transport scratches",
                    bytes: flife_core::memory::bytes_for::<TransportScratch>(workers),
                })?;
        }
        if target_lanes > self.fused_accum.len() {
            self.fused_accum
                .try_reserve_exact(target_lanes - self.fused_accum.len())
                .map_err(|_| CoreError::AllocationFailed {
                    buffer: "fused source accumulators",
                    bytes: flife_core::memory::bytes_for::<Vec<f64>>(target_lanes),
                })?;
        }
        self.transport.extend(added_transport);
        self.transport.truncate(workers);
        self.transport.shrink_to(workers);
        if self.fused {
            self.fused_accum.extend(added_fused);
            self.fused_accum.truncate(target_lanes);
            self.fused_accum.shrink_to(target_lanes);
        }
        Ok(())
    }
}

fn fused_lane_count(workers: usize) -> Result<usize, CoreError> {
    let channels = usize::try_from(FIELD_PLANES - 1).map_err(|_| overflow())?;
    Ok(workers.clamp(1, channels.max(1)))
}

fn allocate_fused_accumulators(cells: usize, count: usize) -> Result<Vec<Vec<f64>>, CoreError> {
    let mut buffers = Vec::new();
    buffers
        .try_reserve_exact(count)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: "fused source accumulators",
            bytes: flife_core::memory::bytes_for::<Vec<f64>>(count),
        })?;
    for _ in 0..count {
        let mut buf = Vec::new();
        buf.try_reserve_exact(cells)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "fused source accumulator",
                bytes: flife_core::memory::bytes_for::<f64>(cells),
            })?;
        buf.resize(cells, 0.0);
        buffers.push(buf);
    }
    Ok(buffers)
}

/// [`advance_field`] on up to [`FieldWork::workers`] threads: this one and
/// scoped workers it joins before returning (plan §3.4's bounded reference
/// adapter). Planes are transported independently ([`transport_on`]). Each
/// source's support is computed on the workers, in contiguous runs of
/// sources: a support depends only on the grid and its source's center.
/// Every source's energy is then deposited in source order on this thread,
/// and the moment planes are split over the workers, each adding the
/// sources in the same order. Every value is computed exactly as
/// [`advance_field`] computes it, and the error is the first in source
/// order, returned once the sources before it are deposited.
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
    advance_field_v2_on(
        pair,
        plan,
        kernel,
        sources,
        step,
        FieldStepOptions::BASELINE,
        work,
    )
}

/// [`advance_field_on`] with optional `field-ecology-v2` Phase 2 overrides
/// (`V2-02` heterogeneous per-source oasis kernels and `V2-09` passive
/// symmetric tight-junction face conductances).
///
/// With [`FieldStepOptions::BASELINE`], executes the exact `v1` baseline
/// operators bit for bit.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] if `options.source_kernels` does not match
/// `sources.len()`, [`CoreError::StateConflict`] if per-source kernels mix
/// fused and non-fused modes or a worker panics, or as [`advance_field_on`]
/// and [`transport_flux_on`].
pub fn advance_field_v2_on(
    pair: &mut FieldPair,
    plan: &TransportPlan,
    kernel: &SourceKernel,
    sources: &[Source],
    step: TimeStep,
    options: FieldStepOptions<'_>,
    work: &mut FieldWork,
) -> Result<FieldStepReport, CoreError> {
    advance_field_wave2_on(
        pair,
        plan,
        kernel,
        sources,
        step,
        FieldStepPhase2::from_options(options),
        work,
    )
}

/// [`advance_field_on`] composing all Wave-1 & Wave-2 Phase 2 mechanisms:
/// `V2-02` (`oases`), `V2-09` (`tight-junctions`), `V2-14` (`vascular-field`),
/// and `V2-15` (`dual-carrier-trails`).
///
/// With [`FieldStepPhase2::BASELINE`] on a 33-plane field without vascular
/// state, executes the exact `v1` baseline operators bit for bit.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] for mismatched `source_kernels`, missing
/// `trail_plan` on a 34-plane field, or missing vascular buffers on `pair`;
/// [`CoreError::StateConflict`] if per-source kernels mix fused modes,
/// `vascular` is set without `carrier0_edges`, or a worker panics; or as
/// [`transport_on`] / [`transport_flux_range_on`] / [`VascularField::update_edges`].
pub fn advance_field_wave2_on(
    pair: &mut FieldPair,
    plan: &TransportPlan,
    kernel: &SourceKernel,
    sources: &[Source],
    step: TimeStep,
    phase2: FieldStepPhase2<'_>,
    work: &mut FieldWork,
) -> Result<FieldStepReport, CoreError> {
    if let Some(kernels) = phase2.source_kernels {
        if kernels.len() != sources.len() {
            return Err(CoreError::LayoutMismatch {
                buffer: "per-source kernels",
                detail: format!("{} kernels for {} sources", kernels.len(), sources.len()),
            });
        }
        if kernels.iter().any(|k| k.is_fused() != kernel.is_fused()) {
            return Err(CoreError::StateConflict {
                operation: "advance field",
                detail: "per-source kernels must share the base kernel's fused mode".into(),
            });
        }
    }
    let FieldWork {
        transport: scratches,
        supports,
        fused_accum,
        ..
    } = work;
    let (current, next) = pair.stage();
    if current.is_dual_carrier() != next.is_dual_carrier() {
        return Err(crate::layout::mismatch(
            "current and candidate field states disagree on carrier layout".into(),
        ));
    }
    let transported = if let Some(carrier1) = current.carrier1() {
        let Some(trail_plan) = phase2.trail_plan else {
            return Err(CoreError::LayoutMismatch {
                buffer: "dual-carrier trail transport plan",
                detail: "34-plane dual-carrier advance requires phase2.trail_plan".into(),
            });
        };
        let c0 = CarrierTransport {
            plan,
            edges: phase2.carrier0_edges,
            range: current.carrier0().plane_range(),
            energy_plane: ENERGY_PLANE as usize,
        };
        let c1 = CarrierTransport {
            plan: trail_plan,
            edges: phase2.trail_edges,
            range: carrier1.plane_range(),
            energy_plane: CARRIER1_ENERGY_PLANE as usize,
        };
        transport_dual_carrier_on(&c0, &c1, current, next, scratches)?
    } else {
        match phase2.carrier0_edges {
            Some(edges) => transport_flux_on(plan, edges, current, next, scratches)?,
            None => transport_on(plan, current, next, scratches)?,
        }
    };

    if let Some(vascular) = phase2.vascular {
        let Some(edges) = phase2.carrier0_edges else {
            return Err(CoreError::StateConflict {
                operation: "advance vascular field",
                detail: "phase2.vascular requires precomputed phase2.carrier0_edges".into(),
            });
        };
        let (Some(prev_edges), Some(next_edges)) =
            (current.vascular_edges(), next.vascular_edges_mut())
        else {
            return Err(crate::layout::mismatch(
                "phase2.vascular requires vascular edge buffers enabled on FieldPair".into(),
            ));
        };
        vascular.update_edges(plan, step, edges, current.energy(), prev_edges, next_edges)?;
    }

    if supports.len() < sources.len() {
        supports.resize_with(sources.len(), SourceSupport::new);
    }
    let supports = &mut supports[..sources.len()];
    let grid = *next.grid();
    let fused = kernel.is_fused();
    let refused = supports_on(
        kernel,
        phase2.source_kernels,
        &grid,
        sources,
        supports,
        (!fused).then_some(step),
        scratches.len(),
    )?;
    let supported = refused.as_ref().map_or(sources.len(), |(index, _)| *index);
    let cells = usize::try_from(grid.cell_count()).map_err(|_| overflow())?;
    let channels = next.carrier0_moments_count();

    let deposits = if fused {
        if let Some((_, error)) = refused {
            return Err(error);
        }
        let used = scratches.len().clamp(1, channels.max(1));
        if fused_accum.len() < used {
            fused_accum.resize_with(used, Vec::new);
        }
        deposit_energy_fused(next, sources, step, supports, &mut fused_accum[0])?
    } else {
        let mut deposits = Vec::with_capacity(sources.len());
        for (source, support) in sources.iter().zip(supports.iter_mut()).take(supported) {
            deposits.push(deposit_energy(next, source, step, support)?);
        }
        if let Some((_, error)) = refused {
            return Err(error);
        }
        deposits
    };

    let supports: &[SourceSupport] = supports;
    let moments = next.moments_mut();
    if cells == 0 || moments.len() != cells * channels {
        return Err(crate::layout::mismatch(
            "the candidate field has no moment planes".to_owned(),
        ));
    }
    let worker_budget = scratches.len();
    let used = worker_budget.clamp(1, channels.max(1));
    if fused {
        if fused_accum.len() < used {
            fused_accum.resize_with(used, Vec::new);
        }
        let mut plane_tasks: Vec<&mut [f32]> = moments.chunks_exact_mut(cells).collect();
        flife_core::pool::try_parallel_tasks_with_lanes(
            worker_budget,
            "deposit",
            &mut fused_accum[..used],
            &mut plane_tasks,
            |channel, plane, accum| deposit_moments_fused(plane, sources, channel, supports, accum),
        )?;
    } else if worker_budget > 1
        && cells >= 16_384
        && channels >= 4
        && supports.iter().all(|s| !s.has_cell_out_of_bounds(cells))
    {
        // Partition moment planes into 4-plane groups and each plane into
        // L2-resident cell bands so every worker shares `support.amounts`
        // across 4 planes with zero `cell_indices` traffic and balanced
        // P/E-core work stealing (0-ulp bit-identical to per-plane deposit).
        const GROUP_PLANES: usize = 4;
        let bands = (cells / 32_768).clamp(4, 16);
        let band_len = cells.div_ceil(bands);
        struct BandGroupTask<'a> {
            band_start: usize,
            band_end: usize,
            channel_base: usize,
            planes: Vec<&'a mut [f32]>,
        }
        let group_count = channels.div_ceil(GROUP_PLANES);
        let mut tasks: Vec<BandGroupTask<'_>> = Vec::with_capacity(group_count * bands);
        for (group_idx, group_buf) in moments.chunks_mut(GROUP_PLANES * cells).enumerate() {
            let channel_base = group_idx * GROUP_PLANES;
            let group_planes = group_buf.len() / cells;
            let mut plane_iters: Vec<_> = group_buf
                .chunks_exact_mut(cells)
                .map(|p| p.chunks_mut(band_len))
                .collect();
            for band_idx in 0..bands {
                let band_start = (band_idx * band_len).min(cells);
                let band_end = (band_start + band_len).min(cells);
                if band_start >= band_end {
                    break;
                }
                let mut band_planes = Vec::with_capacity(group_planes);
                for iter in &mut plane_iters {
                    if let Some(slice) = iter.next() {
                        band_planes.push(slice);
                    }
                }
                tasks.push(BandGroupTask {
                    band_start,
                    band_end,
                    channel_base,
                    planes: band_planes,
                });
            }
        }
        flife_core::pool::try_parallel_tasks(worker_budget, "deposit", &mut tasks, |_, task| {
            deposit_moment_band_group(
                task.band_start,
                task.band_end,
                task.channel_base,
                &mut task.planes,
                sources,
                supports,
            )
        })?;
    } else {
        let mut plane_tasks: Vec<&mut [f32]> = moments.chunks_exact_mut(cells).collect();
        flife_core::pool::try_parallel_tasks(
            worker_budget,
            "deposit",
            &mut plane_tasks,
            |channel, plane| {
                for (source, support) in sources.iter().zip(supports) {
                    deposit_moment(plane, component(source, channel), support)?;
                }
                Ok(())
            },
        )?;
    }
    Ok(step_report(transported, deposits, next))
}

/// Each source's support ([`SourceKernel::support_into`]) on up to
/// `workers` threads (zero is taken as one), the sources split into
/// contiguous runs, one run per thread. A run stops at its first refusal.
/// Returns the first refusal in source order with its source's index, or
/// `None` when every support is made.
///
/// # Errors
/// [`CoreError::StateConflict`] for a worker that panicked, when no run
/// before its own was refused.
fn supports_on(
    kernel: &SourceKernel,
    per_source: Option<&[SourceKernel]>,
    grid: &FieldGrid,
    sources: &[Source],
    supports: &mut [SourceSupport],
    prepare_step: Option<TimeStep>,
    workers: usize,
) -> Result<Option<(usize, CoreError)>, CoreError> {
    if sources.is_empty() {
        return Ok(None);
    }
    let build_one =
        |index: usize, source: &Source, support: &mut SourceSupport| -> Option<CoreError> {
            if source.power() == 0.0 {
                support.clear_for_zero_power();
                return None;
            }
            let active_kernel = per_source
                .and_then(|kernels| kernels.get(index))
                .unwrap_or(kernel);
            if let Err(err) = active_kernel.support_into(grid, source.center(), support) {
                return Some(err);
            }
            if let Some(step) = prepare_step {
                support.prepare_amounts(source, step);
            }
            None
        };
    if workers <= 1 || sources.len() == 1 {
        for (index, (source, support)) in sources.iter().zip(supports.iter_mut()).enumerate() {
            if let Some(error) = build_one(index, source, support) {
                return Ok(Some((index, error)));
            }
        }
        return Ok(None);
    }
    let mut tasks: Vec<(&Source, &mut SourceSupport, Option<CoreError>)> = sources
        .iter()
        .zip(supports.iter_mut())
        .map(|(source, support)| (source, support, None))
        .collect();
    flife_core::pool::parallel_tasks(
        workers,
        "source supports",
        &mut tasks,
        |index, (source, support, err)| {
            *err = build_one(index, source, support);
        },
    )?;
    for (index, (_, _, err)) in tasks.into_iter().enumerate() {
        if let Some(error) = err {
            return Ok(Some((index, error)));
        }
    }
    Ok(None)
}
