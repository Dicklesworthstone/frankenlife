//! FL-031 (`flife-8le`), first part: phase 2 of the tick on Metal (plan
//! §8.1, §10.7, §11.1). [`MetalField`] is a [`FieldBackend`]: transport,
//! decay and the external sources, bit-identical to
//! [`flife_field::advance_field_on`].
//!
//! The work is split by what each side does best:
//! - the device runs everything per cell, in `field.metal` on
//!   `softfloat64.metal`'s binary64: every plane's five-point substeps,
//!   every candidate cell's source weight (its `exp`), and every moment
//!   value's decay and deposits;
//! - the host runs every sequential F64 sum, through flife-field's own
//!   functions: the energy totals ([`energy_report`]), each source's support
//!   and weight sum ([`SourceKernel::weigh_into`], fed the device's
//!   weights), the energy plane's deposits ([`deposit_energy`]) and the
//!   step's report ([`step_report`]).
//!
//! One step is two commands: transport and weights, then the moments once
//! the host has each source's deposit amounts (`deposit_energy`'s, written
//! over the candidates' weights). The field is copied to the
//! device and back every step; it is not yet resident (see the crate
//! contract's known gaps).
//!
//! Phase 9 (FL-032) is [`crate::reduce`]'s, and phase 10's field check
//! [`crate::check`]'s, on the same owner.

use std::ops::Range;

use flife_core::{CoreError, TimeStep};
use flife_field::grid::{
    CARRIER0_MOMENT_CHANNELS, CARRIER1_ENERGY_PLANE, DUAL_CARRIER_PLANES, FIELD_PLANES,
};
use flife_field::{
    ConeTolerance, DepositSummary, Deposits, FieldBackend, FieldGrid, FieldLayout, FieldPair,
    FieldScan, FieldState, FieldStepOptions, FieldStepPhase2, FieldStepReport, FluxEdges, Grants,
    Source, SourceKernel, SourceSupport, TransportPlan, TransportReport, combine_transport_reports,
    deposit_energy, deposit_energy_fused, energy_report, step_report,
};
use ft_kernel_metal::compute::{Grid, MathMode, Pipeline};

use crate::check::Check;
use crate::owner::{Binding, DeviceOwner, Kernel, Limits, Owned, Purpose, metal};
use crate::reduce::Reduce;

/// The field kernel source, with the software binary64, Philox, and shared
/// numerics (`d_pow_positive` for `V2-14` Murray vascular update) ahead of it.
pub const FIELD_MSL: &str = concat!(
    include_str!("softfloat64.metal"),
    include_str!("philox.metal"),
    include_str!("numerics.metal"),
    include_str!("field.metal")
);

/// Moment channels in single-carrier mode (`32`), and Carrier-1 total planes
/// (`e_1 + 8` trail moments = `9`) in 34-plane dual-carrier mode (`V2-15`).
const CHANNELS: usize = 32;
const CARRIER1_TOTAL_PLANES: usize = 9;

/// Words per source descriptor; the offsets are `field.metal`'s (77 words:
/// 73 baseline words + per-source `r_j²` and `2σ_j²` for `V2-02` oases).
const DESCRIPTOR: usize = 77;
const FIRST: usize = 0;
const ROWS: usize = 1;
const COLUMNS: usize = 2;
const ROWS_AT: usize = 3;
const COLUMNS_AT: usize = 4;
const CENTER_X: usize = 5;
const CENTER_Y: usize = 7;
const DIRECTION: usize = 9;
const RADIUS_SQUARED: usize = 73;
const TWO_SIGMA_SQUARED: usize = 75;

/// `field.metal`'s weight markers.
const ABSENT: u64 = u64::MAX;
const REFUSED: u64 = u64::MAX - 1;

/// A rank-table entry for a row or column that is no candidate.
const NOT_CANDIDATE: u32 = u32::MAX;

/// Words of the stencil, flux-stencil, weights-header, moments, trail-decay,
/// and vascular-update parameters.
const STENCIL_WORDS: usize = 11;
const FLUX_STENCIL_WORDS: usize = 5;
const HEADER_WORDS: usize = 10;
const MOMENT_WORDS: usize = 9;
const TRAIL_DECAY_WORDS: usize = 4;
const VASC_PARAMS_WORDS: usize = 17;

/// Threads per threadgroup.
const THREADS: usize = 256;

fn words(value: f64) -> [u32; 2] {
    let bits = value.to_bits();
    [(bits & 0xFFFF_FFFF) as u32, (bits >> 32) as u32]
}

fn put(target: &mut [u32], at: usize, value: f64) {
    target[at..at + 2].copy_from_slice(&words(value));
}

fn overflow() -> CoreError {
    CoreError::Overflow {
        quantity: "metal field size",
    }
}

/// A source's device weights and its host support disagree in count.
fn mismatch() -> CoreError {
    CoreError::StateConflict {
        operation: "metal source weights",
        detail: "the device weights and the support differ in count".to_owned(),
    }
}

/// Device buffers for `V2-09` flux-form tight-junction / `V2-14` vascular
/// diffusion, allocated lazily on first flux use (0 bytes on v1).
#[derive(Debug)]
struct FluxBuffers {
    params: Owned<u32>,
    edge_x: Owned<u32>,
    edge_y: Owned<u32>,
}

/// Device buffers for `V2-14` Murray / Physarum edge conductance updates,
/// allocated lazily only when `vascular-field` is active (0 bytes when disabled).
#[derive(Debug)]
struct VascularBuffers {
    params: Owned<u32>,
    prev_x: Owned<f32>,
    prev_y: Owned<f32>,
    next_x: Owned<f32>,
    next_y: Owned<f32>,
}

/// Device buffers for `V2-15` 34-plane Carrier-1 trail transport and decay,
/// allocated lazily only on 34-plane dual-carrier fields (0 bytes on 33-plane).
#[derive(Debug)]
struct TrailBuffers {
    stencil: Owned<u32>,
    decay_params: Owned<u32>,
    flux: Option<FluxBuffers>,
    out: Owned<f32>,
}

