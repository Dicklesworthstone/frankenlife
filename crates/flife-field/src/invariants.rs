//! Committed-field invariants INV-01 and INV-02 (plan §4.2, §11.5, §21.2).
//!
//! Every committed cell energy is finite and nonnegative, every moment
//! component is finite, and every cell satisfies `||m_c||₂ ≤ e_c` within the
//! declared cone tolerance. The cone residual is `max(0, ||m_c||₂ − e_c)`.
//! Arithmetic is F64 over the exact F32 values: squares are summed in channel
//! order, and the energy total is summed in cell-index order.

use std::ops::Range;

use flife_core::CoreError;

use crate::grid::{CARRIER0_MOMENT_CHANNELS, CARRIER1_ENERGY_PLANE, CARRIER1_FIRST_MOMENT_PLANE};
use crate::state::FieldState;

/// The cone residual a committed cell may carry:
/// `residual ≤ absolute + relative × e_c`. It is part of a declared
/// numerical profile, never a silent correction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConeTolerance {
    absolute: f64,
    relative: f64,
}

impl ConeTolerance {
    /// No residual at all, for exact fixtures.
    pub const EXACT: Self = Self {
        absolute: 0.0,
        relative: 0.0,
    };

    /// A tolerance of `absolute` energy plus `relative` times the cell's
    /// energy.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless both are finite and
    /// nonnegative.
    pub fn new(absolute: f64, relative: f64) -> Result<Self, CoreError> {
        for (quantity, value) in [
            ("absolute cone tolerance", absolute),
            ("relative cone tolerance", relative),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(CoreError::NumericalInvalid {
                    quantity,
                    detail: format!("{value:?} is not finite and nonnegative"),
                });
            }
        }
        Ok(Self { absolute, relative })
    }

    /// The absolute part.
    #[must_use]
    pub const fn absolute(&self) -> f64 {
        self.absolute
    }

    /// The part relative to the cell's energy.
    #[must_use]
    pub const fn relative(&self) -> f64 {
        self.relative
    }

    /// Whether `residual` is admissible in a cell holding `energy`.
    #[must_use]
    pub fn allows(&self, residual: f64, energy: f64) -> bool {
        residual <= self.absolute + self.relative * energy
    }
}

/// What a passing invariant check measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FieldSummary {
    /// `Σ_c e_c`, summed in F64 in cell-index order.
    pub energy_total: f64,
    /// The largest cone residual.
    pub max_cone_residual: f64,
    /// The first cell with that residual, when it is positive.
    pub max_cone_residual_cell: Option<(u32, u32)>,
    /// Cells with zero energy.
    pub empty_cells: u64,
}

/// The fewest cells one thread of [`FieldState::check_invariants_on`]
/// checks: a smaller range costs more to hand to a thread than it saves.
const CELLS_PER_RANGE: usize = 4_096;

/// What one range of cells measured: its largest residual and the first of
/// its cells with that residual, when it is positive, and its empty cells.
#[derive(Debug, Clone, Copy, Default)]
struct RangeSummary {
    max_cone_residual: f64,
    max_cone_residual_cell: Option<(u32, u32)>,
    empty_cells: u64,
}

impl FieldState {
    /// Check INV-01 and INV-02 on this field.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] naming the first offending cell in
    /// cell-index order: nonfinite or negative energy, a nonfinite moment,
    /// or a cone residual beyond `tolerance`.
    pub fn check_invariants(&self, tolerance: ConeTolerance) -> Result<FieldSummary, CoreError> {
        self.check_invariants_on(tolerance, 1)
    }

