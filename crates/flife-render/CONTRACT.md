# flife-render contract

**Purpose.** FL-017 (`flife-0cg`) covers immutable compact render epochs (plan §9.5, §12.4, §13.2–§13.5,
Appendix B.3, C.7, C.15, INV-09, INV-10). FL-020 (`flife-err`, part) adds interpolation between two epochs (§13.5,
C.15). FL-024 (`flife-xut`, part) adds what plan §14.1's persistent telemetry reads to each epoch's metrics: the
world's own quantities at its tick, and a tally of the committed ticks the publisher was given.

- An `Epoch` is a compact projection of exactly one committed world, with every row in stable identity order.
- A `Publisher` owned by the world's owner fills preallocated slots and publishes the latest one.
- Consumers read epochs through typed per-viewer `Lease`s.
- A leased or displayable epoch is never written. When every slot is busy, the boundary is skipped and the last
  displayable epoch stays.

A slow consumer can therefore cost display freshness, never the world.

**Inputs.**
- `&World`, read only: the committed tick and resource epoch, the population rows, each genome's topology and
  affinity vector and the world's `affinity_norm_limit` (FL-019), the field's 33 planes (energy and the 32 moment
  channels) and grid, the timestep, the speed limit (`World::maximum_speed`, added by FL-017) and the
  event cursor. FL-024: each genome's parameter count, the genome quota's logical and allocated bytes, the run
  ledger's report and whether a numerical fault stopped the run.
- FL-024: `Publisher::count(&TickReport)`. The owner gives every report it publishes, in order, before offering the
  world.
- The slot count: `execution.render_queue_capacity`, at least 2. The validator refuses 1 (`IntBound::AtLeastTwo`),
  because one displayable epoch and one being written need two slots.
- A release hook, run after every lease drops, so an owner can retry a pending boundary.
- Per viewer, a lease limit in `1..capacity`. `DISPLAY_PAIR` = 2 is the current epoch and its prior, for
  `committed-states-one-epoch-late`.

**Outputs (B.3).**
- `Epoch`, which states:
  - its source: identity (`RenderEpoch`, issued once and strictly increasing), source tick and resource epoch;
  - its `Prior`: the epoch it replaced as latest;
  - the world's timestep, speed limit and extent, and the affinity norm limit its affinity view is scaled by.
- Its views, each with a declared `Layout` (`View`, `Dtype`, components, count, `Bounds`), in identity order:

  | View | Dtype × components | Bounds |
  |---|---|---|
  | identity | u64 × 1 | strictly increasing |
  | position | f32 × 2 | each component in `[0, extent)` after rounding |
  | velocity | f32 × 2 | each component within the F32 speed limit |
  | energy | f32 × 1 | finite, ≥ 0 |
  | lineage | u64 × 1 | — |
  | generation | u32 × 1 | — |
  | topology | u8 × 1 | — |
  | affinity | f32 × 3 | each component in `[−1, 1]` |
  | field | f32 × 1 per texel | finite, ≥ 0 |
  | field-composition | f32 × 3 per texel | each component in `[−1, 1]` |

  These are the inputs of §13.4's mappings. Turning them into sizes and colours is FL-019's.
- The affinity view (FL-019, `affinity_projection`) is a fixed projection of each organism's 32 affinity terms
  `a`: `q_k = (Σ_j H[r_k][j] a_j) / (√32 · L)` for the rows `r = AFFINITY_AXES = [7, 14, 25]` of the 32 × 32
  Sylvester Hadamard matrix `H[r][j] = (−1)^popcount(r & j)`, with `L` the world's `affinity_norm_limit`. The rows
  are orthonormal after the `√32`, so `q` is the affinity vector's orthogonal projection onto three fixed axes, in
  units of the limit, and its length is at most 1. Each component is summed in F64 in channel order, clamped to
  `[−1, 1]` and rounded to F32; a non-finite one, or a limit that is not finite and positive, gives 0. No row of
  the matrix is fitted to a population, so a vector keeps its projection in every world with the same limit.
- `FieldTexture`: channel `EnergyDensity`, the source grid, the block of cells per texel, the texel count and the
  texel size in world units. Each texel is the mean energy density of its block, summed in F64, rows then columns.
  Texels per axis are at most `FIELD_TEXELS_MAX` = 512. A grid that does not divide evenly ends in a partial block.