/// The device buffers for one grid, plane count and source count.
#[derive(Debug)]
struct Buffers {
    grid: FieldGrid,
    planes: usize,
    sources: usize,
    /// The committed field, then the substeps' two targets.
    field: Owned<f32>,
    ping: Owned<f32>,
    diffused: Owned<f32>,
    stencil: Owned<u32>,
    flux: Option<FluxBuffers>,
    vascular: Option<VascularBuffers>,
    trail: Option<TrailBuffers>,
    header: Owned<u32>,
    descriptors: Owned<u32>,
    axes: Owned<u32>,
    ranks: Owned<u32>,
    weights: Owned<u32>,
    moment_params: Owned<u32>,
    moments: Owned<f32>,
    fault: Owned<u32>,
}

/// What the host prepares and reads back each step.
#[derive(Debug, Default)]
struct Host {
    energy: Vec<f32>,
    readback: Vec<f32>,
    edge_x: Vec<u32>,
    edge_y: Vec<u32>,
    trail_edge_x: Vec<u32>,
    trail_edge_y: Vec<u32>,
    trail_out: Vec<f32>,
    descriptors: Vec<u32>,
    axes: Vec<u32>,
    ranks: Vec<u32>,
    weights: Vec<u32>,
    supports: Vec<SourceSupport>,
    fused_accum: Vec<f64>,
}

/// Phase 2 of the tick on Metal; see the module documentation.
pub struct MetalField {
    owner: DeviceOwner,
    stencil: Pipeline,
    stencil_flux: Pipeline,
    weights: Pipeline,
    moments: Pipeline,
    trail_decay: Pipeline,
    vascular_update: Pipeline,
    buffers: Option<Buffers>,
    host: Host,
    reduce: Reduce,
    check: Check,
}

impl std::fmt::Debug for MetalField {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MetalField")
            .field("device", &self.owner.device_name())
            .field("buffers", &self.buffers)
            .finish_non_exhaustive()
    }
}

