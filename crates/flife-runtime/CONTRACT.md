# flife-runtime contract

**Purpose.** FL-014 (`flife-fp2`) covers the Asupersync world-session owner and command admission (plan §12.1–§12.4,
§8.1 phase 1, Appendix C.8 and C.14). One owner task holds the committed `World` and its `CommandBook` and is the
only commit authority. Clients submit simulation-changing commands and control requests through a
`SessionHandle`. Admission gives every accepted command one admission order and one exact effective tick, and every
accepted command ends in exactly one terminal outcome. The control loop stays responsive at a committed boundary
while paused. Cancellation races publication at a declared boundary, and exactly one side wins. FL-016
(`flife-gh0`) checkpoints a session, the world and its command book, in one container (§16.3, C.14, B.2). FL-036
(`flife-ka2`) adds `write-ahead-before-apply` sessions: the owner makes every acceptance, cancellation and outcome
durable through a command journal before relying on it, and a session reopens from what the journal holds (§12.3,
§12.4, §16.6, C.14). The journal is a port here; `flife-store` implements it over FrankenSQLite.
FL-039 (`flife-05q`) forks a command book at a checkpoint into a branch session (§14.4, §16.3). The branch
inherits every command and outcome, records the fork, and from then on admits only its own session's commands.

**Inputs.**
- A `World` founded from a validated configuration, the same configuration, and a `SessionId`.
  `SessionConfig::from_config` reads `[execution]`: `command_durability`, `command_queue_capacity`,
  `control_queue_capacity`, `render_queue_capacity` (FL-017), and the configuration hash as the session's revision.
- `Command`: the session, a client `CommandId`, a `Target` (`At { tick, retarget }` or `NextAdmissible`), an
  optional expected revision, and a validated `flife_engine::Intervention`: `InjectEnergy { cell, amount }`,
  declared external energy added to one field cell; (FL-022) `RemoveOrganisms { center, radius, disposition }`,
  every organism in a disk removed at the boundary; (FL-022) `InjectMaterial { cell, amount, direction }`, the
  material brush: declared energy with a signed composition direction, `||direction||₂ ≤ 1`; or (FL-022)
  `IntroduceOrganisms { cell, count, energy }`, seeded introduction: `count` founder-drawn organisms, each endowed
  with `energy`, entering one cell at the end of the tick. The canonical record's payload opens with its
  NUL-terminated tag (`inject-energy`, `inject-material`, `remove-organisms` or `introduce-organisms`); a removal's
  disposition is its NUL-terminated name; a material record carries the 32 direction components' bits in channel
  order after the amount; an introduction carries the cell, the count (`u32`) and the energy's bits. An energy
  injection's record is byte for byte FL-014's, and every earlier record decodes unchanged.
- Control requests: pause, resume, run until a tick, step, cancel, status, inspect, close and suspend (FL-037);
  checkpoint and durable outcome (FL-036); inspect one organism (FL-021); hold a tick and release it (FL-024).
- For `open_journaled` (FL-036): a `JournalWriter` started over a `Journal` implementation, and the
  `JournalContents` it read back when it opened the session's run.

**Outputs.**
- `Admission`: `Accepted { accepted, status, retry }` with the admission order, the effective tick, whether a late
  target was retargeted, and its `Durability` (`Volatile`, or `WriteAhead` once the journal holds it); or
  `Rejected(Rejection)`. A rejection records nothing.
- `CommandStatus`: `Pending`, `Sealed` or `Terminal(Outcome)`. `Outcome` is one of:
  - `Applied`: the boundary tick, the published tick and epoch, and the energy the command moved: into the field
    for an injection, out of the organisms it removed for a removal;
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
- `Snapshot`: the committed boundary and world digest, live count, running state, the tick a run stops at and the
  tick held, unfinished, admitted and outcome counts, whether the run stopped, and the render publisher's
  `RenderStats` (FL-017).
- FL-024: `SessionHandle::hold(tick)` returns a `Holding` at once (its `tick` and the boundary it was `set` at), whose
  `reached()` answers with the boundary once the committed tick is `tick`; `SessionHandle::release()` returns the
  committed boundary.
- FL-021: `SessionHandle::inspect_organism(id)`: the committed world's `World::organism_snapshot(id)`, an
  `OrganismSnapshot` whose `tick` is the boundary it was read at, or `None` when the committed world does not hold
  the organism. The owner's world then watches that organism (`World::watch`), so a later inspection of it carries
  what it sensed, its newest evaluation's activations and the transactions of its steps since; a world watches the
  organism most recently inspected.
  A closing owner answers without moving the watch.
