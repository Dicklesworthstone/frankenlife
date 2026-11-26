//! External sources: the normalized truncated minimum-image Gaussian kernel,
//! fixed-total-power deposits, fixed chemical directions and the smooth
//! torus path (plan §4.5, A.3 items 7–9, Appendix C.17).
//!
//! A source `j` with total power `P_j` deposits exactly `P_j Δt` per tick in
//! F64. Raw weights `k_c = exp(−|d_c|²/(2σ²))` are taken at cell centers
//! `((i + ½) h_x, (j + ½) h_y)` within the cutoff radius `cσ`, where `d_c`
//! is the minimum-image displacement. Cell `c` receives
//! `q_c = P_j Δt · k_c / Σ k`, which adds `(q_c, q_c a_j)` to `(e_c, m_c)`.
//! Every sum runs in cell-index order. Changing the grid resolution changes
//! the support, never the total. An empty support is refused.
//!
//! Storing into the F32 field rounds each cell once. The deposit reports the
//! declared total, its F64 normalized total and the energy actually applied,
//! so the ledger can record the residual instead of hiding it.
//!
//! Random draws are not made here. Seeded centers, chemical directions and
//! path directions come from `fnp-philox-semantic-v1` in
//! [`crate::seeding`] (FL-010). This module defines the deterministic
//! arithmetic those draws feed.

use flife_core::ids::SourceId;
use flife_core::memory::{bytes_for, capacity_bytes, total};
use flife_core::numerics::{normalize_turns, sin_cos_turns};
use flife_core::rng::SemanticRng;
use flife_core::{CoreError, Tick, TickSpan, TimeStep, V2Feature, ValidatedConfig};

use crate::grid::{FieldGrid, minimum_image, wrap_coordinate};
use crate::layout::overflow;
use crate::numerics::{exp_neg, exp_neg_4};
use crate::seeding::source_sigma;
use crate::state::{CHANNELS, FieldState};

/// The spatial normalization this module implements.
pub const FIXED_TOTAL_POWER: &str = "fixed-total-power";

/// The source kernel this module implements (sequential per-source F32 rounding).
pub const TRUNCATED_GAUSSIAN_V1: &str = "normalized-truncated-minimum-image-gaussian-v1";

/// The fused source kernel (`flife-ze4`): identical normalized Gaussian weights
/// to [`TRUNCATED_GAUSSIAN_V1`], but all simultaneous sources are accumulated
/// per cell in F64 and rounded once to F32 per tick, eliminating M-fold
/// coherent F32 rounding bias in high-source energy-rich worlds.
pub const TRUNCATED_GAUSSIAN_FUSED_V1: &str =
    "normalized-truncated-minimum-image-gaussian-fused-v1";

/// The chemical-direction policy this module implements: a source's
/// direction never changes.
pub const FIXED_PER_SOURCE: &str = "fixed-per-source";

/// A normalized truncated minimum-image Gaussian of width `σ`, cut off at
/// `cutoff_sigma × σ`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceKernel {
    sigma: f64,
    cutoff_sigma: f64,
    radius: f64,
    fused: bool,
}

impl SourceKernel {
    /// A kernel of width `sigma` (world-length) truncated at
    /// `cutoff_sigma × sigma` using sequential per-source F32 rounding
    /// ([`TRUNCATED_GAUSSIAN_V1`]).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless both are finite and positive.
    pub fn new(sigma: f64, cutoff_sigma: f64) -> Result<Self, CoreError> {
        Self::with_mode(sigma, cutoff_sigma, false)
    }

    /// A fused kernel of width `sigma` truncated at `cutoff_sigma × sigma`
    /// ([`TRUNCATED_GAUSSIAN_FUSED_V1`]): accumulates simultaneous sources per
    /// cell in F64 and rounds once to F32 per tick.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless both are finite and positive.
    pub fn new_fused(sigma: f64, cutoff_sigma: f64) -> Result<Self, CoreError> {
        Self::with_mode(sigma, cutoff_sigma, true)
    }

