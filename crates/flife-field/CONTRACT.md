# flife-field — contract

**Purpose.** FL-003 (`flife-82g`) covers the field representation of plan §4.2, §9.3 and §19.3. The field is 33
typed F32 planes (energy plus 32 chemical moments) on a periodic grid. It comes with a semantic layout descriptor
that FrankenNumPy supplies and checks, per-buffer limits (INV-15), the configured initial field, and the INV-01
and INV-02 checks for a committed field.

**Inputs.**
- A `FieldGrid` (cells and world size), or a `ValidatedConfig` (`world` and `[field_initialization]`).
- `BufferLimits`: the largest single buffer a backend can allocate.
- A `DeclaredLayout` read from a checkpoint or interchange record.
- A `ConeTolerance` taken from the declared numerical profile.

**Outputs.**
- `FieldState`: one planar `[plane, y, x]` buffer of `33 G_x G_y` real `f32` values. Plane 0 holds energy;
  plane `k + 1` holds moment channel `k`. Every plane is a cell amount in `ecological-energy`; divide by
  `cell_area` for a density.
- `FieldLayout`: arrangement, shape, byte strides, dtype, base alignment, ownership, unit, measure and grid.
- `FieldView`: a borrowed buffer checked against its declared layout.
- `relayout`: a copy into another arrangement that moves each value to its semantic index.
- `FieldPair`: the current committed state plus the next candidate; `commit` swaps them without reallocating.
- `FieldFootprint`: checked byte counts.
- `InitialEnergy`: the stored per-cell value and the exact injected total, for the ledger.
- `FieldSummary`: energy total, maximum cone residual and empty-cell count.
- `DeclaredLayout::canonical_text` and its SHA-256.

**Invariants.**
- Storage is `Vec<f32>`. It is never an `f64` copy carrying an F32 tag. Moving to or from FrankenNumPy
  `ArrayStorage::F32` moves the buffer without copying it, and `F64` storage is refused.
- Strides come from `fnp_ndarray::contiguous_strides` and are cross-checked against independent `u64`
  arithmetic. Every `(plane, x, y)` has exactly one byte offset, and those offsets cover the buffer exactly.
- A reader states the layout it needs. Arrangement, grid, world size (and therefore cell size and energy
  density), shape, strides and length must all agree. Equal byte lengths never make two layouts compatible.
- A declared layout is admitted before any allocation (Appendix B.2). It must name this schema and dtype `f32`
  with 4-byte items. Its axis names must match the arrangement, and it must have 33 planes, an
  `ecological-energy` `cell-integrated` unit and 4-byte alignment. FrankenNumPy checks that the view stays
  inside the declared bytes and does not overlap itself. The strides must be the canonical contiguous ones, and
  the byte length must be exact.
- Every count is checked. A state is admitted against `BufferLimits` and against `isize::MAX` bytes before it
  is allocated. The allocation itself uses `try_reserve_exact`, so a refusal is reported as an error instead of
  aborting the process.
- Periodic boundary. A cell index wraps by `rem_euclid`. A coordinate wraps into `[0, L)`, and a coordinate
  within rounding below zero maps to `0.0`, never to `L`. The minimum image lies in `[-L/2, L/2)`, and its
  correction is exact (Sterbenz).
- INV-01: every energy is finite and nonnegative (`-0.0` counts as zero), and every moment is finite.
  INV-02: `max(0, ||m||₂ − e) ≤ absolute + relative · e`. The checks run in F64 over the exact F32 values,
  summing squares in channel order and energy in cell-index order.
- Initial field. `energy_per_cell` is rounded once to F32 and the composition is `zero` (no moment). The stored
  value is reported. `injected_total` equals the cell-order F64 sum of the stored field whenever the grid has at
  most 2^29 cells.

**Refusal cases.**
- A zero cell count, or a nonfinite or nonpositive world or cell side.
- Byte counts past `u64::MAX` or `isize::MAX`.
- A buffer above its limit.
- Any layout mismatch, including one of equal length.
- A slice whose length disagrees with its declared layout.
- Each inconsistent declaration listed under Invariants.
- Non-F32 storage.
- An unimplemented boundary, dtype, channel count or composition.
- An initial energy beyond F32.
- The first cell that violates INV-01 or INV-02, named by its coordinates.

**Determinism.** Pure functions of their inputs, with fixed iteration orders. No hash-map iteration, and no
parallel reduction here.

**Validity domain.** `field-ecology-v1` with the `periodic` boundary, the `f32` field dtype and the `zero`
composition, all implemented here. Layout schema `frankenlife.field-layout.v1`, host buffers only.

**Independent tests** (`tests/field_layout.rs`). Expected strides are NumPy's
`np.zeros(shape, np.float32).strides`. The pinned digest is `shasum -a 256` of the literal canonical text. Byte
counts are §19.3's table: 4 MiB, 128 MiB, 132 MiB and 264 MiB at 1024², measured on the real allocation as well
as computed.

**Known gaps.**
- Device arrangements such as channel-blocked tiles, device alignment, and sharding by maximum buffer length are
  the Metal backend's work. Their layouts will need their own descriptors and benchmarks (§9.3, §9.2).
- Transport and source injection are FL-004. Uptake and the reduction rule of the ledger are FL-005. Sensing and
  the pyramid are FL-007.
- Whole-run peak-memory admission is FL-043.
- The cone tolerance values belong to a numerical profile that is not yet declared.
