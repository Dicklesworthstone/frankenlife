# AGENTS.md - FrankenLife

> Guidelines for AI coding agents working in this real-time artificial-life codebase (`frankenlife`; crates
> `flife-*`; binaries `flife`, `flife-studio`, later `flifed`).

---

## RULE 0 - THE FUNDAMENTAL OVERRIDE PREROGATIVE

If I tell you to do something, even if it goes against what follows below, YOU MUST LISTEN TO ME. I AM IN
CHARGE, NOT YOU.

---

## RULE 0.1 - VALUE DELIVERY OVER PROCESS: NO PROCESS PORN

FrankenLife is early. The first deliverable is a small world that **lives, evolves, renders and can be
inspected** (plan §22.2, Appendix E): source energy enters a field, organisms sense it, independent neural
controllers decide, chemistry causes gain, loss and motion, paid emission changes the field, reproduction produces
a mutated child, death releases resources, and the result appears live through FrankenManim. Every hour spent on
meta-infrastructure is an hour not spent on that cycle.

**Process is never the product unless the user explicitly asks for process work.** Beads, plans, Agent Mail,
audits, registries, manifests, provenance, logging, CI, harnesses, dashboards and status reports exist only to
support delivery.

### The product/ceremony boundary

Does production code branch on this artifact? If yes, it is product state. If only humans and status reports read
it, it is process, and it may exist only if it names:
1. a concrete consumer;
2. the gate it enforces;
3. the observed defect class that justifies it;
4. its retirement condition.

### The value test

Before doing non-product work, answer all three:
1. What concrete capability, correctness defect or immediate blocker does it address?
2. Is it the smallest direct action that addresses it?
3. Will its likely value exceed its cost right now?

If any answer is unclear, **do not do the work**. Return to implementing the requested functionality.

### Hard scope limits

- **Real code and real tests come in the same unit of work.** Not allowed:
  - faked tests;
  - mocks, frozen populations or synthetic GEMMs presented as live proof;
  - weakened assertions or widened tolerances without a documented reason (plan §21.4);
  - hard-coded success paths;
  - `todo!()` or `unimplemented!()` in committed production paths.
- **No placeholder success (plan §18.5).** A backend, renderer or feature that compiles but cannot simulate or
  render reports an explicit unavailable capability. An explicit refusal beats a placeholder that reports success,
  but refusal-only work never closes an item that promises a positive capability.
- **Release scope control (plan §22.5).**
  - M1 is never delayed by a daemon, plugin marketplace, arbitrary graphs, a 3D world, a distributed grid or
    automated narration.
  - M4 is never declared complete through a shared-policy neural shortcut, a reduced undeclared cadence, a
    prerecorded animation or a renderer benchmark with inference disabled.
- **Reviews stay narrow.** A review request authorizes:
  - reading the relevant code;
  - reproducing concrete defects;
  - the smallest sound fixes and focused regressions.

  It does not authorize redesigning adjacent infrastructure.
- **No bloat.** Never turn a small repair into hundreds of lines of harness, schema, logging or planning without
  explicit approval. Do not build a validator for a validator or a harness for a harness. Do not create dozens of
  empty crates before the first living world (plan §17.6).
- **Stay on target.**
  - Do not chase a failure already present on `HEAD` unless it blocks the requested deliverable.
  - Do not spawn an agent swarm for a narrow task.
  - Do not build distributed or speculative infrastructure while a single world still copies its full neural bank
    per frame (plan §19.7).

### Implementation must dominate

Spend the dominant share of effort on working product code and its direct known-answer, property, metamorphic and
integration tests. Prefer an existing test seam over a new framework. Run the narrowest relevant checks while
iterating. Optimize only measured bottlenecks, in the order of plan §19.7.

### Mandatory checkpoint and stop rule

At the first sign of scope expansion, state:
- the outcome;
- the files involved and the size of the expansion;
- why it is necessary;
- the smaller alternatives.

If that cannot establish immediate value in a few sentences, ask first. When work has drifted into process porn,
stop, freeze the tree, disclose the state and wait.

---

## RULE NUMBER 1: NO FILE DELETION

**YOU ARE NEVER ALLOWED TO DELETE A FILE WITHOUT EXPRESS PERMISSION.** Even a new file that you yourself
created, such as a test file. You must always ask and receive clear, written permission before deleting a
file or folder of any kind.