    fn with_mode(sigma: f64, cutoff_sigma: f64, fused: bool) -> Result<Self, CoreError> {
        let radius = sigma * cutoff_sigma;
        for (quantity, value) in [
            ("sources.sigma_world_units", sigma),
            ("sources.kernel_cutoff_sigma", cutoff_sigma),
            ("source support radius", radius),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(CoreError::NumericalInvalid {
                    quantity,
                    detail: format!("{value:?} is not finite and positive"),
                });
            }
        }
        Ok(Self {
            sigma,
            cutoff_sigma,
            radius,
            fused,
        })
    }

    /// The same kernel family and cutoff factor with a different width `sigma`
    /// (`V2-02` heterogeneous oasis radii).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless `sigma` and `sigma * cutoff_sigma`
    /// are finite and positive.
    pub fn with_sigma(&self, sigma: f64) -> Result<Self, CoreError> {
        Self::with_mode(sigma, self.cutoff_sigma, self.fused)
    }

    /// The cutoff factor `cutoff_sigma` in units of `σ`.
    #[must_use]
    pub const fn cutoff_sigma(&self) -> f64 {
        self.cutoff_sigma
    }

    /// Whether simultaneous sources fuse in F64 per cell before one F32 store
    /// (`flife-ze4`).
    #[must_use]
    pub const fn is_fused(&self) -> bool {
        self.fused
    }

    /// The kernel a validated configuration declares.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] for another kernel, normalization or
    /// chemical-direction policy, and the errors of [`SourceKernel::new`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let sources = &config.config().sources;
        let fused = match sources.kernel.as_str() {
            TRUNCATED_GAUSSIAN_V1 => false,
            TRUNCATED_GAUSSIAN_FUSED_V1 => true,
            other => {
                return Err(CoreError::MissingCapability {
                    capability: format!("source kernel `{other}`"),
                });
            }
        };
        for (role, name, implemented) in [
            (
                "source normalization",
                &sources.spatial_normalization,
                FIXED_TOTAL_POWER,
            ),
            (
                "source chemical-direction policy",
                &sources.chemical_direction_policy,
                FIXED_PER_SOURCE,
            ),
        ] {
            if name != implemented {
                return Err(CoreError::MissingCapability {
                    capability: format!("{role} `{name}`"),
                });
            }
        }
        Self::with_mode(
            sources.sigma_world_units,
            sources.kernel_cutoff_sigma,
            fused,
        )
    }

    /// `σ` in world-length.
    #[must_use]
    pub const fn sigma(&self) -> f64 {
        self.sigma
    }

    /// The support radius `cutoff_sigma × σ` in world-length.
    #[must_use]
    pub const fn radius(&self) -> f64 {
        self.radius
    }

    /// Fill `support` with the cells whose centers lie within the radius of
    /// `center` on the torus, with their raw weights, in cell-index order.
    /// Each cell appears once, even when the support wraps around the
    /// whole grid. This is [`SourceKernel::candidates_into`], then
    /// [`SourceKernel::weigh_into`] with [`SourceKernel::weight`].
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] when `center` is not a wrapped
    /// position or the support holds no cell (A.3 item 9).
    pub fn support_into(
        &self,
        grid: &FieldGrid,
        center: [f64; 2],
        support: &mut SourceSupport,
    ) -> Result<(), CoreError> {
        self.candidates_into(grid, center, support)?;
        let SourceSupport {
            cell_indices,
            cell_weights,
            max_cell,
            weight_sum,
            columns,
            rows,
            ..
        } = support;
        cell_indices.clear();
        cell_weights.clear();
        *max_cell = 0;
        *weight_sum = 0.0;
        if let (Some(&first_col), Some(&first_row)) = (columns.first(), rows.first()) {
            let (radius_squared, two_sigma_squared) = self.weight_constants();
            let cell_w = grid.cell_width();
            let cell_h = grid.cell_height();
            let lx = grid.world_width();
            let ly = grid.world_height();

            // Validate `center[0]` then `center[1]` in the exact order of
            // `FieldGrid::displacement(center, cell_center)`.
            let dx0 = minimum_image(center[0], (f64::from(first_col) + 0.5) * cell_w, lx)?;
            let _ = minimum_image(center[1], (f64::from(first_row) + 0.5) * cell_h, ly)?;

            // Hoist per-column `dx²` once so 2D support evaluation pays O(W + H)
            // minimum-image calls instead of O(W × H), with zero heap traffic for
            // grids up to 1024 candidate columns wide and fallible OOM-safe growth
            // beyond that.
            const STACK_COLS: usize = 1024;
            let mut dx_sq_stack = [0.0_f64; STACK_COLS];
            let mut dx_sq_heap = Vec::new();
            let dx_sq: &mut [f64] = if columns.len() <= STACK_COLS {
                &mut dx_sq_stack[..columns.len()]
            } else {
                dx_sq_heap.try_reserve_exact(columns.len()).map_err(|_| {
                    CoreError::AllocationFailed {
                        buffer: "source support column distances",
                        bytes: bytes_for::<f64>(columns.len()),
                    }
                })?;
                dx_sq_heap.resize(columns.len(), 0.0);
                &mut dx_sq_heap
            };
            dx_sq[0] = dx0 * dx0;
            for (slot, &column) in dx_sq[1..].iter_mut().zip(&columns[1..]) {
                let dx = minimum_image(center[0], (f64::from(column) + 0.5) * cell_w, lx)?;
                *slot = dx * dx;
            }

            let max_col = *columns.last().unwrap_or(&0) as usize;
            let mut batch_idx = [0_usize; 4];
            let mut batch_dist_sq = [0.0_f64; 4];
            let mut batch_len = 0_usize;

            for &row in rows.iter() {
                let dy = minimum_image(center[1], (f64::from(row) + 0.5) * cell_h, ly)?;
                let dy_sq = dy * dy;
                if dy_sq > radius_squared {
                    continue;
                }
                let row_base = grid
                    .cell_index(0, row)
                    .filter(|base| base.checked_add(max_col).is_some());
                for (&column, &dx2) in columns.iter().zip(dx_sq.iter()) {
                    let distance_squared = dx2 + dy_sq;
                    if distance_squared > radius_squared {
                        continue;
                    }
                    let index = match row_base {
                        Some(base) => base + column as usize,
                        None => grid.cell_index(column, row).ok_or_else(overflow)?,
                    };
                    batch_idx[batch_len] = index;
                    batch_dist_sq[batch_len] = distance_squared;
                    batch_len += 1;
                    if batch_len == 4 {
                        let args = [
                            batch_dist_sq[0] / two_sigma_squared,
                            batch_dist_sq[1] / two_sigma_squared,
                            batch_dist_sq[2] / two_sigma_squared,
                            batch_dist_sq[3] / two_sigma_squared,
                        ];
                        let weights = if let Some(weights) = exp_neg_4(args) {
                            weights
                        } else {
                            let mut fallback = [0.0_f64; 4];
                            for lane in 0..4 {
                                fallback[lane] = exp_neg(args[lane]).ok_or_else(|| {
                                    CoreError::NumericalInvalid {
                                        quantity: "source weight",
                                        detail: format!(
                                            "|d|² = {:?} has no weight",
                                            batch_dist_sq[lane]
                                        ),
                                    }
                                })?;
                            }
                            fallback
                        };
                        for lane in 0..4 {
                            let idx = batch_idx[lane];
                            let weight = weights[lane];
                            cell_indices.push(idx);
                            cell_weights.push(weight);
                            if idx > *max_cell {
                                *max_cell = idx;
                            }
                            *weight_sum += weight;
                        }
                        batch_len = 0;
                    }
                }
            }
            for lane in 0..batch_len {
                let distance_squared = batch_dist_sq[lane];
                let weight = exp_neg(distance_squared / two_sigma_squared).ok_or_else(|| {
                    CoreError::NumericalInvalid {
                        quantity: "source weight",
                        detail: format!("|d|² = {distance_squared:?} has no weight"),
                    }
                })?;
                let idx = batch_idx[lane];
                cell_indices.push(idx);
                cell_weights.push(weight);
                if idx > *max_cell {
                    *max_cell = idx;
                }
                *weight_sum += weight;
            }
        }
        if cell_indices.is_empty() || *weight_sum <= 0.0 || !weight_sum.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "source support",
                detail: format!(
                    "no cell center lies within {:?} of {center:?} with positive weight",
                    self.radius
                ),
            });
        }
        rebuild_runs(cell_indices, &mut support.runs);
        Ok(())
    }

    /// The most candidate columns and rows one support on `grid` takes
    /// (FL-043): the cells whose centers lie within the radius of a center,
    /// widened by one on each side against rounding, which is at most
    /// `2r/h + 5` of an axis's cells, and at most the whole axis.
    #[must_use]
    pub fn candidate_bound(&self, grid: &FieldGrid) -> [usize; 2] {
        let axis = |cell: f64, cells: u32| {
            let span = (2.0 * self.radius / cell).ceil() + 5.0;
            let whole = cells as usize;
            if span.is_finite() && span < f64::from(cells) {
                // Below the axis's cell count, so it fits.
                (span as usize).min(whole)
            } else {
                whole
            }
        };
        [
            axis(grid.cell_width(), grid.cells_x()),
            axis(grid.cell_height(), grid.cells_y()),
        ]
    }

    /// The first half of [`SourceKernel::support_into`]: the candidate rows
    /// and columns whose cells may lie within the radius of `center`
    /// ([`SourceSupport::rows`], [`SourceSupport::columns`]). It empties the
    /// support's cells.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite `center`.
    pub fn candidates_into(
        &self,
        grid: &FieldGrid,
        center: [f64; 2],
        support: &mut SourceSupport,
    ) -> Result<(), CoreError> {
        support.cell_indices.clear();
        support.cell_weights.clear();
        support.runs.clear();
        support.max_cell = 0;
        support.weight_sum = 0.0;
        support.prepared = None;
        let columns = axis_candidates(
            center[0],
            self.radius,
            grid.cell_width(),
            grid.cells_x(),
            &mut support.columns,
        );
        let rows = axis_candidates(
            center[1],
            self.radius,
            grid.cell_height(),
            grid.cells_y(),
            &mut support.rows,
        );
        if !columns || !rows {
            return Err(CoreError::NumericalInvalid {
                quantity: "source center",
                detail: format!("{center:?} is not a finite position"),
            });
        }
        Ok(())
    }

    /// The second half of [`SourceKernel::support_into`]: every candidate
    /// cell, rows ascending and then columns ascending (which is cell-index
    /// order), given its raw weight by `weigh(column, row)` or left out for
    /// `None`. The weights are summed in that order. An accelerator that
    /// computes [`SourceKernel::weight`] itself passes its results here, so
    /// the support, its sum and its refusals are this function's.
    ///
    /// # Errors
    /// `weigh`'s errors, and [`CoreError::NumericalInvalid`] when the
    /// support holds no cell or its weights do not sum to a finite positive
    /// value.
    pub fn weigh_into(
        &self,
        grid: &FieldGrid,
        center: [f64; 2],
        support: &mut SourceSupport,
        mut weigh: impl FnMut(u32, u32) -> Result<Option<f64>, CoreError>,
    ) -> Result<(), CoreError> {
        let SourceSupport {
            cell_indices,
            cell_weights,
            runs,
            max_cell,
            weight_sum,
            columns,
            rows,
            prepared,
            ..
        } = support;
        cell_indices.clear();
        cell_weights.clear();
        runs.clear();
        *max_cell = 0;
        *weight_sum = 0.0;
        *prepared = None;
        for &row in rows.iter() {
            for &column in columns.iter() {
                let Some(weight) = weigh(column, row)? else {
                    continue;
                };
                let index = grid.cell_index(column, row).ok_or_else(overflow)?;
                cell_indices.push(index);
                cell_weights.push(weight);
                if index > *max_cell {
                    *max_cell = index;
                }
                *weight_sum += weight;
            }
        }
        if cell_indices.is_empty() || *weight_sum <= 0.0 || !weight_sum.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "source support",
                detail: format!(
                    "no cell center lies within {:?} of {center:?} with positive weight",
                    self.radius
                ),
            });
        }
        rebuild_runs(cell_indices, runs);
        Ok(())
    }

    /// `(r², 2σ²)`, the constants of [`SourceKernel::weight`].
    #[must_use]
    pub fn weight_constants(&self) -> (f64, f64) {
        (self.radius * self.radius, 2.0 * self.sigma * self.sigma)
    }

    /// The raw weight `exp(−|d|²/(2σ²))` of cell `(column, row)` for a source
    /// at `center`, where `d` is the minimum-image displacement from
    /// `center` to the cell's center; `None` beyond the radius.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] when `center` is not a wrapped
    /// position, or the weight does not exist.
    pub fn weight(
        &self,
        grid: &FieldGrid,
        center: [f64; 2],
        column: u32,
        row: u32,
    ) -> Result<Option<f64>, CoreError> {
        let (radius_squared, two_sigma_squared) = self.weight_constants();
        let cell_center = [
            (f64::from(column) + 0.5) * grid.cell_width(),
            (f64::from(row) + 0.5) * grid.cell_height(),
        ];
        let [dx, dy] = grid.displacement(center, cell_center)?;
        let distance_squared = dx * dx + dy * dy;
        if distance_squared > radius_squared {
            return Ok(None);
        }
        exp_neg(distance_squared / two_sigma_squared)
            .map(Some)
            .ok_or_else(|| CoreError::NumericalInvalid {
                quantity: "source weight",
                detail: format!("|d|² = {distance_squared:?} has no weight"),
            })
    }
}

