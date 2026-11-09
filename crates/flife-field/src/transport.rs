//! `positive-five-point-v1` transport with `subcycle` stability and
//! exponential decay (plan §4.5, §11.5, Appendix C.6, Appendix D).
//!
//! Every plane, energy and each moment channel, gets the same periodic
//! five-point operator
//!
//! `u'_c = (1 − 2r_x − 2r_y) u_c + r_x (u_w + u_e) + r_y (u_s + u_n)`,
//!
//! with `r_x = D Δt / h_x²` and `r_y = D Δt / h_y²`. Its coefficients are
//! nonnegative, and they sum to one along both rows and columns. It
//! therefore keeps energy nonnegative, conserves the periodic total before
//! decay, and preserves `||m|| ≤ e` (Appendix D). When `r_x + r_y > 1/2`
//! the tick is split into the fewest equal substeps that keep the center
//! coefficient nonnegative. The substep count is recorded, and diffusion is
//! never clamped.
//!
//! Decay then multiplies every plane by `exp(−λΔt)` (see
//! [`crate::numerics::exp_neg`]), and the removed energy is charged to
//! dissipation.
//!
//! Reference arithmetic: inputs widen exactly to F64. Each output is
//! `((c·u + r_x·(u_w + u_e)) + r_y·(u_s + u_n))`, evaluated in that order
//! and rounded once to F32. Decay is `f32(f64(u) × factor)`.

use flife_core::{CoreError, TimeStep, ValidatedConfig};

use crate::footprint::BufferLimits;
use crate::grid::{ENERGY_PLANE, FIELD_PLANES, FieldGrid};
use crate::layout::{BUFFER, mismatch, overflow};
use crate::numerics::exp_neg;
use crate::state::FieldState;

/// The transport scheme this module implements.
pub const POSITIVE_FIVE_POINT_V1: &str = "positive-five-point-v1";

/// The stability policy this module implements.
pub const SUBCYCLE: &str = "subcycle";

/// The most substeps one tick may take. A configuration needing more is
/// refused rather than run at unbounded cost.
pub const MAX_SUBSTEPS: u32 = 1024;

/// The transport of one tick: its substeps, per-substep coefficients and
/// decay factor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportPlan {
    grid: FieldGrid,
    requested_r_x: f64,
    requested_r_y: f64,
    substeps: u32,
    r_x: f64,
    r_y: f64,
    center: f64,
    decay_factor: f64,
}