---

## Irreversible Git & Filesystem Actions - DO NOT EVER BREAK GLASS

1. **Absolutely forbidden commands:** `git reset --hard`, `git clean -fd`, `rm -rf`, or any command that can
   delete or overwrite code/data, unless the user explicitly provides the exact command and states, in the
   same message, that they understand and want the irreversible consequences.
2. **No guessing:** if there is any uncertainty about what a command might delete or overwrite, stop and ask.
3. **Safer alternatives first:** `git status`, `git diff`, backups, hand-written patches.
4. **Mandatory explicit plan:** even after authorization, restate the command verbatim, list what will be
   affected, and wait for confirmation.
5. **Document the confirmation** in your final response: the authorizing text, the command, the time.

Never stash, revert, overwrite or otherwise disturb another agent's work.

---

## Git Branch: Canonical `main`, Mirrored to `master`

All work happens on `main`. The only other permitted branch is `master`, an exact mirror of `main`.
After publishing `main`, fast-forward both local and remote `master` to the same commit. Never develop on
`master`, force-push it, or discard unique history to make it match; integrate useful work into `main` first.

## RULE 2: NO GIT BRANCHES. NO GIT WORKTREES. EVER.

**Forbidden:**
- feature branches, `git checkout -b`, `git switch -c` and `git worktree add`;
- pushing refs other than `main` and its `master` mirror;
- creating pull requests;
- scratch clones of this repository used to isolate work;
- any tool that creates branches or worktrees as a side effect.

**Instead:**
- commit to `main` when the work is ready, and keep unfinished work in the tree;
- coordinate through Agent Mail reservations and work-item IDs;
- never revert or stash another agent's changes.

---

## Project Truth Sources

- `COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md` is the technical constitution (§1–§28, Appendices A–E).
  It sets:
  - the model, FieldEcologyV1, and its equations (§4–§8);
  - data layout, accelerator and reduction design (§9–§11);
  - the Asupersync runtime and command protocol (§12);
  - FrankenManim rendering and the Studio (§13–§14);
  - experiments, checkpoints and extension contracts (§15–§17);
  - the workspace, performance model and operating profiles (§18–§19);
  - invariants INV-01–INV-15, gates G0–G10 and tolerances (§21);
  - milestones M0–M6 and the backlog FL-001–FL-058 (§22–§23);
  - decision rules and architecture decisions ADR-01–ADR-14 (§24, §26);
  - the source register S1–S22 with pinned research revisions (§27);
  - the starter configuration, artifact schemas, known-answer fixtures C.1–C.17 and the field-invariant proof
    (Appendices A–D).
- `franken-stack.lock` records the qualified dependency closure and the probes that passed it. The closure probe in
  `crates/flife-probes` (FL-001) fails the build's tests when `Cargo.lock` drifts from it.
- Each substantial crate has a `CONTRACT.md` (plan §18.4) once another crate depends on it.

When this file and the plan disagree on a technical rule, the plan wins; flag the discrepancy. When the plan and
observed code or measurement disagree, the plan is subordinate to what was observed (Appendix E): record the
evidence and propose the plan change.

---

## FrankenLife - This Project

FrankenLife is a real-time artificial-life laboratory in memory-safe Rust. Hundreds of thousands of continuously
positioned organisms each carry their own heritable neural controller and chemistry, and exchange energy and
information only through a shared field. Its execution substrate is Asupersync, FrankenTorch and FrankenNumPy;
its animation and presentation system is FrankenManim.

Three operating modes share one engine (plan §2.1):

| Mode | Purpose | Rule |
|---|---|---|
| Studio | Watch, inspect, intervene, discover | Never runs its own physics inside a frame callback |
| Laboratory | Reproduce and compare experiments | Headless runs, seed ensembles, branches, metrics, checkpoints |
| Cinema | Explanatory or artistic output | Never invents simulation events |

The primary inspiration is a set of screenshots of Fraser Price's Neural-Kimchi-4B and its description. They are
design evidence, not an executable specification. FrankenLife implements an original, explicit rule set and must
never claim numerical parity with Neural-Kimchi without an independently obtained executable reference and a
defined comparison protocol (plan §1.2).

### What this project is NOT

