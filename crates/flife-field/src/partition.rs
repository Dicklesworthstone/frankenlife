//! Single-world multi-device spatial partitioning evaluation (FL-058;
//! plan §20.4, §24.2, ADR-11).
//!
//! Decomposes a periodic 2D [`FieldGrid`] into `tiles_x × tiles_y` disjoint
//! rectangular tiles and implements exact, barrier-synchronized distributed
//! equivalents of the cross-boundary simulation phases with zero stale
//! boundaries:
//!
//! 1. **Field transport with periodic halo exchange** ([`transport_partitioned`]):
//!    each tile holds its own local `(w + 2h) × (h + 2h) × planes` buffer,
//!    exchanges periodic halos with its neighbor tiles, runs the exact
//!    `positive-five-point-v1` stencil and decay independently in parallel,
//!    and reproduces [`crate::transport_on`] bit-for-bit (`0 ULP`).
//! 2. **Boundary-consistent demand/grant uptake** ([`resolve_partitioned_uptake`]):
//!    each tile computes its local organisms' `grid-cic-v1` requests from its
//!    local energy plus a 1-cell periodic energy halo, routes cross-seam demand
//!    packets to the owning cell's tile, reconciles each cell authoritatively
//!    on its owner tile via `cell-quantum-largest-remainder-v1` (`INV-04`), and
//!    routes exact grant replies back to the organism's tile — reproducing
//!    [`crate::resolve_uptake`] bit-for-bit (`0 ULP`).
//! 3. **Boundary-consistent deposit reduction** ([`apply_partitioned_deposits`]):
//!    splits paid emissions over `grid-cic-v1` footprints, routes cross-seam
//!    packets to the cell's owning tile, and reduces in ascending `OrganismId`
//!    order — reproducing [`crate::apply_deposits`] bit-for-bit (`0 ULP`).
//! 4. **Atomic post-motion organism & full-genome migration**
//!    ([`migrate_organisms`]): transfers organisms whose wrapped post-motion
//!    positions cross tile seams, preserving exact population count and
//!    `INV-07` identity uniqueness while measuring exact genome wire traffic.
//! 5. **Communication budget & interconnect evaluator**
//!    ([`estimate_comm_budget`]): quantifies field halo, multi-level sensor
//!    pyramid halo, boundary uptake/deposit, and genome migration traffic
//!    against real interconnect bandwidth and barrier latency profiles.

use std::collections::BTreeMap;

use flife_core::CoreError;
use flife_core::ids::OrganismId;
use flife_core::ledger::{CompensatedSum, add_down};

use crate::deposit::{DepositSummary, round_down_f32, split as split_deposit_offer};
use crate::exchange::ExchangeStencil;
use crate::grid::{ENERGY_PLANE, FIELD_PLANES, FieldGrid};
use crate::state::{CHANNELS, FieldState};
use crate::transport::{TransportPlan, TransportReport, energy_report};
use crate::uptake::{CellDebit, GrantPacket, RequestRule, Share, UptakeSummary, resolve_cell};

/// Wire size (bytes) of one cross-seam demand message:
/// `from_tile(4) + to_tile(4) + cell(8) + organism(8) + requested(8)`.
pub const BOUNDARY_DEMAND_WIRE_BYTES: u64 = 32;

/// Wire size (bytes) of one cross-seam grant reply message:
/// `from_tile(4) + to_tile(4) + cell(8) + organism(8) + granted(8)`.
pub const BOUNDARY_GRANT_WIRE_BYTES: u64 = 32;

/// Wire size (bytes) of one cross-seam deposit packet:
/// `from_tile(4) + to_tile(4) + cell(8) + organism(8) + amount(8) + direction(32 * 8)`.
pub const BOUNDARY_DEPOSIT_WIRE_BYTES: u64 = 32 + (CHANNELS as u64) * 8;

/// Wire overhead (bytes) per migrating organism excluding its controller weight
/// buffer (`OrganismId`, `GenomeId`, lineage, generation, birth tick, cooldown,
/// position, velocity, energy, traits, held actions).
pub const MIGRATION_HEADER_WIRE_BYTES: u64 = 256;

/// Inclusive-exclusive cell bounds `[x0, x1) × [y0, y1)` owned by one tile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileBounds {
    /// Tile ordinal (`ty * tiles_x + tx`).
    pub tile: usize,
    /// Tile column in `0..tiles_x`.
    pub tx: u32,
    /// Tile row in `0..tiles_y`.
    pub ty: u32,
    /// First owned cell column (inclusive).
    pub x0: u32,
    /// End cell column (exclusive).
    pub x1: u32,
    /// First owned cell row (inclusive).
    pub y0: u32,
    /// End cell row (exclusive).
    pub y1: u32,
}

impl TileBounds {
    /// Owned columns (`x1 - x0`).
    #[must_use]
    pub const fn width(self) -> usize {
        (self.x1 - self.x0) as usize
    }

    /// Owned rows (`y1 - y0`).
    #[must_use]
    pub const fn height(self) -> usize {
        (self.y1 - self.y0) as usize
    }

    /// Owned cell count (`width * height`).
    #[must_use]
    pub const fn cell_count(self) -> usize {
        self.width() * self.height()
    }
}

/// Disjoint 2D periodic domain decomposition of a [`FieldGrid`] into
/// `tiles_x × tiles_y` rectangular tiles.
#[derive(Debug, Clone, PartialEq)]
pub struct TilePartition {
    grid: FieldGrid,
    tiles_x: u32,
    tiles_y: u32,
    x_splits: Vec<u32>,
    y_splits: Vec<u32>,
}

impl TilePartition {
    /// Build a `tiles_x × tiles_y` partition of `grid`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] when `tiles_x == 0`, `tiles_y == 0`,
    /// `tiles_x > grid.cells_x()`, or `tiles_y > grid.cells_y()`.
    pub fn new(grid: FieldGrid, tiles_x: u32, tiles_y: u32) -> Result<Self, CoreError> {
        if tiles_x == 0 || tiles_x > grid.cells_x() {
            return Err(CoreError::NumericalInvalid {
                quantity: "partition tiles_x",
                detail: format!(
                    "tiles_x={tiles_x} must lie in 1..={} for this grid",
                    grid.cells_x()
                ),
            });
        }
        if tiles_y == 0 || tiles_y > grid.cells_y() {
            return Err(CoreError::NumericalInvalid {
                quantity: "partition tiles_y",
                detail: format!(
                    "tiles_y={tiles_y} must lie in 1..={} for this grid",
                    grid.cells_y()
                ),
            });
        }
        let split_axis = |cells: u32, tiles: u32| -> Vec<u32> {
            (0..=tiles)
                .map(|t| ((u64::from(t) * u64::from(cells)) / u64::from(tiles)) as u32)
                .collect()
        };
        Ok(Self {
            grid,
            tiles_x,
            tiles_y,
            x_splits: split_axis(grid.cells_x(), tiles_x),
            y_splits: split_axis(grid.cells_y(), tiles_y),
        })
    }

    /// Underlying periodic grid.
    #[must_use]
    pub const fn grid(&self) -> &FieldGrid {
        &self.grid
    }

    /// Tile columns.
    #[must_use]
    pub const fn tiles_x(&self) -> u32 {
        self.tiles_x
    }

    /// Tile rows.
    #[must_use]
    pub const fn tiles_y(&self) -> u32 {
        self.tiles_y
    }

    /// Total tile count (`tiles_x * tiles_y`).
    #[must_use]
    pub const fn tile_count(&self) -> usize {
        (self.tiles_x as usize) * (self.tiles_y as usize)
    }

    /// Bounds owned by `tile`.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if `tile >= self.tile_count()`.
    pub fn bounds(&self, tile: usize) -> Result<TileBounds, CoreError> {
        if tile >= self.tile_count() {
            return Err(CoreError::LayoutMismatch {
                buffer: "tile partition",
                detail: format!("tile {tile} out of range for {} tiles", self.tile_count()),
            });
        }
        let tx = (tile % (self.tiles_x as usize)) as u32;
        let ty = (tile / (self.tiles_x as usize)) as u32;
        Ok(TileBounds {
            tile,
            tx,
            ty,
            x0: self.x_splits[tx as usize],
            x1: self.x_splits[tx as usize + 1],
            y0: self.y_splits[ty as usize],
            y1: self.y_splits[ty as usize + 1],
        })
    }

