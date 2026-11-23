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

use std::ops::Range;

use flife_core::memory::capacity_bytes;
use flife_core::{CoreError, TimeStep, V2Feature, ValidatedConfig};

use crate::footprint::BufferLimits;
use crate::grid::{ENERGY_PLANE, FIELD_PLANES, FieldGrid};
use crate::layout::{BUFFER, mismatch, overflow};
use crate::numerics::exp_neg;
use crate::state::FieldState;

/// The transport scheme this module implements.
pub const POSITIVE_FIVE_POINT_V1: &str = "positive-five-point-v1";

/// The stability policy this module implements.
pub const SUBCYCLE: &str = "subcycle";

/// Harmonic face-conductance composition for `V2-09` (`tight-junctions`).
pub const HARMONIC_V1: &str = "harmonic-v1";

/// Minimum face-conductance composition for `V2-09` (`tight-junctions`).
pub const MIN_V1: &str = "min-v1";

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

    /// Bytes its two planes hold (FL-043).
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        capacity_bytes(&self.first).saturating_add(capacity_bytes(&self.second))
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

/// Face-conductance combination rule for `V2-09` (`tight-junctions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BarrierMode {
    /// Commutative harmonic mean `2 (φ_c φ_n) / (φ_c + φ_n)`.
    HarmonicV1,
    /// Minimum `min(φ_c, φ_n)`.
    MinV1,
}

impl BarrierMode {
    /// Parse a `v2.barrier.mode` identifier.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            HARMONIC_V1 => Some(Self::HarmonicV1),
            MIN_V1 => Some(Self::MinV1),
            _ => None,
        }
    }

    /// Canonical mode string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HarmonicV1 => HARMONIC_V1,
            Self::MinV1 => MIN_V1,
        }
    }
}

/// Passive organism-impeded symmetric face diffusion (`V2-09`, slug
/// `tight-junctions`, 100% pure field-mediated `INV-08`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TightJunctions {
    mode: BarrierMode,
    minimum_permeability: f64,
    occupancy_impedance: f64,
}