- Not a port of Neural-Kimchi-4B, and not affiliated with it.
- Not a decorative particle animation. No structure is painted into the simulation; a membrane overlay may
  describe detected structure but never feeds forces back (plan §2.3).
- Not a shared-policy, distilled or low-rank network presented as independent controllers (plan §6.5).
- Not trained by gradient in the baseline; no autograd tape, optimizer, minibatch or global loss (plan §6.2).
- No direct pairwise forces, collisions, adhesion or predation in the baseline. Those define later model
  variants, never fixes for unattractive output (plan §5.4).
- No hidden culling, no automatic reseeding after extinction, no aesthetic fitness function inside the world
  (plan §7.5, §15.5).
- Not a 3D ecology, distributed single world, network service or full-scale browser application in the initial
  scope (plan §1.4, §14.6, §20).

---

## THE FOUR CLAIMS - NEVER CONFLATE THEM (plan Executive design, §15, §21)

| Claim | Meaning | Evidence | Permitted wording |
|---|---|---|---|
| **Rules** | The simulation obeys its own rules | INV-01–INV-15 and the energy ledger on every committed tick | "INV-03 holds within residual r over n ticks" |
| **Implementation** | The code reproduces those rules | Independent fixtures (Appendix C), the scalar oracle, backend parity (G1, G4) | "matches fixture C.9 to tolerance t" |
| **Visual** | The images have the intended character | Human review against the references, raw views alongside aesthetic ones (G5) | "reviewed against the references on <date>" |
| **Ecological** | Populations show measurable evolutionary phenomena | Ablations, seed ensembles, held-out seeds, failed runs kept (G9) | "effect e over k of n seeds versus control" |

A beautiful image establishes only the visual claim. A performance number is a separate measurement on named
hardware (Appendix B.4) and is evidence for none of the four. A gate names the claim it establishes.

---

## Status Labels And Honesty Rules

The plan's labels apply to code comments, docs, commit messages and reports:

| Label | Meaning |
|---|---|
| **Observed** | Seen in inspected source, a run or a measurement, with its revision or record |
| **Derived** | Calculated from stated assumptions; show the calculation |
| **Target** | An acceptance criterion, not a result until a gate records it |
| **Hypothesis** | An empirical question, with the test that would refute it (plan §15.4, H1–H6) |
| *(unlabelled)* | A proposed design, not existing software |

1. **Never regenerate a known-answer fixture from the candidate.** Oracles stay separate from production kernels;
   a test that computes expected and actual with the same helper validates nothing (plan §21.1).
2. **No hidden energy.** No correction may silently add or remove energy to balance the ledger (plan §4.4).
   Requested cost and actual paid cost are distinct values; unpaid maintenance is not dissipation (plan §8.2).
3. **Never fill a result with its target.** An unmeasured field is absent or explicitly unavailable (Appendix B.1).
4. **Name the profile.** F16 genomes are a distinct model, multi-rate cadence is a separately named profile, and
   G7 requires full evaluation. Never lower cadence or enlarge `Δt` under load while keeping the model identity
   (plan §6.6, §10.3, §12.2).
5. **Capacity effects are visible.** A birth rejected by a quota or memory limit is externally imposed selection,
   not an ecological result (plan §7.5, §15.7).
6. **Numerical faults never masquerade as death.** Research mode stops or quarantines under a named fault policy
   (plan §7.6).
7. **No unearned platform claims.** Do not claim the dependency graph is unsafe-free (plan §3.3), that unified
   memory is zero-copy (§10.4, §13.6), or that GPU frames inherit FrankenManim's certified CPU pixel identity
   (§13.6). Do not call upstream Rayon work Asupersync-scheduled (§3.4).
8. **Galleries are honest.** Recordings are real simulation output carrying run ID, seed or checkpoint, model
   version and display mapping. A curated seed bank is never presented as the result of arbitrary random
   initialization, and selection procedures are stated (plan §2.4, §7.2, §15.5).
9. **Report failures.** Extinct and failed runs are kept when evaluating claims. Partial never rounds up.

---

## Data, Assets And Attribution

- **Reference screenshots are not committed.** The Neural-Kimchi-4B images (`IMG_2165`–`IMG_2168`) are third-party
  design evidence, read locally and cited by file name.
- **No large artifacts in git.** Recordings, frame dumps, checkpoints, chunk stores and run histories belong in
  artifact storage (plan §18.5). The repository carries the plan, code, small independent fixtures, presets,
  benchmark definitions and compact gallery manifests.
