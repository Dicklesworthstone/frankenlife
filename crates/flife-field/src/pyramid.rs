//! The sensor density pyramid and its sampling (plan §6.1, §8.1 phase 3).
//!
//! `positive-box-v1`. Level 0 is the field's density, `R_0 = e/(h_x h_y)`
//! and `M_0 = m/(h_x h_y)`, read from the F32 field in F64. Level `l + 1` is
//! the 2×2 average `¼((a + b) + (c + d))` of level `l`, for `R` and for
//! every component of `M`. Its spacing doubles and its cell centers stay
//! anchored to the same torus origin. Averaging densities, rather than
//! averaging cell energies and dividing by a coarse area, keeps a uniform
//! density uniform at every level.
//!
//! Sampling one level interpolates its raw densities bilinearly at the
//! position and differentiates that same interpolant analytically with the
//! level's physical spacings. At a knot (a stored cell center) the
//! floor-selected interval to the right is used. Geometric cell edges are
//! not knots of this cell-centered interpolant. The interpolant is written
//! in lerp form, so equal corner values give that value exactly and a zero
//! derivative exactly.
//!
//! `log-linear-v1`. Level `l` has scale `s_l = 2^l max(h_x, h_y)`. A sensing
//! scale `s` between `s_l` and `s_(l+1)` blends the two levels' raw values
//! and derivatives with `t = log₂(s/s_l)`, again in lerp form. Normalization
//! (regularized composition, energy observation, saturation) happens only
//! after blending, in the sensor encoder.
//!
//! The reference profile stores coarse levels in F64, about two thirds of
//! an F32 base field. An F32 pyramid is a different numerical profile.

use flife_core::{CoreError, ValidatedConfig};

use crate::exchange::split;
use crate::footprint::BufferLimits;
use crate::grid::{FIELD_PLANES, FieldGrid};
use crate::numerics::log2;
use crate::state::FieldState;

/// The pyramid filter this module implements.
pub const POSITIVE_BOX_V1: &str = "positive-box-v1";

/// The scale blend this module implements.
pub const LOG_LINEAR_V1: &str = "log-linear-v1";

/// Planes per level: energy density, then 32 moment densities.
pub const PLANES: usize = FIELD_PLANES as usize;

const BUFFER: &str = "sensor pyramid level bytes";

/// One level's geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelGrid {
    /// Cells along x.
    pub cells_x: u32,
    /// Cells along y.
    pub cells_y: u32,
    /// Cell spacing along x, in world-length.
    pub spacing_x: f64,
    /// Cell spacing along y, in world-length.
    pub spacing_y: f64,
}

/// Reusable storage for the coarse levels of one grid's pyramid.
#[derive(Debug, Clone, PartialEq)]
pub struct SensorPyramid {
    grid: FieldGrid,
    levels: u32,
    /// `coarse[l − 1]` holds level `l` as planar `[plane, y, x]` densities.
    coarse: Vec<Vec<f64>>,
}

impl SensorPyramid {
    /// Storage for `levels` coarse levels above `grid`.
    ///
    /// # Errors
    /// - [`CoreError::NumericalInvalid`] for 32 or more levels, or a grid
    ///   side not divisible by `2^levels`.
    /// - [`CoreError::BudgetExhausted`] for a level above `limits`.
    /// - [`CoreError::AllocationFailed`] if the host refuses.
    pub fn new(grid: FieldGrid, levels: u32, limits: &BufferLimits) -> Result<Self, CoreError> {
        let invalid = |detail: String| CoreError::NumericalInvalid {
            quantity: "sensor pyramid levels",
            detail,
        };
        if levels >= 32 {
            return Err(invalid(format!(
                "{levels} levels; at most 31 are supported"
            )));
        }
        let divisor = 1_u32 << levels;
        if !grid.cells_x().is_multiple_of(divisor) || !grid.cells_y().is_multiple_of(divisor) {
            return Err(invalid(format!(
                "a {} × {} grid is not divisible by 2^{levels}",
                grid.cells_x(),
                grid.cells_y()
            )));
        }
        let mut coarse = Vec::new();
        for level in 1..=levels {
            let elements = PLANES
                * usize::try_from(grid.cells_x() >> level).map_err(|_| overflow())?
                * usize::try_from(grid.cells_y() >> level).map_err(|_| overflow())?;
            let bytes = u64::try_from(elements)
                .ok()
                .and_then(|elements| elements.checked_mul(8))
                .ok_or_else(overflow)?;
            limits.admit(BUFFER, bytes)?;
            let mut plane = Vec::new();
            plane
                .try_reserve_exact(elements)
                .map_err(|_| CoreError::AllocationFailed {
                    buffer: BUFFER,
                    bytes,
                })?;
            plane.resize(elements, 0.0);
            coarse.push(plane);
        }
        Ok(Self {
            grid,
            levels,
            coarse,
        })
    }