- The field's composition (FL-019, plan §13.4's "Local field", `FieldTexture::composition`): per texel, the block's
  moments summed channel by channel, `Σm`, and its energy, `Σe`, in F64, rows then columns, give
  `affinity_projection(Σm, Σe)`, that is `q_k = (Σ_j H[r_k][j] Σm_j) / (√32 · Σe)` on the affinity view's axes.
  It is the block's mean composition per unit energy, weighted by energy, in the coordinates an affinity direction
  at its norm limit takes, so an organism and the material it converts best (`a / ‖a‖`) share them. Every cell
  holds `‖m‖₂ ≤ e` (§4.2), so the composition lies in the unit ball. A block with no energy has none, `[0; 3]`, as
  does every texel of `FieldTexture::from_plane` (an energy plane alone); `from_planes` takes the 33 planes.
  The projection runs a texel row at a time through each moment plane with the axes as a sign table, which gives
  the same sums in the same order, bit for bit, as `affinity_projection` of the block's channel sums. Measured
  (scratch, release, load average about 44, cv up to 20%): on the preset's 128 × 128 field the texture from 33
  planes took 0.29 ms against 0.05 ms from energy alone, and a whole epoch 0.57 ms against a 39.5 ms tick; a
  512 × 512 field 6.3 ms, a 1024 × 1024 field (2 × 2 blocks) 26 ms.
- `Metrics`: source tick, live count, organism energy (F64 in identity order), field energy (F64 in cell order),
  highest generation and event cursor. FL-024 adds:
  - `parameters`: the live organisms' logical neural parameters, summed;
  - `genome_logical_bytes` and `genome_allocated_bytes`: the genome quota's;
  - `ledger`: the run ledger's `LedgerReport` (cumulative injection, removal, dissipation, turnover and residual);
  - `faulted`: whether a numerical fault stopped the run;
  - `tally: Option<Tally>`: from the tick the publisher opened at (`since`), the births, introductions and deaths,
    controller evaluations (regular and forced), birth attempts whose every candidate was rejected, and the energy
    organisms gained by uptake (the uptake transfer's credit), each summed in tick order. An epoch carries it only
    when the publisher counted every tick from `since` through the epoch's own; `Epoch::standalone` carries none.
- `Publication`:
  - `Published { epoch, replaced }`;
  - `Current`: the latest epoch already shows this state;
  - `Busy { tick, kept }`: every slot is busy, the boundary is pending, and `kept` stays displayable.
- `RenderStats`:
  - epochs published;
  - busy boundaries, each counted once however often it was retried;
  - the pending tick;
  - the first projection failure.
- FL-043: `Epoch::standalone(world)`, the world's committed state as one slot outside any feed (identity 1, no
  prior, never published), so a host can size its observers before its feed exists. The Studio sizes its feed by
  it: `render_queue_capacity` slots of its `reserved_bytes()`.
- `pairing`:
  - `pairs(prior, current)`: a merge by identity, yielding `Continuing`, `Born` or `Died` rows;
  - `Gap::between`: the tick gap, the travel bound `ticks · Δt · maximum speed`, and whether twice that bound, with
    `ROUNDING_MARGIN` (2⁻²⁰), is under each extent;
  - `wrapped_between(from, to, length, fraction)`: `from` moved `fraction` of the minimum wrapped displacement
    toward `to`, wrapped into `[0, length)`; exactly `from` at 0 and `to` at 1;
  - `interpolate(prior, current, fraction, into)` (FL-020) fills an `Interpolation` per row of `current`: a
    continuing organism at `wrapped_between` on each axis when the gap is unambiguous, otherwise snapped to its
    `current` position with `snapped` set (trails reset); a birth at its `current` position, shown only at
    fraction 1 (never before its birth); and `fading`, the `prior` rows of the dead. `into` is reused.

**Invariants.**
- *One committed world.* A projection reads only committed state, through `&World`. It writes the slot alone, and
  only while the publisher holds the slot's sole reference (`Arc::get_mut`).
- *Immutability (INV-10).* A lease and the feed's latest-epoch cell each hold a shared reference. A leased or
  displayable slot therefore cannot be written; the type system refuses it. A consumer only sees an epoch after it
  is complete.
- *Bounded memory (§9.5).* Every slot is allocated once, with fallible reservation, at the population ceiling
  (53 bytes a row) and the texture size (16 bytes a texel: 4 of energy, 12 of composition). No projection
  reallocates. `Publisher::reserved_bytes`
  is the render budget.
- *Busy feeds skip.* When no slot is free, nothing is written or allocated. The latest epoch stays, and the boundary
  stays pending. The next offer, or one prompted by a released lease, publishes the newest committed state, linked
  to the epoch it replaced. This is the drop arm of `drop-observer-or-pause-unchanged-world` (§7.5).
- *Bounded leases.* A viewer holds at most its limit, and a limit is below the slot count, so no single consumer
  holds every slot. Peeking with `latest_after` takes nothing from the budget. A lease outlives its viewer. Dropping
  it releases the slot reference, then the budget, then runs the release hook.
