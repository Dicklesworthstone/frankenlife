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
use crate::grid::{DUAL_CARRIER_PLANES, FIELD_PLANES, FieldGrid, field_planes_for_config};
use crate::numerics::log2;
use crate::state::FieldState;

/// The pyramid filter this module implements.
pub const POSITIVE_BOX_V1: &str = "positive-box-v1";

/// The scale blend this module implements.
pub const LOG_LINEAR_V1: &str = "log-linear-v1";

/// Planes per single-carrier level: energy density, then 32 moment densities.
pub const PLANES: usize = FIELD_PLANES as usize;

/// Planes per 34-plane dual-carrier level (`V2-15`, `FL-055`):
/// Carrier 0 (`e_0` + 24 moments) + Carrier 1 (`e_1` + 8 trail moments).
pub const MAX_PLANES: usize = DUAL_CARRIER_PLANES as usize;

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

/// Reusable storage for the coarse levels of one grid's pyramid (`33` planes
/// single-carrier or `34` planes dual-carrier, `V2-15`).
#[derive(Debug, Clone, PartialEq)]
pub struct SensorPyramid {
    grid: FieldGrid,
    planes: usize,
    levels: u32,
    /// `coarse[l − 1]` holds level `l` as planar `[plane, y, x]` densities.
    coarse: Vec<Vec<f64>>,
}

impl SensorPyramid {
    /// Storage for `levels` coarse levels above `grid` (33 planes single-carrier).
    ///
    /// # Errors
    /// - [`CoreError::NumericalInvalid`] for 32 or more levels, or a grid
    ///   side not divisible by `2^levels`.
    /// - [`CoreError::BudgetExhausted`] for a level above `limits`.
    /// - [`CoreError::AllocationFailed`] if the host refuses.
    pub fn new(grid: FieldGrid, levels: u32, limits: &BufferLimits) -> Result<Self, CoreError> {
        Self::new_with_planes(grid, levels, PLANES, limits)
    }

