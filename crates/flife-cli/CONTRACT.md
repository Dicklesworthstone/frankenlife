# flife-cli contract

**Purpose.** FL-015 (`flife-iw0`) adds the first real small-world preset and a smoke run that observes births,
deaths, emissions and heritable variation in raw state (plan §7.2, §15.1, §15.2, §19.5, §22.2, Appendix A). The
preset is `presets/reference-small-world.toml`. The crate is the `flife` command line's first two commands,
`validate` and `run`, and the library behind them:
- `run` is a headless run through the FL-014 session owner;
- `observe` holds a tally of the published reports and a survey of a committed world's raw state.

FL-037 (`flife-e5u`) adds run directories and `flife resume` (plan G3, §12.3, §16.3–§16.7, C.13, C.14), in
`persist`: a run kept in one publishes checkpoints to FL-035's chunk store, indexes them in FL-036's run catalog,
journals its commands there under `write-ahead-before-apply`, and resumes from its newest restorable checkpoint to
exactly the state of the uninterrupted run.

FL-039 (`flife-05q`) adds `flife branch` (plan §14.4, §15.3, §16.3, §16.7, B.1), in `branch`. It makes a new run
directory that starts from a durable checkpoint of another run directory, as a new session. A control branch has no
commands of its own; an intervention branch schedules its own. Each branch shares the parent's immutable chunk files
and keeps its own command history, and runs on with `flife resume` independently of its parent and its siblings.

FL-040 (`flife-aex`) adds incremental metrics and lineage summaries (plan §15.2, §7.7, §16.8), in `metrics`. A run
given `--metrics-every M` folds every published report into a `Tracker` and emits a `metrics` record at every
multiple of M and at the end. Each record separates what is complete, folded from every tick, from what is sampled
from the render epoch at the record's tick, and the start record declares both rules.

**Inputs.**
- A configuration or preset as TOML, validated by `flife_core::ValidatedConfig`.
- `flife validate <config>`.
- `flife run <config> --ticks N --every K --cpu-workers W --controllers C`, with N = 1,024, K = 128, W from the
  configuration and C = `auto` by default. C is `cpu` (the reference alone), `metal` (refused without a Metal device)
  or `auto` (Metal where the host has it and W ≤ `AUTO_METAL_MAX_WORKERS` = 8, otherwise the reference). `flife
  render` takes the same `--controllers`.
- In the library, `run(config, RunOptions { ticks, every, cpu_workers, controllers }, emit)`. `emit` receives every
  record in order.
- FL-037, on `flife run`:
  - `--run-dir DIR`: a new or empty directory to keep the run in;
  - `--checkpoint-every T`: a checkpoint at every multiple of tick T, the starting tick included, and at the
    end; 0 for the end alone. The default is `[persistence] checkpoint_interval_simulation_seconds` in ticks,
    rounded up;
  - `--checkpoint-durability power-loss|process-crash`: FL-035's profiles, `power-loss` by default;
  - `--inject TICK:X,Y:AMOUNT`, repeatable: an `InjectEnergy` command at exactly that committed tick, as commands
    1, 2, … in order, each required to be admitted afresh.
- `flife resume DIR --until T --every K --cpu-workers W --controllers C`, with the same two checkpoint flags.
- In the library: `run_with(config, options, injections, persistence, emit)` and `resume(dir, ResumeOptions,
  emit)`, with `Persistence::new(dir, config)` giving the configured defaults.
- FL-040: `--metrics-every M` on `flife run` and `flife resume`, 0 (none) by default; `RunOptions::metrics_every`
  and `ResumeOptions::metrics_every` in the library. `Tracker::new(world, M)`, `Tracker::record(report)` and
  `Tracker::sample(epoch)` are public, so a client that steps a world itself can use them.
- FL-039: `flife branch PARENT DIR --at T --name NAME --inject TICK:X,Y:AMOUNT --checkpoint-durability D`.
  - `PARENT` is a run directory that no process holds; `DIR` is new or empty.
  - `--at` is the committed tick of a parent checkpoint. By default it is the newest one that restores.
  - `--name` is 1 to 256 bytes, `DIR`'s name by default.
  - `--inject` gives the branch's own brushes, at or after T, numbered after every inherited command.
  - In the library: `branch(parent, dir, &BranchOptions { at, name, injections, durability })`.

**Outputs.** Every line on standard output is one JSON record.
- `frankenlife.validate.v1`: name, model, configuration hash, seed, Δt, grid, founders, population ceiling, default
  parameters, the founders' and the worst admitted genome bytes, whether the worst case fits the logical quota,
  the cooldown in ticks, and the command durability.