impl TightJunctions {
    /// Construct a tight-junction permeability rule.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless `minimum_permeability ∈ (0, 1]`
    /// and `occupancy_impedance ≥ 0` are finite.
    pub fn new(
        mode: BarrierMode,
        minimum_permeability: f64,
        occupancy_impedance: f64,
    ) -> Result<Self, CoreError> {
        if !minimum_permeability.is_finite()
            || minimum_permeability <= 0.0
            || minimum_permeability > 1.0
        {
            return Err(CoreError::NumericalInvalid {
                quantity: "v2.barrier.minimum_permeability",
                detail: format!("{minimum_permeability:?} is outside (0, 1]"),
            });
        }
        if !occupancy_impedance.is_finite() || occupancy_impedance < 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "v2.barrier.occupancy_impedance",
                detail: format!("{occupancy_impedance:?} is not finite and nonnegative"),
            });
        }
        Ok(Self {
            mode,
            minimum_permeability,
            occupancy_impedance,
        })
    }

    /// Resolve `[v2.barrier]` from a validated configuration: `None` on `v1`
    /// or when `V2Feature::TightJunctions` is disabled.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for an unrecognized barrier mode, or as
    /// [`TightJunctions::new`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Option<Self>, CoreError> {
        let Some(v2) = config.config().v2.as_ref() else {
            return Ok(None);
        };
        if !v2.is_enabled(V2Feature::TightJunctions) {
            return Ok(None);
        }
        let barrier = &v2.barrier;
        let mode =
            BarrierMode::parse(&barrier.mode).ok_or_else(|| CoreError::UnknownAlgorithm {
                role: "v2.barrier.mode",
                name: barrier.mode.clone(),
            })?;
        Self::new(
            mode,
            barrier.minimum_permeability,
            barrier.occupancy_impedance,
        )
        .map(Some)
    }

    /// The face-conductance combination mode.
    #[must_use]
    pub const fn mode(&self) -> BarrierMode {
        self.mode
    }

    /// Minimum permeability `φ_min ∈ (0, 1]`.
    #[must_use]
    pub const fn minimum_permeability(&self) -> f64 {
        self.minimum_permeability
    }

    /// Occupancy impedance `κ_tj ≥ 0`.
    #[must_use]
    pub const fn occupancy_impedance(&self) -> f64 {
        self.occupancy_impedance
    }

    /// Cell permeability `φ_c = φ_min + (1 − φ_min) / (1 + κ_tj · n_c)` in F64.
    #[must_use]
    #[inline]
    pub fn cell_permeability(&self, occupancy: u32) -> f64 {
        self.minimum_permeability
            + (1.0 - self.minimum_permeability)
                / (1.0 + self.occupancy_impedance * f64::from(occupancy))
    }

    /// Precompute symmetric east (`edge_x`) and north (`edge_y`) face
    /// conductances for `plan` from per-cell organism occupancy `occupancy`
    /// (`V2-09` single-pass outside the 33/34-plane loop), with mandatory
    /// upper clamp to `(r_x, r_y)` guaranteeing nonnegative center weights.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if `occupancy` or `edges` does not match
    /// `plan.grid()`.
    pub fn fill_edges(
        &self,
        plan: &TransportPlan,
        occupancy: &[u32],
        edges: &mut FluxEdges,
    ) -> Result<(), CoreError> {
        if edges.grid != plan.grid {
            return Err(mismatch(
                "flux edges belong to a different grid than the transport plan".into(),
            ));
        }
        let cells = usize::try_from(plan.grid.cell_count()).map_err(|_| overflow())?;
        if occupancy.len() != cells || edges.edge_x.len() != cells || edges.edge_y.len() != cells {
            return Err(mismatch(format!(
                "tight-junction buffers ({}, {}, {}) do not match {cells} grid cells",
                occupancy.len(),
                edges.edge_x.len(),
                edges.edge_y.len()
            )));
        }
        let width = usize::try_from(plan.grid.cells_x()).map_err(|_| overflow())?;
        let height = usize::try_from(plan.grid.cells_y()).map_err(|_| overflow())?;
        let (r_x, r_y) = plan.ratios();
        for y in 0..height {
            let row = y * width;
            let north_row = if y + 1 == height { 0 } else { (y + 1) * width };
            for x in 0..width {
                let east = if x + 1 == width { 0 } else { x + 1 };
                let c = row + x;
                let phi_c = self.cell_permeability(occupancy[c]);
                let phi_e = self.cell_permeability(occupancy[row + east]);
                let phi_n = self.cell_permeability(occupancy[north_row + x]);
                let (face_x, face_y) = match self.mode {
                    BarrierMode::HarmonicV1 => {
                        let fx = r_x * ((2.0 * (phi_c * phi_e)) / (phi_c + phi_e));
                        let fy = r_y * ((2.0 * (phi_c * phi_n)) / (phi_c + phi_n));
                        (fx, fy)
                    }
                    BarrierMode::MinV1 => (r_x * phi_c.min(phi_e), r_y * phi_c.min(phi_n)),
                };
                edges.edge_x[c] = face_x.min(r_x);
                edges.edge_y[c] = face_y.min(r_y);
            }
        }
        Ok(())
    }
}

/// Precomputed symmetric east and north face conductances (`edge_x`, `edge_y`)
/// for doubly-stochastic flux-form diffusion (`V2-09`, reusable by `V2-14`
/// vascular fields and `V2-15` dual-carrier trails).
///
/// `edge_x[c]` is the conductance across the east face of cell `c` (between
/// `(x, y)` and `((x + 1) mod W, y)`), and `edge_y[c]` is the conductance
/// across the north face of cell `c` (between `(x, y)` and `(x, (y + 1) mod H)`).
#[derive(Debug, Clone, PartialEq)]
pub struct FluxEdges {
    grid: FieldGrid,
    edge_x: Vec<f64>,
    edge_y: Vec<f64>,
}