    /// Owning tile of cell `(x, y)`.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if `(x, y)` lies outside the grid.
    pub fn tile_of_cell(&self, x: u32, y: u32) -> Result<usize, CoreError> {
        if x >= self.grid.cells_x() || y >= self.grid.cells_y() {
            return Err(CoreError::LayoutMismatch {
                buffer: "tile partition",
                detail: format!(
                    "cell ({x}, {y}) outside {}x{}",
                    self.grid.cells_x(),
                    self.grid.cells_y()
                ),
            });
        }
        // `partition_point(|&s| s <= x) - 1` finds the unique `t` with
        // `splits[t] <= x < splits[t + 1]`.
        let tx = self.x_splits.partition_point(|&s| s <= x).saturating_sub(1);
        let ty = self.y_splits.partition_point(|&s| s <= y).saturating_sub(1);
        Ok(ty * (self.tiles_x as usize) + tx)
    }

    /// Owning tile of row-major global cell index `cell = y * G_x + x`.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if `cell` lies outside the grid.
    pub fn tile_of_cell_index(&self, cell: usize) -> Result<usize, CoreError> {
        let total = usize::try_from(self.grid.cell_count()).map_err(|_| CoreError::Overflow {
            quantity: "partition cell index",
        })?;
        if cell >= total {
            return Err(CoreError::LayoutMismatch {
                buffer: "tile partition",
                detail: format!("cell index {cell} outside {total} cells"),
            });
        }
        let gx = self.grid.cells_x() as usize;
        let x = (cell % gx) as u32;
        let y = (cell / gx) as u32;
        self.tile_of_cell(x, y)
    }

    /// Owning tile of a world position (wrapped periodically onto the torus).
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for nonfinite coordinates.
    pub fn tile_of_position(&self, position: [f64; 2]) -> Result<usize, CoreError> {
        let wrapped = self.grid.wrap_position(position)?;
        let cx =
            ((wrapped[0] / self.grid.cell_width()).floor() as u32).min(self.grid.cells_x() - 1);
        let cy =
            ((wrapped[1] / self.grid.cell_height()).floor() as u32).min(self.grid.cells_y() - 1);
        self.tile_of_cell(cx, cy)
    }

    /// Number of halo cells a single tile receives from neighbor tiles for a
    /// `halo`-cell ghost border (zero when `tiles_x == 1 && tiles_y == 1`).
    #[must_use]
    pub fn halo_cells_per_tile(&self, tile: usize, halo: u32) -> u64 {
        if self.tile_count() <= 1 || halo == 0 {
            return 0;
        }
        let Ok(b) = self.bounds(tile) else {
            return 0;
        };
        let w = b.width() as u64;
        let h = b.height() as u64;
        let r = u64::from(halo);
        // Only axes actually split across devices require remote halo transfers.
        let hx = if self.tiles_x > 1 { r } else { 0 };
        let hy = if self.tiles_y > 1 { r } else { 0 };
        (w + 2 * hx) * (h + 2 * hy) - w * h
    }
}

/// Halo exchange cadence for multi-substep five-point transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HaloMode {
    /// Exchange a 1-cell halo before every substep (`substeps` barriers/tick,
    /// minimal halo bandwidth).
    PerSubstep,
    /// Exchange a `substeps`-cell wide halo once per tick (`1` barrier/tick,
    /// redundant halo compute inside the ghost apron).
    WideTick,
}

/// Standalone local field storage for one device tile, including a `halo`-cell
/// ghost apron on every side (`(w + 2*halo) × (h + 2*halo)` per plane).
#[derive(Debug, Clone, PartialEq)]
pub struct TileField {
    bounds: TileBounds,
    halo: usize,
    planes: usize,
    stride_x: usize,
    stride_y: usize,
    data: Vec<f32>,
}

impl TileField {
    /// Allocate a zeroed standalone tile buffer for `bounds`, `planes`, and
    /// `halo` ghost cells per side.
    #[must_use]
    pub fn zeroed(bounds: TileBounds, planes: usize, halo: usize) -> Self {
        let stride_x = bounds.width() + 2 * halo;
        let stride_y = bounds.height() + 2 * halo;
        let data = vec![0.0_f32; planes * stride_x * stride_y];
        Self {
            bounds,
            halo,
            planes,
            stride_x,
            stride_y,
            data,
        }
    }

    /// Tile interior bounds.
    #[must_use]
    pub const fn bounds(&self) -> TileBounds {
        self.bounds
    }

    /// Ghost border thickness in cells.
    #[must_use]
    pub const fn halo(&self) -> usize {
        self.halo
    }

    /// Plane slice `(stride_x * stride_y)` for `plane`.
    #[must_use]
    pub fn plane(&self, plane: usize) -> &[f32] {
        let plane_len = self.stride_x * self.stride_y;
        &self.data[plane * plane_len..(plane + 1) * plane_len]
    }

    /// Mutable plane slice `(stride_x * stride_y)` for `plane`.
    pub fn plane_mut(&mut self, plane: usize) -> &mut [f32] {
        let plane_len = self.stride_x * self.stride_y;
        &mut self.data[plane * plane_len..(plane + 1) * plane_len]
    }

    /// Copy owned interior cells from a single global plane slice `src_plane`
    /// (`grid.cells_x() * grid.cells_y()`) into local `plane`.
    pub fn load_plane(&mut self, grid: &FieldGrid, plane: usize, src_plane: &[f32]) {
        let gx = grid.cells_x() as usize;
        let w = self.bounds.width();
        let h = self.bounds.height();
        let x0 = self.bounds.x0 as usize;
        let y0 = self.bounds.y0 as usize;
        let halo = self.halo;
        let sx = self.stride_x;
        let dst_plane = self.plane_mut(plane);
        for ly in 0..h {
            let src_row = &src_plane[(y0 + ly) * gx + x0..][..w];
            let dst_row = &mut dst_plane[(ly + halo) * sx + halo..][..w];
            dst_row.copy_from_slice(src_row);
        }
    }

    /// Copy owned interior cells from a global [`FieldState`] (zero remote
    /// halo reads).
    pub fn load_interior(&mut self, global: &FieldState) {
        let grid = *global.grid();
        let cells = (grid.cells_x() as usize) * (grid.cells_y() as usize);
        if cells == 0 {
            return;
        }
        let src_all = global.as_slice();
        let count = self.planes.min(src_all.len() / cells);
        for p in 0..count {
            self.load_plane(&grid, p, &src_all[p * cells..(p + 1) * cells]);
        }
    }

    /// Write owned interior cells back into a global [`FieldState`].
    pub fn store_interior(&self, global: &mut FieldState) {
        let gx = global.grid().cells_x() as usize;
        let gy = global.grid().cells_y() as usize;
        let cells = gx * gy;
        if cells == 0 {
            return;
        }
        let dst_all = global.as_mut_slice();
        let w = self.bounds.width();
        let h = self.bounds.height();
        let x0 = self.bounds.x0 as usize;
        let y0 = self.bounds.y0 as usize;
        let count = self.planes.min(dst_all.len() / cells);
        for p in 0..count {
            let src_plane = self.plane(p);
            let dst_plane = &mut dst_all[p * cells..(p + 1) * cells];
            for ly in 0..h {
                let src_row = &src_plane[(ly + self.halo) * self.stride_x + self.halo..][..w];
                let dst_row = &mut dst_plane[(y0 + ly) * gx + x0..][..w];
                dst_row.copy_from_slice(src_row);
            }
        }
    }

