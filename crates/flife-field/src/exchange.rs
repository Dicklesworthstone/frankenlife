//! Exchange footprints (`grid-cic-v1` and `fixed-world-gaussian-v1`) and the
//! A.1 uptake request law (plan §4.3, §5.1, §11.6, A.1; FL-005, FL-053).
//!
//! Consumption is local. Under `grid-cic-v1`, an organism at `x` exchanges
//! energy only with the cells of its bilinear cloud-in-cell footprint. The
//! continuous cell index is `(x/h_x − ½, y/h_y − ½)`. Its floor selects the
//! lower-left cell and its fractional part `(f_x, f_y)` gives the four weights
//! `(1−f_x)(1−f_y)`, `f_x(1−f_y)`, `(1−f_x)f_y` and `f_x f_y`, wrapped
//! periodically. A cell named twice on a grid one cell wide is combined into
//! one entry, and zero weights are dropped. The weights are nonnegative and
//! sum to one up to F64 rounding.
//!
//! Under `fixed-world-gaussian-v1` (FL-053, plan §4.3, §11.6), the physical
//! exchange radius `R` is fixed in world units rather than shrinking with
//! grid spacing `h`. Every cell center within minimum-image distance `R` of
//! `x` receives raw weight `k_c = exp(−|d_c|² / (2σ²))` with `σ = R / 3`
//! (3σ cutoff, using host-independent `exp_neg`), normalized by `Σ k_c` in
//! ascending cell-index order so `w_c ≥ 0` and `Σ w_c = 1` up to F64
//! rounding. Under grid refinement at fixed physical world size and radius
//! `R`, both sampled local density `Σ w_c e_c / (h_x h_y)` and deposited
//! density converge smoothly to the continuous fixed-radius Gaussian integral.
//! A long-range sensor therefore never becomes long-range feeding under either
//! kernel.

use flife_core::memory::{bytes_for, capacity_bytes, total};
use flife_core::{CoreError, TimeStep, ValidatedConfig};

use crate::grid::{FieldGrid, minimum_image};
use crate::numerics::{exp_neg, exp_neg_4};
use crate::state::FieldState;

/// The baseline bilinear cloud-in-cell exchange kernel.
pub const GRID_CIC_V1: &str = "grid-cic-v1";

/// The fixed-world-radius truncated minimum-image Gaussian exchange kernel
/// (FL-053, plan §4.3, §11.6).
pub const FIXED_WORLD_GAUSSIAN_V1: &str = "fixed-world-gaussian-v1";

/// Cutoff radius in units of `σ` for [`FIXED_WORLD_GAUSSIAN_V1`]: `R = 3σ`,
/// matching the baseline source kernel's 3σ cutoff.
pub const FIXED_WORLD_GAUSSIAN_CUTOFF_SIGMA: f64 = 3.0;

/// The most cells one fixed-world exchange footprint may span (FL-053,
/// FL-043).
pub const MAXIMUM_EXCHANGE_CELLS: usize = 4_096;

/// Reusable scratch for building an [`ExchangeKernel`] footprint without
/// per-organism heap allocations during steady-state ticks (FL-043, FL-053).
#[derive(Debug, Clone, Default)]
pub struct ExchangeScratch {
    columns: Vec<u32>,
    rows: Vec<u32>,
    cells: Vec<(usize, f64)>,
}

