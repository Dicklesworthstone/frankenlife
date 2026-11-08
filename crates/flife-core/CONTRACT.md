# flife-core — contract

**Purpose.** FL-002 (`flife-jb0`): the shared vocabulary every FrankenLife crate builds on (plan §18.1, §18.3). That
covers identities, integer clocks, units, failure classes, the named-algorithm registry, the
`frankenlife.config.v1` configuration with its A.2 validator, canonical serialization, and the model descriptor.
FL-005 (`flife-f6x`) adds the global energy ledger (§4.4, §8.2, §21.4, INV-03 and INV-05). FL-006 (`flife-h6c`)
adds the host-independent elementary functions of the reference numerical profile (A.1, C.11).

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
  - The per-organism `EnergyBudget`, with `Payment`.
  - `CompensatedSum`, and the round-down helpers `add_down` and `sub_down`.
- Numerics (FL-006): `exp_neg`, `expm1_neg`, `tanh` and `sigmoid`.

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

**Refusal cases (ledger).**
- A negative or nonfinite holding, flow, transfer, budget or payment.
- A transfer that credits more than it debits.
- A residual outside the per-tick or the cumulative allowance.
- A tick that is out of order, does not open where the last one closed, or was checked against another budget.
- A credit to an exhausted budget.

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
  traits `affinity_direction` and `motion_response` have their representation, bound and use there, but stay
  `Specified` under FL-010 until they mutate as A.3 item 4 specifies.
- The tiny-energy policy `dissipate-and-record` belongs to FL-012. Clearing a tiny reservoir is the last budget
  stage before an exhausted organism dies (§8.2). The ledger's `tiny-reservoir` flow is ready for it.
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

**Determinism.** Pure functions of their inputs. No hash-map iteration and no floating clocks. The canonical forms
and both digests are pinned by tests. Ledger sums run in a fixed order.

**Independent tests.**
- `tests/ledger.rs`:
  - C.1's accounting, checked exactly.
  - Round-down and the INV-05 bound, checked against exact `i128` arithmetic on dyadic values.
  - The cumulative gate tripping at a computed tick, and every reported normalization checked exactly.
- `tests/numerics.rs`:
  - 24 pinned bit patterns, each within 1 ulp of its correctly rounded value.
  - Agreement with the platform `libm` over a log-spaced and dense sweep, within bounds that hold on any
    conforming host.
  - Domains, saturation, odd symmetry, `σ(x) + σ(−x) = 1`, and monotonicity.
  - C.11: the drag coefficient, the F32 cancellation, and `sigmoid(8)` against `sigmoid(tanh(8))`.

**Validity domain.** `frankenlife.config.v1`, `field-ecology-v1` and `frankenlife.model-descriptor.v1`.
