//! Owned F32 field storage, read-only views and the ping-pong pair
//! (plan §4.2, §9.3, §9.5).
//!
//! The authoritative state is one planar buffer of `33 G_x G_y` real `f32`
//! values. Plane 0 is energy and plane `k + 1` is moment channel `k`; each
//! plane is a row-major image. A reader that does not own the buffer gets a
//! [`FieldView`], which checks the buffer against its declared layout.
//! Converting to another arrangement goes through [`relayout`], which copies
//! each value to its semantic index.

use flife_core::model::MOMENT_CHANNELS;
use flife_core::{CoreError, ValidatedConfig};
use fnp_dtype::ArrayStorage;

use crate::footprint::BufferLimits;
use crate::grid::{ENERGY_PLANE, FIELD_PLANES, FieldGrid};
use crate::layout::{BUFFER, FieldLayout, HOST_ALIGNMENT, mismatch, overflow};

/// Moment channels per cell, as an array length.
pub const CHANNELS: usize = MOMENT_CHANNELS as usize;

/// One cell's values: its energy `e` and energy-weighted moment `m`
/// (§4.2).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    /// Spendable ecological energy in the cell.
    pub energy: f32,
    /// The energy-weighted chemical signature.
    pub moments: [f32; CHANNELS],
}

impl Cell {
    /// A cell with no energy and no moment.
    pub const EMPTY: Self = Self {
        energy: 0.0,
        moments: [0.0; CHANNELS],
    };
}

/// The only initial composition `field-ecology-v1` defines: no moment.
pub const ZERO_COMPOSITION: &str = "zero";

/// The energy an initial field holds (§4.4: external injection).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InitialEnergy {
    /// The configured energy per cell.
    pub declared_per_cell: f64,
    /// The energy per cell actually stored in F32.
    pub stored_per_cell: f32,
    /// `cells × stored_per_cell` in F64. It is exact, and equal to the
    /// cell-order F64 sum of the stored field, whenever the grid has at most
    /// 2^29 cells: a 24-bit F32 significand times a 29-bit count fits F64's 53
    /// bits.
    pub injected_total: f64,
}

impl InitialEnergy {
    fn new(declared_per_cell: f64, stored_per_cell: f32, cells: u64) -> Self {
        // Exact for every grid of at most 2^53 cells.
        let count = cells as f64;
        Self {
            declared_per_cell,
            stored_per_cell,
            injected_total: f64::from(stored_per_cell) * count,
        }
    }
}

/// One complete field state in the authoritative planar layout. Its owner is
/// its only writer.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldState {
    layout: FieldLayout,
    data: Vec<f32>,
}

impl FieldState {
    /// An all-zero field on `grid`. The buffer is admitted against `limits`
    /// before it is allocated, and a refused allocation is reported, not
    /// aborted.
    ///
    /// # Errors
    /// [`CoreError::BudgetExhausted`] above `limits`;
    /// [`CoreError::AllocationFailed`] if the host refuses;
    /// [`CoreError::Overflow`] past the address space.
    pub fn zeroed(grid: FieldGrid, limits: &BufferLimits) -> Result<Self, CoreError> {
        let layout = FieldLayout::planar(grid)?;
        limits.admit("field buffer bytes", layout.byte_length())?;
        let data = zeroed_f32(&layout)?;
        Ok(Self { layout, data })
    }