/// The candidate cell indices along one axis, sorted and distinct: every
/// index whose center can lie within `radius` of `coordinate`. Returns false
/// for a nonfinite coordinate.
fn axis_candidates(
    coordinate: f64,
    radius: f64,
    cell: f64,
    cells: u32,
    out: &mut Vec<u32>,
) -> bool {
    out.clear();
    if !coordinate.is_finite() {
        return false;
    }
    // Centers at (i + ½) h within `radius`: i ∈ [(x − r)/h − ½, (x + r)/h − ½].
    // Widen by one cell on each side against rounding; the exact distance
    // test decides membership.
    const MAX_EXACT_AXIS_INDEX: f64 = (1_u64 << 52) as f64;
    let low = ((coordinate - radius) / cell - 0.5).floor() - 1.0;
    let high = ((coordinate + radius) / cell - 0.5).ceil() + 1.0;
    let span = high - low + 1.0;
    if !span.is_finite()
        || !low.is_finite()
        || !high.is_finite()
        || low.abs() >= MAX_EXACT_AXIS_INDEX
        || high.abs() >= MAX_EXACT_AXIS_INDEX
        || span >= f64::from(cells)
    {
        out.extend(0..cells);
        return true;
    }
    // |low|, |high| < 2^52 < i64::MAX, so both casts are exact and never saturate.
    let (low, high) = (low as i64, high as i64);
    let modulus = i64::from(cells);
    out.extend((low..=high).filter_map(|index| u32::try_from(index.rem_euclid(modulus)).ok()));
    out.sort_unstable();
    out.dedup();
    true
}

/// Contiguous cell-index span inside a [`SourceSupport`], letting energy and
/// moment deposition stream linear plane slices without reading `cell_indices`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CellRun {
    cell_start: usize,
    offset: usize,
    len: usize,
}

fn rebuild_runs(cell_indices: &[usize], runs: &mut Vec<CellRun>) {
    runs.clear();
    let n = cell_indices.len();
    let mut i = 0;
    while i < n {
        let cell_start = cell_indices[i];
        let offset = i;
        let mut prev = cell_start;
        i += 1;
        while i < n && cell_indices[i] == prev + 1 {
            prev = cell_indices[i];
            i += 1;
        }
        runs.push(CellRun {
            cell_start,
            offset,
            len: i - offset,
        });
    }
}

/// A source's support: cell indices and raw weights, in cell-index order.
/// Stored as parallel SoA arrays plus compact contiguous [`CellRun`] spans so
/// 32-plane moment deposition streams linear plane slices without dragging
/// `cell_indices` or raw weights through cache.
/// It is reused across ticks, so steady state does not allocate.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceSupport {
    cell_indices: Vec<usize>,
    cell_weights: Vec<f64>,
    runs: Vec<CellRun>,
    max_cell: usize,
    weight_sum: f64,
    columns: Vec<u32>,
    rows: Vec<u32>,
    /// Each cell's deposited energy this tick, in `cell_indices` order.
    amounts: Vec<f64>,
    /// Cached `(declared, normalized_total)` when [`Self::prepare_amounts`]
    /// precomputed `amounts` in parallel right after [`SourceKernel::support_into`].
    prepared: Option<(f64, f64)>,
}

impl SourceSupport {
    /// An empty support.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty support with room for any of `kernel`'s on `grid`
    /// ([`SourceKernel::candidate_bound`]), so that filling it never
    /// allocates (FL-043).
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when the host refuses the room.
    pub fn for_kernel(kernel: &SourceKernel, grid: &FieldGrid) -> Result<Self, CoreError> {
        let [columns, rows] = kernel.candidate_bound(grid);
        let cells = columns.saturating_mul(rows);
        let max_runs = rows.saturating_mul(2);
        let mut support = Self::new();
        let refused = |bytes: u64| CoreError::AllocationFailed {
            buffer: "source support",
            bytes,
        };
        support
            .columns
            .try_reserve_exact(columns)
            .map_err(|_| refused(bytes_for::<u32>(columns)))?;
        support
            .rows
            .try_reserve_exact(rows)
            .map_err(|_| refused(bytes_for::<u32>(rows)))?;
        support
            .cell_indices
            .try_reserve_exact(cells)
            .map_err(|_| refused(bytes_for::<usize>(cells)))?;
        support
            .cell_weights
            .try_reserve_exact(cells)
            .map_err(|_| refused(bytes_for::<f64>(cells)))?;
        support
            .runs
            .try_reserve_exact(max_runs)
            .map_err(|_| refused(bytes_for::<CellRun>(max_runs)))?;
        support
            .amounts
            .try_reserve_exact(cells)
            .map_err(|_| refused(bytes_for::<f64>(cells)))?;
        Ok(support)
    }