- **No personal data or secrets** in fixtures, docs, examples or commit messages.
- **No affiliation is implied** with Neural-Kimchi-4B, its author, or the related systems the plan cites
  (neural cellular automata, Lenia, Flow-Lenia, Particle Lenia).

---

## Pins

| Item | Pin |
|---|---|
| Toolchain | `nightly-2026-08-31` (`rust-toolchain.toml`), shared with the FrankenSuite siblings; Rust 2024 edition; one Cargo workspace |
| Async runtime | Asupersync v0.6.2 (`a5d23e8d`), the sole runtime, with one package id per closure. FrankenManim takes it from git and FrankenTorch from crates.io, so the root `[patch.crates-io]` maps the crates.io edge onto the same commit (plan §12.8) |
| Sibling revisions | `franken_manim` `ac39e721`, `frankentorch` `c611501c`, `franken_numpy` `6492ff31`, plus FrankenManim's transitive pins; the full qualified set is in `franken-stack.lock` |
| Root overrides | `[patch.crates-io]` in `Cargo.toml`: asupersync → `a5d23e8d`, and the reviewed `rust-block` `b39ae859d1ee8e8cb5eef6a516471f1578d26b96`. Both live at this root because Cargo ignores a dependency's own `[patch]` (plan §12.8) |

Moving a pin is a deliberate event: requalify the capability probes and keep the old results.

---

## Toolchain, Unsafe Code And Lints

- `#![forbid(unsafe_code)]` in every FrankenLife crate. No raw-pointer optimizations, unchecked indexing, manual
  `Send`/`Sync`, application-level FFI or unsafe shortcuts. Platform bindings and intrinsics stay in their
  reviewed upstream boundary crates; FrankenLife uses only their safe, validated public interfaces, and validates
  shape, length, offset, access mode and shader bounds before every dispatch (plan §3.3).
- `unwrap_used`, `expect_used` and `panic` are denied in production library code. Tests may use them.
- Checked 64-bit totals for parameter counts, byte offsets and artifact lengths. Four billion parameters already
  overflow 32-bit byte offsets (plan §9.2).

### Determinism (plan §16)

- RNG is addressed semantically by `(world_seed, subsystem, stable_entity_id, tick_or_birth_ordinal, draw_lane)`.
  Never seed by worker index, task order, physical slot, wall time or display frame.
- Stable identity is never a storage slot. Reusing a slot never reuses an identity, its randomness or its held
  controller state.
- Fixed reduction order on the qualified deterministic route; no hash-map iteration that affects state.
- Integer ticks for age, cooldowns and deadlines. Never accumulate `Δt` in an F32 clock (plan §4.1).
- Determinism classes (canonical reference, qualified backend, cross-backend numerical, exploratory throughput)
  are stated with every replay claim. Identical seeds alone never promise identical worlds across devices.

### Dependency Policy

- **One runtime.** Asupersync owns scheduling, supervision, deadlines and task lifetime. No detached application
  tasks; no per-organism tasks, futures, actors or mutexes (plan §9.1, §12.1).
- **One worker budget.** Upstream numerical threads and FrankenManim's CPU renderer threads count toward the same
  admission budget. No nested independent worker pools; each adapter states which executor runs its jobs
  (plan §3.4, §12.5).
- **Banned as production dependencies:** Python wrappers, NumPy fallback, PyTorch/libtorch, training frameworks,
  and any second async runtime (tokio and friends). Storage goes through FrankenSQLite, not rusqlite or sqlx;
  analysis through FrankenPandas and FrankenNetworkX, not polars or petgraph.
- **Narrow adapters.** Upstream adaptation lives inside narrow crates (plan §12.8). Missing upstream capability is
  a deliverable in the upstream repository (plan §22.4), never hidden behind "integrate library".
- **A small core.** The headless engine builds without Studio, video encoding, graph analysis or 3D. The engine
  never depends on a concrete backend; the composition root injects one through the core ports (plan §17.6,
  §18.1).
- **Static composition.** Traits and enums chosen at startup, dispatched once per model, bucket or phase. No
  trait-object calls inside per-neuron or per-field-component loops (plan §17.3).
- A new dependency needs a concrete capability and an isolation boundary, not a hypothetical future use.

