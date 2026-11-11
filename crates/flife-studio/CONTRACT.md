# flife-studio contract

**Purpose.** FL-018 (`flife-jdf`) covers FrankenLife's safe native input host and first interactive Studio
(plan §13.1, §13.6). FL-019 (`flife-g90`) adds live switching between its colour mappings, raw views and density
rendering (§13.4). FL-020 (`flife-err`, part) adds the camera (zoom about the pointer, drag pan across the seams,
reset), selection by displayed stable identity (§13.6), and the display one committed state late, organisms moving
between epochs by identity (§13.5). It closes the G0 native-input gap: the presenter's real AppKit events reach FrankenLife as typed
input. The live world is shown in a native window, through FrankenManim's Metal 3D route and the `flife-manim`
adapter.

**Inputs.**
- A `ValidatedConfig`. `[presentation]` gives the window size, the background, the frame rate and the colour
  mapping. `StudioOptions` can override the size, make the window fixed and set the starting `Style`.
- The process main thread, macOS, Metal and a window server.
- The presenter's typed input and its pixel density. They come from frankentorch `ft-kernel-metal` at rev
  `0ae728fe`, through FrankenManim `fmn-render` at rev `b104feec`.
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
    | 1 to 5 | colour by lineage, energy, generation, motion or topology |
    | R | switch the glow and raw looks |
    | D | switch dots and density |
    | F | show or hide the field |

    | 0 | the whole torus again (a `CameraMove::Reset`) |

    The style keys and 0 ignore auto-repeat and ⌘, and produce no intent.
  - Pointer gestures become `CameraMove`s, in logical points, kept in order until `take_moves()` (reserved once
    for a poll's events):
    - a primary press that moves more than `CLICK_SLOP_POINTS = 4` is a drag: `Pan` by the whole motion since the
      press, then by each move, so the pans sum to the pointer's motion;
    - a primary press released within the slop is a click: `Select { at }` at the press;
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
     (scaled to drawable pixels); then takes the view and the selection;
  6. if the feed's latest epoch is newer than the one shown, releases the prior lease, leases the newer epoch and
     makes the shown one its prior: the display then moves from the prior to it (`WorldView::show_between`), at the
     fraction the `Pacer` gives, and the selection stays only if the newer epoch has its identity drawn; after a
     style change it shows the pair again, after a camera move it places it, and otherwise, while the display
     moves, it advances it; once the display rests the prior lease is released;
  7. renders one `ThreeDJob` with `render_three_d` straight into the window's drawable, with no pixel readback.

  It returns a `Frame` (state, events, intents, refusals, statuses, refit, restyled, moved, new epoch, shown
  `Prior`, the `Motion` or `None` at rest, selected organism, hover, outcome). `display_pair()` is the epochs it
  holds and `pacer()` the pacer.
- `Pacer` times the display's motion (§13.5, one-state display latency): `arrive(now)` for an epoch of a running
  stream moves the smoothed interval between arrivals by `ARRIVAL_SMOOTHING = 0.25` of the difference, and a wait
  of `MOTION_MAX = 100 ms` or more restarts it at `MOTION_MAX`; `arrive_alone(now)`, for an epoch that arrives
  while the controls are not running (a step), sets it to `MOTION_MAX`; `fraction(now)` is the time since the last
  arrival over the interval, at most 1, and 1 before an interval is known. So a running world is drawn about one
  arrival interval late, and a step is seen moving within 100 ms. `close()` closes the window and the session and returns the committed world (`StudioClosed`).
- `flife-studio <config> [--width W --height H] [--fixed] [--cpu-workers W] [--controllers C] [--color M]
  [--look glow|raw] [--density] [--no-field]`: the binary. M is `lineage`, `energy`, `generation`, `motion` or
  `topology` (or a registered name such as `founder-lineage-v1`); the default is the configuration's
  `[presentation] color_mapping`. C is
  `cpu`, `torch`, `metal` or `auto` (the default), as for `flife run`: `auto` takes the Metal controllers only at W ≤ 8, and on a
  host without Metal `torch` at W ≤ 4. The Studio's default W is one per core up to 16, so on a host with more
  than 8 cores (or more than 4 without Metal) the controllers run on the CPU reference by default. With 16 workers Metal
  measured slower in the Studio under FrankenManim `dfdc7d18` (72 against 119 ticks per second, one 12 s run each),
  with presentation on the same GPU, and level under `f401a947` (132 against 120 at 960×540, 152 against 161 at
  1,920×1,080, four interleaved 10 s runs each). It paces frames to `[presentation] frames_per_second` and prints `frankenlife.studio.v1` NDJSON records:
  - `open`, naming the window, the CPU workers, the controller backend, and the style: `mapping`, its `meaning`,
    `look`, `particles` and `field`;
  - one `status` per status;
  - one `view` per style change, with the same style fields;
  - one `selection` each time the selected organism changes (a click, or the organism gone from a newer epoch):
    its identity or null, its position and the shown tick;
  - a `frame` summary about once a second, with the camera's center and zoom, and the bytes the Metal route has
    copied so far: to the device (`upload_bytes`, and the most in one frame, `peak_upload_bytes`) and back to the
    host (`readback_bytes`, none for frames presented to the window);
  - `end`, with the committed tick, the world digest, the controller evaluations the backend returned and
    deferred, and its failure if it failed, and the same copy totals.

  Errors are `frankenlife.error.v1` on standard error. The exit code is 0 after a clean close, 2 when the
  configuration or window fails, and 1 after a later failure.

**Invariants.**
- *Main-thread presentation (§13.6).* Window creation, event polling, presentation and close happen on the process
  main thread. Nothing on it waits for a tick, a command or the session's lanes: intents use `try_send`, statuses
  use `try_iter`, and epochs come from the lock-free feed.
- *Observation neutrality.* The main thread reads the world only through render epochs. It holds at most
  `DISPLAY_PAIR` = 2 leases: the shown epoch, and while the display moves, its prior; at rest, one. Input reaches the
  world only as session intents: camera moves, selection and the display's motion never do.
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
- *Same science.* A live session's committed world equals the same configuration stepped directly to the same tick.
- *No unsafe.* The crate is `forbid(unsafe_code)`. All AppKit and Core Graphics FFI is in frankentorch's reviewed
  presenter.

**Refusal cases.**
- `Studio::open` off the main thread, without Metal or without a window server: `MissingCapability`. A background
  that is not `#rrggbb`: `NumericalInvalid`. The founding and session errors of `World::found` and
  `flife_runtime::open`.
- A window that cannot be polled, rendered, presented or closed: `StateConflict`, carrying the presenter's error.
- `SessionThread::send` on a full lane: `BudgetExhausted`. On a gone thread: `QueueClosed`.
- The session's own refusals arrive as `Status::Refused` with their error class.

**Determinism.** The session's science is the owner's (FL-014): deterministic for its configuration and the ticks it
commits, whatever the frame rate, focus or window size. Frame timing, coalescing and pointer positions are
presentation state, not scientific state.

**Independent tests.**
- `tests/controls.rs` (12 tests):
  - Space toggling and ignoring repeat; stepping only while paused, with repeat stepping; Esc and ⌘W closing once.
  - Buttons through presses, motion and capture loss.
  - Focus, geometry rounding, hover through a non-integral logical size, and the tally.
  - Following the session: a late end of run 1 does not stop run 2, and run 2 stopping on its own does.
  - A resume refused at the lane or by the session leaves the controls paused.
  - FL-020: a drag pans by the pointer's whole motion (worked deltas) and selects nothing; a click within the slop
    selects at the press, while secondary presses, lost presses and buttonless motion make no move, and a drag
    lost to capture loss pans no further; scrolls zoom by `exp(rate · dy)` in both units, limited, with no move for
    a zero or non-finite delta; 0 resets once (not on repeat or with ⌘) and is no style key; 256 moves wait in
    order until taken.
- `tests/pacing.rs` (5 tests, FL-020): at rest before an interval is known; the display moves over the interval
  between arrivals (worked fractions, capped at 1, a frame before the arrival at 0) and later intervals smooth by a
  quarter; a five-second wait counts as `MOTION_MAX`, as does a wait of exactly `MOTION_MAX`, after a fast stream
  too; a step alone moves over `MOTION_MAX` whatever came before and smooths down once epochs flow; arrivals with
  no time between leave the display at rest.
- `tests/live.rs` (harness-free, 31 checks, `--features synthetic-input`): the real 4,096-organism reference preset
  in a real 640 × 400 window. Every input is a real NSEvent or Core Graphics event posted to the application queue
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
  - Esc closes at the last stepped tick (three steps after the pause);
  - the committed world's digest equals the directly stepped world's.
  - Off macOS it reports itself skipped.
- In `flife-runtime`, `tests/session.rs` checks `SessionHandle::start` across six Lab seeds: the start comes back at
  once, and the end comes back for a pause, a reached tick and a close.
- In frankentorch, `ft-kernel-metal`'s `tests/native_input.rs` (13 real-window cases) qualifies the bridge itself,
  backing density included.
- In `flife-probes`, `native_input` (static, every host) and `native_window` (a real window, `flife-probe
  --window`) are qualified.
- `tests/admission.rs` (FL-043, no window): the Studio's observer bytes are its feed (`render_queue_capacity`
  slots of the founding epoch), its view's buffers, one frame's job, the frame's upload (host staging and device
  copy, each at the preparation limit plus the view's texel bytes), `DEVICE_BYTES_PER_PIXEL` (24) per drawable
  pixel and a status word per 16 x 16 tile. With `torch` the backend gets the budget's remainder (255.3 MiB of
  2,048 on the preset at a 3840 x 2160 drawable, 97.2 MiB before the layers were textures), with `cpu` nothing, and
  a budget one byte short of the observers is refused.
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
- Planted bugs in this crate were each caught (listed in the FL-018 commit).