    /// Bytes its cell lists hold (FL-043).
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        total([
            capacity_bytes(&self.cell_indices),
            capacity_bytes(&self.cell_weights),
            capacity_bytes(&self.runs),
            capacity_bytes(&self.columns),
            capacity_bytes(&self.rows),
            capacity_bytes(&self.amounts),
        ])
    }

    /// Cells in the support.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cell_indices.len()
    }

    /// Whether the support holds no cell.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cell_indices.is_empty()
    }

    /// `Σ k_c` in cell-index order.
    #[must_use]
    pub const fn weight_sum(&self) -> f64 {
        self.weight_sum
    }

    /// The candidate rows of the last [`SourceKernel::candidates_into`],
    /// ascending.
    #[must_use]
    pub fn rows(&self) -> &[u32] {
        &self.rows
    }

    /// The candidate columns of the last [`SourceKernel::candidates_into`],
    /// ascending.
    #[must_use]
    pub fn columns(&self) -> &[u32] {
        &self.columns
    }

    /// Each cell's energy from the last [`deposit_energy`], in support order:
    /// the amounts the moment planes receive in proportion to the source's
    /// direction.
    #[must_use]
    pub fn amounts(&self) -> &[f64] {
        &self.amounts
    }

    /// Clear support cells and amounts when a source has zero power this tick
    /// (`V2-02` inactive oasis), retaining allocated capacity.
    pub(crate) fn clear_for_zero_power(&mut self) {
        self.cell_indices.clear();
        self.cell_weights.clear();
        self.runs.clear();
        self.max_cell = 0;
        self.weight_sum = 0.0;
        self.amounts.clear();
        self.prepared = None;
    }

    /// Precompute per-cell deposited energy `amounts` and their exact
    /// left-to-right `normalized_total` while `cell_weights` is still hot in
    /// the support worker's L1/L2 cache (bit-identical to [`deposit_energy`]).
    pub(crate) fn prepare_amounts(&mut self, source: &Source, step: TimeStep) {
        if source.power == 0.0 {
            self.amounts.clear();
            self.prepared = Some((0.0, 0.0));
            return;
        }
        let declared = source.power * f64::from(step.numerator()) / f64::from(step.denominator());
        let weight_sum = self.weight_sum;
        let len = self.cell_weights.len();
        if self.amounts.len() >= len {
            self.amounts.truncate(len);
        } else {
            self.amounts.resize(len, 0.0);
        }
        let mut normalized_total = 0.0;
        for (dst, &weight) in self.amounts.iter_mut().zip(&self.cell_weights) {
            let amount = declared * (weight / weight_sum);
            *dst = amount;
            normalized_total += amount;
        }
        self.prepared = Some((declared, normalized_total));
    }

    /// Whether any support cell lies at or beyond `cells`.
    #[inline]
    pub(crate) fn has_cell_out_of_bounds(&self, cells: usize) -> bool {
        !self.cell_indices.is_empty() && self.max_cell >= cells
    }

    /// `(cell index, k_c / Σ k)` in cell-index order.
    pub fn normalized(&self) -> impl Iterator<Item = (usize, f64)> + '_ {
        self.cell_indices
            .iter()
            .zip(&self.cell_weights)
            .map(|(&index, &weight)| (index, weight / self.weight_sum))
    }
}

/// One external source at its sampled center for this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Source {
    id: SourceId,
    center: [f64; 2],
    direction: [f64; CHANNELS],
    power: f64,
}

impl Source {
    /// Source `id` at `center` (a wrapped world position) with chemical
    /// direction `direction`, where `||direction||₂ ≤ 1`, and total power
    /// `power` in ecological-energy per simulation-second.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite or unwrapped center,
    /// a direction whose norm exceeds one, or a negative or nonfinite power.
    pub fn new(
        id: SourceId,
        center: [f64; 2],
        direction: [f64; CHANNELS],
        power: f64,
    ) -> Result<Self, CoreError> {
        if !power.is_finite() || power < 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "sources.power_per_source",
                detail: format!("{power:?} is not finite and nonnegative"),
            });
        }
        if !center
            .iter()
            .all(|value| value.is_finite() && *value >= 0.0)
        {
            return Err(CoreError::NumericalInvalid {
                quantity: "source center",
                detail: format!("{center:?} is not a wrapped position"),
            });
        }
        let norm_squared = squared_norm(&direction);
        if !norm_squared.is_finite() || norm_squared > 1.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "source chemical direction",
                detail: format!("||a||² = {norm_squared:?} exceeds 1"),
            });
        }
        Ok(Self {
            id,
            center,
            direction,
            power,
        })
    }

    /// The source identity.
    #[must_use]
    pub const fn id(&self) -> SourceId {
        self.id
    }

    /// The sampled center.
    #[must_use]
    pub const fn center(&self) -> [f64; 2] {
        self.center
    }

    /// The fixed chemical direction `a_j`.
    #[must_use]
    pub const fn direction(&self) -> &[f64; CHANNELS] {
        &self.direction
    }

    /// The same source at a new center. Under `fixed-per-source` a path
    /// update moves the center and leaves the chemical direction unchanged.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for an unwrapped center.
    pub fn moved_to(&self, center: [f64; 2]) -> Result<Self, CoreError> {
        Self::new(self.id, center, self.direction, self.power)
    }

    /// Total power `P_j`.
    #[must_use]
    pub const fn power(&self) -> f64 {
        self.power
    }

    /// The same source with a new total power `power` (`V2-02` oasis
    /// modulation).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a negative or nonfinite power.
    pub fn with_power(&self, power: f64) -> Result<Self, CoreError> {
        Self::new(self.id, self.center, self.direction, power)
    }
}

/// Phase-staggered ephemeral oasis resource landscape (`V2-02`, slug `oases`).
///
/// Modulates each source's power deterministically as a pure function of
/// committed tick `n` via host-independent [`normalize_turns`] and
/// [`sin_cos_turns`], normalizing across all `M` sources so total global
/// injected power stays constant `M × P_base` across every tick while
/// individual oases bloom and fade.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OasisSchedule {
    bloom_period_seconds: f64,
    minimum_power_fraction: f64,
    sigma_min_world_units: f64,
    sigma_max_world_units: f64,
}