impl ExchangeScratch {
    /// Empty scratch.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Scratch reserved for any footprint of `kernel` on `grid` (FL-043).
    /// Returns empty scratch (0 bytes) for [`ExchangeKernel::GridCicV1`],
    /// whose four-cell stencil lives on the stack.
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when the host refuses the room.
    pub fn for_kernel(kernel: ExchangeKernel, grid: &FieldGrid) -> Result<Self, CoreError> {
        let ExchangeKernel::FixedWorldGaussianV1 { radius, .. } = kernel else {
            return Ok(Self::new());
        };
        let cols = axis_candidate_bound(radius, grid.cell_width(), grid.cells_x());
        let rows = axis_candidate_bound(radius, grid.cell_height(), grid.cells_y());
        let max_cells = kernel.max_cells(grid);
        let mut scratch = Self::new();
        let refused = |bytes: u64| CoreError::AllocationFailed {
            buffer: "exchange footprint scratch",
            bytes,
        };
        scratch
            .columns
            .try_reserve_exact(cols)
            .map_err(|_| refused(bytes_for::<u32>(cols)))?;
        scratch
            .rows
            .try_reserve_exact(rows)
            .map_err(|_| refused(bytes_for::<u32>(rows)))?;
        scratch
            .cells
            .try_reserve_exact(max_cells)
            .map_err(|_| refused(bytes_for::<(usize, f64)>(max_cells)))?;
        Ok(scratch)
    }

    /// Bytes held by its scratch vectors (FL-043).
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        total([
            capacity_bytes(&self.columns),
            capacity_bytes(&self.rows),
            capacity_bytes(&self.cells),
        ])
    }

    /// The last built footprint: `(cell_index, weight)` pairs in strictly
    /// ascending cell-index order, each weight positive.
    #[must_use]
    pub fn cells(&self) -> &[(usize, f64)] {
        &self.cells
    }
}

/// The organism-field exchange kernel (plan §4.3, §5.1, §11.6; FL-005,
/// FL-053).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExchangeKernel {
    /// `grid-cic-v1`: bilinear cloud-in-cell footprint over at most four
    /// adjacent cells.
    GridCicV1,
    /// `fixed-world-gaussian-v1`: normalized truncated minimum-image Gaussian
    /// over cell centers within physical world-unit radius `radius` and
    /// width `sigma = radius / 3`.
    FixedWorldGaussianV1 {
        /// Cutoff radius `R` in world-length.
        radius: f64,
        /// Gaussian width `σ = R / 3` in world-length.
        sigma: f64,
    },
}

impl ExchangeKernel {
    /// The baseline `grid-cic-v1` kernel.
    #[must_use]
    pub const fn grid_cic() -> Self {
        Self::GridCicV1
    }

    /// `fixed-world-gaussian-v1` with cutoff `radius` in world-length and
    /// width `σ = radius / 3`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless `radius` is finite and
    /// strictly positive.
    pub fn fixed_world_gaussian(radius: f64) -> Result<Self, CoreError> {
        let sigma = radius / FIXED_WORLD_GAUSSIAN_CUTOFF_SIGMA;
        if !(radius.is_finite() && radius > 0.0 && sigma.is_finite() && sigma > 0.0) {
            return Err(CoreError::NumericalInvalid {
                quantity: "world.exchange_radius_world_units",
                detail: format!("{radius:?} is not finite and positive"),
            });
        }
        Ok(Self::FixedWorldGaussianV1 { radius, sigma })
    }

