# flife-store contract

**Purpose.** FL-035 (`flife-rmr`) is the checkpoint chunk store and its atomic manifest (plan §16.4, §16.6, §16.7,
Appendix B.2). It publishes FL-016's checkpoints so that a crash, a cancellation, a refused storage operation or
damage to stored bytes always leaves the last valid checkpoint recoverable. It keeps the newest valid checkpoints
under a retention limit. FL-036 (`flife-ka2`) adds the run catalog and command journal (§12.3, §16.6, Appendix B.1,
C.14): one FrankenSQLite database that records each run, makes a `write-ahead-before-apply` session's acceptances,
cancellations and outcomes durable through `flife_runtime`'s `Journal` port, and indexes the checkpoints published
to the chunk store.
FL-039 (`flife-05q`) adds publication from a donor store and branch catalogs (§14.4, §16.7, B.1). A branch's
founding checkpoint links the parent store's verified chunk files instead of copying them. A branch's catalog
records its lineage and origin, and starts its journal with the history it inherited.
FL-041 (`flife-rln`) adds the experiment catalog (§15.5, B.1): one FrankenSQLite file per experiment directory
that keeps the experiment's identity and design and one row per run, however the run ended, so a continued
experiment knows what already ran and a report can list every run.

**Inputs.**
- A checkpoint: FL-016's sealed container (`flife_runtime::checkpoint` writes one).
- A store directory and a `Durability` profile, `process-crash` or `power-loss`.
- An observer for `publish_observed`, called after each step, which may cancel before the commit point.
- An acceptance function for `latest_valid` and `retain`, normally `flife_runtime::restore`.
- FL-038: a checkpoint encoded into a `Stream` (`flife_runtime::checkpoint_into`) and its sealed prefix.
- For the journal (FL-036): a database path, a `SessionId` and its configuration hash, `flife_runtime` journal
  records, and checkpoint index rows (sequence, tick, epoch, container SHA-256).
- FL-041: a database path, an experiment identity and its design text, and `CatalogedRun` rows (ordinal,
  condition, split, seed, configuration hash, status, and the caller's result and manifest JSON and records hash).

**Outputs.**
- `ChunkStore` with `open`, `publish`, `publish_observed`, `load`, `latest_valid`, `retain`, `sequences`,
  `manifest_path`, `chunk_path`, `next_sequence` and `swept`.
- `Publication`: the sequence number and how many chunks were written, shared and repaired, with the bytes written.
- `Recovered`: the newest accepted checkpoint and every newer one skipped, each with its error class and reason.
- `Retention`: the valid checkpoints counted, the manifests removed, and the chunk files removed and kept.
- `Step` and `Flow`, and the constants `STORE_FORMAT` and `MANIFEST_FILE_MAX_BYTES`.
- In `flife-core`: `checkpoint::CheckpointManifest` and `ManifestEntry`, which verify a container's sealed prefix
  without its payload (`CheckpointReader::parse` now builds on them), and the `storage-failed` error class.
- `CommandJournal` (FL-036) with `open`, `path`, `session`, `modes`, `watermarks`, `index_checkpoint`,
  `checkpoints` and `runs`, implementing `flife_runtime::Journal` (`open_run`, `append`); `IndexedCheckpoint`; and
  `CATALOG_FORMAT`.
- FL-038: `ChunkStore::stream`, `stream_observed`, `commit`, `verify` and `retain_intact`; `Stream` (a
  `flife_core::checkpoint::ChunkSink`) and `Observer`.
- FL-039:
  - `ChunkStore::publish_sharing` and `publish_sharing_observed`, taking a donor `ChunkStore`;
  - `Step::ChunkLinked`, and `Publication::linked`, a part of `written`;
  - `CommandJournal::open_branch`, `lineage` and `branch_origin`, and `BranchOrigin`.
- FL-041: `ExperimentCatalog` with `open`, `path`, `design`, `runs` and `record`; `CatalogedRun`; and
  `EXPERIMENT_CATALOG_FORMAT`.

**Invariants — layout.**
- `FLIFESTORE` holds `frankenlife.chunk-store.v1`. A directory that is neither empty nor a store is not taken over.
- `chunks/ab/abcd…` holds one file per distinct chunk, named by its SHA-256. A chunk unchanged between checkpoints
  is stored once: ticks 64 and 65 of the 4,096-organism preset share 158 of their 161 MiB, the genomes among it,
  because no genome changed between them.
- `checkpoints/<sequence>.manifest` is a checkpoint's sealed prefix (header, manifest and seal), with sixteen hex
  digits of sequence number. Numbers increase and are never reused, even after an invalid manifest.
- `tmp/` holds writes in flight. `open` takes the writer's lock and then removes every file in it.
- One writer at a time: `open` takes an exclusive lock on `lock`, and a second writer is refused (`state-conflict`).
  The kernel releases the lock when the process dies.

**Invariants — publication (§16.4).**
- The whole checkpoint is verified (`CheckpointReader::parse`) before anything is written, so a damaged container
  is never stored.
- Each chunk the store does not already hold intact is written to `tmp/`, flushed under `power-loss`, and renamed
  to its digest's path; under `power-loss` its directory is then flushed. A stored copy that is intact (its length
  and SHA-256 checked) is shared. A damaged one is stored again, which repairs every checkpoint that names it.