    /// [`FieldState::check_invariants`] on up to `workers` threads: this one
    /// and scoped workers it joins before returning. The cells are split into
    /// contiguous ranges of at least 4,096 cells. Each thread checks its
    /// range in cell-index order and stops at its first offending cell, so
    /// the first range with an error holds the first offending cell. A later
    /// range's largest residual replaces an earlier one's only when it is
    /// strictly larger, which keeps the first cell on a tie. The energy total
    /// is summed on this thread in cell-index order. The summary and the
    /// error are the same for every worker count. Zero workers is taken as
    /// one.
    ///
    /// # Errors
    /// As [`FieldState::check_invariants`]; [`CoreError::StateConflict`] for
    /// a worker that panicked.
    pub fn check_invariants_on(
        &self,
        tolerance: ConeTolerance,
        workers: usize,
    ) -> Result<FieldSummary, CoreError> {
        let grid = *self.grid();
        let planes = self.plane_slices();
        let Some((energy, moments)) = planes.split_first() else {
            return Ok(FieldSummary {
                energy_total: 0.0,
                max_cone_residual: 0.0,
                max_cone_residual_cell: None,
                empty_cells: 0,
            });
        };
        let dual = self.is_dual_carrier();
        let (c0_moments, c1): (&[&[f32]], Option<(&[f32], &[&[f32]])>) = if dual {
            let c0_end = CARRIER0_MOMENT_CHANNELS as usize;
            let c1_e_idx = (CARRIER1_ENERGY_PLANE - 1) as usize;
            let c1_m_start = (CARRIER1_FIRST_MOMENT_PLANE - 1) as usize;
            (
                moments.get(..c0_end).unwrap_or(&[]),
                moments
                    .get(c1_e_idx)
                    .map(|&e1| (e1, moments.get(c1_m_start..).unwrap_or(&[]))),
            )
        } else {
            (moments, None)
        };
        let width = u64::from(grid.cells_x());
        let cells = energy.len();
        let ranges = workers.min(cells / CELLS_PER_RANGE).max(1);
        let check = |range: usize| {
            let start = cells / ranges * range + cells % ranges * range / ranges;
            let end = cells / ranges * (range + 1) + cells % ranges * (range + 1) / ranges;
            match c1 {
                None => check_range(energy, c0_moments, start..end, tolerance, width),
                Some((e1, m1)) => {
                    check_range_dual(energy, c0_moments, e1, m1, start..end, tolerance, width)
                }
            }
        };
        let check = &check;
        let checked = std::thread::scope(|scope| {
            let workers: Vec<_> = (1..ranges)
                .map(|range| scope.spawn(move || check(range)))
                .collect();
            let mut checked = Vec::with_capacity(ranges);
            checked.push(check(0));
            for worker in workers {
                checked.push(worker.join().map_err(|_| CoreError::StateConflict {
                    operation: "check field invariants",
                    detail: "an invariant worker panicked".to_owned(),
                })?);
            }
            Ok::<_, CoreError>(checked)
        })?;
        let mut summary = FieldSummary {
            energy_total: 0.0,
            max_cone_residual: 0.0,
            max_cone_residual_cell: None,
            empty_cells: 0,
        };
        for range in checked {
            let range = range?;
            if range.max_cone_residual > summary.max_cone_residual {
                summary.max_cone_residual = range.max_cone_residual;
                summary.max_cone_residual_cell = range.max_cone_residual_cell;
            }
            summary.empty_cells += range.empty_cells;
        }
        summary.energy_total = self.energy_total();
        Ok(summary)
    }
}

/// What a scan of INV-01 and INV-02 found (FL-032): a backend's phase 10
/// check of a field, which [`FieldState::scan_invariants`] gives on the host
/// and [`FieldState::summarize_scan`] turns into the [`FieldSummary`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FieldScan {
    /// The first cell, in cell-index order, that fails INV-01 or INV-02.
    pub offending: Option<usize>,
    /// When no cell fails, the largest cone residual and the first cell
    /// holding it, if it is positive; `None` when a cell fails.
    pub largest: Option<(usize, f64)>,
}

impl FieldState {
    /// The field's planes as slices: plane 0 first, then the remaining planes.
    fn plane_slices(&self) -> Vec<&[f32]> {
        (0..self.planes())
            .filter_map(|plane| self.plane(plane))
            .collect()
    }

