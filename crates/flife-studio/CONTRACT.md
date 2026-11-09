# flife-studio contract

**Purpose.** FL-018 (`flife-jdf`) covers FrankenLife's safe native input host and first interactive Studio
(plan §13.1, §13.6). It closes the G0 native-input gap: the presenter's real AppKit events reach FrankenLife as typed
input. The live world is shown in a native window, through FrankenManim's Metal 3D route and the `flife-manim`
adapter.

**Inputs.**
- A `ValidatedConfig`. `[presentation]` gives the window size, the background and the frame rate.
  `StudioOptions` can override the size and make the window fixed.
- The process main thread, macOS, Metal and a window server.
- The presenter's typed input. It comes from frankentorch `ft-kernel-metal` at rev `1c4febfb`, through FrankenManim
  `fmn-render` at rev `abd57cbf`.
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

  - `drawable_size` is the content area in whole points. `hover` is the world point under the pointer.
- `SessionThread`: the FL-014 session owner and one controller task on an Asupersync current-thread runtime, on its
  own thread.
  - Intents arrive on a bounded lane of `INTENT_CAPACITY` = 16. The main thread only `try_send`s.
  - Every intent gets exactly one `Status`, on a standard channel the main thread drains without blocking.
  - A resume uses `SessionHandle::start`, added to `flife-runtime` by FL-018. It reports `Running { run, from }` at
    once. A watcher task, owned by the controller and joined at close, reports the run's end as `Ended { run, at,
    why }`, and run numbers keep a late end from touching a newer run.
  - A pause reports `Paused { at }`. A step reports `Stepped { tick }` or `StepHeld { at, why }`. A refusal reports
    `Refused`, and close reports `Closed { at }`.
- `Studio` is the main-thread host. Each `frame()`:
  1. drains typed input through `Controls` and offers the resulting intents;
  2. drains statuses;
  3. refits the `WorldView` when the content area changed;
  4. leases the newest epoch if it is newer than the one shown, lowers it and drops the lease;
  5. renders one `ThreeDJob` with `render_three_d` straight into the window's drawable, with no pixel readback.

  It returns a `Frame` (state, events, intents, refusals, statuses, refit, new epoch, shown `Prior`, hover,
  outcome). `close()` closes the window and the session and returns the committed world (`StudioClosed`).
- `flife-studio <config> [--width W --height H] [--fixed]`: the binary. It paces frames to
  `[presentation] frames_per_second` and prints `frankenlife.studio.v1` NDJSON records:
  - `open`;
  - one `status` per status;
  - a `frame` summary about once a second;
  - `end`, with the committed tick and the world digest.

  Errors are `frankenlife.error.v1` on standard error. The exit code is 0 after a clean close, 2 when the
  configuration or window fails, and 1 after a later failure.

**Invariants.**
- *Main-thread presentation (§13.6).* Window creation, event polling, presentation and close happen on the process
  main thread. Nothing on it waits for a tick, a command or the session's lanes: intents use `try_send`, statuses
  use `try_iter`, and epochs come from the lock-free feed.
- *Observation neutrality.* The main thread reads the world only through render epochs. It holds at most one lease,
  and only while lowering. Input reaches the world only as session intents.
- *Bounded everything.* The presenter queue holds 1024 events, the intent lane 16, the viewer one lease, the cloud
  its ceiling, and the main thread's event buffer is reserved once at 1024 events. An intent refused because the lane
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
- `tests/controls.rs` (7 tests):
  - Space toggling and ignoring repeat; stepping only while paused, with repeat stepping; Esc and ⌘W closing once.
  - Buttons through presses, motion and capture loss.
  - Focus, geometry rounding, hover through a non-integral logical size, and the tally.
  - Following the session: a late end of run 1 does not stop run 2, and run 2 stopping on its own does.
  - A resume refused at the lane or by the session leaves the controls paused.
- `tests/live.rs` (harness-free, 17 checks, `--features synthetic-input`): the real 4,096-organism reference preset
  in a real 640 × 400 window. Every input is a real NSEvent or Core Graphics event posted to the application queue
  and delivered through the presenter's typed bridge:
  - the first poll's real `Resized` (640 × 400 points), and a real `FocusGained` (made key by a real key-window
    change if another application holds the keyboard);
  - the founding epoch on screen as 4,096 dots in one cloud, reserved for the 16,384 ceiling;
  - Space runs, and live epochs reach the screen;
  - Space pauses at a reported boundary, which is what is shown, and no epoch arrives while paused;
  - → and `.` each step one tick;
  - a real resize to 800 × 450 refits the viewport (3.515625 px per world unit) without new draw storage, and the
    next frame is presented at the new drawable size;
  - real pointer motion hovers the right world point within a pixel, and a real scroll arrives;
  - focus loss with a button held loses the capture and clears the drag, and focus regain arrives;
  - no intent refused and no input dropped;
  - Esc closes at the last stepped tick;
  - the committed world's digest equals the directly stepped world's.
  - Off macOS it reports itself skipped.
- In `flife-runtime`, `tests/session.rs` checks `SessionHandle::start` across six Lab seeds: the start comes back at
  once, and the end comes back for a pause, a reached tick and a close.
- In frankentorch, `ft-kernel-metal`'s `tests/native_input.rs` (12 real-window cases) qualifies the bridge itself.
- In `flife-probes`, `native_input` (static, every host) and `native_window` (a real window, `flife-probe
  --window`) are qualified.
- Planted bugs in this crate were each caught (listed in the FL-018 commit).

**Validity domain.** macOS on Apple silicon with a window server, one window and one session per process, the
reference CPU world. Linux has no native Studio (§13.6). Linux guarantees headless runs and frame export.

**Known gaps.**
- The presenter sizes the window's content area to the frame. So the Studio renders one drawable pixel per logical
  point, and the compositor scales that up on high-DPI displays. Rendering at backing resolution needs the presenter
  to size the drawable apart from the content area (upstream, with FL-020's camera work). `Viewport` already maps
  logical points through any drawable ratio.
- Camera changes, zoom (scroll is only tallied), selection by displayed stable identity, and brushes as `CommandPort`
  commands are FL-020's and FL-022's.
- Input latency and frame-time percentiles under simulation load (§13.6, §13.7, G6) are not measured here. The
  synchronous present path is an integration starting point, not latency evidence.
- The field layer and halos are a first legibility pass, not FL-019's mappings. The field is drawn from the epoch's
  bounded `FieldTexture` at its texel resolution, so the picture is soft rather than detailed. Motion between ticks
  is not interpolated (FL-020).
- A run ends only on pause, capacity shortfall, failure or close. A tick limit, checkpoints and recording are later
  work (FL-023, FL-038).
