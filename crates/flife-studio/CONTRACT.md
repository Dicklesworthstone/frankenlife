# flife-studio contract

**Purpose.** FL-018 (`flife-jdf`) covers FrankenLife's safe native input host and first interactive Studio
(plan §13.1, §13.6). FL-019 (`flife-g90`) adds live switching between its colour mappings, raw views and density
rendering (§13.4), the configured bloom on glowing frames (§2.3), the shown mapping's colour key in the overlay
(§13.4's legend: `legends`, `flife_manim::ColorMapping::legend`), and the field's chemical composition beside its
energy (§13.4's "Local field": key C, `--field-mapping`, `flife_manim::FieldMapping`), keyed after the colour key. FL-020 (`flife-err`, part) adds the camera (zoom about the pointer, drag pan across the seams,
reset), selection by displayed stable identity (§13.6), and the display one committed state late, organisms moving
between epochs by identity (§13.5). FL-021 (`flife-zyc`, part) reads a selected organism back from the session as a
bounded snapshot at its committed boundary (§14.2), with what the committed field offers it where it stands, its
children's and descendants' counts and the ancestors its line passes through (the world traces its ancestry from
the founding), and what it sensed, its controller's activations and what its budget stages paid and received once
the session has stepped while watching it, and draws it as a panel in the
overlay, read again once a second while the world runs. FL-022 (`flife-2jj`, part) adds energy, removal, material and
seeded-introduction brushes: a click becomes a world-space command that the session applies at its next open boundary, the region a click would act on is outlined on the world, and the Studio reports the tick it applied
at and the energy it moved (§14.3, §12.3); T has the camera track the selected organism (§14.3's camera-follow
target). FL-024 (`flife-xut`, part) reports its clocks honestly (§12.2): the
scientific ticks committed per wall second and the simulation-time to wall-time ratio, beside the frames presented.
They are drawn as text in the window's top-left corner, with what the session is doing and what a click will do.
The overlay's lines are `overlay_text`'s: the run and its tick, the clocks, plan §14.1's persistent telemetry
(`telemetry_lines`), the shown mapping's key (each swatch in the colour that mapping gives its label; the topology
key labels the run's catalog members by their hidden widths), the field's key while the field shows its
composition, the armed brush and the selection, the armed
brush's preview (`preview_line`, §14.3), then the inspector panel. A count of a trillion or more, and a magnitude of 100,000 or more, is written in scientific
notation, so no line grows without bound. The telemetry is the shown epoch's (`flife_render::Metrics`): the live
population and its logical parameters; the neural cadence and the full-population evaluations a wall second; the
genomes' allocated bytes against their quota and the memory budget's headroom above the admitted peak; the
births, introductions and deaths since founding, the capacity pauses and whether a fault stopped the run; the
ledger's injection, removal and dissipation, the organisms' uptake and the residual; and the model, seed and
durable checkpoint. The binary prints the same quantities as a `telemetry` record a second.
It closes the G0 native-input gap: the presenter's real AppKit events reach FrankenLife as typed
input. The live world is shown in a native window, through FrankenManim's Metal 3D route and the `flife-manim`
adapter. For FL-024's frame study (G6, §13.7) the same Studio runs with no window (`Surface::Offscreen`): typed
input is posted to it, each frame is rendered through the same Metal route and read back, and the binary's
`--offscreen` runs its own loop that way and reports its frames' times and its camera moves' latency.
FL-023 (`flife-w5q`, part) records the Studio's presentation timeline (§13.8, live capture). A recording Studio
(`StudioOptions::record`, the binary's `--timeline FILE`) notes every change a frame makes to its view, every
frame drawn with the tick shown and its wall time, and the commands the session applied. `replay` founds the
world again and steps it with those commands at their ticks, and redraws each frame through the same Metal route,
byte for byte, closing on the Studio's world or refusing. `export` (the binary's `--export FILE --out DIR`)
writes that replay through FrankenManim's frame and encoding paths: a PNG a frame, a y4m video on a constant
clock with its repeats and drops declared, and a manifest of the provenance (G10).
FL-024 (`flife-xut`, part) keeps a session in a run directory (`StudioOptions::run`, the binary's `--run DIR`;
`Keeping`, §14.1's durable checkpoint tick, §16.3): the configuration, the command journal and checkpoints, kept by
`flife-cli`'s own run-directory library, so `flife resume`, `flife branch` and `flife inspect` take it, and the
telemetry line names the newest checkpoint's tick. It also reopens such a directory (`Studio::resume`, the
binary's `--resume DIR`): the run goes on from its newest valid checkpoint and the journal past it, in the same
directory, with its pending commands and its checkpoint chain.

**Inputs.**
- A `ValidatedConfig`. `[presentation]` gives the window size, the background, the frame rate, the colour
  mapping, the bloom (`bloom_enabled`, `bloom_strength`: `WorldView::set_bloom`, FL-019) and the dots'
  smallest radius on screen (`point_radius_pixels`, in points: `WorldView::set_point_radius`, FL-019; a radius
  above 24 points refuses the start).
  `StudioOptions` can override the size, make the window fixed and set the starting `Style`.
- With `StudioOptions::run` (`--run DIR`): a directory that is absent or empty, and a configuration whose
  `maximum_total_bytes` less `maximum_allocated_genome_bytes` holds a checkpoint (`flife_cli::admit`). Its
  `[persistence]` gives the interval, mode and retention, and `[execution] command_durability` the journal.
- With `Studio::resume(dir, options)` (`--resume DIR`, no configuration argument): a run directory `flife run`, the
  Studio or `flife resume` kept. Its own configuration is the run's; `flife_cli::recover` reads it, checks the
  run identity, restores the newest checkpoint that verifies (skipping and naming any that do not; with none, the
  founding world, which a branch does not have, so a branch with none is refused), and `flife_cli::open_session`
  replays the journal past it. `options.run` (the run stays in its own directory) and `options.record` (a timeline
  replays from the founding world, where a resumed run does not start) are refused before anything opens, as
  is a directory that holds no run.
- For a window: the process main thread, macOS, Metal and a window server. Offscreen
  (`StudioOptions::surface = Surface::Offscreen { scale }`): any thread and a Metal device; the content area is
  `size` points at `scale` pixels a point, and `title` and `resizable` are unused.
- The presenter's typed input and its pixel density. They come from frankentorch `ft-kernel-metal` at rev
  `0ae728fe`, through FrankenManim `fmn-render` at rev `c6ad9c5b`.
  - The kinds are pointer moved, button, scroll, key, modifiers changed, focus gained, focus lost, pointer capture
    lost and resized (with logical size, backing scale and drawable size).
  - The presenter queues at most `INPUT_QUEUE_CAPACITY` = 1024 events between polls. It coalesces motion, scroll and
    resize, and counts every drop.

**Outputs.**
- `Controls`: a pure state machine from `InputEvent`s to `Intent`s and presentation state. It holds the pointer, the
  held buttons, focus, the surface metrics and a tally by kind. `follow(status)` tracks the session's current run,
  and `refused(intent)` handles an intent the lane refused.
  - Keys go by virtual key code, so they do not depend on layout:

    | Key | Intent |
    |---|---|
    | Space (not auto-repeat) | `Resume` or `Pause` |
    | → or `.` (auto-repeat steps again), while paused | `Step` |
    | Esc, or ⌘W | `Close` (once) |
    | 1 to 6 | colour by lineage, energy, generation, motion, topology or chemical affinity |
    | R | switch the glow and raw looks |
    | D | switch dots and density |
    | F | show or hide the field |
    | C | show the field's energy or its chemical composition (FL-019) |
    | 0 | the whole torus again (a `CameraMove::Reset`) |
    | E | arm the energy brush, or disarm it (FL-022) |
    | X | arm the removal brush, or disarm it (FL-022) |
    | M | arm the material brush, or disarm it (FL-022) |
    | I | arm the introduction brush, or disarm it (FL-022) |
    | W (without ⌘) | dump the selected organism's whole controller (a `CameraMove::Weights`, FL-021) |
    | T | track the selected organism, or stop (a `CameraMove::Track`, plan §14.3) |

    The style keys, 0, W, T and the brush keys ignore auto-repeat and ⌘ (⌘W closes), and produce no intent. A brush key with another
    brush armed switches to its brush. `brush()` is the armed brush.
  - Pointer gestures become `CameraMove`s, in logical points, kept in order until `take_moves()` (reserved once
    for a poll's events):
    - a primary press that moves more than `CLICK_SLOP_POINTS = 4` is a drag: `Pan` by the whole motion since the
      press, then by each move, so the pans sum to the pointer's motion;
    - a primary press released within the slop is a click: `Select { at }` at the press, or with a brush armed at
      the release, `Brush { at, brush }` (FL-022);
    - capture loss ends a press: no click, no more panning;
    - a vertical scroll is `Zoom { anchor, factor }` about the pointer, `factor = scroll_zoom(dy, unit) =
      exp(rate · dy)` with `ZOOM_PER_POINT = 0.01` (trackpad points) or `ZOOM_PER_LINE = 0.1` (wheel lines), the
      exponent limited to ±`ZOOM_STEP_LIMIT = 8`; up zooms in.
  - `drawable_size` is the content area at the screen's density (`SurfaceMetrics::backing_size`: the logical size
    times the backing scale, rounded on each axis). `hover` is the world point under the pointer.
- `SessionThread`: the FL-014 session owner and one controller task on an Asupersync current-thread runtime, on its
  own thread.
  - Intents arrive on a bounded lane of `INTENT_CAPACITY` = 16. The main thread only `try_send`s.
  - Every intent gets exactly one `Status`, on a standard channel the main thread drains without blocking.
  - A resume uses `SessionHandle::start`, added to `flife-runtime` by FL-018. It reports `Running { run, from }` at
    once. A watcher task, owned by the controller and joined at close, reports the run's end as `Ended { run, at,
    why }`, and run numbers keep a late end from touching a newer run.
  - A pause reports `Paused { at }`. A step reports `Stepped { tick }` or `StepHeld { at, why }`. A refusal reports
    `Refused`, and close reports `Closed { at }`.
  - FL-021: `Intent::Inspect { organism, seen }` asks `SessionHandle::inspect_organism` and reports
    `Inspected { organism, seen, snapshot }`: `seen` is the source tick of the epoch it was selected on, unchanged,
    and the snapshot is the organism at the session's committed boundary, which its `tick` names, or `None` when
    the committed world no longer holds it.
  - FL-021: `Intent::DumpWeights { organism, seen }` asks `SessionHandle::organism_weights` and reports `Weights {
    organism, seen, weights }`: `seen` unchanged, and the organism's whole controller (`ControllerWeights`) at the
    session's committed boundary, which its `tick` names, or `None` when the committed world no longer holds it.
  - FL-022: `Intent::Intervene { stroke, seen }` submits the stroke's intervention as a `Command` for
    `Target::NextAdmissible`, stating the session's configuration hash, with identities counting on from the
    session's last command (`CommandBook::last_command`, read once as the session thread starts: 0 for a fresh
    session, the earlier sessions' last for a resumed run, so an identity is never reused). Commands a resumed run
    left unfinished get a watcher each as the thread starts, so their outcomes are reported as the session's own
    are. An acceptance reports `Submitted { command, stroke, seen, effective }`,
    `effective` being the committed tick whose boundary it applies at; a rejection reports `Refused` with the
    engine's error class for an invalid payload, or `wrong-session`, `revision-mismatch`, `late`, `queue-full`,
    `conflicting-reuse`, `stopped` or `closed`, and takes no identity. A watcher task, owned by the controller and
    joined at close, asks `SessionHandle::status` every `OUTCOME_POLL` = 20 ms and reports
    `Finished { command, outcome }` once the command is terminal: for `Applied`, the tick it applied at, the tick
    that published it and the energy it moved. A paused session holds the command until a step or a run reaches
    its boundary. At close, a pending command's watcher stops when the session refuses its status.
  - FL-024: `SessionThread::spawn_kept(owner, handle, Some(checkpoints))` keeps the session's run directory
    (`keep`). It publishes the founding checkpoint before it takes any intent (when the interval is not zero).
    A checkpointer task, owned by the controller, reads the committed boundary every `CHECKPOINT_POLL` = 50 ms
    through `checkpoint_with` and, once it is at or past the next multiple of the interval after the newest
    checkpoint, takes one (`flife_cli::Checkpoints::take`) at the boundary the session is at then. Closing stops it,
    pauses the session, publishes the closing boundary unless the newest holds it or a fault stopped the world,
    publishes a background checkpoint in flight, and suspends the session rather than closing it. Each
    checkpoint reports `Checkpointed { tick, epoch, sequence, sha256, bytes, retired, mode, paused_seconds }`; a
    failed one reports `Unkept { kind, detail }`, and no more are taken while the session runs on.
  - `close_reporting()` is `close()` that also returns every status not yet taken, those reported while it
    closed last (a run's end, a kept session's last checkpoints, the close); `Studio::close` puts them in
    `StudioClosed::statuses`.
- `Keeping` (FL-024): `Keeping::new(dir, config)` refuses a directory that holds anything (`flife_cli::vacant`) and
  admits the checkpoint (`flife_cli::admit`), creating nothing; `checkpoint_bytes()` is the checkpoint term the
  Studio's memory admission charges (`memory::admit_kept`); `open(world, config, session)` writes the
  configuration, opens the chunk store and catalog, and opens the session through `flife_cli::open_session`
  (journaled under `write-ahead-before-apply`). `Studio::open` asks `Keeping::new` before any surface opens.
  `Studio::kept()` names the directory and the admission. `Keeping::recovered(dir, config, admission)` keeps a
  directory `flife_cli::recover` reopened, with the admission its `Checkpoints` made; `Studio::resume` opens the
  session over the recovered world and command book through `flife_cli::open_session`, charges the same
  checkpoint term, and keeps the reopened `Checkpoints`, so the chain's sequences go on from the newest.
  `Studio::resumed()` is `Some(Resumed { checkpoint, skipped, tick })` for a resumed Studio: the checkpoint it
  restored (`None` when only the founding world and journal were there), the checkpoints skipped as invalid, and
  the tick it opened at.
- `brush` (FL-022): `Brush::{Energy, Removal, Material, Introduction}` and `Stroke`, a click's world-space payload.
  `preview(brush, viewport, logical, cells, pointer, supplies)` is plan §14.3's preview: `Preview::Stroke` with
  exactly the stroke `stroke` makes for a click at the pointer and, for a cell, that cell's world rectangle
  (`cell_region`), or `OffWorld` (no pointer or content area, or the margin), or `NoMaterial` (the material brush
  over the world with no material). `overlay::preview_line` says it: what a click adds or removes, in the model's
  energy units, and its region in world units, such as `a click adds 64.0 energy to cell 12, 30 (x 12.00 to
  13.00, y 30.00 to 31.00 world units)`, or for a removal `a click removes the organisms within 1.23 world units
  of the pointer: 50% of their energy to the field, the rest dissipated, none out of the world`
  (`[death] residual_energy_recycle_fraction`); figures carry three significant digits of the cell width or the
  radius. The removal preview names the pointer, not its coordinates, so moving within a cell, or with the removal
  brush, does not change the overlay's text and lay it out again. `preview_region(preview)` is the region the
  frame outlines on the world (`flife_manim::Region`): the cell's rectangle, or the removal's disc, and whether
  the stroke removes energy (a removal does: the share not recycled to the field is dissipated); none without a
  stroke.
  `stroke(brush, viewport, logical, cells, at, supplies)` maps the logical point `at` through the viewport to the
  world point under it, `None` in the letterbox margin or while the content area is empty:
  - energy: `Stroke::Energy { cell, amount: BRUSH_ENERGY }` (64, 32 founders' endowments), the field cell whose
    square holds the world point, `⌊x / (L / cells)⌋` on each axis; `Intervention::InjectEnergy`;
  - removal: `Stroke::Removal { center, radius }`, centred on the world point, `BRUSH_RADIUS_POINTS` = 12 points on
    screen at the drawable's density and the view's zoom, at most half the shorter world side;
    `Intervention::RemoveOrganisms` with `Disposition::Recycle`, so removed energy goes where a death's does;
  - material: `Stroke::Material { cell, amount: BRUSH_ENERGY, direction }` in the same cell, `direction` being
    `Supplies::material`, or no stroke without one; `Intervention::InjectMaterial`. The Studio supplies the selected
    organism's `material_for(affinity)`: the unit direction `a / ‖a‖₂`, shrunk as source directions are until
    `‖·‖₂² ≤ 1` holds as the engine sums it (`flife_field::sources::unit_direction`). Under `bounded-affinity-v1` a
    packet of composition `z` converts at `η_max tanh(k a·z − b)`, which over `‖z‖₂ ≤ 1` is largest along `a`, so
    the brush adds the material that organism converts best; the cell's composition moves towards it;
  - introduction: `Stroke::Introduction { cell, count: BRUSH_INTRODUCTION, energy }` in the same cell, 8 organisms
    drawn as founders, each with `Supplies::founder_energy` (`population.initial_energy`);
    `Intervention::IntroduceOrganisms`.
- `Studio` is the main-thread host. It opens the window at `PixelDensity::Backing`: the window keeps its size in
  points, and each frame fills its device pixels (FL-020). Each `frame()`:
  1. drains typed input through `Controls` and offers the resulting intents;
  2. drains statuses;
  3. refits the `WorldView` when the content area or the screen's density changed. The view's pixels per point is
     the backing scale, so sizes given in points (dot floor and ceiling, the selection ring, density bins, the pick
     radius) look the same on any screen;
  4. takes the controls' style if it changed;
  5. applies the frame's camera moves in order to a copy of the viewport: a zoom about the pointer's drawable
     position, a pan by the drag times drawable pixels per point, a reset to the fit; a click picks among the shown
     epoch's organisms through the view as it stood at that click, within `PICK_RADIUS_POINTS = 8` logical points
     (scaled to drawable pixels); then takes the view and the selection. A click that selects an organism offers
     `Intent::Inspect` for it with the shown epoch's tick as `seen` (FL-021), counted with the frame's intents or
     refusals. After the frame's moves and strokes, a `CameraMove::Weights` offers `Intent::DumpWeights` for
     whatever is then selected, with the shown epoch's tick as `seen` (FL-021), and nothing when nothing is
     selected. A brush click makes its stroke through the view as it stood at that click, over the shown epoch's
     field grid, and is offered as `Intent::Intervene` with the shown epoch's tick as `seen` (FL-022), counted in
     `FrameTally::strokes` and with the intents or refusals. A brush click that makes no stroke (the margin, or the
     material brush with no material) is counted in `FrameTally::unstroked`. The material is the one the last
     `Inspected` snapshot gives, while that organism is still selected. A `CameraMove::Track` starts tracking or
     stops it; a pan or a reset stops it;
  6. if the feed's latest epoch is newer than the one shown, releases the prior lease, leases the newer epoch and
     makes the shown one its prior: the display then moves from the prior to it (`WorldView::show_between`), at the
     fraction the `Pacer` gives, and the selection stays only if the newer epoch has its identity drawn; after a
     style change it shows the pair again, after a camera move it places it, and otherwise, while the display
     moves, it advances it. While tracking (plan §14.3's camera-follow target), it then centres the view on where
     this frame draws the selected organism (`WorldView::selected_position`), at the zoom it has, and places the
     pair again there (`place_between`; the timeline records a `View` and a `Place`), counted in
     `FrameTally::tracked`: a tracked frame lowers its organisms twice. Nothing selected (a click on nothing, or
     an epoch that no longer draws the organism) stops tracking. Once the display rests the prior lease is
     released;
  7. renders one `ThreeDJob` with `render_three_d` straight into the window's drawable, with no pixel readback.
     Offscreen, it renders the job with `MetalRenderer::render_three_d_rgba8`, which runs the same dispatch to
     the route's RGBA8 surface and reads it back (`Studio::last_frame`, tight RGBA8 at the drawable's size; the
     last frame is dropped before the next is read), counted in `FrameTally::rendered`.

  Offscreen, step 1 drains what `Studio::post_input` posted since the last frame: the first frame drains the
  surface's `Resized` geometry and `FocusGained`, as a key window's first poll does, and a posted `Resized`
  becomes the surface's geometry. Past `INPUT_QUEUE_CAPACITY` undrained events a post is dropped and counted, as
  the native queue drops events. `post_input` on a window, and `post` (AppKit's synthetic input) offscreen, are
  `StateConflict`.

  It returns a `Frame` (state, events, intents, refusals, statuses, refit, restyled, moved, tracked, tracking, new
  epoch, shown
  `Prior`, the `Motion` or `None` at rest, selected organism, hover, armed brush, the armed brush's `preview`
  (`brush::preview` at the pointer through the view as the frame left it, with the supplies a click there would
  use, its `preview_region` outlined in that frame's drawing), outcome, and `drawn`: presented
  or rendered offscreen; the outcome is `None` offscreen). `FrameTally::drawn()` is the frames presented or
  rendered. `display_pair()` is the epochs it
  holds and `pacer()` the pacer.
- `Pacer` times the display's motion (§13.5, one-state display latency): `arrive(now)` for an epoch of a running
  stream moves the smoothed interval between arrivals by `ARRIVAL_SMOOTHING = 0.25` of the difference, and a wait
  of `MOTION_MAX = 100 ms` or more restarts it at `MOTION_MAX`; `arrive_alone(now)`, for an epoch that arrives
  while the controls are not running (a step), sets it to `MOTION_MAX`; `fraction(now)` is the time since the last
  arrival over the interval, at most 1, and 1 before an interval is known. So a running world is drawn about one
  arrival interval late, and a step is seen moving within 100 ms. `close()` closes the window and the session and returns the committed world (`StudioClosed`).
- `flife-studio <config> [--width W --height H] [--fixed] [--cpu-workers W] [--controllers C] [--color M]
  [--look glow|raw] [--density] [--no-field] [--field-mapping energy|composition] [--offscreen SECONDS [--scale N]
  [--offscreen-zooms]] [--run DIR]`, or `flife-studio --resume DIR [those options but --run]`: the
  binary. `--resume` takes the run's own configuration from DIR, so it takes no `<config>`, and it conflicts with
  `--run`, `--export` and `--timeline`; a configuration given with it exits 2 (usage) and prints nothing. With `--offscreen` it opens no window: it runs its loop offscreen for SECONDS wall seconds at N pixels
  a point (2 by default) and closes, and nothing posts input but `--offscreen-zooms`' camera moves: in every
  third frame interval, at a moment drawn uniformly over it (a fixed seed), a 4-point scroll at the content's
  centre, zooming in and out in turn; a move arriving during a frame's work waits for the next frame, as the
  native queue holds it. Frames start on a fixed schedule `1 / frames_per_second` apart: a wait that overshoots
  (macOS adds slack to a sleeping thread's timer) shortens the next, and a frame that runs past its slot starts
  the schedule again from its end. M is `lineage`, `energy`, `generation`, `motion` or
  `topology` (or a registered name such as `founder-lineage-v1`); the default is the configuration's
  `[presentation] color_mapping`. C is
  `cpu`, `torch`, `metal` or `auto` (the default), as for `flife run`: `auto` takes the Metal controllers only at W ≤ 8, and on a
  host without Metal `torch` at W ≤ 4. The Studio's default W is one per core up to 16, so on a host with more
  than 8 cores (or more than 4 without Metal) the controllers run on the CPU reference by default. With 16 workers Metal
  measured slower in the Studio under FrankenManim `dfdc7d18` (72 against 119 ticks per second, one 12 s run each),
  with presentation on the same GPU, and level under `f401a947` (132 against 120 at 960×540, 152 against 161 at
  1,920×1,080, four interleaved 10 s runs each). It paces frames to `[presentation] frames_per_second` and prints `frankenlife.studio.v1` NDJSON records:
  - `open`, naming the window, the CPU workers, the controller backend, the style (`mapping`, its `meaning`,
    `look`, `particles`, `field` and `field_mapping`), `clock_telemetry`, the configuration's `[presentation]
    show_clock_telemetry`, the `surface`, `window` or `offscreen`, and with `--run` the `run_dir` and its
    `checkpoints` admission (declared and admitted mode, retained bytes, peak, headroom, fallback), the peak
    charged as the memory admission's `checkpoint`; with `--resume` the same, and `resumed` (`checkpoint` with its
    `sequence` and `tick`, or null; `skipped`; the `tick` it opened at), null otherwise;
  - FL-024, with `--run` or `--resume`: one `checkpoint` per checkpoint published (`tick`, `epoch`, `sequence`, `sha256`,
    `bytes`, `retired`, `mode`, `paused_seconds`), and a `status` `unkept` if one fails;
  - one `status` per status;
  - one `view` per style change, with the same style fields;
  - one `selection` each time the selected organism changes (a click, or the organism gone from a newer epoch):
    its identity or null, its position and the shown tick;
  - one `inspected` per inspection answered (FL-021), on a click's selection and about once a second while it
    stays selected and the world runs (`Studio::inspect_selected`): the organism, `seen_tick`, the committed `tick` it was read
    at (null when the world no longer holds it) and a `snapshot`: genome, parent, lineage, generation, birth tick,
    age, position, velocity, energy and its change rate, phase, cooldown, topology, hidden widths, parameter count,
    sensing scale, motion response, affinity, `local` (what the committed field offers it where it stands: its
    exchange footprint's `cells`, each with its `cell` index, `weight`, `energy` and `efficiency`; the footprint's
    `density`, A.1's `request` there, and the `gain` and `toxicity` per unit taken), and the decoded held output
    (attempt rate and probability, child
    fraction, emission gate, emission direction and spend fraction) or null; `sensed` (its step's start `tick`,
    `scale`, `evaluated`, `raw` as the three planes `R, M_0, …, M_31`, x and y derivatives, the 104 `inputs` and
    the 33 `gradients`) and `activations` (the output's `tick` and each `hidden` layer's values) or null; and
    `transactions`, what its recorded steps add up to as the panel sums them (`flife_studio::recent`): `steps`, the
    first and newest steps' `ticks`, chemistry's net `uptake`, the `maintenance`, `motion` and `emission` paid,
    `births` and what their splits debited (`endowed`), the `net` change, and the newest step's `reproduction`
    (`cooling-down`, `no-attempt`, `unviable`, `born` or `rejected`), or null; and `descendants`: the tick its
    counts run `since`, its `children_born` and `children_living`, its descendants `born` and `living`, the
    `ancestors` its line passes through, nearest first and at most `flife_engine::ANCESTRY_PATH` (8), each with
    its `organism`, `generation`, `birth_tick`, whether it is `alive`, and its descendants `born` and `living`, and
    how many more lie `beyond` them, or null when the world traces no ancestry;
  - one `weights` per weight dump answered (FL-021, plan §14.2's on-demand weight dump, on W): the `organism`,
    `seen_tick`, the committed `tick` it was read at and its `controller` (null, with the tick, when the world no
    longer holds it): `genome`, `topology`, `parameters` and its `layers` as `flife inspect --weights` writes them
    (`flife_cli::InspectedWeights::of`): `inputs`, `outputs`, `weights[out][in]` and `biases[out]`, each `f32` as
    stored. The preset's largest controller writes out as a few hundred kilobytes on one line;
  - FL-022: one `brush` each time the armed brush changes (`energy`, `removal` or null); one `submitted` per
    accepted stroke: its `command`, `brush`, its payload (the `cell` and `amount`, with the material's 32-component
    `direction`; the `center` and `radius`; or the `cell`, `count` and each one's `endowment`), `seen_tick` and
    `effective_tick`; and one `finished` per command: `outcome` `applied` with its `tick`, `published_tick` and
    `energy`, or `cancelled` or `failed` with a `detail`;
  - a `frame` summary about once a second, with the frames `presented` and `rendered` offscreen, the camera's
    center and zoom, and the bytes the Metal route has copied so far: to the device (`upload_bytes`, and the
    most in one frame, `peak_upload_bytes`) and back to the host (`readback_bytes`, none for frames presented to
    the window);
  - FL-024, while `show_clock_telemetry` is set: a `clock` record after each `frame` summary but the first, over
    the wall time since the previous one (`wall_seconds`): the `ticks` the session committed in it and
    `ticks_per_second`; `simulation_rate`, simulated seconds per wall second (the ticks times `Δt`, 1 being real
    time, 0 while paused); the `frames` drawn (presented, or rendered offscreen) and `frames_per_second`; and the
    newest `committed_tick`, the
    `shown_tick` and how far the shown epoch is behind it (`behind_ticks`). The committed tick is the newest epoch
    in the render feed (`Studio::committed`), so a slow world reads slow. It also carries the controller
    `evaluations` the shown epoch's tally gained over the window and `evaluations_per_second`;
  - FL-024, plan §14.1: a `telemetry` record after each `frame` summary (and its `clock`), from the shown epoch:
    `shown_tick`, `live`, `parameters`, `genome_logical_bytes`, `genome_allocated_bytes`, `headroom_bytes` (the
    budget less the admitted peak), the tally's `counted_from_tick`, `births`, `introductions`, `deaths`,
    `evaluations`, `rejected_births` and `uptake` (null without a tally), the ledger's `injected`, `removed`,
    `dissipated` and `residual`, `faulted`, the `capacity_pauses` reported, and `full_evaluations_per_second`, the
    clock's evaluations a second over the live population (null without a counted clock or a live population);
  - the statuses reported while the Studio closes (a run's end, with `--run` the closing checkpoint, the close),
    then `end`, with the committed tick, the world digest, the newest checkpoint's tick (`checkpoint_tick`, with
    `--run`), the strokes offered and the brush clicks that made none
    (`unstroked`), the controller evaluations the backend
    returned and deferred, and its failure if it failed, and the same copy totals; `frame_times` (FL-024's G6
    evidence: `FrameTimes`' median, 95th and 99th percentiles and longest, in milliseconds, of each frame's work
    from draining input to the overlay set, without the wait for the next frame); and with
    `--offscreen-zooms` the `camera_moves` posted, those whose frame did not move the view
    (`camera_moves_unmoved`), and `camera_latency`, each move's time from its arrival to the end of the frame
    that drew it, as the same percentiles.
- `FrameTimes` counts frame times in 0.1 ms buckets (`FRAME_TIME_BUCKET`) up to one second
  (`FRAME_TIME_RANGE`), longer frames above it, and keeps the longest exactly: fixed memory for a run of any
  length. `percentile(p)` is the upper edge of the bucket holding the `⌈p · frames⌉`-th shortest frame (at most
  the longest), and the longest when that frame took the range or more.

  Errors are `frankenlife.error.v1` on standard error. The exit code is 0 after a clean close, 2 when the
  configuration or window fails, and 1 after a later failure.

**Invariants.**
- *Main-thread presentation (§13.6).* Window creation, event polling, presentation and close happen on the process
  main thread. Nothing on it waits for a tick, a command or the session's lanes: intents use `try_send`, statuses
  use `try_iter`, and epochs come from the lock-free feed.
- *Observation neutrality.* The main thread reads the world only through render epochs. It holds at most
  `DISPLAY_PAIR` = 2 leases: the shown epoch, and while the display moves, its prior; at rest, one. Input reaches the
  world only as session intents: camera moves, selection and the display's motion never do, and a brush click
  reaches it only as a command the session admits and applies at a committed boundary (FL-022).
- *Committed states only (§13.5).* The display is drawn only between two committed epochs of one world, by
  identity: a birth appears with the first epoch that has it, a death fades and is never picked, an ambiguous gap
  snaps, and a prior that is not an earlier state of the same world is not drawn between.
- *Selection by identity (§13.6).* A click picks the displayed epoch's stable identity; each newer epoch shown
  revalidates it by identity and drops it when absent, never reinterpreting a stale hit as its slot's new
  occupant.
- *Bounded everything.* The presenter queue holds 1024 events, the intent lane 16, the viewer two leases (below the
  reference preset's three render slots, so the publisher always has one), the cloud its ceiling, and the main
  thread's event buffer is reserved once at 1024 events. An intent refused because the lane
  is full is counted, and a refused resume leaves the controls paused.
- *Capture safety.* Losing focus with buttons held delivers `PointerCaptureLost` before `FocusLost`, and the controls
  clear the held buttons.
- *Same science.* A live session's committed world equals the same configuration stepped directly to the same tick,
  with the same interventions at the same ticks.
- *No unsafe.* The crate is `forbid(unsafe_code)`. All AppKit and Core Graphics FFI is in frankentorch's reviewed
  presenter.

**Refusal cases.**
- `Studio::open` for a window off the main thread, without Metal or without a window server, or offscreen without
  a Metal device: `MissingCapability`. A background that is not `#rrggbb`, or an offscreen drawable past
  `u32::MAX` pixels a side: `NumericalInvalid`. The founding and session errors of `World::found` and
  `flife_runtime::open`.
- A window that cannot be polled, rendered, presented or closed, or an offscreen frame that cannot be rendered or
  read back: `StateConflict`, carrying the presenter's or the renderer's error.
- The binary refuses (exit 2, before anything opens) an `--offscreen` that is not a positive, finite number of
  seconds, a `--scale` of 0, and `--scale` or `--offscreen-zooms` without `--offscreen`.
- `SessionThread::send` on a full lane: `BudgetExhausted`. On a gone thread: `QueueClosed`.
- The session's own refusals arrive as `Status::Refused` with their error class, a rejected stroke's included.
  The Studio does not check a stroke itself: the engine's validation refuses a cell off the grid or a disk wider
  than half the world (`numerical-invalid`).

**Determinism.** The session's science is the owner's (FL-014): deterministic for its configuration and the ticks it
commits, whatever the frame rate, focus or window size. Frame timing, coalescing and pointer positions are
presentation state, not scientific state.

**Independent tests.**
- `tests/controls.rs` (17 tests):
  - Space toggling and ignoring repeat; stepping only while paused, with repeat stepping; Esc and ⌘W closing once.
  - Buttons through presses, motion and capture loss.
  - Focus, geometry rounding, hover through a non-integral logical size, and the tally.
  - The style keys (FL-019): 1 to 6 choose the six mappings in order, R, D and F switch the look, particles and
    field, C the field's mapping (energy or composition, its code pinned to `kVK_ANSI_C`), and auto-repeat or ⌘
    with a digit chooses nothing.
  - Following the session: a late end of run 1 does not stop run 2, and run 2 stopping on its own does.
  - A resume refused at the lane or by the session leaves the controls paused.
  - FL-020: a drag pans by the pointer's whole motion (worked deltas) and selects nothing; a click within the slop
    selects at the press, while secondary presses, lost presses and buttonless motion make no move, and a drag
    lost to capture loss pans no further; scrolls zoom by `exp(rate · dy)` in both units, limited, with no move for
    a zero or non-finite delta; 0 resets once (not on repeat or with ⌘) and is no style key; 256 moves wait in
    order until taken.
  - FL-022: E, X, M and I arm, switch and disarm the brushes (their codes pinned to `kVK_ANSI_E`, `kVK_ANSI_X`,
    `kVK_ANSI_M` and `kVK_ANSI_I`), not on repeat or with ⌘, with no move, intent or style change; an armed click
    brushes at the press with the brush armed at the release, a drag still pans, and once disarmed a click selects.
  - FL-021: W makes one `CameraMove::Weights` and no intent, not on repeat; ⌘W closes and makes no move; it is no
    style key.
  - Plan §14.3: each press of T makes one `CameraMove::Track` and no intent (its code pinned to `kVK_ANSI_T`), not
    on repeat or with ⌘; it is no style key.
- `tests/pacing.rs` (5 tests, FL-020): at rest before an interval is known; the display moves over the interval
  between arrivals (worked fractions, capped at 1, a frame before the arrival at 0) and later intervals smooth by a
  quarter; a five-second wait counts as `MOTION_MAX`, as does a wait of exactly `MOTION_MAX`, after a fast stream
  too; a step alone moves over `MOTION_MAX` whatever came before and smooths down once epochs flow; arrivals with
  no time between leave the display at rest.
- `tests/live.rs` (harness-free, 31 checks when last run, before FL-022 added three; `--features synthetic-input`):
  the real 4,096-organism reference preset in a real 640 × 400 window. Every input is a real NSEvent or Core Graphics event posted to the application queue
  and delivered through the presenter's typed bridge:
  - the first poll's real `Resized` (640 × 400 points), and a real `FocusGained` (made key by a real key-window
    change if another application holds the keyboard);
  - FL-020: frames drawn at the screen's density (1280 × 800 pixels on a 2× display), the window kept at 640 × 400
    points, the view at the backing scale in pixels per point, and logical points mapped to the drawable at that
    ratio. Every later position is a drawable one, posted at its logical point;
  - the founding epoch on screen as 4,096 dots in one cloud, reserved for the 16,384 ceiling, in four scene
    objects (the field, the cloud, the density layer and the selection ring);
  - Space runs, and live epochs reach the screen;
  - Space pauses at a reported boundary, where the display comes to rest holding only that epoch's lease, and no
    epoch arrives while paused;
  - → and `.` each step one tick; FL-020: steps are shown one committed state late. Each arrival frame draws every
    organism at its previous committed position, and every moving frame draws exactly what `interpolate` places
    between the two epochs the Studio holds, at the frame's own fraction (part-way frames required). The `.` is
    pressed as soon as the → step is on screen, so it arrives while the display still moves, and is paced over
    `MOTION_MAX` all the same (a running-stream pacing would give less); the display rests on the `.` step with the
    prior lease released;
  - while paused: 2 recolours the shown dots by energy; R drops the halos; D draws density on the Metal route, its
    bins counting every organism; 1, R, D and F restore the lineage colours bit for bit, and no restyle steps the
    world or changes the shown tick;
  - a real resize to 800 × 450 points refits the viewport at the screen's density (1600 × 900 pixels and 7.03125 px
    per world unit on a 2× display) without new draw storage, and the next frame is presented at the new drawable
    size with the window at 800 × 450 points;
  - real pointer motion hovers the right world point within a pixel, and a real scroll arrives;
  - focus loss with a button held loses the capture and clears the drag, and focus regain arrives;
  - FL-020, all real events: a scroll up zooms (e^0.5 for 50 precise points) about the pointer, which stays over
    its world point within 1e-9, and the paused epoch is placed again, every dot at its scene point in the new
    view; a primary drag of (40, 25) points keeps the grabbed world point under the pointer and selects nothing; a
    click selects the organism drawn under it, by identity (the oracle is `pick` over the shown rows), with the
    ring as the last draw; a step keeps the selection exactly when the new epoch has the organism; a click on an
    empty spot deselects; 0 restores the fitted viewport bit for bit;
  - no intent refused and no input dropped;
  - FL-022 (added with the brushes, compiled but not run here, so not yet evidence): E arms the energy brush; a
    real click is `Submitted` as command 1 with the stroke `brush::stroke` makes for that point through the fitted
    view, for the boundary of the paused tick, and selects nothing; a step reports it `Applied` there, published at
    the next tick; E disarms;
  - Esc closes at the last stepped tick (four steps after the pause);
  - the committed world's digest equals the directly stepped world's, given the stroke at its tick, which moves
    the same energy bit for bit.
  - Off macOS it reports itself skipped.