    /// The reference's scan for a backend's phase 10 (FL-032): each cell in
    /// cell-index order through [`FieldState::check_invariants`]'s
    /// arithmetic, stopping at the first that fails, else the first cell
    /// with the largest positive cone residual. In 34-plane dual-carrier mode
    /// (`V2-15`), checks BOTH Carrier 0 and Carrier 1 cones independently.
    #[must_use]
    pub fn scan_invariants(&self, tolerance: ConeTolerance) -> FieldScan {
        let planes = self.plane_slices();
        let mut scan = FieldScan {
            offending: None,
            largest: None,
        };
        let Some((energy, moments)) = planes.split_first() else {
            return scan;
        };
        let dual = self.is_dual_carrier();
        let (c0_moments, c1): (&[&[f32]], Option<(&[f32], &[&[f32]])>) = if dual {
            let c0_end = CARRIER0_MOMENT_CHANNELS as usize;
            let c1_e_idx = (CARRIER1_ENERGY_PLANE - 1) as usize;
            let c1_m_start = (CARRIER1_FIRST_MOMENT_PLANE - 1) as usize;
            (
                moments.get(..c0_end).unwrap_or(&[]),
                moments
                    .get(c1_e_idx)
                    .map(|&e1| (e1, moments.get(c1_m_start..).unwrap_or(&[]))),
            )
        } else {
            (moments, None)
        };
        let width = u64::from(self.grid().cells_x());
        for index in 0..energy.len() {
            let checked = match c1 {
                None => check_cell(energy, c0_moments, index, tolerance, width),
                Some((e1, m1)) => {
                    check_cell_dual(energy, c0_moments, e1, m1, index, tolerance, width)
                }
            };
            match checked {
                Err(_) => {
                    return FieldScan {
                        offending: Some(index),
                        largest: None,
                    };
                }
                Ok(residual) => {
                    if residual > scan.largest.map_or(0.0, |(_, largest)| largest) {
                        scan.largest = Some((index, residual));
                    }
                }
            }
        }
        scan
    }

    /// The [`FieldSummary`] [`FieldState::check_invariants`] gives, from a
    /// backend's `scan` of this field at `tolerance` (FL-032): its largest
    /// residual, which the host recomputes for its cell and requires to be
    /// the same bits, with the energy total summed in cell-index order and
    /// the empty cells counted on the host. `None` when the scan names a
    /// failing cell: [`FieldState::check_invariants_on`] then names the
    /// error.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when the scan cannot be this field's: a
    /// cell outside the field, a largest residual that is not positive or
    /// not the host's own for its cell, or a cell the host finds failing
    /// (the largest, or any cell's energy).
    pub fn summarize_scan(
        &self,
        tolerance: ConeTolerance,
        scan: &FieldScan,
    ) -> Result<Option<FieldSummary>, CoreError> {
        let conflict = |detail: String| CoreError::StateConflict {
            operation: "field check backend",
            detail,
        };
        let planes = self.plane_slices();
        let mut summary = FieldSummary {
            energy_total: 0.0,
            max_cone_residual: 0.0,
            max_cone_residual_cell: None,
            empty_cells: 0,
        };
        let Some((energy, moments)) = planes.split_first() else {
            return Ok(Some(summary));
        };
        let dual = self.is_dual_carrier();
        let (c0_moments, c1): (&[&[f32]], Option<(&[f32], &[&[f32]])>) = if dual {
            let c0_end = CARRIER0_MOMENT_CHANNELS as usize;
            let c1_e_idx = (CARRIER1_ENERGY_PLANE - 1) as usize;
            let c1_m_start = (CARRIER1_FIRST_MOMENT_PLANE - 1) as usize;
            (
                moments.get(..c0_end).unwrap_or(&[]),
                moments
                    .get(c1_e_idx)
                    .map(|&e1| (e1, moments.get(c1_m_start..).unwrap_or(&[]))),
            )
        } else {
            (moments, None)
        };
        let width = u64::from(self.grid().cells_x());
        if let Some(index) = scan.offending {
            if index >= energy.len() {
                return Err(conflict(format!(
                    "failing cell {index} of {}",
                    energy.len()
                )));
            }
            return Ok(None);
        }
        if let Some((index, residual)) = scan.largest {
            let own = match c1 {
                None => check_cell(energy, c0_moments, index, tolerance, width),
                Some((e1, m1)) => {
                    check_cell_dual(energy, c0_moments, e1, m1, index, tolerance, width)
                }
            }
            .map_err(|error| conflict(format!("its largest cell {index}: {error}")))?;
            if own <= 0.0 || own.to_bits() != residual.to_bits() {
                return Err(conflict(format!(
                    "cell {index}'s residual is {own:e}, not {residual:e}"
                )));
            }
            summary.max_cone_residual = residual;
            summary.max_cone_residual_cell = Some(locate(index, width));
        }
        let mut total_c0 = 0.0_f64;
        for (index, &cell_energy) in energy.iter().enumerate() {
            let e = f64::from(cell_energy);
            if !e.is_finite() || e < 0.0 {
                return Err(conflict(format!("cell {index} holds {cell_energy:?}")));
            }
            total_c0 += e;
            if e == 0.0 {
                summary.empty_cells += 1;
            }
        }
        if let Some((e1, _)) = c1 {
            let mut total_c1 = 0.0_f64;
            for (index, &cell_energy) in e1.iter().enumerate() {
                let e = f64::from(cell_energy);
                if !e.is_finite() || e < 0.0 {
                    return Err(conflict(format!(
                        "carrier 1 cell {index} holds {cell_energy:?}"
                    )));
                }
                total_c1 += e;
            }
            summary.energy_total = total_c0 + total_c1;
        } else {
            summary.energy_total = total_c0;
        }
        Ok(Some(summary))
    }
}

