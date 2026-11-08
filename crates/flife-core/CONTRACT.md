# flife-core — contract

**Purpose.** FL-002 (`flife-jb0`): the shared vocabulary every FrankenLife crate builds on (plan §18.1, §18.3). That
covers identities, integer clocks, units, failure classes, the named-algorithm registry, the
`frankenlife.config.v1` configuration with its A.2 validator, canonical serialization, and the model descriptor.

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

**Refusal cases (A.2).**
- Nonfinite numbers and negative costs.
- Efficiency outside (0, 1]. Nonpositive discrimination, toxicity or saturation scales.
- A moment-channel count other than 32. A sensor width other than 104 or an action width other than 36.
- More than two hidden layers, or a default network above the parameter cap.
- An unsupported dtype, or an unknown or misplaced algorithm.
- No viable child fraction at the minimum attempt energy, or child-fraction bounds out of order.
- Strict no-op rejection with every mutation source at zero.
- A sensing scale below the finest pyramid level, or a grid not divisible by `2^levels`.
- A source kernel that can miss every cell center.
- A source segment that is not tick-aligned.
- Memory quotas out of order, or an initial population above the logical genome quota.
- Gradient training, mutation-rate evolution or automatic reseeding (none is part of `field-ecology-v1`).
- An initial population above the ceiling.
- A malformed name, background or schema string.

Every issue is reported, not just the first.

**Not refused here (by design).**
- The worst admitted allocation exceeding the logical quota. It is reported in `worst_case_fits_logical_quota`,
  and §7.5 admission enforces it at run time.
- Host capability checks and backend/renderer combinations. Only one backend and one renderer are registered today.
- Topology-catalog membership beyond depth and parameter cap (FL-009).

**Registry status.** `field-ecology-v1` and the `stable-identity` phase policy are implemented here with fixtures.
`flife-field` implements the `periodic` boundary, the `f32` field dtype and the `zero` field composition
(FL-003). It also implements `positive-five-point-v1`, `subcycle`, `fixed-total-power`,
`normalized-truncated-minimum-image-gaussian-v1` and `fixed-per-source` (FL-004). Every other entry is `Specified`: defined in the plan and owned by the named FL item, with no
known-answer vectors yet. A run naming one cannot pass G1 or G3 until its owner implements it. An `Implemented`
entry cites a `test:<crate>::<path>::<fn>`, and the registry test checks that the cited function exists.

**Error classes.** `LayoutMismatch` covers a buffer whose declared layout is impossible or is not the one its
reader requires; such a buffer is refused, never reinterpreted. `AllocationFailed` covers a host that refuses
an allocation every checked limit admitted.

**Determinism.** Pure functions of their inputs. No hash-map iteration and no floating clocks. The canonical forms
and both digests are pinned by tests.

**Validity domain.** `frankenlife.config.v1`, `field-ecology-v1` and `frankenlife.model-descriptor.v1`.