- *Stable identity (C.7, §13.5).* Rows follow identity, never storage slot. Two arrangements of one world publish
  identical epochs, and matching never links a reused slot's dead occupant to its newborn.
- *Observation neutrality (INV-09).* Nothing reaches the world mutably. A projection failure is recorded and
  returned and leaves the last epoch displayable.
- *A whole tally or none (FL-024).* `count` adds a report only when it follows the last tick counted
  (`report.from` is that tick). A report that does not, a tick missed or given twice, ends the count, and no later
  epoch carries a tally. A busy boundary is counted though not published, so the next epoch published carries
  every tick between.

**Refusal cases.**
- Fewer than two slots: `InvalidConfig`, at `execution.render_queue_capacity`.
- A lease limit outside `1..capacity`: `NumericalInvalid`. A lease beyond a viewer's limit: `BudgetExhausted`.
- An exhausted identity sequence: `Overflow`, recorded, with the last epoch kept.
- A slot that cannot be allocated: `AllocationFailed`.
- A world of another size than its slots: `LayoutMismatch`. More rows than the ceiling: `BudgetExhausted`.
- `Gap::between` between epochs of different worlds, or not in order: `StateConflict`; `interpolate` likewise, and
  `NumericalInvalid` for a fraction outside `[0, 1]`.
- `FieldTexture::from_plane` with an empty grid or a plane of the wrong length: `LayoutMismatch`.

**Determinism.** A projection is a pure function of the committed world. Its F32 values are round-to-nearest
conversions of the F64 state, and its sums are F64 in a fixed order. Slot choice is the lowest free one, and
identities are sequential, so a deterministic sequence of offers and lease drops gives the same publications.

**Independent tests.**
- `tests/epochs.rs` (9 tests):
  - Every view, metric and texel, at founding and over 288 ticks with births and deaths, matches the world's own
    rows and an independent block-mean oracle. Every declared layout holds. Each texel's composition equals, bit
    for bit, an oracle that sums the world's moment planes per block and projects them with the Hadamard matrix
    built by its recursion; the run's sources and emissions give texels a composition.
  - The composition (FL-019), by hand: a composition along an axis lands on it, its opposite on the opposite end;
    cancelled moments and a cell without energy have none; one channel lands at `±1/√32` by its column's signs. In
    blocks of two, three units along an axis and one against it land half way: weighted by energy. A 521 × 515
    grid in 2 × 2 blocks with partial edges equals the oracle and stays in the unit ball. An energy plane alone has
    no composition; planes of the wrong length are refused.
  - A 1,030 × 515 plane gives a 344 × 258 texture of 3 × 2 blocks with partial edges, equal to the oracle. Bad
    planes are refused.
  - C.7: Compact and FreeList worlds publish identical epochs and identical matches at every tick of 288, while
    their slots differ.
  - Over 1,024 FreeList ticks, matching agrees with every tick's births and deaths, and every reused slot's newborn
    is a birth while its dead occupant is gone.
  - Slots reserve exactly 72 × 53 + 32 × 32 × 16 bytes, and never more, through a run to the population ceiling.
  - The affinity view (FL-019): the axes are orthonormal rows of a Hadamard matrix built by its recursion, not the
    crate's popcount form; a vector along a row projects onto its axis, and three times past the limit onto its
    clamped end; 200 pseudo-random vectors project bit for
    bit as that oracle does, inside the unit ball; a zero, NaN or negative limit and an infinite term give the
    origin. A world at `affinity_norm_limit = 0.5` publishes its own limit and projects its founders as spread as at
    1, not halved, and a publisher made for a limit-1 world states and scales by the 0.5 world it is offered. Every view check above includes the affinity view against the oracle.
  - C.15: a one-tick gap is unambiguous. A 600-tick skipped gap on the 32-unit world (18.75 units of reach) is not.
    Epochs out of order or from another world are refused.
  - Selection by identity.
- `tests/leases.rs` (5 tests):
  - Leased epochs stay identical while 20 boundaries are busy; the feed keeps its last epoch and reports the
    pending tick. Releases run the hook. The next offer publishes the newest state as epoch 4, linked to the kept
    one.
  - A dropped lease returns its reference and budget before the release hook runs, so a woken owner finds the
    slot free.
  - Per-viewer limits, independent budgets, free peeking, and leases outliving viewers.
  - Refusals: one slot, an exhausted sequence before and after founding. The world digest is unchanged and the
    last epoch is kept.
  - Two reader threads check every epoch they lease while the owner publishes 200 boundaries. Every offer is
    published or busy, and the budgets return to zero.