    /// The exchange kernel a validated configuration declares.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for an unrecognized kernel name,
    /// [`CoreError::NumericalInvalid`] for a missing or invalid radius.
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let world = &config.config().world;
        match world.exchange_kernel.as_str() {
            GRID_CIC_V1 => Ok(Self::GridCicV1),
            FIXED_WORLD_GAUSSIAN_V1 => {
                let radius = world.exchange_radius_world_units.ok_or_else(|| {
                    CoreError::NumericalInvalid {
                        quantity: "world.exchange_radius_world_units",
                        detail: "fixed-world-gaussian-v1 requires exchange_radius_world_units"
                            .to_owned(),
                    }
                })?;
                Self::fixed_world_gaussian(radius)
            }
            other => Err(CoreError::UnknownAlgorithm {
                role: "world.exchange_kernel",
                name: other.to_owned(),
            }),
        }
    }

    /// Canonical registry name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::GridCicV1 => GRID_CIC_V1,
            Self::FixedWorldGaussianV1 { .. } => FIXED_WORLD_GAUSSIAN_V1,
        }
    }

    /// Whether this is the baseline `grid-cic-v1` kernel.
    #[must_use]
    pub const fn is_grid_cic(self) -> bool {
        matches!(self, Self::GridCicV1)
    }

    /// Physical cutoff radius in world-length, for fixed-world kernels.
    #[must_use]
    pub const fn radius(self) -> Option<f64> {
        match self {
            Self::GridCicV1 => None,
            Self::FixedWorldGaussianV1 { radius, .. } => Some(radius),
        }
    }

    /// Gaussian width `σ` in world-length, for fixed-world kernels.
    #[must_use]
    pub const fn sigma(self) -> Option<f64> {
        match self {
            Self::GridCicV1 => None,
            Self::FixedWorldGaussianV1 { sigma, .. } => Some(sigma),
        }
    }

    /// Upper bound on the number of cells one footprint of this kernel can
    /// touch on `grid` (used for steady-state scratch sizing and FL-043
    /// memory admission).
    #[must_use]
    pub fn max_cells(self, grid: &FieldGrid) -> usize {
        match self {
            Self::GridCicV1 => 4,
            Self::FixedWorldGaussianV1 { radius, .. } => {
                let bx = axis_footprint_bound(radius, grid.cell_width(), grid.cells_x());
                let by = axis_footprint_bound(radius, grid.cell_height(), grid.cells_y());
                let total = usize::try_from(grid.cell_count()).unwrap_or(usize::MAX);
                bx.saturating_mul(by).min(total)
            }
        }
    }

    /// Compute the footprint at `position` (wrapped onto the torus) into
    /// `scratch`, producing `(cell_index, weight)` pairs in strictly
    /// ascending cell-index order with positive weights summing to one up
    /// to F64 rounding.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite coordinate or an
    /// empty/nonpositive fixed-world support.
    pub fn footprint_into(
        self,
        grid: &FieldGrid,
        position: [f64; 2],
        scratch: &mut ExchangeScratch,
    ) -> Result<(), CoreError> {
        scratch.cells.clear();
        match self {
            Self::GridCicV1 => {
                let stencil = ExchangeStencil::at(grid, position)?;
                for (&cell, &weight) in stencil.cells().iter().zip(stencil.weights()) {
                    scratch.cells.push((cell, weight));
                }
                Ok(())
            }
            Self::FixedWorldGaussianV1 { radius, sigma } => {
                let center = grid.wrap_position(position)?;
                let cols_ok = axis_candidates(
                    center[0],
                    radius,
                    grid.cell_width(),
                    grid.cells_x(),
                    &mut scratch.columns,
                );
                let rows_ok = axis_candidates(
                    center[1],
                    radius,
                    grid.cell_height(),
                    grid.cells_y(),
                    &mut scratch.rows,
                );
                if !cols_ok || !rows_ok {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "exchange position",
                        detail: format!("{position:?} is not a finite position"),
                    });
                }
                let radius_squared = radius * radius;
                let two_sigma_squared = 2.0 * sigma * sigma;
                let width = grid.world_width();
                let height = grid.world_height();
                let hx = grid.cell_width();
                let hy = grid.cell_height();
                let cells_x = grid.cells_x();
                let cells_y = grid.cells_y();
                let stride = cells_x as usize;
                let mut weight_sum = 0.0;
                const STACK_COLS: usize = 64;
                if scratch.columns.len() <= STACK_COLS && usize::try_from(grid.cell_count()).is_ok()
                {
                    let mut col_dx_sq = [0.0_f64; STACK_COLS];
                    let col_len = scratch.columns.len();
                    for (slot, &column) in col_dx_sq.iter_mut().zip(&scratch.columns) {
                        if column >= cells_x {
                            return Err(CoreError::NumericalInvalid {
                                quantity: "exchange footprint",
                                detail: format!("column {column} is outside the grid"),
                            });
                        }
                        let cx = (f64::from(column) + 0.5) * hx;
                        let dx = minimum_image(center[0], cx, width)?;
                        *slot = dx * dx;
                    }
                    let mut pending_cells = [0_usize; 4];
                    let mut pending_args = [0.0_f64; 4];
                    for &row in &scratch.rows {
                        if row >= cells_y {
                            return Err(CoreError::NumericalInvalid {
                                quantity: "exchange footprint",
                                detail: format!("row {row} is outside the grid"),
                            });
                        }
                        let cy = (f64::from(row) + 0.5) * hy;
                        let dy = minimum_image(center[1], cy, height)?;
                        let dy_sq = dy * dy;
                        if dy_sq > radius_squared {
                            continue;
                        }
                        let row_base = (row as usize) * stride;
                        let mut pending = 0_usize;
                        for (&column, &dx_sq) in scratch.columns.iter().zip(&col_dx_sq[..col_len]) {
                            let d2 = dx_sq + dy_sq;
                            if d2 > radius_squared {
                                continue;
                            }
                            pending_cells[pending] = row_base + (column as usize);
                            pending_args[pending] = d2 / two_sigma_squared;
                            pending += 1;
                            if pending == 4 {
                                let weights = exp_neg_4(pending_args).ok_or_else(|| {
                                    CoreError::NumericalInvalid {
                                        quantity: "exchange weight",
                                        detail: format!(
                                            "Gaussian args {pending_args:?} have no weight"
                                        ),
                                    }
                                })?;
                                for (&cell, &raw) in pending_cells.iter().zip(&weights) {
                                    if raw > 0.0 {
                                        scratch.cells.push((cell, raw));
                                        weight_sum += raw;
                                    }
                                }
                                pending = 0;
                            }
                        }
                        for idx in 0..pending {
                            let raw = exp_neg(pending_args[idx]).ok_or_else(|| {
                                CoreError::NumericalInvalid {
                                    quantity: "exchange weight",
                                    detail: format!(
                                        "Gaussian arg {:?} has no weight",
                                        pending_args[idx]
                                    ),
                                }
                            })?;
                            if raw > 0.0 {
                                scratch.cells.push((pending_cells[idx], raw));
                                weight_sum += raw;
                            }
                        }
                    }
                } else {
                    for &row in &scratch.rows {
                        let cy = (f64::from(row) + 0.5) * hy;
                        let dy = minimum_image(center[1], cy, height)?;
                        let dy_sq = dy * dy;
                        if dy_sq > radius_squared {
                            continue;
                        }
                        for &column in &scratch.columns {
                            let cx = (f64::from(column) + 0.5) * hx;
                            let dx = minimum_image(center[0], cx, width)?;
                            let d2 = dx * dx + dy_sq;
                            if d2 > radius_squared {
                                continue;
                            }
                            let raw = exp_neg(d2 / two_sigma_squared).ok_or_else(|| {
                                CoreError::NumericalInvalid {
                                    quantity: "exchange weight",
                                    detail: format!("|d|² = {d2:?} has no Gaussian weight"),
                                }
                            })?;
                            if raw <= 0.0 {
                                continue;
                            }
                            let cell = grid.cell_index(column, row).ok_or_else(|| {
                                CoreError::NumericalInvalid {
                                    quantity: "exchange footprint",
                                    detail: format!("cell ({column}, {row}) is outside the grid"),
                                }
                            })?;
                            scratch.cells.push((cell, raw));
                            weight_sum += raw;
                        }
                    }
                }
                if scratch.cells.is_empty() || !weight_sum.is_finite() || weight_sum <= 0.0 {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "exchange footprint",
                        detail: format!(
                            "no cell center lies within radius {radius:?} of {center:?} with \
                             positive weight"
                        ),
                    });
                }
                for (_, weight) in &mut scratch.cells {
                    *weight /= weight_sum;
                }
                scratch.cells.retain(|&(_, weight)| weight > 0.0);
                if scratch.cells.is_empty() {
                    return Err(CoreError::NumericalInvalid {
                        quantity: "exchange footprint",
                        detail: format!(
                            "normalized weights at {center:?} underflowed to empty support"
                        ),
                    });
                }
                Ok(())
            }
        }
    }

    /// Local energy density `Σ w_c e_c / (h_x h_y)` at `position` on
    /// `field`, reusing `scratch` (ecological-energy per world-length²).
    ///
    /// # Errors
    /// As [`ExchangeKernel::footprint_into`] and [`ExchangeStencil::density`].
    pub fn density_with_scratch(
        self,
        field: &FieldState,
        position: [f64; 2],
        scratch: &mut ExchangeScratch,
    ) -> Result<f64, CoreError> {
        match self {
            Self::GridCicV1 => ExchangeStencil::at(field.grid(), position)?.density(field),
            Self::FixedWorldGaussianV1 { .. } => {
                self.footprint_into(field.grid(), position, scratch)?;
                let energy = field.energy();
                let mut amount = 0.0;
                for &(cell, weight) in scratch.cells() {
                    let value =
                        energy
                            .get(cell)
                            .copied()
                            .ok_or_else(|| CoreError::LayoutMismatch {
                                buffer: "field",
                                detail: format!("exchange cell {cell} is not in this field"),
                            })?;
                    amount += weight * f64::from(value);
                }
                Ok(amount / field.grid().cell_area())
            }
        }
    }

    /// Local energy density `Σ w_c e_c / (h_x h_y)` at `position` on `field`.
    ///
    /// # Errors
    /// As [`ExchangeKernel::density_with_scratch`].
    pub fn density(self, field: &FieldState, position: [f64; 2]) -> Result<f64, CoreError> {
        let mut scratch = ExchangeScratch::new();
        self.density_with_scratch(field, position, &mut scratch)
    }
}