- `frankenlife.run.v1` records, in order:
  - `start`: name, hash, seed, founders, grid, Δt and the requested ticks;
  - one `progress` every K published ticks: tick, live count and the tally so far;
  - `end`: why the run stopped (`reached`, `capacity-paused` or `failed` with the error's kind), the committed tick
    and world digest, the whole tally, and the survey of the committed world.
  - FL-037, for a run kept in a run directory:
    - `checkpoint`: the committed tick and epoch, the chunk-store sequence, the container's SHA-256 and size,
      chunks written and shared, and the sequences retired under `retain_recent_checkpoints`;
    - `resume` (after `start`, from `flife resume`): the checkpoint recovered (sequence and tick, or null for the
      founding world), every newer one passed over with its error kind and reason, the committed tick and digest
      after replaying the journal, and the journal's durable watermarks;
    - `end.persisted`: the run directory, the command durability, the newest checkpoint, and the journal's final
      durable watermarks.
  - FL-039, `branch` (the only record of `flife branch`):
    - the branch's name and session;
    - the parent: run directory, session, and the checkpoint's sequence, tick and SHA-256;
    - the lineage, one fork per ancestor, oldest first: the session forked from, the admissions it had made, the
      tick, and the checkpoint's SHA-256;
    - what was inherited: admissions, outcomes, and commands still pending;
    - the branch's own command identities;
    - its founding checkpoint: sequence and tick, SHA-256 and size, and the chunks written and linked;
    - the world's digest, which is the parent's at that tick.
  - FL-040, with `--metrics-every M`:
    - `start.metrics` declares `frankenlife.metrics.v1`: M, the complete and sampled rules in words, the bin grid
      (16 a side) and the lineages listed (8);
    - one `metrics` record at every multiple of M and, when ticks remain, at the end (a capacity pause or a failed
      tick included), so the windows partition the run. Each holds:
      - `window`, complete, the ticks since the previous record: a `Tally` of their reports (births, deaths by
        stage, mutation acceptance, emission, the live range), viable attempts, every flow and every transfer's
        credited total by canonical name, `I`, `O` and `D`, the holdings when it closed, uptake (energy requested and
        granted, contended and drained cell-ticks, the most packets on one cell), lifespans of the organisms that
        died (count, total, shortest, longest, and a histogram by power of two), and lineages that went extinct;
      - `population`, complete, at the record's tick: live, the capacity and occupancy, the next identity, the
        highest and mean generation, and live counts by topology (the architecture distribution);
      - `lineages`, complete: lineages alive and extinct, the Shannon entropy of the live lineage shares (nats) and
        its exponential, the inverse Simpson index, the largest share, and the 8 largest lineages with their live
        count, births, deaths and deepest generation;
      - `sampled`, from the render epoch (the epoch's identity and tick, and the lag behind the record's tick):
        motility (mean and top speed, the speed limit, the moving fraction), dispersion of organisms over a 16 × 16
        torus grid (occupied fraction, variance-to-mean ratio, Morisita's index, Moran's I with rook neighbours), and
        the energy-density texture (texels and block, mean and top density, Gini, top-decile share, Moran's I).

  `progress` and `end` also carry `wall_seconds`, which is not scientific state. `start` carries `cpu_workers` and
  `controllers` (the backend and device, or `cpu-reference`), which no result depends on. `end` carries
  `controllers`: the backend, the evaluations it returned and deferred, the ticks the reference redid whole, and why
  the backend failed if it did. Those say where the arithmetic ran, so a record shows whether the GPU did the work.
  `metrics` carries `window.record_seconds` and `sample_seconds`, the tracker's own wall-clock cost, which are not
  scientific state either.
- `flife render` (`frankenlife.render.v1`): a `frame` record per PNG written (frame index, tick, live count, path,
  render seconds) at the founding tick and every `--every` ticks, then `end` (frames, tick, the committed digest,
  the video path if encoded, the controller summary, wall seconds, and the style: `mapping`, its `meaning`, `look`,
  `particles` and `field`). FL-019's style flags choose what is drawn: `--color M` (`lineage`, `energy`,
  `generation`, `motion` or `topology`, or a registered name such as `founder-lineage-v1`; by default the
  configuration's `[presentation] color_mapping`), `--look glow|raw`, `--density` and `--no-field`. No style
  changes the world. Frames are the Studio's `WorldView` rendered on FrankenManim's CPU 3D
  route. Each frame's PNG is encoded on its own thread, at most four at once, while the world steps and the next
  frame draws; the bytes are those of encoding them in order, and a failed encode or write fails the render with
  its error. `--video` encodes `frame-%06d.png` to H.264 with `ffmpeg` after the last frame is written.
- `Tally`:
  - published ticks;
  - births, rejected births, and deaths by the budget stage that exhausted them;
  - emitter-ticks, emission energy paid and deposited, child endowment and recycled energy, from each tick's closed
    ledger;
  - the births' perturbed and replaced parameters, topology changes and rejected candidates;
  - the peak and least live counts, and the run ledger's gated residual.
- `Survey`, read from the committed world alone:
  - live founders and descendants, live counts by generation, and surviving founder lineages;
  - organism identities issued, live counts by topology, and distinct genomes: whole contents, `(topology,
    parameters)` sets and trait vectors, each counted separately;
  - `Heredity`. Live parent–child pairs are compared bit for bit over parameters and by distance over the trait
    vector (affinity over `affinity_norm_limit`, motion response over `motion_response_norm_limit`, and ln sensing
    scale: dimensionless, so it compares across motility gains), against consecutive live founders as the unrelated
    baseline. Live grandparent–parent–child triples report how many of the parent's parameter variants the child
    carries, and the child's trait distance to its parent and to its grandparent.
- Errors: one `frankenlife.error.v1` record on standard error. Its `kind` is `CoreError::kind`, or `io` for an
  unreadable file. Exit status is 0 on success, 1 when a run stops early (a capacity pause or a failed tick), and 2
  for an unreadable or invalid configuration, a refused session or a refused run directory.
- A run directory (FL-037) holds `config.toml` (the configuration's canonical TOML, written, flushed and renamed
  into place), `catalog.db` (the run catalog and command journal) and `store/` (the chunk store). A branch's catalog also records
  its lineage and its origin: the name, the parent run, the parent checkpoint's sequence and the creation time.
- In `flife-core`, FL-015 adds `CoreError::kind`, a stable kebab-case class name, distinct for every variant.

**Invariants.**
- The run is a client of the session. The owner task holds the world and is the only commit authority. The client
  steps it one tick at a time over the control lane, closes it, and surveys the world the owner returns. Nothing
  reads or writes the world outside the session.
- A run's scientific content (every field but `wall_seconds`) depends only on the configuration and the tick count.
  The preset's committed world after 1,024 ticks has one digest. It is the same in optimized test builds with debug
  assertions and in release builds, and is pinned by the smoke test.
- The survey agrees with the reports:
  - identities issued = founders + births;
  - live = founders + births − deaths;
  - re-surveying the returned world gives the same survey.
- The session's limits are checked before the world is founded. An unadmitted durability is refused without
  founding the population or emitting a `start` record.
- The preset is Appendix A with exactly its declared changes. Its header declares each one, and the smoke test
  refuses any other difference.
- FL-037:
  - Checkpoints are `pause-and-stream`: the client takes no step while one is taken and published, and the session
    takes one only once every journal record has its receipt, so the catalog's index is written while the journal
    is idle. Keeping a run changes nothing in it: a `volatile-in-memory` run ends at the same digest with or
    without a run directory (tested), and a `write-ahead-before-apply` one at the same digest with checkpoints
    every 7 ticks or only at the end (observed when FL-037 was written).
  - Commands are admitted before the founding checkpoint is taken, so it holds them: a `volatile-in-memory`
    run's scheduled commands survive a crash through its checkpoints.
  - A kept run suspends its session at the end instead of closing it: commands still pending stay pending in the
    final checkpoint and the journal alike, and apply when the run is resumed past their ticks.
  - Resume takes the newest checkpoint that loads and restores to a world of the run's configuration hash, falling
    back to the founding world, and reports every newer one it passed over. `write-ahead-before-apply` then
    replays every journaled outcome past it, each reproduced exactly (FL-036). The resumed run's committed digest
    and final checkpoint bytes (world and command closure) equal the uninterrupted run's.
  - After each publication the store keeps the newest `retain_recent_checkpoints` that restore; the catalog's
    index keeps every row, and only a manifest makes a checkpoint resumable (§16.6).
  - A resumed run's tally covers the ticks that call published.
- FL-039:
  - A branch is a new session, `branch_session(SHA-256 of the parent checkpoint, name)`. Making the same branch again
    gives the same session and the same founding checkpoint bytes; siblings with different names differ (tested).
    A session already in the lineage is refused.
  - The founding checkpoint is the parent checkpoint's world with the branch's command book: every inherited command
    and outcome, the lineage, and the branch's own commands. Its ten world chunks are byte-identical to the
    parent's. The branch's store links the parent's verified chunk files instead of writing them (FL-035's
    `publish_sharing`), and writes only the command closure. A parent chunk that does not verify is written.
  - Command histories are independent. A branch admits only its own session's commands. Its
    `write-ahead-before-apply` journal starts with the history it inherited and then holds its own commands. The
    parent's journal never holds a branch's command, and a branch's never holds a later command of its parent.
  - A control branch continues exactly as its parent would have: the same world digests and the same outcomes. An
    intervention branch matches its control until its first command applies.
  - Resuming a branch takes the newest checkpoint of its own session that restores. Checkpoints of another session
    are passed over, and a branch has no founding world to fall back to.
- FL-040:
  - The tracker never reads the world after its start. It starts from the committed population (each organism's
    identity, genome, lineage, generation, birth tick and topology) and then folds each published report: births
    first (a child takes its parent's lineage, the next generation, and its parent's topology or the one its
    mutation changed to), then deaths. Per tick its work is proportional to that tick's births and deaths, plus a
    constant for the window's sums.
  - Identities are checked as they are folded. A report must advance from the tracker's tick. A child's parent must
    be alive, and its identity must be above every one issued before it. A topology change must start at the
    parent's topology. A death must be of a live organism with the genome, lineage and generation the tracker holds,
    and the live count must equal the report's. The first disagreement stops the run with `state-conflict`; nothing
    wrong is summarized.
  - A sample is checked against its epoch. At the tracker's tick the epoch must list the same identities in order,
    with the same lineages, generations and topologies. An epoch ahead of the tracker is refused. One behind it is
    summarized with its lag, unchecked. A session publishes each tick's epoch before the client hears of the tick,
    so in a run the lag is 0 unless every render slot is leased.
  - The sample costs O(live + 16² + texels · log texels), at most once per M ticks, and is timed in the record. It
    runs on the client between steps, so it delays the next step by that much and no more.
  - Sums run in tick, identity, lineage or cell order, and the entropy uses `flife_core::numerics`, so the records'
    scientific content is identical on every qualified host. Tracking changes nothing in the world: a run with
    metrics ends at the digest of one without (tested).
  - A resumed `write-ahead-before-apply` session replays its journal before the client sees a tick. When the
    tracker is behind the session's starting tick, it starts again from a checkpoint the session takes there.
    Lineage births and deaths count from the tracker's start, so they cover the ticks this call publishes, like the
    tally.
  - The parent's chunk store is opened as its writer for the whole branch operation, so nothing moves under the
    checkpoint while it is read and linked.

**The preset.** `reference-small-world` is a viability-calibrated start with declared parameter tuning (§7.2) at
the Reference profile's scale (§19.5):
- 4,096 founders on a 128 × 128 periodic field, with a population ceiling of 16,384;
- F32 controllers evaluated every tick at Δt = 1/128 s;
- seed 42.

Versus Appendix A it changes:

| Key | Appendix A | Preset |
|---|---|---|
| `chemistry.uptake_capacity_per_second` | 0.4 | 6.0 |
| `sources.power_per_source` | 80 | 960 |
| `costs.basal_energy_per_second` | 0.005 | 0.8 |
| `reproduction.minimum_parent_energy_before_attempt` | 4.0 | 3.0 |
| `reproduction.cooldown_seconds` | 1.0 | 0.5 |

It also changes the name, and `command_durability` from `write-ahead-before-apply` to `volatile-in-memory`. The
durability change is a capability: `flife run` without a run directory opens no command journal, so it refuses
`write-ahead-before-apply`; with `--run-dir` it opens FL-036's journal there (FL-037).

Appendix A as written has no birth and no death in its first 1,024 ticks. Its founders start at 2.0, below the 4.0
threshold, take at most 0.4 energy per second, and emit from the first tick. The preset keeps their 2.0 start, so
every birth comes from an organism that fed.

Over 8,192 ticks, with seeds 42, 7, 99 and 1234:
- unfed founders starved, and the population fell to 470–596 within 1,024 ticks;
- it recovered to 1,430–1,669, then ended at 857–1,142 live;
- there were 3,122–3,431 births;
- 184–200 lineages survived, at most 13–16 generations deep, and the four other catalog topologies arose;
- no run paused, and every ledger closed within 6e-7.

These figures come from `flife run` in release builds when the preset was introduced. They are a record, not a
test.

**Refusal cases.**
- An unreadable file: `io`.
- An invalid configuration, such as an unknown key: `invalid-config`.
- `write-ahead-before-apply` without a run directory: `missing-capability`, before anything is founded.
- FL-037, each before anything runs or is printed on standard output: `--run-dir` naming a directory that already
  holds a run, or any other files: `state-conflict`; `flife resume` of a directory without `config.toml` and
  `catalog.db`, of a run whose `config.toml` no longer hashes to the catalog's configuration, of a run already past
  `--until`, or of a run another process holds (the chunk store's writer lock): `state-conflict`; a journal that
  disagrees with the checkpoint: FL-036's `state-conflict` refusals.
- FL-039, `flife branch`, all `state-conflict` with nothing printed on standard output:
  - a parent that is not a run directory, or that another process holds;
  - a parent with no checkpoint that restores (at `--at`, when given);
  - a name that is empty or longer than 256 bytes;
  - a command due before the checkpoint's tick;
  - a target that holds a run or any other files.

  The first four are refused before the branch directory is made. `flife resume` of a branch none of whose
  checkpoints restores is also refused.
- FL-040: a report or an epoch that disagrees with the tracker stops the run with `state-conflict` (operation
  `track run metrics`), exit status 2. The records before it are still emitted.
- Any other refusal from founding or the session, by its `CoreError` kind.
- A run that pauses on capacity or fails a tick ends with that `stop` and exit status 1. Its records up to that
  point are still emitted.

**Independent tests.** `tests/smoke.rs` (5 tests):
- `the_preset_is_appendix_a_with_exactly_its_declared_changes`. It parses both TOML documents leaf by leaf, from
  the plan's own Appendix A. It refuses any difference except the eight declared, and checks that the header
  declares each one.
- `the_preset_validates_at_the_reference_scale_and_its_session_admits_it`. Appendix A as written validates, and
  `flife run` without a run directory refuses it as `missing-capability`, naming the command journal it needs.
- `the_smoke_run_shows_births_deaths_emissions_and_heritable_variation_in_raw_state`. It runs 1,024 ticks of the
  preset through the session on the native runtime.
  - The records: one start, progress at 256, 512, 768 and 1,024, and one end carrying the returned tally and
    survey.
  - Births, deaths and emissions occurred. The deposits are at most 0.9 of the payments, and the endowment is
    positive. Every death is exhaustion, which recycles nothing.
  - The survey agrees with the reports, and at least three generations are alive. Every live genome is distinct,
    in its parameters and in its traits separately. The ledger closes below 1e-5.
  - At least 100 live pairs: every child differs from its parent and keeps over 99% of its parameters, while
    founders share under 5%. A child's trait distance to its parent is under a tenth of the founders'.
  - At least 20 live triples: the child carries over 99% of its parent's variants, and is closer to its parent
    than to its grandparent.
  - The digest is pinned, and so are the whole tally and survey, every float included. The survey uses
    `flife_core::numerics::ln`, so it is identical on every host.

  Observed at seed 42: 507 births and 3,775 deaths (3,613 starved in maintenance, 159 poisoned in chemistry, 3
  exhausted by motion). Paid 1,009 energy and deposited 908. Kept 99.79% of parameters, while founders share
  1.3%. Trait distance 0.058 to the parent, 1.99 between founders. 219 triples carried 99.72% of their parents'
  variants. Trait distance 0.058 to the parent and 0.081 to the grandparent, a ratio near √2, as accumulated
  heritable perturbations predict. (Before the motility change, Appendix A's gain gave 359 births and 3,753
  deaths, and 0.081 and 0.113.)
- `the_flife_binary_validates_runs_and_refuses_with_typed_errors`. It runs the built `flife`:
  - `validate` prints the preset's hash;
  - `run --ticks 2 --every 1` prints start, progress, progress, end;
  - an unreadable file is `io` and an unknown key is `invalid-config`, each with exit status 2;
  - Appendix A validates, and `run` refuses it as `missing-capability` with no output.
- `a_capacity_pause_ends_the_run_with_its_stop_and_status_one`. With the population ceiling at the founders' 4,096,
  the first viable birth has no slot under `pause-before-commit`. This needs the first birth before the first
  death, so the test keeps Appendix A's motility gain: at the preset's, a poisoning death comes first and frees a
  slot. The run ends `capacity-paused` before tick 1,024,
  with no birth published, and `flife` exits with status 1.

`tests/render.rs` (2 tests):
- `rendering_writes_png_frames_and_leaves_the_world_as_a_plain_run_leaves_it`. Six ticks at three world threads
  write PNG frames at ticks 0, 3 and 6 with the requested size in their headers, and the end digest is a plain
  one-thread run's. The same run rendered by energy, raw and as density ends at the same digest,
  names that style in its `end`, and writes a different frame.
- `the_render_colour_is_the_configured_mapping_unless_the_flag_overrides_it`. The `flife` binary renders a preset
  configured for `energy-lightness-v1` by energy; `--color motion` and `--color motion-heading-v1` both override it;
  a configuration naming an unregistered mapping is refused as `invalid-config` before anything renders.

The test profile is optimized (`[profile.test] opt-level = 3`, with debug assertions and overflow checks on), so
the smoke run takes about 30 s. Rust floating point has no fast-math and no implicit contraction, so optimization
changes no result. FL-013's digest, pinned before the change, still holds under it.

Twenty-two planted bugs were each caught by at least one of these tests or `flife-core`'s `errors_name_their_class`
(listed in the FL-015 commit).

`tests/resume.rs` (5 tests, FL-037, about 11 s), each on a 32 × 32 Appendix A ecology of 48 founders that breeds,
controllers every 4 ticks in per-organism phases, checkpoints every 7 ticks, energy brushes at ticks 3, 66, 150
and 230 on cells whose X and Y differ, and a 240-tick horizon whose uninterrupted run is the reference (it
publishes the founding checkpoint, every multiple of 7 and 240, keeps 4, journals 4 acceptances and 4 outcomes,
and its catalog indexes exactly the checkpoints it published):
- `a_run_resumes_from_its_newest_restorable_checkpoint_to_the_uninterrupted_state`. A run to tick 152 journals 3
  outcomes and no cancellation. Its checkpoint at 147 holds controller outputs evaluated before 147, a cooldown
  running past it, newborn genomes, command 2 applied and commands 3 and 4 pending. With the 152 manifest damaged,
  `flife resume --until 240` reports 152 passed over, recovers 147, replays command 3's outcome to tick 150 or
  later, and ends at the reference digest and final checkpoint SHA-256, with 4 outcomes journaled.
- `a_killed_write_ahead_run_resumes_to_the_uninterrupted_state`. A run with no horizon is SIGKILLed after
  reporting tick 70 (exit by signal 9, no `end`); resume recovers a checkpoint at 63 or later, publishes nothing
  at or before it, and reaches the reference. Over 10 consecutive runs of the final binary it always recovered
  63; 6 times the journal held command 2's outcome (replayed to 67), 4 times only its acceptance (applied again
  at 66), so both recovery paths ran.
- `a_killed_volatile_run_resumes_to_the_uninterrupted_state`. The same at `volatile-in-memory` under `power-loss`
  publication: no journal, and the end digest equals the same configuration run without a run directory.
- `checkpoints_of_another_configuration_are_passed_over`. With another configuration's store moved in, resume
  passes over all three of its checkpoints by name and replays the journal from the founding world to the digest
  of the same run made directly.
- `run_directories_refuse_what_they_cannot_continue`: an existing run, a non-empty directory, a non-run
  directory, an edited `config.toml`, a tick already passed, and a run directory another live process holds; and
  the configured interval (3,840 ticks), giving only the founding and final checkpoints of a 10-tick run.

A `persist` unit test pins `--inject` parsing. Twenty-four planted mutants in the run directories, the driver,
suspension, `open_restored` and the CLI guard were each caught. One, `flife run` without its own write-ahead
guard, was caught only after the smoke test checked that the refusal names `--run-dir`: the session refuses in
the same words, but after founding the world.

`tests/branch.rs` (3 tests, FL-039, about 11 s) uses FL-037's ecology, injections and horizon. A
`write-ahead-before-apply` parent runs to tick 120 and is branched at its checkpoint at 112, which holds commands 1
and 2 applied and 3 and 4 pending:
- `a_control_branch_continues_as_its_parent_while_an_intervention_branch_diverges`.
  - Branching: a control branch and an intervention branch with a brush at tick 130 (its command 5). Each
    inherits 4 admissions, 2 outcomes and 2 pending commands, writes 1 chunk and links 10, which are the same
    inodes as the parent's files. Their founding SHA-256 differs from the parent checkpoint's.
  - The parent alone: it resumes to 240 at the uninterrupted run's digest and journal. Its retention removes its
    own names for chunks the branches share, and the control's founding checkpoint still loads.
  - Paired runs: the control and the intervention are identical at 129. A branch of the control at 129 carries
    both forks and links the control's 10 chunks.
  - At 240: the control and its branch reach the uninterrupted digest, and the intervention differs. The control's
    journal equals the uninterrupted run's. The intervention's holds the same four inherited acceptances, then its
    brush (order 4, effective at 130) and its outcome.
  - The catalog: the nested branch records both forks and its origin.
  - Admission after a restore: the intervention's checkpoint holds its brush accepted write-ahead. It refuses a
    control-session command (`WrongSession`) and a reuse of identity 3 (`ConflictingReuse`), and admits its own
    command 6.
- `a_volatile_branch_shares_its_parents_chunks_and_journals_nothing`. The same sharing and control trajectory at
  `volatile-in-memory`. The branch is named after its directory, its session is `branch_session` of the parent
  checkpoint and that name, and its catalog holds the lineage but no journal.
- `branching_refuses_what_it_cannot_branch`, covering:
  - every refusal above, each with exit status 2 and nothing made;
  - the same branch made twice, and a sibling with another name;
  - the default `--at`;
  - a branch whose only manifest is damaged, which refuses to resume while its parent still resumes;
  - another session's checkpoint copied into a branch's store as sequence 9, which resume passes over, naming the
    session.

`flife-runtime` and `flife-store` each have their own `tests/branch.rs`, for the command book's lineage and for the
linked publication and branch catalog. Thirty-four planted mutants were run against the three `tests/branch.rs`
and `tests/resume.rs`: in the book's lineage, its encoding and checks, chunk linking, the branch catalog, and
`flife branch` and resume. A first pass caught 30. The four survivors (a repeated session, a fork past the
admissions, and forks out of order, each accepted on restore, and a restored record's session left unchecked) are
reachable only through a crafted checkpoint. `flife-runtime`'s `a_crafted_lineage_is_refused_on_restore` now
catches each of them.

`tests/metrics.rs` (6 tests, FL-040, about 14 s):
- `the_tracker_holds_the_committed_world_at_every_tick`. FL-037's 48-founder ecology with architecture mutation at
  a quarter of births, stepped directly for 1,200 ticks, and the preset's first 320 ticks. The tracker is fed only
  the reports and sampled every tick.
  - At every tick its organisms (identity, genome, lineage, generation, birth tick, topology) equal the committed
    world's, and so do its population figures and its lineage figures. The lineages' births, deaths and deepest
    generation are recounted from the world and the reports' identities.
  - Each one-tick window equals that report: tally, flows, transfers, `I`/`O`/`D`, holdings and uptake. Its lifespans
    equal the deaths' ticks minus the birth ticks the world held before the step.
  - A second tracker sampled every 64 ticks has windows equal to sums over those reports in tick order, and the same
    population, lineage and sampled figures.
  - Every 160 ticks the sampled figures equal brute-force ones: speeds by `sqrt`, bins counted naively, Gini by the
    Lorenz curve, and Moran's I from an explicit list of neighbour pairs.
  - In the ecology, children change topology (16 times), organisms born in the run die (30) and lineages go extinct
    (39). In the preset, founders die by the thousand.
- `the_spatial_statistics_are_the_textbook_ones`. A checkerboard's Moran's I is −1, stripes' 0 and a constant's
  undefined. One unit among 50 has Gini 49/50 and all of the top decile. Gini and Moran's I equal the
  mean-absolute-difference and neighbour-pair formulas on pseudo-random grids. A clump in one bin has Morisita Q and
  an even spread 0, and a position at the extent wraps to the first bin.
- `a_run_declares_its_metrics_and_its_windows_partition_it`. 400 ticks with M = 96 give records at 96, 192, 288, 384
  and 400. The windows abut, their births, deaths, rejected births and lifespans add up to the end tally, every
  sample's lag is 0, and the last record agrees with the end survey. Without metrics the start record has no
  `metrics` key, no metrics records are emitted, and the digest is the same.
- `a_resumed_run_tracks_from_the_state_its_journal_replayed_to`. FL-037's damaged-newest case: resumed from tick
  147, the journal replays to 150. The tracker starts there, and every record after it agrees with an uninterrupted
  run's: population, lineage shares, sampled figures, and every window that starts after the resume.
- `the_tracker_refuses_reports_that_break_identity`. Real reports tampered one field at a time: a report from
  another tick, an unknown parent, a child given the identity of a live organism or of a dead one, a topology change
  from another topology, an unknown death, a death under another genome, lineage or generation, and another live
  count. Each is refused by the check meant for it, named in the error; the untampered report is then followed.
- `a_sample_refuses_an_epoch_of_another_world`. Refused: the preset's founders against the ecology's (by count); a
  seed-7 world at the first tick where it holds as many organisms as seed 42's but not the same ones (row by row);
  and an epoch one tick ahead, each by the check meant for it. An epoch behind is summarized with its lag.

Twenty-six planted mutants were run against `tests/metrics.rs`: in the tracker's births, deaths, identity checks,
lineage and window sums, lifespan buckets, the epoch check, the statistics, and the run's re-founding and final
window. A first pass caught 25. The survivor (the check that a child's identity is above every one issued, disabled)
was not equivalent: the test had reused a live organism's identity, which the live count also refuses. The test now
also reuses a dead organism's identity, which only that check refuses, and requires each refusal to name its check.
The survivor and the five other mutants the refusal tests catch were run again, and all six were caught.

**Validity domain.** The `reference-small-world` preset and the configurations `flife-engine` founds, at
`volatile-in-memory`, on the native current-thread runtime with one owner and one client. FL-037's run directories
also at `write-ahead-before-apply`, one process per run directory, on local file systems (APFS tested here).

**Known gaps.**
- Only `validate`, `run`, `resume`, `render` and `branch`. Capabilities, step and inspect, sweep, analyze and export
  are later items (§14.6): FL-021, FL-041 and FL-023.
- FL-039 gaps:
  - §14.4's synchronized simulation-time views and paired metrics are not here: a branch's paired comparison is two
    `flife resume` runs and their records. Experiment reports are FL-041, and side-by-side views are the studio's.
    Nothing assumes common random numbers stay coupled after the branches' births differ.
  - A branch runs the parent's configuration. Changing parameters at a fork (an intervention that is not a
    command) is not supported.
  - Sharing is hard links, so the parent and the branch must be on one file system. Across file systems the
    store falls back to writing each chunk. That path is shared with damaged parent chunks, which are tested, but a
    cross-file-system branch is not.
  - A linked chunk is one file under several names. Damage in place, such as a bit flip, therefore reaches every
    store that links it. Each store verifies a chunk's SHA-256 on load, so the damage is detected but not repaired.
    Retention removes only a store's own names.
  - A branch needs its own founding checkpoint, or a later one, to resume. If none restores, it is refused rather
    than re-derived from the parent.
  - The parent must not be running while it is branched (its store's writer lock refuses it). Branching a live
    session at a checkpoint taken by its owner is not supported.
  - `run_identity`'s refusal of a catalog recording more than one run is not tested.
- FL-037 gaps:
  - The checkpoint container is encoded whole in memory before it is published (one extra copy of the committed
    state). §16.5's low-memory bound on it, and bounded or background checkpoints, are FL-038.
  - Each publication restores the retained checkpoints to choose which to keep: O(`retain_recent_checkpoints`)
    restores per checkpoint. Checkpoints of the 48-founder test ecology are about 2 MB each.
  - Power loss is FL-035's protocol (flush order), exercised by publishing under `power-loss`, never by cutting
    power. Kills are tested at whatever point the process reached, not at every step of a checkpoint (FL-035 and
    FL-036 test those).
  - Commands come only from `--inject` at the start of a run. Commands submitted to a resumed run, and the other
    interventions, are FL-022.
  - Retention is by count. Pinned discoveries and journal compaction (§16.7) are later, and the catalog's
    checkpoint index keeps rows for retired manifests.
- `render` draws on FrankenManim's CPU reference 3D route, a tick per frame at most, at the epoch's texel resolution
  for the field. There is no Metal or CUDA offline route, no motion interpolation between ticks, and no audio.
  FrankenManim's Metal 3D route was measured on these frames (`examples/frame_routes.rs`, M5 Ultra, GPU shared
  with other work): 189 ms per 1,280² frame against 48 ms on the CPU route with 36 threads, and 445 against 57 ms at
  2,160²; about 70% of its time is the wait for the GPU. With FrankenManim `dfdc7d18`'s unclipped-triangle path
  both routes prepare faster (CPU 21.5 ms, Metal 163 ms at 1,280², same bytes on each). FrankenManim `f401a947`'s
  per-tile triangle lists then take the Metal route to 28 ms at 1,280² (from 166) and 32–34 ms at 2,160², level with
  the CPU route (23 and 30–41 ms), with the same bytes as before. Its RGBA8 frame differs from the CPU's by one in 0.02% of
  the bytes (332 of 1.6 M pixels at tick 64), so it would also change the written frames. The
  MP4 encoder is an external `ffmpeg` on `PATH`; without it `--video` is a typed `missing-capability` refusal after
  the frames are written.
- `--inject` (FL-037) is the only command source: single-cell energy brushes at fixed ticks. The other
  interventions and recorded commands are FL-022's.
- The survey sees only live organisms. Pairs and triples need their relatives alive. FL-040's records carry each
  lineage's births, deaths, deepest generation and extinction, but no ancestry tree.
- The survey compares whole genomes and hashes every live genome, which is cheap at this scale.
- FL-040 gaps:
  - Metrics are NDJSON records only. §16.6 puts lineage summaries and sampled metrics in the run catalog; no catalog
    table holds them yet. FL-041's complete run catalog is where they would be stored and compared.
  - No genome-content diversity per window. The client sees genome identities, not contents. Every birth perturbs
    the traits, so distinct contents would equal the live count. Sampled trait distances need genome access, which
    only the end survey has (exact, once).
  - The sample runs on the client between steps. It is bounded and timed, but not in the background: a sampler that
    holds the epoch's lease while the session steps on is not built.
  - The sampled figures are of the F32 epoch: positions, velocities, and the field averaged over texel blocks (at
    most 512 a side). They are not the F64 committed state.
  - Measured at 48 and 4,096 organisms (the preset for 1,024 ticks on 16 CPU workers with a record every 128
    ticks: folding took 4 ms in all and a sample 0.7 ms median, 3.2 ms at most; the digest is the pinned one with
    and without metrics; whole runs took 9.15 s against 8.86 s (medians of three interleaved runs each), inside the
    spread of the runs without metrics (8.85–9.78 s) at host load 55–65). 400k is not measured.
  - A resumed run's lineage counts start at its tracker's start. Lineages extinct before it are not reported.
- The preset is calibrated for viability, not studied. It is not an evolved population, a selected seed or a claim
  of indefinite persistence. Ecological parameters are refined through declared experiments in FL-048.
- Deaths recycle energy only on explicit removal, which is not a command yet (FL-022). In the preset every death is
  exhaustion.