impl MetalField {
    /// Open the default Metal device and compile the field kernels.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] without a Metal device (always, off
    /// macOS); [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn open() -> Result<Self, CoreError> {
        Self::with_owner(DeviceOwner::open(Limits::UNBOUNDED)?)
    }

    /// The field kernels on `owner`'s device; their buffers are `owner`'s.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] if the kernels do not compile.
    pub fn with_owner(owner: DeviceOwner) -> Result<Self, CoreError> {
        let library = owner.library(FIELD_MSL, MathMode::Safe)?;
        let pipeline = |name: &str| library.pipeline(name).map_err(metal);
        Ok(Self {
            stencil: pipeline("field_stencil")?,
            stencil_flux: pipeline("field_stencil_flux")?,
            weights: pipeline("source_weights")?,
            moments: pipeline("source_moments")?,
            trail_decay: pipeline("trail_decay")?,
            vascular_update: pipeline("vascular_update")?,
            reduce: Reduce::new(&owner)?,
            check: Check::new(&owner)?,
            owner,
            buffers: None,
            host: Host::default(),
        })
    }

    /// The device's name.
    #[must_use]
    pub fn device_name(&self) -> String {
        self.owner.device_name()
    }

    /// The owner of every buffer these kernels use.
    #[must_use]
    pub const fn owner(&self) -> &DeviceOwner {
        &self.owner
    }

    /// Buffers for `grid`, at least `planes` field planes, and `sources`
    /// sources, allocated on first use and again when any grows/changes; the
    /// old ones retire when dropped. Optional `flux`, `vascular`, and `trail`
    /// sub-buffers allocate lazily (0 bytes VRAM when disabled).
    #[expect(
        clippy::too_many_arguments,
        reason = "lazy VRAM flags keep disabled V2 features at 0 bytes"
    )]
    fn ensure_buffers(
        &mut self,
        grid: FieldGrid,
        planes: usize,
        sources: usize,
        need_flux: bool,
        need_vascular: bool,
        need_trail: bool,
        need_trail_flux: bool,
    ) -> Result<(), CoreError> {
        let planes = planes.max(FIELD_PLANES as usize);
        let current = self.buffers.as_ref().is_some_and(|buffers| {
            buffers.grid == grid && buffers.planes == planes && buffers.sources == sources
        });
        let cells = usize::try_from(grid.cell_count()).map_err(|_| overflow())?;
        if !current {
            self.buffers = None;
            let values = cells.checked_mul(planes).ok_or_else(overflow)?;
            let axis = (grid.cells_x() as usize)
                .checked_add(grid.cells_y() as usize)
                .ok_or_else(overflow)?;
            let owner = &self.owner;
            self.buffers = Some(Buffers {
                grid,
                planes,
                sources,
                field: owner.zeroed_owned(values)?,
                ping: owner.zeroed_owned(values)?,
                diffused: owner.zeroed_owned(values)?,
                stencil: owner.zeroed_owned(STENCIL_WORDS)?,
                flux: None,
                vascular: None,
                trail: None,
                header: owner.zeroed_owned(HEADER_WORDS)?,
                descriptors: owner.zeroed_owned(DESCRIPTOR * sources.max(1))?,
                axes: owner.zeroed_owned(axis.checked_mul(sources.max(1)).ok_or_else(overflow)?)?,
                ranks: owner
                    .zeroed_owned(axis.checked_mul(sources.max(1)).ok_or_else(overflow)?)?,
                // Every cell of every source: a support may cover the grid.
                weights: owner.zeroed_owned(
                    cells
                        .checked_mul(sources.max(1))
                        .and_then(|entries| entries.checked_mul(2))
                        .ok_or_else(overflow)?,
                )?,
                moment_params: owner.zeroed_owned(MOMENT_WORDS)?,
                moments: owner.zeroed_owned(cells.checked_mul(CHANNELS).ok_or_else(overflow)?)?,
                fault: owner.zeroed_owned(1)?,
            });
        }
        let edge_words = cells.checked_mul(2).ok_or_else(overflow)?;
        if let Some(buffers) = self.buffers.as_mut() {
            if need_flux && buffers.flux.is_none() {
                buffers.flux = Some(FluxBuffers {
                    params: self.owner.zeroed_owned(FLUX_STENCIL_WORDS)?,
                    edge_x: self.owner.zeroed_owned(edge_words)?,
                    edge_y: self.owner.zeroed_owned(edge_words)?,
                });
            }
            if need_vascular && buffers.vascular.is_none() {
                buffers.vascular = Some(VascularBuffers {
                    params: self.owner.zeroed_owned(VASC_PARAMS_WORDS)?,
                    prev_x: self.owner.zeroed_owned(cells)?,
                    prev_y: self.owner.zeroed_owned(cells)?,
                    next_x: self.owner.zeroed_owned(cells)?,
                    next_y: self.owner.zeroed_owned(cells)?,
                });
            }
            if need_trail {
                if buffers.trail.is_none() {
                    let trail_values = cells
                        .checked_mul(CARRIER1_TOTAL_PLANES)
                        .ok_or_else(overflow)?;
                    buffers.trail = Some(TrailBuffers {
                        stencil: self.owner.zeroed_owned(STENCIL_WORDS)?,
                        decay_params: self.owner.zeroed_owned(TRAIL_DECAY_WORDS)?,
                        flux: None,
                        out: self.owner.zeroed_owned(trail_values)?,
                    });
                }
                if need_trail_flux
                    && let Some(trail) = buffers.trail.as_mut()
                    && trail.flux.is_none()
                {
                    trail.flux = Some(FluxBuffers {
                        params: self.owner.zeroed_owned(FLUX_STENCIL_WORDS)?,
                        edge_x: self.owner.zeroed_owned(edge_words)?,
                        edge_y: self.owner.zeroed_owned(edge_words)?,
                    });
                }
            }
        }
        Ok(())
    }

    /// Flux-form doubly-stochastic transport and decay over `plane_range` on
    /// Metal (`V2-09` tight junctions, reusable by `V2-14` vascular fields and
    /// `V2-15` dual-carrier splits), bit-identical to
    /// [`flife_field::transport_flux_range_on`].
    ///
    /// Planes outside `plane_range` in `next` are never modified.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for mismatched grids, buffers or an
    /// out-of-range `plane_range`; [`CoreError::NumericalInvalid`] for invalid
    /// edge conductances or non-finite field values; [`CoreError::Overflow`]
    /// if dimensions exceed `u32`.
    #[expect(
        clippy::too_many_lines,
        reason = "single command flux transport over plane_range plus host decay/report"
    )]
    pub fn transport_flux_range(
        &mut self,
        plan: &TransportPlan,
        edges: &FluxEdges,
        current: &FieldState,
        next: &mut FieldState,
        plane_range: Range<usize>,
        energy_plane: Option<usize>,
    ) -> Result<Option<TransportReport>, CoreError> {
        let grid = *plan.grid();
        if *current.grid() != grid || *next.grid() != grid || *edges.grid() != grid {
            return Err(CoreError::LayoutMismatch {
                buffer: "field",
                detail: "flux transport inputs do not share the transport plan's grid".into(),
            });
        }
        let cells = usize::try_from(grid.cell_count()).map_err(|_| overflow())?;
        let source_all = current.as_slice();
        let target_all = next.as_mut_slice();
        if cells == 0
            || source_all.len() != target_all.len()
            || !source_all.len().is_multiple_of(cells)
        {
            return Err(CoreError::LayoutMismatch {
                buffer: "field",
                detail: "a field plane is missing".into(),
            });
        }
        let total_planes = source_all.len() / cells;
        if plane_range.start > plane_range.end || plane_range.end > total_planes {
            return Err(CoreError::LayoutMismatch {
                buffer: "field",
                detail: format!("plane range {plane_range:?} is outside 0..{total_planes}"),
            });
        }
        validate_flux_edges(plan, edges, cells)?;
        let plane_count = plane_range.len();
        if plane_count == 0 {
            return Ok(None);
        }

        let word = |value: usize| u32::try_from(value).map_err(|_| overflow());
        let sources = self.buffers.as_ref().map_or(0, |b| b.sources);
        self.ensure_buffers(grid, total_planes, sources, true, false, false, false)?;
        let Self {
            owner,
            stencil_flux,
            buffers,
            host,
            ..
        } = self;
        let buffers = buffers.as_ref().ok_or_else(overflow)?;
        let flux = buffers.flux.as_ref().ok_or_else(overflow)?;

        pack_flux_edges(edges, &mut host.edge_x, &mut host.edge_y);
        let flux_params = [
            grid.cells_x(),
            grid.cells_y(),
            word(cells)?,
            word(plane_range.start)?,
            word(plane_count)?,
        ];
        owner.write(buffers.field.buffer())?.write(0, source_all)?;
        owner.write(flux.params.buffer())?.write(0, &flux_params)?;
        owner.write(flux.edge_x.buffer())?.write(0, &host.edge_x)?;
        owner.write(flux.edge_y.buffer())?.write(0, &host.edge_y)?;
        owner.write(buffers.fault.buffer())?.write(0, &[0])?;

        let substeps = plan.substeps().max(1) as usize;
        let (field, ping, diffused) = (
            buffers.field.buffer(),
            buffers.ping.buffer(),
            buffers.diffused.buffer(),
        );
        let target = |substep: usize| {
            if (substeps - substep).is_multiple_of(2) {
                diffused
            } else {
                ping
            }
        };
        let mut bindings = Vec::with_capacity(substeps);
        for substep in 1..=substeps {
            let source = if substep == 1 {
                field
            } else {
                target(substep - 1)
            };
            bindings.push([
                source.reads(),
                flux.params.buffer().reads(),
                flux.edge_x.buffer().reads(),
                flux.edge_y.buffer().reads(),
                target(substep).writes(),
                buffers.fault.buffer().writes(),
            ]);
        }
        let dispatch_items = cells.checked_mul(plane_count).ok_or_else(overflow)?;
        let kernels: Vec<Kernel<'_>> = bindings
            .iter()
            .map(|bindings| Kernel {
                pipeline: stencil_flux,
                bindings,
                grid: Grid::linear(dispatch_items.div_ceil(THREADS), THREADS),
            })
            .collect();
        owner.submit_batch(&kernels)?;
        check_fault(owner, &buffers.fault)?;

        let read_len = plane_range.end.checked_mul(cells).ok_or_else(overflow)?;
        host.readback.resize(read_len, 0.0);
        owner
            .read(diffused, Purpose::Readback)?
            .read(&mut host.readback)?;
        let start = plane_range.start * cells;
        target_all[start..read_len].copy_from_slice(&host.readback[start..read_len]);

        let factor = plan.decay_factor();
        let mut report = None;
        for plane in plane_range {
            let range = plane * cells..(plane + 1) * cells;
            if energy_plane == Some(plane) {
                let before = flife_field::transport::sum(&source_all[range.clone()]);
                report = Some(energy_report(plan, before, &mut target_all[range]));
            } else if factor != 1.0 {
                for value in &mut target_all[range] {
                    *value = (f64::from(*value) * factor) as f32;
                }
            }
        }
        Ok(report)
    }
}