/// Upper bound on cell centers along one axis within distance `radius` of
/// any position (plus one extra cell against floating-point edge rounding).
fn axis_footprint_bound(radius: f64, cell: f64, cells: u32) -> usize {
    let whole = cells as usize;
    let span = (2.0 * radius / cell).floor();
    if span.is_finite() && span >= 0.0 && span < f64::from(cells) {
        (span as usize).saturating_add(2).min(whole)
    } else {
        whole
    }
}

/// Upper bound on candidate indices produced by [`axis_candidates`] before
/// the exact disk test (widened by one cell on each side against rounding).
fn axis_candidate_bound(radius: f64, cell: f64, cells: u32) -> usize {
    let whole = cells as usize;
    let span = (2.0 * radius / cell).ceil() + 5.0;
    if span.is_finite() && span >= 0.0 && span < f64::from(cells) {
        (span as usize).min(whole)
    } else {
        whole
    }
}

/// Candidate cell indices along one periodic axis, sorted and distinct:
/// every index whose center can lie within `radius` of `coordinate`.
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
    let low = ((coordinate - radius) / cell - 0.5).floor() - 1.0;
    let high = ((coordinate + radius) / cell - 0.5).ceil() + 1.0;
    let span = high - low + 1.0;
    if !span.is_finite() || span >= f64::from(cells) {
        out.extend(0..cells);
        return true;
    }
    let (low, high) = (low as i64, high as i64);
    let modulus = i64::from(cells);
    if low >= 0 && high < modulus {
        #[expect(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            reason = "low >= 0 and high < cells <= u32::MAX checked above"
        )]
        out.extend((low as u32)..=(high as u32));
        return true;
    }
    out.extend((low..=high).filter_map(|index| u32::try_from(index.rem_euclid(modulus)).ok()));
    out.sort_unstable();
    out.dedup();
    true
}