impl TransportPlan {
    /// The plan for diffusion `D` and decay rate `λ` at timestep `step` on
    /// `grid`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a negative or nonfinite `D` or
    /// `λ`, or a tick that would need more than [`MAX_SUBSTEPS`] substeps.
    pub fn new(
        grid: &FieldGrid,
        step: TimeStep,
        diffusion: f64,
        decay_per_second: f64,
    ) -> Result<Self, CoreError> {
        for (quantity, value) in [
            ("world.diffusion", diffusion),
            ("world.decay_per_second", decay_per_second),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(CoreError::NumericalInvalid {
                    quantity,
                    detail: format!("{value:?} is not finite and nonnegative"),
                });
            }
        }
        let (numerator, denominator) = (f64::from(step.numerator()), f64::from(step.denominator()));
        let ratio = |cell: f64| diffusion * numerator / denominator / (cell * cell);
        let requested_r_x = ratio(grid.cell_width());
        let requested_r_y = ratio(grid.cell_height());
        if !requested_r_x.is_finite() || !requested_r_y.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "world.diffusion",
                detail: format!(
                    "D = {diffusion:?} over {:?}×{:?} cells gives a nonfinite r",
                    grid.cell_width(),
                    grid.cell_height()
                ),
            });
        }

        // The fewest substeps whose center coefficient is nonnegative. The
        // estimate ⌈2(r_x + r_y)⌉ can be one short when rounding pushes the
        // center coefficient just below zero, so the check is on the
        // coefficient actually used.
        let estimate = (2.0 * (requested_r_x + requested_r_y)).ceil().max(1.0);
        let mut substeps = if estimate > f64::from(MAX_SUBSTEPS) {
            MAX_SUBSTEPS + 1
        } else {
            // A whole number in 1..=MAX_SUBSTEPS, so the conversion is exact.
            estimate as u32
        };
        loop {
            if substeps > MAX_SUBSTEPS {
                return Err(CoreError::NumericalInvalid {
                    quantity: "world.diffusion",
                    detail: format!(
                        "r_x + r_y = {:?} needs more than {MAX_SUBSTEPS} substeps per tick",
                        requested_r_x + requested_r_y
                    ),
                });
            }
            let count = f64::from(substeps);
            let (r_x, r_y) = (requested_r_x / count, requested_r_y / count);
            let center = 1.0 - 2.0 * r_x - 2.0 * r_y;
            if center >= 0.0 {
                let decay = decay_per_second * numerator / denominator;
                let decay_factor = exp_neg(decay).ok_or_else(|| CoreError::NumericalInvalid {
                    quantity: "world.decay_per_second",
                    detail: format!("λΔt = {decay:?} has no decay factor"),
                })?;
                return Ok(Self {
                    grid: *grid,
                    requested_r_x,
                    requested_r_y,
                    substeps,
                    r_x,
                    r_y,
                    center,
                    decay_factor,
                });
            }
            substeps += 1;
        }
    }

    /// The plan a validated configuration declares.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] for another scheme or stability
    /// policy, and the errors of [`TransportPlan::new`] and
    /// [`FieldGrid::from_config`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let world = &config.config().world;
        if world.transport_scheme != POSITIVE_FIVE_POINT_V1 {
            return Err(CoreError::MissingCapability {
                capability: format!("transport scheme `{}`", world.transport_scheme),
            });
        }
        if world.stability_policy != SUBCYCLE {
            return Err(CoreError::MissingCapability {
                capability: format!("stability policy `{}`", world.stability_policy),
            });
        }
        Self::new(
            &FieldGrid::from_config(config)?,
            config.report().timestep,
            world.diffusion,
            world.decay_per_second,
        )
    }

    /// The grid.
    #[must_use]
    pub const fn grid(&self) -> &FieldGrid {
        &self.grid
    }

    /// `(r_x, r_y)` over the whole tick, before subcycling.
    #[must_use]
    pub const fn requested_ratios(&self) -> (f64, f64) {
        (self.requested_r_x, self.requested_r_y)
    }

    /// Substeps per tick: 1 unless the requested ratios exceed the bound.
    #[must_use]
    pub const fn substeps(&self) -> u32 {
        self.substeps
    }

    /// `(r_x, r_y)` of one substep.
    #[must_use]
    pub const fn ratios(&self) -> (f64, f64) {
        (self.r_x, self.r_y)
    }

    /// The center coefficient `1 − 2r_x − 2r_y` of one substep, never
    /// negative.
    #[must_use]
    pub const fn center(&self) -> f64 {
        self.center
    }

    /// `exp(−λΔt)`.
    #[must_use]
    pub const fn decay_factor(&self) -> f64 {
        self.decay_factor
    }
}

/// Two reusable plane buffers for subcycling, allocated once and admitted
/// like any other buffer (§9.4).
#[derive(Debug, Clone, PartialEq)]
pub struct TransportScratch {
    cells: usize,
    first: Vec<f32>,
    second: Vec<f32>,
}

impl TransportScratch {
    /// Scratch for `grid`.
    ///
    /// # Errors
    /// [`CoreError::BudgetExhausted`] above `limits`;
    /// [`CoreError::AllocationFailed`] if the host refuses.
    pub fn new(grid: &FieldGrid, limits: &BufferLimits) -> Result<Self, CoreError> {
        let cells = usize::try_from(grid.cell_count()).map_err(|_| overflow())?;
        let bytes = grid.cell_count().checked_mul(4).ok_or_else(overflow)?;
        limits.admit("transport scratch bytes", bytes)?;
        let plane = || -> Result<Vec<f32>, CoreError> {
            let mut data = Vec::new();
            data.try_reserve_exact(cells)
                .map_err(|_| CoreError::AllocationFailed {
                    buffer: "transport scratch",
                    bytes,
                })?;
            data.resize(cells, 0.0);
            Ok(data)
        };
        Ok(Self {
            cells,
            first: plane()?,
            second: plane()?,
        })
    }
}

/// What one tick of transport did to the energy plane, summed in F64 in
/// cell order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportReport {
    /// Substeps taken.
    pub substeps: u32,
    /// `Σ e` of the committed field.
    pub energy_before: f64,
    /// `Σ e` after transport, before decay.
    pub energy_transported: f64,
    /// `Σ e` after decay.
    pub energy_after: f64,
    /// Energy removed by decay: `Σ_c (e_c − decayed e_c)`, charged to
    /// dissipation.
    pub decay_removed: f64,
    /// `energy_transported − energy_before`: the transport's numerical
    /// residual. It is zero in exact arithmetic.
    pub transport_residual: f64,
}

