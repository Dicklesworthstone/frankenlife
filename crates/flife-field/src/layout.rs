//! The semantic field layout descriptor (plan §9.3, Appendix B.2).
//!
//! A field buffer is described by its arrangement, shape, byte strides,
//! dtype, base alignment, units and grid. FrankenNumPy supplies the
//! contiguous strides and checks bounds and self-overlap. A reader states the
//! layout it needs. A buffer whose declared layout differs is refused, even
//! when its byte length agrees: the length alone never identifies a layout.

use std::fmt;
use std::fmt::Write as _;

use flife_core::CoreError;
use flife_core::canonical::{Sha256Digest, canonical_float};
use flife_core::units::Unit;
use fnp_dtype::DType;
use fnp_ndarray::{MemoryOrder, NdLayout, ShapeError, contiguous_strides};

use crate::grid::{FIELD_PLANES, FieldGrid};

/// The schema of a field layout descriptor.
pub const FIELD_LAYOUT_SCHEMA: &str = "frankenlife.field-layout.v1";

/// The quantity every plane holds: an amount per cell, not a density. Divide
/// by [`FieldGrid::cell_area`] for a density (§4.3).
pub const CELL_INTEGRATED: &str = "cell-integrated";

/// The unit of every plane. The energy plane holds ecological energy; each
/// moment plane holds an energy-weighted chemical signature in the same unit
/// (§4.2).
pub const PLANE_UNIT: Unit = Unit::Energy;

/// The base alignment of a host field buffer: that of `f32`.
pub const HOST_ALIGNMENT: u64 = 4;

pub(crate) const BUFFER: &str = "field";

/// A named axis of a field buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Axis {
    /// The plane: 0 is energy, `k + 1` is moment channel `k`.
    Plane,
    /// The cell row.
    Y,
    /// The cell column.
    X,
}

impl Axis {
    /// The canonical axis name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Plane => "plane",
            Self::Y => "y",
            Self::X => "x",
        }
    }
}

/// How the 33 planes of a grid are arranged in one buffer. Both are C-order
/// (row-major) over their axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Arrangement {
    /// Axes `[plane, y, x]`. Each plane is one contiguous row-major image,
    /// which suits stencils. This is the authoritative host layout (§9.3).
    Planar,
    /// Axes `[y, x, plane]`. A cell's 33 values are adjacent, which suits
    /// gathers at a few positions.
    Interleaved,
}

impl Arrangement {
    /// Every arrangement this build implements.
    pub const ALL: [Self; 2] = [Self::Planar, Self::Interleaved];

    /// The canonical name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Planar => "planar",
            Self::Interleaved => "interleaved",
        }
    }

    /// The axes, outermost first.
    #[must_use]
    pub const fn axes(self) -> [Axis; 3] {
        match self {
            Self::Planar => [Axis::Plane, Axis::Y, Axis::X],
            Self::Interleaved => [Axis::Y, Axis::X, Axis::Plane],
        }
    }

    /// The arrangement with canonical name `name`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|arrangement| arrangement.name() == name)
    }
}

impl fmt::Display for Arrangement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Who may write a buffer through this descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ownership {
    /// The descriptor of the buffer's owner, its only writer.
    Owned,
    /// A borrowed, read-only view.
    ReadOnlyView,
}

/// The layout of one complete field state: 33 F32 planes over a grid.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldLayout {
    grid: FieldGrid,
    arrangement: Arrangement,
    ownership: Ownership,
    shape: [u64; 3],
    strides: [u64; 3],
    byte_length: u64,
}