    /// Storage for `levels` coarse levels above `grid` with `planes` (`33` or
    /// `34`, `V2-15`).
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for an unsupported plane count; otherwise
    /// as [`SensorPyramid::new`].
    pub fn new_with_planes(
        grid: FieldGrid,
        levels: u32,
        planes: usize,
        limits: &BufferLimits,
    ) -> Result<Self, CoreError> {
        if planes != PLANES && planes != MAX_PLANES {
            return Err(CoreError::LayoutMismatch {
                buffer: "sensor pyramid",
                detail: format!("{planes} planes where {PLANES} or {MAX_PLANES} are supported"),
            });
        }
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
            let elements = planes
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
            planes,
            levels,
            coarse,
        })
    }

    /// The pyramid a validated configuration needs: its grid, plane count
    /// (`33` single-carrier or `34` dual-carrier) and `pyramid_levels`.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for another pyramid filter or scale
    /// blend; otherwise as [`FieldGrid::from_config`] and
    /// [`SensorPyramid::new_with_planes`].
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
        let planes = field_planes_for_config(config) as usize;
        Self::new_with_planes(
            FieldGrid::from_config(config)?,
            config.report().pyramid_levels,
            planes,
            limits,
        )
    }

    /// The base grid.
    #[must_use]
    pub const fn grid(&self) -> &FieldGrid {
        &self.grid
    }

    /// Planes stored per level (`33` single-carrier or `34` dual-carrier).
    #[must_use]
    pub const fn planes(&self) -> usize {
        self.planes
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
        self.build_on(field, 1)
    }

    /// [`SensorPyramid::build`] with the 33 independent planes split over up
    /// to `workers` threads (this one and scoped workers joined before
    /// returning). Each plane's coarse levels depend only on that plane, so
    /// the result is bit-identical at every worker count.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] when `field` is on another grid, or
    /// [`CoreError::StateConflict`] if a worker panicked.
    pub fn build_on<'a>(
        &'a mut self,
        field: &'a FieldState,
        workers: usize,
    ) -> Result<PyramidView<'a>, CoreError> {
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
        let planes = self.planes;
        if field.planes() as usize != planes {
            return Err(CoreError::LayoutMismatch {
                buffer: "sensor pyramid",
                detail: format!(
                    "built for {planes} planes, offered a {}-plane field",
                    field.planes()
                ),
            });
        }
        let area = self.grid.cell_area();
        let inv_area = exact_reciprocal(area);
        let cells_x = usize::try_from(self.grid.cells_x()).map_err(|_| overflow())?;
        let cells_y = usize::try_from(self.grid.cells_y()).map_err(|_| overflow())?;
        let base_cells = cells_x.checked_mul(cells_y).ok_or_else(overflow)?;
        let field_slice = field.as_slice();
        if self.levels > 0 && field_slice.len() < planes.saturating_mul(base_cells) {
            return Err(CoreError::LayoutMismatch {
                buffer: "sensor pyramid",
                detail: "the field does not hold every plane".to_owned(),
            });
        }
        let used = if self.levels == 0 || base_cells < 16_384 {
            1
        } else {
            workers.clamp(1, planes)
        };
        if used == 1 {
            for level in 1..=self.levels {
                let fine_x = cells_x >> (level - 1);
                let fine_cells = fine_x * (cells_y >> (level - 1));
                let coarse_cells = (cells_x >> level) * (cells_y >> level);
                let index = usize::try_from(level - 1).map_err(|_| overflow())?;
                let (finer, rest) = self.coarse.split_at_mut(index);
                let target = rest.first_mut().ok_or_else(overflow)?;
                for (plane, coarse) in (0_usize..).zip(target.chunks_exact_mut(coarse_cells)) {
                    if level == 1 {
                        let start = plane * base_cells;
                        let source = field_slice
                            .get(start..start + base_cells)
                            .ok_or_else(overflow)?;
                        average_base(source, fine_x, coarse, area, inv_area);
                    } else {
                        let source = finer
                            .last()
                            .and_then(|prev| prev.chunks_exact(fine_cells).nth(plane))
                            .ok_or_else(overflow)?;
                        average(source, fine_x, coarse, |density| density);
                    }
                }
            }
        } else {
            // Split each coarse level into disjoint whole-plane slices per
            // worker so all levels `1..=L` of each plane run in one thread
            // scope while staying hot in L2 cache.
            let per = planes.div_ceil(used);
            let mut per_worker: Vec<(usize, usize, Vec<&mut [f64]>)> = Vec::with_capacity(used);
            for w in 0..used {
                let first = w * per;
                if first >= planes {
                    break;
                }
                let count = (planes - first).min(per);
                per_worker.push((first, count, Vec::with_capacity(self.coarse.len())));
            }
            for (level_idx, level_buf) in self.coarse.iter_mut().enumerate() {
                let level = level_idx + 1;
                let coarse_cells = (cells_x >> level) * (cells_y >> level);
                let mut rest = level_buf.as_mut_slice();
                for &mut (_, count, ref mut levels) in &mut per_worker {
                    let (head, tail) = rest
                        .split_at_mut_checked(count * coarse_cells)
                        .ok_or_else(overflow)?;
                    levels.push(head);
                    rest = tail;
                }
            }
            let build_planes = |first_plane: usize, count: usize, levels: &mut [&mut [f64]]| {
                for local_plane in 0..count {
                    let plane = first_plane + local_plane;
                    for level_idx in 0..levels.len() {
                        let level = level_idx + 1;
                        let fine_x = cells_x >> (level - 1);
                        let fine_cells = fine_x * (cells_y >> (level - 1));
                        let coarse_cells = (cells_x >> level) * (cells_y >> level);
                        let (finer, rest) = levels.split_at_mut(level_idx);
                        let target = &mut rest[0]
                            [local_plane * coarse_cells..(local_plane + 1) * coarse_cells];
                        if level == 1 {
                            let start = plane * base_cells;
                            let source = &field_slice[start..start + base_cells];
                            average_base(source, fine_x, target, area, inv_area);
                        } else {
                            let prev = &finer[level_idx - 1];
                            let source =
                                &prev[local_plane * fine_cells..(local_plane + 1) * fine_cells];
                            average(source, fine_x, target, |density| density);
                        }
                    }
                }
            };
            let build_planes = &build_planes;
            let panicked = std::thread::scope(|scope| {
                let mut iter = per_worker.iter_mut();
                let head = iter.next();
                let handles: Vec<_> = iter
                    .map(|(first, count, levels)| {
                        let (first, count) = (*first, *count);
                        scope.spawn(move || build_planes(first, count, levels))
                    })
                    .collect();
                if let Some((first, count, levels)) = head {
                    build_planes(*first, *count, levels);
                }
                let mut panicked = false;
                for handle in handles {
                    panicked |= handle.join().is_err();
                }
                panicked
            });
            if panicked {
                return Err(CoreError::StateConflict {
                    operation: "build sensor pyramid",
                    detail: "a pyramid worker panicked".to_owned(),
                });
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

    /// The level and blend weight `(l, t)` for sensing scale `scale`, with
    /// `s_l ≤ s < s_(l+1)` and `t = log₂(s/s_l) ∈ [0, 1)`; at the coarsest
    /// scale, `(L, 0)`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite scale, or one outside
    /// `[s_0, s_L]`.
    pub fn blend(&self, scale: f64) -> Result<(u32, f64), CoreError> {
        let finest = self.finest_scale();
        let coarsest = finest * f64::from(1_u32 << self.levels);
        let ratio = scale / finest;
        if !scale.is_finite() || !(1.0..=f64::from(1_u32 << self.levels)).contains(&ratio) {
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
        let level = (floor as u32).min(self.levels);
        Ok((level, log - f64::from(level)))
    }

    /// Where `level`'s interpolant reads at `position`: its geometry, the
    /// four corner cells and the fractions between them.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a level above the coarsest or a
    /// nonfinite position.
    pub fn corners(&self, level: u32, position: [f64; 2]) -> Result<Corners, CoreError> {
        let wrapped = self.grid.wrap_position(position)?;
        self.corners_wrapped(level, wrapped)
    }

    fn corners_wrapped(&self, level: u32, wrapped: [f64; 2]) -> Result<Corners, CoreError> {
        let geometry = self
            .level_grid(level)
            .ok_or_else(|| CoreError::NumericalInvalid {
                quantity: "pyramid level",
                detail: format!("{level} is above the coarsest level {}", self.levels),
            })?;
        let [x, y] = wrapped;
        let (column, fx) =
            split(div_exact(x, geometry.spacing_x, exact_reciprocal(geometry.spacing_x)) - 0.5);
        let (row, fy) =
            split(div_exact(y, geometry.spacing_y, exact_reciprocal(geometry.spacing_y)) - 0.5);
        let x0 = wrap_cell_index(column, geometry.cells_x);
        let x1 = if x0 + 1 < geometry.cells_x { x0 + 1 } else { 0 };
        let y0 = wrap_cell_index(row, geometry.cells_y);
        let y1 = if y0 + 1 < geometry.cells_y { y0 + 1 } else { 0 };
        Ok(Corners {
            geometry,
            columns: [x0, x1],
            rows: [y0, y1],
            fx,
            fy,
        })
    }
}

/// Where one level's bilinear interpolant reads at a position (§6.1): the
/// cell centers to the left of and below it, the next ones (wrapped), and
/// the fractions of the way from the first to the second.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Corners {
    /// The level's geometry.
    pub geometry: LevelGrid,
    /// The left and right columns.
    pub columns: [u32; 2],
    /// The lower and upper rows.
    pub rows: [u32; 2],
    /// The fraction along x, in `[0, 1)`.
    pub fx: f64,
    /// The fraction along y, in `[0, 1)`.
    pub fy: f64,
}

/// One organism's sensing query (§8.1 phase 3): its position and its
/// sensing scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SensorQuery {
    /// Where it senses, in world-length.
    pub position: [f64; 2],
    /// Its sensing scale, in world-length.
    pub scale: f64,
}

