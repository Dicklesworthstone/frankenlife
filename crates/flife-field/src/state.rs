//! Owned F32 field storage, read-only views and the ping-pong pair
//! (plan §4.2, §9.3, §9.5).
//!
//! The authoritative state is one planar buffer of `33 G_x G_y` real `f32`
//! values. Plane 0 is energy and plane `k + 1` is moment channel `k`; each
//! plane is a row-major image. A reader that does not own the buffer gets a
//! [`FieldView`], which checks the buffer against its declared layout.
//! Converting to another arrangement goes through [`relayout`], which copies
//! each value to its semantic index.

use flife_core::memory::capacity_bytes;
use flife_core::model::MOMENT_CHANNELS;
use flife_core::{CoreError, V2Feature, ValidatedConfig};
use fnp_dtype::ArrayStorage;

use crate::footprint::BufferLimits;
use crate::grid::{
    CARRIER0_MOMENT_CHANNELS, CARRIER1_ENERGY_PLANE, CARRIER1_FIRST_MOMENT_PLANE,
    CARRIER1_MOMENT_CHANNELS, CarrierLayout, DUAL_CARRIER_PLANES, ENERGY_PLANE, FIELD_PLANES,
    FieldGrid, field_planes_for_config,
};
use crate::layout::{BUFFER, FieldLayout, HOST_ALIGNMENT, mismatch, overflow};

/// Moment channels per cell, as an array length.
pub const CHANNELS: usize = MOMENT_CHANNELS as usize;

/// Default initial edge conductance `g_0` for `V2-14` (`vascular-field`).
pub const VASCULAR_INITIAL_CONDUCTANCE: f32 = 0.25;

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
    /// Optional `V2-14` (`vascular-field`) east-face conductances `g^E_c ∈ [g_min, 1.0]`.
    /// Empty (`0 B`) when disabled. Stored on both `FieldPair` buffers for `C.8` cancel-safety.
    edge_x_cond: Vec<f32>,
    /// Optional `V2-14` (`vascular-field`) north-face conductances `g^N_c ∈ [g_min, 1.0]`.
    /// Empty (`0 B`) when disabled.
    edge_y_cond: Vec<f32>,
}

impl FieldState {
    /// An all-zero 33-plane single-carrier field on `grid`. The buffer is
    /// admitted against `limits` before it is allocated, and a refused
    /// allocation is reported, not aborted.
    ///
    /// # Errors
    /// [`CoreError::BudgetExhausted`] above `limits`;
    /// [`CoreError::AllocationFailed`] if the host refuses;
    /// [`CoreError::Overflow`] past the address space.
    pub fn zeroed(grid: FieldGrid, limits: &BufferLimits) -> Result<Self, CoreError> {
        Self::zeroed_with_planes(grid, FIELD_PLANES, limits)
    }

    /// An all-zero field on `grid` with `planes` (`33` single-carrier or `34`
    /// dual-carrier, `V2-15` / `FL-055`). Vascular edge storage starts empty
    /// (`0 B`) until [`FieldState::enable_vascular`] is called.
    ///
    /// # Errors
    /// As [`FieldLayout::planar_with_planes`] and [`FieldState::zeroed`].
    pub fn zeroed_with_planes(
        grid: FieldGrid,
        planes: u32,
        limits: &BufferLimits,
    ) -> Result<Self, CoreError> {
        let layout = FieldLayout::planar_with_planes(grid, planes)?;
        limits.admit("field buffer bytes", layout.byte_length())?;
        let data = zeroed_f32(&layout)?;
        Ok(Self {
            layout,
            data,
            edge_x_cond: Vec::new(),
            edge_y_cond: Vec::new(),
        })
    }