---

## Workspace Crates

The proposed workspace (plan §18.1). **A crate joins the workspace only with its first real slice.** No new crate
outside this map without explicit user approval.

| Crate | Responsibility |
|---|---|
| `flife-core` | IDs, units, schemas, errors, epochs, model descriptors, kernel-port contracts |
| `flife-field` | Field semantics, exchange/transport reference operators, layout contracts |
| `flife-genome` | Chemistry/controller genomes, mutations, topology catalog, ancestry encoding |
| `flife-neural` | Sensor/action ABI, scalar oracle, FrankenTorch population adapter |
| `flife-engine` | Tick state machine, budgets, lifecycle transactions, backend-neutral orchestration |
| `flife-backend-cpu` | Qualified CPU phase execution and controlled parallel kernels |
| `flife-backend-metal` | Device owner, kernel templates, resource leases, Metal execution |
| `flife-runtime` | Asupersync session ownership, commands, supervision, queue policies |
| `flife-render` | Compact render projections and visual mapping semantics |
| `flife-manim` | Bulk world adapter, camera/timeline, overlays, export integration |
| `flife-store` | Chunk format, checkpoint transactions, run/lineage catalog |
| `flife-analysis` | Metrics, controlled comparisons, graph/column analysis |
| `flife-experiments` | Seed ensembles, sweeps, branching, experiment reports |
| `flife-cli` | Local and machine interfaces |
| `flife-studio` | Interactive application shell |

Split a crate by responsibility when review becomes difficult, not to hit an arbitrary line count.

---

## Model Semantics That Are Easy To Break

Each of these has a plausible-looking wrong version. The fixture named beside it exists to catch that version.
Changing any of them changes the **model identity** and needs a new model version, never a silent edit
(plan §8.4, §17.2, §17.5).

1. **Field.** Every cell keeps `e ≥ 0` and `‖m‖₂ ≤ e`. Energy and every moment component move with the same
   nonnegative weights. Opposite packets cancel moment, never energy (C.3, C.6, Appendix D).
2. **Uptake is two-pass demand/grant** from one immutable snapshot. Never subtract-and-clamp in a single pass, and
   never repair an overspend by zeroing the cell while leaving particle gains intact (§5.2, C.1).
3. **Chemistry runs per granted packet**, never on the requested mixture or the average granted composition
   (§11.3, C.9).
4. **Toxicity debits the clipped actual amount**, `min(requested, E + G)`, never the requested amount (§5.3, C.2).
5. **The final network layer is linear.** Sigmoid is applied exactly once, in decoding (§6.2, C.11).
6. **Batching never shares weights.** `Y_b = X_b W_b + b_b`: `W_b` varies with the organism. A rare topology gets
   a correct fallback, never a different controller, a skip or a cull (§6.5, §10.2, INV-13).
7. **The sensor pyramid averages densities**, not cell-integrated energy. Blend raw values and derivatives across
   levels first, then compute regularized composition (§6.1, C.10).
8. **Use `expm1`** (or a qualified equivalent) for drag, emission fractions and birth hazards (C.11).
9. **Unaffordable motion scales the whole proposed velocity**, not only the active force (§5.4, Appendix A.1).
10. **Phase 7 order is motion, then emission, then reproduction**, and exhaustion is rechecked after every budget
    stage. A zero-energy organism never survives publication (§8.1–§8.2, C.12).
11. **Held raw outputs** are re-decoded against the current energy every tick. A newborn gets a forced first
    evaluation and never inherits stale slot data (§6.6, C.13).
12. **Births are transactions.** At most eight addressed mutation proposals; a rejected birth charges no energy and
    resets no cooldown (§7.3–§7.4, C.5, C.16).
13. **Emission deposits at the post-motion position** and is first visible on the next tick (§5.5, C.4).
14. **Source power is normalized per source**: exactly `P · Δt` per tick regardless of grid or clipping (§4.5,
    C.17).
15. **Costs follow canonical model work**, never wall time, kernel padding, cache behaviour or GPU model. A faster
    backend reproduces the same modeled costs (§5.6, Appendix A.1).
16. **Presentation is pure.** Render fps, resolution, palette, camera and inspector state never change scientific
    state; observer pressure may change wall time but never a birth decision (§7.5, §14.2, C.7, C.14).

---

## Core Invariants (plan §21.2)