    /// The initial field a validated configuration declares
    /// (`[field_initialization]`, §4.2, §4.4): `energy_per_cell` in every
    /// cell and, for the `zero` composition, no chemical moment.
    ///
    /// The configured energy is stored in F32. The returned
    /// [`InitialEnergy`] carries the stored value, and its exact total is the
    /// declared external injection for the ledger. Rounding to F32 is
    /// recorded there rather than hidden.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] for an unimplemented grid or
    /// composition; [`CoreError::NumericalInvalid`] for an energy that is
    /// negative, nonfinite or beyond F32; and the errors of
    /// [`FieldState::zeroed`].
    pub fn initial(
        config: &ValidatedConfig,
        limits: &BufferLimits,
    ) -> Result<(Self, InitialEnergy), CoreError> {
        let initialization = &config.config().field_initialization;
        if initialization.composition != ZERO_COMPOSITION {
            return Err(CoreError::MissingCapability {
                capability: format!("field composition `{}`", initialization.composition),
            });
        }
        let declared = initialization.energy_per_cell;
        // Round to nearest: F32 is the declared field dtype, and the stored
        // value is what the ledger records.
        let stored = declared as f32;
        if !declared.is_finite() || declared < 0.0 || !stored.is_finite() {
            return Err(CoreError::NumericalInvalid {
                quantity: "field_initialization.energy_per_cell",
                detail: format!("{declared:?} is not a finite nonnegative F32 energy"),
            });
        }
        let grid = FieldGrid::from_config(config)?;
        let mut state = Self::zeroed(grid, limits)?;
        state.energy_mut().fill(stored);
        let energy = InitialEnergy::new(declared, stored, grid.cell_count());
        Ok((state, energy))
    }

    /// The owned planar layout.
    #[must_use]
    pub const fn layout(&self) -> &FieldLayout {
        &self.layout
    }

    /// The grid.
    #[must_use]
    pub const fn grid(&self) -> &FieldGrid {
        self.layout.grid()
    }

    /// Every value, in planar order.
    #[must_use]
    pub fn as_slice(&self) -> &[f32] {
        &self.data
    }

