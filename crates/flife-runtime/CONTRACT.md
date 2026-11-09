# flife-runtime contract

**Purpose.** FL-014 (`flife-fp2`) covers the Asupersync world-session owner and command admission (plan §12.1–§12.4,
§8.1 phase 1, Appendix C.8 and C.14). One owner task holds the committed `World` and its `CommandBook` and is the
only commit authority. Clients submit simulation-changing commands and control requests through a
`SessionHandle`. Admission gives every accepted command one admission order and one exact effective tick, and every
accepted command ends in exactly one terminal outcome. The control loop stays responsive at a committed boundary
while paused. Cancellation races publication at a declared boundary, and exactly one side wins.

**Inputs.**
- A `World` founded from a validated configuration, the same configuration, and a `SessionId`.
  `SessionConfig::from_config` reads `[execution]`: `command_durability`, `command_queue_capacity`,
  `control_queue_capacity`, and the configuration hash as the session's revision.
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
  counts, and whether the run stopped.
- `SessionClosed`: the committed world, the closed book and the final boundary.
- In `flife-engine`, FL-014 adds `World::advance_with` and `World::step_with`, plus each `TickReport`'s applied
  interventions: tick, position, intervention and applied energy.

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
- Twenty-nine planted bugs were each caught by at least one of these tests (listed in the FL-014 commit).

**Validity domain.** `volatile-in-memory` durability; `cpu-reference` and `qualified-reference`; one owner task on
any Asupersync runtime; `InjectEnergy` as the only intervention.

**Known gaps.**
- No durability. Acceptance is held in memory, and everything since the session opened is lost if the process
  ends. Appendix A's `write-ahead-before-apply` needs FL-036's FrankenSQLite command journal, with:
  - durable admission and cancellation records, and persistence receipts;
  - reconciliation of an uncertain append by command identity and journal watermark;
  - crash and retry tests (C.14's interrupted energy brush).
- Idempotency records are kept for the session's whole life. A bounded window belongs to the journal.
- Checkpoints of the schedule, sealed boundary and outcome stream (C.14's checkpoint after B) are FL-016 and FL-037.
- Brushes, regions, material, removal, seeded introduction, source edits and their native input mapping are FL-022.
  FL-014's single-cell `InjectEnergy` is the canonical payload they will extend.
- The tick runs on the owner task. Admitting it as bounded compute work with cancellation between tiles is later,
  with the CPU worker budget (§12.5) and accelerated backends. Responsiveness while running is one tick.
- Observers, render epochs and leases (FL-017), checkpoint commands and branching (FL-016, FL-039) are not
  commands yet.
- Lab tests cover protocol outcomes and publication invariants under seeded interleavings, not every possible
  execution schedule (§12.7).