- FL-021: `SessionHandle::organism_weights(id)` (plan §14.2's on-demand weight dump): the committed world's
  `World::organism_weights(id)`, a `ControllerWeights` whose `tick` is the boundary it was read at, or `None` when
  the committed world does not hold the organism. It copies that one genome's parameters, watches nothing and
  changes no result; a running owner answers between ticks, and a closing one at the closed boundary.
- Plan §14.3: `SessionHandle::measure_region(center, radius)` (the Studio's temporary measurement region): the
  committed world's `World::measure_region`, a `RegionMeasurement` whose `tick` is the boundary it was read at,
  or the world's refusal of the disk. It copies no genome, watches nothing and changes no result. A running owner
  answers between ticks, and a closing one at the closed boundary.
- `SessionClosed`: the committed world, the book (closed, or for a suspended session with its unfinished commands
  still pending), the final boundary and the render counters.
- FL-036, for a journaled session:
  - `JournalState` in `Snapshot` and `SessionClosed`: the durable `Watermarks` (acceptances and outcomes, two
    contiguous counts), how many records wait for a receipt, and the journal's failure, if any;
  - `SessionHandle::checkpoint`: a `SessionCheckpoint` (its boundary and FL-016 container) taken by the owner at a
    committed boundary with nothing waiting for a receipt;
  - `SessionHandle::durable_outcome`: a command's terminal outcome once the journal holds it, the explicitly
    durable response (§12.3);
  - the `journal` module: the `Journal` port, `JournalRecord`, `Watermarks`, `JournalContents`, `JournalWriter`,
    `JOURNAL_ATTEMPTS` and `block_on_inline`; `CommandBook::recover`, `set_durability` and `cancel_for`.
- `SessionHandle::feed` (FL-017): the session's `flife_render::Feed`. Viewers lease epochs from it directly, never
  through the lanes. `open_with` now returns a `Result`, because it allocates the render slots.
- In `flife-engine`, FL-014 adds `World::advance_with` and `World::step_with`, plus each `TickReport`'s applied
  interventions: tick, position, intervention and applied energy.
- Checkpoints (FL-016): `checkpoint`, `restore` and `Restored`, `SESSION_CHECKPOINT_CHUNKS`,
  `CommandBook::{write_checkpoint, read_checkpoint}` with `COMMANDS_CHUNK_VERSION`, and
  `Command::from_canonical_bytes`.
- FL-039:
  - `CommandBook::branch(session, tick, checkpoint)` returns the branch's book;
  - `Fork { parent, admitted, tick, checkpoint }`, and `CommandBook::lineage` and `admitted_by`;
  - `admitting_session(lineage, session, order)`, the same rule for a journal;
  - `CommandBook::history`, the `JournalContents` a journal of the book would hold, and `last_command`;
  - `BRANCHED_COMMANDS_CHUNK_VERSION` (2), the commands chunk of a branch.

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
- Suspending (`SessionHandle::suspend`, FL-037) closes the same way but cancels nothing: every unfinished command
  stays pending in the returned book and unended in the journal, so a later session of the same run, restored from
  a checkpoint taken before it, applies it at its tick. A close that reaches the owner with a suspension wins.
- `open_restored` (FL-037) opens a `volatile-in-memory` session on a restored world and book, refusing a book of
  another session or revision as `open_journaled` does.
- Scientific ticks are never skipped and the timestep never changes. A run to tick `t` stops exactly at `t`.
- A hold (FL-024, for a kept Studio session's checkpoints on their multiples) keeps the committed boundary at its
  tick: a run stays running there and ends only as it would have (a pause, its tick, a close), and a step requested
  there waits with it, as a step waits for a journal receipt. Control stays live, so a checkpoint, an inspection or
  a pause is answered at the held boundary. A hold's wait is answered when the committed tick reaches it, at once
  if it is there; with a state conflict when the hold is moved or released first; as closed when the session
  closes. A hold at a tick the committed one has passed is refused and the earlier hold stays; a hold that arrives
  at the cancellation boundary is decided after that candidate, so a running session can pass the committed tick
  it names. Holding changes no result.
- An organism inspection (FL-021), like `inspect`, is answered at a committed boundary: at once while paused; while
  running, deferred from the cancellation boundary to the next committed one; while closing, at the closed
  boundary. It reads only the committed world, so a session commits the same states whether or not it is inspected.

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
- FL-024 (plan §14.1): before offering a published tick, stepped or run, the owner counts its report into the
  publisher's tally (`Publisher::count`), so every epoch the feed publishes carries the births, introductions,
  deaths, controller evaluations, rejected births and uptake since the publisher opened, through its own tick. A
  journal's replay runs before the publisher opens, so a recovered session counts from the recovered tick (the
  tally's `since`).

**Invariants — checkpoints (FL-016; §16.3, C.14, B.2).**
- FL-038: `checkpoint_into(world, book, sink)` writes the same container through a `flife_core::checkpoint::ChunkSink`
  a page at a time and returns its sealed prefix; `checkpoint` is it with an in-memory sink.
  `SessionHandle::checkpoint_with(cx, take)` runs `take` on the owner's task with the committed world, its book
  and the boundary, exactly where `checkpoint` would encode them: once every journal record sent has its
  receipt, and with no step until `take` returns. A closed session answers `take` with its error.
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

**Invariants — branches (FL-039; §14.4, §16.3).**
- `book.branch(session, tick, checkpoint)` copies the book: every record, pending or ended, the outcome stream, the
  admission numbering and the outcome cursor. The world it is checkpointed with therefore continues exactly as the
  parent's would. It appends one `Fork`: the parent session, the next admission order at the fork, the tick and
  the parent checkpoint's SHA-256. The branch is open even if the parent was closed. It starts `Volatile`, and the
  caller sets its durability. The parent book is unchanged.
- Every admission order has exactly one admitting session, `admitted_by(order)`. An order before a fork's
  `admitted` belongs to that fork's parent, taking the oldest such fork; any later order belongs to the book's
  own session. A branch admits only its own session's commands (`WrongSession` otherwise). Identities are still
  one namespace per book, so a branch command that reuses an inherited identity is `ConflictingReuse`.
- A book that is not a branch writes FL-016's commands chunk, version 1, byte for byte as before. A branch writes
  version 2: the same fields, then its lineage, each fork with its parent, admitted order, tick and checkpoint
  SHA-256. Only the commands chunk of a branch's checkpoint differs from its parent's checkpoint at the same
  tick (tested).
- `restore` of a version 2 chunk requires a lineage of at least one fork. Each fork's session must be distinct
  from the others and from the book's. Each fork's admitted order must be at most the next order, and its tick at
  most the committed tick. Forks must be in order of admissions and ticks. Every record's command must be of the
  session that `admitted_by` its order.
- `recover` and the journal (`flife-store`) apply the same rule. An inherited acceptance must be the parent's, and
  an acceptance after the fork must be the branch's.

**Invariants — write-ahead-before-apply (FL-036; §12.3, §12.4, §16.6, C.14).**
- One writer thread (`flife-journal`) owns the journal and appends records in the order the owner sends them,
  answering each with one receipt. The owner never waits on it: it sends records and reads receipts as messages.
- An acceptance is answered only once its receipt says it, and every acceptance before it, is durable. A retry of
  an identity still waiting shares that receipt. Its durability is then `WriteAhead`.
- A boundary is held while an acceptance due there waits for its receipt, while any cancellation does, or while
  the records waiting reach the session's limit (command plus control capacity). The control lane stays live
  meanwhile; a step or checkpoint requested then waits with the boundary. An acceptance for a later tick never
  holds the current one.
- A cancellation is answered once its record is durable. One that wins at the cancellation boundary discards the
  candidate first, and the candidate is rebuilt without the command only once the record is durable.
- Outcomes are journaled as they occur without holding the boundary; one lost in a crash is reproduced by replay.
  `durable_outcome` answers only once the outcome's cursor is below the durable outcome watermark.
- Appends are idempotent by identity, and each record must be the next of its kind (admission order, outcome
  cursor), so the journal never holds a record without every earlier one. The writer retries a failed append
  `JOURNAL_ATTEMPTS` (3) times; then the failure is sticky and every later record fails without being tried.
- A failure is uncertain, not a refusal: every request waiting on it is answered with the error, the uncertain
  identities are remembered, the run stops and new commands are refused, while control and close still work. A
  retry of an uncertain identity is refused in that session; a reopened session reconciles it from what the journal
  actually holds, by identity.
- `open_journaled` merges the journal into the starting book (`CommandBook::recover`): acceptances numbered from
  zero, those the book holds identical in content and admission, new ones due at or after the committed tick, a
  journal never behind the book, and outcomes contiguous by cursor, the prefix equal to the book's. Every outcome
  the book does not hold is replayed: the world advances through the same boundaries with the same commands, each
  journaled cancellation is applied at its boundary, and every outcome must be reproduced exactly. The session
  then starts at the boundary the replay reached, with every later acceptance pending and nothing journaled twice.
- Closing journals the session-closed cancellations (suspending journals none), answers everything queued, waits
  for every receipt and then stops the writer, so `SessionClosed.journal` reports the final durable watermarks.

**Refusal cases.**
- `SessionConfig::from_config`:
  - any durability other than `volatile-in-memory` and `write-ahead-before-apply` is refused as
    `UnknownAlgorithm`;
  - a zero capacity is refused.
- `open` and `open_with` refuse `write-ahead-before-apply` as `MissingCapability`, naming `open_journaled`.
  `open_journaled` refuses (`StateConflict`) a volatile configuration, a book, world and limits that disagree on the
  session or configuration, a journal that disagrees with the book, and a replay that does not reproduce the
  journal (an outcome that differs, or a journaled cancellation of a command that already ended). `durable_outcome` on a volatile session is a `StateConflict`.
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
  positive or above `f32::MAX`. A `RemoveOrganisms` is refused for a centre outside the world, or a radius that
  is not finite, not strictly positive or above half the shorter world side.
- `Command::from_canonical_bytes` refuses (`IncompatibleCheckpoint`) an unknown payload or disposition tag, a tag
  longer than 32 bytes, and trailing or missing bytes.
- FL-039: `CommandBook::branch` refuses (`StateConflict`) the book's own session or one in its lineage.
  `restore` refuses (`IncompatibleCheckpoint`) a commands chunk of another version, a version 2 chunk with no fork,
  and a lineage that is not one (see the branch invariants).

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
- `tests/session.rs` (16 tests; until FL-024's hold it had 15, though this list counted 13 and named neither the
  started run nor `open_restored`'s refusals), with the owner and clients as separate Asupersync Lab tasks,
  quiescent and invariant-clean under each seed:
  - `volatile-in-memory` admitted, and `write-ahead-before-apply` refused by `open` with a pointer to
    `open_journaled`;
  - C.14's full scientific queue while paused, with inspect, step, run, pause, resume and close still served;
  - commands applied at exactly their ticks in admission order, with the world equal to the direct one;
  - retries returning the first outcome, conflicting reuse, late and retargeted targets, and a wrong revision;
  - C.8: a cancellation racing sealing, the candidate and publication over 24 seeded interleavings. All three
    outcomes occur (before sealing, at the cancellation boundary, too late), the world shows exactly the winner,
    and each seed replays;
  - a pause while running, at a reported boundary from which the next step advances one tick;
  - a started run reports its start at once and its end later: an open run ends on a pause at the pause's
    boundary, a bounded run at its tick, and a run open at the close ends closed;
  - FL-024, over six seeds: a hold set at tick 0 is reached by a run at tick 3, which stays running there (the
    snapshot's boundary and hold, a checkpoint of that boundary); moved to 7, the run reaches it; a hold at 5 is
    refused and 7 stays; a pause at the hold ends the run there; a step requested there by another client does not
    pass it (the boundary unmoved 80 yields later) until the hold is released, then publishes 8; a moved and a
    released hold's waits end in state conflicts and an open one's in the session's close; the closed world is the
    direct one at 8;
  - a capacity pause that keeps its boundary sealed: the identical retry, a refused late command, and a sealed
    cancellation;
  - a numerical fault that fails every unfinished command, refuses new work, steps and runs, and preserves the
    committed state;
  - closing while running, which cancels unfinished commands and refuses later requests;
  - suspending while running, over four seeds: the unfinished command stays pending, the book open and no outcome
    recorded, later requests are refused; with a close sent in the same poll the close wins;
  - dropping every handle, while paused and while running (the latter within a 2,000-step Lab bound, so an owner
    that ignores the dropped handles fails quickly instead of hanging);
  - `open_restored` refuses a book of another session or another revision (state conflict) and a
    `write-ahead-before-apply` limit (missing capability), and opens the world's own book;
  - the same protocol on the native current-thread runtime.
- `tests/measure.rs` (3 tests, plan §14.3), Lab seeds, each answer compared with the measurement of a world
  stepped directly to the answer's tick, both energies bit for bit:
  - paused at tick 6, over four seeds: three disks (across both seams, small, half the world), each answered at
    tick 6; a disk wider than half the world refused as a `measurement radius`; the boundary unmoved, and the closed
    world the direct one;
  - during a run to tick 24, over four seeds: the disks measured one after another, each answer the direct
    measurement at its own tick, at three or more distinct ticks in all; the run reaches 24 at the direct digest;
  - a measurement sent in the same poll as a close while running, over six seeds: answered at the closed boundary
    with the direct measurement there; the next is refused.
- `tests/inspect.rs` (6 tests, FL-021), Lab seeds, each answer compared with the snapshot or weight dump of a world
  stepped directly to the answer's tick:
  - paused at tick 6, over four seeds: every live organism's answer, at tick 6; an unknown identity, none; the
    boundary unmoved, and the closed world the direct one;
  - during a run to tick 24, over four seeds: founders inspected one after another, each answer the direct
    snapshot at its own tick, some answered strictly between the run's start and end; an organism answered with
    none is dead at tick 24; the run reaches 24 at the direct digest;
  - an inspection sent in the same poll as a close while running, over six seeds: answered at the closed boundary
    with the direct snapshot of an organism alive throughout; the next inspection is refused;
  - paused at tick 6, over four seeds: an organism's first inspection records nothing, and after three steps its
    next is exactly the snapshot of a direct world that watched it from tick 6, observation and activations
    included; the closed world is that world;
  - weight dumps paused at tick 6, over four seeds: every live organism's dump the direct world's at tick 6, an
    unknown identity none; a survivor dumped last and first inspected three steps later has neither what it
    sensed nor activations (a dump watches nothing); the closed world at tick 9 the direct one;
  - weight dumps during a run to tick 24 and then a second run, over six seeds: founders dumped one after another,
    each the direct dump at its own tick, some strictly between the run's start and end; a dump and a close sent
    in the same poll while the second run goes, the dump answered at the closed boundary for a founder alive
    throughout (when the run's step deferred it, by the closing owner as it shuts down); the next dump refused;
    the closed world the direct one at its tick.
- Planted bugs in the weight dumps (FL-021), each caught: a running owner, or a closing one, answering none, and
  a dump that watches its organism (with the CLI's and the Studio's, 10 of 10). The closing owner's answer
  survived until the test sent its dump and close while a run was going: an idle owner answers the dump before
  it shuts down.
- Planted bugs in the hold (FL-024), each caught by the hold test: a step passing it; a hold that holds neither
  runs nor steps (caught when the Lab's step bound ran out, about six minutes); a wait never answered; a hold at
  a passed tick taken; a moved hold's wait dropped unanswered; a release that does nothing; the snapshot naming
  no hold.
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
- `tests/telemetry.rs` (1 test, FL-024), over two Lab seeds: a session submits an introduction (4 organisms at
  tick 30) and a removal (a disk of radius 12 at tick 40), steps 48 ticks while a viewer holds two epochs from step
  10 to 20, runs to tick 60 holding one, then to 96 holding two. Every epoch the feed shows, the founding one, the
  stale one kept while every slot is held and the pending one published on release included, carries a tally
  equal to a sum over a world stepped directly with the same commands at the shown tick (uptake bit for bit), with
  births, deaths and the four introductions counted. Two planted bugs in the owner were caught by it: the tick not
  counted, and counted after it is offered (each epoch then carries no tally).

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

- FL-036's journaled sessions are tested in `flife-store`'s `tests/journal.rs`, against its FrankenSQLite journal
  and test journals that gate, lose or crash at appends (see that crate's contract).
- `tests/branch.rs` (4 tests, FL-039), on a book that applied one command and holds another pending at its fork:
  - `a_branch_inherits_every_command_and_admits_only_its_own`. The fork is recorded, and `admitted_by` attributes
    each order to its session. The branch has the same history, cursor and pending command. It refuses the
    parent's session and a reuse of an inherited identity, and numbers its own command next. The parent is
    unchanged. Re-branching into a session of the lineage is refused, and a closed book branches into an open,
    volatile one.
  - `a_branch_round_trips_with_its_lineage_and_continues_its_parents_world`. The branch's checkpoint has a
    version 2 commands chunk, restores to the same lineage and history, and re-encodes to the same bytes. It
    differs from the parent's checkpoint in the commands chunk alone. A grandchild carries both forks. The parent
    and the branch both continue to the digest of the world stepped directly with the same interventions, with the
    same outcomes.
  - `a_lineage_that_is_not_one_is_refused`. A fork tick after the committed tick is refused on restore. `recover`
    refuses an inherited acceptance of the branch's session and a later one of the parent's, and accepts the
    branch's own.
  - `a_crafted_lineage_is_refused_on_restore`. A grandchild's checkpoint is re-sealed with its commands chunk
    edited, and an unedited rewrite reproduces it byte for byte. Each of these is refused: a repeated session;
    the book's own session as a parent; a fork past the admissions; forks out of admission or tick order; a
    lineage that gives an inherited command to the branch; a version 2 chunk with no fork; and version 3.

**Validity domain.** `volatile-in-memory` and `write-ahead-before-apply` durability; `cpu-reference` and
`qualified-reference`; one owner task on any Asupersync runtime, plus the journal's writer thread for a journaled
session; `InjectEnergy` and `RemoveOrganisms` as the interventions.

**Known gaps.**
- `volatile-in-memory` sessions still lose everything since opening when the process ends; that is the profile.
- Write-ahead durability is as strong as the journal's flush: process crashes are tested (SIGKILL at each C.14
  point), power loss is not.
- Idempotency records, in the book and the journal, are kept for the session's whole life. No bounded window or
  journal compaction yet.
- A failed journal stops the session for good; recovering means reopening from the journal. No in-place retry.
- Replay from a checkpoint re-runs every tick after it. `flife run --run-dir` and `flife resume` (FL-037, in
  `flife-cli`) open journaled runs and resume them from the newest valid checkpoint; `SessionHandle::checkpoint` and
  the library calls `checkpoint`/`restore` remain the only checkpoint paths here.
- `block_on_inline` must not run on an Asupersync runtime thread (FrankenSQLite reaches for the thread's runtime
  while it parks, and an open never completes), so journal I/O runs on the writer thread or a plain thread.
- Area brushes, source edits and the native input mapping are FL-022. Single-cell `InjectEnergy` (FL-014),
  `InjectMaterial` and `IntroduceOrganisms` (FL-022) and disk `RemoveOrganisms` (FL-022) are the canonical
  payloads so far.
- `tests/removal.rs` (3 tests, FL-022): an injection's record against one written out by hand, and a removal's
  round trip for each disposition, three distinct content hashes; unknown, unterminated, trailing and truncated
  records refused; and the book admitting a removal for tick 20 and applying it there, its applied outcome the
  removed organisms' energy (1e-12) and the world equal to one stepped directly with the same removal.
- `tests/material.rs` (3 tests, FL-022): a material's record ends with its tag, cell, amount and all 32
  components' bits written out by hand, shares the injection's prefix and round-trips; swapped components, a
  `-0.0` component and a zero-direction material against an injection give distinct content hashes; a record one
  byte short, one byte long or with a misspelled tag is refused; the book refuses a direction of norm above one,
  admits one inside for tick 9 and applies it there, its outcome the reported energy's bits and the world equal
  to one stepped directly with the same material.
- `tests/introduction.rs` (3 tests, FL-022): an introduction's record ends with its tag, cell, count and energy
  written out by hand, shares the injection's prefix and round-trips, and a different cell, count or energy gives
  another content hash; a record one byte short, one byte long or with a misspelled tag is refused; the book
  refuses an introduction of no organisms, admits seven at 2.25 for tick 9 and applies them there, its outcome
  `7 × 2.25` bit for bit and the world equal to one stepped directly with the same introduction.
- The tick runs on the owner task. Admitting it as bounded compute work with cancellation between tiles is later,
  with the CPU worker budget (§12.5) and accelerated backends. Responsiveness while running is one tick.
- Branching (FL-039) is a library operation on a book at a checkpoint (`flife branch` in `flife-cli`), not a session
  request or a command; checkpoints are control requests, not commands. Render epochs are not commands: viewers
  read the feed directly (FL-017).
- The render projection runs on the owner task, O(live + texels) per published boundary.
- Lab tests cover protocol outcomes and publication invariants under seeded interleavings, not every possible
  execution schedule (§12.7).