    /// Read one cell from this tile's owned interior or filled halo apron using
    /// global coordinates `(x, y)` within `halo` cells of `bounds` on the torus.
    #[must_use]
    pub fn read_local_or_halo(
        &self,
        grid: &FieldGrid,
        plane: usize,
        x: u32,
        y: u32,
    ) -> Option<f32> {
        let gx = i64::from(grid.cells_x());
        let gy = i64::from(grid.cells_y());
        let halo_i = self.halo as i64;
        let map_axis = |coord: u32, start: u32, len: usize, period: i64| -> Option<usize> {
            let c = i64::from(coord);
            let s = i64::from(start);
            let len_i = len as i64;
            for shift in [0, period, -period] {
                let d = c + shift - s;
                if d >= -halo_i && d < len_i + halo_i {
                    return usize::try_from(d + halo_i).ok();
                }
            }
            None
        };
        let lx = map_axis(x, self.bounds.x0, self.bounds.width(), gx)?;
        let ly = map_axis(y, self.bounds.y0, self.bounds.height(), gy)?;
        let plane_slice = self.plane(plane);
        plane_slice.get(ly * self.stride_x + lx).copied()
    }
}

/// Fill every tile's `halo`-cell ghost border strictly from the owning neighbor
/// tiles' interiors (never from a global array and without cloning tile
/// interiors), returning total remote cells transferred (cross-tile only;
/// local periodic wrap on a single-tile axis does not cross a device boundary).
fn exchange_tile_halos(
    partition: &TilePartition,
    tiles: &mut [TileField],
) -> Result<u64, CoreError> {
    let grid = *partition.grid();
    let tile_count = tiles.len();
    if tile_count == 0 {
        return Ok(0);
    }
    let planes = tiles[0].planes;
    let halo = tiles[0].halo;
    if halo == 0 {
        return Ok(0);
    }

    // Gather ONLY the ghost-apron entries (`~1.6 MB` at 1024²×33 vs `138 MB`
    // for all interiors) from neighbor tiles' immutable interiors, then scatter
    // them into each destination tile's ghost apron.
    let mut remote_cells_total = 0_u64;
    let mut staged: Vec<Vec<(usize, Vec<f32>)>> = Vec::with_capacity(tile_count);

    for (dst_idx, dst_tile) in tiles.iter().enumerate() {
        let b = dst_tile.bounds;
        let w = b.width();
        let h = b.height();
        let halo_i = halo as i64;
        let apron_cells = dst_tile.stride_x * dst_tile.stride_y - w * h;
        let mut writes: Vec<(usize, Vec<f32>)> = Vec::with_capacity(apron_cells);

        for ly in 0..dst_tile.stride_y {
            let gy_raw = i64::from(b.y0) + (ly as i64) - halo_i;
            let in_y = ly >= halo && ly < halo + h;
            for lx in 0..dst_tile.stride_x {
                let in_x = lx >= halo && lx < halo + w;
                if in_x && in_y {
                    continue;
                }
                let gx_raw = i64::from(b.x0) + (lx as i64) - halo_i;
                let (wx, wy) = grid.wrap_cell(gx_raw, gy_raw);
                let src_tile_idx = partition.tile_of_cell(wx, wy)?;
                if src_tile_idx != dst_idx {
                    remote_cells_total += 1;
                }
                let src_tile = &tiles[src_tile_idx];
                let sx = (wx - src_tile.bounds.x0) as usize + src_tile.halo;
                let sy = (wy - src_tile.bounds.y0) as usize + src_tile.halo;
                let src_offset = sy * src_tile.stride_x + sx;
                let dst_offset = ly * dst_tile.stride_x + lx;
                let mut vals = Vec::with_capacity(planes);
                for p in 0..planes {
                    vals.push(src_tile.plane(p)[src_offset]);
                }
                writes.push((dst_offset, vals));
            }
        }
        staged.push(writes);
    }

    for (tile, writes) in tiles.iter_mut().zip(staged) {
        let plane_len = tile.stride_x * tile.stride_y;
        for (dst_offset, vals) in writes {
            for (p, val) in vals.into_iter().enumerate() {
                tile.data[p * plane_len + dst_offset] = val;
            }
        }
    }

    Ok(remote_cells_total)
}

/// Persistent per-device tile buffers for steady-state partitioned transport
/// (allocated once, reused across ticks with zero per-tick interior cloning).
#[derive(Debug, Clone)]
pub struct ResidentTileField {
    planes: usize,
    halo: usize,
    tiles: Vec<TileField>,
    scratch: Vec<TileField>,
}

impl ResidentTileField {
    /// Allocate resident tile + scratch buffers for `partition` and `halo`.
    ///
    /// # Errors
    /// As [`TilePartition::bounds`].
    pub fn new(partition: &TilePartition, planes: usize, halo: usize) -> Result<Self, CoreError> {
        let tile_count = partition.tile_count();
        let mut tiles = Vec::with_capacity(tile_count);
        let mut scratch = Vec::with_capacity(tile_count);
        for t in 0..tile_count {
            let bounds = partition.bounds(t)?;
            tiles.push(TileField::zeroed(bounds, planes, halo));
            scratch.push(TileField::zeroed(bounds, planes, halo));
        }
        Ok(Self {
            planes,
            halo,
            tiles,
            scratch,
        })
    }

    /// Copy owned interior cells from a global [`FieldState`] into each tile.
    pub fn load_from(&mut self, global: &FieldState) {
        for tile in &mut self.tiles {
            tile.load_interior(global);
        }
    }

    /// Write owned interior cells from each tile back into a global [`FieldState`].
    pub fn store_into(&self, global: &mut FieldState) {
        for tile in &self.tiles {
            tile.store_interior(global);
        }
    }