impl OasisSchedule {
    /// Construct an oasis modulation schedule.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless `bloom_period_seconds > 0`,
    /// `minimum_power_fraction ∈ [0, 1]`, and `0 < sigma_min <= sigma_max`
    /// with all values finite.
    pub fn new(
        bloom_period_seconds: f64,
        minimum_power_fraction: f64,
        sigma_min_world_units: f64,
        sigma_max_world_units: f64,
    ) -> Result<Self, CoreError> {
        if !bloom_period_seconds.is_finite() || bloom_period_seconds <= 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "v2.oases.bloom_period_seconds",
                detail: format!("{bloom_period_seconds:?} is not finite and positive"),
            });
        }
        if !minimum_power_fraction.is_finite() || !(0.0..=1.0).contains(&minimum_power_fraction) {
            return Err(CoreError::NumericalInvalid {
                quantity: "v2.oases.minimum_power_fraction",
                detail: format!("{minimum_power_fraction:?} is outside [0, 1]"),
            });
        }
        if !sigma_min_world_units.is_finite()
            || !sigma_max_world_units.is_finite()
            || sigma_min_world_units <= 0.0
            || sigma_max_world_units < sigma_min_world_units
        {
            return Err(CoreError::NumericalInvalid {
                quantity: "v2.oases.sigma_world_units",
                detail: format!(
                    "invalid sigma range [{sigma_min_world_units:?}, {sigma_max_world_units:?}]"
                ),
            });
        }
        Ok(Self {
            bloom_period_seconds,
            minimum_power_fraction,
            sigma_min_world_units,
            sigma_max_world_units,
        })
    }

    /// Resolve `[v2.oases]` from a validated configuration: `None` on `v1` or
    /// when `V2Feature::Oases` is disabled.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for invalid bounds or when
    /// `0 < sources.count < 3` with `minimum_power_fraction == 0.0` (which
    /// hits `0/0 = NaN` at phase zeroes).
    pub fn from_config(config: &ValidatedConfig) -> Result<Option<Self>, CoreError> {
        let Some(v2) = config.config().v2.as_ref() else {
            return Ok(None);
        };
        if !v2.is_enabled(V2Feature::Oases) {
            return Ok(None);
        }
        let oases = &v2.oases;
        let count = config.config().sources.count;
        if count > 0 && count < 3 && oases.minimum_power_fraction == 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "v2.oases.minimum_power_fraction",
                detail: format!(
                    "oases with sources.count = {count} (< 3) and minimum_power_fraction = 0.0 \
                     can hit 0/0 = NaN at phase zeroes"
                ),
            });
        }
        let base_sigma = config.config().sources.sigma_world_units;
        let sigma_min = oases.sigma_min_world_units.unwrap_or(base_sigma);
        let sigma_max = oases.sigma_max_world_units.unwrap_or(sigma_min);
        Self::new(
            oases.bloom_period_seconds,
            oases.minimum_power_fraction,
            sigma_min,
            sigma_max,
        )
        .map(Some)
    }

    /// Bloom period `T_bloom` in simulation-seconds.
    #[must_use]
    pub const fn bloom_period_seconds(&self) -> f64 {
        self.bloom_period_seconds
    }

    /// Minimum envelope floor `∈ [0, 1]`.
    #[must_use]
    pub const fn minimum_power_fraction(&self) -> f64 {
        self.minimum_power_fraction
    }

    /// Minimum per-source Gaussian width `σ_min` in world-length.
    #[must_use]
    pub const fn sigma_min_world_units(&self) -> f64 {
        self.sigma_min_world_units
    }

    /// Maximum per-source Gaussian width `σ_max` in world-length.
    #[must_use]
    pub const fn sigma_max_world_units(&self) -> f64 {
        self.sigma_max_world_units
    }

    /// Compute constant-total-power modulated source powers `P_j(t)` for
    /// `count` sources at committed tick `tick` (mid-tick time
    /// `t = (n + 0.5) Δt`), writing them into `out` (reusing its capacity).
    ///
    /// Includes a defensive `sum_w > 0.0` guard returning zero power rather
    /// than `0/0 = NaN` if called directly with `count < 3` and floor `0.0`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] if `base_power` is negative/nonfinite
    /// or the mid-tick phase is nonfinite.
    pub fn powers_at(
        &self,
        tick: Tick,
        step: TimeStep,
        count: usize,
        base_power: f64,
        out: &mut Vec<f64>,
    ) -> Result<(), CoreError> {
        if !base_power.is_finite() || base_power < 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "sources.power_per_source",
                detail: format!("{base_power:?} is not finite and nonnegative"),
            });
        }
        out.clear();
        if count == 0 {
            return Ok(());
        }
        out.resize(count, 0.0);
        let dt = f64::from(step.numerator()) / f64::from(step.denominator());
        let mid_time = (tick.get() as f64 + 0.5) * dt;
        let m_f64 = count as f64;
        let min_frac = self.minimum_power_fraction;
        let span = 1.0 - min_frac;
        let mut sum_w = 0.0;
        for (j, slot) in out.iter_mut().enumerate() {
            let u_j = (mid_time / self.bloom_period_seconds) + (j as f64 / m_f64);
            let phase = normalize_turns(u_j).ok_or_else(|| CoreError::NumericalInvalid {
                quantity: "oasis phase",
                detail: format!("u_{j} = {u_j:?} is not finite"),
            })?;
            let (sin_val, _) = sin_cos_turns(phase).ok_or_else(|| CoreError::NumericalInvalid {
                quantity: "oasis phase",
                detail: format!("normalized phase {phase:?} left [0, 1)"),
            })?;
            let pos = sin_val.max(0.0);
            let w_j = min_frac + span * (pos * pos);
            *slot = w_j;
            sum_w += w_j;
        }
        let total_power = m_f64 * base_power;
        if !total_power.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "sources.power_per_source",
                detail: format!("total oasis power {total_power:?} overflows F64"),
            });
        }
        if sum_w > 0.0 && sum_w.is_finite() {
            for slot in out.iter_mut() {
                *slot = if *slot == 0.0 {
                    0.0
                } else {
                    total_power * (*slot / sum_w)
                };
            }
        } else {
            out.fill(0.0);
        }
        Ok(())
    }

    /// Update each source's power in `sources` to its modulated `P_j(t)` at
    /// committed tick `tick`, reusing `scratch`.
    ///
    /// # Errors
    /// As [`OasisSchedule::powers_at`] and [`Source::with_power`].
    pub fn apply_powers(
        &self,
        tick: Tick,
        step: TimeStep,
        base_power: f64,
        sources: &mut [Source],
        scratch: &mut Vec<f64>,
    ) -> Result<(), CoreError> {
        self.powers_at(tick, step, sources.len(), base_power, scratch)?;
        for (source, &power) in sources.iter_mut().zip(scratch.iter()) {
            *source = source.with_power(power)?;
        }
        Ok(())
    }

    /// Deterministically seed one [`SourceKernel`] per source with its
    /// heterogeneous `σ_j ∈ [sigma_min_world_units, sigma_max_world_units]`,
    /// preserving `base`'s cutoff factor and fused mode.
    ///
    /// # Errors
    /// As [`source_sigma`] and [`SourceKernel::with_sigma`].
    pub fn seed_kernels(
        &self,
        rng: &SemanticRng,
        base: &SourceKernel,
        sources: &[Source],
    ) -> Result<Vec<SourceKernel>, CoreError> {
        let mut kernels = Vec::with_capacity(sources.len());
        for source in sources {
            let sigma = source_sigma(
                rng,
                source.id(),
                self.sigma_min_world_units,
                self.sigma_max_world_units,
            )?;
            kernels.push(base.with_sigma(sigma)?);
        }
        Ok(kernels)
    }
}

/// `Σ a_k²` in channel order, in F64: the norm rule for every chemical
/// direction (a source's, a material brush's).
#[must_use]
pub fn squared_norm(values: &[f64]) -> f64 {
    values
        .iter()
        .fold(0.0, |total, &value| total + value * value)
}