**Validity domain.** macOS on Apple silicon with a window server, one window and one session per process, the
reference CPU world. Linux has no native Studio (§13.6). Linux guarantees headless runs and frame export.

**Known gaps.**
- Moving the window to a screen of another density changes the backing scale; the next frame then takes the new
  drawable size and the view its new pixels per point. Only one 2× display is exercised here, so that path has no
  real-window test, and frankentorch's planted bug for `contentsScale` following a screen change survives for the
  same reason.
- Camera smoothing, trails and a pinch-zoom gesture are not implemented (§13.5 allows smoothing and trails; G6's
  100 ms budget argues against smoothing the camera). Brushes as `CommandPort` commands are FL-022's.
- While the world runs faster than frames, each frame's newest epoch arrives at fraction 0, so the display shows the
  previous committed state each frame: one state late, with no visible in-between motion. Motion between epochs is
  seen when epochs arrive slower than frames, and for steps.
- A selection shows only as the ring and the `selection` record: there is no inspector (FL-021).
- FL-043: `Studio::open` admits the world and its observers before installing the backend
  (`memory::admit`), which `tests/admission.rs` checks without a window. That `open` itself does so, installs the
  bounded backend and reports `memory` in its `open` record runs only in a window, which no test here opened.
  - A resize past the opening size, in pixels and density bins, is not admitted again.
  - The device terms (`DEVICE_BYTES_PER_PIXEL`: the Metal route's RGBA16F and RGBA8 surfaces and three window
    drawables) come from FrankenManim's and AppKit's buffers as declared, not from a measurement: device memory is
    outside the heap the tests count.
  - FrankenManim bounds neither the Metal route's host staging of a frame nor its device copy. Both are charged at
    the view's preparation limit plus its texel bytes, which they stayed within on 48 measured frames (at most
    59.7% and 43.2%); a frame beyond them is not refused. From `b104feec` the device keeps the last frame's texels
    between frames, at most the view's texel bytes, inside the same term. FrankenManim drops them before it
    uploads others, so it never holds two sets, but no test sees device memory, so that is read from its code.
  - On the preset at 1920 x 1080 points with a 2x display, the observers take 391.1 MiB and the backend gets
    255.3 MiB of the 2,048, where it was unbounded: parameter shards that do not fit are evaluated on the reference
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
- Input latency and frame-time percentiles under simulation load (§13.6, §13.7, G6) are not measured here, at
  one pixel per point or at backing density, where a 2× screen has four times the pixels per frame. The
  synchronous present path is an integration starting point, not latency evidence.
- There is no on-screen legend: the Metal 3D route has no text, and the presenter has no window-title setter. The
  style's meaning is in the `open` and `view` records. The field is drawn from the epoch's bounded `FieldTexture`
  at its texel resolution, so the picture is soft rather than detailed.
- A run ends only on pause, capacity shortfall, failure or close. A tick limit, checkpoints and recording are later
  work (FL-023, FL-038).