impl FluxEdges {
    /// Allocate reusable `(edge_x, edge_y)` face-conductance buffers for
    /// `grid`, admitted against `limits` (FL-043 / INV-15).
    ///
    /// # Errors
    /// [`CoreError::BudgetExhausted`] above `limits`,
    /// [`CoreError::AllocationFailed`] if the host refuses, or
    /// [`CoreError::Overflow`].
    pub fn new(grid: &FieldGrid, limits: &BufferLimits) -> Result<Self, CoreError> {
        let cells = usize::try_from(grid.cell_count()).map_err(|_| overflow())?;
        let per_axis_bytes = grid.cell_count().checked_mul(8).ok_or_else(overflow)?;
        let total_bytes = per_axis_bytes.checked_mul(2).ok_or_else(overflow)?;
        limits.admit("flux edge conductance bytes", total_bytes)?;
        let mut edge_x = Vec::new();
        edge_x
            .try_reserve_exact(cells)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "flux east edge conductances",
                bytes: per_axis_bytes,
            })?;
        edge_x.resize(cells, 0.0);
        let mut edge_y = Vec::new();
        edge_y
            .try_reserve_exact(cells)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "flux north edge conductances",
                bytes: per_axis_bytes,
            })?;
        edge_y.resize(cells, 0.0);
        Ok(Self {
            grid: *grid,
            edge_x,
            edge_y,
        })
    }

    /// Bytes its two face-conductance buffers hold (FL-043).
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        capacity_bytes(&self.edge_x).saturating_add(capacity_bytes(&self.edge_y))
    }

    /// The grid these face conductances belong to.
    #[must_use]
    pub const fn grid(&self) -> &FieldGrid {
        &self.grid
    }

    /// East face conductances `r^E_c`, in row-major cell order.
    #[must_use]
    pub fn edge_x(&self) -> &[f64] {
        &self.edge_x
    }

    /// North face conductances `r^N_c`, in row-major cell order.
    #[must_use]
    pub fn edge_y(&self) -> &[f64] {
        &self.edge_y
    }

    /// Writable east face conductances (for `V2-14` vascular composition).
    pub fn edge_x_mut(&mut self) -> &mut [f64] {
        &mut self.edge_x
    }

    /// Writable north face conductances (for `V2-14` vascular composition).
    pub fn edge_y_mut(&mut self) -> &mut [f64] {
        &mut self.edge_y
    }
}

/// Exact integer per-cell organism occupancy `n_c` from world positions
/// (associative/commutative `INV-11`, shared by `V2-09` tight junctions and
/// `V2-07` `cell-occupancy-v1`), reusing `occupancy`'s allocation.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for a nonfinite position, or
/// [`CoreError::Overflow`] if the grid cell count exceeds `usize`.
pub fn count_cell_occupancy(
    grid: &FieldGrid,
    positions: &[[f64; 2]],
    occupancy: &mut Vec<u32>,
) -> Result<(), CoreError> {
    let cells = usize::try_from(grid.cell_count()).map_err(|_| overflow())?;
    occupancy.clear();
    occupancy.resize(cells, 0);
    let max_x = grid.cells_x().saturating_sub(1);
    let max_y = grid.cells_y().saturating_sub(1);
    let cell_w = grid.cell_width();
    let cell_h = grid.cell_height();
    for &position in positions {
        let [x, y] = grid.wrap_position(position)?;
        let cx = ((x / cell_w).floor() as u32).min(max_x);
        let cy = ((y / cell_h).floor() as u32).min(max_y);
        if let Some(index) = grid.cell_index(cx, cy)
            && let Some(slot) = occupancy.get_mut(index)
        {
            *slot = slot.saturating_add(1);
        }
    }
    Ok(())
}

/// Flux-form doubly-stochastic transport and decay of `current` into `next`
/// using precomputed symmetric face conductances `edges` (`V2-09`).
///
/// # Errors
/// As [`transport_flux_on`].
pub fn transport_flux(
    plan: &TransportPlan,
    edges: &FluxEdges,
    current: &FieldState,
    next: &mut FieldState,
    scratch: &mut TransportScratch,
) -> Result<TransportReport, CoreError> {
    transport_flux_on(plan, edges, current, next, std::slice::from_mut(scratch))
}