/// The unit chemical direction of `seeded-unit-vectors` from 32 independent
/// standard-normal draws (A.3 item 7). It is `z / ||z||₂`, with the squares
/// summed in channel order and divided by one correctly rounded `sqrt`. The
/// draws themselves come from [`crate::seeding::chemical_direction`].
///
/// Rounding can leave `||a||² = 1 + 2^-52`. The cone argument needs
/// `||a|| ≤ 1` exactly (Appendix D), so while the channel-order sum of
/// squares exceeds one, every component is scaled by `1 − 2^-53`. One pass
/// almost always suffices.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for a nonfinite or all-zero draw.
pub fn unit_direction(normals: &[f64; CHANNELS]) -> Result<[f64; CHANNELS], CoreError> {
    let norm = squared_norm(normals).sqrt();
    if !norm.is_finite() || norm <= 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "source chemical direction",
            detail: format!("normal draws with norm {norm:?} have no direction"),
        });
    }
    let mut unit = normals.map(|value| value / norm);
    let shrink = 1.0 - f64::EPSILON / 2.0;
    for _ in 0..64 {
        if squared_norm(&unit) <= 1.0 {
            return Ok(unit);
        }
        unit = unit.map(|value| value * shrink);
    }
    Err(CoreError::NumericalInvalid {
        quantity: "source chemical direction",
        detail: "could not bring ||a|| within 1".into(),
    })
}

/// What one source deposited this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DepositReport {
    /// The source.
    pub source: SourceId,
    /// Cells in its support.
    pub support_cells: usize,
    /// `P_j Δt`: the declared external injection.
    pub declared: f64,
    /// `Σ_c q_c` in F64. It equals `declared` up to F64 rounding,
    /// whatever the support.
    pub normalized_total: f64,
    /// `Σ_c (e'_c − e_c)` over the F32 values actually stored.
    pub applied: f64,
}

/// Deposit `source` into `state` for one tick of `step`, reusing `support`.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for an empty support, and
/// [`CoreError::LayoutMismatch`] if a cell falls outside `state`.
pub fn deposit(
    state: &mut FieldState,
    kernel: &SourceKernel,
    source: &Source,
    step: TimeStep,
    support: &mut SourceSupport,
) -> Result<DepositReport, CoreError> {
    if source.power == 0.0 {
        support.clear_for_zero_power();
        return Ok(DepositReport {
            source: source.id,
            support_cells: 0,
            declared: 0.0,
            normalized_total: 0.0,
            applied: 0.0,
        });
    }
    let grid = *state.grid();
    kernel.support_into(&grid, source.center, support)?;
    let report = deposit_energy(state, source, step, support)?;
    let active = state.carrier0_moments_count();
    for (channel, &component) in (0_u32..).zip(source.direction.iter().take(active)) {
        let plane = state.moment_mut(channel).ok_or_else(outside)?;
        deposit_moment(plane, component, support)?;
    }
    Ok(report)
}

/// The energy part of [`deposit`], in support order, for a `support`
/// already computed at the source's center. It records each cell's amount
/// for the moment planes. An accelerator that deposits the moments itself
/// calls this for the energy plane, so the report is this function's.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] if a support cell falls outside `state`.
pub fn deposit_energy(
    state: &mut FieldState,
    source: &Source,
    step: TimeStep,
    support: &mut SourceSupport,
) -> Result<DepositReport, CoreError> {
    if source.power == 0.0 {
        support.amounts.clear();
        support.prepared = None;
        return Ok(DepositReport {
            source: source.id,
            support_cells: 0,
            declared: 0.0,
            normalized_total: 0.0,
            applied: 0.0,
        });
    }
    let declared = source.power * f64::from(step.numerator()) / f64::from(step.denominator());
    let energy = state.energy_mut();
    if let Some((prep_decl, prep_norm)) = support.prepared
        && prep_decl.to_bits() == declared.to_bits()
        && support.amounts.len() == support.cell_indices.len()
    {
        let mut applied = 0.0;
        if !support.runs.is_empty() && support.max_cell < energy.len() {
            for &run in &support.runs {
                let e_slice = &mut energy[run.cell_start..run.cell_start + run.len];
                let a_slice = &support.amounts[run.offset..run.offset + run.len];
                for (slot, &amount) in e_slice.iter_mut().zip(a_slice) {
                    let before = f64::from(*slot);
                    let after = (before + amount) as f32;
                    applied += f64::from(after) - before;
                    *slot = after;
                }
            }
        } else {
            for (&index, &amount) in support.cell_indices.iter().zip(&support.amounts) {
                let slot = energy.get_mut(index).ok_or_else(outside)?;
                let before = f64::from(*slot);
                let after = (before + amount) as f32;
                applied += f64::from(after) - before;
                *slot = after;
            }
        }
        return Ok(DepositReport {
            source: source.id,
            support_cells: support.len(),
            declared,
            normalized_total: prep_norm,
            applied,
        });
    }
    let mut normalized_total = 0.0;
    let mut applied = 0.0;
    let weight_sum = support.weight_sum;
    support.prepared = None;
    if !support.runs.is_empty()
        && support.max_cell < energy.len()
        && support.cell_weights.len() >= support.cell_indices.len()
    {
        let len = support.cell_indices.len();
        if support.amounts.len() >= len {
            support.amounts.truncate(len);
        } else {
            support.amounts.resize(len, 0.0);
        }
        for &run in &support.runs {
            let e_slice = &mut energy[run.cell_start..run.cell_start + run.len];
            let w_slice = &support.cell_weights[run.offset..run.offset + run.len];
            let a_slice = &mut support.amounts[run.offset..run.offset + run.len];
            for ((slot, &weight), dst) in e_slice.iter_mut().zip(w_slice).zip(a_slice) {
                let amount = declared * (weight / weight_sum);
                normalized_total += amount;
                let before = f64::from(*slot);
                let after = (before + amount) as f32;
                applied += f64::from(after) - before;
                *slot = after;
                *dst = amount;
            }
        }
    } else {
        support.amounts.clear();
        for (&index, &weight) in support.cell_indices.iter().zip(&support.cell_weights) {
            let amount = declared * (weight / weight_sum);
            normalized_total += amount;
            let energy = energy.get_mut(index).ok_or_else(outside)?;
            let before = f64::from(*energy);
            let after = (before + amount) as f32;
            applied += f64::from(after) - before;
            *energy = after;
            support.amounts.push(amount);
        }
    }
    Ok(DepositReport {
        source: source.id,
        support_cells: support.len(),
        declared,
        normalized_total,
        applied,
    })
}