    /// Bytes the allocation holds: its capacity times `size_of::<f32>()`.
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        u64::try_from(self.data.capacity())
            .ok()
            .and_then(|elements| elements.checked_mul(HOST_ALIGNMENT))
            .unwrap_or(u64::MAX)
    }

    /// Plane `plane` as a row-major image, or `None` past plane 32.
    #[must_use]
    pub fn plane(&self, plane: u32) -> Option<&[f32]> {
        let range = self.plane_range(plane)?;
        self.data.get(range)
    }

    /// Plane `plane`, writable.
    #[must_use]
    pub fn plane_mut(&mut self, plane: u32) -> Option<&mut [f32]> {
        let range = self.plane_range(plane)?;
        self.data.get_mut(range)
    }

    /// The energy plane.
    #[must_use]
    pub fn energy(&self) -> &[f32] {
        self.plane(ENERGY_PLANE).unwrap_or(&[])
    }

    /// The energy plane, writable.
    #[must_use]
    pub fn energy_mut(&mut self) -> &mut [f32] {
        self.plane_mut(ENERGY_PLANE).unwrap_or(&mut [])
    }

    /// Moment channel `channel`, or `None` past channel 31.
    #[must_use]
    pub fn moment(&self, channel: u32) -> Option<&[f32]> {
        self.plane(
            channel
                .checked_add(1)
                .filter(|&plane| plane < FIELD_PLANES)?,
        )
    }

    /// Moment channel `channel`, writable.
    #[must_use]
    pub fn moment_mut(&mut self, channel: u32) -> Option<&mut [f32]> {
        self.plane_mut(
            channel
                .checked_add(1)
                .filter(|&plane| plane < FIELD_PLANES)?,
        )
    }

    /// Cell `(x, y)`, or `None` outside the grid.
    #[must_use]
    pub fn cell(&self, x: u32, y: u32) -> Option<Cell> {
        let index = self.grid().cell_index(x, y)?;
        let mut cell = Cell::EMPTY;
        cell.energy = *self.energy().get(index)?;
        for (channel, value) in (0_u32..).zip(cell.moments.iter_mut()) {
            *value = *self.moment(channel)?.get(index)?;
        }
        Some(cell)
    }

    /// Overwrite cell `(x, y)`; `None` outside the grid, leaving the field
    /// unchanged.
    #[must_use]
    pub fn set_cell(&mut self, x: u32, y: u32, cell: &Cell) -> Option<()> {
        let index = self.grid().cell_index(x, y)?;
        *self.energy_mut().get_mut(index)? = cell.energy;
        for (channel, &value) in (0_u32..).zip(cell.moments.iter()) {
            *self.moment_mut(channel)?.get_mut(index)? = value;
        }
        Some(())
    }

    /// A read-only view.
    #[must_use]
    pub fn view(&self) -> FieldView<'_> {
        FieldView {
            layout: self.layout.as_read_only(),
            data: &self.data,
        }
    }

    /// Overwrite this state with `source`, which must have the same layout.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for a different layout.
    pub fn copy_from(&mut self, source: &Self) -> Result<(), CoreError> {
        self.layout.ensure_compatible(&source.layout)?;
        self.data.copy_from_slice(&source.data);
        Ok(())
    }

    /// Hand the buffer to FrankenNumPy as `ArrayStorage::F32` with its
    /// layout. Nothing is copied or widened.
    #[must_use]
    pub fn into_array_storage(self) -> (ArrayStorage, FieldLayout) {
        (ArrayStorage::F32(self.data), self.layout)
    }

    /// Take a FrankenNumPy buffer declared as `layout`, which must be the
    /// planar layout of its grid.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for another arrangement or grid, storage
    /// of another dtype (an F64 copy is refused, not narrowed), or a length
    /// that disagrees with the layout.
    pub fn from_array_storage(
        storage: ArrayStorage,
        layout: &FieldLayout,
    ) -> Result<Self, CoreError> {
        let expected = FieldLayout::planar(*layout.grid())?;
        expected.ensure_compatible(layout)?;
        let ArrayStorage::F32(data) = storage else {
            return Err(mismatch(format!(
                "{} storage where real f32 storage is required; the field is never widened or \
                 narrowed",
                storage.dtype().name()
            )));
        };
        let elements = u64::try_from(data.len()).map_err(|_| overflow())?;
        if elements != expected.element_count() {
            return Err(mismatch(format!(
                "{elements} f32 values for a layout of {}",
                expected.element_count()
            )));
        }
        Ok(Self {
            layout: expected,
            data,
        })
    }

    fn plane_range(&self, plane: u32) -> Option<std::ops::Range<usize>> {
        if plane >= FIELD_PLANES {
            return None;
        }
        let cells = usize::try_from(self.grid().cell_count()).ok()?;
        let start = usize::try_from(plane).ok()?.checked_mul(cells)?;
        Some(start..start.checked_add(cells)?)
    }
}

/// A read-only field buffer checked against its declared layout.
#[derive(Debug, Clone)]
pub struct FieldView<'a> {
    layout: FieldLayout,
    data: &'a [f32],
}

impl<'a> FieldView<'a> {
    /// View `data` as `layout`. The slice must hold exactly the layout's
    /// bytes, starting at its declared alignment.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for a wrong length or misaligned base.
    pub fn new(data: &'a [f32], layout: &FieldLayout) -> Result<Self, CoreError> {
        let bytes = u64::try_from(std::mem::size_of_val(data)).map_err(|_| overflow())?;
        if bytes != layout.byte_length() {
            return Err(mismatch(format!(
                "a {bytes}-byte buffer declared as a {}-byte {} layout",
                layout.byte_length(),
                layout.arrangement()
            )));
        }
        let address = u64::try_from(data.as_ptr().addr()).map_err(|_| overflow())?;
        if !address.is_multiple_of(HOST_ALIGNMENT) {
            return Err(mismatch(format!(
                "base address {address:#x} is not {HOST_ALIGNMENT}-byte aligned"
            )));
        }
        Ok(Self {
            layout: layout.as_read_only(),
            data,
        })
    }

    /// The declared layout, read-only.
    #[must_use]
    pub const fn layout(&self) -> &FieldLayout {
        &self.layout
    }