/// Phase 3's raw sampling on another device (plan §10.7; FL-031).
pub trait SensorBackend: std::fmt::Debug + Send {
    /// The backend and its device, for run records.
    fn describe(&self) -> String;

    /// Every query's raw sample, in order: `samples[i]` is
    /// [`PyramidView::sample`] of `queries[i]` on `pyramid` built from
    /// `field`, bit for bit.
    ///
    /// # Errors
    /// Any. The world then samples the tick on the reference, which
    /// reproduces the error or finds none, and stops using a backend that
    /// failed where the reference did not.
    fn sample(
        &mut self,
        pyramid: &SensorPyramid,
        field: &FieldState,
        queries: &[SensorQuery],
        samples: &mut Vec<RawSample>,
    ) -> Result<(), CoreError>;

    /// Dual-carrier (`34`-plane, `V2-15`) raw sampling on another device:
    /// `samples[i]` must equal [`PyramidView::sample_dual`] bit for bit.
    ///
    /// Backends that do not override this return [`CoreError::MissingCapability`]
    /// so the world falls back cleanly to the CPU reference.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] when unimplemented, or as
    /// [`SensorBackend::sample`].
    fn sample_dual(
        &mut self,
        _pyramid: &SensorPyramid,
        _field: &FieldState,
        _queries: &[SensorQuery],
        _samples: &mut Vec<DualRawSample>,
    ) -> Result<(), CoreError> {
        Err(CoreError::MissingCapability {
            capability: "34-plane dual-carrier sensor sampling".into(),
        })
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

/// Raw blended densities and their physical derivatives at one position for a
/// 34-plane dual-carrier hybrid field (`V2-15`, `FL-055`):
/// - Plane `0`: `R_0` (Carrier 0 energy density)
/// - Planes `1..=24`: `M^{(0)}_{0..23}` (Carrier 0 moments)
/// - Plane `25`: `R_1` (Carrier 1 trail energy density)
/// - Planes `26..=33`: `M^{(1)}_{0..7}` (Carrier 1 trail moments)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DualRawSample {
    /// All 34 plane densities.
    pub values: [f64; MAX_PLANES],
    /// `∂/∂x` of each plane, per world-length.
    pub dx: [f64; MAX_PLANES],
    /// `∂/∂y` of each plane, per world-length.
    pub dy: [f64; MAX_PLANES],
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
        if x >= geometry.cells_x || y >= geometry.cells_y || plane >= self.pyramid.planes {
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
        self.pyramid.blend(scale)
    }

    /// Raw densities and derivatives of `level` at `position`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a level above the coarsest, a
    /// nonfinite position or a nonfinite result.
    pub fn sample_level(&self, level: u32, position: [f64; 2]) -> Result<RawSample, CoreError> {
        if self.pyramid.planes != PLANES {
            return Err(CoreError::LayoutMismatch {
                buffer: "sensor pyramid",
                detail: format!(
                    "sample requires {PLANES} planes, found {}; use sample_dual for 34-plane fields",
                    self.pyramid.planes
                ),
            });
        }
        let Corners {
            geometry,
            columns: [x0, x1],
            rows: [y0, y1],
            fx,
            fy,
        } = self.pyramid.corners(level, position)?;
        let mut sample = RawSample {
            values: [0.0; PLANES],
            dx: [0.0; PLANES],
            dy: [0.0; PLANES],
        };
        let missing = || CoreError::LayoutMismatch {
            buffer: "sensor pyramid",
            detail: format!("level {level} is missing a corner cell"),
        };
        let cx = usize::try_from(geometry.cells_x).map_err(|_| missing())?;
        let cy = usize::try_from(geometry.cells_y).map_err(|_| missing())?;
        let cells = cx.checked_mul(cy).ok_or_else(missing)?;
        let (x0, x1, y0, y1) = (x0 as usize, x1 as usize, y0 as usize, y1 as usize);
        if x0 >= cx || x1 >= cx || y0 >= cy || y1 >= cy {
            return Err(missing());
        }
        let i00 = y0 * cx + x0;
        let i10 = y0 * cx + x1;
        let i01 = y1 * cx + x0;
        let i11 = y1 * cx + x1;
        let total_needed = PLANES.checked_mul(cells).ok_or_else(missing)?;
        let spacing_x = geometry.spacing_x;
        let spacing_y = geometry.spacing_y;
        let inv_sx = exact_reciprocal(spacing_x);
        let inv_sy = exact_reciprocal(spacing_y);
        let mut all_finite = true;

        if level == 0 {
            let data = self.field.as_slice();
            if data.len() < total_needed {
                return Err(missing());
            }
            let area = self.area;
            let inv_area = exact_reciprocal(area);
            for plane in 0..PLANES {
                let base = plane * cells;
                let v00 = div_exact(f64::from(data[base + i00]), area, inv_area);
                let v10 = div_exact(f64::from(data[base + i10]), area, inv_area);
                let v01 = div_exact(f64::from(data[base + i01]), area, inv_area);
                let v11 = div_exact(f64::from(data[base + i11]), area, inv_area);
                let across_lower = v10 - v00;
                let across_upper = v11 - v01;
                let up_left = v01 - v00;
                let up_right = v11 - v10;
                let lower = v00 + fx * across_lower;
                let upper = v01 + fx * across_upper;
                let value = lower + fy * (upper - lower);
                let dx = div_exact(
                    across_lower + fy * (across_upper - across_lower),
                    spacing_x,
                    inv_sx,
                );
                let dy = div_exact(up_left + fx * (up_right - up_left), spacing_y, inv_sy);
                all_finite &= value.is_finite() & dx.is_finite() & dy.is_finite();
                sample.values[plane] = value;
                sample.dx[plane] = dx;
                sample.dy[plane] = dy;
            }
        } else {
            let stored = self
                .pyramid
                .coarse
                .get(usize::try_from(level - 1).map_err(|_| missing())?)
                .ok_or_else(missing)?;
            if stored.len() < total_needed {
                return Err(missing());
            }
            for plane in 0..PLANES {
                let base = plane * cells;
                let v00 = stored[base + i00];
                let v10 = stored[base + i10];
                let v01 = stored[base + i01];
                let v11 = stored[base + i11];
                let across_lower = v10 - v00;
                let across_upper = v11 - v01;
                let up_left = v01 - v00;
                let up_right = v11 - v10;
                let lower = v00 + fx * across_lower;
                let upper = v01 + fx * across_upper;
                let value = lower + fy * (upper - lower);
                let dx = div_exact(
                    across_lower + fy * (across_upper - across_lower),
                    spacing_x,
                    inv_sx,
                );
                let dy = div_exact(up_left + fx * (up_right - up_left), spacing_y, inv_sy);
                all_finite &= value.is_finite() & dx.is_finite() & dy.is_finite();
                sample.values[plane] = value;
                sample.dx[plane] = dx;
                sample.dy[plane] = dy;
            }
        }
        if all_finite {
            Ok(sample)
        } else {
            Err(CoreError::NumericalInvalid {
                quantity: "sensor sample",
                detail: "interpolation produced a nonfinite value".into(),
            })
        }
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

    /// Sample a slice of queries into `out` plane-by-plane, bit-identical to
    /// calling [`PyramidView::sample`] on each query in order.
    ///
    /// Reading each of the 33 planes across a batch of organisms before moving
    /// to the next plane avoids 33-way L1D cache-set conflict misses on
    /// power-of-two grids (such as 512² where planes lie 1 MiB apart).
    ///
    /// # Errors
    /// The first error [`PyramidView::sample`] would return, in query order,
    /// or [`CoreError::LayoutMismatch`] if `out.len() != queries.len()`.
    pub fn sample_batch(
        &self,
        queries: &[SensorQuery],
        out: &mut [RawSample],
    ) -> Result<(), CoreError> {
        if self.pyramid.planes != PLANES {
            return Err(CoreError::LayoutMismatch {
                buffer: "sensor pyramid",
                detail: format!(
                    "sample_batch requires {PLANES} planes, found {}; use sample_dual_batch for 34-plane fields",
                    self.pyramid.planes
                ),
            });
        }
        if queries.len() != out.len() {
            return Err(CoreError::LayoutMismatch {
                buffer: "sensor samples",
                detail: format!("{} outputs for {} queries", out.len(), queries.len()),
            });
        }
        const BATCH: usize = 32;
        #[derive(Clone, Copy)]
        struct PreparedLevel {
            level: u32,
            cells: usize,
            i00: usize,
            i10: usize,
            i01: usize,
            i11: usize,
            fx: f64,
            fy: f64,
            spacing_x: f64,
            spacing_y: f64,
            inv_sx: Option<f64>,
            inv_sy: Option<f64>,
        }
        #[derive(Clone, Copy)]
        struct PreparedQuery {
            lo: PreparedLevel,
            hi: Option<PreparedLevel>,
            t: f64,
        }
        let missing = |level: u32| CoreError::LayoutMismatch {
            buffer: "sensor pyramid",
            detail: format!("level {level} is missing a corner cell"),
        };
        let prepare_level = |level: u32, wrapped: [f64; 2]| -> Result<PreparedLevel, CoreError> {
            let Corners {
                geometry,
                columns: [x0, x1],
                rows: [y0, y1],
                fx,
                fy,
            } = self.pyramid.corners_wrapped(level, wrapped)?;
            let cx = usize::try_from(geometry.cells_x).map_err(|_| missing(level))?;
            let cy = usize::try_from(geometry.cells_y).map_err(|_| missing(level))?;
            let cells = cx.checked_mul(cy).ok_or_else(|| missing(level))?;
            let (x0, x1, y0, y1) = (x0 as usize, x1 as usize, y0 as usize, y1 as usize);
            if x0 >= cx || x1 >= cx || y0 >= cy || y1 >= cy {
                return Err(missing(level));
            }
            let total_needed = PLANES.checked_mul(cells).ok_or_else(|| missing(level))?;
            if level == 0 {
                if self.field.as_slice().len() < total_needed {
                    return Err(missing(level));
                }
            } else {
                let stored = self
                    .pyramid
                    .coarse
                    .get(usize::try_from(level - 1).map_err(|_| missing(level))?)
                    .ok_or_else(|| missing(level))?;
                if stored.len() < total_needed {
                    return Err(missing(level));
                }
            }
            Ok(PreparedLevel {
                level,
                cells,
                i00: y0 * cx + x0,
                i10: y0 * cx + x1,
                i01: y1 * cx + x0,
                i11: y1 * cx + x1,
                fx,
                fy,
                spacing_x: geometry.spacing_x,
                spacing_y: geometry.spacing_y,
                inv_sx: exact_reciprocal(geometry.spacing_x),
                inv_sy: exact_reciprocal(geometry.spacing_y),
            })
        };

        let field_data = self.field.as_slice();
        let area = self.area;
        let inv_area = exact_reciprocal(area);
        let eval_level = |p: &PreparedLevel, plane: usize| -> (f64, f64, f64) {
            let base = plane * p.cells;
            let (v00, v10, v01, v11) = if p.level == 0 {
                (
                    div_exact(f64::from(field_data[base + p.i00]), area, inv_area),
                    div_exact(f64::from(field_data[base + p.i10]), area, inv_area),
                    div_exact(f64::from(field_data[base + p.i01]), area, inv_area),
                    div_exact(f64::from(field_data[base + p.i11]), area, inv_area),
                )
            } else {
                let stored = &self.pyramid.coarse[(p.level - 1) as usize];
                (
                    stored[base + p.i00],
                    stored[base + p.i10],
                    stored[base + p.i01],
                    stored[base + p.i11],
                )
            };
            let across_lower = v10 - v00;
            let across_upper = v11 - v01;
            let up_left = v01 - v00;
            let up_right = v11 - v10;
            let lower = v00 + p.fx * across_lower;
            let upper = v01 + p.fx * across_upper;
            let value = lower + p.fy * (upper - lower);
            let dx = div_exact(
                across_lower + p.fy * (across_upper - across_lower),
                p.spacing_x,
                p.inv_sx,
            );
            let dy = div_exact(up_left + p.fx * (up_right - up_left), p.spacing_y, p.inv_sy);
            (value, dx, dy)
        };

        let dummy_level = PreparedLevel {
            level: 0,
            cells: 0,
            i00: 0,
            i10: 0,
            i01: 0,
            i11: 0,
            fx: 0.0,
            fy: 0.0,
            spacing_x: 1.0,
            spacing_y: 1.0,
            inv_sx: Some(1.0),
            inv_sy: Some(1.0),
        };
        let mut prepared = [PreparedQuery {
            lo: dummy_level,
            hi: None,
            t: 0.0,
        }; BATCH];

        for (q_chunk, out_chunk) in queries.chunks(BATCH).zip(out.chunks_mut(BATCH)) {
            let count = q_chunk.len();
            let mut all_level0 = true;
            for (slot, query) in prepared.iter_mut().zip(q_chunk) {
                let (level, t) = self.blend(query.scale)?;
                let wrapped = self.pyramid.grid.wrap_position(query.position)?;
                let lo = prepare_level(level, wrapped)?;
                let hi = if t == 0.0 {
                    None
                } else {
                    Some(prepare_level(level + 1, wrapped)?)
                };
                all_level0 &= level == 0 && hi.is_none();
                *slot = PreparedQuery { lo, hi, t };
            }

            let mut all_finite = true;
            if all_level0 {
                let cells = prepared[0].lo.cells;
                let spacing_x = prepared[0].lo.spacing_x;
                let spacing_y = prepared[0].lo.spacing_y;
                let inv_sx = prepared[0].lo.inv_sx;
                let inv_sy = prepared[0].lo.inv_sy;
                if area == 1.0 && spacing_x == 1.0 && spacing_y == 1.0 {
                    for plane in 0..PLANES {
                        let plane_slice = &field_data[plane * cells..(plane + 1) * cells];
                        for (pq, sample) in prepared.iter().zip(out_chunk.iter_mut()).take(count) {
                            let p = &pq.lo;
                            let v00 = f64::from(plane_slice[p.i00]);
                            let v10 = f64::from(plane_slice[p.i10]);
                            let v01 = f64::from(plane_slice[p.i01]);
                            let v11 = f64::from(plane_slice[p.i11]);
                            let across_lower = v10 - v00;
                            let across_upper = v11 - v01;
                            let up_left = v01 - v00;
                            let up_right = v11 - v10;
                            let lower = v00 + p.fx * across_lower;
                            let upper = v01 + p.fx * across_upper;
                            let value = lower + p.fy * (upper - lower);
                            let dx = across_lower + p.fy * (across_upper - across_lower);
                            let dy = up_left + p.fx * (up_right - up_left);
                            all_finite &= value.is_finite() & dx.is_finite() & dy.is_finite();
                            sample.values[plane] = value;
                            sample.dx[plane] = dx;
                            sample.dy[plane] = dy;
                        }
                    }
                } else if let (Some(ra), Some(rx), Some(ry)) = (inv_area, inv_sx, inv_sy) {
                    for plane in 0..PLANES {
                        let plane_slice = &field_data[plane * cells..(plane + 1) * cells];
                        for (pq, sample) in prepared.iter().zip(out_chunk.iter_mut()).take(count) {
                            let p = &pq.lo;
                            let v00 = f64::from(plane_slice[p.i00]) * ra;
                            let v10 = f64::from(plane_slice[p.i10]) * ra;
                            let v01 = f64::from(plane_slice[p.i01]) * ra;
                            let v11 = f64::from(plane_slice[p.i11]) * ra;
                            let across_lower = v10 - v00;
                            let across_upper = v11 - v01;
                            let up_left = v01 - v00;
                            let up_right = v11 - v10;
                            let lower = v00 + p.fx * across_lower;
                            let upper = v01 + p.fx * across_upper;
                            let value = lower + p.fy * (upper - lower);
                            let dx = (across_lower + p.fy * (across_upper - across_lower)) * rx;
                            let dy = (up_left + p.fx * (up_right - up_left)) * ry;
                            all_finite &= value.is_finite() & dx.is_finite() & dy.is_finite();
                            sample.values[plane] = value;
                            sample.dx[plane] = dx;
                            sample.dy[plane] = dy;
                        }
                    }
                } else {
                    for plane in 0..PLANES {
                        let plane_slice = &field_data[plane * cells..(plane + 1) * cells];
                        for (pq, sample) in prepared.iter().zip(out_chunk.iter_mut()).take(count) {
                            let p = &pq.lo;
                            let v00 = f64::from(plane_slice[p.i00]) / area;
                            let v10 = f64::from(plane_slice[p.i10]) / area;
                            let v01 = f64::from(plane_slice[p.i01]) / area;
                            let v11 = f64::from(plane_slice[p.i11]) / area;
                            let across_lower = v10 - v00;
                            let across_upper = v11 - v01;
                            let up_left = v01 - v00;
                            let up_right = v11 - v10;
                            let lower = v00 + p.fx * across_lower;
                            let upper = v01 + p.fx * across_upper;
                            let value = lower + p.fy * (upper - lower);
                            let dx =
                                (across_lower + p.fy * (across_upper - across_lower)) / spacing_x;
                            let dy = (up_left + p.fx * (up_right - up_left)) / spacing_y;
                            all_finite &= value.is_finite() & dx.is_finite() & dy.is_finite();
                            sample.values[plane] = value;
                            sample.dx[plane] = dx;
                            sample.dy[plane] = dy;
                        }
                    }
                }
            } else {
                for plane in 0..PLANES {
                    for (pq, sample) in prepared.iter().zip(out_chunk.iter_mut()).take(count) {
                        let (lv, ldx, ldy) = eval_level(&pq.lo, plane);
                        all_finite &= lv.is_finite() & ldx.is_finite() & ldy.is_finite();
                        if let Some(hi) = &pq.hi {
                            let (hv, hdx, hdy) = eval_level(hi, plane);
                            let t = pq.t;
                            let v = lv + t * (hv - lv);
                            let dx = ldx + t * (hdx - ldx);
                            let dy = ldy + t * (hdy - ldy);
                            all_finite &= hv.is_finite()
                                & hdx.is_finite()
                                & hdy.is_finite()
                                & v.is_finite()
                                & dx.is_finite()
                                & dy.is_finite();
                            sample.values[plane] = v;
                            sample.dx[plane] = dx;
                            sample.dy[plane] = dy;
                        } else {
                            sample.values[plane] = lv;
                            sample.dx[plane] = ldx;
                            sample.dy[plane] = ldy;
                        }
                    }
                }
            }
            if !all_finite {
                for (query, sample) in q_chunk.iter().zip(out_chunk.iter_mut()) {
                    *sample = self.sample(query.position, query.scale)?;
                }
            }
        }
        Ok(())
    }

    /// Raw densities and derivatives of all 34 planes of `level` at `position`
    /// on a dual-carrier pyramid (`V2-15`).
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if this pyramid does not hold 34 planes,
    /// or as [`PyramidView::sample_level`].
    pub fn sample_dual_level(
        &self,
        level: u32,
        position: [f64; 2],
    ) -> Result<DualRawSample, CoreError> {
        if self.pyramid.planes != MAX_PLANES {
            return Err(CoreError::LayoutMismatch {
                buffer: "sensor pyramid",
                detail: format!(
                    "sample_dual requires {MAX_PLANES} planes, found {}",
                    self.pyramid.planes
                ),
            });
        }
        let Corners {
            geometry,
            columns: [x0, x1],
            rows: [y0, y1],
            fx,
            fy,
        } = self.pyramid.corners(level, position)?;
        let mut sample = DualRawSample {
            values: [0.0; MAX_PLANES],
            dx: [0.0; MAX_PLANES],
            dy: [0.0; MAX_PLANES],
        };
        let missing = || CoreError::LayoutMismatch {
            buffer: "sensor pyramid",
            detail: format!("level {level} is missing a corner cell"),
        };
        let cx = usize::try_from(geometry.cells_x).map_err(|_| missing())?;
        let cy = usize::try_from(geometry.cells_y).map_err(|_| missing())?;
        let cells = cx.checked_mul(cy).ok_or_else(missing)?;
        let (x0, x1, y0, y1) = (x0 as usize, x1 as usize, y0 as usize, y1 as usize);
        if x0 >= cx || x1 >= cx || y0 >= cy || y1 >= cy {
            return Err(missing());
        }
        let i00 = y0 * cx + x0;
        let i10 = y0 * cx + x1;
        let i01 = y1 * cx + x0;
        let i11 = y1 * cx + x1;
        let total_needed = MAX_PLANES.checked_mul(cells).ok_or_else(missing)?;
        let spacing_x = geometry.spacing_x;
        let spacing_y = geometry.spacing_y;
        let inv_sx = exact_reciprocal(spacing_x);
        let inv_sy = exact_reciprocal(spacing_y);
        let mut all_finite = true;

        if level == 0 {
            let data = self.field.as_slice();
            if data.len() < total_needed {
                return Err(missing());
            }
            let area = self.area;
            let inv_area = exact_reciprocal(area);
            for plane in 0..MAX_PLANES {
                let base = plane * cells;
                let v00 = div_exact(f64::from(data[base + i00]), area, inv_area);
                let v10 = div_exact(f64::from(data[base + i10]), area, inv_area);
                let v01 = div_exact(f64::from(data[base + i01]), area, inv_area);
                let v11 = div_exact(f64::from(data[base + i11]), area, inv_area);
                let across_lower = v10 - v00;
                let across_upper = v11 - v01;
                let up_left = v01 - v00;
                let up_right = v11 - v10;
                let lower = v00 + fx * across_lower;
                let upper = v01 + fx * across_upper;
                let value = lower + fy * (upper - lower);
                let dx = div_exact(
                    across_lower + fy * (across_upper - across_lower),
                    spacing_x,
                    inv_sx,
                );
                let dy = div_exact(up_left + fx * (up_right - up_left), spacing_y, inv_sy);
                all_finite &= value.is_finite() & dx.is_finite() & dy.is_finite();
                sample.values[plane] = value;
                sample.dx[plane] = dx;
                sample.dy[plane] = dy;
            }
        } else {
            let stored = self
                .pyramid
                .coarse
                .get(usize::try_from(level - 1).map_err(|_| missing())?)
                .ok_or_else(missing)?;
            if stored.len() < total_needed {
                return Err(missing());
            }
            for plane in 0..MAX_PLANES {
                let base = plane * cells;
                let v00 = stored[base + i00];
                let v10 = stored[base + i10];
                let v01 = stored[base + i01];
                let v11 = stored[base + i11];
                let across_lower = v10 - v00;
                let across_upper = v11 - v01;
                let up_left = v01 - v00;
                let up_right = v11 - v10;
                let lower = v00 + fx * across_lower;
                let upper = v01 + fx * across_upper;
                let value = lower + fy * (upper - lower);
                let dx = div_exact(
                    across_lower + fy * (across_upper - across_lower),
                    spacing_x,
                    inv_sx,
                );
                let dy = div_exact(up_left + fx * (up_right - up_left), spacing_y, inv_sy);
                all_finite &= value.is_finite() & dx.is_finite() & dy.is_finite();
                sample.values[plane] = value;
                sample.dx[plane] = dx;
                sample.dy[plane] = dy;
            }
        }
        if all_finite {
            Ok(sample)
        } else {
            Err(CoreError::NumericalInvalid {
                quantity: "sensor sample",
                detail: "interpolation produced a nonfinite value".into(),
            })
        }
    }

    /// Raw blended densities and derivatives of all 34 planes at `position`
    /// for sensing scale `scale` on a dual-carrier pyramid (`V2-15`).
    ///
    /// # Errors
    /// As [`PyramidView::blend`] and [`PyramidView::sample_dual_level`].
    pub fn sample_dual(&self, position: [f64; 2], scale: f64) -> Result<DualRawSample, CoreError> {
        let (level, t) = self.blend(scale)?;
        let lower = self.sample_dual_level(level, position)?;
        if t == 0.0 {
            return Ok(lower);
        }
        let upper = self.sample_dual_level(level + 1, position)?;
        let lerp = |a: &[f64; MAX_PLANES], b: &[f64; MAX_PLANES]| {
            let mut out = [0.0; MAX_PLANES];
            for ((slot, low), high) in out.iter_mut().zip(a).zip(b) {
                *slot = low + t * (high - low);
            }
            out
        };
        finite_dual(DualRawSample {
            values: lerp(&lower.values, &upper.values),
            dx: lerp(&lower.dx, &upper.dx),
            dy: lerp(&lower.dy, &upper.dy),
        })
    }

    /// Sample a batch of queries into `out` on a 34-plane dual-carrier pyramid
    /// (`V2-15`), bit-identical to [`PyramidView::sample_dual`] per query.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if `out.len() != queries.len()`, or as
    /// [`PyramidView::sample_dual`].
    pub fn sample_dual_batch(
        &self,
        queries: &[SensorQuery],
        out: &mut [DualRawSample],
    ) -> Result<(), CoreError> {
        if queries.len() != out.len() {
            return Err(CoreError::LayoutMismatch {
                buffer: "sensor samples",
                detail: format!("{} outputs for {} queries", out.len(), queries.len()),
            });
        }
        for (query, sample) in queries.iter().zip(out.iter_mut()) {
            *sample = self.sample_dual(query.position, query.scale)?;
        }
        Ok(())
    }
}

fn finite_dual(sample: DualRawSample) -> Result<DualRawSample, CoreError> {
    let mut all = true;
    for plane in 0..MAX_PLANES {
        all &= sample.values[plane].is_finite()
            & sample.dx[plane].is_finite()
            & sample.dy[plane].is_finite();
    }
    if all {
        Ok(sample)
    } else {
        Err(CoreError::NumericalInvalid {
            quantity: "sensor sample",
            detail: "interpolation produced a nonfinite value".into(),
        })
    }
}

fn finite(sample: RawSample) -> Result<RawSample, CoreError> {
    let mut all = true;
    for plane in 0..PLANES {
        all &= sample.values[plane].is_finite()
            & sample.dx[plane].is_finite()
            & sample.dy[plane].is_finite();
    }
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

/// If `divisor` is a positive normal exact power of two `2^k` whose reciprocal
/// `2^(-k)` is also normal (`-1022 <= k <= 1022`), returns `Some(1.0 / divisor)`.
/// For such divisors, `x * (1.0 / divisor)` is bit-identical to `x / divisor`
/// for every finite `f64` (including `±0.0` and subnormals), because both
/// compute the exact real value `x · 2^(-k)` with single IEEE-754 rounding.
#[inline]
fn exact_reciprocal(divisor: f64) -> Option<f64> {
    let bits = divisor.to_bits();
    let sign = bits >> 63;
    let exp = (bits >> 52) & 0x7FF;
    let frac = bits & 0x000F_FFFF_FFFF_FFFF;
    if sign == 0 && frac == 0 && (1..=2045).contains(&exp) {
        Some(1.0 / divisor)
    } else {
        None
    }
}

#[inline]
fn div_exact(value: f64, divisor: f64, inv: Option<f64>) -> f64 {
    if let Some(r) = inv {
        if r == 1.0 { value } else { value * r }
    } else {
        value / divisor
    }
}

#[inline]
fn wrap_cell_index(index: i64, cells: u32) -> u32 {
    let n = i64::from(cells);
    if index >= 0 && index < n {
        index as u32
    } else if index == -1 {
        cells.saturating_sub(1)
    } else if index == n {
        0
    } else {
        u32::try_from(index.rem_euclid(n)).unwrap_or(0)
    }
}

fn average_base(
    source: &[f32],
    fine_x: usize,
    target: &mut [f64],
    area: f64,
    inv_area: Option<f64>,
) {
    if area == 1.0 {
        average(source, fine_x, target, f64::from);
    } else if let Some(inv) = inv_area {
        average(source, fine_x, target, |energy| f64::from(energy) * inv);
    } else {
        average(source, fine_x, target, |energy| f64::from(energy) / area);
    }
}