/// Transport and decay `current` into `next` (phase 2, §8.1). `next` is
/// overwritten. `current` is only read.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] if a state or the scratch does not belong
/// to the plan's grid.
pub fn transport(
    plan: &TransportPlan,
    current: &FieldState,
    next: &mut FieldState,
    scratch: &mut TransportScratch,
) -> Result<TransportReport, CoreError> {
    transport_on(plan, current, next, std::slice::from_mut(scratch))
}

/// [`transport`] with the planes split over up to `scratches.len()`
/// threads: this one and scoped workers it joins before returning (plan
/// §3.4's bounded reference adapter). Every plane is transported on its own,
/// so the split cannot change a value, and the energy plane's sums are taken
/// in cell order as before.
///
/// # Errors
/// As [`transport`], for any scratch; [`CoreError::StateConflict`] for none,
/// or a worker that panicked.
pub fn transport_on(
    plan: &TransportPlan,
    current: &FieldState,
    next: &mut FieldState,
    scratches: &mut [TransportScratch],
) -> Result<TransportReport, CoreError> {
    let expected = crate::layout::FieldLayout::planar(plan.grid)?;
    expected.ensure_compatible(current.layout())?;
    expected.ensure_compatible(next.layout())?;
    let cells = usize::try_from(plan.grid.cell_count()).map_err(|_| overflow())?;
    for scratch in scratches.iter() {
        if scratch.cells != scratch.first.len()
            || scratch.cells != scratch.second.len()
            || scratch.cells != cells
        {
            return Err(mismatch(format!(
                "transport scratch for {} cells on a grid of {}",
                scratch.cells,
                plan.grid.cell_count()
            )));
        }
    }
    let width = usize::try_from(plan.grid.cells_x()).map_err(|_| overflow())?;
    let height = usize::try_from(plan.grid.cells_y()).map_err(|_| overflow())?;
    let coefficients = Coefficients {
        center: plan.center,
        r_x: plan.r_x,
        r_y: plan.r_y,
        width,
        height,
    };
    let planes = usize::try_from(FIELD_PLANES).map_err(|_| overflow())?;
    let source = current.as_slice();
    let target = next.as_mut_slice();
    if cells == 0 || source.len() != cells * planes || target.len() != cells * planes {
        return Err(missing_plane());
    }

    let energy_before = sum(current.energy());
    let used = scratches.len().min(planes);
    let per = planes.div_ceil(used.max(1));
    let run = |first_plane: usize,
               source: &[f32],
               target: &mut [f32],
               scratch: &mut TransportScratch|
     -> Option<EnergyTerms> {
        let mut energy = None;
        for (offset, (source, target)) in source
            .chunks_exact(cells)
            .zip(target.chunks_exact_mut(cells))
            .enumerate()
        {
            let is_energy = first_plane + offset == ENERGY_PLANE as usize;
            if let Some(terms) =
                transport_plane(&coefficients, plan, source, target, scratch, is_energy)
            {
                energy = Some(terms);
            }
        }
        energy
    };
    let Some((head_scratch, others)) = scratches.split_first_mut() else {
        return Err(CoreError::StateConflict {
            operation: "transport",
            detail: "no transport scratch".to_owned(),
        });
    };
    let mut sources = source.chunks(per * cells);
    let mut targets = target.chunks_mut(per * cells);
    let (Some(head_source), Some(head_target)) = (sources.next(), targets.next()) else {
        return Err(missing_plane());
    };
    let run = &run;
    let energy = std::thread::scope(|scope| {
        let workers: Vec<_> = sources
            .zip(targets)
            .zip(others.iter_mut())
            .enumerate()
            .map(|(index, ((source, target), scratch))| {
                scope.spawn(move || run((index + 1) * per, source, target, scratch))
            })
            .collect();
        let mut energy = run(0, head_source, head_target, head_scratch);
        for worker in workers {
            match worker.join() {
                Ok(terms) => energy = energy.or(terms),
                Err(_) => {
                    return Err(CoreError::StateConflict {
                        operation: "transport",
                        detail: "a transport worker panicked".to_owned(),
                    });
                }
            }
        }
        energy.ok_or_else(missing_plane)
    })?;
    Ok(report(plan, energy_before, energy))
}