    /// Execute one tick of resident-tile `positive-five-point-v1` transport +
    /// decay across all planes (`0..planes`, including energy) and all tiles in
    /// parallel with zero interior reallocation, returning communication stats.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if the resident buffer halo/tile count
    /// does not match `partition` / `plan` / `mode`; [`CoreError::StateConflict`]
    /// on worker pool errors.
    pub fn transport_step(
        &mut self,
        partition: &TilePartition,
        plan: &TransportPlan,
        mode: HaloMode,
        workers: usize,
    ) -> Result<TransportPartitionStats, CoreError> {
        self.transport_step_inner(partition, plan, mode, workers, true)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "explicit per-substep and wide-tick resident halo transport with exact reference F64 arithmetic"
    )]
    fn transport_step_inner(
        &mut self,
        partition: &TilePartition,
        plan: &TransportPlan,
        mode: HaloMode,
        workers: usize,
        decay_energy: bool,
    ) -> Result<TransportPartitionStats, CoreError> {
        let tile_count = partition.tile_count();
        let substeps = plan.substeps();
        let required_halo = match mode {
            HaloMode::PerSubstep => 1_usize,
            HaloMode::WideTick => substeps.max(1) as usize,
        };
        if self.tiles.len() != tile_count || self.halo != required_halo {
            return Err(CoreError::LayoutMismatch {
                buffer: "resident tile field",
                detail: format!(
                    "buffer has {} tiles / halo {}, expected {tile_count} tiles / halo {required_halo}",
                    self.tiles.len(),
                    self.halo
                ),
            });
        }
        let planes = self.planes;
        let halo = self.halo;
        let (r_x, r_y) = plan.ratios();
        let center = plan.center();
        let decay_factor = plan.decay_factor();

        let step_tile_substep = |src_tile: &TileField, dst_tile: &mut TileField, margin: usize| {
            let sx = src_tile.stride_x;
            let x_start = margin;
            let x_end = src_tile.stride_x - margin;
            let y_start = margin;
            let y_end = src_tile.stride_y - margin;
            for p in 0..planes {
                let src = src_tile.plane(p);
                let dst = dst_tile.plane_mut(p);
                for y in y_start..y_end {
                    let row_s = (y - 1) * sx;
                    let row_c = y * sx;
                    let row_n = (y + 1) * sx;
                    for x in x_start..x_end {
                        let c = f64::from(src[row_c + x]);
                        let w = f64::from(src[row_c + x - 1]);
                        let e = f64::from(src[row_c + x + 1]);
                        let s = f64::from(src[row_s + x]);
                        let n = f64::from(src[row_n + x]);
                        dst[row_c + x] = (center * c + r_x * (w + e) + r_y * (s + n)) as f32;
                    }
                }
            }
        };

        let mut remote_halo_cells = 0_u64;
        let mut barriers = 0_u32;
        let effective_workers = workers.max(1).min(tile_count);

        match mode {
            HaloMode::PerSubstep => {
                for _ in 0..substeps {
                    remote_halo_cells += exchange_tile_halos(partition, &mut self.tiles)?;
                    barriers += 1;
                    if effective_workers > 1 {
                        let mut pairs: Vec<(&TileField, &mut TileField)> =
                            self.tiles.iter().zip(self.scratch.iter_mut()).collect();
                        flife_core::pool::parallel_tasks(
                            effective_workers,
                            "partitioned transport substep",
                            &mut pairs,
                            |_, (src, dst)| step_tile_substep(src, dst, 1),
                        )?;
                    } else {
                        for (src, dst) in self.tiles.iter().zip(self.scratch.iter_mut()) {
                            step_tile_substep(src, dst, 1);
                        }
                    }
                    std::mem::swap(&mut self.tiles, &mut self.scratch);
                }
            }
            HaloMode::WideTick => {
                remote_halo_cells += exchange_tile_halos(partition, &mut self.tiles)?;
                barriers += 1;
                let run_tile_wide = |src: &mut TileField, tmp: &mut TileField| {
                    for step in 0..substeps as usize {
                        let margin = step + 1;
                        step_tile_substep(src, tmp, margin);
                        std::mem::swap(src, tmp);
                    }
                };
                if effective_workers > 1 {
                    let mut pairs: Vec<(&mut TileField, &mut TileField)> =
                        self.tiles.iter_mut().zip(self.scratch.iter_mut()).collect();
                    flife_core::pool::parallel_tasks(
                        effective_workers,
                        "partitioned transport wide",
                        &mut pairs,
                        |_, (src, tmp)| run_tile_wide(src, tmp),
                    )?;
                } else {
                    for (src, tmp) in self.tiles.iter_mut().zip(self.scratch.iter_mut()) {
                        run_tile_wide(src, tmp);
                    }
                }
            }
        }

        if decay_factor != 1.0 {
            let first_decay_plane = usize::from(!decay_energy);
            let decay_planes = |tile: &mut TileField| {
                let h = tile.bounds.height();
                let w = tile.bounds.width();
                for p in first_decay_plane..planes {
                    let sx = tile.stride_x;
                    let plane = tile.plane_mut(p);
                    for ly in 0..h {
                        let row = &mut plane[(ly + halo) * sx + halo..][..w];
                        for cell in row {
                            *cell = (f64::from(*cell) * decay_factor) as f32;
                        }
                    }
                }
            };
            if effective_workers > 1 {
                flife_core::pool::parallel_tasks(
                    effective_workers,
                    "partitioned transport decay",
                    &mut self.tiles,
                    |_, tile| decay_planes(tile),
                )?;
            } else {
                for tile in &mut self.tiles {
                    decay_planes(tile);
                }
            }
        }

        Ok(TransportPartitionStats {
            tiles: tile_count,
            mode,
            substeps,
            barriers,
            remote_halo_cells,
            remote_halo_bytes: remote_halo_cells * (planes as u64) * 4,
        })
    }
}

/// Communication and execution statistics for [`transport_partitioned`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportPartitionStats {
    /// Tile count (`tiles_x * tiles_y`).
    pub tiles: usize,
    /// Halo exchange mode used.
    pub mode: HaloMode,
    /// Transport substeps executed.
    pub substeps: u32,
    /// Synchronization barriers executed during transport.
    pub barriers: u32,
    /// Total cross-tile halo cell transfers (counted per grid cell, across all
    /// barriers).
    pub remote_halo_cells: u64,
    /// Total cross-tile halo bytes transferred (`remote_halo_cells * planes * 4`).
    pub remote_halo_bytes: u64,
}

/// Run `positive-five-point-v1` transport + exponential decay across
/// `partition`'s independent device tiles using explicit periodic halo exchange
/// and up to `workers` parallel threads on `flife_core::pool`, returning the
/// exact global [`TransportReport`] and communication statistics.
///
/// Produces `0 ULP` bit-for-bit identical cell values and report totals to
/// [`crate::transport_on`].
///
/// # Errors
/// [`CoreError::LayoutMismatch`] for incompatible field layouts;
/// [`CoreError::StateConflict`] on worker pool errors.
pub fn transport_partitioned(
    partition: &TilePartition,
    plan: &TransportPlan,
    current: &FieldState,
    next: &mut FieldState,
    mode: HaloMode,
    workers: usize,
) -> Result<(TransportReport, TransportPartitionStats), CoreError> {
    let grid = *partition.grid();
    if *plan.grid() != grid {
        return Err(CoreError::LayoutMismatch {
            buffer: "partitioned transport",
            detail: "transport plan grid differs from tile partition grid".to_owned(),
        });
    }
    let expected = crate::layout::FieldLayout::planar(grid)?;
    expected.ensure_compatible(current.layout())?;
    expected.ensure_compatible(next.layout())?;

    let planes = FIELD_PLANES as usize;
    let substeps = plan.substeps();
    let halo = match mode {
        HaloMode::PerSubstep => 1_usize,
        HaloMode::WideTick => substeps.max(1) as usize,
    };

    let energy_before = current
        .energy()
        .iter()
        .fold(0.0_f64, |acc, &v| acc + f64::from(v));

    let mut resident = ResidentTileField::new(partition, planes, halo)?;
    resident.load_from(current);
    // Plane 0 (energy) is decayed in exact global cell order by `energy_report`
    // below so `TransportReport` matches `transport_on` bit-for-bit.
    let stats = resident.transport_step_inner(partition, plan, mode, workers, false)?;
    resident.store_into(next);

    let report = energy_report(plan, energy_before, next.energy_mut());
    Ok((report, stats))
}

/// One local organism participating in partitioned uptake on its owning tile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PartitionedOrganism {
    /// Stable organism identity.
    pub organism: OrganismId,
    /// Wrapped world position.
    pub position: [f64; 2],
    /// Optional explicit request amount `d_i`; when `None`, computed from the
    /// local + 1-cell energy halo via [`RequestRule`].
    pub explicit_request: Option<f64>,
}

/// One cross-seam uptake demand routed from the organism's tile to the target
/// cell's owning tile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundaryDemandMsg {
    /// Tile owning the requesting organism.
    pub from_tile: usize,
    /// Tile owning `cell`.
    pub to_tile: usize,
    /// Global target cell index.
    pub cell: usize,
    /// Requesting organism identity.
    pub organism: OrganismId,
    /// Index in the flattened global demand slice (for parity inspection).
    pub demand_index: usize,
    /// `q_ic = d_i * w_ic`.
    pub requested: f64,
}

/// One cross-seam grant reply routed from the cell's owning tile back to the
/// organism's tile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundaryGrantMsg {
    /// Tile owning `cell`.
    pub from_tile: usize,
    /// Tile owning `organism`.
    pub to_tile: usize,
    /// Global cell index.
    pub cell: usize,
    /// Recipient organism identity.
    pub organism: OrganismId,
    /// Index in the flattened global demand slice.
    pub demand_index: usize,
    /// Exact granted energy `u_ic`.
    pub granted: f64,
}

/// Communication statistics for [`resolve_partitioned_uptake`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UptakeCommStats {
    /// Cross-tile 1-cell energy halo transfers needed to compute local density.
    pub energy_halo_cells: u64,
    /// Bytes transferred for the 1-cell energy halo (`energy_halo_cells * 4`).
    pub energy_halo_bytes: u64,
    /// Cross-seam demand packets routed (`from_tile != to_tile`).
    pub boundary_demand_packets: u64,
    /// Wire bytes for cross-seam demand packets.
    pub boundary_demand_bytes: u64,
    /// Cross-seam grant replies routed back to organism tiles.
    pub boundary_grant_packets: u64,
    /// Wire bytes for cross-seam grant replies.
    pub boundary_grant_bytes: u64,
    /// Barriers required (energy halo + demand scatter + grant return).
    pub barriers: u32,
}

