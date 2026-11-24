//! Committed-field invariants INV-01 and INV-02 (plan §4.2, §11.5, §21.2).
//!
//! Every committed cell energy is finite and nonnegative, every moment
//! component is finite, and every cell satisfies `||m_c||₂ ≤ e_c` within the
//! declared cone tolerance. The cone residual is `max(0, ||m_c||₂ − e_c)`.
//! Arithmetic is F64 over the exact F32 values: squares are summed in channel
//! order, and the energy total is summed in cell-index order.

use std::ops::Range;

use flife_core::CoreError;

use crate::grid::{
    CARRIER0_MOMENT_CHANNELS, CARRIER1_ENERGY_PLANE, CARRIER1_FIRST_MOMENT_PLANE,
    CARRIER1_MOMENT_CHANNELS,
};
use crate::layout::BUFFER;
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

/// What a bounded, non-energy-creating moment contraction (plan §11.5)
/// resolved after `INV-01` and `INV-02` passed.
///
/// Energy planes (`e_0` and optional `e_1`) are never mutated; only moment
/// vectors whose cone residual exceeded the contraction trigger are scaled
/// radially inward onto the closed F32 cone (`||m_c||₂ ≤ e_c`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ConeContractionSummary {
    /// Carrier-cell pairs whose moment vector was contracted (in 34-plane
    /// dual-carrier mode, Carrier 0 and Carrier 1 are counted independently).
    pub cells: u64,
    /// Largest norm contraction `||m_before||₂ − ||m_after||₂` across
    /// contracted cells, in cell energy units.
    pub max_contraction: f64,
    /// Largest relative contraction `contraction / e_c` across contracted
    /// cells with `e_c > 0` (`0.0` when none had positive energy).
    pub max_relative: f64,
}

/// The fewest cells one thread of [`FieldState::check_invariants_on`]
/// checks: a smaller range costs more to hand to a thread than it saves.
const CELLS_PER_RANGE: usize = 4_096;

/// What one range of cells measured: its largest residual and the first of
/// its cells with that residual, when it is positive, its empty cells, and
/// whether any cell exceeded the contraction trigger.
#[derive(Debug, Clone, Copy, Default)]
struct RangeSummary {
    max_cone_residual: f64,
    max_cone_residual_cell: Option<(u32, u32)>,
    empty_cells: u64,
    any_exceeds_trigger: bool,
}

/// Carrier-0 moment slices plus optional Carrier-1 `(energy, moments)` slices.
type CarrierMoments<'a> = (&'a [&'a [f32]], Option<(&'a [f32], &'a [&'a [f32]])>);