/// The energy plane's part of [`transport`], for an accelerator that
/// diffused every plane itself: `plane` is the energy plane after
/// diffusion and `energy_before` the committed field's [`sum`]. The plane is
/// decayed in place, and the report is the one [`transport`] gives.
#[must_use]
pub fn energy_report(
    plan: &TransportPlan,
    energy_before: f64,
    plane: &mut [f32],
) -> TransportReport {
    report(plan, energy_before, energy_terms(plan.decay_factor, plane))
}

fn report(plan: &TransportPlan, energy_before: f64, energy: EnergyTerms) -> TransportReport {
    TransportReport {
        substeps: plan.substeps,
        energy_before,
        energy_transported: energy.transported,
        energy_after: energy.after,
        decay_removed: energy.decay_removed,
        transport_residual: energy.transported - energy_before,
    }
}

/// One plane's sums, which only the energy plane reports.
#[derive(Debug, Clone, Copy)]
struct EnergyTerms {
    transported: f64,
    decay_removed: f64,
    after: f64,
}

/// Transport then decay one plane. The energy plane also reports its sums,
/// in cell order.
fn transport_plane(
    coefficients: &Coefficients,
    plan: &TransportPlan,
    source: &[f32],
    target: &mut [f32],
    scratch: &mut TransportScratch,
    is_energy: bool,
) -> Option<EnergyTerms> {
    diffuse(coefficients, plan.substeps, source, target, scratch);
    if is_energy {
        return Some(energy_terms(plan.decay_factor, target));
    }
    if plan.decay_factor != 1.0 {
        decay(target, plan.decay_factor);
    }
    None
}

/// Decay the diffused energy plane and take its sums, in cell order.
fn energy_terms(decay_factor: f64, plane: &mut [f32]) -> EnergyTerms {
    let transported = sum(plane);
    let decay_removed = if decay_factor == 1.0 {
        0.0
    } else {
        decay(plane, decay_factor)
    };
    EnergyTerms {
        transported,
        decay_removed,
        after: sum(plane),
    }
}

struct Coefficients {
    center: f64,
    r_x: f64,
    r_y: f64,
    width: usize,
    height: usize,
}

/// `substeps` applications of the stencil from `source` into `target`,
/// alternating through the scratch planes.
fn diffuse(
    coefficients: &Coefficients,
    substeps: u32,
    source: &[f32],
    target: &mut [f32],
    scratch: &mut TransportScratch,
) {
    if substeps <= 1 {
        stencil(coefficients, source, target);
        return;
    }
    stencil(coefficients, source, &mut scratch.first);
    let mut latest_in_first = true;
    for _ in 2..substeps {
        if latest_in_first {
            stencil(coefficients, &scratch.first, &mut scratch.second);
        } else {
            stencil(coefficients, &scratch.second, &mut scratch.first);
        }
        latest_in_first = !latest_in_first;
    }
    let last = if latest_in_first {
        &scratch.first
    } else {
        &scratch.second
    };
    stencil(coefficients, last, target);
}

/// One periodic five-point step in the reference arithmetic.
fn stencil(coefficients: &Coefficients, source: &[f32], target: &mut [f32]) {
    let Coefficients { width, height, .. } = *coefficients;
    let cells = width.checked_mul(height);
    match cells {
        Some(cells) if width > 0 && source.len() == cells && target.len() == cells => {
            stencil_rows(coefficients, source, target);
        }
        _ => stencil_checked(coefficients, source, target),
    }
}

