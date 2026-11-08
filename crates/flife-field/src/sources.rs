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
use flife_core::{CoreError, TickSpan, TimeStep, ValidatedConfig};

use crate::grid::{FieldGrid, wrap_coordinate};
use crate::layout::overflow;
use crate::numerics::exp_neg;
use crate::state::{CHANNELS, FieldState};

/// The spatial normalization this module implements.
pub const FIXED_TOTAL_POWER: &str = "fixed-total-power";

/// The source kernel this module implements.
pub const TRUNCATED_GAUSSIAN_V1: &str = "normalized-truncated-minimum-image-gaussian-v1";

/// The chemical-direction policy this module implements: a source's
/// direction never changes.
pub const FIXED_PER_SOURCE: &str = "fixed-per-source";

/// A normalized truncated minimum-image Gaussian of width `σ`, cut off at
/// `cutoff_sigma × σ`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceKernel {
    sigma: f64,
    radius: f64,
}

impl SourceKernel {
    /// A kernel of width `sigma` (world-length) truncated at
    /// `cutoff_sigma × sigma`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless both are finite and positive.
    pub fn new(sigma: f64, cutoff_sigma: f64) -> Result<Self, CoreError> {
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
        Ok(Self { sigma, radius })
    }

    /// The kernel a validated configuration declares.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] for another kernel, normalization or
    /// chemical-direction policy, and the errors of [`SourceKernel::new`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let sources = &config.config().sources;
        for (role, name, implemented) in [
            ("source kernel", &sources.kernel, TRUNCATED_GAUSSIAN_V1),
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
        Self::new(sources.sigma_world_units, sources.kernel_cutoff_sigma)
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
    /// whole grid.
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
        support.cells.clear();
        support.weight_sum = 0.0;
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
        let radius_squared = self.radius * self.radius;
        let two_sigma_squared = 2.0 * self.sigma * self.sigma;
        for &row in &support.rows {
            for &column in &support.columns {
                let cell_center = [
                    (f64::from(column) + 0.5) * grid.cell_width(),
                    (f64::from(row) + 0.5) * grid.cell_height(),
                ];
                let [dx, dy] = grid.displacement(center, cell_center)?;
                let distance_squared = dx * dx + dy * dy;
                if distance_squared > radius_squared {
                    continue;
                }
                let weight = exp_neg(distance_squared / two_sigma_squared).ok_or(
                    CoreError::NumericalInvalid {
                        quantity: "source weight",
                        detail: format!("|d|² = {distance_squared:?} has no weight"),
                    },
                )?;
                let index = grid.cell_index(column, row).ok_or_else(overflow)?;
                support.cells.push((index, weight));
                support.weight_sum += weight;
            }
        }
        if support.cells.is_empty() || support.weight_sum <= 0.0 || !support.weight_sum.is_finite()
        {
            return Err(CoreError::NumericalInvalid {
                quantity: "source support",
                detail: format!(
                    "no cell center lies within {:?} of {center:?} with positive weight",
                    self.radius
                ),
            });
        }
        Ok(())
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
    let low = ((coordinate - radius) / cell - 0.5).floor() - 1.0;
    let high = ((coordinate + radius) / cell - 0.5).ceil() + 1.0;
    let span = high - low + 1.0;
    if !span.is_finite() || span >= f64::from(cells) {
        out.extend(0..cells);
        return true;
    }
    // |low|, |high| < cells + radius / cell, which the span check bounds.
    let (low, high) = (low as i64, high as i64);
    let modulus = i64::from(cells);
    out.extend((low..=high).filter_map(|index| u32::try_from(index.rem_euclid(modulus)).ok()));
    out.sort_unstable();
    out.dedup();
    true
}

/// A source's support: cell indices and raw weights, in cell-index order.
/// It is reused across ticks, so steady state does not allocate.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceSupport {
    cells: Vec<(usize, f64)>,
    weight_sum: f64,
    columns: Vec<u32>,
    rows: Vec<u32>,
}

impl SourceSupport {
    /// An empty support.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Cells in the support.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Whether the support holds no cell.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// `Σ k_c` in cell-index order.
    #[must_use]
    pub const fn weight_sum(&self) -> f64 {
        self.weight_sum
    }

    /// `(cell index, k_c / Σ k)` in cell-index order.
    pub fn normalized(&self) -> impl Iterator<Item = (usize, f64)> + '_ {
        self.cells
            .iter()
            .map(|&(index, weight)| (index, weight / self.weight_sum))
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
}

/// `Σ a_k²` in channel order.
fn squared_norm(values: &[f64]) -> f64 {
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
    let grid = *state.grid();
    kernel.support_into(&grid, source.center, support)?;
    let declared = source.power * f64::from(step.numerator()) / f64::from(step.denominator());
    let mut normalized_total = 0.0;
    let mut applied = 0.0;
    for (index, weight) in support.normalized() {
        let amount = declared * weight;
        normalized_total += amount;
        let energy = state.energy_mut().get_mut(index).ok_or_else(outside)?;
        let before = f64::from(*energy);
        let after = (before + amount) as f32;
        applied += f64::from(after) - before;
        *energy = after;
        for (channel, &component) in (0_u32..).zip(source.direction.iter()) {
            if component == 0.0 {
                continue;
            }
            let moment = state
                .moment_mut(channel)
                .and_then(|plane| plane.get_mut(index))
                .ok_or_else(outside)?;
            *moment = (f64::from(*moment) + amount * component) as f32;
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
