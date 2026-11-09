# flife-field — contract

**Purpose.**
- FL-003 (`flife-82g`) covers the field representation of plan §4.2, §9.3 and §19.3. The field is 33 typed F32
  planes (energy plus 32 chemical moments) on a periodic grid. It comes with a semantic layout descriptor that
  FrankenNumPy supplies and checks, per-buffer limits (INV-15), the configured initial field, and the INV-01 and
  INV-02 checks.
- FL-004 (`flife-03d`) covers phase 2 of the tick (§4.5, §8.1, §11.5, A.3 items 7–9). That is
  `positive-five-point-v1` transport with `subcycle` stability, exponential decay, and `fixed-total-power`
  deposits through the `normalized-truncated-minimum-image-gaussian-v1` kernel with `fixed-per-source`
  chemistry. It also covers the analytic `seeded-smooth-torus-v1` path integrator.
- FL-005 (`flife-f6x`) covers local uptake (§5.1, §5.2, §11.3, A.1, Appendix C.1, INV-04). That is the
  `grid-cic-v1` exchange footprint, A.1's saturating request, two-pass demand/grant resolution with
  `cell-quantum-largest-remainder-v1` reconciliation, and the proportional field debit. The energy ledger it
  feeds is `flife_core::ledger`.
- FL-007 (`flife-bzl`) covers the sensor density pyramid (§6.1, §8.1 phase 3, C.10): `positive-box-v1` density
  levels, bilinear sampling with analytic physical derivatives, and `log-linear-v1` scale blending. Encoding the
  sample into controller inputs is `flife-neural`.
- FL-010 (`flife-rf8`) supplies FL-004's seeded draws from `fnp-philox-semantic-v1` (A.3 items 7 and 8, §16.2):
  chemical directions, initial source centers and per-segment travel directions, each named by the source's
  stable identity.
- FL-011 (`flife-t24`) covers the reduction of paid emission deposits into the next field (§5.5, §8.1 phase 9,
  INV-03, INV-05, C.4). The emission settlement that stages them is `flife_engine::emission`.

**Inputs.**
- A `FieldGrid` (cells and world size), or a `ValidatedConfig` (`world`, `[field_initialization]`, `[sources]`).
- `BufferLimits`: the largest single buffer a backend can allocate.
- A `DeclaredLayout` read from a checkpoint or interchange record.
- A `ConeTolerance` taken from the declared numerical profile.
- `Source`s for this tick: identity, sampled center, chemical direction and power. Segment directions for
  `PathSegment`. Seeded values come from a `SemanticRng` through `seeding` (FL-010); nothing else here draws.
- `Demand`s for this tick: organism identity, position and requested amount `d_i`. Each organism demands at
  most once per tick.
- For sensing: the observation snapshot's `FieldState`, the pyramid level count, and a position and sensing scale
  per query.
- For deposits (FL-011): each emitter's identity, post-motion position, payment `w`, offer `ρ w` and chemical
  direction, and the next field after that tick's uptake debits.

**Outputs.**
- `FieldState`: one planar `[plane, y, x]` buffer of `33 G_x G_y` real `f32` values. Plane 0 holds energy;
  plane `k + 1` holds moment channel `k`. Every plane is a cell amount in `ecological-energy`; divide by
  `cell_area` for a density. `energy_total` gives the field's holdings for the ledger.