- In `flife-runtime`, `tests/session.rs` checks `SessionHandle::start` across six Lab seeds: the start comes back at
  once, and the end comes back for a pause, a reached tick and a close.
- `tests/inspect.rs` (3 tests, FL-021, no window): a `SessionThread` over a 32 x 32 preset world, driven through
  its intent lane on the native runtime:
  - paused after four steps: eight organisms and an unknown identity inspected, each answer in order, `seen`
    unchanged (3), the snapshots the direct world's at tick 4, none for the unknown one; the closed world the
    direct one;
  - while running: 48 inspections, then a pause; every snapshot the direct world's at its own tick, every
    organism answered with none dead at the close, and the closed world the direct one. Which ticks the answers
    land on depends on thread timing; the Lab test in `flife-runtime` is the one that requires answers mid-run;
  - paused after four steps: six organisms' and an unknown identity's weight dumps, each answer in order, `seen`
    unchanged (3), each the direct world's dump at tick 4, none for the unknown one; the closed world the direct
    one.
- `tests/brush.rs` (10 tests, FL-022, no window):
  - the preview at every point of the lattice, for every brush with and without a material, at fit, zoomed 2.75
    and across the seams, is `Preview::Stroke` of exactly `stroke`'s stroke there, a cell's region holding the
    world point, outlined (`preview_region`) as that cell, or the removal's disc, adding energy except for a
    removal; the material brush over the world without a material is `NoMaterial`, and the margin, a missing
    pointer and a missing content area are `OffWorld`, neither outlined;
  - an energy click names the cell whose square holds the world point under it (the oracle is the cell's bounds,
    on a 64 x 16 grid of non-square cells over a lattice of clicks), at fit, zoomed 2.75 and centred on the seams
    at zoom 2; both ends of both axes are reached, either side of the seams gives the last cells and the first,
    and the margin, an empty content area and an empty grid make none;
  - a removal click is centred on the world point under it with the world distance to the point 12 points to its
    right as its radius (within 1e-12) at zooms 1, 4 and 13.5, a quarter at zoom 4 of zoom 1's, held to 16 in a
    16-point window over 32 world units, and none in the margin;
  - the material and introduction brushes name the energy brush's cell under every click of a lattice at zoom 1.7;
    the material brush carries the supplied composition and makes nothing without one, and the introduction brush 8
    organisms at the supplied endowment;
  - 24 founders' materials are within the unit ball as the engine checks it, unit to within 1e-14, parallel to
    their affinities (so the unit composition with the largest `a·z`); a zero or nonfinite affinity has none;
  - the payloads are exactly `InjectEnergy`, `RemoveOrganisms` with `Disposition::Recycle`, `InjectMaterial` and
    `IntroduceOrganisms`;
  - a `SessionThread` over a 32 x 32 preset world paused at tick 2: an energy stroke (seen at 1) and a removal
    about a living organism are `Submitted` as commands 1 and 2 for the boundary of tick 2 and wait there through
    ten polls; the next step reports both `Applied` at tick 2, published at 3, with the energy a direct world
    stepped with both interventions at tick 2 reports, bit for bit; the organism is gone, and the closed world is
    the direct one;
  - while running: 16 strokes, then a pause; every stroke applied at the tick it was submitted for and published
    at the next, over more than one boundary; a direct world stepped with each tick's strokes in command order
    reports every energy bit for bit and ends on the closed world's digest. Which ticks they land on depends on
    thread timing;
  - a cell off the grid and a disk wider than half the world are refused as `numerical-invalid` and take no
    identity; a stroke still pending at close never applies, and the close returns with the founding world;
  - paused at tick 3: a material stroke of a living organism's affinity direction and an introduction apply at the
    next step, each moving what the direct world stepped with both reports, bit for bit (the introduction 8 organisms
    at 2 energy each, 16), and the closed world is the direct one.