impl UptakeCommStats {
    /// Total wire bytes transferred across devices during uptake.
    #[must_use]
    pub const fn total_bytes(self) -> u64 {
        self.energy_halo_bytes + self.boundary_demand_bytes + self.boundary_grant_bytes
    }
}

/// Exact output of [`resolve_partitioned_uptake`].
#[derive(Debug, Clone, PartialEq)]
pub struct PartitionedUptakeResult {
    /// Global uptake summary matching [`crate::resolve_uptake`].
    pub summary: UptakeSummary,
    /// All cell debits in ascending cell order (merged from disjoint tile owners).
    pub debits: Vec<CellDebit>,
    /// All resolved packets in ascending `(cell, organism)` order.
    pub packets: Vec<GrantPacket>,
    /// Per-organism total granted energy (`add_down` in ascending cell order),
    /// indexed by the input organism's global index.
    pub totals: Vec<f64>,
    /// Measured cross-tile communication traffic.
    pub comm: UptakeCommStats,
}

/// Execute Phase 5 two-pass demand/grant uptake across `partition`'s disjoint
/// tiles with strict cell ownership (`INV-04`) and zero stale boundaries.
///
/// Each tile reads only its own energy cells plus a 1-cell periodic energy halo
/// to compute `grid-cic-v1` requests, routes cross-seam demand packets to the
/// target cell's owning tile, reconciles each cell authoritatively on its owner
/// tile in ascending `OrganismId` order via `cell-quantum-largest-remainder-v1`,
/// and routes exact grants back to the requesting organism's tile.
///
/// Produces `0 ULP` bit-for-bit identical debits, packets, and organism totals
/// to [`crate::demands`] + [`crate::resolve_uptake`].
///
/// # Errors
/// [`CoreError::NumericalInvalid`] for invalid positions/requests/energies;
/// [`CoreError::LayoutMismatch`] for mismatched grid/field.
#[expect(
    clippy::too_many_lines,
    reason = "three-phase partitioned uptake (halo request, owner-tile reconciliation, grant reply) with exact wire accounting"
)]
pub fn resolve_partitioned_uptake(
    partition: &TilePartition,
    field: &FieldState,
    rule: Option<&RequestRule>,
    organisms: &[PartitionedOrganism],
) -> Result<PartitionedUptakeResult, CoreError> {
    let grid = *partition.grid();
    if *field.grid() != grid {
        return Err(CoreError::LayoutMismatch {
            buffer: "partitioned uptake",
            detail: "field grid differs from tile partition grid".to_owned(),
        });
    }
    let tile_count = partition.tile_count();

    // Refuse duplicate `OrganismId`s identically to global `resolve_uptake`.
    let mut seen_orgs: BTreeMap<OrganismId, usize> = BTreeMap::new();
    for (idx, org) in organisms.iter().enumerate() {
        if let Some(prev) = seen_orgs.insert(org.organism, idx) {
            return Err(CoreError::StateConflict {
                operation: "partitioned uptake demand",
                detail: format!(
                    "organism {} demands twice (entries {prev} and {idx})",
                    org.organism.get()
                ),
            });
        }
    }

    // Build 1-plane energy tiles with 1-cell periodic halo directly from
    // `field.energy()` (zero intermediate full-field allocation).
    let mut energy_tiles: Vec<TileField> = Vec::with_capacity(tile_count);
    for t in 0..tile_count {
        let mut tile = TileField::zeroed(partition.bounds(t)?, 1, 1);
        tile.load_plane(&grid, 0, field.energy());
        energy_tiles.push(tile);
    }
    let energy_halo_cells = exchange_tile_halos(partition, &mut energy_tiles)?;

    // Assign organisms to their owning spatial tile (each tile holds only its
    // own local organisms).
    let mut tile_organisms: Vec<Vec<(usize, PartitionedOrganism)>> = vec![Vec::new(); tile_count];
    for (idx, &org) in organisms.iter().enumerate() {
        let owner = partition.tile_of_position(org.position)?;
        tile_organisms[owner].push((idx, org));
    }

    // Phase 5a: Each tile computes local requests from its local+halo energy
    // buffer and partitions packets into local vs cross-seam boundary messages.
    let mut owner_packets: Vec<Vec<(usize, GrantPacket)>> = vec![Vec::new(); tile_count];
    let mut boundary_demands: Vec<BoundaryDemandMsg> = Vec::new();
    let gx = grid.cells_x() as usize;

    for (src_tile, local_orgs) in tile_organisms.iter().enumerate() {
        let e_tile = &energy_tiles[src_tile];
        for &(demand_index, org) in local_orgs {
            let stencil = ExchangeStencil::at(&grid, org.position)?;
            let amount = match org.explicit_request {
                Some(explicit) => {
                    if !explicit.is_finite() || explicit < 0.0 {
                        return Err(CoreError::NumericalInvalid {
                            quantity: "uptake demand",
                            detail: format!("organism {} requests {explicit}", org.organism.get()),
                        });
                    }
                    explicit
                }
                None => {
                    let rule = rule.ok_or_else(|| CoreError::StateConflict {
                        operation: "partitioned uptake",
                        detail: "missing RequestRule for organism without explicit_request"
                            .to_owned(),
                    })?;
                    let mut local_energy = 0.0_f64;
                    for (&cell, &weight) in stencil.cells().iter().zip(stencil.weights()) {
                        let cx = (cell % gx) as u32;
                        let cy = (cell / gx) as u32;
                        let e = e_tile
                            .read_local_or_halo(&grid, ENERGY_PLANE as usize, cx, cy)
                            .ok_or_else(|| CoreError::LayoutMismatch {
                                buffer: "partitioned uptake halo",
                                detail: format!(
                                    "cell ({cx}, {cy}) outside 1-cell halo of tile {src_tile}"
                                ),
                            })?;
                        local_energy += weight * f64::from(e);
                    }
                    let density = local_energy / grid.cell_area();
                    rule.request(density)?
                }
            };
            for (&cell, &weight) in stencil.cells().iter().zip(stencil.weights()) {
                let requested = amount * weight;
                if requested == 0.0 {
                    continue;
                }
                let dst_tile = partition.tile_of_cell_index(cell)?;
                let pkt = GrantPacket {
                    cell,
                    organism: org.organism,
                    demand: demand_index,
                    requested,
                    granted: 0.0,
                };
                if dst_tile != src_tile {
                    boundary_demands.push(BoundaryDemandMsg {
                        from_tile: src_tile,
                        to_tile: dst_tile,
                        cell,
                        organism: org.organism,
                        demand_index,
                        requested,
                    });
                }
                owner_packets[dst_tile].push((src_tile, pkt));
            }
        }
    }

    // Phase 5b: Each owning tile reconciles ONLY its own interior cells using
    // its local energy buffer and `resolve_cell`, guaranteeing strict `INV-04`
    // and bit-exact parity with global `resolve_uptake`.
    let mut all_debits: Vec<CellDebit> = Vec::new();
    let mut all_packets: Vec<GrantPacket> = Vec::new();
    let mut boundary_grants: Vec<BoundaryGrantMsg> = Vec::new();
    let mut shares_buf: Vec<Share> = Vec::new();
    let mut rank_buf: Vec<usize> = Vec::new();

    for (owner_tile, packets_with_src) in owner_packets.iter_mut().enumerate() {
        packets_with_src.sort_unstable_by_key(|(_, pkt)| (pkt.cell, pkt.organism));
        let e_tile = &energy_tiles[owner_tile];
        let mut cursor = 0_usize;
        while cursor < packets_with_src.len() {
            let cell = packets_with_src[cursor].1.cell;
            let mut end = cursor + 1;
            while end < packets_with_src.len() && packets_with_src[end].1.cell == cell {
                end += 1;
            }
            let cx = (cell % gx) as u32;
            let cy = (cell / gx) as u32;
            let cell_e = e_tile
                .read_local_or_halo(&grid, ENERGY_PLANE as usize, cx, cy)
                .ok_or_else(|| CoreError::LayoutMismatch {
                    buffer: "partitioned uptake owner",
                    detail: format!("owned cell ({cx}, {cy}) missing from tile {owner_tile}"),
                })?;
            let mut cell_pkts: Vec<GrantPacket> = packets_with_src[cursor..end]
                .iter()
                .map(|(_, pkt)| *pkt)
                .collect();
            let debit = resolve_cell(&mut cell_pkts, cell, cell_e, &mut shares_buf, &mut rank_buf)?;
            all_debits.push(debit);

            // Phase 5c: Emit cross-seam grant replies back to requesting tiles.
            for ((src_tile, _), resolved) in
                packets_with_src[cursor..end].iter().zip(cell_pkts.iter())
            {
                if *src_tile != owner_tile {
                    boundary_grants.push(BoundaryGrantMsg {
                        from_tile: owner_tile,
                        to_tile: *src_tile,
                        cell,
                        organism: resolved.organism,
                        demand_index: resolved.demand,
                        granted: resolved.granted,
                    });
                }
                all_packets.push(*resolved);
            }
            cursor = end;
        }
    }

    // Reorder merged global inspection arrays into canonical ascending cell
    // order (matching `Grants::debits()`, `Grants::packets()`, and `add_down`
    // summation order in `Grants::totals()`).
    all_debits.sort_unstable_by_key(|d| d.cell);
    all_packets.sort_unstable_by_key(|p| (p.cell, p.organism));

    let mut totals = vec![0.0_f64; organisms.len()];
    for pkt in &all_packets {
        totals[pkt.demand] = add_down(totals[pkt.demand], pkt.granted);
    }

    let mut req_sum = CompensatedSum::ZERO;
    let mut grant_sum = CompensatedSum::ZERO;
    let mut summary = UptakeSummary::default();
    let mut p_cursor = 0_usize;
    for debit in &all_debits {
        req_sum.add(debit.demanded);
        grant_sum.add(debit.granted);
        summary.cells += 1;
        summary.contended_cells += usize::from(debit.contended);
        summary.drained_cells += usize::from(debit.granted > 0.0 && debit.energy_after == 0.0);
        let start = p_cursor;
        while p_cursor < all_packets.len() && all_packets[p_cursor].cell == debit.cell {
            p_cursor += 1;
        }
        summary.largest_cell_packets = summary.largest_cell_packets.max(p_cursor - start);
    }
    summary.requested = req_sum.value();
    summary.granted = grant_sum.value();

    let comm = UptakeCommStats {
        energy_halo_cells,
        energy_halo_bytes: energy_halo_cells * 4,
        boundary_demand_packets: boundary_demands.len() as u64,
        boundary_demand_bytes: (boundary_demands.len() as u64) * BOUNDARY_DEMAND_WIRE_BYTES,
        boundary_grant_packets: boundary_grants.len() as u64,
        boundary_grant_bytes: (boundary_grants.len() as u64) * BOUNDARY_GRANT_WIRE_BYTES,
        barriers: if tile_count > 1 { 3 } else { 0 },
    };

    Ok(PartitionedUptakeResult {
        summary,
        debits: all_debits,
        packets: all_packets,
        totals,
        comm,
    })
}