/// An organism's exchange footprint: at most four distinct cells in
/// ascending index order, each with a positive weight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExchangeStencil {
    cells: [usize; 4],
    weights: [f64; 4],
    len: usize,
}

impl ExchangeStencil {
    /// The `grid-cic-v1` footprint at `position`, which is wrapped onto the
    /// torus first.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite coordinate.
    pub fn at(grid: &FieldGrid, position: [f64; 2]) -> Result<Self, CoreError> {
        let [x, y] = grid.wrap_position(position)?;
        let (column, fx) = split(x / grid.cell_width() - 0.5);
        let (row, fy) = split(y / grid.cell_height() - 0.5);
        let cx_u32 = grid.cells_x();
        let cy_u32 = grid.cells_y();
        // Fast path for non-degenerate grids (`cells_x >= 2`, `cells_y >= 2`):
        // all four corners are guaranteed distinct and interior positions
        // (`x0 < x1`, `y0 < y1`, `fx > 0`, `fy > 0`, ~99%+ of calls) are
        // already in strictly ascending cell-index order with zero `rem_euclid`
        // divisions, dedup scans, or insertion sort.
        if cx_u32 >= 2 && cy_u32 >= 2 && usize::try_from(grid.cell_count()).is_ok() {
            let cx_i = i64::from(cx_u32);
            let cy_i = i64::from(cy_u32);
            if (-1..=cx_i).contains(&column) && (-1..=cy_i).contains(&row) {
                let cx = cx_u32 as usize;
                let cy = cy_u32 as usize;
                #[expect(
                    clippy::cast_sign_loss,
                    clippy::cast_possible_truncation,
                    reason = "column is checked in 0..cx_i <= u32::MAX"
                )]
                let x0 = if column < 0 {
                    cx - 1
                } else if column >= cx_i {
                    0
                } else {
                    column as usize
                };
                let x1 = if x0 + 1 < cx { x0 + 1 } else { 0 };
                #[expect(
                    clippy::cast_sign_loss,
                    clippy::cast_possible_truncation,
                    reason = "row is checked in 0..cy_i <= u32::MAX"
                )]
                let y0 = if row < 0 {
                    cy - 1
                } else if row >= cy_i {
                    0
                } else {
                    row as usize
                };
                let y1 = if y0 + 1 < cy { y0 + 1 } else { 0 };
                let one_minus_fx = 1.0 - fx;
                let one_minus_fy = 1.0 - fy;
                let w00 = one_minus_fx * one_minus_fy;
                let w10 = fx * one_minus_fy;
                let w01 = one_minus_fx * fy;
                let w11 = fx * fy;
                let row0 = y0 * cx;
                let row1 = y1 * cx;
                if x0 < x1 && y0 < y1 && w00 > 0.0 && w10 > 0.0 && w01 > 0.0 && w11 > 0.0 {
                    return Ok(Self {
                        cells: [row0 + x0, row0 + x1, row1 + x0, row1 + x1],
                        weights: [w00, w10, w01, w11],
                        len: 4,
                    });
                }
                // Edge-wrapped or zero-weight corner on a `>= 2x2` grid: all
                // corners remain distinct; arrange rows then columns in
                // ascending cell order without `rem_euclid` or dedup.
                let (xa, xb, wx_a, wx_b) = if x0 < x1 {
                    (x0, x1, one_minus_fx, fx)
                } else {
                    (x1, x0, fx, one_minus_fx)
                };
                let (ra, rb, wy_a, wy_b) = if y0 < y1 {
                    (row0, row1, one_minus_fy, fy)
                } else {
                    (row1, row0, fy, one_minus_fy)
                };
                let ordered = [
                    (ra + xa, wx_a * wy_a),
                    (ra + xb, wx_b * wy_a),
                    (rb + xa, wx_a * wy_b),
                    (rb + xb, wx_b * wy_b),
                ];
                let mut stencil = Self {
                    cells: [0; 4],
                    weights: [0.0; 4],
                    len: 0,
                };
                for (cell, weight) in ordered {
                    if weight != 0.0 {
                        stencil.cells[stencil.len] = cell;
                        stencil.weights[stencil.len] = weight;
                        stencil.len += 1;
                    }
                }
                return Ok(stencil);
            }
        }
        let corners = [
            (0, 0, (1.0 - fx) * (1.0 - fy)),
            (1, 0, fx * (1.0 - fy)),
            (0, 1, (1.0 - fx) * fy),
            (1, 1, fx * fy),
        ];
        let mut stencil = Self {
            cells: [0; 4],
            weights: [0.0; 4],
            len: 0,
        };
        for (dx, dy, weight) in corners {
            if weight == 0.0 {
                continue;
            }
            let (cx, cy) = grid.wrap_cell(column + dx, row + dy);
            let cell = grid
                .cell_index(cx, cy)
                .ok_or_else(|| CoreError::NumericalInvalid {
                    quantity: "exchange footprint",
                    detail: format!("cell ({cx}, {cy}) is outside the grid"),
                })?;
            let len = stencil.len;
            if let Some(slot) = stencil.cells[..len].iter().position(|&c| c == cell) {
                stencil.weights[slot] += weight;
            } else {
                stencil.cells[len] = cell;
                stencil.weights[len] = weight;
                stencil.len += 1;
            }
        }
        // Insertion sort by cell index: at most four entries.
        for i in 1..stencil.len {
            let mut j = i;
            while j > 0 && stencil.cells[j - 1] > stencil.cells[j] {
                stencil.cells.swap(j - 1, j);
                stencil.weights.swap(j - 1, j);
                j -= 1;
            }
        }
        Ok(stencil)
    }

    /// The footprint's cells, in ascending index order.
    #[must_use]
    pub fn cells(&self) -> &[usize] {
        &self.cells[..self.len]
    }

    /// The weight of each cell in [`ExchangeStencil::cells`].
    #[must_use]
    pub fn weights(&self) -> &[f64] {
        &self.weights[..self.len]
    }

    /// The footprint's local energy density `Σ w_c e_c / (h_x h_y)`, in
    /// ecological-energy per world-length².
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] when a footprint cell is not in
    /// `field`, which means the stencil was made for another grid.
    pub fn density(&self, field: &FieldState) -> Result<f64, CoreError> {
        let energy = field.energy();
        let mut amount = 0.0;
        for (&cell, &weight) in self.cells().iter().zip(self.weights()) {
            let value = energy
                .get(cell)
                .copied()
                .ok_or_else(|| CoreError::LayoutMismatch {
                    buffer: "field",
                    detail: format!("exchange cell {cell} is not in this field"),
                })?;
            amount += weight * f64::from(value);
        }
        Ok(amount / field.grid().cell_area())
    }
}