| ID | Invariant |
|---|---|
| INV-01 | Committed field energy is finite and nonnegative; every committed live organism has finite, strictly positive energy |
| INV-02 | Every committed chemical moment satisfies the declared cone tolerance |
| INV-03 | Global energy change equals declared injection minus removal and dissipation, within the numerical budget |
| INV-04 | Cell grants never exceed the available cell resource |
| INV-05 | No organism spends the same energy twice |
| INV-06 | A birth commits identity, genome, slot and energy transfer together |
| INV-07 | A dead identity cannot be resurrected by slot reuse |
| INV-08 | Baseline organism interaction is exclusively field-mediated |
| INV-09 | Presentation and inspection do not change scientific state |
| INV-10 | Published epochs are complete and immutable to observers |
| INV-11 | RNG does not depend on worker order or display cadence |
| INV-12 | Capacity policy is explicit and recorded |
| INV-13 | Neural batching preserves independent per-organism weights |
| INV-14 | Every resumable checkpoint contains a complete, validated reconstruction closure |
| INV-15 | Every allocated buffer and dispatch fits checked limits |

### Clocks and publication (plan §8.3, §12.2–§12.6)

- Four clocks: the scientific tick and neural cadence never change automatically; the presentation clock may;
  the export clock follows its declared mode. Never skip scientific ticks or drop accepted interventions.
- Whole-epoch publication. Cancellation before publication discards the candidate; the engine/session owner is the
  sole commit authority. Never replay energy-transfer kernels over partial GPU output.
- GPU cancellation is cooperative: stop new submissions and await the completion receipt before releasing buffers.
  A timed-out future does not prove the device stopped touching memory.
- Native window creation, event polling and presentation stay on the process main thread. A main-thread callback
  never blocks on a simulation kernel, checkpoint or encoder (plan §13.6).
- No blocking database writes, video encoding or analytics while holding the simulation owner (plan §12.4).

---

## Testing Policy

- **Independent fixtures.** Appendix C (C.1–C.17) holds hand-computable known answers. Implement them against an
  oracle that does not share helpers with production kernels.
- **The scalar reference evaluator** with explicitly ordered accumulation is the oracle for neural outputs and
  action decoding (plan §10.1).
- **Metamorphic and adversarial tests** from plan §21.5: slot permutation, view neutrality, worker-count
  agreement, grid-aligned translation, a million requests to one cell, cancellation at every phase, truncated
  checkpoints, duplicate commands, fast-forwarded integer clocks.
- **Asupersync Lab tests** for pause/resume races, full queues, cancellation during checkpoint admission, writer
  failure, device-fence delay and shutdown with a partially prepared birth epoch (plan §12.7).
- **Tolerances.** Exact values where arithmetic permits; otherwise published absolute and relative tolerances per
  quantity (plan §21.4). Long-run cross-backend comparisons are statistical, not bitwise.
- **Placement.** Unit tests sit next to the code. Cross-crate and end-to-end tests live in `tests/`.

---

## Gates And Milestones

| Gate | Required evidence (plan §21.3) |
|---|---|
| G0 Stack | Native crates compile at one qualified lockset with root patches resolved; tensor/RNG/Manim/runtime probes pass; native input acquisition demonstrated or tracked as a blocking host task |
| G1 Arithmetic | Hand fixtures pass; the scalar model obeys INV-01–INV-08; transport stability and units verified |
| G2 First world | Real births, mutations, deaths, paid emission, field response, pause/step and Manim rendering in one runnable application |
| G3 Replay | Off-phase/newborn checkpoint resume and crash/retry recovery match the reference; observation stays state-neutral under lease pressure |
| G4 Accelerated parity | CPU/Metal stage comparisons within declared tolerances, including contention and variable topology |
| G5 Visual | Human review against the references; crisp dense detail, useful deep zoom, no misleading synthetic structure |
| G6 Interactive | 30 fps display and p95 input/camera latency ≤ 100 ms during live inference |
| G7 Scale | ≥ 380,000 live independent genomes, mean 9,000–11,000 parameters, ≥ 30 full-evaluation ticks/s and ≥ 30 fps on a named machine for ≥ 10 minutes |
| G8 Endurance | ≥ 8 wall-clock hours at the qualifying workload with turnover, bounded memory, valid accounting and checkpoint/restart evidence |
| G9 Research | Seed ensembles and ablations support each ecological claim made |
| G10 Export | A recorded interval renders through FrankenManim with correct clock, camera, event and provenance alignment |

