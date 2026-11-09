# flife-runtime contract

**Purpose.** FL-014 (`flife-fp2`) covers the Asupersync world-session owner and command admission (plan §12.1–§12.4,
§8.1 phase 1, Appendix C.8 and C.14). One owner task holds the committed `World` and its `CommandBook` and is the
only commit authority. Clients submit simulation-changing commands and control requests through a
`SessionHandle`. Admission gives every accepted command one admission order and one exact effective tick, and every
accepted command ends in exactly one terminal outcome. The control loop stays responsive at a committed boundary
while paused. Cancellation races publication at a declared boundary, and exactly one side wins. FL-016
(`flife-gh0`) checkpoints a session, the world and its command book, in one container (§16.3, C.14, B.2).

**Inputs.**
- A `World` founded from a validated configuration, the same configuration, and a `SessionId`.
  `SessionConfig::from_config` reads `[execution]`: `command_durability`, `command_queue_capacity`,
  `control_queue_capacity`, `render_queue_capacity` (FL-017), and the configuration hash as the session's revision.
- `Command`: the session, a client `CommandId`, a `Target` (`At { tick, retarget }` or `NextAdmissible`), an
  optional expected revision, and a validated `flife_engine::Intervention`. The only intervention so far is
  `InjectEnergy { cell, amount }`: declared external energy added to one field cell.
- Control requests: pause, resume, run until a tick, step, cancel, status, inspect and close.

**Outputs.**
- `Admission`: `Accepted { accepted, status, retry }` with the admission order, the effective tick, whether a late
  target was retargeted, and `Durability::Volatile`; or `Rejected(Rejection)`. A rejection records nothing.
- `CommandStatus`: `Pending`, `Sealed` or `Terminal(Outcome)`. `Outcome` is one of:
  - `Applied`: the boundary tick, the published tick and epoch, and the energy that entered the field;
  - `Cancelled`: the boundary, and `Requested` or `SessionClosed`;
  - `Failed`.

  Each outcome carries its position in the session's outcome stream.
- `Cancellation`:
  - `Cancelled { outcome, sealed }`, where `sealed` says a candidate already held the command and was discarded;
  - `TooLate(applied)`;
  - `AlreadyTerminal`;
  - `Unknown`.
- `StepOutcome`:
  - `Published`: the boundary, the applied commands in order, and the tick's report;
  - `Paused`: a capacity shortfall, at an unchanged boundary;
  - `Failed`: the error, and whether the run stopped.
- `RunEnd` for a bounded run: `Reached`, `Paused`, `CapacityPaused`, `Failed` or `Closed`, each with its boundary.
- `Snapshot`: the committed boundary and world digest, live count, running state, unfinished, admitted and outcome
  counts, whether the run stopped, and the render publisher's `RenderStats` (FL-017).
- `SessionClosed`: the committed world, the closed book, the final boundary and the render counters.
- `SessionHandle::feed` (FL-017): the session's `flife_render::Feed`. Viewers lease epochs from it directly, never
  through the lanes. `open_with` now returns a `Result`, because it allocates the render slots.
- In `flife-engine`, FL-014 adds `World::advance_with` and `World::step_with`, plus each `TickReport`'s applied
  interventions: tick, position, intervention and applied energy.
- Checkpoints (FL-016): `checkpoint`, `restore` and `Restored`, `SESSION_CHECKPOINT_CHUNKS`,
  `CommandBook::{write_checkpoint, read_checkpoint}` with `COMMANDS_CHUNK_VERSION`, and
  `Command::from_canonical_bytes`.

**Invariants — admission and the schedule (§12.3).**
- A command's identity is `(session, command id)`, and its content is the SHA-256 of its canonical record
  (`frankenlife.command.v1`): session, identity, target, expected revision and payload bits. A repeat with the
  same content returns the existing acceptance and its current status, terminal or not, and nothing applies twice.
  A repeat with other content is refused (`ConflictingReuse`).
- The earliest open boundary is the committed tick, or the next one while the committed tick's boundary is sealed.
  `NextAdmissible` takes it. `At { tick }` keeps `tick` if it is still open. Otherwise it is refused as `Late`,
  unless `retarget` permits moving it to the earliest open boundary, which is then recorded.