/// A.1's saturating request `d = Δt U_max r/(K + r)` for local density
/// `r`, uptake capacity `U_max` (ecological-energy per simulation-second)
/// and half-saturation density `K`. It is zero at zero density and below
/// `Δt U_max` always. It is a request: the grant resolver still limits what
/// is transferred.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] unless `U_max` and `r` are finite and
/// nonnegative and `K` is finite and positive.
pub fn saturating_request(
    step: TimeStep,
    capacity_per_second: f64,
    half_saturation_density: f64,
    density: f64,
) -> Result<f64, CoreError> {
    check_request(capacity_per_second, half_saturation_density, density)?;
    Ok(step.seconds_per_tick() * capacity_per_second * density
        / (half_saturation_density + density))
}

/// [`saturating_request`]'s checks alone, which a device computing the
/// request applies to its own density (FL-031).
///
/// # Errors
/// As [`saturating_request`].
pub fn check_request(
    capacity_per_second: f64,
    half_saturation_density: f64,
    density: f64,
) -> Result<(), CoreError> {
    for (quantity, value, positive) in [
        (
            "chemistry.uptake_capacity_per_second",
            capacity_per_second,
            false,
        ),
        (
            "chemistry.uptake_half_saturation_density",
            half_saturation_density,
            true,
        ),
        ("local energy density", density, false),
    ] {
        let admissible = value.is_finite() && if positive { value > 0.0 } else { value >= 0.0 };
        if !admissible {
            return Err(CoreError::NumericalInvalid {
                quantity,
                detail: format!("{value} is out of range"),
            });
        }
    }
    Ok(())
}

/// The floor of a continuous index and its fractional part.
pub(crate) fn split(continuous: f64) -> (i64, f64) {
    let floor = continuous.floor();
    // `continuous` lies in `[−½, G − ½]` with `G < 2^32`, so the floor is
    // an exact small integer.
    (floor as i64, continuous - floor)
}
