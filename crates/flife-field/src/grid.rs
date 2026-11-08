//! The periodic field grid (plan §4.1, §4.3).
//!
//! The world is the torus `[0, L_x) × [0, L_y)`, covered by a regular
//! `G_x × G_y` grid of cells `h_x = L_x / G_x` wide and `h_y = L_y / G_y`
//! high. Cells are numbered row-major: `x` varies fastest.

use flife_core::model::MOMENT_CHANNELS;
use flife_core::{CoreError, ValidatedConfig};

/// Planes per field: one energy plane and 32 moment planes (§4.2, §9.3).
pub const FIELD_PLANES: u32 = 1 + MOMENT_CHANNELS;

/// The energy plane's index. Moment channel `k` is plane `k + 1`.
pub const ENERGY_PLANE: u32 = 0;

/// The only boundary `field-ecology-v1` defines (§4.1).
pub const PERIODIC: &str = "periodic";

/// The only field dtype `field-ecology-v1` defines (§4.2, §9.3).
pub const F32: &str = "f32";

/// A regular grid over a periodic world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FieldGrid {
    cells_x: u32,
    cells_y: u32,
    world_width: f64,
    world_height: f64,
    cell_width: f64,
    cell_height: f64,
}

impl FieldGrid {
    /// A `cells_x × cells_y` grid over a `world_width × world_height` torus.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a zero cell count, or a world
    /// side or cell side that is not finite and positive.
    pub fn new(
        cells_x: u32,
        cells_y: u32,
        world_width: f64,
        world_height: f64,
    ) -> Result<Self, CoreError> {
        let cell_width = side("grid width", cells_x, world_width)?;
        let cell_height = side("grid height", cells_y, world_height)?;
        Ok(Self {
            cells_x,
            cells_y,
            world_width,
            world_height,
            cell_width,
            cell_height,
        })
    }

    /// The grid a validated configuration declares.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] for a boundary, field dtype or
    /// moment-channel count this crate does not implement, and the errors of
    /// [`FieldGrid::new`].
    pub fn from_config(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let world = &config.config().world;
        if world.boundary != PERIODIC {
            return Err(CoreError::MissingCapability {
                capability: format!("field boundary `{}`", world.boundary),
            });
        }
        if world.field_dtype != F32 {
            return Err(CoreError::MissingCapability {
                capability: format!("field dtype `{}`", world.field_dtype),
            });
        }
        if world.moment_channels != MOMENT_CHANNELS {
            return Err(CoreError::MissingCapability {
                capability: format!("{} moment channels", world.moment_channels),
            });
        }
        Self::new(
            world.grid_width,
            world.grid_height,
            world.width,
            world.height,
        )
    }

    /// Cells along x.
    #[must_use]
    pub const fn cells_x(&self) -> u32 {
        self.cells_x
    }

    /// Cells along y.
    #[must_use]
    pub const fn cells_y(&self) -> u32 {
        self.cells_y
    }

    /// Cells in the grid. A `u32 × u32` product always fits in `u64`.
    #[must_use]
    pub const fn cell_count(&self) -> u64 {
        self.cells_x as u64 * self.cells_y as u64
    }

    /// `L_x` in world-length.
    #[must_use]
    pub const fn world_width(&self) -> f64 {
        self.world_width
    }

    /// `L_y` in world-length.
    #[must_use]
    pub const fn world_height(&self) -> f64 {
        self.world_height
    }

    /// `h_x = L_x / G_x` in world-length.
    #[must_use]
    pub const fn cell_width(&self) -> f64 {
        self.cell_width
    }

    /// `h_y = L_y / G_y` in world-length.
    #[must_use]
    pub const fn cell_height(&self) -> f64 {
        self.cell_height
    }

    /// `h_x h_y` in world-length². A cell amount divided by this is a
    /// density (§4.3).
    #[must_use]
    pub fn cell_area(&self) -> f64 {
        self.cell_width * self.cell_height
    }

    /// The row-major index `y G_x + x` of cell `(x, y)`, or `None` outside
    /// the grid.
    #[must_use]
    pub fn cell_index(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.cells_x || y >= self.cells_y {
            return None;
        }
        let index = u64::from(y) * u64::from(self.cells_x) + u64::from(x);
        usize::try_from(index).ok()
    }