    /// The pyramid a validated configuration needs: its grid and
    /// `pyramid_levels`.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for another pyramid filter or scale
    /// blend; otherwise as [`FieldGrid::from_config`] and
    /// [`SensorPyramid::new`].
    pub fn from_config(config: &ValidatedConfig, limits: &BufferLimits) -> Result<Self, CoreError> {
        let sensors = &config.config().sensors;
        if sensors.pyramid_filter != POSITIVE_BOX_V1 {
            return Err(CoreError::UnknownAlgorithm {
                role: "sensors.pyramid_filter",
                name: sensors.pyramid_filter.clone(),
            });
        }
        if sensors.pyramid_scale_blend != LOG_LINEAR_V1 {
            return Err(CoreError::UnknownAlgorithm {
                role: "sensors.pyramid_scale_blend",
                name: sensors.pyramid_scale_blend.clone(),
            });
        }
        Self::new(
            FieldGrid::from_config(config)?,
            config.report().pyramid_levels,
            limits,
        )
    }

    /// The base grid.
    #[must_use]
    pub const fn grid(&self) -> &FieldGrid {
        &self.grid
    }

    /// The number of coarse levels `L`; levels run from 0 to `L`.
    #[must_use]
    pub const fn levels(&self) -> u32 {
        self.levels
    }

    /// The geometry of `level`, or `None` above the coarsest.
    #[must_use]
    pub fn level_grid(&self, level: u32) -> Option<LevelGrid> {
        if level > self.levels {
            return None;
        }
        let factor = f64::from(1_u32 << level);
        Some(LevelGrid {
            cells_x: self.grid.cells_x() >> level,
            cells_y: self.grid.cells_y() >> level,
            spacing_x: self.grid.cell_width() * factor,
            spacing_y: self.grid.cell_height() * factor,
        })
    }

    /// `s_l = 2^l max(h_x, h_y)`, or `None` above the coarsest level.
    #[must_use]
    pub fn level_scale(&self, level: u32) -> Option<f64> {
        (level <= self.levels).then(|| self.finest_scale() * f64::from(1_u32 << level))
    }

    /// The bytes the coarse levels occupy.
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        self.coarse
            .iter()
            .map(|level| {
                u64::try_from(level.len())
                    .unwrap_or(u64::MAX)
                    .saturating_mul(8)
            })
            .fold(0, u64::saturating_add)
    }

    /// Build every coarse level from `field` and return a view that samples
    /// it. The view borrows the field, so it cannot outlive or disagree with
    /// the field it was built from.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] when `field` is on another grid.
    pub fn build<'a>(&'a mut self, field: &'a FieldState) -> Result<PyramidView<'a>, CoreError> {
        if field.grid() != &self.grid {
            return Err(CoreError::LayoutMismatch {
                buffer: "sensor pyramid",
                detail: format!(
                    "built for {:?}, offered a field on {:?}",
                    self.grid,
                    field.grid()
                ),
            });
        }
        let area = self.grid.cell_area();
        let cells_x = usize::try_from(self.grid.cells_x()).map_err(|_| overflow())?;
        let cells_y = usize::try_from(self.grid.cells_y()).map_err(|_| overflow())?;
        for level in 1..=self.levels {
            let fine_x = cells_x >> (level - 1);
            let fine_cells = fine_x * (cells_y >> (level - 1));
            let coarse_cells = (cells_x >> level) * (cells_y >> level);
            let index = usize::try_from(level - 1).map_err(|_| overflow())?;
            let (finer, rest) = self.coarse.split_at_mut(index);
            let target = rest.first_mut().ok_or_else(overflow)?;
            for (plane, coarse) in (0_u32..).zip(target.chunks_exact_mut(coarse_cells)) {
                if level == 1 {
                    let source = field
                        .plane(plane)
                        .ok_or_else(|| CoreError::LayoutMismatch {
                            buffer: "sensor pyramid",
                            detail: format!("the field has no plane {plane}"),
                        })?;
                    average(source, fine_x, coarse, |energy| f64::from(energy) / area);
                } else {
                    let source = finer
                        .last()
                        .and_then(|level| level.chunks_exact(fine_cells).nth(plane as usize))
                        .ok_or_else(overflow)?;
                    average(source, fine_x, coarse, |density| density);
                }
            }
        }
        Ok(PyramidView {
            field,
            pyramid: self,
            area,
        })
    }

    fn finest_scale(&self) -> f64 {
        self.grid.cell_width().max(self.grid.cell_height())
    }
}