- **Milestones** M0–M6 (plan §22.1) are evidence gates, not calendar promises.
- **Critical path** to the first living world: FL-001 → core/field/controller contracts → authoritative tick →
  Asupersync owner → Manim bulk adapter → small-world Studio (plan §23.2).
- **Immediate order** is plan §26.2.
- **Completion evidence (plan §23.3).** A work item closes when its implementation and its listed evidence exist.
  A stub, an unchecked screenshot, a synthetic benchmark unrelated to the workload or a plan paragraph does not
  close it.

---

## Code Editing Discipline

- **No script-based code changes.** No broad regex rewrites; focused hand patches.
- **No file proliferation.** No `main_v2.rs`, `improved.rs` or duplicate module copies.
- **No new crates without approval** (see Workspace Crates).
- **Backwards compatibility.** None is owed to our own pre-1.0 APIs. Checkpoints load only under exact schema and
  model compatibility or an explicit migration that creates a new branch and provenance record (plan §17.5).
  Never deserialize a genome into a superficially similar but semantically different controller.
- **Comments** document invariants, units (world-length, ecological energy, simulation versus display seconds),
  clocks, budgets and no-claim boundaries, not restated code.

---

## Output Style

- Library crates do not print.
- The CLI prints deterministic human text and versioned JSON or NDJSON with stable identifiers and explicit errors
  (plan §14.6).
- Failures are typed and distinct: missing capability, budget exhaustion, schema mismatch, incompatible
  checkpoint, queue closure, numerical invalidity and cancellation (plan §18.3).
- Hot loops never emit per-organism events. Metrics are sampled at a bounded rate with declared completeness.

---

## Compiler Checks

After substantive Rust changes, once the workspace exists:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Use RCH for heavy Cargo work. Never bypass the `rch` cargo shim.

```bash
rch exec -- env CARGO_TARGET_DIR="${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}/rch_target_flife" cargo test --workspace
```

Always base new target dirs on `${RCH_TARGET_BASE:-${TMPDIR:-/tmp}}`, never bare `/tmp`. Before heavy builds on a
laptop, check free disk (`sbh check --need 20G`). Report the exact command and result; if a check did not run,
say so.

The Metal backend, the native presenter and Studio input can only be qualified on an Apple Silicon host. A Linux
worker validates the CPU reference route and headless frame export, nothing more (plan §13.6, §20.1). Say which
host produced every result.

rch copies the working tree to workers and does not read `.gitignore`; anything held out of git for size or
attribution reasons also goes in `.rchignore`.

---

## Documentation

Each substantial crate gets a `CONTRACT.md` before another crate depends on it (plan §18.4), covering:
1. purpose;
2. inputs;
3. outputs;
4. invariants;
5. refusal cases;
6. determinism;
7. validity domain;
8. independent tests;
9. known gaps.

Contracts describe actual behaviour and meaningful limits; they are not a paperwork substitute for implementing
the world. Architecture decisions extend the plan's ADR-01–ADR-14 record (§26.1). Plan edits preserve
identifiers (FL-, INV-, G-, M-, ADR-, S-, C-), keep the status labels, and never append a development diary:
evidence lives near the implementation or in a milestone report (plan §23.3).

---

## Agent Workflow

- **Start.** Read:
  - this file;
  - the relevant plan sections;
  - the crate `CONTRACT.md`;
  - the work item.

  Then inspect the tree.
- **While working.** Keep changes scoped and do not disturb unrelated files. Attach every claim to a test, a
  fixture, a measurement record or explicit no-claim language.
- **End.**
  1. Run the applicable checks.
  2. Report exactly what passed, what did not run, and on which host.
  3. Update the work item and, once beads exist, run `br sync --flush-only`.
  4. Release reservations and leave handoff notes.

---

## MCP Agent Mail - Multi-Agent Coordination

When Agent Mail is available:
- register for this project path;
- reserve files before editing;
- use the work-item ID as the thread ID;
- send start, progress and completion messages;
- release reservations when finished.

Reservations replace branches and worktrees.

Never run:
- `am service restart` or `am service stop`;
- `am doctor fix`, `am doctor repair` or `am doctor reconstruct`;
- `kill` against any `am` or `mcp-agent-mail` process.