fn validate_flux_edges(
    plan: &TransportPlan,
    edges: &FluxEdges,
    cells: usize,
) -> Result<(), CoreError> {
    if *edges.grid() != *plan.grid() {
        return Err(CoreError::LayoutMismatch {
            buffer: "flux edges",
            detail: "flux edges do not share the transport plan's grid".into(),
        });
    }
    if edges.edge_x().len() != cells || edges.edge_y().len() != cells {
        return Err(CoreError::LayoutMismatch {
            buffer: "flux edges",
            detail: format!(
                "flux edges ({}, {}) do not match {cells} grid cells",
                edges.edge_x().len(),
                edges.edge_y().len()
            ),
        });
    }
    let (r_x, r_y) = plan.ratios();
    for (axis, values, bound) in [
        ("flux edge_x", edges.edge_x(), r_x),
        ("flux edge_y", edges.edge_y(), r_y),
    ] {
        if let Some((cell, &conductance)) = values
            .iter()
            .enumerate()
            .find(|(_, v)| !v.is_finite() || **v < 0.0 || **v > bound)
        {
            return Err(CoreError::NumericalInvalid {
                quantity: axis,
                detail: format!(
                    "cell {cell} conductance {conductance:?} is outside [0, {bound:?}]"
                ),
            });
        }
    }
    Ok(())
}

fn pack_flux_edges(edges: &FluxEdges, edge_x: &mut Vec<u32>, edge_y: &mut Vec<u32>) {
    edge_x.clear();
    edge_x.reserve(edges.edge_x().len().saturating_mul(2));
    for &value in edges.edge_x() {
        edge_x.extend_from_slice(&words(value));
    }
    edge_y.clear();
    edge_y.reserve(edges.edge_y().len().saturating_mul(2));
    for &value in edges.edge_y() {
        edge_y.extend_from_slice(&words(value));
    }
}

/// A command's fault word, nonzero where a kernel met a non-finite operand.
fn check_fault(owner: &DeviceOwner, fault: &Owned<u32>) -> Result<(), CoreError> {
    let mut word = [0_u32];
    owner
        .read(fault.buffer(), Purpose::Readback)?
        .read(&mut word)?;
    if word[0] == 0 {
        return Ok(());
    }
    Err(CoreError::NumericalInvalid {
        quantity: "metal field operand",
        detail: "a field value the device read is not finite".to_owned(),
    })
}

impl FieldBackend for MetalField {
    fn describe(&self) -> String {
        format!("metal ({})", self.owner.device_name())
    }

    fn advance(
        &mut self,
        pair: &mut FieldPair,
        plan: &TransportPlan,
        kernel: &SourceKernel,
        sources: &[Source],
        step: TimeStep,
    ) -> Result<FieldStepReport, CoreError> {
        self.advance_wave2(pair, plan, kernel, sources, step, FieldStepPhase2::BASELINE)
    }

    fn advance_v2(
        &mut self,
        pair: &mut FieldPair,
        plan: &TransportPlan,
        kernel: &SourceKernel,
        sources: &[Source],
        step: TimeStep,
        options: FieldStepOptions<'_>,
    ) -> Result<FieldStepReport, CoreError> {
        self.advance_wave2(
            pair,
            plan,
            kernel,
            sources,
            step,
            FieldStepPhase2::from_options(options),
        )
    }