/// Flux-form doubly-stochastic transport and decay across all `FIELD_PLANES`
/// planes split over up to `scratches.len()` threads (`V2-09`).
///
/// # Errors
/// As [`transport_flux_range_on`].
pub fn transport_flux_on(
    plan: &TransportPlan,
    edges: &FluxEdges,
    current: &FieldState,
    next: &mut FieldState,
    scratches: &mut [TransportScratch],
) -> Result<TransportReport, CoreError> {
    let planes = usize::try_from(FIELD_PLANES).map_err(|_| overflow())?;
    transport_flux_range_on(
        plan,
        edges,
        current,
        next,
        0..planes,
        Some(ENERGY_PLANE as usize),
        scratches,
    )?
    .ok_or_else(missing_plane)
}

fn validate_flux_inputs(
    plan: &TransportPlan,
    edges: &FluxEdges,
    current: &FieldState,
    next: &FieldState,
    plane_range: &Range<usize>,
    scratches: &[TransportScratch],
) -> Result<usize, CoreError> {
    if *current.grid() != plan.grid || *next.grid() != plan.grid || edges.grid != plan.grid {
        return Err(mismatch(
            "flux transport inputs do not share the transport plan's grid".into(),
        ));
    }
    let cells = usize::try_from(plan.grid.cell_count()).map_err(|_| overflow())?;
    if cells == 0 {
        return Err(missing_plane());
    }
    let source_all = current.as_slice();
    let target_all = next.as_slice();
    if source_all.len() != target_all.len() || !source_all.len().is_multiple_of(cells) {
        return Err(missing_plane());
    }
    let total_planes = source_all.len() / cells;
    if plane_range.start > plane_range.end || plane_range.end > total_planes {
        return Err(mismatch(format!(
            "plane range {plane_range:?} is outside 0..{total_planes}"
        )));
    }
    if edges.edge_x.len() != cells || edges.edge_y.len() != cells {
        return Err(mismatch(format!(
            "flux edges ({}, {}) do not match {cells} grid cells",
            edges.edge_x.len(),
            edges.edge_y.len()
        )));
    }
    let (r_x, r_y) = plan.ratios();
    for (axis, values, bound) in [
        ("flux edge_x", &edges.edge_x, r_x),
        ("flux edge_y", &edges.edge_y, r_y),
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
    for scratch in scratches {
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
    Ok(cells)
}

/// Flux-form doubly-stochastic transport and decay over `plane_range`
/// (reusable by `V2-09` tight junctions, `V2-14` vascular fields, and `V2-15`
/// 34-plane dual-carrier splits).
///
/// Evaluates every cell in the exact IEEE-754 order shared with
/// `field.metal::field_stencil_flux`:
/// `center = (1.0 - (r_w + r_e)) - (r_s + r_n)`
/// `next = ((center * u_c + (r_w * u_w + r_e * u_e)) + (r_s * u_s + r_n * u_n)) as f32`.
///
/// Returns `Some(TransportReport)` when `energy_plane` lies inside
/// `plane_range`, else `None`.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] for mismatched grids, buffers, or an
/// out-of-range `plane_range`; [`CoreError::NumericalInvalid`] if any edge
/// conductance is nonfinite, negative, or exceeds `(r_x, r_y)`;
/// [`CoreError::StateConflict`] for empty `scratches` or a panicked worker.
pub fn transport_flux_range_on(
    plan: &TransportPlan,
    edges: &FluxEdges,
    current: &FieldState,
    next: &mut FieldState,
    plane_range: Range<usize>,
    energy_plane: Option<usize>,
    scratches: &mut [TransportScratch],
) -> Result<Option<TransportReport>, CoreError> {
    let cells = validate_flux_inputs(plan, edges, current, next, &plane_range, scratches)?;
    let planes = plane_range.len();
    if planes == 0 {
        return Ok(None);
    }
    let source_all = current.as_slice();
    let target_all = next.as_mut_slice();
    let first_plane_index = plane_range.start;
    let slice_range = first_plane_index * cells..plane_range.end * cells;
    let source = &source_all[slice_range.clone()];
    let target = &mut target_all[slice_range];
    let energy_before = energy_plane
        .filter(|p| plane_range.contains(p))
        .map(|p| sum(&source_all[p * cells..(p + 1) * cells]));

    let width = usize::try_from(plan.grid.cells_x()).map_err(|_| overflow())?;
    let height = usize::try_from(plan.grid.cells_y()).map_err(|_| overflow())?;
    let flux = FluxCoefficients {
        edge_x: &edges.edge_x,
        edge_y: &edges.edge_y,
        width,
        height,
    };
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
            let is_energy = energy_plane == Some(first_plane + offset);
            if let Some(terms) =
                transport_flux_plane(&flux, plan, source, target, scratch, is_energy)
            {
                energy = Some(terms);
            }
        }
        energy
    };
    let Some((head_scratch, others)) = scratches.split_first_mut() else {
        return Err(CoreError::StateConflict {
            operation: "flux transport",
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
                scope.spawn(move || {
                    run(
                        first_plane_index + (index + 1) * per,
                        source,
                        target,
                        scratch,
                    )
                })
            })
            .collect();
        let mut energy = run(first_plane_index, head_source, head_target, head_scratch);
        for worker in workers {
            match worker.join() {
                Ok(terms) => energy = energy.or(terms),
                Err(_) => {
                    return Err(CoreError::StateConflict {
                        operation: "flux transport",
                        detail: "a transport worker panicked".to_owned(),
                    });
                }
            }
        }
        Ok(energy)
    })?;
    Ok(energy_before.and_then(|before| energy.map(|terms| report(plan, before, terms))))
}