- Only then is the manifest written to `tmp/`, flushed under `power-loss`, and hard-linked into `checkpoints/`.
  The link is the commit point. It never replaces an existing manifest, and under `power-loss` `checkpoints/` is
  flushed afterwards.
- A cancellation at any step before the link, or a refused operation at any step, removes that attempt's
  temporary files (including a partially written one) and leaves the published checkpoints as they were. From
  the link on, a cancellation is too late and the checkpoint is published.
- `power-loss` flushes with `File::sync_all`, which on macOS is `fcntl(F_FULLFSYNC)` and on Linux `fsync`.
  `process-crash` relies only on rename and link atomicity.
- FL-039, `publish_sharing(checkpoint, donor)`: the same protocol, except for each chunk the store does not hold
  intact. If the donor holds that chunk intact (length and SHA-256 checked), the donor's file is hard-linked into
  `tmp/` and then renamed into place, so one inode carries both names. Otherwise the chunk is written. A linked
  chunk counts as written, and also as linked, with no bytes written. Under `power-loss` it is flushed like a
  written one, and so is its directory. The donor is only read. It is never written, and its retention removes only
  its own names, so a chunk both stores hold stays whole in the other.

**Invariants — streamed publication (FL-038; §16.4, §16.5).**
- A `Stream` writes each chunk's pages to `tmp/` as they are encoded. When the chunk ends it is stored exactly as
  `publish` stores it (shared when held intact, otherwise flushed under `power-loss`, renamed to its digest's path,
  and its directory flushed), with the same steps. A streamed checkpoint and the same container published whole
  give the same steps, publication, manifest and chunk files (tested under both profiles).
- `commit(stream, sealed)` refuses a failed stream (its own error), a stream of another store, a chunk still open,
  a sealed prefix that does not verify or is followed by bytes, one naming other chunks, other lengths or offsets,
  or more or fewer chunks than the stream stored, and a stored chunk no longer in place at its length. It then
  links the manifest as `publish` does, the same commit point.
- Chunks begin in manifest order; one begun out of order is refused. A stream's first refusal (a cancellation, a
  refused write) sticks: every later call and the commit return it. A stream dropped, cancelled or failed removes
  its temporary files; the chunks it already stored stay unreferenced until retention removes them.
- `verify(sequence)` checks what `load` checks, hashing each chunk a 1 MiB buffer at a time, and returns the
  container's SHA-256 without holding it. `retain_intact(n)` is `retain(n)` counting a checkpoint valid when it
  verifies; it never decodes one.

**Invariants — recovery and retention (§16.7, B.2).**
- `load` checks that the manifest is a regular file of at most `MANIFEST_FILE_MAX_BYTES`, and verifies its seal
  with nothing after it. It then checks every chunk's stored length against the manifest before reserving
  anything for the payload, and verifies each chunk's SHA-256 as it is read. The result is the original
  container, byte for byte.
- `latest_valid` tries checkpoints newest first and returns the first that loads and is accepted. It records every
  newer one with the reason it was passed over. A damaged checkpoint is never returned as a different world.
- `retain(n)` counts the newest `n` checkpoints that load and are accepted. It removes every manifest older than
  the oldest of them, then every chunk file that no remaining parseable manifest names. Invalid manifests newer
  than the oldest kept one stay, for inspection. With fewer than `n` valid checkpoints nothing is removed, and
  `n = 0` is refused. Manifests go before chunks, so a failure part way never breaks a kept checkpoint.

