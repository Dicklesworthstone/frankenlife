# flife-core — contract

**Purpose.** FL-002 (`flife-jb0`): the shared vocabulary every FrankenLife crate builds on (plan §18.1, §18.3). That
covers identities, integer clocks, units, failure classes, the named-algorithm registry, the
`frankenlife.config.v1` configuration with its A.2 validator, canonical serialization, and the model descriptor.
FL-005 (`flife-f6x`) adds the global energy ledger (§4.4, §8.2, §21.4, INV-03 and INV-05). FL-006 (`flife-h6c`)
adds the host-independent elementary functions of the reference numerical profile (A.1, C.11). FL-010
(`flife-rf8`) adds `fnp-philox-semantic-v1`, the semantically addressed random source every model draw comes from
(§16.2, A.3 item 3, INV-11), with the `exp`, `ln` and turn-angle sine and cosine its distributions need.
FL-011 (`flife-t24`) adds the `birthing` stream and the `attempt` lane kind for reproduction (A.3 item 10), and
`EnergyBudget::retain` for the share a parent keeps (§7.4). FL-016 (`flife-gh0`) adds Appendix B.2's checkpoint
container: a sealed header and manifest over typed, versioned, hashed chunks, and the bounded little-endian
codec the chunks are written in (§16.3, INV-14).