struct FluxCoefficients<'a> {
    edge_x: &'a [f64],
    edge_y: &'a [f64],
    width: usize,
    height: usize,
}

fn transport_flux_plane(
    flux: &FluxCoefficients<'_>,
    plan: &TransportPlan,
    source: &[f32],
    target: &mut [f32],
    scratch: &mut TransportScratch,
    is_energy: bool,
) -> Option<EnergyTerms> {
    diffuse_flux(flux, plan.substeps, source, target, scratch);
    if is_energy {
        return Some(energy_terms(plan.decay_factor, target));
    }
    if plan.decay_factor != 1.0 {
        decay(target, plan.decay_factor);
    }
    None
}

fn diffuse_flux(
    flux: &FluxCoefficients<'_>,
    substeps: u32,
    source: &[f32],
    target: &mut [f32],
    scratch: &mut TransportScratch,
) {
    if substeps <= 1 {
        stencil_flux(flux, source, target);
        return;
    }
    stencil_flux(flux, source, &mut scratch.first);
    let mut latest_in_first = true;
    for _ in 2..substeps {
        if latest_in_first {
            stencil_flux(flux, &scratch.first, &mut scratch.second);
        } else {
            stencil_flux(flux, &scratch.second, &mut scratch.first);
        }
        latest_in_first = !latest_in_first;
    }
    let last = if latest_in_first {
        &scratch.first
    } else {
        &scratch.second
    };
    stencil_flux(flux, last, target);
}

#[inline]
fn flux_cell(values: [f32; 5], rates: [f64; 4]) -> f32 {
    let [u_c, u_w, u_e, u_s, u_n] = values;
    let [r_w, r_e, r_s, r_n] = rates;
    let center = (1.0 - (r_w + r_e)) - (r_s + r_n);
    ((center * f64::from(u_c) + (r_w * f64::from(u_w) + r_e * f64::from(u_e)))
        + (r_s * f64::from(u_s) + r_n * f64::from(u_n))) as f32
}

fn stencil_flux(flux: &FluxCoefficients<'_>, source: &[f32], target: &mut [f32]) {
    let FluxCoefficients {
        edge_x,
        edge_y,
        width,
        height,
    } = *flux;
    match width.checked_mul(height) {
        Some(cells)
            if width > 0
                && source.len() == cells
                && target.len() == cells
                && edge_x.len() == cells
                && edge_y.len() == cells =>
        {
            stencil_flux_rows(flux, source, target);
        }
        _ => stencil_flux_checked(flux, source, target),
    }
}