- The pending schedule is ordered by `(effective tick, admission order)`. Admission order and application order can
  differ: applying B leaves A, accepted earlier for a later tick, pending and unapplied. The outcome stream counts
  terminal outcomes in the order they occur. It is not the admission order.
- At most `command_queue_capacity` commands are unfinished. Beyond that a command is refused as `QueueFull` and not
  recorded, so a later retry is admitted afresh.
- A command for another session, another expected revision or an invalid payload is refused. So is any new
  identity once the run has stopped or the book has closed. A closed or stopped book still answers a retry of an
  accepted identity with its outcome.

**Invariants — sealing, publication and cancellation (§12.3, C.8).**
- Before a candidate is computed, the boundary's commands are sealed in admission order and passed to
  `World::advance_with`, which applies them with phase 2's external sources. A sealed boundary admits nothing new.
- A candidate that pauses on capacity, fails, or is discarded leaves its boundary sealed. The retry applies the
  identical set, less any command whose cancellation won since.
- `CommandBook::published` records the sealed set as applied only if the report applies exactly that set, in order,
  at that boundary. Otherwise nothing is recorded.
- Between preparation and publication the owner yields and then decides every control message that arrived: the
  cancellation boundary.
  - A cancellation of a sealed command there wins. The candidate is discarded and rebuilt from the same committed
    state without it, so no published epoch contains a cancelled intervention.
  - A cancellation after publication is `TooLate` and names the applied outcome.
  - A pause there takes effect at the boundary publication reaches, which is reported. A close there discards the
    candidate.
- A candidate borrows the world mutably, so nothing else can read or write the world until it is published or
  dropped. The owner is the only task that holds the world.

**Invariants — the owner loop (§12.1, §12.2, §12.4).**
- Control and scientific submissions arrive on separate bounded channels, so the control lane's capacity is
  reserved: a full scientific queue never blocks pause, resume, step, inspection, cancellation or close.
- At every committed boundary the owner drains the control lane first, then the scientific lane. Admission never
  waits for a tick, so a future-tick command cannot hold up anything behind it.
- While paused the owner waits for the next message, so no control request needs a future tick to occur.
- A step authorizes exactly one candidate advancement and reports whether it published, paused or failed. It is
  refused while running and after the run stops.
- A capacity shortfall pauses the run at an unchanged boundary. A `NumericalInvalid` stops the run under
  `stop-and-preserve`: every unfinished command fails, every later step or run is refused, and the committed state
  is the last published epoch.
- Closing discards any unpublished candidate, cancels every unfinished command as `session-closed`, answers
  everything still queued, and returns the committed world. A close request, the last handle being dropped, or the
  owner's context being cancelled each close the session. A running owner notices the dropped handles at its next
  committed boundary, so an abandoned session never runs on unobserved.
- Scientific ticks are never skipped and the timestep never changes. A run to tick `t` stops exactly at `t`.

**Invariants — render epochs and observer pressure (FL-017; §7.5, §9.5, §12.4, INV-09).**
- The founding state is render epoch 1. Every published tick is offered to the feed before anyone hears of the
  boundary: before a step's reply, a pause's reply or a run's end. Every committed boundary offers again, which is a
  no-op unless a boundary is pending.
- When every slot is leased or displayable, the boundary stays pending and the last displayable epoch stays. A
  lease release signals the owner on a third lane, which holds at most one pending signal. A paused owner wakes on
  it and publishes the newest boundary without advancing.
- The world is read through `&World` alone. A retained, slow or cross-thread viewer changes display freshness and
  wall time only, never a tick, a command, an admission decision or a birth. A render failure is recorded in the
  counters and never stops the run.
- At close the owner offers the final boundary once more and marks the feed closed. The last epoch stays readable.

**Invariants — checkpoints (FL-016; §16.3, C.14, B.2).**
- `checkpoint(world, book)` writes the world's ten chunks and the commands chunk into one container
  (`SESSION_CHECKPOINT_CHUNKS`). It refuses a book whose revision is not the world's configuration hash.
- The commands chunk holds the session, the revision, the capacity, the next admission order, the sealed
  boundary, whether admission is closed, every record (its canonical command, admission order, effective tick,
  retargeting, durability and status with its outcome) and the outcome stream in cursor order.
