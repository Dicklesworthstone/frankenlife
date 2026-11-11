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

FL-041 (`flife-rln`) adds `flife experiment` (plan §15.3–§15.5, §21.6, Appendix B.1), in `experiment`. An
experiment specification names a base configuration, a control, conditions (named ablations, overrides, a sweep),
discovery and held-out seeds and the selection procedure in words. `flife experiment` runs every condition on every
seed it plans, catalogs every run however it ended in FrankenSQLite (`flife-store`'s experiment catalog), keeps each
run's records and Appendix B.1 manifest, and writes a report computed from the catalog alone. A report is the same
bytes at any worker count and backend, and after a kill and a continuation.

FL-043 (`flife-8gy`), first slice, adds the peak-memory preflight (plan §7.5, §19.3, §19.4, INV-12, INV-15), in
`memory`. Before a run, resume or render steps, `memory::admit` adds up §19.4's terms against `[execution]
maximum_total_bytes` and refuses the run when the terms other than the backend's do not fit. The remainder goes to
the controller backend as its `BackendBudget`. A backend never holds more than its budget: a parameter shard that
would not fit is not made, and its jobs go to the reference, which gives the same bits. So a small budget slows a
run and never changes it.

The second slice moves the sum into `flife_engine::World::admit_memory` (`memory::admit` wraps it) and charges
observers. `flife render` builds its view before admission and charges its feed's slots, the view's own buffers,
one frame's FrankenManim job, the CPU route's frame buffers and, while a bloom is configured, its row sums
(`WorldView::{reserved_bytes, frame_bytes, offline_bytes}`) and its frames being encoded, `encoding_bytes(size, encoders)`: per pixel
`ENCODING_BYTES_PER_PIXEL` (the frame's 4 bytes and 48 for the PNG encoder's working set) and
`ENCODING_BYTES_FIXED` (2 MiB) per frame. `ENCODERS` (4) frames encode at once, or as many as the budget has room
for, one at least, before a render is refused. Its first record is `start`, with `memory` and `encoders`.

**Inputs.**
- A configuration or preset as TOML, validated by `flife_core::ValidatedConfig`.
- `flife validate <config>`.
- `flife run <config> --ticks N --every K --cpu-workers W --controllers C`, with N = 1,024, K = 128, W from the
  configuration and C = `auto` by default. C is `cpu` (the reference alone), `torch` (FrankenTorch's CPU kernel on
  the world's workers, `flife-backend-cpu`), `metal` (refused without a Metal device) or `auto` (Metal where the host has it and W ≤ `AUTO_METAL_MAX_WORKERS` = 8,
  otherwise `torch` at W ≤ `AUTO_TORCH_MAX_WORKERS` = 4, otherwise the reference). `flife
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
  - FL-022: `--remove TICK:X,Y:RADIUS:DISPOSITION`, repeatable: a `RemoveOrganisms` command, every organism
    within RADIUS world units of (X, Y) removed at that committed tick with the disposition `remove`, `dissipate`
    or `recycle`, numbered after the `--inject` and `--inject-material` commands.
  - FL-022: `--inject-material TICK:X,Y:AMOUNT:K=V,…`, repeatable: an `InjectMaterial` command, AMOUNT energy
    into cell (X, Y) with the signed composition whose channel K is V for each named K (each below 32, at most
    once) and zero elsewhere, numbered after the `--inject` commands. A direction with norm above one is refused
    at admission: exit 2 before any tick, naming `material direction`.
  - FL-022: `--introduce TICK:X,Y:COUNT:ENERGY`, repeatable: an `IntroduceOrganisms` command, COUNT founder-drawn
    organisms, each endowed with ENERGY, entering cell (X, Y) at the end of that committed tick, all or none under
    the population cap and genome quota, numbered after the `--inject`, `--inject-material` and `--remove`
    commands. A count of 0 or above 4096, or an endowment that is not a finite positive energy at most `f32::MAX`,
    is refused at admission: exit 2 before any tick.
- `flife resume DIR --until T --every K --cpu-workers W --controllers C`, with the same two checkpoint flags.
- In the library: `run_with(config, options, scheduled, persistence, emit)` and `resume(dir, ResumeOptions,
  emit)`, with `Persistence::new(dir, config)` giving the configured defaults. `scheduled` is a list of
  `ScheduledCommand`s (`Inject(Injection)`, `Material(Material)`, `Remove(Removal)` or
  `Introduce(Introduction)`, FL-022); `ScheduledCommand::listed(injections, materials, removals, introductions)`
  numbers them as the command line does.
- FL-040: `--metrics-every M` on `flife run` and `flife resume`, 0 (none) by default; `RunOptions::metrics_every`
  and `ResumeOptions::metrics_every` in the library. `Tracker::new(world, M)`, `Tracker::record(report)` and
  `Tracker::sample(epoch)` are public, so a client that steps a world itself can use them.
- FL-039: `flife branch PARENT DIR --at T --name NAME --inject TICK:X,Y:AMOUNT --checkpoint-durability D`.
  - `PARENT` is a run directory that no process holds; `DIR` is new or empty.
  - `--at` is the committed tick of a parent checkpoint. By default it is the newest one that restores.
  - `--name` is 1 to 256 bytes, `DIR`'s name by default.
  - `--inject` gives the branch's own brushes, at or after T, numbered after every inherited command, then
    `--inject-material` (FL-022) its own material brushes, `--remove` (FL-022) its own removals, and
    `--introduce` (FL-022) its own introductions, in that order.
  - In the library: `branch(parent, dir, &BranchOptions { at, name, commands, durability })`.

- FL-041: `flife experiment SPEC --out DIR --cpu-workers W --controllers C`. `DIR` is new, empty, or holds this
  experiment's catalog; W and C are as for `run` and change no reported byte. `SPEC` is TOML
  (`frankenlife.experiment.v1`, unknown keys refused):
  - `name`, `hypothesis`, `selection` (the procedure in words, §15.5), `control` (a condition's name), `ticks`,
    `metrics_every` (0 by default), and `base`, a configuration file relative to the specification;
  - `[seeds] discovery = [...]`, `held_out = [...]`: distinct, at least one discovery seed, each a TOML integer;
  - `[set]`: dotted overrides of keys the base already has, shared by every condition;
  - `[[conditions]]`: `name`, optional `ablation` (one of `ABLATIONS`), optional `[conditions.set]`, which replaces
    `[set]` key by key;
  - `[sweep] "key" = [values]`: one more condition per combination, named `key=value;…`;
  - `[select] measure = M, rule = "max"|"min"`: optional; picks one non-control condition from the discovery runs.
  - The named ablations (`ABLATIONS`, each a fixed set of validated keys): `no-emission` (emission rate 0),
    `fixed-topology` (architecture mutation off), `no-complexity-cost` (neural cost per parameter evaluation 0),
    `fixed-controller` (weight and replacement probabilities 0 and architecture mutation off; traits still mutate),
    `chemistry-only` (model `field-ecology-v1-chemistry-only`) and `fresh-offspring` (model
    `field-ecology-v1-fresh-offspring`); the two models are `flife-engine`'s.
  - In the library: `Experiment::load(path)` or `Experiment::new(spec, base_text)`, then `experiment(&experiment,
    dir, ExperimentOptions { cpu_workers, controllers }, emit)`. `Experiment::configuration(condition, seed)` gives
    any planned run's validated configuration.

- `flife inspect DIR --at T --field` (plan §14.6; FL-048's raw state). `DIR` is a run directory no process holds;
  `--at` the committed tick of one of its checkpoints, the newest that restores by default. In the library:
  `inspect(dir, InspectOptions { at, field })` returns an `Inspection`.

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
- FL-043: `start.memory` is the admission: `budget_bytes`, the fixed logical quota (`logical_parameters`,
  `logical_genome_bytes`), and the terms in bytes: `genomes` (the allocated genome quota), `state`, `field`,
  `scratch`, `births`, `render`, `checkpoint`, `reserve` and `backend`, with `peak_bytes` their sum. `end.controllers`
  carries `resident_peak_bytes`, the most the backend held after any batch. A run admitted on the reference alone
  has `backend` 0. None of these is scientific state.
- `flife render` (`frankenlife.render.v1`): a `frame` record per PNG written (frame index, tick, live count, path,
  render seconds) at the founding tick and every `--every` ticks, then `end` (frames, tick, the committed digest,
  the video path if encoded, the controller summary, wall seconds, and the style: `mapping`, its `meaning`, `look`,
  `particles` and `field`). FL-019's style flags choose what is drawn: `--color M` (`lineage`, `energy`,
  `generation`, `motion` or `topology`, or a registered name such as `founder-lineage-v1`; by default the
  configuration's `[presentation] color_mapping`), `--look glow|raw`, `--density` and `--no-field`. No style
  changes the world. Frames are the Studio's `WorldView` rendered on FrankenManim's CPU 3D
  route, with the configuration's bloom (`[presentation] bloom_enabled`, `bloom_strength`) on glowing frames and
  none on raw ones (FL-019); `tests/render.rs` renders the preset with `bloom_enabled = false` too: the glow frame
  differs and the raw frame is the same. Each frame's PNG is encoded on its own thread, at most four at once, while the world steps and the next
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
- `flife experiment` (`frankenlife.experiment.v1`): `experiment-start` (name, identity, conditions, and how many
  runs the catalog already holds), one `experiment-run` per run as it is cataloged (ordinal, condition, split,
  seed, result, wall seconds), `experiment-select` when a rule picked, and `experiment-end` (runs, complete, extinct,
  the report path and its SHA-256). Its directory holds:
  - `experiment.sqlite`: the identity, the canonical design, and one row per run (ordinal, condition, split, seed,
    configuration hash, status, result, manifest, the records file's SHA-256);
  - `runs/run-NNNNNN.ndjson`: the run's `flife run` records (with `metrics` records when `metrics_every` is set),
    or the `frankenlife.error.v1` record of a run that was refused;
  - `report.json` (`frankenlife.experiment-report.v1`): the design as resolved (conditions with every override
    their ablation and overrides set, the seeds, the control, the base configuration's hash), the selection (the
    procedure in words, the rule, each candidate's mean over its complete discovery runs, the pick), run counts by
    condition and split (reached, extinct, capacity-paused, failed, error), every comparison with the control, and
    every cataloged run with its result.
  - A result is `reached`, `capacity-paused`, `failed` or `error` (refused before or broken during the run, with
    its `CoreError` kind), the committed tick and digest, whether it is extinct, and its measures. FL-048 extends
    them to 24 (`MEASURES`), each with its basis (§15.2), which the report lists (`measures`):
    - complete over the run (the tally): peak and least live, births, deaths, emitter-ticks, emission paid,
      topology changes, mutated parameters;
    - complete at the end (the survey and the last metrics record's complete figures): survived, final live,
      deepest generation, lineages and distinct genomes alive, live topologies, lineage entropy, the dominant
      lineage's share, mean generation;
    - sampled from the render epoch at the last metrics record: occupied fraction, Morisita's index and Moran's I
      of the organisms over 16 × 16 bins, the field's Gini and Moran's I, mean speed and moving fraction.
    The last-record measures (`LastRecord`) need `metrics_every` > 0 and a metrics record at the run's last tick;
    otherwise they are undefined.
  - A comparison, per split and condition: the paired seeds (both runs reached their tick), the unpaired ones, and
    per measure n, both means, the mean difference, seeds higher, lower and equal, the differences in seed order,
    and the exact two-sided sign-flip p-value over all 2^n sign assignments (up to `EXACT_TEST_MAX_SEEDS` = 20).
    A measure undefined in either run of a pair (a sampled statistic of an extinct or uniform world) leaves that
    pair out of its effect, and the effect names the seed (`undefined_seeds`).
    That is the "effect e over k of n seeds versus control" a claim is worded in.
  - A run manifest (`frankenlife.run-manifest.v1`) in each catalog row, grouped as Appendix B.1: identity
    (experiment, ordinal, condition, ablation, split, seed, parent, creation time), model (model, descriptor
    digest, configuration hash, version), source (FrankenLife revision, the FrankenSuite commits `Cargo.lock`
    resolves, rustc, target, profile), numerics, clocks, hardware (CPU, cores, memory, OS, CPU workers, controller
    backend), capacity, provenance (seed, founders, initializer, RNG, durability, interventions), results (the
    result, wall seconds, ticks per second) and artifacts (the records file and its SHA-256).
  - Exit status 0 once the report is written, whatever the runs' endings; 2 for a refused specification or
    directory.
- Errors: one `frankenlife.error.v1` record on standard error. Its `kind` is `CoreError::kind`, or `io` for an
  unreadable file. Exit status is 0 on success, 1 when a run stops early (a capacity pause or a failed tick), and 2
  for an unreadable or invalid configuration, a refused session or a refused run directory.
- A run directory (FL-037) holds `config.toml` (the configuration's canonical TOML, written, flushed and renamed
  into place), `catalog.db` (the run catalog and command journal) and `store/` (the chunk store). A branch's catalog also records
  its lineage and its origin: the name, the parent run, the parent checkpoint's sequence and the creation time.
- In `flife-core`, FL-015 adds `CoreError::kind`, a stable kebab-case class name, distinct for every variant.
- `frankenlife.inspect.v1` (`flife inspect`): an `inspect` record (the checkpoint's sequence, tick and SHA-256, the
  world's digest, model, size and grid, the live count, and the energy the field and the organisms hold), one
  `organism` record per live organism in identity order (identity, genome, lineage, generation, parent, birth
  tick, position, velocity, energy, topology index, parameter count and affinity), and with `--field` one `field`
  record: every cell's energy, row-major (`y · cells_x + x`), each `f32` widened exactly. Every number is the stored
  value. The directory's configuration and catalog are checked before its store is opened, so a directory that
  holds no run is refused untouched; the store is opened as `resume` and `branch` open it.

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
- FL-043, the preflight's terms:
  - `genomes` is `maximum_allocated_genome_bytes`, which the genome quota already enforces. The logical quota stays
    the fixed scientific one: the preflight never changes which birth is accepted (§7.5).
  - `state`, `field`, `scratch` and `births` are the founded world's `World::footprint`, after its workers are set,
    plus `World::batched_bytes` for a run with a controller backend. The engine reserves the fields and the tick's
    scratch for `population.maximum_count` when the world is founded, its workers are set or a backend is
    installed, so they do not grow while the run steps.
  - `render` is what the observers hold (`flife render`: as above; a headless run has none). `checkpoint` is FL-038's admitted peak, or 0 without a run directory. `reserve` is
    `RESERVE_BYTES` (64 MiB), a declared allowance for the runtime, the session, the store and the device driver.
  - `backend` is `budget_bytes` minus the other terms, or 0 for `--controllers cpu`.
- FL-043: the admission comes before anything is created. A refused run makes no run directory and prints nothing.
  `flife resume` admits the resumed world under the same rule.
- The preset is Appendix A with exactly its declared changes. Its header declares each one, and the smoke test
  refuses any other difference.
- FL-037:
  - Checkpoints are taken in the admitted mode (FL-038, below). The session encodes one only once every journal
    record has its receipt and takes no step meanwhile; the catalog's index is written while the journal is
    idle. Keeping a run changes nothing in it: a `volatile-in-memory` run ends at the same digest with or
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
  - After each publication the store keeps the newest `retain_recent_checkpoints` that verify (FL-038: seal,
    lengths and hashes, never decoded); the catalog's index keeps every row, and only a manifest makes a
    checkpoint resumable (§16.6). Resume still restores.
  - A resumed run's tally covers the ticks that call published.
- FL-038 (§16.5, §19.4, A.2), `checkpointing`:
  - `pause-and-stream` (the preset's): `SessionHandle::checkpoint_with` hands the borrowed committed world to
    `flife_runtime::checkpoint_into`, which streams it into a `flife_store::Stream` a 1 MiB page at a time; the
    store commits, verifies (the container's SHA-256, read back), and the checkpoint is indexed and retained.
  - `bounded-background`: the encoder's pages go through a `Queue` to a writer thread that stores, commits and
    verifies them. The queue holds at most `[execution] checkpoint_retained_bytes` of pages, counting the one
    being written, and the encoder waits while it is full; the session resumes once the last page is queued.
    One checkpoint is in flight: the next one and the run's end wait for it, and a branch publication is refused
    meanwhile (`state-conflict`). Its catalog
    row is written on the paused session (`checkpoint_with`), where the journal is idle, then it is retained and
    its record emitted, with the wait counted in `paused_seconds`.
  - `admit` (the checkpoint term of §19.4's preflight): headroom = `maximum_total_bytes` −
    `maximum_allocated_genome_bytes`. `pause-and-stream` peaks at `CHECKPOINT_FIXED_BYTES` (two pages, the
    largest manifest and 1 MiB of handles, 3.06 MiB); `bounded-background` at that plus its budget. A
    background budget under one page, or a peak over the headroom, falls back to `pause-and-stream` and the
    start record says why; a headroom under `CHECKPOINT_FIXED_BYTES` is refused (`budget-exhausted`, budget
    `checkpoint memory`) before the run directory is created or the world founded.
  - `--checkpoint-mode` and `--checkpoint-retained-bytes` on `run` and `resume` override the configuration. No
    result depends on the mode: the checkpoints are byte-identical (tested).
  - Records: the start record's `checkpoints` (declared and admitted mode, retained bytes, peak, headroom,
    fallback); each checkpoint record's `mode`, `retained_peak` (zero for `pause-and-stream`) and
    `paused_seconds` (wall clock, not scientific state).
  - A metrics tracker re-founded on resume reads the committed world through `checkpoint_with`, without a copy.
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
- FL-041:
  - Runs are planned in order: every condition on every discovery seed, then the held-out seeds, ordinals from 1.
    With a `[select]` rule only the control and the pick run on the held-out seeds; without one, every condition.
  - The rule reads only complete discovery runs, so the held-out runs cannot influence the pick, and picks again
    the same way when the experiment is continued after its held-out runs are cataloged. A tie goes to the
    condition listed first; when no candidate has a complete discovery run, nothing is picked and only the control
    runs on the held-out seeds.
  - A run's records are written, flushed and renamed into place before its catalog row is committed. A continued
    experiment skips every cataloged ordinal and runs the rest; a run killed before its row commits runs again.
    Cataloging an ordinal again with another row is refused.
  - The report is computed from the catalog and the specification alone: no wall time, host, build or worker count
    enters it (those are in the manifests). Runs execute one at a time, and each run's result is the same at any
    worker count and backend (`flife-engine`'s determinism), so the report's bytes are too.
  - An experiment's identity is the SHA-256 of its canonical design: the base configuration's hash, every resolved
    condition, the seeds, ticks, metrics interval, control, selection words and rule, and (FL-048) the names of the
    measures compared. A directory cataloging one identity refuses another, so a catalog is never continued by a
    build that measures runs differently.
  - Every planned run is cataloged and listed, whatever its ending. Extinct runs that reached their tick are
    compared like any other; runs that paused, failed or were refused are counted and named as unpaired seeds.
    Nothing is dropped from the report.
  - Every condition's configuration is validated at the first discovery seed before anything runs.

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
- FL-041, before anything runs: a specification with an unknown key, another schema, an empty name, hypothesis or
  selection, zero ticks, no discovery seed, a repeated seed (a held-out seed equal to a discovery seed included), a
  seed beyond a TOML integer, an override of a key the base lacks or of `seed` or `schema`, a condition or `[set]`
  that sets a key its ablation sets, an unknown ablation, an empty or repeated condition name, a control that names
  no condition, fewer than two conditions, an empty sweep, an unknown measure or rule, or a condition whose
  configuration does not validate (`invalid-config` or `unknown-algorithm`); a directory holding files but no
  catalog, or another experiment's catalog (`state-conflict`). A run's own refusal (for example
  `write-ahead-before-apply` without a run directory) is cataloged as an `error` run, not returned.
- FL-043: a run whose terms other than the backend's exceed `maximum_total_bytes`: `budget-exhausted` (budget
  `peak memory (maximum_total_bytes)`), before a run directory is made or a record is printed.
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

FL-022: `removals_are_journaled_checkpointed_and_replayed_like_any_command` (in `tests/resume.rs`) runs the
ecology with two removals (tick 100 at the centre, recycled; tick 160 across both seams, dissipated): it journals
six commands, removes organisms and ends elsewhere than the run without them. Stopped at 102 with its newest
checkpoint damaged, it resumes from 98 with both removals pending, replays the first's outcome and reaches the
uninterrupted world and final checkpoint bytes; SIGKILLed after tick 130 it does too.
`a_removal_branch_matches_its_control_until_the_removal_and_differs_after` (in `tests/branch.rs`) branches at
tick 112 with `--remove` at 125: the removal is command 5, the branches match at 125 and differ at 126, where only
the treated branch has removed organisms. A `persist` unit test pins `--remove` parsing.

FL-022's material brush: `materials_are_journaled_checkpointed_and_replayed_like_any_command` (in
`tests/resume.rs`) runs the ecology with two materials (tick 100 at the centre, `0=0.6,1=-0.8`; tick 160 at a
corner cell, `5=1`): it journals six commands and ends elsewhere than the run without them, and elsewhere than a
run with the same energies as plain `--inject`s, so the composition reached the field. Stopped at 102 with its
newest checkpoint damaged, it resumes from 98 with both materials pending, replays the first's outcome and reaches
the uninterrupted world and final checkpoint bytes. A direction of norm √2 is refused with exit 2 and no end
record. `a_material_branch_matches_its_control_until_the_material_and_differs_after` (in `tests/branch.rs`):
command 5, the same world at 125, another at 126. `persist` unit tests pin `--inject-material` parsing (named
channels, each once, below 32) and `listed`'s order (injections, materials, removals).
Thirteen planted mutants across the material brush (engine, codec, CLI) each ran against suites that first passed
unmutated: the moment written from the declared rather than the applied energy, the norm bound exclusive, no norm
check, zero components written, the channel off by one, a material removing organisms, a material skipping the
cell and amount check, the direction encoded backwards, the amount decoded after the direction, a repeated channel
accepted, removals listed before materials, a scheduled material sent as an energy injection, and the last channel
entry dropped. Twelve were caught on their first run. The survivor, `removes_at` answering true for a material,
is unreachable from the world (phase 1 asks only removals) and was caught once the engine test asked the public
`removes_at` directly.

FL-022's seeded introduction: `introductions_are_journaled_checkpointed_and_replayed_like_any_command` (in
`tests/resume.rs`) runs the ecology with two introductions (six organisms at the centre at tick 100, three at a
corner cell at 160): it journals six commands and ends elsewhere than the run without them. Stopped at 102 with its
newest checkpoint damaged, it resumes from 98 with both pending, replays the first's outcome and reaches the
uninterrupted world and final checkpoint bytes, so the introduced organisms, genomes and identities come back from
checkpoint and journal alike. An introduction of no organisms is refused with exit 2, no end record and
`introduction count` named. `an_introduction_branch_matches_its_control_until_the_introduction_and_differs_after`
(in `tests/branch.rs`): command 5, the same world at 125, another at 126. `persist` unit tests pin `--introduce`
parsing and `listed`'s order (injections, materials, removals, introductions). The metrics tracker admits each
report's introductions as founders of their own lineages (generation zero, born at the tick, above every earlier
identity): `the_tracker_follows_introduced_organisms_until_they_die` (in `tests/metrics.rs`) introduces six
organisms at ticks 0 and 40 and checks at every one of 600 ticks that the tracker holds exactly the committed world,
through the introduced organisms' deaths. A tracker that ignores the introductions refuses the introduction's own
tick, its live count short of the report's (planted mutant M20).
Twenty planted mutants across the introduction (engine, codec, CLI, metrics) each ran against suites that first
passed unmutated: the cap exclusive, the cap ignoring the tick's earlier entrants, a quota shortfall keeping the organisms
staged so far, a refused introduction still spending identities, the x and y cells swapped, every organism in
lineage 0, no cooldown, phase zero, the endowments unrecorded, a refused introduction reporting its declared
energy, no lower bound on the count, the limit itself refused, the introduced energy left out of the holdings,
the report not resorted, the count not encoded, the energy decoded before the count, introductions listed before
removals, a scheduled introduction sent as an energy injection, the report listing no introductions, and the
metrics tracker ignoring them. Seventeen were caught on their first run. Phase
zero survived because the ecology evaluates every tick (every phase is zero), and the unsorted report survived
because no test mixed an introduction with an earlier-phase intervention; both were caught once a test ran a
controller interval of 4 and one sealed an introduction before an injection and a removal. The energy-injection
mutant first failed to compile (`intervention` is `const`) and was caught once rewritten.

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

`tests/experiment.rs` (7 tests: FL-041's six and one of FL-048's, about 9 s in release), on FL-037's 32 × 32
ecology from the plan's own Appendix A:
- `repeated_experiments_report_byte_identically_at_any_worker_count_and_across_a_kill`. Control, `no-emission` and
  `chemistry-only` over three discovery and two held-out seeds (15 runs). One CPU worker on the reference and three
  on `auto` write byte-identical reports. The `flife` binary is killed with SIGKILL after its fourth
  `experiment-run` record, then continued with another worker count: the start record counts what was cataloged,
  only the rest run, and the report is byte-identical to the uninterrupted one; the end record's hash is the
  report's. Another experiment is refused in that directory. Every comparison is recomputed by the test's own code
  (pairs, means, counts, and the p-value by recursive enumeration of sign assignments).
- `the_catalog_keeps_every_run_whatever_its_ending`. Four conditions over two discovery and one held-out seed: a
  control, `starved` (extinct by tick 120), `crowded` (paused at the population ceiling) and `journaled` (refused:
  `write-ahead-before-apply` without a run directory). All 12 are cataloged and listed in order with their status,
  extinction, tick and detail. Each records file hashes to its row; its last line is the end record (same digest
  and tick) or the error. Each manifest names its experiment, condition, split, seed, ordinal, configuration hash,
  result, records file and hash, a host with cores, and the asupersync commit `Cargo.lock` resolves. Counts match
  per condition and split; the extinct runs are compared (survival difference −1 on every seed); the paused and
  refused ones are unpaired.
- `a_selection_rule_sees_only_discovery_runs_and_holds_out_the_rest`. A basal-cost sweep (0.3 and 1.2) with `max
  final_live`. The pick is the condition with the higher discovery mean, the candidates' means and run counts are
  the discovery runs', and only the control and the pick run on the held-out seeds, with one held-out comparison.
  Continuing the finished experiment runs nothing and gives the same selection and the same report bytes.
- `every_named_ablation_changes_what_it_names`. All six ablations against a control with architecture mutation at
  half of births, 240 ticks, from each run's own records: no emission paid; no topology change but parameters
  still mutated; a controller-cost flow under 1e-9 of the control's (rounding residue, below); no mutated
  parameter or topology change, births still happen and live parent–child pairs are bit-identical; no emitter-tick
  but births; under fresh offspring under a quarter of parent–child parameters identical against over 90% in the
  control.
- `specifications_that_would_hide_a_choice_are_refused`. A good specification resolves, with each condition's
  ablation keys and shared overrides recorded. Thirteen one-change variants are refused (overlapping seeds, no
  discovery seed, no control, a blank selection, an override of a key the base lacks, the seed, an ablation's key,
  an unknown ablation, a repeated name, an invalid value, an empty sweep, an unknown measure, an unknown field), and
  so is a directory holding other files.
- `the_sign_flip_test_by_hand`: [1, 2, 3] → 1/4, [3, 1] → 1/2, [1, −1] and zeros → 1, twenty ones → 2/2^20, and no
  value for no seeds or more than twenty.
- FL-048 extends three of them. The endings test reads each run's last `metrics` line from its records file and
  requires the last-record measures to equal it field by field, and the live-topology count to equal the end
  survey's. An extinct run's clumping, speed and lineage entropy are undefined, and its comparison's
  `organism_morisita` effect has n = 0 and names every paired seed, while `field_gini` keeps every pair. The
  report declares all 24 measures with the right basis. The refusals test requires the design to list the
  measures and the identity to hash the design. Every comparison check now pairs per measure.
- `a_run_is_measured_by_its_last_metrics_record_only_at_its_end` (FL-048): a 120-tick run with a metrics record
  every 50 ticks has records at 50, 100 and 120. `Measures::of` takes `LastRecord::of` the tick-120 record, and
  none from the tick-100 record or from no record.

Seven planted mutants in FL-048's measures were each caught: undefined seeds not named, an undefined measure
counted as zero, organism Moran's I read from the field, the identity without the measures, topologies counted
with none alive, a sampled measure declared complete, and a last record of another tick used. The last survived
its first run, since a run always ends with a metrics record at its last tick; the direct test above was added
and catches it.

Eleven planted mutants were run against `tests/experiment.rs` (the two model mutants also against `flife-engine`'s
`tests/ablation.rs`), and every one was caught:
- a continuation that reruns cataloged runs (the kill test and the selection test's continuation);
- a selection rule that also counts held-out runs (the selection test's continuation);
- incomplete runs paired, the report leaving out runs that did not reach their tick, and only complete runs
  cataloged (the endings test);
- the sign-flip test counting only strictly more extreme sums, and its denominator off by one (the test by hand
  and the recomputed comparisons);
- a condition allowed to set its ablation's key, and held-out seeds allowed to repeat discovery seeds (the
  refusals test);
- `chemistry-only` decoding the controller's output and `fresh-offspring` inheriting the parent's genome (the
  ablation test here, and each model's own evidence test in `tests/ablation.rs`).

`tests/inspect.rs` (2 tests): `inspection_reports_the_committed_world_as_a_direct_world_holds_it` runs the
48-founder ecology to tick 30 with checkpoints every 7 ticks; the newest inspection is tick 30 with the run's last
digest, and inspections at 28 and 21 equal a world founded and stepped directly in the test, organism by organism
bit for bit, every field cell, the digest and both energy totals; ticks 7 (retired) and 29 (never taken) are
refused. `a_directory_without_a_run_is_refused_untouched`. Six planted mutants (the tick ignored, velocity for
position, the field plane reversed, affinity left out, a founder its own parent, organism energy doubled) were each
caught.

`tests/memory_admission.rs` (11 tests, FL-043, about 15 s; a counting global allocator, `peak_alloc`, test only):
- `the_preset_is_admitted_term_by_term`: on the preset each term is its source (the configuration's quotas, the
  world's footprint, `RESERVE_BYTES`) and the peak is their sum: 1,375.7 MiB on the reference (genomes 1,152.0,
  state 19.63, field 6.95, scratch 79.25, births 53.88, reserve 64.0). With `torch` the scratch adds the batched
  lists (105.13 MiB) and the backend is granted the remaining 646.4 MiB of the 2,048.
- `a_budget_below_the_terms_is_refused_before_anything_is_created`: a budget of the genome quota plus 8 MiB, with
  `pause-and-stream` checkpoints, is refused with `budget-exhausted`, and the run directory does not exist.
- `a_small_backend_budget_defers_and_never_changes_the_run`: a 48 MiB backend grant on `torch` for 24 ticks ends
  at the reference's digest, with both batched and deferred evaluations, and the backend's resident peak at most
  the grant (40.9 MB of 50.3 MB).
- `a_full_population_holds_no_more_heap_than_its_terms`: `population.maximum_count` 4,096, the founders' count, on
  `torch` for 48 ticks, the population full throughout. The allocated fields and scratch never change, and the
  process's heap peak from founding (356.5 MiB) is
  at most the admitted terms with the genome term at the arena's actual peak and the backend at its resident peak
  (370.8 MiB).
- `a_filling_introduction_stays_within_the_staging_term`: 512 founders of a capacity of 4,096, and at tick 0 an
  introduction of the other 3,584. The step holds 142.60 MiB above the world before it: within the staging term
  (13.47 MiB), the introduced genomes' parameters (136.55 MiB) and the state's room to its bound (1.34 MiB).
- `a_render_holds_no_more_heap_than_a_plain_run_and_its_render_term`: the preset rendered at 1280 x 720 for 8 ticks
  every 2, in points and in density, holds at most the heap of the same world stepped without observers plus the
  admitted render term (observers held 52.7 MiB above the plain run, against a term of 302.5 MiB, which charges
  incompressible frames and the full ceiling).
- `a_renders_term_is_its_observers_and_a_short_budget_encodes_fewer_frames`: at 320 x 180 the term is exactly the
  feed, the view's buffers, the frame's job, the CPU route's buffers and four encodings. A budget with room for two
  encodings gets two; one byte short of room for one is refused.
- `a_frames_encoding_holds_no_more_than_its_allowance`: one PNG encoding of noise (no matches) and of a flat frame
  at 96 x 64 to 1920 x 1080 holds, with the frame, at most 47.8 bytes a pixel (noise at 640 x 360), within the
  allowance.
- `a_views_own_buffers_hold_no_more_heap_than_its_reserved_bytes`: a view of the preset at its 16,384 ceiling, shown
  twice in points and twice in density, holds 9.40 of its 9.68 MiB at 1280 x 720 and 16.00 of 16.27 at 4K (FL-033's
  textures and the field's values copy; the vertex grids held 14.49 of 16.41 and 45.56 of 47.82).
- `a_pan_or_zoom_allocates_no_layer_texture` (FL-033): that world's 4K view, in points and in density, through a zoom
  of 3, a pan and a zoom of 16, allocates at most 440 bytes a placement (the quads), less than the field's 256 KiB
  texture: both layers keep their textures. A layer that rebuilds on a pan fails it (the field's 262,584 bytes, the
  density's 2,074,040).
- `a_cpu_frame_holds_no_more_heap_than_the_views_frame_terms`: a 4K CPU frame of that world holds its draw list
  within what `frame_bytes` adds to the preparation limit (7.75 MiB in points), its job within `frame_bytes`
  (72.1 MiB of 88.8 in points, 0.01 in density) and, above the job, within `offline_bytes` (55.6 of 95.5 MiB;
  with the preset's bloom 63.65 of 114.24 MiB, 8.08 MiB more: the bloom's ring of 138 rows of row sums at sigma
  1.5 (8.09 MiB), within the 18.75 MiB charged for the widest bloom's 320; the whole frame's took 126.56 MiB
  before FrankenManim `c6ad9c5b`).
  FL-033 found the job's heap above FrankenManim's accounting once the grids' triangle allowance was gone (72.1 MiB
  against a `frame_bytes` of 56.8); `JOB_BYTES_PER_DRAW` now charges it.

Eleven planted mutants were run against these tests and `flife-engine`'s, `flife-backend-cpu`'s and
`flife-backend-metal`'s FL-043 tests, and every one was caught: the source supports, the lanes at a new worker
count and the batched lanes left unreserved (the footprint and installation tests); the CPU backend making a shard
whatever its budget, the Metal backend skipping its shard check, and its owner unbounded (the budget world tests);
the admission never refusing, the births term left out of the sum, and the run directory created before the
admission (the tests here); and the staging term set to zero (the filling introduction). Two were first missed:
the unbounded owner, until the Metal test asserted the owner's limit, and the zero staging term, until the
filling introduction was added. The full-population heap test alone has too little margin to see it.

Second slice: 24 planted bugs, 22 caught. Caught: the admission's render term, batched lists, backend grant and
refusal; render's view buffers, frame job, CPU frame buffers, fewer encoders and `start` record; the view's dot
and triangle charges, its field once shown and once made, the grid's quad count, the grids counted once, the float
frame, the density bins and the draw list; the Studio's upload, device and feed-slot terms; and an unprojected
standalone epoch. The triangle charge (640 cut to 400) and the draw list survived the first run and were caught
after a grid-dominated world (a ceiling of 64) joined the frame sweep and the draw list was measured on its own.
Two survive. The encoding loop ignoring the admitted count (`>= ENCODERS`) is unseen: the number of frames
encoding at once is not observable from outside, and the render term's slack hides it on the heap. A job compiled
without the limit is unseen because a correct limit is never reached; FrankenManim's refusal past it was seen only
when the triangle charge was planted short.

**Validity domain.** The `reference-small-world` preset and the configurations `flife-engine` founds, at
`volatile-in-memory`, on the native current-thread runtime with one owner and one client. FL-037's run directories
also at `write-ahead-before-apply`, one process per run directory, on local file systems (APFS tested here).

**Known gaps.**
- FL-043 gaps (first slice):
  - The Studio is admitted (second slice), but only `flife-studio`'s headless tests check it: see its contract.
  - The encoding allowance and the preparation coefficients are measured on the pinned FrankenManim, not derived
    from it. The preparation limit is enforced by FrankenManim; the encoder's working set is not, and is charged
    at its worst measured case (noise).
  - `reserve` is a declared allowance, not a measurement of the runtime's, the store's or the driver's heap.
  - The births term is an analytic worst case. The filling introduction tests its entrant and event part; a tick
    in which every organism reproduces, which would test its birth part, is not made.
  - Shards are never freed before `forget`, so a backend stays at its peak (FL-026); the budget bounds that peak.
- Only `validate`, `run`, `resume`, `render`, `branch` and `experiment`. Capabilities, step and inspect, analyze
  and export are later items (§14.6): FL-021 and FL-023.
- FL-039 gaps:
  - §14.4's synchronized simulation-time views and paired metrics are not here: a branch's paired comparison is two
    `flife resume` runs and their records. Experiment reports (FL-041) compare runs founded afresh per seed, not
    branches, and side-by-side views are the studio's. Nothing assumes common random numbers stay coupled after the
    branches' births differ.
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
  - Checkpoints of the 48-founder test ecology are about 2 MB each.
  - Power loss is FL-035's protocol (flush order), exercised by publishing under `power-loss`, never by cutting
    power. Kills are tested at whatever point the process reached, not at every step of a checkpoint (FL-035 and
    FL-036 test those).
  - Commands come only from `--inject`, `--inject-material`, `--remove` and `--introduce` at the start of a run
    or branch. Commands submitted to a resumed run, and the other interventions, are FL-022.
  - Retention is by count. Pinned discoveries and journal compaction (§16.7) are later, and the catalog's
    checkpoint index keeps rows for retired manifests.
- FL-038 evidence (`tests/checkpoint_memory.rs`, a counting global allocator, `peak_alloc`, test only; and
  `tests/resume.rs`):
  - A `pause-and-stream` checkpoint of the preset (160.7 MiB) streamed, committed, verified and retained peaks at
    1.02 MiB of heap beyond the live world, within `CHECKPOINT_FIXED_BYTES` (3.06 MiB). The same checkpoint
    encoded whole in memory, the negative control, peaks at 421.5 MiB.
  - Runs of the preset to tick 64, checkpointing every 32 ticks, in `pause-and-stream` and in
    `bounded-background` with four pages and with one: the same checkpoints (ticks 0, 32, 64) byte for byte, the
    same digest as a run without checkpoints, and the newest restoring to it. Background pages retained:
    2,162,700, 4,194,304 and 4,194,304 bytes against four pages; exactly one page against one. Heap peaks (one
    run each, M5 Ultra): the run without a run directory 226.3 MiB; `pause-and-stream` and one page the same;
    four pages 1.95 MiB above it.
    The bound asserted is the admitted peak plus an 8 MiB run-directory allowance.
  - Admission by the numbers on the preset, the fallbacks and their reasons, the refusal before anything is
    created, and the configuration naming the mode.
  - The journaled ecology with four brushes, its checkpoints taken `bounded-background` with one page: the same
    checkpoints as `pause-and-stream` at every multiple of 7, each indexed in the catalog, the same final world;
    SIGKILLed once it had reported tick 70 (its checkpoint at 63 not yet committed), it resumed from 56 to the
    uninterrupted state.
  - The queue's own tests: a full queue holds the encoder at its budget; a stopped writer releases every queued
    page and refuses more; an abandoned encoder ends the queue without a seal.
  - 14 planted mutants in the queue, admission, the run's checkpoint path and its records, each caught; one
    (`Producer`'s drop not ending the queue) by hanging the queue test until the harness's 600 s timeout.
    Restoring the whole in-memory encoding in `pause-and-stream` peaked 419.8 MiB above the run without
    checkpoints and failed the run-level bound. With `flife-store`'s 16, 30 in all; 29 caught on their first
    run.
  - Cost (the `flife` binary, the preset for 256 ticks on 16 CPU workers, checkpoints at 0, 128 and 256 under
    `power-loss`; one run each on an M5 Ultra at host load about 25, so no speed claim): without checkpoints RSS
    243–245 MiB and 3.3–3.4 s. `pause-and-stream` RSS 265.4 MiB, 6.23 s, each checkpoint pausing the run 0.9–1.0 s
    (before FL-038: RSS 1,621 MiB, 6.87 s). `bounded-background` with the preset's 128 MiB: RSS 328.9 MiB, 5.00 s,
    28–45 MiB retained, pauses 0.37–0.78 s (the last includes the wait at the run's end); with 4 MiB: RSS
    274.5 MiB, 5.06 s, 4 MiB retained. The checkpoints are 152–169 MB; the digest is the same in every run.
- FL-038 gaps:
  - No incremental mode: every checkpoint is written in full and a single birth rewrites the genome chunk; there
    is no copy-on-write paged genome bank (§16.5, A.2). A 4,096-organism checkpoint is about 161 MiB.
  - `bounded-background` needs a budget of at least one page, or it streams instead.
  - The admission covers the checkpoint's own heap only. §19.4's other terms (state, scratch, render, births,
    backend) and the whole M_peak preflight are FL-043.
  - The heap is measured by the test allocator, not RSS; file-system caches and FrankenSQLite's own pages are
    outside it. The 400k profile is not measured.
  - Retention verifies hashes without decoding (see `flife-store`'s gaps).
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
- `--inject` (FL-037) and `--remove` (FL-022) are the only command sources: single-cell energy brushes and disk
  removals at fixed ticks. The other interventions and recorded commands are FL-022's.
- The survey sees only live organisms. Pairs and triples need their relatives alive. FL-040's records carry each
  lineage's births, deaths, deepest generation and extinction, but no ancestry tree.
- The survey compares whole genomes and hashes every live genome, which is cheap at this scale.
- FL-040 gaps:
  - Metrics are NDJSON records only. §16.6 puts lineage summaries and sampled metrics in the run catalog; no catalog
    table holds them yet. FL-041's experiment catalog hashes each run's records file, metrics included, but does
    not index the metrics themselves.
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
- FL-041 gaps:
  - Runs execute one after another in one process, each without a run directory: no checkpoints, no journal, so
    `write-ahead-before-apply` conditions are refused (and cataloged as such), and a killed run restarts from its
    founding. Runs on several hosts or processes are not supported.
  - §21.6's shuffled-inheritance and shuffled- or frozen-controller variants are not implemented; `chemistry-only`
    decodes every action from zero (σ(0) everywhere), and `fresh-offspring` gives each newborn its founder genome.
  - `no-emission` sets the emission rate to 0, so gates still open (emitter-ticks are counted) but nothing is paid
    or deposited. A gate threshold of 1.0 passes configuration validation but `ActionDecoding::new` refuses it at
    founding ("out of range"); that inconsistency predates FL-041 and is why the ablation uses the rate.
  - `no-complexity-cost` leaves a controller-cost flow of a few ulps per tick: `MaintenanceBill::settle` attributes
    the rounded payment's remainder after basal and sensor to the controller term. Removing it would change the
    model's arithmetic (a new model identity); the test bounds it instead.
  - The measures are end-of-run scalars: tally totals, the end state, and one render-epoch sample at the last
    metrics record (FL-048). Time series, trait distributions and lineage shapes stay in the records files; the
    report does not compare them. The sampled figures are of the F32 epoch at 16 × 16 bins, not of the F64
    committed state.
  - The sign-flip test is exact up to 20 paired seeds and absent beyond; no multiple-comparison correction is
    applied, so a claim names its measure in advance (§15.5) and the report lists every comparison.
  - The source revision is `git rev-parse HEAD` when the build script last ran (uncommitted edits are not
    detected; `unavailable` outside a checkout). The hardware group reads `sysctl`/`sw_vers` on macOS and
    `/proc` and `/etc/os-release` elsewhere; a fact a host does not expose is absent.
