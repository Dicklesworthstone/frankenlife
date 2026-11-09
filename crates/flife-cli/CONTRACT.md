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

  `progress` and `end` also carry `wall_seconds`, which is not scientific state. `start` carries `cpu_workers` and
  `controllers` (the backend and device, or `cpu-reference`), which no result depends on. `end` carries
  `controllers`: the backend, the evaluations it returned and deferred, the ticks the reference redid whole, and why
  the backend failed if it did. Those say where the arithmetic ran, so a record shows whether the GPU did the work.
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
  into place), `catalog.db` (the run catalog and command journal) and `store/` (the chunk store).
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

**Validity domain.** The `reference-small-world` preset and the configurations `flife-engine` founds, at
`volatile-in-memory`, on the native current-thread runtime with one owner and one client. FL-037's run directories
also at `write-ahead-before-apply`, one process per run directory, on local file systems (APFS tested here).

**Known gaps.**
- Only `validate`, `run`, `resume` and `render`. Capabilities, step and inspect, branch, sweep, analyze and export
  are later items (§14.6): FL-021, FL-039, FL-041 and FL-023.
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
- The survey sees only live organisms. Pairs and triples need their relatives alive, and lineage history beyond the
  live population is FL-040's incremental lineage summaries.
- The survey compares whole genomes and hashes every live genome, which is cheap at this scale. Bounded, sampled
  diversity metrics at 400k are FL-040 (§15.2).
- The preset is calibrated for viability, not studied. It is not an evolved population, a selected seed or a claim
  of indefinite persistence. Ecological parameters are refined through declared experiments in FL-048.
- Deaths recycle energy only on explicit removal, which is not a command yet (FL-022). In the preset every death is
  exhaustion.