If `am` fails, retry once after a few seconds, then continue without it.

---

## Beads (`br`) - Issue Tracking

`.beads/issues.jsonl` mirrors the plan's backlog: 58 beads (prefix `flife`, one per FL-001 … FL-058) and the
plan's 141 dependency edges, acyclic. Each title starts with its FL identifier and its `external_ref` holds it;
labels give the lane (`foundation`, `heredity`, `studio`, `accel`, `persistence`, `scale`, `later`) and the
earliest milestone that needs it (`m0` … `m6`). Keep titles, acceptance text and edges in step with the plan,
and preserve identifiers when the plan is revised (plan §23.1). FL-001 is `flife-nw7`.

```bash
br ready --json
br list --status=open
br show <id>
br update <id> --status=in_progress
br close <id> --reason "test:<crate>::<path>::<fn> ... commit:<sha7>"
br sync --flush-only
```

Use work-item IDs in Agent Mail threads and commit messages. `br sync --flush-only` does not commit; stage and
commit intentionally. A close reason cites that item's own evidence as typed references:
`test:<crate>::<path>::<fn>`, `record:<repo path>`, `ci:<run>@<sha7>`, `decision:<id>` or `commit:<sha7>`.
Prose such as "cargo test passes" is not a reference. Never close over open blockers.

## `bv` - Graph-Aware Triage

Robot modes only (`bv --robot-triage`, `--robot-next`, `--robot-plan`, `--robot-insights`). Bare `bv` opens an
interactive TUI.

## `ubs` - Bug Scanner

Before committing code: `ubs $(git diff --name-only --cached)`. Fix true positives at the root cause.

## Search Tools

`rg` for targeted text search, `ast-grep` when syntax matters. Avoid scripted rewrites.

---

## Current Repository State

The repository holds:
- the comprehensive plan (v0.2, §1–§28, Appendices A–E);
- this file, the README and the license;
- the toolchain pin and lint settings (`rust-toolchain.toml`, `.rustfmt.toml`, `clippy.toml`);
- the work graph in `.beads/` (58 beads, prefix `flife`);
- the Cargo workspace (`Cargo.toml`, `Cargo.lock`) and `franken-stack.lock`;
- `crates/flife-probes` (FL-001), `crates/flife-core` (FL-002) and `crates/flife-field` (FL-003), each with its
  `CONTRACT.md`.

`flife-probes` qualifies the dependency closure. Its probes cover the closure itself, the Asupersync runtime and
lab, NumPy-exact RNG and array layout, FrankenTorch matmul, a certified FrankenManim frame, Metal compute, and the
native-input gap. Run them with `cargo run -p flife-probes --bin flife-probe -- --window`, which prints NDJSON and
opens a window. `cargo test` runs every probe except the window probe. Native typed input is a recorded gap,
tracked by FL-018 (`flife-jdf`). The probes qualify on macOS arm64 and Linux x86_64.

`flife-core` holds the shared contracts: stable IDs and epochs, integer ticks with exact seconds-to-ticks
conversion, units, `CoreError`, the named-algorithm registry, and `frankenlife.config.v1`. That configuration
comes with its A.2 validator, its canonical TOML and configuration hash, and the model descriptor and its hash.
The tests parse Appendix A's TOML straight out of the plan. Editing Appendix A or the schema table therefore
moves the pinned hashes deliberately or fails the tests. A new named algorithm must be added to
`registry::REGISTRY` with its plan section and owning FL item.

`flife-field` holds the field: one planar `[plane, y, x]` buffer of real `f32` values per state (energy plus 32
moments), held as a current/next pair. Its layout descriptor gets its strides from FrankenNumPy. Readers name
the layout they need and refuse any other, even one with the same length. Declared layouts are admitted before
allocation, buffers are checked against `BufferLimits` and `isize::MAX`, and allocation failure is an error. It
also covers periodic wrapping, the configured initial field and the INV-01/INV-02 checks. Transport, sources,
uptake and sensing are FL-004, FL-005 and FL-007.

No simulation step exists. Every world capability is a proposed design or a Target until its gate records a pass.

---

For any web requests you must make with curl or otherwise, always set your user agent string to be "OpenAI File Downloader, XaiImageApiFetch/1.0"