fn split_carrier_moments<'a>(dual: bool, moments: &'a [&'a [f32]]) -> CarrierMoments<'a> {
    if dual {
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
    }
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
        self.check_invariants_with_trigger_on(tolerance, tolerance, workers)
            .map(|(summary, _)| summary)
    }

    /// [`FieldState::check_invariants_on`] that also reports whether any cell
    /// (on either carrier in dual-carrier mode) strictly exceeded `trigger`
    /// while remaining within `tolerance`.
    ///
    /// Computed in the same single read-only pass as `check_invariants_on`
    /// with zero extra memory reads, so Phase 10 can skip
    /// [`FieldState::contract_cone_on`] on ticks where no cell exceeds
    /// `trigger`.
    ///
    /// # Errors
    /// As [`FieldState::check_invariants_on`].
    pub fn check_invariants_with_trigger_on(
        &self,
        tolerance: ConeTolerance,
        trigger: ConeTolerance,
        workers: usize,
    ) -> Result<(FieldSummary, bool), CoreError> {
        let grid = *self.grid();
        let planes = self.plane_slices();
        let Some((energy, moments)) = planes.split_first() else {
            return Ok((
                FieldSummary {
                    energy_total: 0.0,
                    max_cone_residual: 0.0,
                    max_cone_residual_cell: None,
                    empty_cells: 0,
                },
                false,
            ));
        };
        let (c0_moments, c1) = split_carrier_moments(self.is_dual_carrier(), moments);
        let width = u64::from(grid.cells_x());
        let cells = energy.len();
        let ranges = if workers <= 1 {
            1
        } else {
            workers
                .saturating_mul(4)
                .min(64)
                .min(cells / CELLS_PER_RANGE)
                .max(1)
        };
        let bounds = CheckBounds { tolerance, trigger };
        let check = |range: usize| {
            let start = cells / ranges * range + cells % ranges * range / ranges;
            let end = cells / ranges * (range + 1) + cells % ranges * (range + 1) / ranges;
            match c1 {
                None => check_range(energy, c0_moments, start..end, bounds, width),
                Some((e1, m1)) => {
                    check_range_dual(energy, c0_moments, e1, m1, start..end, bounds, width)
                }
            }
        };
        let check = &check;
        let mut summary = FieldSummary {
            energy_total: 0.0,
            max_cone_residual: 0.0,
            max_cone_residual_cell: None,
            empty_cells: 0,
        };
        let mut any_exceeds_trigger = false;
        if ranges <= 1 {
            let range = check(0)?;
            summary.max_cone_residual = range.max_cone_residual;
            summary.max_cone_residual_cell = range.max_cone_residual_cell;
            summary.empty_cells = range.empty_cells;
            any_exceeds_trigger = range.any_exceeds_trigger;
        } else {
            let mut checked: Vec<Option<Result<RangeSummary, CoreError>>> =
                (0..ranges).map(|_| None).collect();
            flife_core::pool::parallel_tasks(
                workers,
                "check field invariants",
                &mut checked,
                |range, slot| {
                    *slot = Some(check(range));
                },
            )?;
            for part in checked {
                let range = part.ok_or_else(|| CoreError::StateConflict {
                    operation: "check field invariants",
                    detail: "an invariant worker returned no result".to_owned(),
                })??;
                if range.max_cone_residual > summary.max_cone_residual {
                    summary.max_cone_residual = range.max_cone_residual;
                    summary.max_cone_residual_cell = range.max_cone_residual_cell;
                }
                summary.empty_cells += range.empty_cells;
                any_exceeds_trigger |= range.any_exceeds_trigger;
            }
        }
        summary.energy_total = self.energy_total();
        Ok((summary, any_exceeds_trigger))
    }

    /// Plan §11.5 bounded, non-energy-creating moment contraction on one
    /// thread: contracts every cell whose cone residual strictly exceeds
    /// `trigger` radially inward onto the closed F32 cone (`||m_c||₂ ≤ e_c`)
    /// without modifying any energy plane (`e_0` or `e_1`).
    ///
    /// # Errors
    /// As [`FieldState::contract_cone_on`].
    pub fn contract_cone(
        &mut self,
        trigger: ConeTolerance,
    ) -> Result<ConeContractionSummary, CoreError> {
        self.contract_cone_on(trigger, 1)
    }

    /// [`FieldState::contract_cone`] on up to `workers` threads (zero is taken
    /// as one). Cells are partitioned into disjoint contiguous ranges; each
    /// cell's contraction depends only on its own F32 energy and moments, so
    /// the resulting field and [`ConeContractionSummary`] are bit-identical at
    /// every worker count. Handles both 33-plane single-carrier and 34-plane
    /// dual-carrier (`V2-15`) fields, contracting Carrier 0 and Carrier 1
    /// cones independently while keeping energy slices strictly immutable.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if the backing slice does not match the
    /// field's grid and carrier plane layout, [`CoreError::NumericalInvalid`]
    /// if a cell holds nonfinite/negative energy or a nonfinite moment, or
    /// [`CoreError::StateConflict`] if a worker panicked.
    pub fn contract_cone_on(
        &mut self,
        trigger: ConeTolerance,
        workers: usize,
    ) -> Result<ConeContractionSummary, CoreError> {
        let cells = self.energy().len();
        let total_planes = self.planes() as usize;
        if cells == 0 || total_planes <= 1 {
            return Ok(ConeContractionSummary::default());
        }
        let width = u64::from(self.grid().cells_x());
        let dual = self.is_dual_carrier();
        let values = self.as_mut_slice();
        if dual {
            let c0_count = CARRIER0_MOMENT_CHANNELS as usize;
            let c1_count = CARRIER1_MOMENT_CHANNELS as usize;
            let (e0, rest) = cut_mut(values, cells)?;
            let (m0_all, c1_all) = cut_mut(rest, c0_count * cells)?;
            let (e1, m1_all) = cut_mut(c1_all, cells)?;
            if m1_all.len() != c1_count * cells {
                return Err(CoreError::LayoutMismatch {
                    buffer: BUFFER,
                    detail: format!(
                        "dual-carrier trail moment slice holds {} values, expected {}",
                        m1_all.len(),
                        c1_count * cells
                    ),
                });
            }
            let s0 = contract_carrier_on(
                e0,
                m0_all,
                CarrierShape {
                    cells,
                    moment_count: c0_count,
                    width,
                },
                trigger,
                workers,
            )?;
            let s1 = contract_carrier_on(
                e1,
                m1_all,
                CarrierShape {
                    cells,
                    moment_count: c1_count,
                    width,
                },
                trigger,
                workers,
            )?;
            Ok(merge_contraction(s0, s1))
        } else {
            let c0_count = total_planes - 1;
            let (e0, m0_all) = cut_mut(values, cells)?;
            if m0_all.len() != c0_count * cells {
                return Err(CoreError::LayoutMismatch {
                    buffer: BUFFER,
                    detail: format!(
                        "field moment slice holds {} values, expected {}",
                        m0_all.len(),
                        c0_count * cells
                    ),
                });
            }
            contract_carrier_on(
                e0,
                m0_all,
                CarrierShape {
                    cells,
                    moment_count: c0_count,
                    width,
                },
                trigger,
                workers,
            )
        }
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
        let (c0_moments, c1) = split_carrier_moments(self.is_dual_carrier(), moments);
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
        let (c0_moments, c1) = split_carrier_moments(self.is_dual_carrier(), moments);
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

/// The hard INV-02 tolerance alongside the plan §11.5 contraction trigger
/// checked in the same single pass.
#[derive(Debug, Clone, Copy)]
struct CheckBounds {
    tolerance: ConeTolerance,
    trigger: ConeTolerance,
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
/// stopping at the first offending cell across either carrier and recording
/// whether either carrier's residual strictly exceeded `bounds.trigger`.
fn check_range_dual(
    e0: &[f32],
    m0: &[&[f32]],
    e1: &[f32],
    m1: &[&[f32]],
    range: Range<usize>,
    bounds: CheckBounds,
    width: u64,
) -> Result<RangeSummary, CoreError> {
    let mut summary = RangeSummary::default();
    let end = range.end.min(e0.len());
    for index in range.start..end {
        let r0 = check_cell(e0, m0, index, bounds.tolerance, width)?;
        let r1 = check_cell(e1, m1, index, bounds.tolerance, width)?;
        let e0_val = e0.get(index).copied().map_or(0.0, f64::from);
        let e1_val = e1.get(index).copied().map_or(0.0, f64::from);
        if !bounds.trigger.allows(r0, e0_val) || !bounds.trigger.allows(r1, e1_val) {
            summary.any_exceeds_trigger = true;
        }
        let residual = r0.max(r1);
        if residual > summary.max_cone_residual {
            summary.max_cone_residual = residual;
            summary.max_cone_residual_cell = Some(locate(index, width));
        }
        if e0_val == 0.0 {
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
/// cell, and measure them (including whether any cell strictly exceeded
/// `bounds.trigger`, at zero extra memory reads).
fn check_range(
    energy: &[f32],
    moments: &[&[f32]],
    range: Range<usize>,
    bounds: CheckBounds,
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
                let residual = check_cell(energy, moments, index, bounds.tolerance, width)?;
                let e = energy.get(index).copied().map_or(0.0, f64::from);
                if !bounds.trigger.allows(residual, e) {
                    summary.any_exceeds_trigger = true;
                }
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
                let residual = check_cell(energy, moments, index, bounds.tolerance, width)?;
                if !bounds.trigger.allows(residual, e) {
                    summary.any_exceeds_trigger = true;
                }
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
            if !bounds.tolerance.allows(residual, e) {
                // Fall back to `check_cell` from this exact cell to produce
                // the identical error detail.
                let _ = check_cell(energy, moments, base + offset, bounds.tolerance, width)?;
            }
            if !bounds.trigger.allows(residual, e) {
                summary.any_exceeds_trigger = true;
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

/// Merge two disjoint [`ConeContractionSummary`] values.
const fn merge_contraction(
    a: ConeContractionSummary,
    b: ConeContractionSummary,
) -> ConeContractionSummary {
    ConeContractionSummary {
        cells: a.cells + b.cells,
        max_contraction: if b.max_contraction > a.max_contraction {
            b.max_contraction
        } else {
            a.max_contraction
        },
        max_relative: if b.max_relative > a.max_relative {
            b.max_relative
        } else {
            a.max_relative
        },
    }
}

/// Split `slice` at `count`, refusing a short buffer cleanly without panicking.
fn cut_mut(slice: &mut [f32], count: usize) -> Result<(&mut [f32], &mut [f32]), CoreError> {
    slice
        .split_at_mut_checked(count)
        .ok_or_else(|| CoreError::LayoutMismatch {
            buffer: BUFFER,
            detail: format!("field buffer is shorter than {count} elements"),
        })
}

/// Round a finite F64 toward zero to F32: the F32 value of largest magnitude
/// at or below `|value|`, preserving sign (canonical `+0.0` for zero).
/// Never underflows bit patterns and clamps F32 overflow to `±f32::MAX`.
#[inline]
fn round_toward_zero_f32(value: f64) -> f32 {
    if value == 0.0 {
        return 0.0;
    }
    let nearest = value as f32;
    if !nearest.is_finite() {
        return if value.is_sign_negative() {
            -f32::MAX
        } else {
            f32::MAX
        };
    }
    if f64::from(nearest).abs() > value.abs() {
        // `|nearest| > |value| >= 0` guarantees `nearest != 0.0`, so its
        // magnitude bits are at least `1` and subtracting `1` steps one ULP
        // toward zero without altering the sign bit.
        let stepped = f32::from_bits(nearest.to_bits() - 1);
        if stepped == 0.0 { 0.0 } else { stepped }
    } else if nearest == 0.0 {
        0.0
    } else {
        nearest
    }
}

/// One worker's disjoint cell range for [`contract_carrier_on`]: immutable
/// energy slice plus mutable per-channel moment slices over the same range.
struct ContractionLane<'a> {
    start: usize,
    energy: &'a [f32],
    moments: Vec<&'a mut [f32]>,
}

/// Grid dimensions needed for error reporting and plane chunking inside
/// [`contract_carrier_on`].
#[derive(Clone, Copy)]
struct CarrierShape {
    cells: usize,
    moment_count: usize,
    width: u64,
}

/// Contract one carrier's moments across up to `workers` scoped threads while
/// holding its energy plane strictly immutable.
fn contract_carrier_on(
    energy: &[f32],
    moments_flat: &mut [f32],
    shape: CarrierShape,
    trigger: ConeTolerance,
    workers: usize,
) -> Result<ConeContractionSummary, CoreError> {
    let cells = shape.cells;
    if cells == 0 || shape.moment_count == 0 {
        return Ok(ConeContractionSummary::default());
    }
    let ranges = workers.min(cells / CELLS_PER_RANGE).max(1);
    let mut lanes = Vec::with_capacity(ranges);
    for range in 0..ranges {
        let start = cells / ranges * range + cells % ranges * range / ranges;
        let end = cells / ranges * (range + 1) + cells % ranges * (range + 1) / ranges;
        let e_slice = energy
            .get(start..end)
            .ok_or_else(|| CoreError::LayoutMismatch {
                buffer: BUFFER,
                detail: format!("energy slice out of bounds for range {start}..{end}"),
            })?;
        lanes.push(ContractionLane {
            start,
            energy: e_slice,
            moments: Vec::with_capacity(shape.moment_count),
        });
    }
    for mut plane in moments_flat.chunks_exact_mut(cells) {
        for lane in &mut lanes {
            let (head, tail) = cut_mut(plane, lane.energy.len())?;
            plane = tail;
            lane.moments.push(head);
        }
    }
    let width = shape.width;
    let run = |lane: &mut ContractionLane<'_>| contract_lane(lane, trigger, width);
    let run = &run;
    let mut total = ConeContractionSummary::default();
    if ranges <= 1 {
        if let Some(head) = lanes.first_mut() {
            total = run(head)?;
        }
    } else {
        let mut results: Vec<Result<ConeContractionSummary, CoreError>> = (0..lanes.len())
            .map(|_| Ok(ConeContractionSummary::default()))
            .collect();
        flife_core::pool::scope_workers(workers, "contract field cone", |scope| {
            let mut pairs = lanes.iter_mut().zip(results.iter_mut());
            let head = pairs.next();
            for (lane, slot) in pairs {
                scope.spawn(move || {
                    *slot = run(lane);
                });
            }
            if let Some((lane, slot)) = head {
                *slot = run(lane);
            }
        })?;
        for part in results {
            total = merge_contraction(total, part?);
        }
    }
    Ok(total)
}

/// Contract every cell in `lane` whose cone residual strictly exceeds
/// `trigger`, using the same cache-friendly 64-cell plane-major block scan as
/// [`check_range`]. Leaves energy untouched.
fn contract_lane(
    lane: &mut ContractionLane<'_>,
    trigger: ConeTolerance,
    width: u64,
) -> Result<ConeContractionSummary, CoreError> {
    let mut summary = ConeContractionSummary::default();
    let total_cells = lane.energy.len();
    let mut base = 0_usize;
    while base < total_cells {
        let len = (total_cells - base).min(CHECK_BLOCK);
        let energy_block = lane.energy.get(base..base + len).unwrap_or(&[]);
        let mut energies = [0.0_f64; CHECK_BLOCK];
        for (offset, (slot, &cell_energy)) in energies.iter_mut().zip(energy_block).enumerate() {
            let e = f64::from(cell_energy);
            if !e.is_finite() || e < 0.0 {
                let (x, y) = locate(lane.start + base + offset, width);
                return Err(CoreError::NumericalInvalid {
                    quantity: "field energy",
                    detail: format!("cell ({x}, {y}) holds {cell_energy:?} (INV-01)"),
                });
            }
            *slot = e;
        }
        let mut squares = [0.0_f64; CHECK_BLOCK];
        for (channel, plane) in lane.moments.iter().enumerate() {
            let slice = plane.get(base..base + len).unwrap_or(&[]);
            for (offset, (square, &m_raw)) in squares.iter_mut().zip(slice).enumerate() {
                if !m_raw.is_finite() {
                    let (x, y) = locate(lane.start + base + offset, width);
                    return Err(CoreError::NumericalInvalid {
                        quantity: "field moment",
                        detail: format!(
                            "cell ({x}, {y}) channel {channel} holds {m_raw:?} (INV-01)"
                        ),
                    });
                }
                let m = f64::from(m_raw);
                *square += m * m;
            }
        }
        for (offset, (&e, &sq)) in energies.iter().zip(&squares).enumerate().take(len) {
            if sq == 0.0 {
                continue;
            }
            let norm = sq.sqrt();
            let residual = (norm - e).max(0.0);
            if trigger.allows(residual, e) {
                continue;
            }
            let local = base + offset;
            if e == 0.0 {
                for plane in &mut lane.moments {
                    if let Some(slot) = plane.get_mut(local) {
                        *slot = 0.0;
                    }
                }
                summary.cells += 1;
                if norm > summary.max_contraction {
                    summary.max_contraction = norm;
                }
            } else {
                // `scale` pulls the F64 vector strictly inside the sphere of
                // radius `e` by `64 * f64::EPSILON`, and rounding every
                // component toward zero to F32 guarantees `sqrt(Σ m'_k²) ≤ e`
                // in exact channel-order F64 arithmetic across all binades.
                let scale = (e / norm) * (1.0 - 64.0 * f64::EPSILON);
                let mut after_sq = 0.0_f64;
                for plane in &mut lane.moments {
                    if let Some(slot) = plane.get_mut(local) {
                        let scaled = round_toward_zero_f32(f64::from(*slot) * scale);
                        *slot = scaled;
                        let v = f64::from(scaled);
                        after_sq += v * v;
                    }
                }
                let contraction = (norm - after_sq.sqrt()).max(0.0);
                let relative = contraction / e;
                summary.cells += 1;
                if contraction > summary.max_contraction {
                    summary.max_contraction = contraction;
                }
                if relative > summary.max_relative {
                    summary.max_relative = relative;
                }
            }
        }
        base += len;
    }
    Ok(summary)
}