**Invariants — the run catalog and command journal (FL-036; §12.3, §16.6, B.1).**
- One FrankenSQLite file, opened in WAL mode with `synchronous = FULL` (FrankenSQLite's per-commit sync), so every
  append's commit is flushed before it returns. `catalog` holds `frankenlife.run-catalog.v1`.
- `runs` records each session once, with its configuration hash and the command schema. Opening a run under
  another hash or schema is refused.
- `commands` holds each acceptance, keyed by `(session, admission order)` and unique by `(session, command id)`,
  with its effective tick, retargeting, content hash and canonical `frankenlife.command.v1` record. `outcomes`
  holds each terminal outcome, keyed by `(session, cursor)` and unique by `(session, command id)`.
- Every append is one transaction. It returns the held watermarks without writing when the identity is already
  held with exactly that record, refuses the identity with other content, and otherwise requires the next number of
  its kind (admission order or outcome cursor) before inserting. An outcome needs its command's acceptance. So the
  two watermarks are contiguous counts, distinct from each other: an acceptance for tick 40 is durable while the
  outcome of one for tick 12 is the only outcome.
- A journal is bound to one session by `open_run`; an append before binding, or of a command addressed to another
  session, is refused.
- `checkpoints` indexes chunk-store publications by `(session, sequence)`; re-indexing the same row is a no-op and
  a different row for the same sequence is refused. The index never makes a checkpoint resumable: recovery asks the
  chunk store (`latest_valid`), whose manifests stay the authority (§16.6).
- The connection is `!Send`, so the journal is opened on the thread that uses it (the session's writer thread),
  and its async calls are driven by `flife_runtime::block_on_inline`, never on an Asupersync runtime thread.
- FL-039, branches:
  - `lineage` holds each branch session's forks, keyed by `(session, depth)`, root first. A fork row holds the
    parent session, the admissions it had made, the tick and the parent checkpoint's SHA-256.
  - `branches` holds each branch's origin: its name, the parent run, the parent checkpoint's sequence and the
    creation time.
  - `open_branch` records the run, its lineage, its origin and its inherited history (acceptances and outcomes) in
    one transaction, and then binds the session.
  - Reading a journal back applies `flife_runtime::admitting_session`. An acceptance before a fork's admissions
    must be of that fork's parent, and one after must be of the branch's own session.

**Invariants — the experiment catalog (FL-041; §15.5, B.1).**
- One FrankenSQLite file in WAL mode with `synchronous = FULL`. `catalog` holds the format
  `frankenlife.experiment-catalog.v1`, the experiment's identity and its design (written once, when the catalog is
  created); `runs` holds one row per run, keyed by ordinal and unique by `(condition, split, seed)`.
- The format is checked before anything else is created, so opening a database of another format (a run catalog,
  say) is refused without adding a table to it. Opening it for another identity is refused.
- `record` is one transaction. The same row again is a no-op; another row under a cataloged ordinal, or a
  cataloged `(condition, split, seed)` under another ordinal, is refused and nothing is written. `runs` reads back
  in ordinal order, whatever order they were recorded in. A `u64` is stored by its bits, so the full range of
  seeds round-trips.
- The catalog never interprets the result or manifest JSON; `flife-cli` writes and reads them.

**Refusal cases.**
- `incompatible-checkpoint`: a container that does not verify, or a missing, oversized, mis-sealed or extended
  manifest, or a missing, mis-sized or damaged chunk (when loading).
- `state-conflict`: a second writer, a non-store directory, and `retain(0)`. In the journal: a run under another
  configuration hash, an append before `open_run` or to another session, an identity held with other content, a
  record that is not the next of its kind, an outcome without an acceptance, and a conflicting index row.
  `open_branch` of a session the catalog already records, with no fork, or with a history that is not numbered
  from zero, or whose acceptance is not of the session that admitted its order.
- FL-041, `state-conflict`: an experiment catalog of another identity, a conflicting run row, or a malformed
  stored row.
- `schema-mismatch`: another store format, or another catalog format (FL-041: the experiment catalog's too).
- `storage-failed`: any operation the host refuses (a full disk, a device or permission failure, a path in the way),
  naming the operation and path. FrankenSQLite's errors map here too.
- `cancelled`: the observer cancelled before the commit point.
- `overflow`: sequence numbers exhausted.

**Determinism.** File contents are exactly the container's bytes: the manifest file is its sealed prefix and each
chunk file is that chunk's bytes. Sequence numbers come from the manifests present. Nothing depends on
wall-clock time or directory-listing order: sequences are sorted, and retention's chunk set is computed from
manifests.

**Independent tests.** `tests/store.rs`, against real checkpoints of FL-013's 48-founder ecology and the
4,096-organism preset, restored through `flife_runtime::restore`:
- Round trip, under both profiles: the store holds one file per distinct chunk, and each checkpoint loads back byte
  for byte and restores to its world's tick and digest. A damaged container is refused with nothing stored. A
  second writer, a foreign directory and another store format are refused. A reopened store continues the
  sequence.
- A crash at every step, under both profiles. A child process publishes and is killed (`SIGKILL`) right after each
  step in turn: all 18 for `process-crash` and all 25 for `power-loss`, checked against the steps of an
  uninterrupted publication. After each kill the reopened store has no temporary file and no invalid manifest, and
  recovers the previous checkpoint if the kill came before the manifest was linked and the new one if after. The
  interrupted checkpoint then publishes again.
- Cancellation at every step, under both profiles: before the link the publication is `cancelled`, no temporary
  file is left and only the previous checkpoint exists (17 and 23 steps); from the link on it is published.
- Damage falls back to the last valid checkpoint with a named reason, under each of these:
  - a flipped, truncated or missing chunk;
  - a flipped manifest byte, a truncated manifest, or a byte after its seal;
  - a re-sealed manifest declaring a 2^60-byte chunk, refused from the stored length before allocation;
  - a garbage manifest, whose number is then not reused.

  A damaged chunk two checkpoints share leaves none valid, and the next publication that needs it repairs both.
- A write the host refuses part way through a chunk (a child under `ulimit -f` with `SIGXFSZ` ignored: a short
  write, as on a full disk) fails with `storage-failed`, removes the partial file at once, and leaves the last
  valid checkpoint. So do a directory in the way of a chunk and a directory in the way of the next manifest.
- Retention of 6 checkpoints to 4 removes the oldest 2 and exactly the chunks only they named. The kept 4 load and
  restore. A damaged newest checkpoint is not counted and stays.
- At the preset's scale: 161.3 MiB published in 0.08–0.10 s (`process-crash`) or 0.23–0.26 s (`power-loss`), over
  two runs on an Apple-silicon Mac. The next tick's checkpoint shares 6 of its 11 chunks, including the genomes,
  so 3.1 MiB is written. Recovery (load, verify and restore) takes about 0.2 s.
- 21 planted mutants in the store and the refactored container reader, each caught by these tests or
  `flife-core`'s `tests/checkpoint.rs`. One (`load` skipping its own chunk hash) first survived, because restoring
  re-verifies the hashes; a direct `load` check now catches it.

- FL-038, streamed publication, against the ecology's checkpoints at ticks 0, 8 and 16:
  - `a_streamed_checkpoint_is_stored_as_the_whole_one_byte_for_byte`, under both profiles: the same steps,
    publication, manifest and chunk files as `publish_observed`, unchanged chunks shared, loading back to the
    in-memory container, and `verify` equal to its SHA-256.
  - `cancellation_at_every_streamed_step_leaves_the_store_as_it_was`: 17 steps (`process-crash`) and 23
    (`power-loss`), each refused as `cancelled` with no temporary file and only the previous checkpoint; from the
    link on it is published.
  - `a_crash_at_every_streamed_step_leaves_the_last_valid_checkpoint`: a child streaming the tick-16 checkpoint
    killed (`SIGKILL`) after each of its 18 and 25 steps; the reopened store has no temporary file and recovers the
    previous checkpoint before the link and the new one after.
  - `a_commit_refuses_what_its_stream_did_not_store`: another checkpoint's sealed prefix, another store's stream,
    a removed chunk, a stream holding only the first chunk, a chunk begun out of order and a cancelled stream are
    each refused, and nothing is published.
  - `retention_by_verification_decides_as_retention_by_restoring`: with no damage and with damage to the newest
    or a middle checkpoint, `retain_intact` keeps and removes exactly what `retain(restore)` does.
  - 14 planted mutants in `commit`, `verify`, `retain_intact` and `Stream`, and 2 in `flife-core`'s chunk writer
    (its page four times larger, caught by `flife-cli`'s memory test; a page left out of the chunk hash). 15 were
    caught on their first run. One (`commit` not refusing a chunk still open) survived until the open-chunk
    refusal case was added.

- `tests/journal.rs` (FL-036, 12 tests; about 2 s), against FL-013's 48-founder ecology with Appendix A's
  `write-ahead-before-apply` durability, sessions on Asupersync's native runtime with the journal's writer thread.
  Every committed world is compared with one stepped directly with the same interventions at the same ticks:
  - The catalog: WAL and `synchronous = FULL` read back; acceptances and outcomes numbered and idempotent; a
    conflicting reuse, a skipped or repeated number, a command addressed to another session, an unknown command's
    outcome and another outcome for an ended command refused; watermarks `(2, 1)` with the tick-40 acceptance pending; everything read back after
    reopening; the checkpoint index idempotent; another configuration hash and another catalog format refused.
  - A journaled session: acceptances answered as `WriteAhead`; retries, conflicting reuse and cancellation;
    commands applied at their ticks; the journal holding each record once; and a session reopened from the
    journal alone, from the founding world, replaying to the same world.
  - A gated journal: while an acceptance due at the boundary waits for its receipt, the boundary, a step and a
    checkpoint wait and inspection, status and pause answer; once released, the client hears, the step applies it
    and the checkpoint restores to its boundary's world. While a cancellation waits, the boundary and a step wait
    the same way, and the step publishes without the command once the canceller has heard.
  - The bound: with command capacity 4 and control capacity 1, four unacknowledged outcomes and one acceptance
    stop the owner taking submissions (they queue unadmitted) and hold the boundary; once released, all eight
    commands apply at their ticks.
  - C.14's lost receipt, from a journal that persists a record and then reports failure: reconciled within the
    writer's retries, and, when every retry fails, the client told the outcome is uncertain, the run stopped, a new
    command refused and not recorded, and a reopened session finding the record by identity and applying it once.
  - A failed append fails every record behind it: with a journal that would take the second record, nothing after
    the failed one is appended, and both clients hear the failure.
  - Recovery refusals, each naming the first disagreement: a journal behind the checkpoint, an acceptance with other
    content than the checkpoint's, a new command due before the checkpoint, an outcome at a boundary already passed,
    an outcome the replay does not reproduce, and a journaled cancellation of a command that already ended (which
    first made the replay loop forever; it now refuses). The unaltered journal opens from the checkpoint and from the
    founding world.
  - C.14's cancellation racing publication: won at the cancellation boundary (the candidate rebuilt without it
    once durable), or too late after publication, with the journal holding the winner.
  - C.14's crash points: a child process runs the energy brush and is killed (`SIGKILL`) at each of six points
    (before durable acceptance, after it, after the client's acknowledgement, after publication, after the outcome
    is durable, and after the client has the durable outcome). The journal holds the acceptance exactly when it was
    durable and the outcome exactly from its receipt on. The recovered session recognises the client's retry (or
    admits it afresh), refuses conflicting reuse, and applies the brush once, at tick 3.
  - C.14 with a checkpoint: A for tick 40 accepted before B for tick 12, C cancelled durably, a checkpoint at tick
    13 published to the chunk store and indexed, then a kill. Recovered from the checkpoint and the journal, A is
    pending and applies at tick 40, B applied once, C cancelled, each outcome journaled once. With the checkpoint's
    manifest damaged, its index row does not make it resumable: recovery falls back to the founding world and the
    journal, and replays to the checkpoint's world.
- 32 planted mutants in the write-ahead session, recovery and replay, the writer thread, the FrankenSQLite journal
  and `flife run`'s guard, each caught by these tests, `flife-runtime`'s `tests/session.rs` or `flife-cli`'s smoke
  test: 31 by a failing assertion, and one (the replay without its progress guard) by the recovery test hanging
  until the harness's timeout. A first pass over 31 left 12 surviving and one hanging. Each got a test or an
  assertion; the hang exposed a replay that looped forever on a journaled cancellation of a command that had
  already ended, which now refuses.
- `tests/branch.rs` (FL-039, 3 tests), over three checkpoints of FL-013's ecology:
  - `a_donors_verified_chunks_are_linked_not_written`, under both durabilities:
    - every chunk of the first checkpoint is linked: the donor's inode under the same name, with no bytes written;
    - the second checkpoint's chunks are shared or linked, with `ChunkLinked` and no `ChunkWritten` steps, and as
      many flushes as links under `power-loss`;
    - a chunk the donor holds damaged is written, as the recipient's own file;
    - each publication restores to its world;
    - the donor's retention removes its first checkpoint, and the recipient's copy still restores.
  - `cancelling_a_sharing_publication_at_any_step_leaves_both_stores_as_they_were`, under both durabilities. A
    cancellation at every step before the link refuses as `cancelled` and leaves no temporaries and the recipient's
    checkpoints as they were. From the link on, it publishes. The donor's files are unchanged.
  - `a_branch_catalog_records_its_lineage_and_starts_with_its_inherited_history`:
    - a branch catalog opens with the inherited watermarks and takes its own acceptance and an inherited command's
      outcome;
    - it refuses a parent-session acceptance after the fork, and recording the branch twice;
    - reopened, it reads back the history in order, the lineage, the origin and one run;
    - no fork, an inherited acceptance of the branch's session, and an outcome out of turn are each refused, with
      nothing recorded.

`tests/experiment_catalog.rs` (2 tests, FL-041):
- `the_catalog_keeps_each_run_once_in_run_order_across_reopening`: four rows (reached, error, capacity-paused,
  failed; one with seed `u64::MAX`) recorded out of order read back in order, before and after reopening, with
  the design kept from creation. The same row again is a no-op; a changed row under its ordinal and a moved
  `(condition, split, seed)` are each refused with nothing written; the same condition and seed in the other split
  is another run.
- `a_catalog_of_another_experiment_or_format_is_refused_unchanged`: another identity is refused, naming the held
  one, and the catalog still opens for its own; an FL-036 run catalog is refused as `schema-mismatch` (expected
  and found formats) and still opens as a run catalog afterwards.

Four planted mutants were each caught: the schema created before the format check, the format not checked, an
identical row refused, and a conflicting row accepted without being written.

**Validity domain.** `frankenlife.chunk-store.v1` over checkpoint format 1 with raw chunks. A local POSIX
filesystem with atomic `rename`, hard links and advisory `flock` locks: APFS on macOS, and ext4, XFS or btrfs on
Linux. One writer per store. The journal: `frankenlife.run-catalog.v1` on FrankenSQLite 074c2033 (the `native`
file-backed engine), one writer thread per session, one session per database at a time.

**Known gaps.**
- `power-loss` is a protocol, not a proven guarantee. The flush order is tested only by the step sequence: no test
  cuts power, and a drive that ignores flushes defeats it. Process crashes are tested.
- A real full disk is not exercised: the size limit produces the same short write, from the same code path.
- A run's catalog row is its identity, configuration hash and command schema, plus its journal and checkpoint
  index. The rest of Appendix B.1's run manifest (source and suite revisions, toolchain, numerics, clocks,
  hardware, capacity, results and artifacts) is recorded only for experiment runs, in FL-041's experiment catalog;
  a run kept with `flife run --run-dir` has none.