- `tests/clock.rs` (8 tests, FL-024, no window), the `ClockMeter` the binary reads:
  - evaluations (plan §14.1): from 1,000 to 5,096 over 2 s read 4,096 and 2,048 a second; a window with no count at
    either end reads none; a still count reads 0;
  - the first read opens a window and gives nothing; 128 ticks in one second at `Δt = 1/128` read 128 ticks/s,
    rate 1 and the frames' 60 fps, 8 ticks behind;
  - each window starts where the last ended (64 ticks/s and rate 0.5, then 1,024 and 8); the ratio uses the world's
    own timestep (40 ticks of 3/40 s in 2 s are 1.5); a paused world reads 0 ticks and rate 0 with its frames
    still counted; a window of no wall time, or a read before it opened, gives nothing and keeps its start;
  - against a running `SessionThread`'s render feed: over 300 ms the ticks read are exactly what the feed's newest
    epoch advanced, at that over the window's wall seconds and `Δt` times it, and the evaluations what its tally
    advanced (at least one a tick, as the preset evaluates every organism every tick); once paused a window reads
    0 ticks and 0 evaluations.
- `tests/overlay.rs` (8 tests, FL-024, FL-022, FL-021 and FL-019, no window):
  - each preview's line exactly: the energy, material and introduction brushes' cell and bounds, the removal's
    radius and recycled share, three significant digits of a fine grid's cell (`250.000 to 250.250`) and of a deep
    zoom's radius (`0.000123`), `1.23e5` from 100,000, 100% and 0% recycled, off the world and no material; in
    `overlay_text`, after the brush and the selection;
  - plan §14.1's telemetry exactly, for chosen metrics: `4,608 live · 9,124,000 parameters`, `controllers every
    tick · 128.0 full evaluations/s`, `genomes 38.2 MiB of 1.00 GiB · 412 MiB headroom`, `18,586 births · 18,255
    deaths · no capacity pause · no fault`, `injected 1234.5 · uptake 456.7 · dissipated 789.1 · residual
    1.2e-12`, `field-ecology-v1 · seed 42 · no checkpoint`, between the clocks and the key in `overlay_text`; a
    cadence of 4 ticks, no rate without a counted clock or a live population; bytes from `512 B` to `16.0 EiB`
    and no headroom below `0 B`; introductions and removal shown only when present, a negative residual; a tally
    from tick 512 says so on both its lines, and none says `births and deaths not counted` and leaves uptake out;
    one and 2,345 capacity pauses, a fault, scientific notation from 100,000 and a zero residual; a checkpoint's
    tick;
  - `overlay_text` exactly: `opening` before the session reports; `running · tick 1,024`, then `128 ticks/s ·
    1.00× real time · 60 fps`, then the key; one decimal below 100 ticks/s; the brush and the organism on one line,
    grouped in thousands; `999,999,999,999`, then `1.00e12` and `1.84e19` from a trillion, and a clock of
    `1.23e8 ticks/s · 2.50e5× real time`;
  - `legends` of the preset gives each of the six mappings once, in `ColorMapping::ALL`'s order; each key's text
    starts with its mapping's name and has one swatch for each `■`, and at 2x it is set whole, every glyph drawn
    and every `■` coloured by its swatch in order; the topology key names the preset's five members (`8`, `32`,
    `64, 32`, `96, 64`, `128, 96`);
  - the field's key (plan §13.4's "Local field"): the energy field has none; the composition's is exactly `field
    composition ■ none ■■■■■■ as affinity, darker is less energy`, its swatches the affinity key's colours; in
    `overlay_text` it is the line after the colour key's, before the brush, and at 2x it is set whole with its
    swatches coloured after the colour key's; `field_key` (the binary's choice) gives it for a drawn, composed
    field in either look and either particles, and nothing while the field is hidden or under raw density;
  - the inspector panel exactly, from a real snapshot of the preset three ticks in, its organism watched from the
    start, with every printed value set: its thirteen lines under the selection's, `not in the committed world` for
    a gone organism, nothing without a selection; headings of 0°, 90°, 180° and 270° counter-clockwise on screen
    from east (a heading a hair below east reads 0°, not 360°, and one of -0 reads 0°), `still` at rest;
    scientific notation from 100,000 with a sign on every rate; what the field offers it where it stands (`here:
    η -0.120 to +0.450 · gain 0.225 · toxicity 0.012 a unit · asks 0.0123 a tick`, and `η +0.450` alone when the
    footprint's cells agree); `no controller output held`; what it sensed
    (`sensed at tick 5,119 · scale 1.50 · field 0.412`, `gradient 0.031 toward 45° · affinity·Z +0.112`, the
    composition inputs set along its own affinity so the dot product is known) and its activations
    (`activations at tick 5,120 · max |a| 0.98, 0.97 · mean |a| 0.40, 0.53`), then its transactions over the
    snapshot's three recorded steps (`last 3 steps: uptake +0.570 · maintenance -0.012 · motion -0.003`, `emission
    -0.120 · 1 birth -1.050 · net +1.214 · cooling to tick 5,200`: each stage's sum, signed by whether it adds or
    removes energy); a gradient toward 180° with a negative alignment, `no gradient`; one step (`last step`, `no
    births`), two births, and each other reproduction outcome (`bore organism 4,321`, `attempt unviable`, `3
    candidates rejected`, `no reproduction stage`); `sensed from its next tick`, `activations at its next
    evaluation` and `transactions from its next tick` before anything is recorded; and its descendants (`children
    3 born, 2 alive · descendants 41 born, 17 alive`, then `ancestors 455 (gen 41, 204 alive) ← 301 (gen 38,
    dead, 1,022 alive) ← 5 more`: the panel's two nearest, each with how many of its descendants live, and the
    path's other two with the three beyond it), counted `since tick 1,024` with two ancestors and nothing more,
    one ancestor, `ancestors: none alive or where lines meet`, and `descendants not traced`;
  - the longest overlay (every key with every brush, every count one short of a trillion, the panel with every
    value one short of scientific notation and the catalog's largest controller, and the telemetry at its widest:
    four-digit byte counts, energies one short of scientific notation, the widest residual, a tally from a later
    tick, a fault, the largest seed and cadence, and each brush's widest preview: its own amount and count, the
    largest cell indices, bounds and a founder's endowment one short of scientific notation at nine places, and
    the finest radius, the composition's field key, what the field offers the organism where it stands with its
    efficiencies at the width of their bounds and its toxicity and request one short of scientific notation, what
    it sensed and its activations, each decimal one short of scientific notation and negative, a three-digit
    direction and two layers at the largest magnitude, and 128 steps' transactions, each sum one short of
    scientific notation and negative, 127 births and the newest cooling down to the largest tick written in full,
    and its descendants: every count one short of a trillion, counted since the largest tick written in full,
    two dead ancestors at the largest generation, and one short of a trillion more past them), 25 lines laid out
    with its keys' swatches, at 2x: every glyph is drawn, none refused or left out, within four fifths of the
    overlay's budget (the motion key's with the introduction brush, 54,110 vertices and 52,402 triangles, is the
    longest); at 3x, all of it is still drawn.
  - Planted bugs in the transactions' panel and record (FL-021), each caught: the uptake summing the gain alone,
    births charging only the child's endowment, the net change the newest step's, the first step's reproduction
    shown, maintenance shown as a gain, the record's uptake including maintenance, and its ticks one late.
  - Planted bugs in the local chemistry's line and record (FL-021), each caught: the efficiencies written high to
    low, a range for agreeing cells, the request at three places, the gain shown as the toxicity, the line after
    the controller's, the record's gain the toxicity, a cell's weight recorded as its efficiency, the record's
    request the density, and one cell recorded.
  - Planted bugs in the descendants (FL-021), each caught: the host tracing no ancestry (`tests/offscreen.rs`); a
    dead ancestor not written dead, the panel's count of more leaving out those beyond the path, and three
    ancestors named (`tests/overlay.rs`); and the record dropping `beyond` or naming the path farthest first
    (`src/main.rs`). With the engine's, 23 of 23.
  - Planted bugs in the weight dump (FL-021), each caught: W making no move (`tests/controls.rs`,
    `tests/offscreen.rs`), the host never offering the dump (`tests/offscreen.rs`), the session thread answering
    no controller or dropping the shown tick (`tests/inspect.rs`), the record's tick the shown one and its layers
    reversed (`src/main.rs`), and the CLI's layout naming fan-in as fan-out (`flife-cli`'s `tests/inspect.rs` and
    `src/main.rs`). With the runtime's, 10 of 10.
  - Planted bugs in camera tracking (plan §14.3), each caught: T making no move (`tests/controls.rs`); T never
    stopping it, a pan, a reset or losing the selection not stopping it, tracking at zoom 1, the frame never
    reporting a tracked move, and centring on where the previous frame drew the organism (`tests/offscreen.rs`);
    and the pair not placed again once centred, the tracked move or its placing not recorded, and no tracked
    frame counted (`tests/timeline.rs`, whose replay then draws other pixels or counts). 12 of 12.
- `tests/admission.rs` builds its views as `Studio::open` does, with the overlay. The overlay adds exactly its view
  terms to the observers: 139.4 MiB at 3840 x 2160, which leaves the backend 114.3 MiB of the 2 GiB budget (95.5
  with the preset's bloom). Most of it is the frame preparation each of the HUD's triangles is charged, twice: the
  overlay was 32.5 MiB (backend 222.1 MiB) at caps of 15,360, 71.5 MiB (183.1) at 33,792 with the persistent
  telemetry, 82.6 MiB (172.1) at 39,936 vertices and 38,912 triangles with the brush preview, 86.9 MiB (166.8) at
  41,984 and 40,960 with the field's key, 99.9 MiB (153.8) at 48,128 and 47,104 with the inspector's sensed and
  activation lines, 111.0 MiB (142.7) at 54,272 and 52,224 with its two transaction lines, 117.5 MiB (136.2) at
  57,344 and 55,296 with the line for what the field offers the organism where it stands, and is 139.4 MiB at
  68,608 and 65,536 with its two lines of descendants and ancestors (FL-021). Each line the HUD can show costs
  the backend about 11 MiB. The field's compositions (12 bytes a texel in the feed's slots and the
  view's copy, FL-019) add 0.8 MiB to the rest of the observers.
- `tests/frame_times.rs` (4 tests, FL-024): `FrameTimes` against a sorted list of the same times: none before a
  frame; 1 to 100 ms give the 50.1, 95.1 and 99.1 ms bucket edges, the longest at 1, the next rank between
  ranks, and none outside (0, 1]; frames at and past the range are counted and the longest stands for them, a
  frame a nanosecond under it in the last bucket; on 20,000 seeded times (5 to 30 ms, a few long, some past the
  range) every percentile from 0.001 to 1 is within one bucket above its order statistic, or the longest.
- `tests/offscreen.rs` (8 tests, FL-024, FL-022, Metal compute, no window), `Studio::open` offscreen on the preset at
  320 x 200 points and 2 pixels a point:
  - the first frame drains the geometry and the focus, refits to 640 x 400 and is rendered, not presented, with
    the readback counted; the frame read back is byte for byte what a second `MetalRenderer` renders from the
    view's job, and not blank;
  - posted input as a window's: key 2 restyles to the energy mapping; a scroll zooms by exactly `scroll_zoom`'s
    factor about the pointer, its world point held within 1e-9; a click on the shown organism nearest the
    centre selects it (`flife_manim::pick`'s choice) and its inspection answers; a posted `Resized` becomes the
    surface's geometry and refits the view to 800 x 500, its frame that size; Space runs the world on to a shown
    tick of 8, lowering newer epochs (latest-value publication: under load a late frame lowers only the newest,
    so not one a tick); every frame was rendered;
  - FL-019's key on the Metal route: the generation and affinity keys' swatch interiors (2 px inside each square,
    over 1,100 pixels a key) are their sRGB colours exactly with the bloom off; with the preset's bloom (0.15)
    they were within 8 and 16 levels (printed);
  - a running world with a camera move arriving at a random moment of every third frame interval of a 60 Hz
    loop: every move reaches the next frame; frame work and arrival-to-drawn latency are printed, not asserted;
  - the queue keeps at most `INPUT_QUEUE_CAPACITY` events between frames and counts the rest as dropped;
  - FL-022's host brush path: a click selects the organism nearest the centre and its inspection answers; E and M
    arm the energy and material brushes (the frame names the armed one), and a click with each is the stroke
    `stroke` makes at that point through the frame's view, the material one of the selected organism's affinity
    direction; both are submitted as commands 1 and 2 for tick 0, seen at tick 0; a step applies both at tick 0,
    published at 1, each moving the energy, bit for bit, that a world stepped directly with the same two strokes
    reports; and the Studio closes on that world's digest, with two strokes and none unstroked. Before each click,
    with the pointer there, the frame's preview is that stroke, the material one's composition included;
  - FL-022's preview through the host: none before a brush is armed; off the world when armed before the pointer
    has moved; with the energy, removal and introduction brushes armed by key and the pointer moved over the
    world, the frame's preview is exactly the stroke `stroke` makes there through the view, a cell's region
    holding the world point under the pointer; the view outlines `preview_region` of it, and on the Metal route
    the pixel just outside the energy brush's cell is amber; a click there then submits that stroke (all three,
    in order); the material brush with no organism selected is `NoMaterial`, the margin `OffWorld`, and disarmed
    there is no preview; none of the three is outlined.
  - Plan §14.3's camera tracking: T with nothing selected tracks nothing; zoomed in, a click on an organism away
    from the centre with T centres the view on it at once at the same zoom (bit for bit), and at rest the view
    stays. While the world runs, every frame draws the tracked organism at the drawable's centre (within 1e-6 px)
    at that zoom, and only losing the selection stops tracking (the test then tracks the organism nearest the
    centre); 40 frames move the view, those between epochs among them (in one run, 40 of 40, none lost). Paused:
    a zoom about another point keeps it centred, at exactly the scroll's factor; T stops and starts it again; a
    drag stops it and leaves the organism where the pan took it (the drag's offset from the centre, in drawable
    pixels); T centres it again; 0 refits to zoom 1 and stops it; and a click where nothing is picked selects
    nothing and stops it.
  - Planted bugs in the region outline (FL-022), each caught: the host outlining nothing or not recording the
    region (this file, `tests/timeline.rs`), a removal outlined as adding energy or with half its radius
    (`tests/brush.rs`), the replay outlining nothing, and a recorded rectangle doubled (`tests/timeline.rs`).
  - Planted bugs in the preview, each run against the tests that should catch it: the preview made with another
    brush, a cell's region one cell on, no-material never said, the region left out (`tests/brush.rs`); the host
    previewing without the pointer, while disarmed, or without the selected organism's material (this file);
    the line left out of the overlay, the recycled share not a percentage, the introduction's total one
    organism's, figures one short of their scale (`tests/overlay.rs`): eleven caught, each by its own assertion.
    The binary's recycled share set to zero survives: no offscreen binary run arms a brush (its run failed once,
    on `tests/offscreen_binary.rs`'s floor of 10 frames in 2.5 s at a load average of 86, not on the mutant).
- `tests/offscreen_binary.rs` (1 test, FL-024, Metal compute, no window): `flife-studio --offscreen 2.5
  --offscreen-zooms` at 320 x 200 points, 2x, CPU controllers on 2 workers: `open` says `offscreen` and carries
  its admission, whose terms sum to its peak within the budget, with no backend term on the CPU reference, and
  whose observers are charged at least the device bytes of the 640 x 400 drawable's extra pixels more than a
  0.1 s run at one pixel a point; the world runs; every `frame` summary counts each frame rendered, none presented, and at least a 640 x 400 frame's bytes
  read back per frame; the `clock` records count frames; the `end` counts every frame rendered, no keys and no
  dropped input, a move in every third frame interval with none unmoved and every move but perhaps the last
  drawn, and ordered percentiles; and the world it closed on is the configuration stepped directly to its tick,
  digest for digest.
- `tests/telemetry_binary.rs` (3 tests, FL-024, FL-019, Metal compute, no window): `flife-studio --offscreen 2.5 --timeline
  T` on the preset at 32 x 32 with 384 founders that start above the birth threshold and pay 1.5 a second to live
  (in one run: through tick 763, 24 births, 361 deaths and 127,235 evaluations, the population alive). Each
  `telemetry` record equals the world stepped directly to its shown tick: the live count, parameters, quota bytes,
  every tally count, the uptake and the ledger's terms bit for bit, the headroom by the `open` record's admission,
  no fault and no capacity pause. Each `clock` record before one counts exactly the evaluations the telemetry
  gained since the last, and the record's full evaluations a second are the clock's rate over the live count.
  Every overlay T recorded has the telemetry lines of the direct world at its tick (the cadence line up to its
  rate, which depends on the clock's window). The records' shown ticks never go back, and advance over the run.
  - The same world with room for its 384 founders and no one more: its first birth pauses the run before it commits
    (in one run at tick 74, with no birth before). The binary's one `ended` status says `capacity-paused` at the
    tick the direct world pauses at; every `telemetry` record before it counts no capacity pause and every one
    after it one (in that run, none before and two after), the last shows the paused tick with no births; every
    overlay says `no capacity pause` or `1 capacity pause`, and the last says `1 capacity pause` and `paused`.
  - The field's key (FL-019): the first run's `open` record names `field_mapping` `energy`, and no overlay carries
    a field key. With `--field-mapping composition` the `open` record names `composition`, and every overlay the
    timeline recorded has the field's key on the line after the colour key's, its swatches after that key's.
  - Planted bugs, FL-024's telemetry, each run against the tests that should catch it (`tests/overlay.rs`,
    `tests/clock.rs`, this file): the telemetry left out of the overlay, births written from deaths, headroom from
    the genome share, full evaluations not divided by the live count, the clock's evaluations not differenced, the
    summary's telemetry stale, the record's uptake taken from the injected energy, the binary reading no
    evaluations, a unit stepped only above 1,024, three figures only from 1,000, capacity pauses never counted, and
    a run's end that does not pause the overlay: all twelve caught (the capacity pause count after the crowded run
    was added, the first run's one survivor).
- `src/main.rs` unit tests (9, FL-022, FL-024, FL-023, FL-021): an `inspected` record of a watched organism of
  the preset two ticks in carries its sensed tick, scale and evaluation, its 104 inputs, three raw planes and 33
  gradients, and its activations' tick and layers, each value bit for bit, and its two steps' transactions: their
  ticks, each sum as written out over the two records, bit for bit, and the newest step's reproduction outcome,
  and null `descendants` in a world that traces none; in a traced world a founder's record counts from tick 0
  with nothing born, and a record of descendants with three ancestors on the path and two beyond is exactly its
  counts and every ancestor in path order, dead or alive, with `beyond`; a `weights` record of a preset
  organism names it, the shown and read ticks, its genome, topology and parameter count, and every weight and
  bias at its place in its layer, bit for bit, and a gone organism's is exactly its identity and ticks with null
  `tick` and `controller`; the parser takes `--timeline FILE` (offscreen too) and
  `--export FILE --out DIR` with or without `--export-fps`, and refuses `--export` without `--out`, `--out` or
  `--export-fps` alone, `--export-fps 0`, and `--export` with `--timeline` or `--offscreen`, so an export never
  opens a Studio; it takes `--offscreen 2.5` (2 pixels a point by default),
  `--scale 3` and `--offscreen-zooms` with it, and refuses `--offscreen` of 0, -1, NaN, inf or a word,
  `--scale 0`, and `--scale` or `--offscreen-zooms` alone, so no refused run can open a window; `next_slot`
  keeps 300 frames of 15 ms work on their 30 Hz slots, exactly, though every wait overshoots by 6 ms, and after
  a frame three periods long starts the next at its end and the one after a period later (that the loop calls it
  with its last slot, not the frame's start, is shown by the pacing A/B below, not by a test); and
  the `submitted` record of each brush's stroke as JSON, exactly (the
  material's 32-component `direction`; absent fields null), the `finished` record of an applied command (tick,
  published tick, energy) and of a cancelled one, and the `clock` record of a reading, its evaluations included.
- In frankentorch, `ft-kernel-metal`'s `tests/native_input.rs` (13 real-window cases) qualifies the bridge itself,
  backing density included.
- In `flife-probes`, `native_input` (static, every host) and `native_window` (a real window, `flife-probe
  --window`) are qualified.
- `tests/admission.rs` (FL-043, no window): the Studio's observer bytes are its feed (`render_queue_capacity`
  slots of the founding epoch), its view's buffers, one frame's job, the frame's upload (host staging and device
  copy, each at the preparation limit plus the view's texel bytes), `DEVICE_BYTES_PER_PIXEL` (24) per drawable
  pixel and a status word per 16 x 16 tile, and, while a bloom is configured, its row sums on the device (16
  bytes a pixel of FrankenManim's ring for its widest bloom: a band of 128 rows and 96 above and below it, or
  the whole drawable when shorter, FL-019). With `torch` the backend gets the budget's remainder (on the
  preset at a
  3840 x 2160 drawable: 254.6 MiB of 2,048 without the bloom, 97.2 MiB before the layers were textures, and
  235.9 MiB with the preset's bloom, exactly 18.75 MiB less), with `cpu` nothing, and a budget one byte short of
  the observers is refused.
- `tests/frame_upload.rs` (FL-043, FL-033, Metal compute, no window): on a `MetalRenderer`, 48 frames of the preset
  at 4,096 and 16,384 organisms, 1280 x 720 and 3840 x 2160, one and two pixels per point, zooms 1 and 16, in
  points, density and raw density: the host heap the route holds to upload a frame, above the frame it returns,
  and the bytes it copies to the device (`MetalReport::upload_bytes`) are each within the view's preparation limit
  plus its texel bytes (at most 59.7% and 43.2%; the largest copy 22.6 MB, a 4K frame of 16,384 dots and halos);
  the readback is the RGBA8 frame. The Studio's frame tally (`FrameTally::count_copies`, the `frame` and `end`
  records' copy fields) counts the same 48 reports' copies: their sum, the largest, and the readback sum.
- `tests/kept_texels.rs` (FL-033, Metal compute, no window): on one `MetalRenderer`, the preset 30 ticks in, in
  points, density and raw density, a pan, a zoom and a frame between epochs copy none of the layers' texels: each
  frame equals a fresh renderer's byte for byte, and its upload is the fresh renderer's less exactly the texels of
  the layers drawn (raw density draws no field). An epoch 30 ticks later copies them again. The old pin
  (FrankenManim `5d08cba2`) and a pan that moves the field's or the density's display scale each fail it. It is
  its own binary because `frame_upload.rs` counts every thread's heap.
- `tests/timeline.rs` (3 tests, FL-023, FL-019, Metal compute, no window):
  - G10. An offscreen Studio on the preset at 320 x 200 points and 2x records its timeline while posted input
    sets an overlay, restyles (key 3, then C, the field's composition), zooms, selects the shown organism nearest
    the centre, has the camera track it (T, which centres the view at once), arms the energy brush and clicks,
    runs 48 frames with a scroll every fifth, pauses, steps and resets the camera (key 0).
    Every kind of record is made (in one run: 268 records, 73 frames, 2 restyles, 65 view records, most of them
    tracking's, 4 brush regions, 51 shows, 56 placements and 9 advances, one between epochs at a fraction
    strictly inside (0, 1)), a
    style record names the composition, and the stroke applies. The replay outlines each region as recorded. Written
    as NDJSON and read back, the records are equal. The replay redraws every frame in order at the tick the
    Studio showed it, its RGBA8 bytes hashing equal, and closes on the Studio's tick and digest with the one
    command. It refuses, with a `StateConflict`, a timeline whose end names another world, one with its
    command a tick late, one with a frame dated to an epoch the view was not given, and another seed's
    configuration.
  - A style record names its field mapping, and one written before FL-019's composition mapping, without one,
    reads as the energy field; each mapping is written and read back to the same style, and an unknown name is
    refused.
  - Export. A 96 x 60 point Studio at 2x records 24 frames, some closer than a 30 fps slot and some farther, with
    an overlay. `export` at 30 fps writes a PNG a frame. Each decodes to exactly the replay's bytes and hashes as
    the manifest says. Each frame's slot count equals a fresh count in the test (slot k at the first frame's time
    plus k/30 s shows the last frame drawn by then), with repeats and drops both present (in one run: 25 slots,
    7 repeated, 6 dropped). The video is byte for byte the header at 30/1 and each slot's frame through
    `rgba_to_nv12` (limited, left), it decodes at 192 x 120 with as many frames as slots, and its SHA-256 is the
    manifest's. The manifest names the timeline's SHA-256, the configuration's hash, the closing digest and
    tick, and parses back from disk. A second export into the directory is refused (`StorageFailed`) with the
    manifest unchanged. The same timeline with a 191 x 120 drawable exports its frames at that size and no
    video, with the reason (`not even, as 4:2:0 needs`), and leaves no partial video.
- `tests/timeline_binary.rs` (1 test, FL-023, Metal compute, no window): `flife-studio --offscreen 1.5 --scale 1
  --width 96 --height 60 --timeline T`. T opens with its header, has a draw for each frame the `end` record
  rendered, and closes on the `end`'s tick and digest. `flife-studio --export T --out D` prints one `export`
  record on that digest and tick, with the frame count, T's SHA-256 and the configuration's 30 fps. D's manifest
  hashes every frame and the video as they are on disk. A second recording onto T exits 2 before a Studio opens,
  saying it is never written over, and a second export into D exits 1, both files unchanged.
- FL-023's planted bugs, 33 in all:
  - 13 in the recording and the replay: a camera move, the overlay, a selection or a restyle not noted; the
    advance's fraction halved; a command applied a tick late; a show replayed as a placement, a placement as an
    advance; the density or the resize ignored; a frame's epoch unchecked; the founding not shown. 12 were
    caught. An advance replayed as a show survives (see the gaps).
  - The four missing-record bugs again, with the test's census of record kinds off, plus a camera that notes
    only its first move. Each was caught by the frame comparison alone (5 of 5).
  - 15 in the export and the binary: the clock at half its rate, a slot showing the frame after its time, the
    video in full range, each frame written once, drops or repeats miscounted, the header's rate halved,
    resizes not followed, an odd drawable not withheld, the timeline's hash of nothing; the binary writing no
    frame's records, no end or no timeline at all, and finding an existing timeline only after the Studio
    opens. All but one were caught. The binary dropping the session's statuses survives (see the gaps).
- `tests/kept.rs` (4 tests, FL-024, no window; the Studio offscreen on headless Metal), a
  `SessionThread::spawn_kept` over a 32 x 32 preset world with a checkpoint every 256 ticks, all retained:
  - `a_kept_studio_session_is_a_run_the_command_line_resumes`, under `volatile-in-memory` and
    `write-ahead-before-apply`: running, an energy stroke applies; paused, two steps, then a stroke accepted for
    the closing boundary is left pending at the close (`close_reporting`). The checkpoints reported hold tick 0,
    then each tick at or past the next multiple after the one before, past 512 before the pause, then the
    closing boundary, their sequences rising, all `pause-and-stream`. Each one `flife_cli::inspect` restores
    has the reported sequence and SHA-256 and the digest of the world stepped directly with the applied stroke
    at its tick; the last is the world the session closed on; and `flife_cli::resume` six ticks on reaches
    exactly the uninterrupted world, the pending stroke applied at the closing boundary.
  - `a_studio_resumes_its_kept_run_where_it_closed`, under both durabilities: a kept session, paused, steps
    twice, applies a stroke at tick 2, steps, and closes with a second stroke pending at tick 3. `Studio::resume`
    on the directory reports the closing checkpoint (its sequence, tick 3), nothing skipped, and tick 3, keeps the
    same directory, and shows tick 3. One step applies the pending stroke at 3 and reports it finished; E and a
    click submit a stroke numbered 3, after the run's two; three more steps apply it. The world it closes on at 7
    is, bit for bit, the world stepped directly with the three strokes at their ticks; it publishes one closing
    checkpoint at 7 with a sequence past the earlier session's last, and `flife_cli::inspect` restores that digest.
  - `what_cannot_be_resumed_is_refused`: `run` set (`kept where it is`), `record` set (`replays from the founding
    world`) and an absent directory (`is not a run directory`), the directory never made.
  - `what_cannot_be_kept_is_refused_before_anything_is_created`: a directory holding a file (`is not empty`),
    one holding a run the Studio kept (`already holds a run`), both unchanged, and a `maximum_total_bytes` one
    mebibyte past the genome bytes (`budget-exhausted`, `checkpoint memory`), the directory never made.
- `tests/kept_binary.rs` (1 test, FL-024, Metal compute, no window): `flife-studio --offscreen 2.5 --run R
  --timeline T` on that world with a checkpoint every 128 ticks. The `open` record names R and the
  `pause-and-stream` admission, whose `CHECKPOINT_FIXED_BYTES` the memory admission's `checkpoint` term
  charges. The `checkpoint` records run from tick 0 to the `end`'s tick in order, `end.checkpoint_tick` names
  it, the `closed` status is the record before `end`, and the run's last end is `paused` at that tick. The
  overlays T recorded name only reported checkpoints, never an older one after a newer, and one past tick 0.
  `flife_cli::inspect(R)` restores the `end`'s digest at its tick. Then the configuration beside `--resume R`
  exits 2 and prints nothing, and `flife-studio --resume R --offscreen 1.5` (CPU controllers): its `open`
  record names R, `resumed` the closing checkpoint's tick and sequence, nothing skipped, and that tick, the
  `pause-and-stream` admission and the same `checkpoint` term; it ends past that tick, its `checkpoint` records
  continue the first run's chain (ticks and sequences rising), the last at its `end`, which `checkpoint_tick`
  names; and its `end` digest is the world founded from the configuration and stepped directly to that tick. no founding checkpoint; a checkpoint due again at the newest's own multiple;
  a checkpointer that never finds one due; no checkpoint of the closing boundary; a kept session closed rather
  than suspended; no pause before the closing checkpoint; the checkpoint not charged; the overlay never naming a
  checkpoint; the closing statuses not printed; the host keeping no checkpoints; the `open` record naming no
  directory; a directory holding files taken; in `flife-cli`, a streamed checkpoint not retained, the store not
  put back after one, and `vacant` taking a directory that holds files. Each was caught. The missing pause
  survived the first run (the session usually suspends at the boundary the closing checkpoint holds) and is
  caught by the run's `paused` end, checked since.
- Planted bugs in this crate were each caught (listed in the FL-018 commit).

**Validity domain.** macOS on Apple silicon with a window server, one window and one session per process, the
reference CPU world; offscreen, any thread of a process on a host with a Metal device. Linux has no native Studio (§13.6). Linux guarantees headless runs and frame export.

**Known gaps.**
- Moving the window to a screen of another density changes the backing scale; the next frame then takes the new
  drawable size and the view its new pixels per point. Only one 2× display is exercised here, so that path has no
  real-window test, and frankentorch's planted bug for `contentsScale` following a screen change survives for the
  same reason.
- Tracking (T) follows one organism. When it dies the selection goes and tracking stops where it was: the camera
  does not move on to a descendant (plan §2.2's "Follow descendants of a selected organism" is not done), and
  §14.3's temporary measurement regions are not implemented. The window shows tracking only by the camera
  itself; no overlay line says so. Tracking has run offscreen only: no window test here pressed T.
- Camera smoothing, trails and a pinch-zoom gesture are not implemented (§13.5 allows smoothing and trails; G6's
  100 ms budget argues against smoothing the camera).
- While the world runs faster than frames, each frame's newest epoch arrives at fraction 0, so the display shows the
  previous committed state each frame: one state late, with no visible in-between motion. Motion between epochs is
  seen when epochs arrive slower than frames, and for steps.
- FL-021's inspector panel is the overlay's last lines: what the last answer read (its tick, lineage, generation,
  energy and its change, age, speed and heading, what the committed field offers it where it stands (its exchange
  footprint's lowest and highest efficiency for its affinity, the gain and toxicity per unit taken in the
  footprint's proportions, and A.1's request there), hidden widths and parameter count, the held output's
  reproduction probability and emission gate, what it sensed, its activations, and what its budget stages
  added up to over its recorded steps, and its children's and descendants' counts and nearest ancestors). The
  host's send on a click runs offscreen in `tests/offscreen.rs`, where a posted click's inspection answers with
  descendants counted from tick 0, and an inspection refreshed while the world runs carries what it sensed, its
  activations and its transactions up to the answer's tick, the newest closing at its energy, and its descendants
  still counted from tick 0, none more alive than born; a posted W then dumps that organism's controller, read
  while the world runs at a tick of 8 or later, its layers laying out all its parameters. The binary's own
  refresh once a second
  (`Studio::inspect_selected`), the panel's drawing and the record's printing run only in a window, which no test
  here opened; `overlay_text` and the session thread's answers are tested without one. While the world runs,
  the snapshot is read at the session's committed boundary when the inspection is answered, usually later than the
  epoch the click selected on; the record carries both ticks and the panel names the tick it was read at, so it is
  up to about a second old. What it sensed is from the newest tick the session stepped while watching it, so on
  a selection while paused the panel says `sensed from its next tick` until a step. The panel shows none of the
  snapshot's genome, parent, position, phase, cooldown, motion response or affinity, nor any single input or
  activation, which the record carries. The transactions are sums over the steps the world recorded while
  watching it, at most 128 (a second at the reference tick), so a selection's first panel after a pause or a
  newly watched organism covers fewer; no single step's settlement is shown or recorded. What the field offers it
  is read from the committed field the Studio draws, not the field the next step's uptake reads after transport,
  and is a rate per unit taken, not a forecast of its grants. The descendants are the world's ancestry index's
  (`World::trace_ancestry`, which `Studio::open` calls before its memory admission, so the index's room, three
  entries for each organism the population can hold, is charged to the world's state): counts of the children
  and descendants born since the trace began and of those alive at the snapshot's tick, and the ancestors its line
  passes through that are alive or where two or more living lines meet. A dead ancestor through which only one
  living line passes is left out of the path, as is everything above a founder or an organism introduced by a
  brush. The panel names the two nearest and counts the rest; the record names up to eight. The Studio founds
  its world, so it traces from tick 0; a world traced later (`since`) is the engine's case, tested in
  `flife-engine`'s `tests/ancestry.rs`, and the panel's `since tick` line in `tests/overlay.rs`. Ancestry is
  observer state: no digest, checkpoint or report carries it, and the engine's tests show a traced world's reports
  and digests equal an untraced one's. W dumps the selected organism's whole controller at the session's committed
  boundary as a `weights` record, the layout `flife inspect --weights` writes for a run directory; the window shows
  no sign of it (only the record does), and with nothing selected W does nothing. The dump's copy (at most the
  genome catalog's largest controller, 29,316 parameters, about 115 KiB of `f32`) and its written record are not in the memory
  admission: they fall within its reserve. The record's printing runs only in a window, which no test here
  opened; the host's offer and the session thread's answer run offscreen and headless.
- FL-022's brushes:
  - The host's brush path (the click through the frame's view to `Intent::Intervene`) runs offscreen in
    `tests/offscreen.rs` for all four brushes as far as the submitted stroke, and for the energy and material
    brushes through to the applied energy; the removal and introduction brushes' applied energy is checked in
    `tests/brush.rs` through the session thread, without the host. The printing of the `brush`, `submitted` and
    `finished` records and the `end` record's `strokes` and `unstroked` are the binary's and run only in a
    window, which no test here opened. The controls, the payload mapping, the session thread and the `submitted`
    and `finished` records' serialization are tested headlessly. `tests/live.rs` has a brush case
    that was compiled and not run.
  - The region is outlined on the world and named in world units in the overlay; a removal's disk is about the
    pointer. The outline in a window is unrun here: offscreen frames and the replay draw it. A command's applied energy is in its `finished`
    record, not the overlay. The binary's filling of the preview (the recycled share from the configuration) runs
    only in a window: no offscreen binary run arms a brush.
  - A stroke is one click of fixed size. Painting by drag, a choice of amount, count or removal disposition, a
    material other than the selected organism's, source edits and parameter edits have no Studio brush.
  - The material brush takes the selected organism's composition from its `inspected` snapshot, which arrives
    after the click that selected it; a material click before then makes no stroke and is counted as
    `unstroked`. The host's tracking of that snapshot runs offscreen in `tests/offscreen.rs`.
  - A finished command is reported up to `OUTCOME_POLL` after its publication; while paused, a pending
    command's watcher keeps asking every 20 ms.
  - Without `--run` the Studio's session is volatile (`flife_runtime::open`): its commands are not journaled, so
    nothing a brush does survives the process. FL-023's timeline records what was presented and the commands
    applied, so a replay reaches the same world; it is not a journal, and no session resumes from it. With
    `--run` the session is a run `flife resume` continues (FL-024).
- FL-024, plan §14.1's telemetry:
  - The v1 schema configures no scientific rate (the Studio runs as fast as it can), so the telemetry shows the
    achieved ticks a second and no configured one.
  - Without `--run` the session keeps no run directory, so the checkpoint line reads `no checkpoint`. With it, a
    running session's checkpoint lands at the first boundary the checkpointer reads at or past the next
    multiple of the interval, and multiples a run passes between two reads share one; `flife run` steps one tick
    at a time and lands on each. In a debug build the 32 x 32 test world ran 49 ticks before the first read
    (`tests/kept.rs` once used an 8-tick interval: its checkpoints were at 0, 49 and the close). In one run of
    `tests/kept_binary.rs` (128 ticks, about 780 ticks a second while rendering) checkpoints landed 3 to 135
    ticks past their multiples, and one (775) covered both 640 and 768.
  - The Studio opens a new run directory or resumes one (`--resume`); it does not branch one (that is `flife
    branch`). A resumed Studio records no timeline (`--timeline` is refused with `--resume`): a timeline
    replays from the founding world. A resumed run's selection, camera and style start afresh; the run directory
    keeps the world and commands, not the view.
  - Closing a kept session pauses it before the closing checkpoint, so a running session's last run ends as
    `paused`, not `closed`.
  - `pause-before-commit` is the only capacity policy, so a capacity rejection pauses the run; the binary counts
    the capacity pauses its session reports (a run's end, or a held step, for a shortfall) and marks the world
    paused when the current run ends. The crowded offscreen run exercises a run's end; a held step's shortfall
    (a step taken while paused) is not exercised through the binary.
  - Full evaluations a second are the evaluations over the clock's window divided by the shown epoch's live count
    at its end, not a sum of each tick's evaluations over that tick's population.
  - Rejected births (every mutation candidate rejected) are in the record, not the overlay.
  - The telemetry is the shown epoch's when the overlay last changed, up to a second old while running, as the
    tick is. In a window it is unrun: no test here opened one.
- FL-024: the binary's `clock` records and overlay run offscreen in `tests/offscreen_binary.rs`, which checks the
  records' frames but neither their rates nor the overlay's pixels, and in a window, which no test here opened.
  `tests/offscreen.rs` draws the colour key through the Metal route; the meter, the record's JSON, the overlay's
  text and its glyphs are tested on their own; `flife-manim`'s `tests/hud.rs` renders the overlay through the CPU
  route. The overlay's tick is the shown tick when the session
  last reported a status or the clocks last read, so while running it is up to a second old. A window shorter than the summary interval, or one in which the feed published no newer epoch,
  reads what the feed shows: the session may have stepped further than it has published.
- FL-043: `Studio::open` admits the world and its observers before installing the backend
  (`memory::admit`), which `tests/admission.rs` checks without a window. That `open` itself does so and reports
  `memory` in its `open` record runs offscreen in `tests/offscreen_binary.rs`: on the CPU reference the terms sum
  to the peak, within the budget, with no backend term, and the observers are charged for the drawable in pixels.
  That it installs the bounded Metal backend runs only in a window, which no test here opened.
  - A resize past the opening size, in pixels and density bins, is not admitted again.
  - The device terms (`DEVICE_BYTES_PER_PIXEL`: the Metal route's RGBA16F and RGBA8 surfaces and three window
    drawables; and the bloom's row sums) come from FrankenManim's and AppKit's buffers as declared, not from a measurement: device memory is
    outside the heap the tests count.
  - FrankenManim bounds neither the Metal route's host staging of a frame nor its device copy. Both are charged at
    the view's preparation limit plus its texel bytes, which they stayed within on 48 measured frames (at most
    59.7% and 43.2%); a frame beyond them is not refused. From `b104feec` the device keeps the last frame's texels
    between frames, at most the view's texel bytes, inside the same term. FrankenManim drops them before it
    uploads others, so it never holds two sets, but no test sees device memory, so that is read from its code.
  - On the preset at 1920 x 1080 points with a 2x display, the observers take 410.5 MiB and the backend gets
    235.9 MiB of the 2,048 (391.8 and 254.6 without the preset's bloom, whose f32 row sums are charged 18.75
    MiB of the device: since FrankenManim `c6ad9c5b` its bloom works in bands, holding 146 rows at sigma 3
    where it held the whole frame's 126.56 MiB), where it was unbounded: parameter shards that do not fit are evaluated on the reference
    (the same bits, slower). The Studio's frame and inference rates under that budget are not measured here.
  - FL-033: the field and density layers are textures on one quad each. Since FrankenManim `b104feec` their
    texels are copied to the device only when a frame's textures differ in content from the last frame's, so a
    pan, a zoom or a frame between epochs copies none (`tests/kept_texels.rs`, headless; the window's frames were
    not run here). Measured headlessly against the vertex grids they replace, before that, in one interleaved
    invocation (six runs a side, 16,384 organisms, two pixels per point): a 4K density frame copies 1.50 MB to the
    device, not 21.46 MB, and its CPU preparation takes 4.21 ms, not 33.97 ms, at zoom 1. At zoom 16 the whole
    textures are copied, 1.95 MB against the culled grid's 1.45 MB, and the Metal route took 13.01 ms against
    6.69 ms (coefficients of variation 4% and 5%). The frame's preparation and Metal time together still fell,
    38.6 ms to 17.2 ms. The cause is not the texture sampling, as first written here. A later diagnosis read the
    command buffer's device time; the machine's load average was 17 to 58, so these figures are diagnostic, not a
    timing claim. Removing the sample (a constant colour) saved 0.66 ms of the fastest frame's 2.76 ms. The
    textured frame's fastest device time is 2.76 ms against the grids' 2.23 ms. Its median was 8.7 ms against 3.1:
    after its first frames it ran slow almost every time, the same with the sample removed. The grids ran slow
    only now and then, as other processes shared the GPU. That cause is not identified. In a run of that frame
    alone, both took 11.9 ms end to end (median). The Studio's `frame` and `end` records carry `MetalReport`'s
    copies, counted per presented or occluded frame. The tally is tested on real reports without a window, but
    no window was run here, so no session's records were read.
- G6 (§13.7) is measured offscreen, not in a window. The runs are the release `flife-studio --offscreen 20
  --offscreen-zooms` on the preset (4,096 founders, 1920 x 1080 points, 30 frames a second), its controllers on
  the CPU reference with 16 workers, on an Apple M5 Ultra (36 cores) shared with other work (load average 22 to
  34), three interleaved rounds a scale:
  - At 1 pixel a point (1920 x 1080) it drew 30.00 frames a second (30.00, 29.99, 30.02; cv 0.04 %). Frame work
    p95 was 27.4 ms. A camera move's time from its arrival to the end of the frame that drew it was 32.7 ms at the
    median, 51.6 ms at p95 and 62.7 ms at most.
  - At 2 pixels a point (3840 x 2160) it drew 29.79 (29.42, 29.79, 29.80; cv 0.7 %). Frame work p95 was 33.2 ms,
    and the latency was 39.6, 59.9 and 87.5 ms.
  - Every move moved the view.
  - The fixed schedule is the lever (`next_slot`). In the same invocation the incumbent pacing drew 27.87
    (cv 3.6 %) and 28.32 (cv 0.3 %). It waits a period from each frame's start, so every oversleep delays every
    later frame. Paired, the schedule drew 1.074 to 1.145 times as many frames at 1x and 1.045 to 1.052 at 2x.
  - The simulation kept its pace. In an earlier study with the incumbent pacing, each Studio run closed on the
    world `flife run` reaches at the same tick, digest for digest. Its ticks a second were 0.97 to 1.08 (1x) and
    0.94 to 1.03 (2x) of `flife run`'s. Both pacings ran 114.5 to 118.5 ticks a second.

  What this does not show:
  - It includes neither AppKit's event delivery nor the window's present and vsync.
  - The offscreen frame adds a readback that a window does not do (33 MB a frame at 4K).
  - The 4K frame's work p95 sits at the 33.3 ms budget, so a heavier 4K frame misses its slot.
  - The controllers ran on the CPU reference, not on Metal.
  - The host was not quiet.
  - The windowed G6 run is `flife-studio presets/reference-small-world.toml` on a 2x screen, which no test here
    opens.
- The colour key's swatches are their colours on the Metal route only without a bloom: the preset's bloom (0.15),
  which blurs the overlay with everything else, moved their interiors by up to 8 levels (the generation key) and
  16 (the affinity key). The binary's choice of the shown mapping's key each frame runs offscreen in
  `tests/offscreen_binary.rs`, where nothing restyles, and in a window, which no test here opened. The key shows
  sample values, not a continuous ramp; the lineage key cannot say which lineage a colour is. The presenter has no
  window-title setter. The style's meaning is in the `open` and `view` records. The field is drawn from the epoch's bounded `FieldTexture`
  at its texel resolution, so the picture is soft rather than detailed.
- The field's composition (FL-019) is a fixed projection of 32 channels on the affinity view's three axes: two
  materials that differ only off them show the same hue, and no single moment channel can be chosen (plan
  §13.4's "chosen components"). C and `--field-mapping` run offscreen and in the timeline; no test opened a
  window with them, and no human has reviewed the picture (G5).
- A run ends only on pause, capacity shortfall, failure or close. A tick limit is later work; checkpoints are
  `--run`'s (FL-024).
- FL-023's timeline and export:
  - Only live capture exists. Simulation-time export and checkpoint-replay export (§13.8's other modes) and
    Cinema export at 1080p and 4K (FL-050) are not implemented.
  - The video is y4m, FrankenManim's raw 4:2:0 path. It is not encoded (H.264, MP4): FrankenManim's negotiated
    ffmpeg boundary (`fmn-output`) is not wired here.
  - A timeline holds no audio cues, and no captions beyond the overlay's text.
  - Recording in a window is unrun here: no test opened one. A frame drawn in a window replays as it was
    rendered, before the window composited it. A frame the window did not draw (occluded) has no record, and
    the video shows it as repeats.
  - Replay founds the configuration's world and steps it from tick 0, so its time grows with the session's
    ticks. The Studio always founds, so a timeline never starts from a checkpoint or a branch. A world that
    paused for a capacity shortfall cannot be replayed: stepping refuses the pause.
  - The replay's and the export's buffers (a view, a renderer, an RGBA8 frame, an NV12 frame and the encoders'
    scratch, each bounded by one frame) are not admitted against `[resources] memory_budget_bytes` (FL-043).
    The manifest's frame list grows with the frames.
  - An advance replayed as a show survives the planted-bug run. A show re-lowers the field, whose display
    scale is `max(brightest, scale × 0.98)`. That only moves when the field's brightest value falls by more
    than 2% between the epochs an advance redraws, which G10's session never does. In such a session the
    frames would differ, but no test has one.
  - The binary's recording of the session's statuses is not exercised: an offscreen binary run posts no brush
    click, so dropping them survives. The library's records of them (`TimelineRecord::status`) are checked in
    G10.
  - Each test run leaves its exports and timelines, about 1 MB in all, under the target's temporary directory;
    nothing deletes them.