- `restore(bytes, profile)` requires all eleven chunks. The book must carry the world's revision, and a sealed
  boundary must be the world's committed tick. Every order must be below the next order and unique, a pending
  command must be effective at or after the committed tick, and a sealed one at the sealed boundary. No
  identity repeats, every outcome is terminal with its cursor at its index, and the terminal records are
  exactly the outcome stream. `Command::from_canonical_bytes` accepts only the canonical encoding.
- A restored session continues as the uninterrupted one: pending commands apply once at their effective ticks, a
  retry of an admitted command returns its first outcome, reuse with other content is still refused, and a
  sealed boundary stays sealed.

**Refusal cases.**
- `SessionConfig::from_config`:
  - `write-ahead-before-apply` is refused as `MissingCapability`, because its journal is FL-036;
  - any other durability than `volatile-in-memory` is refused as `UnknownAlgorithm`;
  - a zero capacity is refused.
- `Rejection`s:
  - `WrongSession`, `RevisionMismatch`, `Invalid`;
  - `Late`, with the requested and earliest boundaries;
  - `QueueFull`, `ConflictingReuse`, `Stopped`, `Closed`.
- `CommandBook::seal` refuses another boundary while one is sealed, or a pending command whose boundary has
  passed. `CommandBook::published` refuses a report that is not the sealed set.
- A handle's request fails as `QueueClosed` once the session has closed. Its wait fails as `Cancelled` when its
  context is cancelled.
- `World::advance_with` checks every intervention before preparing and refuses an invalid one without stopping the
  run. An `InjectEnergy` is refused for a cell outside the grid, or an amount that is not finite, not strictly
  positive or above `f32::MAX`.

**Determinism.** The book is a deterministic state machine over `BTreeMap`s. The schedule, the sealed sets and the
outcome stream depend only on the order of admissions, cancellations and publications, never on hashing or wall
time. Interventions apply in sealed order, and an applied world is identical to `World::step_with` applying the
same interventions at the same ticks. Lab runs replay exactly by seed.

**Independent tests.**
- `tests/book.rs` (7 tests), driven directly against a real world; the expected worlds are stepped directly:
  - exact effective ticks, with late and retargeted targets and the ordered schedule;
  - idempotent retries before and after application, refused conflicting reuse (payload, cell, target or
    revision), and a full queue that records nothing;
  - C.14's A-for-1000 before B-for-101, with A pending, the cursor counting B alone, and retries changing nothing;
  - cancellation before application, too late after it, a repeat, and an unknown identity;
  - a sealed boundary surviving a discarded candidate, losing only a cancelled command, and refusing a stale
    publication record;
  - energy reconciliation: the injection applies at exactly its tick, never above the request, and is recorded as
    `command-injection` and closed by the ledger;
  - every admission refusal, closing, and failing every unfinished command when the run stops.
- `tests/session.rs` (12 tests), with the owner and clients as separate Asupersync Lab tasks, quiescent and
  invariant-clean under each seed:
  - `volatile-in-memory` admitted and `write-ahead-before-apply` refused;
  - C.14's full scientific queue while paused, with inspect, step, run, pause, resume and close still served;
  - commands applied at exactly their ticks in admission order, with the world equal to the direct one;
  - retries returning the first outcome, conflicting reuse, late and retargeted targets, and a wrong revision;
  - C.8: a cancellation racing sealing, the candidate and publication over 24 seeded interleavings. All three
    outcomes occur (before sealing, at the cancellation boundary, too late), the world shows exactly the winner,
    and each seed replays;
  - a pause while running, at a reported boundary from which the next step advances one tick;
  - a capacity pause that keeps its boundary sealed: the identical retry, a refused late command, and a sealed
    cancellation;
  - a numerical fault that fails every unfinished command, refuses new work, steps and runs, and preserves the
    committed state;
  - closing while running, which cancels unfinished commands and refuses later requests;
  - dropping every handle, while paused and while running (the latter within a 2,000-step Lab bound, so an owner
    that ignores the dropped handles fails quickly instead of hanging);
  - the same protocol on the native current-thread runtime.
