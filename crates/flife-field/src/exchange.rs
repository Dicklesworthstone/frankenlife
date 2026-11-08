//! The `grid-cic-v1` exchange footprint and the A.1 uptake request law
//! (plan §4.3, §5.1, A.1).
//!
//! Consumption is local. An organism at `x` exchanges energy only with the
//! cells of its bilinear cloud-in-cell footprint. The continuous cell index
//! is `(x/h_x − ½, y/h_y − ½)`. Its floor selects the lower-left cell and
//! its fractional part `(f_x, f_y)` gives the four weights
//! `(1−f_x)(1−f_y)`, `f_x(1−f_y)`, `(1−f_x)f_y` and `f_x f_y`, wrapped
//! periodically. A cell named twice on a grid one cell wide is combined
//! into one entry, and zero weights are dropped. The weights are
//! nonnegative and sum to one up to F64 rounding. A long-range sensor
//! therefore never becomes long-range feeding.

use flife_core::{CoreError, TimeStep};

use crate::grid::FieldGrid;
use crate::state::FieldState;

/// The exchange kernel this module implements.
pub const GRID_CIC_V1: &str = "grid-cic-v1";

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
    Ok(step.seconds_per_tick() * capacity_per_second * density
        / (half_saturation_density + density))
}

/// The floor of a continuous index and its fractional part.
pub(crate) fn split(continuous: f64) -> (i64, f64) {
    let floor = continuous.floor();
    // `continuous` lies in `[−½, G − ½]` with `G < 2^32`, so the floor is
    // an exact small integer.
    (floor as i64, continuous - floor)
}