**Inputs.** Configuration TOML (Appendix A's schema). Every key is required and unknown keys are refused, so there
are no hidden defaults.

**Outputs.**
- `ValidatedConfig`. It carries a `ValidationReport` with the timestep, cell size, pyramid levels, cooldown,
  source-segment and checkpoint ticks, the default parameter count, and the logical genome bytes at the initial
  population, at the ceiling and at the parameter cap. It also produces the canonical TOML and the
  configuration hash (SHA-256).
- `ModelDescriptor`. Canonical tab-separated text plus its SHA-256. It holds the schema and unit of every model
  and numerics parameter, and the named component for each role. It excludes values, run identity, execution
  resources and presentation.
- Identities (`OrganismId`, `LineageId`, `GenomeId`, `CommandId`, `SessionId`, `RenderEpoch`, `ResourceEpoch`,
  `SourceId`, `Generation`) and `IdSequence`.
- `Tick`, `TickSpan`, `TimeStep`, `age_seconds` and `stable_phase`.
- `CoreError` and the `REGISTRY`.
- The ledger (FL-005):
  - `Flow`: 15 named kinds, each in one `FlowClass`.
  - `Transfer`: uptake, emission, child endowment and recycling, each with a named loss flow.
  - `Holdings`, and `TickLedger` with its `TickClosure`.
  - The run's `EnergyLedger` with its `LedgerReport`, and `ResidualBudget`.
  - The per-organism `EnergyBudget`, with `Payment`, `pay` and (FL-011) `retain`.
  - `CompensatedSum`, and the round-down helpers `add_down` and `sub_down`.
- Numerics (FL-006, FL-007, FL-010): `exp_neg`, `expm1_neg`, `tanh`, `sigmoid`, `log2`, `ln`, `exp` and
  `sin_cos_turns`.
- Random addressing (FL-010): `SemanticRng`, `Address`, `Stream`, `Lane`, `LaneKind`, `Block`, and the draw
  conventions `half_open_unit`, `open_unit`, `bernoulli`, `choose` and `box_muller`.
- Checkpoints (FL-016): `checkpoint::{CheckpointWriter, CheckpointReader, Chunk, ChunkFamily, ByteWriter,
  ByteReader}` and the format constants. `LedgerState`, `EnergyLedger::state` and `EnergyLedger::restore` carry
  every bit of a ledger; `CompensatedSum::{from_parts, parts}` and `Sha256Digest::from_bytes` support them.
  FL-035 adds `CheckpointManifest` and `ManifestEntry`, which verify a container's sealed prefix without its
  payload, so `flife-store` can keep the prefix as a manifest and the chunks by digest. `CheckpointReader::parse`
  is that check plus the chunk hashes and the payload's exact coverage.
- Streamed checkpoints (FL-038): `checkpoint::ChunkSink` (`begin`, `page`, `end`), `CHECKPOINT_PAGE_BYTES` (1 MiB),
  `CheckpointWriter::streaming(model, sink)` with `chunk_with` (a chunk written through a `ChunkWriter`), `seal`
  (the sealed prefix) and `payload_length`, and `canonical::Sha256Stream`. A streaming writer hands each chunk to
  its sink a page at a time and keeps only the manifest; `seal` returns the prefix the in-memory container would
  start with. `finish` (in memory) now reserves the container's exact length once.
- FL-043: `memory::{capacity_bytes, bytes_for, total}`, the one rule every footprint term is counted by: a buffer is
  charged its capacity times its element size, saturating.

**Invariants.**
- An identity is never a storage slot and is never reissued. Exhausting a sequence is `Overflow`, never a wrap.
- Time is an integer tick and a rational `Δt`. Seconds are derived from ticks; no float accumulates time.
  Seconds-to-ticks conversion is exact arithmetic on the f64's binary value. A duration that is not a whole number
  of ticks, but lies within f64 rounding of one, is refused rather than rounded up a tick.
- Each key's unit, bound and registry role is declared once in `schema::fields`. The validator, the canonical
  writer and the descriptor all read it, and the round-trip test catches a key missing from the table.
- Equal configurations serialize to equal bytes, and the canonical TOML parses back to an equal configuration.
  Floats use shortest round-trip form with `-0.0` written as `0.0`.
- Every named algorithm resolves to exactly one registry entry for its key. An unknown name, or a known name under
  the wrong key, is refused.
- Ledger (INV-03).
  - A tick closes with `ε = (𝓔_close − 𝓔_open) − (I − O − D)`, measured from holdings and never inferred from
    flows.
  - `|ε|` must be at most `absolute + per_tick × (𝓔_open + turnover)`.
  - The cumulative `|Σε|` must be at most `absolute + cumulative × (𝓔_initial + Σ turnover)`.
  - A breach is an error in every build profile. Every closure records the budget it was checked against.
  - A transfer debits one holding, credits another, and charges the difference to its named loss. It can never
    credit more than it debits.
  - The run ledger refuses a tick that is out of order, or that does not open exactly where the previous tick
    closed. Energy that changes between ticks without a flow is therefore caught.
  - The report gives `|Σε|` against `𝓔_initial + turnover`, which is the gated measure. It also gives `|Σε|`
    against initial energy alone and against current energy alone, the largest single-tick residual, and the
    drift slope (§21.4).
  - Sums are Neumaier-compensated and run in a fixed order.
- Budgets (INV-05, §8.2).
  - The balance is rounded down after every credit and payment, so in exact arithmetic
    `Σ paid ≤ available + Σ credited`.
  - A payment reports the requested and paid amounts separately.
  - A budget that a payment empties stays exhausted. Later payments get nothing and credits are refused.
- Numerics (A.1).
  - Only IEEE-754 `+ − × ÷`, exact rounding to an integer and exact power-of-two scaling, in a fixed order with
    no fused multiply-add. Every conforming host computes the same bits; tests pin them.
  - `exp_neg` reduces Cody–Waite style to `n ln 2 + r`. `expm1_neg` uses `1 − exp_neg` where `e^(−x) ≤ 1/2` and a
    series with exact halving and the doubling identity below that, so it does not cancel near zero.
  - Measured against correctly rounded values (Python `Decimal`) over 1,296 arguments from `1e-9` to `745`:
    `exp_neg` within 1 ulp; `expm1_neg`, `tanh` and `sigmoid` within 2 ulp.
  - `sigmoid` takes the sign-stable branch: a large negative logit decays to zero instead of dividing by
    infinity.
  - `log2` reads the exponent from the bits (scaling a subnormal by `2^54` first, exactly), reduces the mantissa
    to `[√2/2, √2]` and sums the `atanh` series of `(m − 1)/(m + 1)`. It is exact at every power of two from
    `2^-1074` to `2^1023` and measured within 2 ulp of the correctly rounded value elsewhere.
  - `ln` shares that reduction and adds `n ln 2` split Cody–Waite style. `exp` is `exp_neg(−x)` for `x ≤ 0` and
    the same reduction otherwise. `sin_cos_turns(u)` returns `(sin 2πu, cos 2πu)` for `u ∈ [0, 1)`. Its quadrant
    and its fold to `[0, π/4]` are exact, so quarter turns are exact; nested Taylor series do the rest. Measured
    against correctly rounded values: `ln` within 2 ulp over 12,387 arguments, `exp` within 1 ulp over 3,292,
    and the sine and cosine within 2 ulp over 4,096 turns each.
- Random addressing (`fnp-philox-semantic-v1`, §16.2, INV-11).
  - An address is `(stream, entity, ordinal, lane, draw)`. Its block is the first four outputs of NumPy's
    `Philox(key=[world_seed, stream code], counter=[entity, ordinal, lane, draw])` through FrankenNumPy's
    NumPy-exact `PhiloxRng`, which is Philox4x64-10 of `counter + 1`.
  - Stream codes are frozen eight-byte ASCII tags: `founders`, `mutation`, `srcchems`, `srcpaths`, `srcplace`
    and (FL-011) `birthing`.
  - A lane packs `candidate << 56 | kind << 48 | index` with frozen kind codes 1–10, and a neuron index
    `layer << 32 | output << 16 | input`. A value out of range is refused (`Overflow`), never wrapped.
  - Distinct addresses never share a block, because the counter map is injective and Philox is a bijection for
    a fixed key.
  - Nothing is stateful. A block depends on the seed and its address, never on worker, task order, storage slot,
    wall time or frame.
  - Draws:
    - the half-open uniform `k 2^-52` and the open uniform `(k + ½) 2^-52` from a word's leading 52 bits `k`,
      both exact;
    - `choose` as `⌊k n / 2^52⌋` in integer arithmetic;
    - Box–Muller `[ρ cos 2πu₂, ρ sin 2πu₂]` with `ρ = √(−2 ln u₁)`, an open `u₁` from the first word, a
      half-open `u₂` from the second, and no cached spare.

**Invariants — the checkpoint container (FL-016, B.2).**
- A streaming writer (FL-038) and an in-memory one write the same bytes: the same chunks, digests and sealed
  prefix. A streaming writer's chunk holds at most one page before it is handed to the sink, and a sink's
  refusal is kept and returned when the chunk ends. `finish` refuses a streaming writer.
- Layout, little-endian: magic `FLIFECKP`, format 1, hash algorithm 1 (SHA-256), the model identifier (UTF-8, at
  most 256 bytes), the manifest's length (at most 64 KiB), the manifest, a SHA-256 seal over every byte before it,
  then the payload.
- The manifest lists each chunk once: its family (one of the eleven `ChunkFamily` codes), codec (0, raw), version,
  item count, payload offset, encoded and decoded lengths, the SHA-256 of its bytes, and at most 16 families it
  depends on. Chunks are contiguous and in manifest order, and every dependency is an earlier chunk, so the
  manifest order is a valid decoding order.
- The writer refuses a family written twice, a dependency not yet written and more than 16 dependencies.
- The reader checks everything before any chunk is decoded, in this order: magic, format, hash algorithm, model
  identifier, manifest bound and seal; then each entry's family, uniqueness, codec, dependencies, lengths,
  contiguity, bounds and hash; then that the chunks cover the payload exactly and that every family the caller
  requires is present. A chunk is handed out only at the version its decoder asks for.
- `ByteReader::count` checks a declared count against its cap and against the bytes left before anything is
  allocated, and `finish` refuses unread bytes. Floats travel as their bit patterns.
- `EnergyLedger::restore` takes only finite state whose next tick is present exactly when a tick has been
  checked, and whose cumulative residual is inside the budget's INV-03 allowance. The restored ledger checks
  later ticks as the original would.

**Refusal cases (A.2).**
- Nonfinite numbers and negative costs.
- Efficiency outside (0, 1]. Nonpositive discrimination, toxicity or saturation scales.
- A moment-channel count other than 32. A sensor width other than 104 or an action width other than 36.
- More than two hidden layers, or a default network above the parameter cap.
- An unsupported dtype, or an unknown or misplaced algorithm.
- No viable child fraction at the minimum attempt energy, or child-fraction bounds out of order.
- Strict no-op rejection with every mutation source at zero, a perturbation with a zero deviation counting as
  none (§7.3).
- More than eight candidate proposals per attempted birth (§7.3).
- A sensing scale below the finest pyramid level, or a grid not divisible by `2^levels`.
- A source kernel that can miss every cell center.
- A source segment that is not tick-aligned.
- Memory quotas out of order, or an initial population above the logical genome quota.
- Gradient training, mutation-rate evolution or automatic reseeding (none is part of `field-ecology-v1`).
- An initial population above the ceiling.
- A malformed name, background or schema string.

Every issue is reported, not just the first.

**Refusal cases (ledger).**
- A negative or nonfinite holding, flow, transfer, budget or payment.
- A transfer that credits more than it debits.
- A residual outside the per-tick or the cumulative allowance.
- A tick that is out of order, does not open where the last one closed, or was checked against another budget.
- A credit to an exhausted budget.
- Retaining more than a budget holds, or retaining from an exhausted budget.

**Not refused here (by design).**
- The worst admitted allocation exceeding the logical quota. It is reported in `worst_case_fits_logical_quota`,
  and §7.5 admission enforces it at run time.
- Host capability checks and backend/renderer combinations. Only one backend and one renderer are registered today.
- Topology-catalog membership beyond depth and parameter cap (FL-009).

**Registry status.**
- Implemented here with fixtures: `field-ecology-v1` and the `stable-identity` phase policy.
- Implemented in `flife-field`:
  - FL-003: the `periodic` boundary, the `f32` field dtype and the `zero` field composition.
  - FL-004: `positive-five-point-v1`, `subcycle`, `fixed-total-power`,
    `normalized-truncated-minimum-image-gaussian-v1` and `fixed-per-source`.
  - FL-005: `grid-cic-v1`.
- Implemented in `flife-engine` (FL-006): `bounded-affinity-v1` and `per-granted-cell-packet`. The heritable
  traits `affinity_direction` and `motion_response` have their representation, bound and use there.
- FL-007: `positive-box-v1` and `log-linear-v1` in `flife-field`; `energy-moment-gradient-104-v1` and
  `fixed-local-scales-v1` in `flife-neural`. The heritable `sensing_scale` is sampled at any admitted scale
  there.
- FL-008, in `flife-neural`: `independent-dense-mlp-v1`, the `tanh` hidden activation, the `linear` output
  activation and `f32` accumulation.
- FL-009, in `flife-genome`: the `f32` weight dtype of the canonical genotype, `dense-104-36-catalog-v1`,
  `overlap-plus-zero-output-v1` and `prefix-and-identity-bridge-v1`.
- FL-010:
  - `fnp-philox-semantic-v1`, here.
  - In `flife-genome`: `fan-in-scaled-symmetric-v1` with the founders' semantic words, `uniform-other-catalog-member`
    and `reject-birth-no-charge`, plus the mutation of the heritable traits `affinity_direction`,
    `motion_response` and `sensing_scale`, which completes them.
  - In `flife-field`: the seeded draws of FL-004's `seeded-unit-vectors` and `seeded-smooth-torus-v1`, which
    completes those too.
- FL-011, in `flife-engine`: `no-emission-no-charge`, the `post-motion` emission position, `next-tick` emission
  visibility and `next-tick` newborn activation. `reject-birth-no-charge` now also cites the birth transaction's
  test. `pause-before-commit` stays `Specified` under FL-013, which owns pausing the tick. FL-011 supplies the
  `CapacityShortfall` it pauses on.
- FL-012, in `flife-engine`: the tiny-energy policy `dissipate-and-record`, the last budget stage (§8.2), recorded
  as the ledger's `tiny-reservoir` flow, and the `genetic-affinity` corpse direction. `stop-and-preserve` stays
  `Specified` and moves to FL-013, which owns stopping the run. FL-012 supplies the refusal of a numerical fault
  by identity.
- FL-013, in `flife-engine`'s `world`: the backend `cpu-reference`, the determinism class `qualified-reference`,
  the capacity policy `pause-before-commit` (the whole tick pauses), the fault policy `stop-and-preserve` (the run
  stops on the committed state), and the founders' `uniform` initial positions and `zero` initial velocity, moved
  from FL-015. Founder positions draw words 0 and 1 of `(founders, organism, 0, lane (0, position, 0), 0)`.
- FL-014, in `flife-runtime`: the command durability `volatile-in-memory`, §12.3's labelled best-effort mode.
  Acceptance is held by the session owner and lost if the process ends.
- FL-036, in `flife-runtime` and `flife-store`: Appendix A's command durability `write-ahead-before-apply`. A
  session opened with `open_journaled` relies on an acceptance, cancellation or outcome only once its FrankenSQLite
  command journal has made it durable; the evidence is C.14's crash at each point of an energy brush.
- FL-037, in `flife-cli`: Appendix A's checkpoint mode `pause-and-stream`. `flife run --run-dir` takes no step
  while a checkpoint is taken and published, and a run stopped by a damaged newest checkpoint or by SIGKILL
  resumes with `flife resume` to the uninterrupted run's digest and final checkpoint bytes. Since FL-038 the
  checkpoint streams into the store a page at a time from the borrowed world.
- FL-038, in `flife-cli`: the checkpoint mode `bounded-background`. A writer thread drains the checkpoint's pages
  through a queue of at most `checkpoint_retained_bytes` while the run steps on; its checkpoints are
  byte-identical to `pause-and-stream`'s.
- FL-017, in `flife-render` and `flife-runtime`: the observer pressure policy
  `drop-observer-or-pause-unchanged-world`.
  - The render feed takes its drop arm: a busy feed skips the boundary and keeps the last displayable epoch, and
    the world is never touched.
  - Its pause arm belongs to observers whose retention competes with the world's resources (FL-021, FL-038).
  - `execution.render_queue_capacity` must be at least 2 (`IntBound::AtLeastTwo`): one displayable epoch and one
    being written.
- FL-041, in `flife-engine`: two more `Role::Model` entries, `field-ecology-v1-chemistry-only` and
  `field-ecology-v1-fresh-offspring` (`model::FIELD_ECOLOGY_V1_CHEMISTRY_ONLY` and
  `FIELD_ECOLOGY_V1_FRESH_OFFSPRING`), each `field-ecology-v1` with one rule changed for §15.3's controls, cited to
  `flife-engine`'s `tests/ablation.rs`.
  A configuration naming one validates like `field-ecology-v1` and has its own configuration hash and model
  descriptor.
- Every other entry is `Specified`: defined in the plan and owned by the named FL item, with no known-answer
  vectors yet. A run naming one cannot pass G1 or G3 until its owner implements it.
- An `Implemented` entry cites a `test:<crate>::<path>::<fn>`, and the registry test checks that the cited
  function exists.

**Error classes.**
- `LayoutMismatch` covers a buffer whose declared layout is impossible or is not the one its reader requires.
  Such a buffer is refused, never reinterpreted.
- `AllocationFailed` covers a host that refuses an allocation every checked limit admitted.
- `StateConflict` covers work prepared against one state and then offered to another state, or offered twice.
  Examples are uptake grants applied to a changed field, a ledger tick that does not continue the last one, and
  a credit to an exhausted budget. Nothing is changed.
- `IncompatibleCheckpoint` (FL-016) covers a checkpoint that fails any container or chunk check. The reason
  names the check. A damaged checkpoint and one of another format share this class.
- `StorageFailed` (FL-035) covers a storage operation the host refuses: a full disk, a device or permission
  failure, or a path in the way. It names the operation and the path.
- `CoreError::kind` (FL-015) names each class for machine output (§14.6), such as `missing-capability` or
  `numerical-invalid`. Every name is stable, kebab-case and distinct. `flife-cli`'s error records carry it, and
  `errors_name_their_class` pins all fourteen.

**Determinism.** Pure functions of their inputs. No hash-map iteration and no floating clocks. The canonical forms
and both digests are pinned by tests. Ledger sums run in a fixed order.

**Independent tests.**
- `tests/ledger.rs`:
  - C.1's accounting, checked exactly.
  - Round-down and the INV-05 bound, checked against exact `i128` arithmetic on dyadic values.
  - The cumulative gate tripping at a computed tick, and every reported normalization checked exactly.
  - `retain` keeps exactly what it is asked to and pays the difference, over 2,000 random cases and C.5's 10 to 6.
    Keeping zero exhausts the budget. It refuses an exhausted budget and invalid amounts.
- `tests/numerics.rs`:
  - 24 pinned bit patterns, each within 1 ulp of its correctly rounded value.
  - Agreement with the platform `libm` over a log-spaced and dense sweep, within bounds that hold on any
    conforming host.
  - Domains, saturation, odd symmetry, `σ(x) + σ(−x) = 1`, and monotonicity.
  - C.11: the drag coefficient, the F32 cancellation, and `sigmoid(8)` against `sigmoid(tanh(8))`.
  - `log2`: every power of two exact, seven pinned bit patterns, agreement with `std` over a sweep, and refusal
    of nonpositive and nonfinite arguments.
  - `ln` and `exp`: 19 pinned bit patterns with their correctly rounded values, agreement with `std`, exactness at
    `ln 1` and `exp 0`, `exp(−x) = exp_neg(x)`, and their domains.
  - Turn angles: nine pinned pairs, exact quarter turns, agreement with `std` and `sin² + cos² = 1` over 4,096
    turns, and the domain.
- `tests/rng.rs`:
  - Philox blocks equal NumPy 2.5.4's at zero, all-ones and carrying counters, and at seven semantic addresses.
    FL-011 adds two pins on the `birthing` stream. Every expected word comes from NumPy itself, through an oracle script that imports neither FrankenLife nor
    FrankenNumPy.
  - Frozen stream codes and injective lane packing, with out-of-range refusals.
  - Blocks are the same in forward, reverse and four-worker order. No two of 576 addresses share a block, and
    another seed moves all of them.
  - Exact uniform and choice conventions.
  - Box–Muller pairs, pinned and within 4 ulp of correctly rounded, with the exact quarter turn.
  - Moments over 200,000 normals.
  - The family and seed read from Appendix A, and another family refused.
- `tests/checkpoint.rs` (FL-016):
  - A written container parses back to the same chunks, versions and item counts. Another version, an absent
    family and a missing required family are refused.
  - The writer's three refusals, and an over-long model identifier.
  - Thirteen container defects, each refused by its own check: seven in the header, the seal or a chunk's bytes,
    and six manifest defects re-sealed so that only the defect can be the reason. A trailing byte and every
    truncation are refused too.
  - Counts bounded by their cap and by the bytes left, a flag byte of 2, and unread bytes.

**Validity domain.** `frankenlife.config.v1`, `field-ecology-v1` (and FL-041's two ablation models) and
`frankenlife.model-descriptor.v1`. FL-016:
checkpoint format 1 with SHA-256 and raw chunks.