/// Fused energy deposition for [`TRUNCATED_GAUSSIAN_FUSED_V1`] (`flife-ze4`):
/// accumulates every simultaneous source's energy contribution per cell in F64
/// (in source order) and rounds each touched cell once to F32. Records each
/// source's per-cell amounts into its support for the moment planes.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] if `sources.len() != supports.len()` or a
/// support cell falls outside `state`, or [`CoreError::AllocationFailed`] if
/// `accum` cannot hold the grid's cells.
pub fn deposit_energy_fused(
    state: &mut FieldState,
    sources: &[Source],
    step: TimeStep,
    supports: &mut [SourceSupport],
    accum: &mut Vec<f64>,
) -> Result<Vec<DepositReport>, CoreError> {
    if sources.len() != supports.len() {
        return Err(CoreError::LayoutMismatch {
            buffer: "fused source supports",
            detail: format!(
                "{} source supports for {} sources",
                supports.len(),
                sources.len()
            ),
        });
    }
    let energy = state.energy_mut();
    let cells = energy.len();
    if accum.len() < cells {
        let extra = cells - accum.len();
        accum
            .try_reserve_exact(extra)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "fused source energy accumulator",
                bytes: bytes_for::<f64>(cells),
            })?;
        accum.resize(cells, 0.0);
    }
    let deltas = &mut accum[..cells];
    deltas.fill(0.0);

    let mut reports = Vec::with_capacity(sources.len());
    for (source, support) in sources.iter().zip(supports.iter_mut()) {
        support.amounts.clear();
        if source.power == 0.0 {
            reports.push(DepositReport {
                source: source.id,
                support_cells: 0,
                declared: 0.0,
                normalized_total: 0.0,
                applied: 0.0,
            });
            continue;
        }
        let declared = source.power * f64::from(step.numerator()) / f64::from(step.denominator());
        let mut normalized_total = 0.0;
        for (&index, &weight) in support.cell_indices.iter().zip(&support.cell_weights) {
            let amount = declared * (weight / support.weight_sum);
            normalized_total += amount;
            let slot = deltas.get_mut(index).ok_or_else(outside)?;
            *slot += amount;
            support.amounts.push(amount);
        }
        reports.push(DepositReport {
            source: source.id,
            support_cells: support.len(),
            declared,
            normalized_total,
            applied: normalized_total,
        });
    }

    let mut total_applied = 0.0;
    for (cell, &delta) in energy.iter_mut().zip(deltas.iter()) {
        if delta != 0.0 {
            let before = f64::from(*cell);
            let after = (before + delta) as f32;
            total_applied += f64::from(after) - before;
            *cell = after;
        }
    }
    if let Some(last_active) = reports.iter().rposition(|r| r.declared > 0.0) {
        let other_applied: f64 = reports
            .iter()
            .enumerate()
            .filter(|&(index, _)| index != last_active)
            .map(|(_, report)| report.applied)
            .sum();
        reports[last_active].applied = total_applied - other_applied;
    }
    Ok(reports)
}

/// Fused moment deposition for one plane (`flife-ze4`): sums every source's
/// `amount * component` per cell in source order in F64 and rounds each touched
/// cell once to F32.
pub(crate) fn deposit_moments_fused(
    plane: &mut [f32],
    sources: &[Source],
    channel: usize,
    supports: &[SourceSupport],
    accum: &mut Vec<f64>,
) -> Result<(), CoreError> {
    if sources.len() != supports.len() {
        return Err(CoreError::LayoutMismatch {
            buffer: "fused source supports",
            detail: format!(
                "{} source supports for {} sources",
                supports.len(),
                sources.len()
            ),
        });
    }
    let cells = plane.len();
    if accum.len() < cells {
        let extra = cells - accum.len();
        accum
            .try_reserve_exact(extra)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "fused source moment accumulator",
                bytes: bytes_for::<f64>(cells),
            })?;
        accum.resize(cells, 0.0);
    }
    let deltas = &mut accum[..cells];
    deltas.fill(0.0);
    for (source, support) in sources.iter().zip(supports) {
        let c = component(source, channel);
        if c == 0.0 {
            continue;
        }
        for (&index, &amount) in support.cell_indices.iter().zip(&support.amounts) {
            let slot = deltas.get_mut(index).ok_or_else(outside)?;
            *slot += amount * c;
        }
    }
    for (moment, &delta) in plane.iter_mut().zip(deltas.iter()) {
        if delta != 0.0 {
            *moment = (f64::from(*moment) + delta) as f32;
        }
    }
    Ok(())
}

/// One moment plane's part of [`deposit`]: `component` of each amount
/// [`deposit_energy`] recorded. A source adds to a cell's moment once, so
/// updating plane by plane computes exactly what cell by cell would.
pub(crate) fn deposit_moment(
    plane: &mut [f32],
    component: f64,
    support: &SourceSupport,
) -> Result<(), CoreError> {
    if component == 0.0 {
        return Ok(());
    }
    let len = support.cell_indices.len().min(support.amounts.len());
    if len == 0 {
        return Ok(());
    }
    if support.max_cell < plane.len() {
        if !support.runs.is_empty() && support.amounts.len() >= support.cell_indices.len() {
            let amounts = &support.amounts;
            for &run in &support.runs {
                let p_run = &mut plane[run.cell_start..run.cell_start + run.len];
                let a_run = &amounts[run.offset..run.offset + run.len];
                for (slot, &amount) in p_run.iter_mut().zip(a_run) {
                    *slot = (f64::from(*slot) + amount * component) as f32;
                }
            }
            return Ok(());
        }
        let indices = &support.cell_indices[..len];
        let amounts = &support.amounts[..len];
        let (idx_chunks, idx_rem) = indices.as_chunks::<4>();
        let (amt_chunks, amt_rem) = amounts.as_chunks::<4>();
        for (&[i0, i1, i2, i3], &[a0, a1, a2, a3]) in idx_chunks.iter().zip(amt_chunks) {
            let d0 = a0 * component;
            let d1 = a1 * component;
            let d2 = a2 * component;
            let d3 = a3 * component;
            plane[i0] = (f64::from(plane[i0]) + d0) as f32;
            plane[i1] = (f64::from(plane[i1]) + d1) as f32;
            plane[i2] = (f64::from(plane[i2]) + d2) as f32;
            plane[i3] = (f64::from(plane[i3]) + d3) as f32;
        }
        for (&index, &amount) in idx_rem.iter().zip(amt_rem) {
            plane[index] = (f64::from(plane[index]) + amount * component) as f32;
        }
        return Ok(());
    }
    for (&index, &amount) in support.cell_indices.iter().zip(&support.amounts) {
        let moment = plane.get_mut(index).ok_or_else(outside)?;
        *moment = (f64::from(*moment) + amount * component) as f32;
    }
    Ok(())
}

/// Deposit every source in exact source order into one cell band
/// `[band_start, band_end)` across up to four moment planes simultaneously.
///
/// Shares each `support.amounts` read across all planes in the group, skips
/// `cell_indices` entirely via contiguous [`CellRun`] slices, and keeps the
/// plane band hot in L2 cache across overlapping sources while preserving
/// exact per-source F32 rounding (0-ulp bit-identical to [`deposit_moment`]).
pub(crate) fn deposit_moment_band_group(
    band_start: usize,
    band_end: usize,
    channel_base: usize,
    planes: &mut [&mut [f32]],
    sources: &[Source],
    supports: &[SourceSupport],
) -> Result<(), CoreError> {
    if band_start >= band_end || planes.is_empty() {
        return Ok(());
    }
    for (source, support) in sources.iter().zip(supports) {
        let runs = &support.runs;
        if runs.is_empty()
            || support.amounts.len() < support.cell_indices.len()
            || support.max_cell < band_start
            || runs[0].cell_start >= band_end
        {
            continue;
        }
        let first_run = runs.partition_point(|r| r.cell_start + r.len <= band_start);
        if first_run >= runs.len() || runs[first_run].cell_start >= band_end {
            continue;
        }
        if let [p0, p1, p2, p3] = planes {
            let c0 = component(source, channel_base);
            let c1 = component(source, channel_base + 1);
            let c2 = component(source, channel_base + 2);
            let c3 = component(source, channel_base + 3);
            if c0 != 0.0 && c1 != 0.0 && c2 != 0.0 && c3 != 0.0 {
                for &run in &runs[first_run..] {
                    if run.cell_start >= band_end {
                        break;
                    }
                    let s = run.cell_start.max(band_start);
                    let e = (run.cell_start + run.len).min(band_end);
                    if s >= e {
                        continue;
                    }
                    let ls = s - band_start;
                    let le = e - band_start;
                    let a_s = run.offset + (s - run.cell_start);
                    let a_e = a_s + (e - s);
                    let amts = &support.amounts[a_s..a_e];
                    for (((v0, v1), (v2, v3)), &amount) in p0[ls..le]
                        .iter_mut()
                        .zip(p1[ls..le].iter_mut())
                        .zip(p2[ls..le].iter_mut().zip(p3[ls..le].iter_mut()))
                        .zip(amts)
                    {
                        *v0 = (f64::from(*v0) + amount * c0) as f32;
                        *v1 = (f64::from(*v1) + amount * c1) as f32;
                        *v2 = (f64::from(*v2) + amount * c2) as f32;
                        *v3 = (f64::from(*v3) + amount * c3) as f32;
                    }
                }
                continue;
            }
        }
        for (offset, plane) in planes.iter_mut().enumerate() {
            let c = component(source, channel_base + offset);
            if c == 0.0 {
                continue;
            }
            for &run in &runs[first_run..] {
                if run.cell_start >= band_end {
                    break;
                }
                let s = run.cell_start.max(band_start);
                let e = (run.cell_start + run.len).min(band_end);
                if s >= e {
                    continue;
                }
                let ls = s - band_start;
                let le = e - band_start;
                let a_s = run.offset + (s - run.cell_start);
                let a_e = a_s + (e - s);
                for (slot, &amount) in plane[ls..le].iter_mut().zip(&support.amounts[a_s..a_e]) {
                    *slot = (f64::from(*slot) + amount * c) as f32;
                }
            }
        }
    }
    Ok(())
}