/// One organism's paid emission deposit staged on its owning tile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PartitionedDeposit {
    /// Emitting organism identity.
    pub organism: OrganismId,
    /// Post-motion wrapped world position.
    pub position: [f64; 2],
    /// Energy paid `w`.
    pub paid: f64,
    /// Energy offered `ρ w <= w`.
    pub offered: f64,
    /// Unit-ball chemical direction `a`.
    pub direction: [f64; CHANNELS],
}

/// One cross-seam deposit packet routed from the emitter's tile to the target
/// cell's owning tile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundaryDepositMsg {
    /// Tile owning the emitter.
    pub from_tile: usize,
    /// Tile owning `cell`.
    pub to_tile: usize,
    /// Global cell index.
    pub cell: usize,
    /// Emitting organism identity.
    pub organism: OrganismId,
    /// Offered packet energy `q_c`.
    pub amount: f64,
    /// Chemical direction `a`.
    pub direction: [f64; CHANNELS],
}

/// Communication statistics for [`apply_partitioned_deposits`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DepositCommStats {
    /// Cross-seam deposit packets routed (`from_tile != to_tile`).
    pub boundary_packets: u64,
    /// Wire bytes for cross-seam deposit packets.
    pub boundary_bytes: u64,
    /// Barriers executed.
    pub barriers: u32,
}

/// Route and apply paid emission deposits across `partition`'s disjoint cell
/// owners in canonical ascending `OrganismId` order, reproducing
/// [`crate::apply_deposits`] bit-for-bit (`0 ULP`).
///
/// # Errors
/// [`CoreError::LayoutMismatch`] for mismatched grid/field;
/// [`CoreError::NumericalInvalid`] for invalid positions/amounts/directions.
pub fn apply_partitioned_deposits(
    partition: &TilePartition,
    field: &mut FieldState,
    deposits: &[PartitionedDeposit],
) -> Result<(DepositSummary, DepositCommStats), CoreError> {
    let grid = *partition.grid();
    let expected = crate::layout::FieldLayout::planar(grid)?;
    expected.ensure_compatible(field.layout())?;
    struct DepositTilePacket {
        cell: usize,
        organism: OrganismId,
        amount: f64,
        direction: [f64; CHANNELS],
    }

    let tile_count = partition.tile_count();
    let mut owner_packets: Vec<Vec<DepositTilePacket>> =
        (0..tile_count).map(|_| Vec::new()).collect();
    let mut boundary_packets = 0_u64;

    let mut paid_sum = CompensatedSum::ZERO;
    let mut offered_sum = CompensatedSum::ZERO;

    // Sort entries by ascending `OrganismId` for deterministic `paid`/`offered`
    // compensated totals matching `Deposits`.
    let mut ordered_indices: Vec<usize> = (0..deposits.len()).collect();
    ordered_indices.sort_unstable_by_key(|&i| deposits[i].organism);

    for &idx in &ordered_indices {
        let dep = &deposits[idx];
        if !(dep.paid.is_finite()
            && dep.paid >= 0.0
            && dep.offered.is_finite()
            && dep.offered >= 0.0)
            || dep.offered > dep.paid
        {
            return Err(CoreError::NumericalInvalid {
                quantity: "partitioned deposit energy",
                detail: format!("invalid paid={:?}, offered={:?}", dep.paid, dep.offered),
            });
        }
        let squared = dep
            .direction
            .iter()
            .fold(0.0_f64, |total, &component| total + component * component);
        if !(squared.is_finite() && squared <= 1.0) {
            return Err(CoreError::NumericalInvalid {
                quantity: "deposit direction",
                detail: format!("||a||² = {squared} is outside the unit ball"),
            });
        }
        paid_sum.add(dep.paid);
        offered_sum.add(dep.offered);

        let src_tile = partition.tile_of_position(dep.position)?;
        let stencil = ExchangeStencil::at(&grid, dep.position)?;
        let shares = split_deposit_offer(&stencil, dep.offered);
        for (&cell, &amount) in stencil.cells().iter().zip(&shares) {
            if amount <= 0.0 {
                continue;
            }
            let dst_tile = partition.tile_of_cell_index(cell)?;
            if dst_tile != src_tile {
                boundary_packets += 1;
            }
            owner_packets[dst_tile].push(DepositTilePacket {
                cell,
                organism: dep.organism,
                amount,
                direction: dep.direction,
            });
        }
    }

    // Reduce each cell strictly on its owning tile in ascending `(cell, organism)` order.
    let cells = usize::try_from(grid.cell_count()).map_err(|_| CoreError::Overflow {
        quantity: "partitioned deposit cells",
    })?;
    let mut cell_received: Vec<(usize, f64)> = Vec::new();

    for packets in &mut owner_packets {
        packets.sort_unstable_by_key(|p| (p.cell, p.organism));
        let mut cursor = 0_usize;
        while cursor < packets.len() {
            let cell = packets[cursor].cell;
            let mut end = cursor + 1;
            while end < packets.len() && packets[end].cell == cell {
                end += 1;
            }
            let slice = &packets[cursor..end];
            let mut total_s = 0.0_f64;
            let mut moment_sum = [0.0_f64; CHANNELS];
            for pkt in slice {
                total_s = add_down(total_s, pkt.amount);
                for (acc, &a) in moment_sum.iter_mut().zip(&pkt.direction) {
                    *acc += pkt.amount * a;
                }
            }
            let energy_before = field.energy()[cell];
            let energy_after = round_down_f32(f64::from(energy_before) + total_s);
            let received = (f64::from(energy_after) - f64::from(energy_before)).max(0.0);
            field.energy_mut()[cell] = energy_after;
            if received > 0.0 && total_s > 0.0 {
                let scale = received / total_s;
                let data = field.as_mut_slice();
                for (ch, &m_acc) in moment_sum.iter().enumerate() {
                    let idx = (1 + ch) * cells + cell;
                    data[idx] = (f64::from(data[idx]) + scale * m_acc) as f32;
                }
            }
            cell_received.push((cell, received));
            cursor = end;
        }
    }

    cell_received.sort_unstable_by_key(|&(cell, _)| cell);
    let mut applied_sum = CompensatedSum::ZERO;
    for &(_, received) in &cell_received {
        applied_sum.add(received);
    }

    let summary = DepositSummary {
        organisms: deposits.len(),
        cells: cell_received.len(),
        paid: paid_sum.value(),
        offered: offered_sum.value(),
        applied: applied_sum.value(),
    };
    let comm = DepositCommStats {
        boundary_packets,
        boundary_bytes: boundary_packets * BOUNDARY_DEPOSIT_WIRE_BYTES,
        barriers: if tile_count > 1 { 1 } else { 0 },
    };
    Ok((summary, comm))
}

