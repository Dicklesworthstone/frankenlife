//! Committed-field invariants INV-01 and INV-02 (plan §4.2, §11.5, §21.2).
//!
//! Every committed cell energy is finite and nonnegative, every moment
//! component is finite, and every cell satisfies `||m_c||₂ ≤ e_c` within the
//! declared cone tolerance. The cone residual is `max(0, ||m_c||₂ − e_c)`.
//! Arithmetic is F64 over the exact F32 values: squares are summed in channel
//! order, and the energy total is summed in cell-index order.

use flife_core::CoreError;

use crate::grid::FIELD_PLANES;
use crate::state::FieldState;
use std::ops::Range;

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
        let planes: Vec<&[f32]> = (0..FIELD_PLANES)
            .filter_map(|plane| self.plane(plane))
            .collect();
        let Some((energy, moments)) = planes.split_first() else {
            return Ok(FieldSummary {
                energy_total: 0.0,
                max_cone_residual: 0.0,
                max_cone_residual_cell: None,
                empty_cells: 0,
            });
        };
        let width = u64::from(grid.cells_x());
        let cells = energy.len();
        let ranges = workers.min(cells / CELLS_PER_RANGE).max(1);
        let check = |range: usize| {
            let start = cells / ranges * range + cells % ranges * range / ranges;
            let end = cells / ranges * (range + 1) + cells % ranges * (range + 1) / ranges;
            check_range(energy, moments, start..end, tolerance, width)
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
        for &cell_energy in energy.iter() {
            summary.energy_total += f64::from(cell_energy);
        }
        Ok(summary)
    }
}

/// Check cells `range` in cell-index order, stopping at the first offending
/// cell, and measure them.
fn check_range(
    energy: &[f32],
    moments: &[&[f32]],
    range: Range<usize>,
    tolerance: ConeTolerance,
    width: u64,
) -> Result<RangeSummary, CoreError> {
    let locate = |index: usize| {
        let index = u64::try_from(index).unwrap_or(u64::MAX);
        let x = u32::try_from(index % width).unwrap_or(u32::MAX);
        let y = u32::try_from(index / width).unwrap_or(u32::MAX);
        (x, y)
    };
    let mut summary = RangeSummary::default();
    let first = range.start;
    for (offset, &cell_energy) in energy.get(range).unwrap_or(&[]).iter().enumerate() {
        let index = first + offset;
        let e = f64::from(cell_energy);
        if !e.is_finite() || e < 0.0 {
            let (x, y) = locate(index);
            return Err(CoreError::NumericalInvalid {
                quantity: "field energy",
                detail: format!("cell ({x}, {y}) holds {cell_energy:?} (INV-01)"),
            });
        }
        let mut squares = 0.0_f64;
        for (channel, plane) in moments.iter().enumerate() {
            let m = plane.get(index).copied().unwrap_or(f32::NAN);
            if !m.is_finite() {
                let (x, y) = locate(index);
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
            let (x, y) = locate(index);
            return Err(CoreError::NumericalInvalid {
                quantity: "chemical moment cone",
                detail: format!(
                    "cell ({x}, {y}): ||m|| exceeds e = {e:?} by {residual:?} (INV-02)"
                ),
            });
        }
        if residual > summary.max_cone_residual {
            summary.max_cone_residual = residual;
            summary.max_cone_residual_cell = Some(locate(index));
        }
        if e == 0.0 {
            summary.empty_cells += 1;
        }
    }
    Ok(summary)
}