/// The direction component `channel` of `source`; zero past the last.
pub(crate) fn component(source: &Source, channel: usize) -> f64 {
    source.direction.get(channel).copied().unwrap_or(0.0)
}

fn outside() -> CoreError {
    CoreError::LayoutMismatch {
        buffer: crate::layout::BUFFER,
        detail: "a source support cell lies outside the field".into(),
    }
}

/// One segment of `seeded-smooth-torus-v1` (A.3 item 8). Over a segment of
/// `K` ticks (`T = K Δt`), the travel direction blends from `a` to `b` by
/// `h(u) = 3u² − 2u³`. The displacement integrates analytically:
///
/// `s(u) = v T [a u + (b − a)(u³ − u⁴/2)]`,  `v(u) = v [(1 − h) a + h b]`.
///
/// The speed is therefore at most `v`. The velocity is continuous across a
/// join because the next segment starts at the wrapped endpoint with
/// direction `b`. Within-segment time is derived from integer ticks, never
/// accumulated.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathSegment {
    anchor: [f64; 2],
    start: [f64; 2],
    end: [f64; 2],
    speed: f64,
    ticks: u64,
    seconds: f64,
}

/// How far a travel direction's norm may stray from one.
const UNIT_TOLERANCE: f64 = 1.0e-12;

impl PathSegment {
    /// The wrapped point the segment starts from.
    #[must_use]
    pub const fn anchor(&self) -> [f64; 2] {
        self.anchor
    }

    /// A segment from the wrapped `anchor`, blending unit direction `start`
    /// into `end`, at `speed` world-length per simulation-second over
    /// `ticks` ticks of `step`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a non-unit direction, a negative
    /// or nonfinite speed, an unwrapped anchor or a zero-length segment.
    pub fn new(
        grid: &FieldGrid,
        anchor: [f64; 2],
        start: [f64; 2],
        end: [f64; 2],
        speed: f64,
        ticks: TickSpan,
        step: TimeStep,
    ) -> Result<Self, CoreError> {
        let invalid = |detail: String| CoreError::NumericalInvalid {
            quantity: "source path",
            detail,
        };
        if grid.wrap_position(anchor)? != anchor {
            return Err(invalid(format!("anchor {anchor:?} is not wrapped")));
        }
        for direction in [start, end] {
            let norm_squared = direction[0] * direction[0] + direction[1] * direction[1];
            if !norm_squared.is_finite() || (norm_squared - 1.0).abs() > UNIT_TOLERANCE {
                return Err(invalid(format!(
                    "direction {direction:?} is not a unit vector"
                )));
            }
        }
        if !speed.is_finite() || speed < 0.0 {
            return Err(invalid(format!(
                "speed {speed:?} is not finite and nonnegative"
            )));
        }
        if ticks.get() == 0 {
            return Err(invalid("a segment of zero ticks".into()));
        }
        Ok(Self {
            anchor,
            start,
            end,
            speed,
            ticks: ticks.get(),
            seconds: step.seconds(ticks),
        })
    }

    /// The segment's length in ticks.
    #[must_use]
    pub const fn ticks(&self) -> u64 {
        self.ticks
    }

    /// `u = (k + ½) / K` for the advance through tick `k` of the segment:
    /// source centers are sampled at mid-tick, `(n + ½) Δt` (A.3 item 9).
    ///
    /// # Errors
    /// [`CoreError::Overflow`] when `k` is not inside the segment.
    pub fn mid_tick(&self, tick_in_segment: u64) -> Result<f64, CoreError> {
        if tick_in_segment >= self.ticks || self.ticks > 1 << 51 {
            return Err(CoreError::Overflow {
                quantity: "source path tick",
            });
        }
        // Both below 2^52, so exact in F64; the division rounds once.
        Ok((2 * tick_in_segment + 1) as f64 / (2 * self.ticks) as f64)
    }

    /// The wrapped position at segment fraction `u ∈ [0, 1]`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for `u` outside `[0, 1]`.
    pub fn position(&self, grid: &FieldGrid, u: f64) -> Result<[f64; 2], CoreError> {
        let u = fraction(u)?;
        let distance = self.speed * self.seconds;
        let blend = u * u * u - u * u * u * u / 2.0;
        let mut position = [0.0; 2];
        for (axis, slot) in position.iter_mut().enumerate() {
            let (a, b) = (self.start[axis], self.end[axis]);
            let displacement = distance * (a * u + (b - a) * blend);
            let length = if axis == 0 {
                grid.world_width()
            } else {
                grid.world_height()
            };
            *slot = wrap_coordinate(self.anchor[axis] + displacement, length)?;
        }
        Ok(position)
    }

    /// The velocity at segment fraction `u ∈ [0, 1]`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for `u` outside `[0, 1]`.
    pub fn velocity(&self, u: f64) -> Result<[f64; 2], CoreError> {
        let u = fraction(u)?;
        let h = 3.0 * u * u - 2.0 * u * u * u;
        Ok([0, 1].map(|axis| self.speed * ((1.0 - h) * self.start[axis] + h * self.end[axis])))
    }

    /// The next segment. It starts at this segment's wrapped endpoint with
    /// this segment's end direction, and blends into `next_end`.
    ///
    /// # Errors
    /// As [`PathSegment::new`].
    pub fn next(
        &self,
        grid: &FieldGrid,
        next_end: [f64; 2],
        step: TimeStep,
    ) -> Result<Self, CoreError> {
        Self::new(
            grid,
            self.position(grid, 1.0)?,
            self.end,
            next_end,
            self.speed,
            TickSpan::new(self.ticks),
            step,
        )
    }
}

fn fraction(u: f64) -> Result<f64, CoreError> {
    if (0.0..=1.0).contains(&u) {
        Ok(u)
    } else {
        Err(CoreError::NumericalInvalid {
            quantity: "source path fraction",
            detail: format!("{u:?} is outside [0, 1]"),
        })
    }
}