- `tests/interpolation.rs` (4 tests, FL-020). The oracle for a gap's true displacement is each organism's
  tick-by-tick path in the world (per-tick minimum-image steps summed), never the endpoints' minimum image.
  - Over unambiguous 40-tick gaps of a fast-moving ecology (ten times the acceleration limit, almost no drag: the
    plain ecology's founders move a few hundredths of a unit per gap), every continuing organism is at its prior
    position at fraction 0 and its current one at 1, exactly, and within 1e-4 of its true path at 0.25, 0.5 and
    0.875; at least three seam crossings are drawn within the gap's travel of both ends; every position is in the
    world.
  - C.15: `wrapped_between` draws a forward move of 80 units on a width-128 axis 48 units in reverse (worked
    numbers), and the true path across the seam under half the world. Over a 600-tick gap every continuing
    organism snaps to its current position, exactly, with `snapped` set.
  - Every founder removed and five organisms introduced in one tick, one taking a removed organism's storage slot:
    the 48 dead fade from their prior rows, and the five new identities are drawn only at fraction 1, at their own
    positions.
  - Refusals: fractions outside `[0, 1]`, and epochs out of order.
- `tests/telemetry.rs` (3 tests, FL-024):
  - Over 160 ticks of the ecology, with five organisms introduced at tick 40 and a disk removed at tick 90, while a
    second viewer holds two epochs for six ticks in every sixteen (in one run: 130 published, 30 skipped), every
    epoch published, the founding one included, carries the world's parameter sum, quota bytes, ledger report and
    fault flag, and a tally equal to an independent sum over the reports through its tick (uptake bit for bit). The
    counts are nonzero (in one run: 37 births, 5 introductions, 10 deaths, 11,112 evaluations). The world's own
    counters agree: its event cursor equals deaths + births + introductions, and its next organism identity equals
    the live count plus deaths. The uptake is what organisms gained: within 1e-9 of the grants less uptake's
    dissipated inefficiency, and below the grants.
  - A publisher opened three ticks in counts from tick 3. A tick offered before it is counted carries no tally, and
    counted afterwards the tally goes on whole into the next epoch. A tick not counted leaves this epoch and every
    later one without a tally; so does a tick counted twice. `Epoch::standalone` has the world's quantities and no
    tally.
  - A world with one source at power 1e40 faults within 200 ticks; its epoch before says no fault and its epoch
    after says a fault stopped the run, with every other quantity the world's.
- In `flife-runtime`, `tests/observation.rs` (3 tests) checks that the session stays state-neutral under pressure,
  and `tests/telemetry.rs` (FL-024) that a session's epochs carry the tally through their ticks.
- Twenty-five planted bugs in this crate, the session's feed, the validator and `World::maximum_speed` were each
  caught by at least one of these tests or `flife-runtime`'s (listed in the FL-017 commit).
- FL-024's telemetry, nine planted bugs here, run against `tests/telemetry.rs` alone: births counted from deaths,
  evaluations not added, uptake taken as the grant (`debited`), a count that ignores the report's `from`, an offer
  that attaches the tally to any tick, parameters not summed, a slot's stale ledger and a fault never shown were
  caught (the last two after the offer-before-count and faulted-world cases were added; the first run's survivors).
  Allocated genome bytes taken as the logical survives, and is equivalent today: with no deduplicating store the
  quota allocates exactly `P x 4` bytes a genome, so the two are equal in every world (15,341,568 both in the
  Studio's crowded run).

**Validity domain.** CPU host memory; one publisher per world; any number of consumers on any threads; the
`field-ecology-v1` energy plane.

**Known gaps.**
- No device residency. Leases are host `Arc`s. Typed Metal buffers and safe leases are FL-028's. The shared-device
  resource bridge (§13.6: device context, layout, completion fence, read lease, release notification) is FL-033's;
  FL-018's adapter (`flife-manim`) copies each epoch into its draw buffer on the CPU and returns the lease before
  rendering. Device reads that must finish before release do not exist yet.
- The field texture carries energy and the composition's fixed projection. Single moment channels ("chosen
  components") and an admitted full-field diagnostic lease do not exist. The composition is projected with every
  epoch, whether or not a consumer draws it.
- Epochs carry no birth ticks, so a birth appears with the first epoch that shows it rather than at its exact
  birth tick within a skipped gap. Recorded winding or displacement (the alternative to snapping) does not exist.
- Trails, and drawing interpolated epochs in the Studio, are FL-020's remaining parts.
- No every-state export path. Recording that needs every sampled state uses its own bounded backpressure policy
  (FL-023).
- The pause arm of `drop-observer-or-pause-unchanged-world` is for observers whose retention competes with the
  world's resources: FL-021's inspector snapshots and FL-038's checkpoint pages.