/// An organism record tracked across post-motion tile migration.
#[derive(Debug, Clone, PartialEq)]
pub struct MigratingOrganism<P> {
    /// Stable organism identity (`INV-07`).
    pub id: OrganismId,
    /// Post-motion world position (may need periodic wrapping).
    pub position: [f64; 2],
    /// Canonical controller parameter byte size carried on migration (`F16`:
    /// `2 * params`, `F32`: `4 * params`).
    pub genome_bytes: u64,
    /// Arbitrary organism payload (traits, energy, controller weights, etc.).
    pub payload: P,
}

/// Statistics for one atomic post-motion migration barrier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MigrationStats {
    /// Total live organisms before and after migration (invariant).
    pub total_organisms: usize,
    /// Organisms that crossed a tile seam (`dst_tile != src_tile`).
    pub migrated_organisms: usize,
    /// Exact wire bytes transferred (`migrated * header + sum(genome_bytes)`).
    pub migration_bytes: u64,
    /// Highest live organism count on any tile after migration.
    pub max_tile_organisms: usize,
    /// Lowest live organism count on any tile after migration.
    pub min_tile_organisms: usize,
    /// `max_tile / mean_tile` load imbalance (`1.0` = perfectly balanced).
    pub load_imbalance_ratio: f64,
}

/// Atomically migrate post-motion organisms whose wrapped positions moved into
/// another tile, preserving exact organism count, `INV-07` uniqueness, and
/// canonical ascending `OrganismId` order within every destination tile.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] if `tiles.len() != partition.tile_count()`;
/// [`CoreError::NumericalInvalid`] for nonfinite positions;
/// [`CoreError::StateConflict`] if duplicate `OrganismId`s are encountered.
pub fn migrate_organisms<P>(
    partition: &TilePartition,
    tiles: &mut [Vec<MigratingOrganism<P>>],
) -> Result<MigrationStats, CoreError> {
    let tile_count = partition.tile_count();
    if tiles.len() != tile_count {
        return Err(CoreError::LayoutMismatch {
            buffer: "partition migration",
            detail: format!("got {} tile vecs for {tile_count} tiles", tiles.len()),
        });
    }
    // Phase 1 (verify-before-mutate): validate every organism's uniqueness
    // (`INV-07`) and finite position before moving any item out of `tiles`,
    // ensuring an error leaves `tiles` completely intact.
    let mut seen_ids: BTreeMap<OrganismId, usize> = BTreeMap::new();
    let mut plans: Vec<Vec<([f64; 2], usize)>> = Vec::with_capacity(tile_count);
    for (src_tile, local) in tiles.iter().enumerate() {
        let mut tile_plan = Vec::with_capacity(local.len());
        for org in local {
            if let Some(prev_tile) = seen_ids.insert(org.id, src_tile) {
                return Err(CoreError::StateConflict {
                    operation: "migrate partitioned organisms",
                    detail: format!(
                        "INV-07 duplicate organism {} present on tiles {prev_tile} and {src_tile}",
                        org.id
                    ),
                });
            }
            let wrapped = partition.grid().wrap_position(org.position)?;
            let dst_tile = partition.tile_of_position(wrapped)?;
            tile_plan.push((wrapped, dst_tile));
        }
        plans.push(tile_plan);
    }

    // Phase 2 (infallible atomic move): route every organism to its validated
    // destination tile.
    let mut incoming: Vec<Vec<MigratingOrganism<P>>> =
        (0..tile_count).map(|_| Vec::new()).collect();
    let mut total_organisms = 0_usize;
    let mut migrated_organisms = 0_usize;
    let mut migration_bytes = 0_u64;

    for ((src_tile, local), tile_plan) in tiles.iter_mut().enumerate().zip(plans) {
        let taken = std::mem::take(local);
        for (mut org, (wrapped, dst_tile)) in taken.into_iter().zip(tile_plan) {
            total_organisms += 1;
            org.position = wrapped;
            if dst_tile == src_tile {
                local.push(org);
            } else {
                migrated_organisms += 1;
                migration_bytes = migration_bytes
                    .saturating_add(MIGRATION_HEADER_WIRE_BYTES)
                    .saturating_add(org.genome_bytes);
                incoming[dst_tile].push(org);
            }
        }
    }

    let mut max_tile = 0_usize;
    let mut min_tile = usize::MAX;
    for (tile_vec, mut arrivals) in tiles.iter_mut().zip(incoming) {
        tile_vec.append(&mut arrivals);
        tile_vec.sort_unstable_by_key(|org| org.id);
        max_tile = max_tile.max(tile_vec.len());
        min_tile = min_tile.min(tile_vec.len());
    }
    if tile_count == 0 {
        min_tile = 0;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "organism and tile counts are far below 2^53"
    )]
    let load_imbalance_ratio = if total_organisms == 0 {
        1.0
    } else {
        (max_tile as f64) / ((total_organisms as f64) / (tile_count as f64))
    };

    Ok(MigrationStats {
        total_organisms,
        migrated_organisms,
        migration_bytes,
        max_tile_organisms: max_tile,
        min_tile_organisms: min_tile,
        load_imbalance_ratio,
    })
}

/// Real interconnect characteristics for multi-device communication budgeting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InterconnectProfile {
    /// Human-readable interconnect name.
    pub name: &'static str,
    /// Effective bidirectional per-device payload bandwidth in GB/s (10^9 B/s).
    pub bandwidth_gb_s: f64,
    /// One-way collective barrier latency in microseconds.
    pub barrier_latency_us: f64,
}

/// Representative hardware interconnects evaluated for FL-058 (plan §20.4, §24.2).
pub const INTERCONNECT_PROFILES: [InterconnectProfile; 5] = [
    InterconnectProfile {
        name: "10GbE TCP",
        bandwidth_gb_s: 1.1,
        barrier_latency_us: 80.0,
    },
    InterconnectProfile {
        name: "80Gbps Thunderbolt 5",
        bandwidth_gb_s: 7.5,
        barrier_latency_us: 25.0,
    },
    InterconnectProfile {
        name: "100GbE RDMA (RoCEv2)",
        bandwidth_gb_s: 11.5,
        barrier_latency_us: 8.0,
    },
    InterconnectProfile {
        name: "400GbE InfiniBand NDR",
        bandwidth_gb_s: 45.0,
        barrier_latency_us: 3.0,
    },
    InterconnectProfile {
        name: "NVLink / UltraFusion-class",
        bandwidth_gb_s: 400.0,
        barrier_latency_us: 1.0,
    },
];

