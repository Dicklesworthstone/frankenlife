# flife-store contract

**Purpose.** FL-035 (`flife-rmr`) is the checkpoint chunk store and its atomic manifest (plan §16.4, §16.6, §16.7,
Appendix B.2). It publishes FL-016's checkpoints so that a crash, a cancellation, a refused storage operation or
damage to stored bytes always leaves the last valid checkpoint recoverable. It keeps the newest valid checkpoints
under a retention limit.

**Inputs.**
- A checkpoint: FL-016's sealed container (`flife_runtime::checkpoint` writes one).
- A store directory and a `Durability` profile, `process-crash` or `power-loss`.
- An observer for `publish_observed`, called after each step, which may cancel before the commit point.
- An acceptance function for `latest_valid` and `retain`, normally `flife_runtime::restore`.

**Outputs.**
- `ChunkStore` with `open`, `publish`, `publish_observed`, `load`, `latest_valid`, `retain`, `sequences`,
  `manifest_path`, `chunk_path`, `next_sequence` and `swept`.
- `Publication`: the sequence number and how many chunks were written, shared and repaired, with the bytes written.
- `Recovered`: the newest accepted checkpoint and every newer one skipped, each with its error class and reason.
- `Retention`: the valid checkpoints counted, the manifests removed, and the chunk files removed and kept.
- `Step` and `Flow`, and the constants `STORE_FORMAT` and `MANIFEST_FILE_MAX_BYTES`.
- In `flife-core`: `checkpoint::CheckpointManifest` and `ManifestEntry`, which verify a container's sealed prefix
  without its payload (`CheckpointReader::parse` now builds on them), and the `storage-failed` error class.

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

**Refusal cases.**
- `incompatible-checkpoint`: a container that does not verify, or a missing, oversized, mis-sealed or extended
  manifest, or a missing, mis-sized or damaged chunk (when loading).
- `state-conflict`: a second writer, a non-store directory, and `retain(0)`.
- `schema-mismatch`: another store format.
- `storage-failed`: any operation the host refuses (a full disk, a device or permission failure, a path in the way),
  naming the operation and path.
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

**Validity domain.** `frankenlife.chunk-store.v1` over checkpoint format 1 with raw chunks. A local POSIX
filesystem with atomic `rename`, hard links and advisory `flock` locks: APFS on macOS, and ext4, XFS or btrfs on
Linux. One writer per store.

**Known gaps.**
- `power-loss` is a protocol, not a proven guarantee. The flush order is tested only by the step sequence: no test
  cuts power, and a drive that ignores flushes defeats it. Process crashes are tested.
- A real full disk is not exercised: the size limit produces the same short write, from the same code path.
- No index: a database of checkpoints, runs and lineage is FL-036, and `flife checkpoint`/`resume` and resuming a
  session from the store are FL-037.
- No readers' leases: retention assumes no other process is reading the store. Pinned discoveries and branch
  closures (§16.7) are not retained specially yet (FL-039).
- Chunks are whole families: a single birth rewrites the full genome chunk. Per-genome pages, deltas and
  compression are later work (§16.5, FL-026), as are incremental and background checkpoint modes (§16.5).
- No RaptorQ or other error correction: damage is detected and recovery falls back, but damaged bytes are not
  reconstructed (§16.7).