impl FieldLayout {
    /// The contiguous owned layout of `grid` in `arrangement`. FrankenNumPy
    /// computes the strides and the byte count, and both are checked against
    /// independent `u64` arithmetic.
    ///
    /// # Errors
    /// [`CoreError::Overflow`] when the buffer would exceed `isize::MAX`
    /// bytes; [`CoreError::LayoutMismatch`] if FrankenNumPy's layout
    /// disagrees with that arithmetic.
    pub fn new(grid: FieldGrid, arrangement: Arrangement) -> Result<Self, CoreError> {
        let item = f32_item_size()?;
        let axes = arrangement.axes();
        let shape = axes.map(|axis| extent(&grid, axis));
        let shape_usize = shape
            .iter()
            .map(|&extent| usize::try_from(extent))
            .collect::<Result<Vec<usize>, _>>()
            .map_err(|_| overflow())?;
        let fnp_strides =
            contiguous_strides(&shape_usize, item, MemoryOrder::C).map_err(shape_err)?;
        let nd = NdLayout::contiguous(shape_usize, item, MemoryOrder::C).map_err(shape_err)?;
        let fnp_bytes = nd.nbytes().map_err(shape_err)?;

        // A Rust slice, like a NumPy array, spans at most `isize::MAX` bytes.
        let byte_length = grid
            .cell_count()
            .checked_mul(u64::from(FIELD_PLANES))
            .and_then(|elements| elements.checked_mul(HOST_ALIGNMENT))
            .filter(|&bytes| isize::try_from(bytes).is_ok())
            .ok_or_else(overflow)?;
        let mut strides = [0_u64; 3];
        let mut stride = HOST_ALIGNMENT;
        for (axis, slot) in shape.iter().zip(strides.iter_mut()).rev() {
            *slot = stride;
            stride = stride.checked_mul(*axis).ok_or_else(overflow)?;
        }
        let agrees = u64::try_from(fnp_bytes).ok() == Some(byte_length)
            && fnp_strides.len() == 3
            && fnp_strides
                .iter()
                .zip(strides)
                .all(|(&got, want)| u64::try_from(got).ok() == Some(want))
            && nd.is_contiguous()
            && !nd.has_internal_overlap();
        if !agrees {
            return Err(mismatch(format!(
                "FrankenNumPy lays out {shape:?} as strides {fnp_strides:?} over {fnp_bytes} \
                 bytes (contiguous: {}, self-overlapping: {}), but the field needs contiguous \
                 strides {strides:?} over {byte_length} bytes",
                nd.is_contiguous(),
                nd.has_internal_overlap()
            )));
        }
        Ok(Self {
            grid,
            arrangement,
            ownership: Ownership::Owned,
            shape,
            strides,
            byte_length,
        })
    }

    /// The authoritative host layout of `grid`.
    ///
    /// # Errors
    /// As [`FieldLayout::new`].
    pub fn planar(grid: FieldGrid) -> Result<Self, CoreError> {
        Self::new(grid, Arrangement::Planar)
    }

    /// The grid.
    #[must_use]
    pub const fn grid(&self) -> &FieldGrid {
        &self.grid
    }

    /// The arrangement.
    #[must_use]
    pub const fn arrangement(&self) -> Arrangement {
        self.arrangement
    }

    /// The ownership this descriptor grants.
    #[must_use]
    pub const fn ownership(&self) -> Ownership {
        self.ownership
    }

    /// The extent of each axis, outermost first.
    #[must_use]
    pub const fn shape(&self) -> [u64; 3] {
        self.shape
    }

    /// The byte stride of each axis, outermost first.
    #[must_use]
    pub const fn strides(&self) -> [u64; 3] {
        self.strides
    }

    /// Bytes in the buffer.
    #[must_use]
    pub const fn byte_length(&self) -> u64 {
        self.byte_length
    }

    /// F32 elements in the buffer: `33 G_x G_y`.
    #[must_use]
    pub const fn element_count(&self) -> u64 {
        self.byte_length / HOST_ALIGNMENT
    }

    /// Bytes in one plane: `4 G_x G_y`.
    #[must_use]
    pub const fn plane_bytes(&self) -> u64 {
        self.byte_length / FIELD_PLANES as u64
    }

    /// The element dtype: real F32 storage, never widened.
    #[must_use]
    pub const fn dtype(&self) -> DType {
        DType::F32
    }

    /// The byte offset of `(plane, x, y)`, or `None` outside the buffer.
    #[must_use]
    pub fn element_offset(&self, plane: u32, x: u32, y: u32) -> Option<u64> {
        if plane >= FIELD_PLANES || x >= self.grid.cells_x() || y >= self.grid.cells_y() {
            return None;
        }
        let mut offset = 0_u64;
        for (axis, stride) in self.arrangement.axes().into_iter().zip(self.strides) {
            let coordinate = match axis {
                Axis::Plane => plane,
                Axis::Y => y,
                Axis::X => x,
            };
            offset = offset.checked_add(u64::from(coordinate).checked_mul(stride)?)?;
        }
        (offset < self.byte_length).then_some(offset)
    }

    /// The F32 element index of `(plane, x, y)`, or `None` outside the
    /// buffer.
    #[must_use]
    pub fn element_index(&self, plane: u32, x: u32, y: u32) -> Option<usize> {
        usize::try_from(self.element_offset(plane, x, y)? / HOST_ALIGNMENT).ok()
    }