- The catalog records a branch's lineage and origin (FL-039), but not lineage of organisms: FL-040's lineage
  summaries are `flife run` records, not catalog rows. `flife checkpoint`/`resume`, a CLI
  that opens a journal (`flife run` refuses `write-ahead-before-apply`) and resuming from the store are FL-037.
- The journal's power-loss durability rests on FrankenSQLite's WAL and `synchronous = FULL` flush; only process
  crashes are tested. Two sessions writing one catalog at once, and two processes opening one session's journal,
  are not tested; FrankenSQLite's concurrent writers are not relied on.
- No journal compaction or bounded idempotency window: every record is kept for the session's life.
- No readers' leases: retention assumes no other process is reading the store. Pinned discoveries and branch
  closures (§16.7) are not retained specially. A branch shares its parent's chunks by hard links instead
  (FL-039), so a parent's retention cannot remove a chunk a branch names. Across file systems a link fails and the
  chunk is written, which is not tested. A linked chunk damaged in place is damaged in every store that names it.
  It is detected on load, as any damage is.
- Chunks are whole families: a single birth rewrites the full genome chunk. Per-genome pages, deltas and
  compression are later work (§16.5, FL-026). A streamed checkpoint is written in full each time; §16.5's
  incremental mode (only changed pages) does not exist. `flife-cli` adds a background mode (FL-038).
- A `Stream` trusts the length and digest its encoder reports for each chunk; `commit` checks them against the
  sealed prefix and the stored files' lengths, not their hashes. `verify` (which `flife-cli` runs after every
  commit) is the hash check.
- `retain_intact` counts a checkpoint that verifies, not one that restores: a checkpoint whose chunks hash
  correctly but do not decode (a writer bug, never damage) would be kept. `latest_valid` still restores.
- No RaptorQ or other error correction: damage is detected and recovery falls back, but damaged bytes are not
  reconstructed (§16.7).
- FL-041: the experiment catalog is written by one process at a time; two processes continuing one experiment
  directory are not prevented (no lock) or tested. `CommandJournal::open` still creates its tables before checking
  the format, so pointing it at an experiment catalog adds empty journal tables to that file before refusing it.