/// [`stencil`] on whole planes, row by row. Each cell is the same
/// expression, evaluated in the same order, as in [`stencil_checked`]; the
/// row slices only let the interior compile to vector code.
fn stencil_rows(coefficients: &Coefficients, source: &[f32], target: &mut [f32]) {
    let Coefficients {
        center,
        r_x,
        r_y,
        width,
        height,
    } = *coefficients;
    let value = |c: f32, w: f32, e: f32, s: f32, n: f32| {
        // Round once to the F32 field dtype.
        (center * f64::from(c)
            + r_x * (f64::from(w) + f64::from(e))
            + r_y * (f64::from(s) + f64::from(n))) as f32
    };
    for (y, out) in target.chunks_exact_mut(width).enumerate() {
        let south = if y == 0 { height - 1 } else { y - 1 };
        let north = if y + 1 == height { 0 } else { y + 1 };
        let row = &source[y * width..][..width];
        let row_south = &source[south * width..][..width];
        let row_north = &source[north * width..][..width];
        let edge = |x: usize| {
            let west = if x == 0 { width - 1 } else { x - 1 };
            let east = if x + 1 == width { 0 } else { x + 1 };
            value(row[x], row[west], row[east], row_south[x], row_north[x])
        };
        out[0] = edge(0);
        if width > 1 {
            out[width - 1] = edge(width - 1);
        }
        if width > 2 {
            let inner = 1..width - 1;
            for (((((slot, &c), &w), &e), &s), &n) in out[inner.clone()]
                .iter_mut()
                .zip(&row[inner.clone()])
                .zip(&row[..width - 2])
                .zip(&row[2..])
                .zip(&row_south[inner.clone()])
                .zip(&row_north[inner])
            {
                *slot = value(c, w, e, s, n);
            }
        }
    }
}

/// [`stencil`] for planes of any length: a missing source cell reads as zero
/// and a missing target cell is skipped.
fn stencil_checked(coefficients: &Coefficients, source: &[f32], target: &mut [f32]) {
    let Coefficients {
        center,
        r_x,
        r_y,
        width,
        height,
    } = *coefficients;
    let at = |index: usize| source.get(index).copied().map_or(0.0, f64::from);
    for y in 0..height {
        let south = if y == 0 { height - 1 } else { y - 1 };
        let north = if y + 1 == height { 0 } else { y + 1 };
        let (row, row_south, row_north) = (y * width, south * width, north * width);
        for x in 0..width {
            let west = if x == 0 { width - 1 } else { x - 1 };
            let east = if x + 1 == width { 0 } else { x + 1 };
            let value = center * at(row + x)
                + r_x * (at(row + west) + at(row + east))
                + r_y * (at(row_south + x) + at(row_north + x));
            if let Some(slot) = target.get_mut(row + x) {
                // Round once to the F32 field dtype.
                *slot = value as f32;
            }
        }
    }
}

/// Multiply a plane by `factor`; returns `Σ (before − after)` in F64.
fn decay(plane: &mut [f32], factor: f64) -> f64 {
    let mut removed = 0.0;
    for value in plane.iter_mut() {
        let before = f64::from(*value);
        let after = (before * factor) as f32;
        removed += before - f64::from(after);
        *value = after;
    }
    removed
}

/// `Σ values` in F64, in index order.
/// `Σ values` in F64, in order: every energy total of the field step.
#[must_use]
pub fn sum(values: &[f32]) -> f64 {
    values
        .iter()
        .fold(0.0, |total, &value| total + f64::from(value))
}

fn missing_plane() -> CoreError {
    CoreError::LayoutMismatch {
        buffer: BUFFER,
        detail: "a field plane is missing".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Coefficients, stencil_checked, stencil_rows};

    /// A deterministic plane with every magnitude class the field holds.
    fn plane(cells: usize, seed: u64) -> Vec<f32> {
        let mut state = seed;
        (0..cells)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let bits = (state >> 40) as u32;
                let scale = [1e-30_f32, 1e-3, 1.0, 1e3, 3e7][(state >> 33) as usize % 5];
                (bits as f32 / 16_777_216.0 - 0.25) * scale
            })
            .collect()
    }

    #[test]
    fn the_row_stencil_is_the_reference_stencil_bit_for_bit() {
        for (width, height) in [(1, 1), (1, 4), (2, 1), (2, 3), (3, 3), (7, 5), (128, 128)] {
            let coefficients = Coefficients {
                center: 0.6,
                r_x: 0.1,
                r_y: 0.1,
                width,
                height,
            };
            let source = plane(width * height, (width * 31 + height) as u64);
            let mut fast = vec![f32::NAN; width * height];
            let mut checked = vec![f32::NAN; width * height];
            stencil_rows(&coefficients, &source, &mut fast);
            stencil_checked(&coefficients, &source, &mut checked);
            let fast: Vec<u32> = fast.iter().map(|v| v.to_bits()).collect();
            let checked: Vec<u32> = checked.iter().map(|v| v.to_bits()).collect();
            assert_eq!(fast, checked, "{width} x {height}");
        }
    }
}