    /// The same layout, read-only.
    #[must_use]
    pub fn as_read_only(&self) -> Self {
        Self {
            ownership: Ownership::ReadOnlyView,
            ..self.clone()
        }
    }

    /// FrankenNumPy's view of this layout. It is writeable only for the
    /// owner.
    ///
    /// # Errors
    /// [`CoreError::Overflow`] if the shape exceeds `usize`.
    pub fn nd_layout(&self) -> Result<NdLayout, CoreError> {
        let shape = self
            .shape
            .iter()
            .map(|&extent| usize::try_from(extent))
            .collect::<Result<Vec<usize>, _>>()
            .map_err(|_| overflow())?;
        let layout =
            NdLayout::contiguous(shape, f32_item_size()?, MemoryOrder::C).map_err(shape_err)?;
        Ok(match self.ownership {
            Ownership::Owned => layout,
            Ownership::ReadOnlyView => layout.as_read_only(),
        })
    }

    /// Refuse a buffer laid out as `found` where this layout is required.
    /// Everything but ownership must agree: arrangement, grid, cell size,
    /// shape, strides and length.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] naming the first difference.
    pub fn ensure_compatible(&self, found: &Self) -> Result<(), CoreError> {
        let length_note = if found.byte_length == self.byte_length {
            "; the byte lengths agree, but length never identifies a layout"
        } else {
            ""
        };
        let (expected_grid, found_grid) = (&self.grid, &found.grid);
        let detail = if found.arrangement != self.arrangement {
            format!(
                "the {} arrangement where {} is required{length_note}",
                found.arrangement, self.arrangement
            )
        } else if (found_grid.cells_x(), found_grid.cells_y())
            != (expected_grid.cells_x(), expected_grid.cells_y())
        {
            format!(
                "a {}×{} grid where {}×{} is required{length_note}",
                found_grid.cells_x(),
                found_grid.cells_y(),
                expected_grid.cells_x(),
                expected_grid.cells_y()
            )
        } else if found_grid.world_width().to_bits() != expected_grid.world_width().to_bits()
            || found_grid.world_height().to_bits() != expected_grid.world_height().to_bits()
        {
            format!(
                "a {:?}×{:?} world where {:?}×{:?} is required: the cell size, and so every \
                 cell's energy density, differs (§4.3){length_note}",
                found_grid.world_width(),
                found_grid.world_height(),
                expected_grid.world_width(),
                expected_grid.world_height()
            )
        } else if found.shape != self.shape
            || found.strides != self.strides
            || found.byte_length != self.byte_length
        {
            format!(
                "shape {:?}, strides {:?} over {} bytes where shape {:?}, strides {:?} over {} \
                 bytes is required",
                found.shape,
                found.strides,
                found.byte_length,
                self.shape,
                self.strides,
                self.byte_length
            )
        } else {
            return Ok(());
        };
        Err(mismatch(detail))
    }

    /// The declaration a checkpoint or interchange record stores. It omits
    /// ownership, which is a property of a live handle.
    #[must_use]
    pub fn describe(&self) -> DeclaredLayout {
        DeclaredLayout {
            schema: FIELD_LAYOUT_SCHEMA.into(),
            dtype: DType::F32.name().into(),
            item_size: HOST_ALIGNMENT,
            arrangement: self.arrangement.name().into(),
            axes: self
                .arrangement
                .axes()
                .iter()
                .map(|axis| axis.name().to_owned())
                .collect(),
            shape: self.shape.to_vec(),
            strides: self
                .strides
                .iter()
                .map(|&stride| i64::try_from(stride).unwrap_or(i64::MAX))
                .collect(),
            byte_length: self.byte_length,
            alignment: HOST_ALIGNMENT,
            unit: PLANE_UNIT.name().into(),
            measure: CELL_INTEGRATED.into(),
            world_width: self.grid.world_width(),
            world_height: self.grid.world_height(),
        }
    }