    /// The raw values in the declared arrangement.
    #[must_use]
    pub const fn as_slice(&self) -> &'a [f32] {
        self.data
    }

    /// The value at `(plane, x, y)`, located through the declared strides;
    /// `None` outside the buffer.
    #[must_use]
    pub fn get(&self, plane: u32, x: u32, y: u32) -> Option<f32> {
        self.data
            .get(self.layout.element_index(plane, x, y)?)
            .copied()
    }

    /// Refuse this view unless it has the layout a reader requires.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] naming the first difference.
    pub fn require(&self, required: &FieldLayout) -> Result<(), CoreError> {
        required.ensure_compatible(&self.layout)
    }
}

/// Copy `source` into a new buffer laid out as `target`, which must be a
/// canonical layout of the same grid. Each value moves to its semantic
/// `(plane, x, y)` index; nothing is inferred from lengths.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] when `target` is not a layout of the
/// source grid; [`CoreError::AllocationFailed`] if the host refuses.
pub fn relayout(source: &FieldView<'_>, target: &FieldLayout) -> Result<Vec<f32>, CoreError> {
    let grid = *source.layout().grid();
    FieldLayout::new(grid, target.arrangement())?.ensure_compatible(target)?;
    let mut out = zeroed_f32(target)?;
    for y in 0..grid.cells_y() {
        for x in 0..grid.cells_x() {
            for plane in 0..FIELD_PLANES {
                let moved = source.get(plane, x, y).and_then(|value| {
                    let slot = out.get_mut(target.element_index(plane, x, y)?)?;
                    *slot = value;
                    Some(())
                });
                if moved.is_none() {
                    return Err(mismatch(format!(
                        "({plane}, {x}, {y}) is outside the {} or {} buffer",
                        source.layout().arrangement(),
                        target.arrangement()
                    )));
                }
            }
        }
    }
    Ok(out)
}

/// The current committed field and the next candidate (§9.5). A tick reads
/// `current` and writes `next`; [`FieldPair::commit`] then swaps them, so
/// neither buffer is reallocated in steady state.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldPair {
    current: FieldState,
    next: FieldState,
}

impl FieldPair {
    /// Two all-zero states on `grid`, each admitted against `limits`.
    ///
    /// # Errors
    /// As [`FieldState::zeroed`].
    pub fn zeroed(grid: FieldGrid, limits: &BufferLimits) -> Result<Self, CoreError> {
        Ok(Self {
            current: FieldState::zeroed(grid, limits)?,
            next: FieldState::zeroed(grid, limits)?,
        })
    }

    /// The committed field.
    #[must_use]
    pub const fn current(&self) -> &FieldState {
        &self.current
    }

    /// The committed field, writable. Only for initialization and edits
    /// applied between ticks.
    #[must_use]
    pub const fn current_mut(&mut self) -> &mut FieldState {
        &mut self.current
    }

    /// Read the committed field while writing the candidate.
    #[must_use]
    pub const fn stage(&mut self) -> (&FieldState, &mut FieldState) {
        (&self.current, &mut self.next)
    }

    /// Make the candidate the committed field. The old committed buffer
    /// becomes the next candidate.
    pub const fn commit(&mut self) {
        std::mem::swap(&mut self.current, &mut self.next);
    }

    /// Bytes both allocations hold.
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        self.current
            .allocated_bytes()
            .saturating_add(self.next.allocated_bytes())
    }
}

/// A zero-filled `f32` buffer of exactly `layout`'s elements. The
/// reservation is fallible, so an allocation refusal is an error.
fn zeroed_f32(layout: &FieldLayout) -> Result<Vec<f32>, CoreError> {
    let elements = usize::try_from(layout.element_count()).map_err(|_| overflow())?;
    let mut data = Vec::new();
    data.try_reserve_exact(elements)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: BUFFER,
            bytes: layout.byte_length(),
        })?;
    data.resize(elements, 0.0);
    Ok(data)
}