    #[expect(
        clippy::too_many_lines,
        reason = "Wave-2 Phase 2 step (V2-02/V2-09/V2-14/V2-15): two device commands and host sums in order"
    )]
    fn advance_wave2(
        &mut self,
        pair: &mut FieldPair,
        plan: &TransportPlan,
        kernel: &SourceKernel,
        sources: &[Source],
        step: TimeStep,
        phase2: FieldStepPhase2<'_>,
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
        let grid = *plan.grid();
        let cells = usize::try_from(grid.cell_count()).map_err(|_| overflow())?;
        let (is_dual_carrier, total_planes) = {
            let (current, next) = pair.stage();
            if current.is_dual_carrier() != next.is_dual_carrier()
                || current.planes() != next.planes()
            {
                return Err(CoreError::LayoutMismatch {
                    buffer: "field",
                    detail: "current and candidate field states disagree on carrier layout".into(),
                });
            }
            let planes = current.planes();
            if planes != FIELD_PLANES && planes != DUAL_CARRIER_PLANES {
                return Err(CoreError::LayoutMismatch {
                    buffer: "field",
                    detail: format!(
                        "unsupported plane count {planes} (expected {FIELD_PLANES} or {DUAL_CARRIER_PLANES})"
                    ),
                });
            }
            let layout = FieldLayout::planar_with_planes(grid, planes)?;
            layout.ensure_compatible(current.layout())?;
            layout.ensure_compatible(next.layout())?;
            if phase2.vascular.is_some() {
                let (Some((px, py)), Some((nx, ny))) =
                    (current.vascular_edges(), next.vascular_edges())
                else {
                    return Err(CoreError::LayoutMismatch {
                        buffer: "vascular edges",
                        detail:
                            "phase2.vascular requires vascular edge buffers enabled on FieldPair"
                                .into(),
                    });
                };
                if px.len() != cells || py.len() != cells || nx.len() != cells || ny.len() != cells
                {
                    return Err(CoreError::LayoutMismatch {
                        buffer: "vascular edges",
                        detail: format!("vascular edge buffers do not match {cells} grid cells"),
                    });
                }
            }
            (current.is_dual_carrier(), planes)
        };
        if let Some(edges) = phase2.carrier0_edges {
            validate_flux_edges(plan, edges, cells)?;
        }
        if phase2.vascular.is_some() && phase2.carrier0_edges.is_none() {
            return Err(CoreError::StateConflict {
                operation: "advance vascular field",
                detail: "phase2.vascular requires precomputed phase2.carrier0_edges".into(),
            });
        }
        let trail_plan = if is_dual_carrier {
            let Some(tp) = phase2.trail_plan else {
                return Err(CoreError::LayoutMismatch {
                    buffer: "dual-carrier trail transport plan",
                    detail: "34-plane dual-carrier advance requires phase2.trail_plan".into(),
                });
            };
            if *tp.grid() != grid {
                return Err(CoreError::LayoutMismatch {
                    buffer: "dual-carrier trail transport plan",
                    detail: "trail transport plan belongs to a different grid".into(),
                });
            }
            if let Some(edges) = phase2.trail_edges {
                validate_flux_edges(tp, edges, cells)?;
            }
            Some(tp)
        } else {
            None
        };

        let (width, height) = (grid.cells_x() as usize, grid.cells_y() as usize);
        let axis = width + height;
        let moment_channels = if is_dual_carrier {
            CARRIER0_MOMENT_CHANNELS as usize
        } else {
            CHANNELS
        };
        let c0_planes = 1 + moment_channels;
        let c0_values = cells.checked_mul(c0_planes).ok_or_else(overflow)?;
        let c1_values = cells
            .checked_mul(CARRIER1_TOTAL_PLANES)
            .ok_or_else(overflow)?;
        let count = sources.len();
        let word = |value: usize| u32::try_from(value).map_err(|_| overflow());
        self.ensure_buffers(
            grid,
            total_planes as usize,
            count,
            phase2.carrier0_edges.is_some(),
            phase2.vascular.is_some(),
            is_dual_carrier,
            is_dual_carrier && phase2.trail_edges.is_some(),
        )?;
        let Self {
            owner,
            stencil: stencil_pipeline,
            stencil_flux: stencil_flux_pipeline,
            weights: weights_pipeline,
            moments: moments_pipeline,
            trail_decay: trail_decay_pipeline,
            vascular_update: vascular_update_pipeline,
            buffers,
            host,
            ..
        } = self;
        let buffers = buffers.as_ref().ok_or_else(overflow)?;

        // The host's part before the device: each active source's candidates
        // and per-source kernel constants (zero-power dormant oases skip
        // cleanly with 0 candidates and NOT_CANDIDATE ranks).
        if host.supports.len() < count {
            host.supports.resize_with(count, SourceSupport::new);
        }
        host.descriptors.clear();
        host.descriptors.resize(DESCRIPTOR * count.max(1), 0);
        host.axes.clear();
        host.ranks.clear();
        host.ranks.resize(axis * count.max(1), NOT_CANDIDATE);
        let mut entries = 0_usize;
        for (index, (source, support)) in sources.iter().zip(&mut host.supports).enumerate() {
            let descriptor = &mut host.descriptors[index * DESCRIPTOR..][..DESCRIPTOR];
            descriptor[FIRST] = word(entries)?;
            if source.power() == 0.0 {
                *support = SourceSupport::new();
                continue;
            }
            let active_kernel = phase2
                .source_kernels
                .and_then(|kernels| kernels.get(index))
                .unwrap_or(kernel);
            active_kernel.candidates_into(&grid, source.center(), support)?;
            let (rows, columns) = (support.rows(), support.columns());
            descriptor[ROWS] = word(rows.len())?;
            descriptor[COLUMNS] = word(columns.len())?;
            descriptor[ROWS_AT] = word(host.axes.len())?;
            descriptor[COLUMNS_AT] = word(host.axes.len() + rows.len())?;
            put(descriptor, CENTER_X, source.center()[0]);
            put(descriptor, CENTER_Y, source.center()[1]);
            for (channel, &component) in source.direction().iter().enumerate() {
                put(descriptor, DIRECTION + 2 * channel, component);
            }
            let (radius_squared, two_sigma_squared) = active_kernel.weight_constants();
            put(descriptor, RADIUS_SQUARED, radius_squared);
            put(descriptor, TWO_SIGMA_SQUARED, two_sigma_squared);
            host.axes.extend_from_slice(rows);
            host.axes.extend_from_slice(columns);
            let ranks = &mut host.ranks[index * axis..][..axis];
            for (rank, &column) in columns.iter().enumerate() {
                ranks[column as usize] = word(rank)?;
            }
            for (rank, &row) in rows.iter().enumerate() {
                ranks[width + row as usize] = word(rank)?;
            }
            entries = rows
                .len()
                .checked_mul(columns.len())
                .and_then(|candidates| entries.checked_add(candidates))
                .ok_or_else(overflow)?;
        }

        // The parameters, and the committed field.
        let mut header = [0_u32; HEADER_WORDS];
        header[0] = word(count)?;
        header[1] = word(entries)?;
        for (at, value) in [
            (2, grid.world_width()),
            (4, grid.world_height()),
            (6, grid.cell_width()),
            (8, grid.cell_height()),
        ] {
            put(&mut header, at, value);
        }
        let mut moment_params = [0_u32; MOMENT_WORDS];
        moment_params[..5].copy_from_slice(&[
            word(cells)?,
            grid.cells_x(),
            grid.cells_y(),
            word(count)?,
            u32::from(plan.decay_factor() != 1.0),
        ]);
        put(&mut moment_params, 5, plan.decay_factor());
        moment_params[7] = u32::from(kernel.is_fused());
        moment_params[8] = word(moment_channels)?;
        owner
            .write(buffers.field.buffer())?
            .write(0, pair.current().as_slice())?;
        if let Some(edges) = phase2.carrier0_edges {
            let flux = buffers.flux.as_ref().ok_or_else(overflow)?;
            pack_flux_edges(edges, &mut host.edge_x, &mut host.edge_y);
            let flux_params = [
                grid.cells_x(),
                grid.cells_y(),
                word(cells)?,
                0,
                word(c0_planes)?,
            ];
            owner.write(flux.params.buffer())?.write(0, &flux_params)?;
            owner.write(flux.edge_x.buffer())?.write(0, &host.edge_x)?;
            owner.write(flux.edge_y.buffer())?.write(0, &host.edge_y)?;
        } else {
            let mut stencil = [0_u32; STENCIL_WORDS];
            stencil[..5].copy_from_slice(&[
                grid.cells_x(),
                grid.cells_y(),
                word(cells)?,
                0,
                word(c0_planes)?,
            ]);
            let (r_x, r_y) = plan.ratios();
            put(&mut stencil, 5, plan.center());
            put(&mut stencil, 7, r_x);
            put(&mut stencil, 9, r_y);
            owner.write(buffers.stencil.buffer())?.write(0, &stencil)?;
        }
        if let Some(tp) = trail_plan {
            let trail = buffers.trail.as_ref().ok_or_else(overflow)?;
            if let Some(edges) = phase2.trail_edges {
                let flux = trail.flux.as_ref().ok_or_else(overflow)?;
                pack_flux_edges(edges, &mut host.trail_edge_x, &mut host.trail_edge_y);
                let flux_params = [
                    grid.cells_x(),
                    grid.cells_y(),
                    word(cells)?,
                    CARRIER1_ENERGY_PLANE,
                    word(CARRIER1_TOTAL_PLANES)?,
                ];
                owner.write(flux.params.buffer())?.write(0, &flux_params)?;
                owner
                    .write(flux.edge_x.buffer())?
                    .write(0, &host.trail_edge_x)?;
                owner
                    .write(flux.edge_y.buffer())?
                    .write(0, &host.trail_edge_y)?;
            } else {
                let mut stencil = [0_u32; STENCIL_WORDS];
                stencil[..5].copy_from_slice(&[
                    grid.cells_x(),
                    grid.cells_y(),
                    word(cells)?,
                    CARRIER1_ENERGY_PLANE,
                    word(CARRIER1_TOTAL_PLANES)?,
                ]);
                let (r_x, r_y) = tp.ratios();
                put(&mut stencil, 5, tp.center());
                put(&mut stencil, 7, r_x);
                put(&mut stencil, 9, r_y);
                owner.write(trail.stencil.buffer())?.write(0, &stencil)?;
            }
            let mut decay_params = [0_u32; TRAIL_DECAY_WORDS];
            decay_params[0] = word(cells)?;
            decay_params[1] = u32::from(tp.decay_factor() != 1.0);
            put(&mut decay_params, 2, tp.decay_factor());
            owner
                .write(trail.decay_params.buffer())?
                .write(0, &decay_params)?;
        }
        if let Some(vascular) = phase2.vascular {
            let vasc = buffers.vascular.as_ref().ok_or_else(overflow)?;
            let (prev_x, prev_y) = pair.current().vascular_edges().ok_or_else(overflow)?;
            let dt = f64::from(step.numerator()) / f64::from(step.denominator());
            let mut vasc_params = [0_u32; VASC_PARAMS_WORDS];
            vasc_params[0] = grid.cells_x();
            vasc_params[1] = grid.cells_y();
            vasc_params[2] = word(cells)?;
            put(&mut vasc_params, 3, f64::from(plan.substeps()));
            put(&mut vasc_params, 5, dt);
            put(&mut vasc_params, 7, vascular.minimum_conductance_ratio());
            put(
                &mut vasc_params,
                9,
                vascular.reinforcement_rate_per_second(),
            );
            put(&mut vasc_params, 11, vascular.pruning_rate_per_second());
            put(&mut vasc_params, 13, vascular.flux_half_saturation());
            put(&mut vasc_params, 15, vascular.flux_exponent());
            owner.write(vasc.params.buffer())?.write(0, &vasc_params)?;
            owner.write(vasc.prev_x.buffer())?.write(0, prev_x)?;
            owner.write(vasc.prev_y.buffer())?.write(0, prev_y)?;
        }
        owner.write(buffers.header.buffer())?.write(0, &header)?;
        owner
            .write(buffers.descriptors.buffer())?
            .write(0, &host.descriptors)?;
        if !host.axes.is_empty() {
            owner.write(buffers.axes.buffer())?.write(0, &host.axes)?;
        }
        owner.write(buffers.ranks.buffer())?.write(0, &host.ranks)?;
        owner
            .write(buffers.moment_params.buffer())?
            .write(0, &moment_params)?;
        owner.write(buffers.fault.buffer())?.write(0, &[0])?;

        // Command one: Carrier-0 substeps (uniform or flux), optional Carrier-1
        // substeps + trail_decay, optional source_weights, and optional V2-14
        // vascular_update (reading pre-transport committed e_0).
        let (field, ping, diffused) = (
            buffers.field.buffer(),
            buffers.ping.buffer(),
            buffers.diffused.buffer(),
        );
        let substeps_0 = plan.substeps().max(1) as usize;
        let target_0 = |substep: usize| {
            if (substeps_0 - substep).is_multiple_of(2) {
                diffused
            } else {
                ping
            }
        };
        let mut c0_uniform_bindings: Vec<[Binding; 4]> = Vec::new();
        let mut c0_flux_bindings: Vec<[Binding; 6]> = Vec::new();
        if phase2.carrier0_edges.is_some() {
            let flux = buffers.flux.as_ref().ok_or_else(overflow)?;
            c0_flux_bindings.reserve(substeps_0);
            for substep in 1..=substeps_0 {
                let source = if substep == 1 {
                    field
                } else {
                    target_0(substep - 1)
                };
                c0_flux_bindings.push([
                    source.reads(),
                    flux.params.buffer().reads(),
                    flux.edge_x.buffer().reads(),
                    flux.edge_y.buffer().reads(),
                    target_0(substep).writes(),
                    buffers.fault.buffer().writes(),
                ]);
            }
        } else {
            c0_uniform_bindings.reserve(substeps_0);
            for substep in 1..=substeps_0 {
                let source = if substep == 1 {
                    field
                } else {
                    target_0(substep - 1)
                };
                c0_uniform_bindings.push([
                    source.reads(),
                    buffers.stencil.buffer().reads(),
                    target_0(substep).writes(),
                    buffers.fault.buffer().writes(),
                ]);
            }
        }

        let mut c1_uniform_bindings: Vec<[Binding; 4]> = Vec::new();
        let mut c1_flux_bindings: Vec<[Binding; 6]> = Vec::new();
        let mut trail_decay_bindings: Option<[Binding; 4]> = None;
        if let Some(tp) = trail_plan {
            let trail = buffers.trail.as_ref().ok_or_else(overflow)?;
            let substeps_1 = tp.substeps().max(1) as usize;
            let target_1 = |substep: usize| {
                if (substeps_1 - substep).is_multiple_of(2) {
                    diffused
                } else {
                    ping
                }
            };
            if phase2.trail_edges.is_some() {
                let flux = trail.flux.as_ref().ok_or_else(overflow)?;
                c1_flux_bindings.reserve(substeps_1);
                for substep in 1..=substeps_1 {
                    let source = if substep == 1 {
                        field
                    } else {
                        target_1(substep - 1)
                    };
                    c1_flux_bindings.push([
                        source.reads(),
                        flux.params.buffer().reads(),
                        flux.edge_x.buffer().reads(),
                        flux.edge_y.buffer().reads(),
                        target_1(substep).writes(),
                        buffers.fault.buffer().writes(),
                    ]);
                }
            } else {
                c1_uniform_bindings.reserve(substeps_1);
                for substep in 1..=substeps_1 {
                    let source = if substep == 1 {
                        field
                    } else {
                        target_1(substep - 1)
                    };
                    c1_uniform_bindings.push([
                        source.reads(),
                        trail.stencil.buffer().reads(),
                        target_1(substep).writes(),
                        buffers.fault.buffer().writes(),
                    ]);
                }
            }
            trail_decay_bindings = Some([
                diffused.reads(),
                trail.decay_params.buffer().reads(),
                trail.out.buffer().writes(),
                buffers.fault.buffer().writes(),
            ]);
        }

        let weight_bindings = [
            buffers.header.buffer().reads(),
            buffers.descriptors.buffer().reads(),
            buffers.axes.buffer().reads(),
            buffers.weights.buffer().writes(),
        ];
        let vascular_bindings: Option<[Binding; 9]> = if phase2.vascular.is_some() {
            let flux = buffers.flux.as_ref().ok_or_else(overflow)?;
            let vasc = buffers.vascular.as_ref().ok_or_else(overflow)?;
            Some([
                field.reads(),
                vasc.params.buffer().reads(),
                flux.edge_x.buffer().reads(),
                flux.edge_y.buffer().reads(),
                vasc.prev_x.buffer().reads(),
                vasc.prev_y.buffer().reads(),
                vasc.next_x.buffer().writes(),
                vasc.next_y.buffer().writes(),
                buffers.fault.buffer().writes(),
            ])
        } else {
            None
        };

        let mut kernels: Vec<Kernel<'_>> = if phase2.carrier0_edges.is_some() {
            c0_flux_bindings
                .iter()
                .map(|bindings| Kernel {
                    pipeline: stencil_flux_pipeline,
                    bindings,
                    grid: Grid::linear(c0_values.div_ceil(THREADS), THREADS),
                })
                .collect()
        } else {
            c0_uniform_bindings
                .iter()
                .map(|bindings| Kernel {
                    pipeline: stencil_pipeline,
                    bindings,
                    grid: Grid::linear(c0_values.div_ceil(THREADS), THREADS),
                })
                .collect()
        };
        if trail_plan.is_some() {
            if phase2.trail_edges.is_some() {
                for bindings in &c1_flux_bindings {
                    kernels.push(Kernel {
                        pipeline: stencil_flux_pipeline,
                        bindings,
                        grid: Grid::linear(c1_values.div_ceil(THREADS), THREADS),
                    });
                }
            } else {
                for bindings in &c1_uniform_bindings {
                    kernels.push(Kernel {
                        pipeline: stencil_pipeline,
                        bindings,
                        grid: Grid::linear(c1_values.div_ceil(THREADS), THREADS),
                    });
                }
            }
            if let Some(bindings) = &trail_decay_bindings {
                kernels.push(Kernel {
                    pipeline: trail_decay_pipeline,
                    bindings,
                    grid: Grid::linear(c1_values.div_ceil(THREADS), THREADS),
                });
            }
        }
        if entries > 0 {
            kernels.push(Kernel {
                pipeline: weights_pipeline,
                bindings: &weight_bindings,
                grid: Grid::linear(entries.div_ceil(64), 64),
            });
        }
        if let Some(bindings) = &vascular_bindings {
            kernels.push(Kernel {
                pipeline: vascular_update_pipeline,
                bindings,
                grid: Grid::linear(cells.div_ceil(THREADS), THREADS),
            });
        }
        owner.submit_batch(&kernels)?;
        check_fault(owner, &buffers.fault)?;

        // The host's part: Carrier-0 energy report, optional Carrier-1 energy
        // report + trail copy, optional vascular candidate readback, then each
        // active source's support, sum, and Carrier-0 energy deposits.
        host.energy.resize(cells, 0.0);
        owner
            .read(diffused, Purpose::Readback)?
            .read(&mut host.energy)?;
        if trail_plan.is_some() {
            let trail = buffers.trail.as_ref().ok_or_else(overflow)?;
            host.trail_out.resize(c1_values, 0.0);
            owner
                .read(trail.out.buffer(), Purpose::Readback)?
                .read(&mut host.trail_out)?;
        }
        host.weights.resize(2 * entries, 0);
        if entries > 0 {
            owner
                .read(buffers.weights.buffer(), Purpose::Readback)?
                .read(&mut host.weights)?;
        }
        let (current, next) = pair.stage();
        let r0 = energy_report(
            plan,
            flife_field::transport::sum(current.energy()),
            &mut host.energy,
        );
        next.energy_mut().copy_from_slice(&host.energy);
        let transport = if let Some(tp) = trail_plan {
            let c1_before =
                flife_field::transport::sum(current.trail_energy().ok_or_else(overflow)?);
            let r1 = energy_report(tp, c1_before, &mut host.trail_out[..cells]);
            let c1_start = (CARRIER1_ENERGY_PLANE as usize) * cells;
            next.as_mut_slice()[c1_start..c1_start + c1_values].copy_from_slice(&host.trail_out);
            combine_transport_reports(r0, r1)
        } else {
            r0
        };
        if phase2.vascular.is_some() {
            let vasc = buffers.vascular.as_ref().ok_or_else(overflow)?;
            let (next_x, next_y) = next.vascular_edges_mut().ok_or_else(overflow)?;
            owner
                .read(vasc.next_x.buffer(), Purpose::Readback)?
                .read(next_x)?;
            owner
                .read(vasc.next_y.buffer(), Purpose::Readback)?
                .read(next_y)?;
        }

        let deposits = if kernel.is_fused() {
            for (index, (source, support)) in sources.iter().zip(&mut host.supports).enumerate() {
                if source.power() == 0.0 {
                    continue;
                }
                let active_kernel = phase2
                    .source_kernels
                    .and_then(|kernels| kernels.get(index))
                    .unwrap_or(kernel);
                let descriptor = &host.descriptors[index * DESCRIPTOR..][..DESCRIPTOR];
                let first = descriptor[FIRST] as usize;
                let candidates = descriptor[ROWS] as usize * descriptor[COLUMNS] as usize;
                let mut weights = host.weights[2 * first..2 * (first + candidates)]
                    .as_chunks::<2>()
                    .0
                    .iter();
                active_kernel.weigh_into(&grid, source.center(), support, |_, _| {
                    let pair = weights.next().ok_or_else(mismatch)?;
                    match u64::from(pair[0]) | (u64::from(pair[1]) << 32) {
                        ABSENT => Ok(None),
                        REFUSED => Err(CoreError::NumericalInvalid {
                            quantity: "world displacement",
                            detail: "the device refused a source displacement".to_owned(),
                        }),
                        bits => Ok(Some(f64::from_bits(bits))),
                    }
                })?;
            }
            let deposits = deposit_energy_fused(
                next,
                sources,
                step,
                &mut host.supports[..count],
                &mut host.fused_accum,
            )?;
            for (index, (source, support)) in sources.iter().zip(&host.supports).enumerate() {
                if source.power() == 0.0 {
                    continue;
                }
                let descriptor = &host.descriptors[index * DESCRIPTOR..][..DESCRIPTOR];
                let first = descriptor[FIRST] as usize;
                let candidates = descriptor[ROWS] as usize * descriptor[COLUMNS] as usize;
                let mut amounts = support.amounts().iter();
                for pair in host.weights[2 * first..2 * (first + candidates)]
                    .as_chunks_mut::<2>()
                    .0
                {
                    if u64::from(pair[0]) | (u64::from(pair[1]) << 32) == ABSENT {
                        continue;
                    }
                    let amount = *amounts.next().ok_or_else(mismatch)?;
                    if !amount.is_finite() {
                        return Err(CoreError::NumericalInvalid {
                            quantity: "source amount",
                            detail: "a source deposits a non-finite amount".to_owned(),
                        });
                    }
                    *pair = words(amount);
                }
                if amounts.next().is_some() {
                    return Err(mismatch());
                }
            }
            deposits
        } else {
            let mut deposits = Vec::with_capacity(count);
            for (index, (source, support)) in sources.iter().zip(&mut host.supports).enumerate() {
                if source.power() == 0.0 {
                    deposits.push(deposit_energy(next, source, step, support)?);
                    continue;
                }
                let active_kernel = phase2
                    .source_kernels
                    .and_then(|kernels| kernels.get(index))
                    .unwrap_or(kernel);
                let descriptor = &host.descriptors[index * DESCRIPTOR..][..DESCRIPTOR];
                let first = descriptor[FIRST] as usize;
                let candidates = descriptor[ROWS] as usize * descriptor[COLUMNS] as usize;
                let mut weights = host.weights[2 * first..2 * (first + candidates)]
                    .as_chunks::<2>()
                    .0
                    .iter();
                active_kernel.weigh_into(&grid, source.center(), support, |_, _| {
                    let pair = weights.next().ok_or_else(mismatch)?;
                    match u64::from(pair[0]) | (u64::from(pair[1]) << 32) {
                        ABSENT => Ok(None),
                        REFUSED => Err(CoreError::NumericalInvalid {
                            quantity: "world displacement",
                            detail: "the device refused a source displacement".to_owned(),
                        }),
                        bits => Ok(Some(f64::from_bits(bits))),
                    }
                })?;
                deposits.push(deposit_energy(next, source, step, support)?);
                // Each present candidate's weight gives way to its amount, the
                // value the reference deposits on the moment planes.
                let mut amounts = support.amounts().iter();
                for pair in host.weights[2 * first..2 * (first + candidates)]
                    .as_chunks_mut::<2>()
                    .0
                {
                    if u64::from(pair[0]) | (u64::from(pair[1]) << 32) == ABSENT {
                        continue;
                    }
                    let amount = *amounts.next().ok_or_else(mismatch)?;
                    // A non-finite amount could alias a marker; the reference
                    // takes such a step.
                    if !amount.is_finite() {
                        return Err(CoreError::NumericalInvalid {
                            quantity: "source amount",
                            detail: "a source deposits a non-finite amount".to_owned(),
                        });
                    }
                    *pair = words(amount);
                }
                if amounts.next().is_some() {
                    return Err(mismatch());
                }
            }
            deposits
        };

        // Command two: Carrier-0 moments (`32` single-carrier or `24` dual-carrier),
        // decayed and deposited.
        if entries > 0 {
            owner
                .write(buffers.weights.buffer())?
                .write(0, &host.weights)?;
        }
        owner.submit_batch(&[Kernel {
            pipeline: moments_pipeline,
            bindings: &[
                diffused.reads(),
                buffers.moment_params.buffer().reads(),
                buffers.descriptors.buffer().reads(),
                buffers.ranks.buffer().reads(),
                buffers.weights.buffer().reads(),
                buffers.moments.buffer().writes(),
                buffers.fault.buffer().writes(),
            ],
            grid: Grid::linear((cells * moment_channels).div_ceil(THREADS), THREADS),
        }])?;
        check_fault(owner, &buffers.fault)?;
        owner
            .read(buffers.moments.buffer(), Purpose::Readback)?
            .read(next.moments_mut())?;
        Ok(step_report(transport, deposits, next))
    }

    fn reduce(
        &mut self,
        field: &mut FieldState,
        grants: &mut Grants,
        deposits: [&mut Deposits; 2],
    ) -> Result<[DepositSummary; 2], CoreError> {
        self.reduce.run(&self.owner, field, grants, deposits)
    }

    fn reduce_dual_carrier(
        &mut self,
        field: &mut FieldState,
        grants: &mut Grants,
        carrier0_deposits: [&mut Deposits; 2],
        trail_deposits: &mut Deposits,
    ) -> Result<([DepositSummary; 2], DepositSummary), CoreError> {
        self.reduce.run_dual_carrier(
            &self.owner,
            field,
            grants,
            carrier0_deposits,
            trail_deposits,
        )
    }

    fn check(
        &mut self,
        field: &FieldState,
        tolerance: ConeTolerance,
    ) -> Result<FieldScan, CoreError> {
        self.check.run(&self.owner, field, tolerance)
    }
}
