# flife-cli contract

**Purpose.** FL-015 (`flife-iw0`) adds the first real small-world preset and a smoke run that observes births,
deaths, emissions and heritable variation in raw state (plan §7.2, §15.1, §15.2, §19.5, §22.2, Appendix A). The
preset is `presets/reference-small-world.toml`. The crate is the `flife` command line's first two commands,
`validate` and `run`, and the library behind them:
- `run` is a headless run through the FL-014 session owner;
- `observe` holds a tally of the published reports and a survey of a committed world's raw state.

**Inputs.**
- A configuration or preset as TOML, validated by `flife_core::ValidatedConfig`.
- `flife validate <config>`.
- `flife run <config> --ticks N --every K --cpu-workers W --controllers C`, with N = 1,024, K = 128, W from the
  configuration and C = `auto` by default. C is `cpu` (the reference alone), `metal` (refused without a Metal device)
  or `auto` (Metal where the host has it and W ≤ `AUTO_METAL_MAX_WORKERS` = 8, otherwise the reference). `flife
  render` takes the same `--controllers`.
- In the library, `run(config, RunOptions { ticks, every, cpu_workers, controllers }, emit)`. `emit` receives every
  record in order.

**Outputs.** Every line on standard output is one JSON record.
- `frankenlife.validate.v1`: name, model, configuration hash, seed, Δt, grid, founders, population ceiling, default
  parameters, the founders' and the worst admitted genome bytes, whether the worst case fits the logical quota,
  the cooldown in ticks, and the command durability.
- `frankenlife.run.v1` records, in order:
  - `start`: name, hash, seed, founders, grid, Δt and the requested ticks;
  - one `progress` every K published ticks: tick, live count and the tally so far;
  - `end`: why the run stopped (`reached`, `capacity-paused` or `failed` with the error's kind), the committed tick
    and world digest, the whole tally, and the survey of the committed world.

  `progress` and `end` also carry `wall_seconds`, which is not scientific state. `start` carries `cpu_workers` and
  `controllers` (the backend and device, or `cpu-reference`), which no result depends on. `end` carries
  `controllers`: the backend, the evaluations it returned and deferred, the ticks the reference redid whole, and why
  the backend failed if it did. Those say where the arithmetic ran, so a record shows whether the GPU did the work.
- `flife render` (`frankenlife.render.v1`): a `frame` record per PNG written (frame index, tick, live count, path,
  render seconds) at the founding tick and every `--every` ticks, then `end` (frames, tick, the committed digest,
  the video path if encoded, the controller summary, wall seconds). Frames are the Studio's `WorldView` rendered on FrankenManim's CPU 3D
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
  for an unreadable or invalid configuration or a refused session.
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
durability change is a capability: the write-ahead journal is FL-036.

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
- `write-ahead-before-apply`: `missing-capability`, before anything is founded.
- Any other refusal from founding or the session, by its `CoreError` kind.
- A run that pauses on capacity or fails a tick ends with that `stop` and exit status 1. Its records up to that
  point are still emitted.

**Independent tests.** `tests/smoke.rs` (5 tests):
- `the_preset_is_appendix_a_with_exactly_its_declared_changes`. It parses both TOML documents leaf by leaf, from
  the plan's own Appendix A. It refuses any difference except the eight declared, and checks that the header
  declares each one.
- `the_preset_validates_at_the_reference_scale_and_its_session_admits_it`. Appendix A as written validates, and
  its session refuses it as `MissingCapability`.
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

`tests/render.rs` (1 test): `rendering_writes_png_frames_and_leaves_the_world_as_a_plain_run_leaves_it`. Six ticks
at three world threads write PNG frames at ticks 0, 3 and 6 with the requested size in their headers, and the end
digest is a plain one-thread run's.

The test profile is optimized (`[profile.test] opt-level = 3`, with debug assertions and overflow checks on), so
the smoke run takes about 30 s. Rust floating point has no fast-math and no implicit contraction, so optimization
changes no result. FL-013's digest, pinned before the change, still holds under it.

Twenty-two planted bugs were each caught by at least one of these tests or `flife-core`'s `errors_name_their_class`
(listed in the FL-015 commit).

**Validity domain.** The `reference-small-world` preset and the configurations `flife-engine` founds, at
`volatile-in-memory`, on the native current-thread runtime with one owner and one client.

**Known gaps.**
- Only `validate`, `run` and `render`. Capabilities, step and inspect, checkpoint and resume, branch, sweep, analyze
  and export are later items (§14.6): FL-016, FL-021, FL-037, FL-039, FL-041 and FL-023.
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
- The run sends no commands. Interventions from the command line come with FL-022's recorded commands.
- The survey sees only live organisms. Pairs and triples need their relatives alive, and lineage history beyond the
  live population is FL-040's incremental lineage summaries.
- The survey compares whole genomes and hashes every live genome, which is cheap at this scale. Bounded, sampled
  diversity metrics at 400k are FL-040 (§15.2).
- The preset is calibrated for viability, not studied. It is not an evolved population, a selected seed or a claim
  of indefinite persistence. Ecological parameters are refined through declared experiments in FL-048.
- Deaths recycle energy only on explicit removal, which is not a command yet (FL-022). In the preset every death is
  exhaustion.