- `FieldLayout`, `FieldView`, `relayout`, and `FieldPair` (current/next; `commit` swaps without reallocating;
  `next` reads the candidate buffer, which FL-013's `Candidate::field` exposes before publication).
- `FieldFootprint`, `InitialEnergy` and `FieldSummary`.
- `DeclaredLayout::canonical_text` and its SHA-256.
- `TransportPlan`: requested and per-substep `r_x`, `r_y`, the substep count, the center coefficient and
  `exp(−λΔt)`.
- `transport` with its `TransportReport`: energy before, transported and after; decay removal; transport
  residual.
- `SourceKernel`, `SourceSupport` (reused across ticks), `deposit` with its `DepositReport` (declared,
  normalized and applied energy), and `unit_direction`.
- `PathSegment`: position, velocity, mid-tick fraction and the next segment.
- `seeding` (FL-010): `chemical_direction`, `initial_center`, `travel_direction`, `initial_source`, and
  `SeededPath` (source, segment ordinal and current segment). FL-016 adds `SeededPath::resume`, which rebuilds
  a path from its source, ordinal and anchor, `PathSegment::anchor`, and `ConeTolerance::{absolute, relative}`.
- `advance_field` with its `FieldStepReport`: the field's ledger terms `I`, `D_decay` and `ε` for one tick.
- `numerics`: the host-independent `exp_neg`, `expm1_neg`, `tanh`, `sigmoid` and `log2`, re-exported from
  `flife-core::numerics`.
- `SensorPyramid`: reusable coarse-level storage admitted against `BufferLimits`, with its `LevelGrid`s and level
  scales `s_l = 2^l max(h_x, h_y)`. `build` returns a `PyramidView` that borrows both the storage and the field,
  so a view cannot outlive, or disagree with, the snapshot it was built from. The view gives `density`, `blend`
  (level and `t`), `sample_level` and `sample`; the last two return a `RawSample` of 33 raw densities with their
  derivatives along x and y.
- `ExchangeStencil`: an organism's footprint cells and weights, and its local density `Σ w_c e_c / (h_x h_y)`.
  `saturating_request` gives `d = Δt U_max r/(K + r)`.
- `resolve_uptake` fills reusable `Grants`. These hold the `GrantPacket`s sorted by (cell, organism), one
  `CellDebit` per demanded cell, each organism's total `U_i` in input order, and an `UptakeSummary`.
  `apply_grants` debits the field.
- `deposit` (FL-011): reusable `Deposits` (`push`, `clear`, `len`, `is_applied`) and `apply_deposits`, which adds
  them to the field and returns a `DepositSummary` (organisms, cells, paid, offered and applied).
  `DepositSummary::record` books the reduction as one `emission` transfer in a `TickLedger`.

**Invariants — representation (FL-003).**
- Storage is `Vec<f32>`, never an `f64` copy carrying an F32 tag. Moving to or from FrankenNumPy
  `ArrayStorage::F32` moves the buffer without copying it, and `F64` storage is refused.
- Strides come from `fnp_ndarray::contiguous_strides` and are cross-checked against independent `u64`
  arithmetic. Every `(plane, x, y)` has exactly one byte offset, and those offsets cover the buffer exactly.
- A reader states the layout it needs. Arrangement, grid, world size (and therefore cell size and energy
  density), shape, strides and length must all agree. Equal byte lengths never make two layouts compatible.
- A declared layout is admitted before allocation (Appendix B.2). It must name this schema and dtype `f32`
  with 4-byte items. Its axis names must match the arrangement, and it must have 33 planes, an
  `ecological-energy` `cell-integrated` unit and 4-byte alignment. FrankenNumPy checks that the view stays
  inside the declared bytes and does not overlap itself. The strides must be the canonical contiguous ones, and
  the byte length must be exact.
- Every count is checked. Buffers are admitted against `BufferLimits` and `isize::MAX` before they are
  allocated. The allocation itself uses `try_reserve_exact`, so a refusal is reported as an error instead of
  aborting the process.
- Periodic boundary. A cell index wraps by `rem_euclid`. A coordinate wraps into `[0, L)`, and a coordinate
  within rounding below zero maps to `0.0`, never to `L`. The minimum image lies in `[-L/2, L/2)`, and its
  correction is exact (Sterbenz).
- INV-01: every energy is finite and nonnegative (`-0.0` counts as zero), and every moment is finite.
  INV-02: `max(0, ||m||₂ − e) ≤ absolute + relative · e`, checked in F64 over the exact F32 values.
- Initial field. `energy_per_cell` is rounded once to F32 and the composition is `zero`. The stored value is
  reported, and `injected_total` is exact for grids of at most 2^29 cells.

**Invariants — phase 2 (FL-004).**
- The same five-point operator is applied to every plane:
  `u' = ((c·u + r_x·(u_w + u_e)) + r_y·(u_s + u_n))` in F64 from exact F32 inputs, rounded once to F32. Its
  coefficients are nonnegative and its rows and columns sum to one. It therefore keeps energy nonnegative,
  conserves the periodic total up to one F32 rounding per cell, and preserves the cone up to rounding.
- Subcycling uses the fewest equal substeps whose computed center coefficient `1 − 2r_x − 2r_y` is
  nonnegative. The test is on the coefficient actually used, not on an estimate. The bound for rectangular
  cells is `r_x + r_y ≤ 1/2`. Diffusion is never clamped, and a tick needing more than `MAX_SUBSTEPS` (1024)
  substeps is refused. One subcycled tick equals the same number of single-substep ticks bit for bit.
- Decay is `f32(f64(u) × exp(−λΔt))` on every plane. The removed energy `Σ (e − e')` is reported and charged to
  dissipation, and `λ = 0` changes nothing.
- A source deposits `q_c = P Δt · k_c / Σ k` with `k_c = exp(−|d_c|²/2σ²)` at cell centers within `cutoff × σ`,
  where `d_c` is the minimum-image displacement. Both sums run in cell-index order, and every cell appears once,
  even when the support wraps around the whole grid. An empty support is refused. The F64 total is `P Δt`
  whatever the resolution or position. Storing into F32 rounds each cell once; the applied energy is reported,
  and it is exact for dyadic fixtures.
- A deposit adds `(q, q a)` with `||a||₂ ≤ 1` checked in F64. `unit_direction` shrinks by one ulp until that
  holds exactly, because Appendix D's cone argument needs `≤ 1`, not `≈ 1`. Under `fixed-per-source`, moving a
  source never changes its direction.
- `PathSegment` integrates `s(u) = vT[a u + (b − a)(u³ − u⁴/2)]` analytically, so the speed is at most `v`.
  The next segment starts at the wrapped endpoint with direction `b`, which keeps position and velocity
  continuous at a join. The mid-tick fraction is `(2k + 1)/(2K)`, computed from integer ticks.
- `exp_neg` uses only IEEE-754 `+ × ÷` in a fixed order with no FMA, so every conforming host computes the
  same bits. The tests pin them, and they agree on macOS arm64 and Linux x86_64. It reduces Cody–Waite style to
  `n ln 2 + r` and is within 1 ulp of the correctly rounded value over `[1e-9, 745]` (measured), and exact at
  the Appendix A decay factor and the kernel cutoff. The implementation lives in `flife-core::numerics`.
- `advance_field` writes only the candidate buffer and reports `ε = (E' − E) − (I − D_decay)`. On error the
  candidate must be discarded; the committed field is untouched.

**Invariants — uptake (FL-005).**
- Consumption is local. The `grid-cic-v1` footprint at `x` uses the continuous index `(x/h_x − ½, y/h_y − ½)`.
  Its floor selects the lower-left cell, and its fractional part gives the four bilinear weights, wrapped
  periodically. A cell named twice on a grid one cell wide is combined into one entry, and zero weights are
  dropped. Every footprint cell lies within one cell spacing of `x`, so a long-range sensor never becomes
  long-range feeding.
- The protocol has two passes over one immutable snapshot. First, each packet `q_ic = d_i w_ic` is formed, and
  the packets are sorted by (cell, organism identity). Second, each cell sums `A_c` in that order and grants
  with `α_c = min(1, e_c/A_c)`.
- Reconciliation follows `cell-quantum-largest-remainder-v1`.
  - A cell's F32 energy is `n_c` quanta of `Q_c`, its F32 unit in the last place. Grants are whole quanta.
  - A contended cell grants all `n_c` quanta. An uncontended cell grants `round(A_c/Q_c)` quanta, ties to
    even.
  - Each packet first gets the floor of `α_c q_ic/Q_c` quanta. The leftover quanta go to the largest
    remainders, with ties going to the lower identity.
- Consequences of the reconciliation:
  - Every grant is nonnegative and within one quantum of its ideal value.
  - A cell's grants sum to exactly its debit, and the debit never exceeds its energy (INV-04).
  - The new cell energy `(n_c − K_c) Q_c` is exact in F32, so the field loses exactly what is granted. Uptake
    adds no residual to the ledger.
- An uncontended request below half a quantum is granted nothing. Granting it would create energy the F32 cell
  cannot give up.
- Each organism's `U_i` is the round-down sum of its at most four exact packets. It therefore never exceeds what
  its cells lost.
- Grants depend on identities only. Permuting the demands permutes the totals and leaves every value and the
  debited field identical bit for bit.
- The debit scales the moment by `e'_c/e_c` in F64, rounded once to F32, and writes exactly zero when the cell
  is drained. This removes composition in proportion and preserves the cone up to rounding.
- Grants record each cell's pre-debit energy. Applying them twice, to another grid, or to a field whose
  demanded cell has changed is refused before anything is written.

**Invariants — sensing pyramid (FL-007).**
- Level 0 is the field's density, `e/(h_x h_y)` and `m/(h_x h_y)`, read in F64. Level `l + 1` is the fixed-order
  2×2 average `¼((a + b) + (c + d))` of level `l`, for every plane. Densities are averaged, not energies, so a
  uniform density stays uniform at every level. Every level stays anchored to the torus origin.
- Sampling a level is bilinear in the continuous index `(x/h − ½, y/h − ½)` at that level's spacing, wrapped
  periodically. At a knot (a stored cell center) the floor-selected interval to the right is used. The derivatives
  are those of the same interpolant, divided by the level's physical spacing.
- The interpolant and the blend are written in lerp form, `a + t(b − a)`. Equal corner values therefore give that
  value, and a zero derivative, exactly, at every level and every blend.
- `log-linear-v1`: a scale `s` between `s_l` and `s_(l+1)` blends the two levels' raw values and derivatives with
  `t = log₂(s/s_0) − l`. A scale that is an exact power of two times `s_0` uses its own level alone.
- The pyramid holds raw densities only. Normalization happens after blending, in `flife-neural`.

**Invariants — seeded sources (FL-010).**
- Chemical direction of source `j`: 32 Box–Muller normals, pair `p` from words 0 and 1 of `(srcchems, j, 0,
  lane (0, direction, p), 0)` giving components `2p` and `2p + 1`, normalized by `unit_direction` to
  `||a|| ≤ 1` exactly. It is fixed for the run (`fixed-per-source`).
- Initial center: `(W u₀, H u₁)` from the half-open uniforms of words 0 and 1 of `(srcplace, j, 0, lane (0,
  position, 0), 0)`, wrapped onto the torus.
- Travel direction `d_k`: `(cos 2πu, sin 2πu)` from the half-open uniform of word 0 of `(srcpaths, j, k, lane (0,
  direction, 0), 0)`, uniform by angle through the host-independent `sin_cos_turns`.
- Segment `k` of `SeededPath` blends `d_k` into `d_(k+1)`. Segment 0 starts at the initial center, and each later
  segment at its predecessor's wrapped endpoint, so position and velocity are continuous at every join.
- Every value depends only on the seed and the source's identity (and segment ordinal), never on the order
  sources are made or advanced.

**Invariants — emission deposits (FL-011).**
- An offer spreads over the emitter's `grid-cic-v1` footprint at its post-motion position, one packet per cell.
  Every cell but the largest-weight one (lowest index on a tie) gets `ρ w w_c` rounded to nearest, and that cell
  gets the remainder rounded down. So the packets never sum to more than the offer, exactly, and none is negative.
- Energy and every moment component share each cell's packet: `(q_c, q_c a)`.
- Packets are sorted by (cell, organism identity) and each cell sums them in that order, rounding down. The new
  cell energy is the largest F32 value at or below `e_c + S_c`, so a deposit never creates energy. What the F32
  cell cannot hold is dissipated, like an uptake request below half a quantum.
- The moment gains `(δ_c/S_c) Σ q a`, where `δ_c` is what the cell received, rounded once per component. The
  composition shrinks with the energy it accompanies, which keeps the cone (INV-02).
- One reduction is one ledger transfer: debit `Σ w`, credit `min(Σ δ_c, Σ w)`, the rest to `emission-loss`.
- The result depends only on organism identities, never on push order or storage slot.
- Deposits are applied after the tick's uptake debits, and `apply_grants` refuses a field that has changed
  since its snapshot, so the order cannot be inverted. Nothing writes the observation snapshot, so a deposit is
  first sensed and absorbable on the next tick.

**Refusal cases.**
- Representation: a zero cell count, or a nonfinite or nonpositive world or cell side; byte counts past
  `u64::MAX` or `isize::MAX`; a buffer above its limit; any layout mismatch, including one of equal length; a
  slice whose length disagrees with its layout; each inconsistent declaration; non-F32 storage.
- Unimplemented names: a boundary, dtype, channel count, composition, transport scheme, stability policy,
  kernel, normalization or chemistry policy this crate does not implement.
- Values: an initial energy beyond F32; a negative or nonfinite diffusion, decay or power; more than 1024
  substeps; a nonpositive kernel width or cutoff; an empty support; a direction with `||a|| > 1`; a non-unit
  path direction, an unwrapped anchor or a zero-tick segment.
- Uptake:
  - A negative or nonfinite demand, or a nonfinite position.
  - An organism that demands twice.
  - A nonfinite or negative cell energy.
  - A request law with negative capacity, nonpositive half-saturation or negative density.
  - Grants applied twice, to another grid, or to a changed field.
- Sensing pyramid:
  - 32 or more levels, a grid not divisible by `2^levels`, or a level above the buffer limit.
  - A field on another grid.
  - A scale outside `[s_0, s_0 2^L]`, a nonfinite position or a nonfinite density.
  - Another filter or blend name.
- Deposits (FL-011): a negative or nonfinite payment or offer; an offer above its payment; a direction outside
  the unit ball; a nonfinite position; staging on another grid, or after the deposits were applied (until
  `clear`); applying twice, to another grid, with one organism staged twice, or to a field missing a moment
  plane. Nothing is written on a refusal.
- Invariants: the first cell that violates INV-01 or INV-02.

**Determinism.** Pure functions of their inputs with fixed iteration orders. No hash maps, no parallel
reduction, and no platform `libm` in model arithmetic. Random words come only from semantic blocks in `seeding`.

**Validity domain.**
- Implemented here: `field-ecology-v1` with `periodic`, `f32`, `zero`, `positive-five-point-v1`, `subcycle`,
  `fixed-total-power`, `normalized-truncated-minimum-image-gaussian-v1`, `fixed-per-source`, `grid-cic-v1`,
  `positive-box-v1`, `log-linear-v1`, `seeded-unit-vectors` and `seeded-smooth-torus-v1` (the last two with
  FL-010's draws).
- Host buffers in layout schema `frankenlife.field-layout.v1`.

**Independent tests.**
- `tests/field_layout.rs`: NumPy strides, a `shasum` digest, and the §19.3 byte counts measured on the real
  allocation.
- `tests/transport_sources.rs`:
  - C.6, both exact (dyadic `r`) and within half an ulp of the rational 1/5.
  - A separately written `rem_euclid` oracle on a random rectangular field.
  - Subcycling minimality across a sweep of diffusion values.
  - Decay accounting that is exact.
  - C.17 exact totals for 1, 2 and 4 support cells (the last across the periodic corner), and resolution and
    position independence at 64², 128² and 256².
  - Path continuity and the speed bound.
  - `exp_neg` against `std` and against correctly rounded references.
  - One simulated second (128 ticks) of the Appendix A field with 8 sources. The ledger closes, with a
    cumulative residual of 1.7e-5, which is 2.7e-8 of the transfers; §21.4 proposes 1e-5.
- `tests/uptake_ledger.rs`. Grants are whole quanta, so the oracles are integers.
  - C.1 exactly: grants 4 and 6, the moment removed, 6.5 dissipated, `ε = 0`.
  - The uncontended remainder and its moment.
  - The two leftover quanta of `2^23/3` going to the two lowest identities under all six input orders.
  - Rounding once per cell after aggregation, including ties to even.
  - A million requests on one cell (§21.5).
  - A 64² field with 100,000 organisms, half in a hotspot, with conservation checked in every cell.
  - Footprint weights, wrapping, combination and locality.
  - A.1's request law.
  - Stale and duplicate application refused.
  - A fault-injection lane taken from rust_scriptbots. Credit without debit, debit without credit, double
    credit, an unrecorded loss, a gain above the grant, overspend repaired by clamping, double apply, a stale
    field, a duplicate demand, recording requested rather than paid maintenance, and an unrecorded change
    between ticks: each must fail through the check meant to catch it.
  - One simulated second of the Appendix A world with 4,096 static organisms taking up energy and paying
    maintenance, committed through the ledger every tick. The cumulative residual is 1.9e-5, which is 2.1e-9 of
    initial energy plus turnover (§21.4 proposes 1e-5). The largest single-tick residual is 7.8e-7. The drift is
    +1.3e-7 energy per tick and comes from phase 2's F32 rounding; uptake contributes none.
- `tests/seeding.rs` (FL-010). Expected values come from an oracle using NumPy 2.5.4's `Philox` and Python
  `Decimal`.
  - Source 5's 32-channel direction within 8 ulp of correctly rounded.
  - A travel direction within 2 ulp.
  - An initial center exact.
  - The same sources in forward, reverse and interleaved order, and another seed moving them.
  - Isotropy, uniform centers and uniform angles over 4,096 sources.
  - Twelve seeded path segments that start at the seeded center, run along their own `d_k` and `d_(k+1)`, stay
    within the speed limit and join continuously.
  - A path unaffected by another source's advances.
  - A moved source keeping its chemistry.
- The pyramid's tests live in `flife-neural`'s `tests/sensing.rs`, next to the sensor encoding they feed: a slow
  direct oracle at every level, C.10's uniform field, knots and edges, translations, and refusals.
- The deposit reduction's tests live in `flife-engine`'s `tests/emission_births.rs` (FL-011), next to the
  emission that stages it. They cover C.4 exactly; shared bilinear weights for energy and moment; push-order
  independence, including an adversarial F32 boundary; ledger closure; no energy created at efficiency 1; and
  every refusal.

**Known gaps.**
- FL-013's tick carries each `SeededPath` and moves a source's center to its mid-tick position every tick.
  FL-016 checkpoints each path's source, ordinal and anchor; the directions are counter-addressed draws, so
  `SeededPath::resume` re-derives them. Resuming a session from a file is FL-037.
- Cone tolerance. The values belong to an undeclared numerical profile. One finding: far from sources,
  diffusion leaves subnormal F32 energies (about 1e-40) with no relative precision. A cone tolerance therefore
  needs an absolute floor at F32's subnormal scale, and the 128-tick test uses `f32::MIN_POSITIVE`. Metal
  commonly flushes subnormals to zero, so CPU–Metal parity (FL-031) must declare its subnormal policy.
- Phase 2's residual drifts slowly upward (+1.3e-7 per tick at Appendix A scale). That is far inside budget, but
  the endurance gate (G8) should track the slope.
- The reference stencil computes in F64. A production F32 kernel is a separate numerical profile with its own
  tolerances (§21.4).
- `cell-quantum-largest-remainder-v1` is this crate's reconciliation. The configuration schema does not name a
  reconciliation, so a different one would be a different numerical profile.
- `UptakeSummary::contended_cells` counts every cell whose demand exceeds its energy, including empty cells
  that receive demand.
- Per-packet chemistry, toxicity and affordable motion are in `flife-engine` (FL-006). Clearing tiny reservoirs
  and death are FL-012. Recycling a dead organism's energy reuses `Deposits` as a separate reduction booked under
  `Transfer::Recycling` (FL-012, `flife_engine::Recycling`). Device arrangements, device alignment and sharding belong to the Metal backend.
  Whole-run memory admission is FL-043.
- The pyramid is rebuilt in full every tick in F64 (two thirds of the F32 base field's bytes). An F32 or Metal
  pyramid is a separate numerical profile. Its contract, with the sensor encoding, is
  `crates/flife-neural/CONTRACT.md`.