    /// Admit a declared layout, for example from a checkpoint manifest,
    /// before allocating anything for it (Appendix B.2). The result is the
    /// owned layout of the buffer its reader will load.
    ///
    /// # Errors
    /// [`CoreError::SchemaMismatch`] for another schema;
    /// [`CoreError::LayoutMismatch`] for another dtype, axis order, plane
    /// count, unit or measure, a view outside its byte length, overlapping
    /// or noncanonical strides, or an inconsistent byte length;
    /// [`CoreError::NumericalInvalid`] for an impossible grid;
    /// [`CoreError::Overflow`] past the address space.
    pub fn from_declared(declared: &DeclaredLayout) -> Result<Self, CoreError> {
        if declared.schema != FIELD_LAYOUT_SCHEMA {
            return Err(CoreError::SchemaMismatch {
                expected: FIELD_LAYOUT_SCHEMA.into(),
                found: declared.schema.clone(),
            });
        }
        if declared.dtype != DType::F32.name() || declared.item_size != HOST_ALIGNMENT {
            let parsed = DType::parse(&declared.dtype).map(DType::item_size);
            return Err(mismatch(format!(
                "dtype `{}` with {}-byte items (FrankenNumPy item size {parsed:?}); the field is \
                 real F32 storage and is never widened or narrowed",
                declared.dtype, declared.item_size
            )));
        }
        let arrangement = Arrangement::parse(&declared.arrangement)
            .ok_or_else(|| mismatch(format!("unknown arrangement `{}`", declared.arrangement)))?;
        let axes = arrangement.axes();
        let axes_agree = declared.axes.len() == axes.len()
            && declared
                .axes
                .iter()
                .zip(axes)
                .all(|(name, axis)| name == axis.name());
        if !axes_agree {
            return Err(mismatch(format!(
                "axes {:?} for a {arrangement} buffer, whose axes are {:?}",
                declared.axes,
                axes.map(Axis::name)
            )));
        }
        let shape = <[u64; 3]>::try_from(declared.shape.as_slice())
            .map_err(|_| mismatch(format!("a rank-{} shape for 3 axes", declared.shape.len())))?;
        let declared_strides = <[i64; 3]>::try_from(declared.strides.as_slice())
            .map_err(|_| mismatch(format!("{} strides for 3 axes", declared.strides.len())))?;
        let extent_of = |wanted: Axis| {
            axes.iter()
                .zip(shape)
                .find_map(|(&axis, extent)| (axis == wanted).then_some(extent))
                .unwrap_or(0)
        };
        if extent_of(Axis::Plane) != u64::from(FIELD_PLANES) {
            return Err(mismatch(format!(
                "{} planes where {FIELD_PLANES} (energy and 32 moments) are required",
                extent_of(Axis::Plane)
            )));
        }
        if declared.unit != PLANE_UNIT.name() || declared.measure != CELL_INTEGRATED {
            return Err(mismatch(format!(
                "planes in `{}` as `{}` where `{}` as `{CELL_INTEGRATED}` is required",
                declared.unit,
                declared.measure,
                PLANE_UNIT.name()
            )));
        }
        if declared.alignment != HOST_ALIGNMENT {
            return Err(mismatch(format!(
                "{}-byte base alignment where a host field declares {HOST_ALIGNMENT}",
                declared.alignment
            )));
        }
        let cells = |axis: Axis| {
            u32::try_from(extent_of(axis)).map_err(|_| {
                mismatch(format!(
                    "{} {} cells exceed u32",
                    extent_of(axis),
                    axis.name()
                ))
            })
        };
        let grid = FieldGrid::new(
            cells(Axis::X)?,
            cells(Axis::Y)?,
            declared.world_width,
            declared.world_height,
        )?;
        let expected = Self::new(grid, arrangement)?;

        // FrankenNumPy's bounds and self-overlap check of the declared view
        // over a flat base of exactly the declared bytes.
        if !declared.byte_length.is_multiple_of(HOST_ALIGNMENT) {
            return Err(mismatch(format!(
                "{} bytes is not a whole number of F32 elements",
                declared.byte_length
            )));
        }
        let item = f32_item_size()?;
        let base_elements =
            usize::try_from(declared.byte_length / HOST_ALIGNMENT).map_err(|_| overflow())?;
        let base =
            NdLayout::contiguous(vec![base_elements], item, MemoryOrder::C).map_err(shape_err)?;
        let view_shape = shape
            .iter()
            .map(|&extent| usize::try_from(extent))
            .collect::<Result<Vec<usize>, _>>()
            .map_err(|_| overflow())?;
        let view_strides = declared_strides
            .iter()
            .map(|&stride| isize::try_from(stride))
            .collect::<Result<Vec<isize>, _>>()
            .map_err(|_| overflow())?;
        let view = base
            .as_strided(view_shape, view_strides)
            .map_err(shape_err)?;
        if view.has_internal_overlap() {
            return Err(mismatch(format!(
                "strides {declared_strides:?} make distinct elements share storage"
            )));
        }
        let canonical = declared_strides
            .iter()
            .zip(expected.strides)
            .all(|(&got, want)| u64::try_from(got).ok() == Some(want));
        if !canonical {
            return Err(mismatch(format!(
                "strides {declared_strides:?} where the contiguous {arrangement} strides are {:?}; \
                 padded, permuted or reversed strides are not admitted by {FIELD_LAYOUT_SCHEMA}",
                expected.strides
            )));
        }
        if declared.byte_length != expected.byte_length {
            return Err(mismatch(format!(
                "{} bytes declared where the shape needs exactly {}",
                declared.byte_length, expected.byte_length
            )));
        }
        Ok(expected)
    }
}

