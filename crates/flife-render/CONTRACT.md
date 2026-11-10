# flife-render contract

**Purpose.** FL-017 (`flife-0cg`) covers immutable compact render epochs (plan §9.5, §12.4, §13.2–§13.5,
Appendix B.3, C.7, C.15, INV-09, INV-10). FL-020 (`flife-err`, part) adds interpolation between two epochs (§13.5,
C.15).

- An `Epoch` is a compact projection of exactly one committed world, with every row in stable identity order.
- A `Publisher` owned by the world's owner fills preallocated slots and publishes the latest one.
- Consumers read epochs through typed per-viewer `Lease`s.
- A leased or displayable epoch is never written. When every slot is busy, the boundary is skipped and the last
  displayable epoch stays.

A slow consumer can therefore cost display freshness, never the world.

**Inputs.**
- `&World`, read only: the committed tick and resource epoch, the population rows and genome topologies, the
  field's energy plane and grid, the timestep, the speed limit (`World::maximum_speed`, added by FL-017) and the
  event cursor.
- The slot count: `execution.render_queue_capacity`, at least 2. The validator refuses 1 (`IntBound::AtLeastTwo`),
  because one displayable epoch and one being written need two slots.
- A release hook, run after every lease drops, so an owner can retry a pending boundary.
- Per viewer, a lease limit in `1..capacity`. `DISPLAY_PAIR` = 2 is the current epoch and its prior, for
  `committed-states-one-epoch-late`.

**Outputs (B.3).**
- `Epoch`, which states:
  - its source: identity (`RenderEpoch`, issued once and strictly increasing), source tick and resource epoch;
  - its `Prior`: the epoch it replaced as latest;
  - the world's timestep, speed limit and extent.
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
  | field | f32 × 1 per texel | finite, ≥ 0 |

  These are the inputs of §13.4's mappings. Turning them into sizes and colours is FL-019's.
- `FieldTexture`: channel `EnergyDensity`, the source grid, the block of cells per texel, the texel count and the
  texel size in world units. Each texel is the mean energy density of its block, summed in F64, rows then columns.
  Texels per axis are at most `FIELD_TEXELS_MAX` = 512. A grid that does not divide evenly ends in a partial block.
- `Metrics`: source tick, live count, organism energy (F64 in identity order), field energy (F64 in cell order),
  highest generation and event cursor.
- `Publication`:
  - `Published { epoch, replaced }`;
  - `Current`: the latest epoch already shows this state;
  - `Busy { tick, kept }`: every slot is busy, the boundary is pending, and `kept` stays displayable.
- `RenderStats`:
  - epochs published;
  - busy boundaries, each counted once however often it was retried;
  - the pending tick;
  - the first projection failure.
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
  (41 bytes a row) and the texture size (4 bytes a texel). No projection reallocates. `Publisher::reserved_bytes`
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
- `tests/epochs.rs` (7 tests):
  - Every view, metric and texel, at founding and over 288 ticks with births and deaths, matches the world's own
    rows and an independent block-mean oracle. Every declared layout holds.
  - A 1,030 × 515 plane gives a 344 × 258 texture of 3 × 2 blocks with partial edges, equal to the oracle. Bad
    planes are refused.
  - C.7: Compact and FreeList worlds publish identical epochs and identical matches at every tick of 288, while
    their slots differ.
  - Over 1,024 FreeList ticks, matching agrees with every tick's births and deaths, and every reused slot's newborn
    is a birth while its dead occupant is gone.
  - Slots reserve exactly 72 × 41 + 32 × 32 × 4 bytes, and never more, through a run to the population ceiling.
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
- In `flife-runtime`, `tests/observation.rs` (3 tests) checks that the session stays state-neutral under pressure.
- Twenty-five planted bugs in this crate, the session's feed, the validator and `World::maximum_speed` were each
  caught by at least one of these tests or `flife-runtime`'s (listed in the FL-017 commit).

**Validity domain.** CPU host memory; one publisher per world; any number of consumers on any threads; the
`field-ecology-v1` energy plane.

**Known gaps.**
- No device residency. Leases are host `Arc`s. Typed Metal buffers and safe leases are FL-028's. The shared-device
  resource bridge (§13.6: device context, layout, completion fence, read lease, release notification) is FL-033's;
  FL-018's adapter (`flife-manim`) copies each epoch into its draw buffer on the CPU and returns the lease before
  rendering. Device reads that must finish before release do not exist yet.
- The field texture shows energy only. Moment channels, fixed projections and an admitted full-field diagnostic
  lease are FL-019's raw views.
- Epochs carry no birth ticks, so a birth appears with the first epoch that shows it rather than at its exact
  birth tick within a skipped gap. Recorded winding or displacement (the alternative to snapping) does not exist.
- Trails, and drawing interpolated epochs in the Studio, are FL-020's remaining parts.
- No every-state export path. Recording that needs every sampled state uses its own bounded backpressure policy
  (FL-023).
- The pause arm of `drop-observer-or-pause-unchanged-world` is for observers whose retention competes with the
  world's resources: FL-021's inspector snapshots and FL-038's checkpoint pages.