fn stencil_flux_rows(flux: &FluxCoefficients<'_>, source: &[f32], target: &mut [f32]) {
    let FluxCoefficients {
        edge_x,
        edge_y,
        width,
        height,
    } = *flux;
    for (y, out) in target.chunks_exact_mut(width).enumerate() {
        let south = if y == 0 { height - 1 } else { y - 1 };
        let north = if y + 1 == height { 0 } else { y + 1 };
        let row = &source[y * width..][..width];
        let row_south = &source[south * width..][..width];
        let row_north = &source[north * width..][..width];
        let ex_row = &edge_x[y * width..][..width];
        let ey_row = &edge_y[y * width..][..width];
        let ey_south = &edge_y[south * width..][..width];
        let edge = |x: usize| {
            let west = if x == 0 { width - 1 } else { x - 1 };
            let east = if x + 1 == width { 0 } else { x + 1 };
            flux_cell(
                [row[x], row[west], row[east], row_south[x], row_north[x]],
                [ex_row[west], ex_row[x], ey_south[x], ey_row[x]],
            )
        };
        out[0] = edge(0);
        if width > 1 {
            out[width - 1] = edge(width - 1);
        }
        if width > 2 {
            let inner = 1..width - 1;
            for x in inner {
                out[x] = flux_cell(
                    [row[x], row[x - 1], row[x + 1], row_south[x], row_north[x]],
                    [ex_row[x - 1], ex_row[x], ey_south[x], ey_row[x]],
                );
            }
        }
    }
}

fn stencil_flux_checked(flux: &FluxCoefficients<'_>, source: &[f32], target: &mut [f32]) {
    let FluxCoefficients {
        edge_x,
        edge_y,
        width,
        height,
    } = *flux;
    let at_f32 = |index: usize| source.get(index).copied().unwrap_or(0.0);
    let at_ex = |index: usize| edge_x.get(index).copied().unwrap_or(0.0);
    let at_ey = |index: usize| edge_y.get(index).copied().unwrap_or(0.0);
    for y in 0..height {
        let south = if y == 0 { height - 1 } else { y - 1 };
        let north = if y + 1 == height { 0 } else { y + 1 };
        let (row, row_south, row_north) = (y * width, south * width, north * width);
        for x in 0..width {
            let west = if x == 0 { width - 1 } else { x - 1 };
            let east = if x + 1 == width { 0 } else { x + 1 };
            let value = flux_cell(
                [
                    at_f32(row + x),
                    at_f32(row + west),
                    at_f32(row + east),
                    at_f32(row_south + x),
                    at_f32(row_north + x),
                ],
                [
                    at_ex(row + west),
                    at_ex(row + x),
                    at_ey(row_south + x),
                    at_ey(row + x),
                ],
            );
            if let Some(slot) = target.get_mut(row + x) {
                *slot = value;
            }
        }
    }
}

fn missing_plane() -> CoreError {
    CoreError::LayoutMismatch {
        buffer: BUFFER,
        detail: "a field plane is missing".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Coefficients, FluxCoefficients, stencil_checked, stencil_flux_checked, stencil_flux_rows,
        stencil_rows,
    };

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

    #[test]
    fn the_flux_row_stencil_is_the_checked_stencil_bit_for_bit() {
        for (width, height) in [(1, 1), (1, 4), (2, 1), (2, 3), (3, 3), (7, 5), (32, 32)] {
            let cells = width * height;
            let source = plane(cells, (width * 41 + height) as u64);
            let edge_x: Vec<f64> = (0..cells)
                .map(|c| 0.01 + 0.14 * ((c % 7) as f64 / 6.0))
                .collect();
            let edge_y: Vec<f64> = (0..cells)
                .map(|c| 0.01 + 0.14 * (((c * 3) % 5) as f64 / 4.0))
                .collect();
            let flux = FluxCoefficients {
                edge_x: &edge_x,
                edge_y: &edge_y,
                width,
                height,
            };
            let mut fast = vec![f32::NAN; cells];
            let mut checked = vec![f32::NAN; cells];
            stencil_flux_rows(&flux, &source, &mut fast);
            stencil_flux_checked(&flux, &source, &mut checked);
            let fast: Vec<u32> = fast.iter().map(|v| v.to_bits()).collect();
            let checked: Vec<u32> = checked.iter().map(|v| v.to_bits()).collect();
            assert_eq!(fast, checked, "flux {width} x {height}");
        }
    }
}