/// Cell `index`'s coordinates on a grid `width` cells wide.
fn locate(index: usize, width: u64) -> (u32, u32) {
    let index = u64::try_from(index).unwrap_or(u64::MAX);
    let x = u32::try_from(index % width).unwrap_or(u32::MAX);
    let y = u32::try_from(index / width).unwrap_or(u32::MAX);
    (x, y)
}

/// Check cell `index`, INV-01 then INV-02 at `tolerance`, and give its cone
/// residual: `max(0, ||m||₂ − e)`, the squares summed in channel order.
fn check_cell(
    energy: &[f32],
    moments: &[&[f32]],
    index: usize,
    tolerance: ConeTolerance,
    width: u64,
) -> Result<f64, CoreError> {
    let cell_energy = energy.get(index).copied().unwrap_or(f32::NAN);
    let e = f64::from(cell_energy);
    if !e.is_finite() || e < 0.0 {
        let (x, y) = locate(index, width);
        return Err(CoreError::NumericalInvalid {
            quantity: "field energy",
            detail: format!("cell ({x}, {y}) holds {cell_energy:?} (INV-01)"),
        });
    }
    let mut squares = 0.0_f64;
    for (channel, plane) in moments.iter().enumerate() {
        let m = plane.get(index).copied().unwrap_or(f32::NAN);
        if !m.is_finite() {
            let (x, y) = locate(index, width);
            return Err(CoreError::NumericalInvalid {
                quantity: "field moment",
                detail: format!("cell ({x}, {y}) channel {channel} holds {m:?} (INV-01)"),
            });
        }
        let m = f64::from(m);
        squares += m * m;
    }
    let residual = (squares.sqrt() - e).max(0.0);
    if !tolerance.allows(residual, e) {
        let (x, y) = locate(index, width);
        return Err(CoreError::NumericalInvalid {
            quantity: "chemical moment cone",
            detail: format!("cell ({x}, {y}): ||m|| exceeds e = {e:?} by {residual:?} (INV-02)"),
        });
    }
    Ok(residual)
}

/// Check cell `index` on a 34-plane dual-carrier field (`V2-15`): Carrier 0
/// (`e_0`, `m^(0)`) then Carrier 1 (`e_1`, `m^(1)`), returning the larger
/// cone residual.
fn check_cell_dual(
    e0: &[f32],
    m0: &[&[f32]],
    e1: &[f32],
    m1: &[&[f32]],
    index: usize,
    tolerance: ConeTolerance,
    width: u64,
) -> Result<f64, CoreError> {
    let r0 = check_cell(e0, m0, index, tolerance, width)?;
    let r1 = check_cell(e1, m1, index, tolerance, width)?;
    Ok(r0.max(r1))
}