/// A field layout as a checkpoint manifest or interchange record states it
/// (Appendix B.2). It is plain data: [`FieldLayout::from_declared`] admits
/// it before anything is allocated.
#[derive(Debug, Clone, PartialEq)]
pub struct DeclaredLayout {
    /// [`FIELD_LAYOUT_SCHEMA`].
    pub schema: String,
    /// The FrankenNumPy dtype name, `f32`.
    pub dtype: String,
    /// Bytes per element.
    pub item_size: u64,
    /// The [`Arrangement`] name.
    pub arrangement: String,
    /// The [`Axis`] names, outermost first.
    pub axes: Vec<String>,
    /// The extent of each axis.
    pub shape: Vec<u64>,
    /// The byte stride of each axis.
    pub strides: Vec<i64>,
    /// Bytes in the buffer.
    pub byte_length: u64,
    /// The base alignment in bytes.
    pub alignment: u64,
    /// The unit of every plane.
    pub unit: String,
    /// Whether planes hold cell amounts or densities.
    pub measure: String,
    /// `L_x` in world-length.
    pub world_width: f64,
    /// `L_y` in world-length.
    pub world_height: f64,
}

impl DeclaredLayout {
    /// Canonical tab-separated `key value` lines in a fixed order.
    #[must_use]
    pub fn canonical_text(&self) -> String {
        let join = |values: &[String]| values.join(",");
        let numbers = |values: &[u64]| join(&values.iter().map(u64::to_string).collect::<Vec<_>>());
        let signed = |values: &[i64]| join(&values.iter().map(i64::to_string).collect::<Vec<_>>());
        let mut text = String::new();
        for (key, value) in [
            ("schema", self.schema.clone()),
            ("dtype", self.dtype.clone()),
            ("item_size", self.item_size.to_string()),
            ("arrangement", self.arrangement.clone()),
            ("axes", join(&self.axes)),
            ("shape", numbers(&self.shape)),
            ("strides", signed(&self.strides)),
            ("byte_length", self.byte_length.to_string()),
            ("alignment", self.alignment.to_string()),
            ("unit", self.unit.clone()),
            ("measure", self.measure.clone()),
            ("world_width", canonical_float(self.world_width)),
            ("world_height", canonical_float(self.world_height)),
        ] {
            let _ = writeln!(text, "{key}\t{value}");
        }
        text
    }

    /// The SHA-256 of [`DeclaredLayout::canonical_text`].
    #[must_use]
    pub fn digest(&self) -> Sha256Digest {
        Sha256Digest::of(self.canonical_text().as_bytes())
    }
}

fn extent(grid: &FieldGrid, axis: Axis) -> u64 {
    match axis {
        Axis::Plane => u64::from(FIELD_PLANES),
        Axis::Y => u64::from(grid.cells_y()),
        Axis::X => u64::from(grid.cells_x()),
    }
}

/// FrankenNumPy's F32 item size, which must be `size_of::<f32>()`.
fn f32_item_size() -> Result<usize, CoreError> {
    let item = DType::F32.item_size();
    if item != std::mem::size_of::<f32>() || u64::try_from(item).ok() != Some(HOST_ALIGNMENT) {
        return Err(mismatch(format!(
            "FrankenNumPy sizes f32 at {item} bytes, not {}",
            std::mem::size_of::<f32>()
        )));
    }
    Ok(item)
}

pub(crate) fn mismatch(detail: String) -> CoreError {
    CoreError::LayoutMismatch {
        buffer: BUFFER,
        detail,
    }
}

pub(crate) const fn overflow() -> CoreError {
    CoreError::Overflow {
        quantity: "field buffer bytes",
    }
}

fn shape_err(error: ShapeError) -> CoreError {
    match error {
        ShapeError::Overflow => overflow(),
        other => mismatch(format!("FrankenNumPy refused the layout: {other}")),
    }
}