    /// The cell an unbounded integer cell coordinate wraps to on the torus,
    /// for stencils and exchange footprints.
    #[must_use]
    pub fn wrap_cell(&self, x: i64, y: i64) -> (u32, u32) {
        (wrap_index(x, self.cells_x), wrap_index(y, self.cells_y))
    }

    /// A world position wrapped into `[0, L_x) × [0, L_y)`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite coordinate.
    pub fn wrap_position(&self, position: [f64; 2]) -> Result<[f64; 2], CoreError> {
        Ok([
            wrap_coordinate(position[0], self.world_width)?,
            wrap_coordinate(position[1], self.world_height)?,
        ])
    }

    /// The minimum-image displacement from `from` to `to`, two wrapped
    /// positions; each component lies in `[-L/2, L/2)`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] when a position is not wrapped.
    pub fn displacement(&self, from: [f64; 2], to: [f64; 2]) -> Result<[f64; 2], CoreError> {
        Ok([
            minimum_image(from[0], to[0], self.world_width)?,
            minimum_image(from[1], to[1], self.world_height)?,
        ])
    }
}

fn side(quantity: &'static str, cells: u32, length: f64) -> Result<f64, CoreError> {
    if cells == 0 {
        return Err(CoreError::NumericalInvalid {
            quantity,
            detail: "0 cells; a grid side needs at least one".into(),
        });
    }
    let cell = length / f64::from(cells);
    if !length.is_finite() || length <= 0.0 || !cell.is_finite() || cell <= 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity,
            detail: format!(
                "a {length:?} world side over {cells} cells is not a finite positive cell"
            ),
        });
    }
    Ok(cell)
}

fn wrap_index(coordinate: i64, cells: u32) -> u32 {
    // `rem_euclid` lies in `[0, cells)`, which a `u32` always holds.
    let wrapped = coordinate.rem_euclid(i64::from(cells));
    u32::try_from(wrapped).unwrap_or(0)
}

/// `coordinate` wrapped into `[0, length)`.
///
/// A negative coordinate within rounding of zero, such as `-1e-20`, would
/// otherwise wrap to `length` itself; it maps to `0.0`, as does `-0.0`.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for a nonfinite coordinate or a
/// nonpositive length.
pub fn wrap_coordinate(coordinate: f64, length: f64) -> Result<f64, CoreError> {
    if !coordinate.is_finite() || !length.is_finite() || length <= 0.0 {
        return Err(CoreError::NumericalInvalid {
            quantity: "world coordinate",
            detail: format!("{coordinate:?} cannot wrap onto a {length:?} period"),
        });
    }
    let wrapped = coordinate.rem_euclid(length);
    if wrapped >= length || wrapped == 0.0 {
        return Ok(0.0);
    }
    Ok(wrapped)
}

/// The minimum-image displacement `to − from` on a period `length`, for two
/// coordinates already in `[0, length)`. The result lies in
/// `[-length/2, length/2)`.
///
/// Given the rounded difference `d = to − from`, the wrap adds no further
/// rounding: `d − length` is applied only when `length/2 ≤ d < length`, and
/// `d + length` only when `−length < d < −length/2`, and Sterbenz's lemma
/// makes both exact.
///
/// # Errors
/// [`CoreError::NumericalInvalid`] when a coordinate lies outside
/// `[0, length)`.
pub fn minimum_image(from: f64, to: f64, length: f64) -> Result<f64, CoreError> {
    let wrapped = |value: f64| value.is_finite() && (0.0..length).contains(&value);
    if !length.is_finite() || length <= 0.0 || !wrapped(from) || !wrapped(to) {
        return Err(CoreError::NumericalInvalid {
            quantity: "world displacement",
            detail: format!("{from:?} → {to:?} are not both in [0, {length:?})"),
        });
    }
    let half = 0.5 * length;
    let difference = to - from;
    Ok(if difference >= half {
        difference - length
    } else if difference < -half {
        difference + length
    } else {
        difference
    })
}