    /// An all-zero 34-plane dual-carrier hybrid field on `grid` (`V2-15`).
    ///
    /// # Errors
    /// As [`FieldState::zeroed_with_planes`].
    pub fn zeroed_dual_carrier(grid: FieldGrid, limits: &BufferLimits) -> Result<Self, CoreError> {
        Self::zeroed_with_planes(grid, DUAL_CARRIER_PLANES, limits)
    }

    /// The initial field a validated configuration declares
    /// (`[field_initialization]`, §4.2, §4.4): `energy_per_cell` in every
    /// cell and, for the `zero` composition, no chemical moment.
    ///
    /// Resolves `34` planes when `V2Feature::DualCarrierTrails` is enabled
    /// (seeding Carrier 0's `e_0` while Carrier 1 starts at zero) and enables
    /// vascular edge storage (`0 B` when disabled) when `V2Feature::VascularField`
    /// is enabled.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] for an unimplemented grid or
    /// composition; [`CoreError::NumericalInvalid`] for an energy that is
    /// negative, nonfinite or beyond F32; and the errors of
    /// [`FieldState::zeroed_with_planes`] / [`FieldState::enable_vascular`].
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
        let planes = field_planes_for_config(config);
        let mut state = Self::zeroed_with_planes(grid, planes, limits)?;
        state.energy_mut().fill(stored);
        if let Some(v2) = config.config().v2.as_ref()
            && v2.is_enabled(V2Feature::VascularField)
        {
            let g_min = v2.vascular.minimum_conductance_ratio;
            let initial = f64::from(VASCULAR_INITIAL_CONDUCTANCE).clamp(g_min, 1.0) as f32;
            state.enable_vascular(initial, limits)?;
        }
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

    /// Number of planes (`33` single-carrier or `34` dual-carrier).
    #[must_use]
    pub const fn planes(&self) -> u32 {
        self.layout.planes()
    }

    /// Whether this field uses the 34-plane dual-carrier hybrid layout (`V2-15`).
    #[must_use]
    pub const fn is_dual_carrier(&self) -> bool {
        self.layout.is_dual_carrier()
    }

    /// Carrier 0's plane layout (`(0, 1, 32)` on 33-plane or `(0, 1, 24)` on 34-plane).
    #[must_use]
    pub const fn carrier0(&self) -> CarrierLayout {
        if self.is_dual_carrier() {
            CarrierLayout::CARRIER_0
        } else {
            CarrierLayout::SINGLE_CARRIER
        }
    }

    /// Carrier 1's plane layout (`Some((25, 26, 8))` on 34-plane, else `None`).
    #[must_use]
    pub const fn carrier1(&self) -> Option<CarrierLayout> {
        if self.is_dual_carrier() {
            Some(CarrierLayout::CARRIER_1)
        } else {
            None
        }
    }

    /// Number of moment channels on Carrier 0 (`32` single-carrier, `24` dual-carrier).
    #[must_use]
    pub const fn carrier0_moments_count(&self) -> usize {
        if self.is_dual_carrier() {
            CARRIER0_MOMENT_CHANNELS as usize
        } else {
            CHANNELS
        }
    }

    /// Every value, in planar order.
    #[must_use]
    pub fn as_slice(&self) -> &[f32] {
        &self.data
    }