/// `target[y][x] = ¼((a + b) + (c + d))` over the 2×2 block of `source`
/// (row length `fine_x`) at `(2x, 2y)`, with `a, b` its top pair and
/// `c, d` its bottom pair.
fn average<T: Copy>(source: &[T], fine_x: usize, target: &mut [f64], density: impl Fn(T) -> f64) {
    let coarse_x = fine_x / 2;
    if coarse_x == 0 {
        return;
    }
    for (row, rows) in target
        .chunks_exact_mut(coarse_x)
        .zip(source.chunks_exact(2 * fine_x))
    {
        let (top, bottom) = rows.split_at(fine_x);
        let (top, _) = top.as_chunks::<2>();
        let (bottom, _) = bottom.as_chunks::<2>();
        for (out, (&[a, b], &[c, d])) in row.iter_mut().zip(top.iter().zip(bottom)) {
            *out = 0.25 * ((density(a) + density(b)) + (density(c) + density(d)));
        }
    }
}

/// A built pyramid together with the field it was built from.
#[derive(Debug, Clone, Copy)]
pub struct PyramidView<'a> {
    field: &'a FieldState,
    pyramid: &'a SensorPyramid,
    area: f64,
}

/// Raw blended densities and their physical derivatives at one position:
/// plane 0 is `R`, plane `k + 1` is `M_k`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawSample {
    /// `R, M_0, …, M_31`.
    pub values: [f64; PLANES],
    /// `∂/∂x` of each, per world-length.
    pub dx: [f64; PLANES],
    /// `∂/∂y` of each, per world-length.
    pub dy: [f64; PLANES],
}