/// Inputs to [`estimate_comm_budget`] combining exact partition geometry with
/// empirically measured seam traffic from a real simulation run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommBudgetInput {
    /// Field plane count (`33` single-carrier or `34` dual-carrier).
    pub planes: u32,
    /// Transport substeps per tick.
    pub substeps: u32,
    /// Halo exchange mode.
    pub halo_mode: HaloMode,
    /// Number of coarse sensor pyramid levels (`0..=pyramid_levels`).
    pub pyramid_levels: u32,
    /// Measured cross-seam uptake demand packets per tick (global total).
    pub boundary_demand_packets_per_tick: u64,
    /// Measured cross-seam deposit packets per tick (global total).
    pub boundary_deposit_packets_per_tick: u64,
    /// Measured cross-seam migrating organisms per tick (global total).
    pub migrated_organisms_per_tick: u64,
    /// Mean canonical controller genome bytes per organism (`2 * params` F16).
    pub mean_genome_bytes: u64,
}

/// Per-tick multi-device communication breakdown (global sum across all tiles
/// and mean per device).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommBudgetBreakdown {
    /// Total devices (`tiles_x * tiles_y`).
    pub devices: usize,
    /// Field transport halo bytes/tick (all devices combined).
    pub transport_halo_bytes: u64,
    /// Multi-level sensor pyramid halo bytes/tick (`1`-cell halo at every level
    /// `0..=pyramid_levels` for bilinear derivative sampling).
    pub pyramid_halo_bytes: u64,
    /// Boundary uptake demand + grant + energy-halo bytes/tick.
    pub uptake_boundary_bytes: u64,
    /// Boundary emission deposit bytes/tick.
    pub deposit_boundary_bytes: u64,
    /// Post-motion organism + full-genome migration bytes/tick.
    pub migration_bytes: u64,
    /// Total global inter-device wire bytes/tick.
    pub total_bytes_per_tick: u64,
    /// Mean bytes sent/received per device per tick.
    pub per_device_bytes_per_tick: u64,
    /// Synchronous tick barriers per tick (`transport + pyramid + 2 uptake + deposit + migration`).
    pub barriers_per_tick: u32,
}

/// Estimated per-tick communication overhead on one [`InterconnectProfile`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InterconnectEstimate {
    /// Evaluated interconnect.
    pub profile: InterconnectProfile,
    /// Serialization + transfer time per tick (ms) for `per_device_bytes_per_tick`.
    pub transfer_ms: f64,
    /// Collective barrier synchronization time per tick (ms).
    pub barrier_ms: f64,
    /// Total communication overhead per tick (`transfer_ms + barrier_ms`).
    pub total_comm_ms: f64,
    /// Fraction of a `30 t/s` (`33.33 ms`) tick consumed purely by communication.
    pub fraction_of_30hz_tick: f64,
}

/// Compute exact per-tick communication bytes and interconnect overhead
/// estimates for `partition` and measured workload `input`.
#[must_use]
pub fn estimate_comm_budget(
    partition: &TilePartition,
    input: CommBudgetInput,
) -> (CommBudgetBreakdown, Vec<InterconnectEstimate>) {
    let devices = partition.tile_count();
    if devices <= 1 {
        let empty = CommBudgetBreakdown {
            devices,
            transport_halo_bytes: 0,
            pyramid_halo_bytes: 0,
            uptake_boundary_bytes: 0,
            deposit_boundary_bytes: 0,
            migration_bytes: 0,
            total_bytes_per_tick: 0,
            per_device_bytes_per_tick: 0,
            barriers_per_tick: 0,
        };
        let est = INTERCONNECT_PROFILES
            .iter()
            .map(|&profile| InterconnectEstimate {
                profile,
                transfer_ms: 0.0,
                barrier_ms: 0.0,
                total_comm_ms: 0.0,
                fraction_of_30hz_tick: 0.0,
            })
            .collect();
        return (empty, est);
    }

    let substeps = input.substeps.max(1);
    let (halo_w, transport_barriers) = match input.halo_mode {
        HaloMode::PerSubstep => (1_u32, substeps),
        HaloMode::WideTick => (substeps, 1_u32),
    };
    let per_barrier_halo_cells: u64 = (0..devices)
        .map(|t| partition.halo_cells_per_tile(t, halo_w))
        .sum();
    let transport_halo_bytes = per_barrier_halo_cells
        .saturating_mul(u64::from(transport_barriers))
        .saturating_mul(u64::from(input.planes))
        .saturating_mul(4);

    // Sensor pyramid: level 0 plus each 2x-coarsened level up to `pyramid_levels`
    // needs at least a 2-cell halo (for central derivatives + bilinear stencil).
    let mut pyramid_halo_bytes = 0_u64;
    let mut gx = partition.grid().cells_x();
    let mut gy = partition.grid().cells_y();
    for level in 0..=input.pyramid_levels {
        if level > 0 {
            gx = gx.div_ceil(2).max(1);
            gy = gy.div_ceil(2).max(1);
        }
        let level_tx = partition.tiles_x().min(gx);
        let level_ty = partition.tiles_y().min(gy);
        if let Ok(level_grid) = FieldGrid::new(
            gx,
            gy,
            partition.grid().world_width(),
            partition.grid().world_height(),
        ) && let Ok(level_part) = TilePartition::new(level_grid, level_tx, level_ty)
        {
            let halo_cells: u64 = (0..level_part.tile_count())
                .map(|t| level_part.halo_cells_per_tile(t, 2))
                .sum();
            pyramid_halo_bytes = pyramid_halo_bytes.saturating_add(
                halo_cells
                    .saturating_mul(u64::from(input.planes))
                    .saturating_mul(4),
            );
        }
    }

    let energy_halo_cells: u64 = (0..devices)
        .map(|t| partition.halo_cells_per_tile(t, 1))
        .sum();
    let uptake_boundary_bytes = energy_halo_cells.saturating_mul(4).saturating_add(
        input
            .boundary_demand_packets_per_tick
            .saturating_mul(BOUNDARY_DEMAND_WIRE_BYTES + BOUNDARY_GRANT_WIRE_BYTES),
    );
    let deposit_boundary_bytes = input
        .boundary_deposit_packets_per_tick
        .saturating_mul(BOUNDARY_DEPOSIT_WIRE_BYTES);
    let migration_bytes = input
        .migrated_organisms_per_tick
        .saturating_mul(MIGRATION_HEADER_WIRE_BYTES.saturating_add(input.mean_genome_bytes));

    let total_bytes_per_tick = transport_halo_bytes
        .saturating_add(pyramid_halo_bytes)
        .saturating_add(uptake_boundary_bytes)
        .saturating_add(deposit_boundary_bytes)
        .saturating_add(migration_bytes);
    let per_device_bytes_per_tick = total_bytes_per_tick.div_ceil(devices as u64);
    // Barriers: transport + pyramid + uptake(demand) + uptake(grant) + deposit + migration
    let barriers_per_tick = transport_barriers + 5;

    let breakdown = CommBudgetBreakdown {
        devices,
        transport_halo_bytes,
        pyramid_halo_bytes,
        uptake_boundary_bytes,
        deposit_boundary_bytes,
        migration_bytes,
        total_bytes_per_tick,
        per_device_bytes_per_tick,
        barriers_per_tick,
    };

    #[expect(
        clippy::cast_precision_loss,
        reason = "communication bytes per tick are far below 2^53"
    )]
    let estimates = INTERCONNECT_PROFILES
        .iter()
        .map(|&profile| {
            // Bytes / (GB/s) = nanoseconds; divide by 1e6 -> milliseconds.
            let transfer_ms = (per_device_bytes_per_tick as f64) / (profile.bandwidth_gb_s * 1.0e6);
            let barrier_ms = f64::from(barriers_per_tick) * profile.barrier_latency_us * 1.0e-3;
            let total_comm_ms = transfer_ms + barrier_ms;
            InterconnectEstimate {
                profile,
                transfer_ms,
                barrier_ms,
                total_comm_ms,
                fraction_of_30hz_tick: total_comm_ms / (1000.0 / 30.0),
            }
        })
        .collect();

    (breakdown, estimates)
}