    /// Bytes the allocation holds: planar data plus optional `V2-14` vascular
    /// edge buffers (`0 B` when disabled).
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        let field_bytes = u64::try_from(self.data.capacity())
            .ok()
            .and_then(|elements| elements.checked_mul(HOST_ALIGNMENT))
            .unwrap_or(u64::MAX);
        field_bytes
            .saturating_add(capacity_bytes(&self.edge_x_cond))
            .saturating_add(capacity_bytes(&self.edge_y_cond))
    }

    /// Plane `plane` as a row-major image, or `None` past the last plane.
    #[must_use]
    pub fn plane(&self, plane: u32) -> Option<&[f32]> {
        let range = self.plane_range(plane)?;
        self.data.get(range)
    }

    /// Every value, writable, in planar order. The length is fixed, so the
    /// layout cannot change through it.
    pub fn as_mut_slice(&mut self) -> &mut [f32] {
        &mut self.data
    }

    /// Carrier 0's moment planes (`0..32` on single-carrier, `0..24` on
    /// dual-carrier), writable. Never includes Carrier 1's planes (`25..34`).
    pub fn moments_mut(&mut self) -> &mut [f32] {
        let cells = usize::try_from(self.grid().cell_count()).unwrap_or(usize::MAX);
        let moments = self.carrier0_moments_count();
        let end = cells.saturating_mul(1 + moments);
        self.data.get_mut(cells..end).unwrap_or(&mut [])
    }

    /// Plane `plane`, writable.
    #[must_use]
    pub fn plane_mut(&mut self, plane: u32) -> Option<&mut [f32]> {
        let range = self.plane_range(plane)?;
        self.data.get_mut(range)
    }

    /// Carrier 0's energy plane (`e_0`).
    #[must_use]
    pub fn energy(&self) -> &[f32] {
        self.plane(ENERGY_PLANE).unwrap_or(&[])
    }

    /// Carrier 1's trail energy plane (`e_1`, plane 25) on a 34-plane field, or
    /// `None` on a 33-plane single-carrier field.
    #[must_use]
    pub fn trail_energy(&self) -> Option<&[f32]> {
        if self.is_dual_carrier() {
            self.plane(CARRIER1_ENERGY_PLANE)
        } else {
            None
        }
    }

    /// Carrier 1's trail energy plane (`e_1`, plane 25), writable.
    #[must_use]
    pub fn trail_energy_mut(&mut self) -> Option<&mut [f32]> {
        if self.is_dual_carrier() {
            self.plane_mut(CARRIER1_ENERGY_PLANE)
        } else {
            None
        }
    }

    /// Carrier 1's trail moment channel `channel` (`0..8`, planes `26..=33`) on
    /// a 34-plane field.
    #[must_use]
    pub fn trail_moment(&self, channel: u32) -> Option<&[f32]> {
        if !self.is_dual_carrier() || channel >= CARRIER1_MOMENT_CHANNELS {
            return None;
        }
        self.plane(CARRIER1_FIRST_MOMENT_PLANE.checked_add(channel)?)
    }

    /// Carrier 1's trail moment channel `channel` (`0..8`, planes `26..=33`),
    /// writable.
    #[must_use]
    pub fn trail_moment_mut(&mut self, channel: u32) -> Option<&mut [f32]> {
        if !self.is_dual_carrier() || channel >= CARRIER1_MOMENT_CHANNELS {
            return None;
        }
        self.plane_mut(CARRIER1_FIRST_MOMENT_PLANE.checked_add(channel)?)
    }

    /// `Σ e_c` in F64, summed in cell-index order: the field's holdings for
    /// the energy ledger. In 34-plane dual-carrier mode, sums `Σ e_0 + Σ e_1`
    /// so `INV-03` closes identically across both carriers.
    #[must_use]
    pub fn energy_total(&self) -> f64 {
        let base = crate::transport::sum(self.energy());
        match self.trail_energy() {
            Some(trail) => base + crate::transport::sum(trail),
            None => base,
        }
    }

    /// Carrier 0's energy plane (`e_0`), writable.
    #[must_use]
    pub fn energy_mut(&mut self) -> &mut [f32] {
        self.plane_mut(ENERGY_PLANE).unwrap_or(&mut [])
    }

    /// Carrier 0 moment channel `channel` (`0..32` on single-carrier, `0..24` on
    /// dual-carrier), or `None` past Carrier 0's channels.
    #[must_use]
    pub fn moment(&self, channel: u32) -> Option<&[f32]> {
        let limit = self.carrier0().moment_count;
        if channel >= limit {
            return None;
        }
        self.plane(channel.checked_add(1)?)
    }

    /// Carrier 0 moment channel `channel`, writable.
    #[must_use]
    pub fn moment_mut(&mut self, channel: u32) -> Option<&mut [f32]> {
        let limit = self.carrier0().moment_count;
        if channel >= limit {
            return None;
        }
        self.plane_mut(channel.checked_add(1)?)
    }

    /// Carrier 0 cell `(x, y)`, or `None` outside the grid. On a 34-plane
    /// dual-carrier field, `moments[24..32]` are left `0.0` (read Carrier 1 via
    /// [`FieldState::trail_cell`]).
    #[must_use]
    pub fn cell(&self, x: u32, y: u32) -> Option<Cell> {
        let index = self.grid().cell_index(x, y)?;
        let mut cell = Cell::EMPTY;
        cell.energy = *self.energy().get(index)?;
        let active = self.carrier0_moments_count();
        for (channel, value) in (0_u32..).zip(cell.moments.iter_mut().take(active)) {
            *value = *self.moment(channel)?.get(index)?;
        }
        Some(cell)
    }

    /// Carrier 1 cell `(x, y)` as `(e_1, [m^(1)_0, …, m^(1)_7])`, or `None` on
    /// a 33-plane field or outside the grid.
    #[must_use]
    pub fn trail_cell(&self, x: u32, y: u32) -> Option<(f32, [f32; 8])> {
        if !self.is_dual_carrier() {
            return None;
        }
        let index = self.grid().cell_index(x, y)?;
        let energy = *self.trail_energy()?.get(index)?;
        let mut moments = [0.0_f32; 8];
        for (channel, slot) in (0_u32..).zip(moments.iter_mut()) {
            *slot = *self.trail_moment(channel)?.get(index)?;
        }
        Some((energy, moments))
    }

    /// Overwrite Carrier 1 cell `(x, y)`; `None` on a 33-plane field or outside
    /// the grid.
    #[must_use]
    pub fn set_trail_cell(
        &mut self,
        x: u32,
        y: u32,
        energy: f32,
        moments: &[f32; 8],
    ) -> Option<()> {
        if !self.is_dual_carrier() {
            return None;
        }
        let index = self.grid().cell_index(x, y)?;
        *self.trail_energy_mut()?.get_mut(index)? = energy;
        for (channel, &value) in (0_u32..).zip(moments.iter()) {
            *self.trail_moment_mut(channel)?.get_mut(index)? = value;
        }
        Some(())
    }

    /// Carrier 0 cell `index`, row-major as [`FieldGrid::cell_index`] numbers
    /// cells, or `None` outside the grid: [`FieldState::cell`] by index.
    #[must_use]
    pub fn cell_at(&self, index: usize) -> Option<Cell> {
        let cells = usize::try_from(self.grid().cell_count()).ok()?;
        if index >= cells {
            return None;
        }
        let mut cell = Cell::EMPTY;
        cell.energy = *self.data.get(index)?;
        let active = self.carrier0_moments_count();
        let mut at = index;
        for value in cell.moments.iter_mut().take(active) {
            at = at.checked_add(cells)?;
            *value = *self.data.get(at)?;
        }
        Some(cell)
    }

    /// Overwrite Carrier 0 cell `(x, y)`; `None` outside the grid, leaving the
    /// field unchanged. On a 34-plane field, writes `e_0` and `moments[0..24]`,
    /// leaving Carrier 1 untouched.
    #[must_use]
    pub fn set_cell(&mut self, x: u32, y: u32, cell: &Cell) -> Option<()> {
        let index = self.grid().cell_index(x, y)?;
        *self.energy_mut().get_mut(index)? = cell.energy;
        let active = self.carrier0_moments_count();
        for (channel, &value) in (0_u32..).zip(cell.moments.iter().take(active)) {
            *self.moment_mut(channel)?.get_mut(index)? = value;
        }
        Some(())
    }

    /// Enable and initialize `V2-14` (`vascular-field`) orthogonal east/north
    /// edge conductance buffers on this state (`8 B/cell`), admitted against
    /// `limits` (INV-15).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] unless `initial ∈ (0, 1]` is finite;
    /// [`CoreError::BudgetExhausted`], [`CoreError::AllocationFailed`], or
    /// [`CoreError::Overflow`]; leaves the state unchanged on error.
    pub fn enable_vascular(
        &mut self,
        initial: f32,
        limits: &BufferLimits,
    ) -> Result<(), CoreError> {
        if !initial.is_finite() || initial <= 0.0 || initial > 1.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "vascular initial conductance",
                detail: format!("{initial:?} is outside (0, 1]"),
            });
        }
        let cells = usize::try_from(self.grid().cell_count()).map_err(|_| overflow())?;
        let per_axis = self
            .grid()
            .cell_count()
            .checked_mul(HOST_ALIGNMENT)
            .ok_or_else(overflow)?;
        let total_bytes = per_axis.checked_mul(2).ok_or_else(overflow)?;
        limits.admit("vascular edge conductance bytes", total_bytes)?;
        let mut edge_x = Vec::new();
        edge_x
            .try_reserve_exact(cells)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "vascular east edge conductances",
                bytes: per_axis,
            })?;
        edge_x.resize(cells, initial);
        let mut edge_y = Vec::new();
        edge_y
            .try_reserve_exact(cells)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "vascular north edge conductances",
                bytes: per_axis,
            })?;
        edge_y.resize(cells, initial);
        self.edge_x_cond = edge_x;
        self.edge_y_cond = edge_y;
        Ok(())
    }

    /// Whether `V2-14` vascular edge conductance buffers are allocated on this
    /// state.
    #[must_use]
    pub fn has_vascular(&self) -> bool {
        !self.edge_x_cond.is_empty()
    }

    /// Read-only slices `(edge_x_cond, edge_y_cond)` when `V2-14` vascular
    /// storage is enabled, or `None` when disabled (`0 B`).
    #[must_use]
    pub fn vascular_edges(&self) -> Option<(&[f32], &[f32])> {
        if self.edge_x_cond.is_empty() {
            None
        } else {
            Some((&self.edge_x_cond, &self.edge_y_cond))
        }
    }

    /// Mutable slices `(edge_x_cond, edge_y_cond)` when `V2-14` vascular
    /// storage is enabled, or `None` when disabled.
    #[must_use]
    pub fn vascular_edges_mut(&mut self) -> Option<(&mut [f32], &mut [f32])> {
        if self.edge_x_cond.is_empty() {
            None
        } else {
            Some((&mut self.edge_x_cond, &mut self.edge_y_cond))
        }
    }

    /// Overwrite `V2-14` vascular edge buffers (for checkpoint restore, crop,
    /// or graft).
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if vascular storage is disabled or slice
    /// lengths do not match `grid.cell_count()`.
    pub fn set_vascular_edges(&mut self, edge_x: &[f32], edge_y: &[f32]) -> Result<(), CoreError> {
        if self.edge_x_cond.is_empty() {
            return Err(mismatch(
                "vascular edge storage is not enabled on this field state".into(),
            ));
        }
        if edge_x.len() != self.edge_x_cond.len() || edge_y.len() != self.edge_y_cond.len() {
            return Err(mismatch(format!(
                "vascular edges ({}, {}) do not match {} cells",
                edge_x.len(),
                edge_y.len(),
                self.edge_x_cond.len()
            )));
        }
        self.edge_x_cond.copy_from_slice(edge_x);
        self.edge_y_cond.copy_from_slice(edge_y);
        Ok(())
    }

    /// A read-only view.
    #[must_use]
    pub fn view(&self) -> FieldView<'_> {
        FieldView {
            layout: self.layout.as_read_only(),
            data: &self.data,
        }
    }

    /// Overwrite this state with `source`, which must have the same layout and
    /// matching vascular storage state.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for a different layout or vascular state.
    pub fn copy_from(&mut self, source: &Self) -> Result<(), CoreError> {
        self.layout.ensure_compatible(&source.layout)?;
        if self.edge_x_cond.len() != source.edge_x_cond.len()
            || self.edge_y_cond.len() != source.edge_y_cond.len()
        {
            return Err(mismatch(
                "source and destination field states disagree on vascular edge storage".into(),
            ));
        }
        self.data.copy_from_slice(&source.data);
        if !self.edge_x_cond.is_empty() {
            self.edge_x_cond.copy_from_slice(&source.edge_x_cond);
            self.edge_y_cond.copy_from_slice(&source.edge_y_cond);
        }
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
        let expected = FieldLayout::planar_with_planes(*layout.grid(), layout.planes())?;
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
            edge_x_cond: Vec::new(),
            edge_y_cond: Vec::new(),
        })
    }

    fn plane_range(&self, plane: u32) -> Option<std::ops::Range<usize>> {
        if plane >= self.layout.planes() {
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
    let planes = source.layout().planes();
    FieldLayout::new_with_planes(grid, target.arrangement(), planes)?.ensure_compatible(target)?;
    let mut out = zeroed_f32(target)?;
    for y in 0..grid.cells_y() {
        for x in 0..grid.cells_x() {
            for plane in 0..planes {
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
    /// Two all-zero 33-plane states on `grid`, each admitted against `limits`.
    ///
    /// # Errors
    /// As [`FieldState::zeroed`].
    pub fn zeroed(grid: FieldGrid, limits: &BufferLimits) -> Result<Self, CoreError> {
        Self::zeroed_with_planes(grid, FIELD_PLANES, limits)
    }

    /// Two all-zero states on `grid` with `planes` (`33` single-carrier or `34`
    /// dual-carrier, `V2-15`), each admitted against `limits`.
    ///
    /// # Errors
    /// As [`FieldState::zeroed_with_planes`].
    pub fn zeroed_with_planes(
        grid: FieldGrid,
        planes: u32,
        limits: &BufferLimits,
    ) -> Result<Self, CoreError> {
        Ok(Self {
            current: FieldState::zeroed_with_planes(grid, planes, limits)?,
            next: FieldState::zeroed_with_planes(grid, planes, limits)?,
        })
    }

    /// Pair an already-initialized committed `current` state with a matching
    /// candidate `next` state.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if layouts or vascular storage disagree.
    pub fn from_states(current: FieldState, next: FieldState) -> Result<Self, CoreError> {
        current.layout().ensure_compatible(next.layout())?;
        if current.has_vascular() != next.has_vascular() {
            return Err(mismatch(
                "current and next field states disagree on vascular edge storage".into(),
            ));
        }
        Ok(Self { current, next })
    }

    /// Enable `V2-14` vascular edge conductance buffers on BOTH `current` and
    /// `next` atomically (`C.8` cancel-safe ping-pong storage).
    ///
    /// # Errors
    /// As [`FieldState::enable_vascular`]; leaves both states unchanged on error.
    pub fn enable_vascular(
        &mut self,
        initial: f32,
        limits: &BufferLimits,
    ) -> Result<(), CoreError> {
        let mut next_copy = self.next.clone();
        next_copy.enable_vascular(initial, limits)?;
        self.current.enable_vascular(initial, limits)?;
        self.next = next_copy;
        Ok(())
    }

    /// Whether `V2-14` vascular edge storage is enabled on this pair.
    #[must_use]
    pub fn has_vascular(&self) -> bool {
        self.current.has_vascular()
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

    /// The candidate buffer, as the last step left it. Before a commit it is
    /// the candidate; after one it is the previous committed field, to be
    /// overwritten by the next step.
    #[must_use]
    pub const fn next(&self) -> &FieldState {
        &self.next
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