impl PyramidView<'_> {
    /// The pyramid.
    #[must_use]
    pub const fn pyramid(&self) -> &SensorPyramid {
        self.pyramid
    }

    /// The stored density of `plane` in cell `(x, y)` of `level`, or `None`
    /// outside the pyramid.
    #[must_use]
    pub fn density(&self, level: u32, plane: usize, x: u32, y: u32) -> Option<f64> {
        let geometry = self.pyramid.level_grid(level)?;
        if x >= geometry.cells_x || y >= geometry.cells_y || plane >= PLANES {
            return None;
        }
        let cells =
            usize::try_from(u64::from(geometry.cells_x) * u64::from(geometry.cells_y)).ok()?;
        let cell =
            usize::try_from(u64::from(y) * u64::from(geometry.cells_x) + u64::from(x)).ok()?;
        if level == 0 {
            let value = self.field.plane(u32::try_from(plane).ok()?)?.get(cell)?;
            return Some(f64::from(*value) / self.area);
        }
        let stored = self.pyramid.coarse.get(usize::try_from(level - 1).ok()?)?;
        stored.get(plane * cells + cell).copied()
    }

    /// The level and blend weight `(l, t)` for sensing scale `scale`, with
    /// `s_l ≤ s < s_(l+1)` and `t = log₂(s/s_l) ∈ [0, 1)`; at the coarsest
    /// scale, `(L, 0)`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite scale, or one outside
    /// `[s_0, s_L]`.
    pub fn blend(&self, scale: f64) -> Result<(u32, f64), CoreError> {
        let finest = self.pyramid.finest_scale();
        let coarsest = finest * f64::from(1_u32 << self.pyramid.levels);
        let ratio = scale / finest;
        if !scale.is_finite() || !(1.0..=f64::from(1_u32 << self.pyramid.levels)).contains(&ratio) {
            return Err(CoreError::NumericalInvalid {
                quantity: "sensing scale",
                detail: format!("{scale} is outside the pyramid's [{finest}, {coarsest}]"),
            });
        }
        let log = log2(ratio).ok_or_else(|| CoreError::NumericalInvalid {
            quantity: "sensing scale",
            detail: format!("log₂({ratio}) is undefined"),
        })?;
        // `0 ≤ log ≤ L < 32`: the floor is an exact small integer and
        // `log − floor` is exact.
        let floor = log.floor();
        let level = (floor as u32).min(self.pyramid.levels);
        Ok((level, log - f64::from(level)))
    }

    /// Raw densities and derivatives of `level` at `position`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a level above the coarsest, a
    /// nonfinite position or a nonfinite result.
    pub fn sample_level(&self, level: u32, position: [f64; 2]) -> Result<RawSample, CoreError> {
        let geometry =
            self.pyramid
                .level_grid(level)
                .ok_or_else(|| CoreError::NumericalInvalid {
                    quantity: "pyramid level",
                    detail: format!(
                        "{level} is above the coarsest level {}",
                        self.pyramid.levels
                    ),
                })?;
        let [x, y] = self.pyramid.grid.wrap_position(position)?;
        let (column, fx) = split(x / geometry.spacing_x - 0.5);
        let (row, fy) = split(y / geometry.spacing_y - 0.5);
        let wrap =
            |index: i64, cells: u32| u32::try_from(index.rem_euclid(i64::from(cells))).unwrap_or(0);
        let (x0, x1) = (
            wrap(column, geometry.cells_x),
            wrap(column + 1, geometry.cells_x),
        );
        let (y0, y1) = (wrap(row, geometry.cells_y), wrap(row + 1, geometry.cells_y));
        let mut sample = RawSample {
            values: [0.0; PLANES],
            dx: [0.0; PLANES],
            dy: [0.0; PLANES],
        };
        let missing = || CoreError::LayoutMismatch {
            buffer: "sensor pyramid",
            detail: format!("level {level} is missing a corner cell"),
        };
        for plane in 0..PLANES {
            let v00 = self.density(level, plane, x0, y0).ok_or_else(missing)?;
            let v10 = self.density(level, plane, x1, y0).ok_or_else(missing)?;
            let v01 = self.density(level, plane, x0, y1).ok_or_else(missing)?;
            let v11 = self.density(level, plane, x1, y1).ok_or_else(missing)?;
            let lower = v00 + fx * (v10 - v00);
            let upper = v01 + fx * (v11 - v01);
            let (across_lower, across_upper) = (v10 - v00, v11 - v01);
            let (up_left, up_right) = (v01 - v00, v11 - v10);
            if let (Some(value), Some(dx), Some(dy)) = (
                sample.values.get_mut(plane),
                sample.dx.get_mut(plane),
                sample.dy.get_mut(plane),
            ) {
                *value = lower + fy * (upper - lower);
                *dx = (across_lower + fy * (across_upper - across_lower)) / geometry.spacing_x;
                *dy = (up_left + fx * (up_right - up_left)) / geometry.spacing_y;
            }
        }
        finite(sample)
    }

    /// Raw densities and derivatives at `position` for sensing scale
    /// `scale`: levels `l` and `l + 1` blended with weight `t` (§6.1).
    ///
    /// # Errors
    /// As [`PyramidView::blend`] and [`PyramidView::sample_level`].
    pub fn sample(&self, position: [f64; 2], scale: f64) -> Result<RawSample, CoreError> {
        let (level, t) = self.blend(scale)?;
        let lower = self.sample_level(level, position)?;
        if t == 0.0 {
            return Ok(lower);
        }
        let upper = self.sample_level(level + 1, position)?;
        let lerp = |a: &[f64; PLANES], b: &[f64; PLANES]| {
            let mut out = [0.0; PLANES];
            for ((slot, low), high) in out.iter_mut().zip(a).zip(b) {
                *slot = low + t * (high - low);
            }
            out
        };
        finite(RawSample {
            values: lerp(&lower.values, &upper.values),
            dx: lerp(&lower.dx, &upper.dx),
            dy: lerp(&lower.dy, &upper.dy),
        })
    }
}

fn finite(sample: RawSample) -> Result<RawSample, CoreError> {
    let all = sample
        .values
        .iter()
        .chain(&sample.dx)
        .chain(&sample.dy)
        .all(|value| value.is_finite());
    if all {
        Ok(sample)
    } else {
        Err(CoreError::NumericalInvalid {
            quantity: "sensor sample",
            detail: "interpolation produced a nonfinite value".into(),
        })
    }
}

fn overflow() -> CoreError {
    CoreError::Overflow {
        quantity: "sensor pyramid size",
    }
}
