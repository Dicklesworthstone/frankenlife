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
use flife_field::grid::FIELD_PLANES;
use flife_field::{
    ConeTolerance, DepositSummary, Deposits, FieldBackend, FieldGrid, FieldLayout, FieldPair,
    FieldScan, FieldState, FieldStepOptions, FieldStepReport, FluxEdges, Grants, Source,
    SourceKernel, SourceSupport, TransportPlan, TransportReport, deposit_energy,
    deposit_energy_fused, energy_report, step_report,
};
use ft_kernel_metal::compute::{Grid, MathMode, Pipeline};

use crate::check::Check;
use crate::owner::{Binding, DeviceOwner, Kernel, Limits, Owned, Purpose, metal};
use crate::reduce::Reduce;

/// The field kernel source, with the software binary64 ahead of it.
pub const FIELD_MSL: &str = concat!(
    include_str!("softfloat64.metal"),
    include_str!("field.metal")
);

/// Moment channels.
const CHANNELS: usize = 32;

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

/// Words of the stencil, flux-stencil, weights-header and moments parameters.
const STENCIL_WORDS: usize = 10;
const FLUX_STENCIL_WORDS: usize = 5;
const HEADER_WORDS: usize = 10;
const MOMENT_WORDS: usize = 8;

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
    /// old ones retire when dropped.
    fn ensure_buffers(
        &mut self,
        grid: FieldGrid,
        planes: usize,
        sources: usize,
        need_flux: bool,
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
        if need_flux
            && let Some(buffers) = self.buffers.as_mut()
            && buffers.flux.is_none()
        {
            let edge_words = cells.checked_mul(2).ok_or_else(overflow)?;
            buffers.flux = Some(FluxBuffers {
                params: self.owner.zeroed_owned(FLUX_STENCIL_WORDS)?,
                edge_x: self.owner.zeroed_owned(edge_words)?,
                edge_y: self.owner.zeroed_owned(edge_words)?,
            });
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
        self.ensure_buffers(grid, total_planes, sources, true)?;
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
        self.advance_v2(
            pair,
            plan,
            kernel,
            sources,
            step,
            FieldStepOptions::BASELINE,
        )
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one step's two commands and the host's part between them, in order"
    )]
    fn advance_v2(
        &mut self,
        pair: &mut FieldPair,
        plan: &TransportPlan,
        kernel: &SourceKernel,
        sources: &[Source],
        step: TimeStep,
        options: FieldStepOptions<'_>,
    ) -> Result<FieldStepReport, CoreError> {
        if let Some(kernels) = options.source_kernels {
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
        let layout = FieldLayout::planar(grid)?;
        {
            let (current, next) = pair.stage();
            layout.ensure_compatible(current.layout())?;
            layout.ensure_compatible(next.layout())?;
        }
        let cells = usize::try_from(grid.cell_count()).map_err(|_| overflow())?;
        if let Some(edges) = options.flux_edges {
            validate_flux_edges(plan, edges, cells)?;
        }
        let (width, height) = (grid.cells_x() as usize, grid.cells_y() as usize);
        let axis = width + height;
        let values = cells * FIELD_PLANES as usize;
        let count = sources.len();
        let word = |value: usize| u32::try_from(value).map_err(|_| overflow());
        self.ensure_buffers(
            grid,
            FIELD_PLANES as usize,
            count,
            options.flux_edges.is_some(),
        )?;
        let Self {
            owner,
            stencil: stencil_pipeline,
            stencil_flux: stencil_flux_pipeline,
            weights: weights_pipeline,
            moments: moments_pipeline,
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
            let active_kernel = options
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
        owner
            .write(buffers.field.buffer())?
            .write(0, pair.current().as_slice())?;
        if let Some(edges) = options.flux_edges {
            let flux = buffers.flux.as_ref().ok_or_else(overflow)?;
            pack_flux_edges(edges, &mut host.edge_x, &mut host.edge_y);
            let flux_params = [
                grid.cells_x(),
                grid.cells_y(),
                word(cells)?,
                0,
                FIELD_PLANES,
            ];
            owner.write(flux.params.buffer())?.write(0, &flux_params)?;
            owner.write(flux.edge_x.buffer())?.write(0, &host.edge_x)?;
            owner.write(flux.edge_y.buffer())?.write(0, &host.edge_y)?;
        } else {
            let mut stencil = [0_u32; STENCIL_WORDS];
            stencil[..4].copy_from_slice(&[
                grid.cells_x(),
                grid.cells_y(),
                word(cells)?,
                FIELD_PLANES,
            ]);
            let (r_x, r_y) = plan.ratios();
            put(&mut stencil, 4, plan.center());
            put(&mut stencil, 6, r_x);
            put(&mut stencil, 8, r_y);
            owner.write(buffers.stencil.buffer())?.write(0, &stencil)?;
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

        // Command one: the substeps (uniform or V2-09 flux-form), ending in
        // `diffused`, then the candidates' weights.
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
        let mut uniform_bindings: Vec<[Binding; 4]> = Vec::new();
        let mut flux_bindings: Vec<[Binding; 6]> = Vec::new();
        if options.flux_edges.is_some() {
            let flux = buffers.flux.as_ref().ok_or_else(overflow)?;
            flux_bindings.reserve(substeps);
            for substep in 1..=substeps {
                let source = if substep == 1 {
                    field
                } else {
                    target(substep - 1)
                };
                flux_bindings.push([
                    source.reads(),
                    flux.params.buffer().reads(),
                    flux.edge_x.buffer().reads(),
                    flux.edge_y.buffer().reads(),
                    target(substep).writes(),
                    buffers.fault.buffer().writes(),
                ]);
            }
        } else {
            uniform_bindings.reserve(substeps);
            for substep in 1..=substeps {
                let source = if substep == 1 {
                    field
                } else {
                    target(substep - 1)
                };
                uniform_bindings.push([
                    source.reads(),
                    buffers.stencil.buffer().reads(),
                    target(substep).writes(),
                    buffers.fault.buffer().writes(),
                ]);
            }
        }
        let weight_bindings = [
            buffers.header.buffer().reads(),
            buffers.descriptors.buffer().reads(),
            buffers.axes.buffer().reads(),
            buffers.weights.buffer().writes(),
        ];
        let mut kernels: Vec<Kernel<'_>> = if options.flux_edges.is_some() {
            flux_bindings
                .iter()
                .map(|bindings| Kernel {
                    pipeline: stencil_flux_pipeline,
                    bindings,
                    grid: Grid::linear(values.div_ceil(THREADS), THREADS),
                })
                .collect()
        } else {
            uniform_bindings
                .iter()
                .map(|bindings| Kernel {
                    pipeline: stencil_pipeline,
                    bindings,
                    grid: Grid::linear(values.div_ceil(THREADS), THREADS),
                })
                .collect()
        };
        if entries > 0 {
            kernels.push(Kernel {
                pipeline: weights_pipeline,
                bindings: &weight_bindings,
                grid: Grid::linear(entries.div_ceil(64), 64),
            });
        }
        owner.submit_batch(&kernels)?;
        check_fault(owner, &buffers.fault)?;

        // The host's part: the energy plane, then each active support, its sum
        // and its energy deposits.
        host.energy.resize(cells, 0.0);
        owner
            .read(diffused, Purpose::Readback)?
            .read(&mut host.energy)?;
        host.weights.resize(2 * entries, 0);
        if entries > 0 {
            owner
                .read(buffers.weights.buffer(), Purpose::Readback)?
                .read(&mut host.weights)?;
        }
        let (current, next) = pair.stage();
        let transport = energy_report(
            plan,
            flife_field::transport::sum(current.energy()),
            &mut host.energy,
        );
        next.energy_mut().copy_from_slice(&host.energy);
        let deposits = if kernel.is_fused() {
            for (index, (source, support)) in sources.iter().zip(&mut host.supports).enumerate() {
                if source.power() == 0.0 {
                    continue;
                }
                let active_kernel = options
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
                let active_kernel = options
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

        // Command two: the moments, decayed and deposited.
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
            grid: Grid::linear((cells * CHANNELS).div_ceil(THREADS), THREADS),
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

    fn check(
        &mut self,
        field: &FieldState,
        tolerance: ConeTolerance,
    ) -> Result<FieldScan, CoreError> {
        self.check.run(&self.owner, field, tolerance)
    }
}