- `tests/observation.rs` (3 tests, FL-017), each world compared with one stepped directly with no session and no
  feed:
  - G3 over three Lab seeds. A viewer retaining two epochs makes boundaries 3–48 busy while the feed keeps tick 2
    and reports each pending tick. Every step's digest, births and deaths still equal the direct trajectory.
    Releasing the leases while paused publishes tick 48 without a tick advancing. A run to 96 under renewed
    pressure reaches the direct world, with 92 busy boundaries.
  - C.14 over five Lab seeds: idle, retaining and slow readers on a near-capacity birth tick. Each run pauses at the
    same boundary with the same shortfall, the identical retry pauses identically, and each returns the direct
    world.
  - A reader thread leasing and checking every epoch it can while the native runtime runs 160 ticks: the direct
    world, and only complete epochs in increasing order.
- Twenty-nine planted bugs were each caught by at least one of these tests (listed in the FL-014 commit). FL-017's
  twenty-five, three of them in the session's feed, are listed in its commit.

- `tests/checkpoint.rs` (FL-016):
  - A 48-founder ecology with a four-tick controller cadence, checkpointed at founding, off phase (tick 5), with
    newborns (tick 33), after deaths (tick 218) and late (tick 401), continues to tick 480 with every published
    report identical to the uninterrupted run's. Each restore has the same digest, tick, epoch, events, ledger
    report, paths and genomes (compared directly, since the digest leaves genome contents out), and encodes to
    the same bytes again.
  - Pending commands: one for tick 40 admitted before one for tick 10, and a cancelled one, checkpointed at
    tick 12. The restored session applies each once at its tick and reaches the reference world. A retry returns
    the first outcome, and conflicting reuse is refused.
  - A sealed boundary stays sealed across a checkpoint.
  - The 4,096-organism preset at tick 64: 161.3 MiB, written in about 0.16 s and verified and restored in about
    0.15 s on an Apple-silicon Mac; ticks 65–96 are identical.
  - Damage, on a 2.1 MB checkpoint: every truncation and a flipped bit at every header, manifest and seal byte
    and at every 97th payload byte (44,936 cases) are refused. So are a trailing byte and 12 re-sealed edits,
    each refused by its own check: the particle count, the ledger's last holdings one ulp off, the quota's live
    count, the seed, a configuration byte, a parameter set to `f32::MAX`, a held phase, a pending command marked
    sealed, the sealed boundary, an outcome's identity, a manifest item count and a missing commands chunk.
    Another numerical profile is refused.
  - A checkpoint binds the world to its configuration and slot arrangement, and refuses a book from another
    configuration.
  - 20 planted mutants in the encoders, decoders, container and path resumption, each caught by these tests or
    `flife-core`'s.

**Validity domain.** `volatile-in-memory` durability; `cpu-reference` and `qualified-reference`; one owner task on
any Asupersync runtime; `InjectEnergy` as the only intervention.

**Known gaps.**
- No durability. Acceptance is held in memory, and everything since the session opened is lost if the process
  ends. Appendix A's `write-ahead-before-apply` needs FL-036's FrankenSQLite command journal, with:
  - durable admission and cancellation records, and persistence receipts;
  - reconciliation of an uncertain append by command identity and journal watermark;
  - crash and retry tests (C.14's interrupted energy brush).
- Idempotency records are kept for the session's whole life. A bounded window belongs to the journal.
- FL-016 encodes the schedule, sealed boundary and outcome stream with the world. C.14's checkpoint after B
  across a crash also needs the journal (FL-036) and resume (FL-037); FL-035's `flife-store` holds the
  checkpoints. There is no
  checkpoint command or session resume yet: `checkpoint` and `restore` are library calls on a closed or
  quiescent world and book.
- Brushes, regions, material, removal, seeded introduction, source edits and their native input mapping are FL-022.
  FL-014's single-cell `InjectEnergy` is the canonical payload they will extend.
- The tick runs on the owner task. Admitting it as bounded compute work with cancellation between tiles is later,
  with the CPU worker budget (§12.5) and accelerated backends. Responsiveness while running is one tick.
- Checkpoint commands and branching (FL-037, FL-039) are not commands yet. Render epochs are not commands: viewers
  read the feed directly (FL-017).
- The render projection runs on the owner task, O(live + texels) per published boundary.
- Lab tests cover protocol outcomes and publication invariants under seeded interleavings, not every possible
  execution schedule (§12.7).