/// Check cells `range` on a 34-plane dual-carrier field in cell-index order,
/// stopping at the first offending cell across either carrier.
fn check_range_dual(
    e0: &[f32],
    m0: &[&[f32]],
    e1: &[f32],
    m1: &[&[f32]],
    range: Range<usize>,
    tolerance: ConeTolerance,
    width: u64,
) -> Result<RangeSummary, CoreError> {
    let mut summary = RangeSummary::default();
    let end = range.end.min(e0.len());
    for index in range.start..end {
        let residual = check_cell_dual(e0, m0, e1, m1, index, tolerance, width)?;
        if residual > summary.max_cone_residual {
            summary.max_cone_residual = residual;
            summary.max_cone_residual_cell = Some(locate(index, width));
        }
        if e0.get(index).is_some_and(|&v| f64::from(v) == 0.0) {
            summary.empty_cells += 1;
        }
    }
    Ok(summary)
}

/// Cells checked per cache-friendly plane-major block in [`check_range`].
/// Accumulating squares plane by plane over 64 consecutive cells eliminates
/// 33-way L1D cache-set conflict misses on power-of-two grids (e.g., 512²
/// where each plane is 1 MiB) while preserving exact channel-order sums.
const CHECK_BLOCK: usize = 64;

/// Check cells `range` in cell-index order, stopping at the first offending
/// cell, and measure them.
fn check_range(
    energy: &[f32],
    moments: &[&[f32]],
    range: Range<usize>,
    tolerance: ConeTolerance,
    width: u64,
) -> Result<RangeSummary, CoreError> {
    let mut summary = RangeSummary::default();
    let end = range.end.min(energy.len());
    let mut base = range.start;
    while base < end {
        let len = (end - base).min(CHECK_BLOCK);
        let energy_block = energy.get(base..base + len).unwrap_or(&[]);
        let mut bad_energy = energy_block.len() != len;
        let mut energies = [0.0_f64; CHECK_BLOCK];
        for (slot, &cell_energy) in energies.iter_mut().zip(energy_block) {
            let e = f64::from(cell_energy);
            if !e.is_finite() || e < 0.0 {
                bad_energy = true;
                break;
            }
            *slot = e;
        }
        if bad_energy {
            for offset in 0..len {
                let index = base + offset;
                let residual = check_cell(energy, moments, index, tolerance, width)?;
                if residual > summary.max_cone_residual {
                    summary.max_cone_residual = residual;
                    summary.max_cone_residual_cell = Some(locate(index, width));
                }
                if energy.get(index).is_some_and(|&v| f64::from(v) == 0.0) {
                    summary.empty_cells += 1;
                }
            }
            base += len;
            continue;
        }

        let mut squares = [0.0_f64; CHECK_BLOCK];
        let mut bad_moment = false;
        for plane in moments {
            let Some(slice) = plane.get(base..base + len) else {
                bad_moment = true;
                break;
            };
            for (square, &m_raw) in squares.iter_mut().zip(slice) {
                if !m_raw.is_finite() {
                    bad_moment = true;
                }
                let m = f64::from(m_raw);
                *square += m * m;
            }
        }
        if bad_moment {
            for (offset, &e) in energies.iter().enumerate().take(len) {
                let index = base + offset;
                let residual = check_cell(energy, moments, index, tolerance, width)?;
                if residual > summary.max_cone_residual {
                    summary.max_cone_residual = residual;
                    summary.max_cone_residual_cell = Some(locate(index, width));
                }
                if e == 0.0 {
                    summary.empty_cells += 1;
                }
            }
            base += len;
            continue;
        }

        for (offset, (&e, &sq)) in energies.iter().zip(&squares).enumerate().take(len) {
            let residual = if sq == 0.0 {
                0.0
            } else {
                (sq.sqrt() - e).max(0.0)
            };
            if !tolerance.allows(residual, e) {
                // Fall back to `check_cell` from this exact cell to produce
                // the identical error detail.
                let _ = check_cell(energy, moments, base + offset, tolerance, width)?;
            }
            if residual > summary.max_cone_residual {
                summary.max_cone_residual = residual;
                summary.max_cone_residual_cell = Some(locate(base + offset, width));
            }
            if e == 0.0 {
                summary.empty_cells += 1;
            }
        }
        base += len;
    }
    Ok(summary)
}
