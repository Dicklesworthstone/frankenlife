//! FrankenLife's checkpoint chunk store (FL-035; plan §16.4, §16.6, §16.7,
//! Appendix B.2).
//!
//! A store is a directory:
//!
//! | Path | Content |
//! |---|---|
//! | `FLIFESTORE` | the store format, [`STORE_FORMAT`] |
//! | `lock` | held exclusively by the one writer |
//! | `chunks/ab/abcd…` | one file per chunk, named by its SHA-256, so equal chunks are stored once |
//! | `checkpoints/<sequence>.manifest` | a checkpoint's sealed prefix (header, manifest and seal; [`CheckpointManifest`]) |
//! | `pins/<sequence>.<holder>.pin` | an empty file: the holder (a saved discovery, FL-042) pins that checkpoint |
//! | `tmp/` | writes in flight; emptied when the store is opened |
//!
//! [`ChunkStore::publish`] follows §16.4. It verifies the whole checkpoint,
//! writes each chunk the store does not already hold intact to a temporary
//! file, flushes it under the store's [`Durability`], and renames it to its
//! digest's path. Only then does it write the manifest and link it into
//! `checkpoints/`. That link is the commit point: before it the checkpoint
//! does not exist, after it every chunk it names is in place. A crash,
//! cancellation or I/O failure at any earlier point leaves the store's
//! published checkpoints as they were, plus at most some unreferenced
//! chunks and temporary files.
//!
//! [`ChunkStore::publish_sharing`] (FL-039) publishes the same way, but
//! takes a chunk it lacks from a donor store, another run's, by linking the
//! donor's file (a hard link: one immutable file under two names) after
//! verifying its hash, rather than writing the bytes again; a branch shares
//! its parent's genomes, field and every other unchanged chunk this way. A
//! chunk the donor lacks or holds damaged, or one the host will not link
//! (another file system), is written as usual. Removing a name in one store
//! leaves the other's: retention in either never reaches into the other.
//!
//! [`ChunkStore::latest_valid`] reads checkpoints newest first. It checks
//! each manifest's seal, each chunk's stored length before allocating, and
//! each chunk's hash, then offers the reassembled checkpoint to the caller
//! (who restores it). The first one accepted is returned, with the reason
//! each newer one was skipped. [`ChunkStore::retain`] keeps the newest valid
//! checkpoints and removes older manifests and every chunk no remaining
//! manifest names.
//!
//! [`ChunkStore::pin`] pins a checkpoint for a holder (§16.7: "Garbage
//! collection traces from committed manifests and pinned discoveries"):
//! while pins are preserved ([`ChunkStore::set_preserve_pinned`], the
//! configuration's `preserve_pinned_discoveries`; the default), retention
//! keeps a pinned manifest however old, and so every chunk it names. A
//! checkpoint stays pinned until its last holder [unpins](ChunkStore::unpin)
//! it.
//!
//! The store holds checkpoints; it does not decode them. Resuming a session
//! from one is FL-037.
//!
//! [`journal`] is FL-036's run catalog and command journal: a FrankenSQLite
//! database implementing `flife_runtime`'s journal port, through which a
//! `write-ahead-before-apply` session makes acceptances and outcomes
//! durable, and which indexes the checkpoints published here.
//!
//! [`experiment`] is FL-041's experiment catalog: a FrankenSQLite database
//! per experiment directory holding the experiment's identity and design
//! and one row per run, whatever its ending.

#![forbid(unsafe_code)]

pub mod experiment;
pub mod journal;
mod stream;

pub use experiment::{CatalogedRun, EXPERIMENT_CATALOG_FORMAT, ExperimentCatalog};
pub use journal::{BranchOrigin, CATALOG_FORMAT, CommandJournal, IndexedCheckpoint};
pub use stream::{Observer, Stream};

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use flife_core::checkpoint::{
    CheckpointManifest, CheckpointReader, ChunkFamily, MANIFEST_MAX_BYTES, MODEL_MAX_BYTES,
    ManifestEntry,
};
use flife_core::{CoreError, Sha256Digest, Sha256Stream};

/// The store format written to `FLIFESTORE`.
pub const STORE_FORMAT: &str = "frankenlife.chunk-store.v1";

/// The largest sealed prefix a manifest file can hold.
pub const MANIFEST_FILE_MAX_BYTES: u64 =
    8 + 4 + 1 + 2 + MODEL_MAX_BYTES as u64 + 4 + MANIFEST_MAX_BYTES as u64 + 32;

const MARKER: &str = "FLIFESTORE";
const LOCK: &str = "lock";
const CHUNKS: &str = "chunks";
const CHECKPOINTS: &str = "checkpoints";
const TEMPORARY: &str = "tmp";
const PINS: &str = "pins";
const MANIFEST_SUFFIX: &str = ".manifest";
const PIN_SUFFIX: &str = ".pin";

/// What a published checkpoint survives (§16.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// Atomic renames and links, no flushes. A published checkpoint survives
    /// the process crashing or being killed at any point, but not the host
    /// losing power.
    ProcessCrash,
    /// Every chunk and manifest is flushed to stable storage (`fsync`;
    /// `F_FULLFSYNC` on macOS) before it is renamed or linked, and each
    /// directory is flushed after its entry changes. A chunk's directory
    /// entry is durable before any manifest that names it is published.
    PowerLoss,
}

impl Durability {
    /// The profile's name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::ProcessCrash => "process-crash",
            Self::PowerLoss => "power-loss",
        }
    }

    const fn flushes(self) -> bool {
        matches!(self, Self::PowerLoss)
    }
}

/// A step of [`ChunkStore::publish_observed`], reported after it completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Chunk `index` is already stored intact and is shared.
    ChunkShared {
        /// The chunk's position in the manifest.
        index: usize,
    },
    /// Chunk `index` is linked from the donor store into a temporary name
    /// ([`ChunkStore::publish_sharing`]).
    ChunkLinked {
        /// The chunk's position in the manifest.
        index: usize,
    },
    /// Chunk `index`'s bytes are in a temporary file.
    ChunkWritten {
        /// The chunk's position in the manifest.
        index: usize,
    },
    /// That file is flushed ([`Durability::PowerLoss`] only), whether
    /// written or linked.
    ChunkFlushed {
        /// The chunk's position in the manifest.
        index: usize,
    },
    /// The file is renamed to its digest's path, and for power loss the
    /// directory is flushed.
    ChunkStored {
        /// The chunk's position in the manifest.
        index: usize,
    },
    /// The manifest is in a temporary file.
    ManifestWritten,
    /// That file is flushed ([`Durability::PowerLoss`] only).
    ManifestFlushed,
    /// The manifest is linked into `checkpoints/`: the checkpoint is
    /// published. A cancellation from here on is too late.
    Published,
    /// `checkpoints/` is flushed ([`Durability::PowerLoss`] only).
    DirectoryFlushed,
}

/// What an observer wants after a [`Step`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Go on.
    Continue,
    /// Stop and leave the store as it was. Ignored from
    /// [`Step::Published`] on.
    Cancel,
}

/// What a publication did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    /// The new checkpoint's sequence number.
    pub sequence: u64,
    /// Chunks the store did not hold before, written or linked.
    pub written: usize,
    /// Of those and the repaired ones, how many were linked from a donor
    /// store rather than written ([`ChunkStore::publish_sharing`]).
    pub linked: usize,
    /// Chunks already stored intact, now shared.
    pub shared: usize,
    /// Chunks stored again because the stored copy was damaged.
    pub repaired: usize,
    /// Chunk bytes written.
    pub bytes_written: u64,
}

/// A checkpoint [`ChunkStore::latest_valid`] passed over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// Its sequence number.
    pub sequence: u64,
    /// The class of the error it failed with ([`CoreError::kind`]).
    pub kind: &'static str,
    /// The error.
    pub reason: String,
}

/// The newest checkpoint that was accepted, and every newer one that was
/// not.
#[derive(Debug)]
pub struct Recovered<T> {
    /// The accepted checkpoint's sequence number and what the caller made
    /// of it, if any checkpoint was accepted.
    pub latest: Option<(u64, T)>,
    /// Newer checkpoints passed over, newest first.
    pub skipped: Vec<Skipped>,
}

/// What [`ChunkStore::retain`] kept and removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retention {
    /// The valid checkpoints counted toward the limit, newest first.
    pub valid: Vec<u64>,
    /// Manifests removed, oldest first.
    pub removed: Vec<u64>,
    /// Manifests older than the kept ones that stayed because they are
    /// pinned and pins are preserved, oldest first.
    pub pinned: Vec<u64>,
    /// Chunk files removed because no remaining manifest names them.
    pub chunks_removed: usize,
    /// Chunk files kept.
    pub chunks_kept: usize,
}

enum Stored {
    Intact,
    Absent,
    Damaged,
}

/// Exclusive writer lock on `store/lock`. Explicitly unlocks on drop so a
/// concurrent `fork`/`posix_spawn` in another thread (e.g. host probes or
/// helper subprocesses) that transiently duplicated the open file description
/// cannot leave `flock` held past `ChunkStore` drop.
#[derive(Debug)]
struct StoreLock(File);

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// An open checkpoint store, held by its one writer.
#[derive(Debug)]
pub struct ChunkStore {
    root: PathBuf,
    durability: Durability,
    next: u64,
    swept: usize,
    preserve_pinned: bool,
    _lock: StoreLock,
}

fn storage(operation: &'static str, path: &Path, error: &io::Error) -> CoreError {
    CoreError::StorageFailed {
        operation,
        path: path.display().to_string(),
        reason: error.to_string(),
    }
}

fn refuse(reason: impl Into<String>) -> CoreError {
    CoreError::IncompatibleCheckpoint {
        reason: reason.into(),
    }
}

fn flow(flow: Flow) -> Result<(), CoreError> {
    match flow {
        Flow::Continue => Ok(()),
        Flow::Cancel => Err(CoreError::Cancelled {
            reason: "checkpoint publication cancelled before its manifest was published".into(),
        }),
    }
}

fn create_directory(path: &Path) -> Result<(), CoreError> {
    fs::create_dir_all(path).map_err(|error| storage("create directory", path, &error))
}

fn flush_directory(path: &Path) -> Result<(), CoreError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| storage("flush directory", path, &error))
}

/// Create `path` with `bytes`, recording it in `temporaries` as soon as it
/// exists, so a write that fails part way (a full disk) is removed too.
fn create_new(
    path: &Path,
    bytes: &[u8],
    temporaries: &mut Vec<PathBuf>,
) -> Result<File, CoreError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| storage("create temporary file", path, &error))?;
    temporaries.push(path.to_path_buf());
    file.write_all(bytes)
        .map_err(|error| storage("write temporary file", path, &error))?;
    Ok(file)
}

/// Where a store at `root` keeps the chunk with `digest`.
fn chunk_path_in(root: &Path, digest: &Sha256Digest) -> PathBuf {
    let hex = digest.to_hex();
    root.join(CHUNKS).join(&hex[..2]).join(hex)
}

/// The bytes a streamed read holds at once (shared ceiling across concurrent
/// chunk verifiers so `CHECKPOINT_FIXED_BYTES` is never exceeded).
const READ_BUFFER_BYTES: usize = 1 << 20;

/// Half of [`READ_BUFFER_BYTES`]: two buffers for a single pipelined reader
/// (`read` overlapped with `SHA-256`) stay strictly within [`READ_BUFFER_BYTES`].
const PIPELINE_HALF_BYTES: usize = READ_BUFFER_BYTES / 2;

/// Quarter of [`READ_BUFFER_BYTES`]: four buffers (either four concurrent
/// chunk verifiers or two double-buffered pipelined lanes) stay strictly
/// within [`READ_BUFFER_BYTES`].
const LANE_BUFFER_BYTES: usize = READ_BUFFER_BYTES / 4;

/// Chunks at or below this byte length hash inline without worker-pool
/// dispatch overhead.
const PARALLEL_HASH_MIN_BYTES: u64 = 64 << 10;

/// Total held payload below which [`ChunkStore::assemble`] reads chunks
/// serially without worker-pool dispatch overhead.
const PARALLEL_LOAD_MIN_BYTES: u64 = 256 << 10;

/// Fixed upper bound on worker threads for store I/O and hashing. Using one
/// consistent worker count across store operations keeps `flife_core::pool`
/// to at most one small cached private pool.
const MAX_STORE_WORKERS: usize = 4;

fn store_workers() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1)
        .clamp(1, MAX_STORE_WORKERS)
}

fn alloc_read_buffer(bytes: usize) -> Result<Vec<u8>, CoreError> {
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(bytes)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: "chunk read buffer",
            bytes: u64::try_from(bytes).unwrap_or(u64::MAX),
        })?;
    buffer.resize(bytes, 0);
    Ok(buffer)
}

/// Reusable double buffer whose combined length never exceeds
/// [`READ_BUFFER_BYTES`].
struct ReadBuffers {
    cur: Vec<u8>,
    next: Vec<u8>,
}

impl ReadBuffers {
    const fn new() -> Self {
        Self {
            cur: Vec::new(),
            next: Vec::new(),
        }
    }

    fn ensure_len(buf: &mut Vec<u8>, want: usize) -> Result<(), CoreError> {
        if buf.len() < want {
            let extra = want - buf.len();
            buf.try_reserve_exact(extra)
                .map_err(|_| CoreError::AllocationFailed {
                    buffer: "chunk read buffer",
                    bytes: u64::try_from(want).unwrap_or(u64::MAX),
                })?;
            buf.resize(want, 0);
        }
        Ok(())
    }

    /// Pre-size the reusable pair for a sequence of chunks whose largest chunk
    /// is `max_len` bytes, staying strictly within [`READ_BUFFER_BYTES`].
    fn prepare_for(&mut self, max_len: u64, workers: usize) -> Result<(), CoreError> {
        if max_len == 0 {
            return Ok(());
        }
        let cur_cap =
            usize::try_from(max_len.min(PIPELINE_HALF_BYTES as u64)).unwrap_or(PIPELINE_HALF_BYTES);
        Self::ensure_len(&mut self.cur, cur_cap)?;
        if workers >= 2 && max_len > PIPELINE_HALF_BYTES as u64 {
            Self::ensure_len(&mut self.next, PIPELINE_HALF_BYTES)?;
        }
        Ok(())
    }
}

/// Fill `buf` from `file`, looping past short reads and `EINTR` until either
/// `buf` is full or EOF is reached. Returns `< buf.len()` on early EOF.
fn read_full(file: &mut File, path: &Path, buf: &mut [u8]) -> Result<usize, CoreError> {
    let mut filled = 0;
    while filled < buf.len() {
        let Some(dst) = buf.get_mut(filled..) else {
            break;
        };
        match file.read(dst) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(storage("read chunk", path, &error)),
        }
    }
    Ok(filled)
}

/// Feed `length` bytes of the file at `path` to `chunk` and optional
/// `container`, reusing `buffers` (whose combined length never exceeds
/// [`READ_BUFFER_BYTES`]).
///
/// For chunks larger than [`PARALLEL_HASH_MIN_BYTES`] on multi-core hosts,
/// overlaps the next block read with SHA-256 compression (and hashes `chunk`
/// and `container` in parallel when both are present). Returns `Ok(false)`
/// when the file ends before `length` bytes.
fn hash_file_with_buffers(
    path: &Path,
    length: u64,
    chunk: &mut Sha256Stream,
    mut container: Option<&mut Sha256Stream>,
    buffers: &mut ReadBuffers,
) -> Result<bool, CoreError> {
    let mut file = File::open(path).map_err(|error| storage("read chunk", path, &error))?;
    if length == 0 {
        return Ok(true);
    }
    let workers = store_workers();
    if workers <= 1 || length <= PARALLEL_HASH_MIN_BYTES {
        let cap =
            usize::try_from(length.min(PIPELINE_HALF_BYTES as u64)).unwrap_or(PIPELINE_HALF_BYTES);
        ReadBuffers::ensure_len(&mut buffers.cur, cap)?;
        let mut remaining = length;
        while remaining > 0 {
            let want = usize::try_from(remaining.min(PIPELINE_HALF_BYTES as u64))
                .unwrap_or(PIPELINE_HALF_BYTES);
            let Some(slice) = buffers.cur.get_mut(..want) else {
                return Ok(false);
            };
            let read = read_full(&mut file, path, slice)?;
            if read < want {
                return Ok(false);
            }
            chunk.update(slice);
            if let Some(container_stream) = container.as_deref_mut() {
                container_stream.update(slice);
            }
            remaining -= u64::try_from(want).unwrap_or(remaining);
        }
        return Ok(true);
    }

    buffers.prepare_for(length, workers)?;
    let mut remaining = length;
    let first_want =
        usize::try_from(remaining.min(PIPELINE_HALF_BYTES as u64)).unwrap_or(PIPELINE_HALF_BYTES);
    let Some(first_dst) = buffers.cur.get_mut(..first_want) else {
        return Ok(false);
    };
    let first_read = read_full(&mut file, path, first_dst)?;
    if first_read < first_want {
        return Ok(false);
    }
    remaining -= u64::try_from(first_want).unwrap_or(remaining);
    let mut cur_len = first_want;

    loop {
        let next_want = usize::try_from(remaining.min(PIPELINE_HALF_BYTES as u64))
            .unwrap_or(PIPELINE_HALF_BYTES);
        if next_want > 0 {
            let mut next_outcome: Result<usize, CoreError> = Ok(0);
            {
                let ready = buffers.cur.get(..cur_len).unwrap_or_default();
                let next_dst = buffers.next.get_mut(..next_want).unwrap_or_default();
                let next_outcome_ref = &mut next_outcome;
                let container_opt = container.as_deref_mut();
                let file_ref = &mut file;
                flife_core::pool::scope_workers(workers, "verify checkpoint", |scope| {
                    if let Some(container_stream) = container_opt {
                        scope.spawn(move || container_stream.update(ready));
                    }
                    scope.spawn(move || {
                        *next_outcome_ref = read_full(file_ref, path, next_dst);
                    });
                    chunk.update(ready);
                })?;
            }
            let read = next_outcome?;
            if read < next_want {
                return Ok(false);
            }
            remaining -= u64::try_from(next_want).unwrap_or(remaining);
            std::mem::swap(&mut buffers.cur, &mut buffers.next);
            cur_len = next_want;
        } else {
            let ready = buffers.cur.get(..cur_len).unwrap_or_default();
            if let Some(container_stream) = container.as_deref_mut() {
                flife_core::pool::scope_workers(workers, "verify checkpoint", |scope| {
                    scope.spawn(move || container_stream.update(ready));
                    chunk.update(ready);
                })?;
            } else {
                chunk.update(ready);
            }
            break;
        }
    }
    Ok(true)
}

/// Verify a single chunk file matches `length` bytes and `digest`, reading
/// into caller-supplied `buf`. Returns `false` on any I/O error, early EOF,
/// or digest mismatch.
fn verify_single_chunk_with_buf(
    path: &Path,
    length: u64,
    digest: &Sha256Digest,
    buf: &mut [u8],
) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    if length == 0 {
        return Sha256Digest::of(&[]) == *digest;
    }
    if buf.is_empty() {
        return false;
    }
    let mut hasher = Sha256Stream::new();
    let mut remaining = length;
    let cap = buf.len();
    while remaining > 0 {
        let want = usize::try_from(remaining.min(cap as u64)).unwrap_or(cap);
        let Some(slice) = buf.get_mut(..want) else {
            return false;
        };
        match read_full(&mut file, path, slice) {
            Ok(read) if read == want => {
                hasher.update(slice);
                remaining -= u64::try_from(want).unwrap_or(remaining);
            }
            _ => return false,
        }
    }
    hasher.finish() == *digest
}

struct ActiveChunk {
    work_index: usize,
    path: PathBuf,
    file: File,
    digest: Sha256Digest,
    remaining: u64,
    cur_len: usize,
    hasher: Sha256Stream,
}

struct PipelinedSlot {
    cur: Vec<u8>,
    next: Vec<u8>,
    active: Option<ActiveChunk>,
}

struct LaneRead<'a> {
    file: &'a mut File,
    path: &'a Path,
    dst: &'a mut [u8],
    out: &'a mut Result<usize, CoreError>,
}

struct LaneTask<'a> {
    hasher: &'a mut Sha256Stream,
    ready: &'a [u8],
    read: Option<LaneRead<'a>>,
}

impl PipelinedSlot {
    fn new(block_bytes: usize) -> Result<Self, CoreError> {
        Ok(Self {
            cur: alloc_read_buffer(block_bytes)?,
            next: alloc_read_buffer(block_bytes)?,
            active: None,
        })
    }

    fn next_want(&self) -> usize {
        self.active.as_ref().map_or(0, |active| {
            let cap = self.next.len();
            usize::try_from(active.remaining.min(cap as u64)).unwrap_or(cap)
        })
    }

    fn prepare_task<'a>(
        &'a mut self,
        want: usize,
        out: &'a mut Result<usize, CoreError>,
    ) -> Option<LaneTask<'a>> {
        let active = self.active.as_mut()?;
        let ready = self.cur.get(..active.cur_len).unwrap_or_default();
        let read = if want > 0 {
            Some(LaneRead {
                file: &mut active.file,
                path: active.path.as_path(),
                dst: self.next.get_mut(..want).unwrap_or_default(),
                out,
            })
        } else {
            None
        };
        Some(LaneTask {
            hasher: &mut active.hasher,
            ready,
            read,
        })
    }
}

struct PipelinedBatch<'a> {
    root: &'a Path,
    pending: &'a [(Sha256Digest, u64)],
    next_job: usize,
    results: Vec<bool>,
}

impl<'a> PipelinedBatch<'a> {
    fn new(root: &'a Path, pending: &'a [(Sha256Digest, u64)]) -> Self {
        Self {
            root,
            pending,
            next_job: 0,
            results: vec![false; pending.len()],
        }
    }

    fn set_result(&mut self, index: usize, ok: bool) {
        if let Some(slot) = self.results.get_mut(index) {
            *slot = ok;
        }
    }

    /// Assign the next multi-block chunk to `slot`, resolving any zero-byte or
    /// single-block (`<= slot.cur.len()`) chunks inline without a scope barrier.
    fn refill(&mut self, slot: &mut PipelinedSlot) {
        let block_cap = slot.cur.len();
        while slot.active.is_none() {
            let Some(&(digest, length)) = self.pending.get(self.next_job) else {
                return;
            };
            let work_index = self.next_job;
            self.next_job += 1;
            let path = chunk_path_in(self.root, &digest);
            let Ok(mut file) = File::open(&path) else {
                self.set_result(work_index, false);
                continue;
            };
            if length == 0 {
                self.set_result(work_index, Sha256Digest::of(&[]) == digest);
                continue;
            }
            let first_want = usize::try_from(length.min(block_cap as u64)).unwrap_or(block_cap);
            let Some(first_dst) = slot.cur.get_mut(..first_want) else {
                self.set_result(work_index, false);
                continue;
            };
            match read_full(&mut file, &path, first_dst) {
                Ok(read) if read == first_want => {}
                _ => {
                    self.set_result(work_index, false);
                    continue;
                }
            }
            let remaining = length.saturating_sub(u64::try_from(first_want).unwrap_or(length));
            if remaining == 0 {
                let intact = Sha256Digest::of(first_dst) == digest;
                self.set_result(work_index, intact);
                continue;
            }
            slot.active = Some(ActiveChunk {
                work_index,
                path,
                file,
                digest,
                remaining,
                cur_len: first_want,
                hasher: Sha256Stream::new(),
            });
        }
    }

    fn advance(
        &mut self,
        slot: &mut PipelinedSlot,
        want: usize,
        read_res: Result<usize, CoreError>,
    ) {
        if slot.active.is_none() {
            return;
        }
        if want == 0 {
            if let Some(done) = slot.active.take() {
                let intact = done.hasher.finish() == done.digest;
                self.set_result(done.work_index, intact);
            }
            return;
        }
        match read_res {
            Ok(read) if read == want => {
                if let Some(active) = slot.active.as_mut() {
                    active.remaining -= u64::try_from(want).unwrap_or(active.remaining);
                    active.cur_len = want;
                    std::mem::swap(&mut slot.cur, &mut slot.next);
                }
            }
            _ => {
                if let Some(failed) = slot.active.take() {
                    self.set_result(failed.work_index, false);
                }
            }
        }
    }
}

enum LoadTask<'a> {
    Streamed {
        path: PathBuf,
        length: u64,
        output: Option<io::Take<File>>,
    },
    Held {
        path: PathBuf,
        family: ChunkFamily,
        digest: Sha256Digest,
        buf: &'a mut [u8],
    },
}

fn remove_all(temporaries: &[PathBuf]) {
    for path in temporaries {
        // Best effort: anything left is removed when the store is next
        // opened, and is never read.
        let _ignored = fs::remove_file(path);
    }
}

/// `name` as a manifest's sequence number: sixteen lowercase hex digits and
/// the suffix.
fn sequence_of(name: &str) -> Option<u64> {
    let digits = name.strip_suffix(MANIFEST_SUFFIX)?;
    (digits.len() == 16
        && digits
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    .then(|| u64::from_str_radix(digits, 16).ok())
    .flatten()
}

/// A pin's file name, `<sequence>.<holder>.pin`: the sequence and holder.
fn pin_of(name: &str) -> Option<(u64, Sha256Digest)> {
    let stem = name.strip_suffix(PIN_SUFFIX)?;
    let (sequence, holder) = stem.split_once('.')?;
    let sequence = sequence_of(&format!("{sequence}{MANIFEST_SUFFIX}"))?;
    if !is_digest_name(holder) {
        return None;
    }
    let mut bytes = [0_u8; 32];
    for (byte, pair) in bytes.iter_mut().zip(holder.as_bytes().as_chunks::<2>().0) {
        let pair = std::str::from_utf8(pair.as_slice()).ok()?;
        *byte = u8::from_str_radix(pair, 16).ok()?;
    }
    Some((sequence, Sha256Digest::from_bytes(bytes)))
}

fn is_digest_name(name: &str) -> bool {
    name.len() == 64
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl ChunkStore {
    /// Open the store at `root`, creating it if `root` is absent or empty.
    /// Takes the writer's lock and removes the temporary files a crashed or
    /// failed writer left.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] if another writer holds the store or
    /// `root` is a non-empty directory that is not a store;
    /// [`CoreError::SchemaMismatch`] for another store format;
    /// [`CoreError::StorageFailed`] if the host refuses an operation.
    pub fn open(root: impl AsRef<Path>, durability: Durability) -> Result<Self, CoreError> {
        let root = root.as_ref().to_path_buf();
        create_directory(&root)?;
        let lock_path = root.join(LOCK);
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(|error| storage("open store lock", &lock_path, &error))?;
        let lock = match lock.try_lock() {
            Ok(()) => StoreLock(lock),
            Err(fs::TryLockError::WouldBlock) => {
                return Err(CoreError::StateConflict {
                    operation: "open checkpoint store",
                    detail: format!("another writer holds {}", root.display()),
                });
            }
            Err(fs::TryLockError::Error(error)) => {
                return Err(storage("lock store", &lock_path, &error));
            }
        };

        let marker = root.join(MARKER);
        let expected = format!("{STORE_FORMAT}\n");
        match fs::read(&marker) {
            Ok(found) => {
                if found != expected.as_bytes() {
                    return Err(CoreError::SchemaMismatch {
                        expected: STORE_FORMAT.into(),
                        found: String::from_utf8_lossy(&found).trim().to_owned(),
                    });
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let mut foreign = false;
                for entry in
                    fs::read_dir(&root).map_err(|error| storage("list store", &root, &error))?
                {
                    let entry = entry.map_err(|error| storage("list store", &root, &error))?;
                    if entry.file_name() != LOCK {
                        foreign = true;
                        break;
                    }
                }
                if foreign {
                    return Err(CoreError::StateConflict {
                        operation: "create checkpoint store",
                        detail: format!("{} is not empty and is not a store", root.display()),
                    });
                }
                let partial = root.join(format!("{MARKER}.partial"));
                let mut written = Vec::new();
                let created =
                    create_new(&partial, expected.as_bytes(), &mut written).and_then(|file| {
                        if durability.flushes() {
                            file.sync_all()
                                .map_err(|error| storage("flush store marker", &partial, &error))?;
                        }
                        drop(file);
                        fs::rename(&partial, &marker)
                            .map_err(|error| storage("publish store marker", &marker, &error))
                    });
                if let Err(error) = created {
                    remove_all(&written);
                    return Err(error);
                }
            }
            Err(error) => return Err(storage("read store marker", &marker, &error)),
        }
        for directory in [CHUNKS, CHECKPOINTS, PINS, TEMPORARY] {
            create_directory(&root.join(directory))?;
        }
        if durability.flushes() {
            flush_directory(&root)?;
        }

        let temporary = root.join(TEMPORARY);
        let mut swept = 0;
        for entry in fs::read_dir(&temporary)
            .map_err(|error| storage("list temporaries", &temporary, &error))?
        {
            let entry = entry.map_err(|error| storage("list temporaries", &temporary, &error))?;
            let path = entry.path();
            if path.is_file() {
                fs::remove_file(&path)
                    .map_err(|error| storage("remove temporary file", &path, &error))?;
                swept += 1;
            }
        }

        let mut store = Self {
            root,
            durability,
            next: 1,
            swept,
            preserve_pinned: true,
            _lock: lock,
        };
        store.next = store.sequences()?.last().map_or(Ok(1), |last| {
            last.checked_add(1).ok_or(CoreError::Overflow {
                quantity: "checkpoint sequence",
            })
        })?;
        Ok(store)
    }

    /// The store's directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The store's durability profile.
    #[must_use]
    pub const fn durability(&self) -> Durability {
        self.durability
    }

    /// Temporary files removed when the store was opened.
    #[must_use]
    pub const fn swept(&self) -> usize {
        self.swept
    }

    /// The sequence number the next publication takes.
    #[must_use]
    pub const fn next_sequence(&self) -> u64 {
        self.next
    }

    /// Every manifest's sequence number, ascending, valid or not.
    ///
    /// # Errors
    /// [`CoreError::StorageFailed`] if `checkpoints/` cannot be listed.
    pub fn sequences(&self) -> Result<Vec<u64>, CoreError> {
        let directory = self.root.join(CHECKPOINTS);
        let mut sequences = Vec::new();
        for entry in fs::read_dir(&directory)
            .map_err(|error| storage("list checkpoints", &directory, &error))?
        {
            let entry = entry.map_err(|error| storage("list checkpoints", &directory, &error))?;
            if let Some(sequence) = entry.file_name().to_str().and_then(sequence_of) {
                sequences.push(sequence);
            }
        }
        sequences.sort_unstable();
        Ok(sequences)
    }

    /// Where checkpoint `sequence`'s manifest is.
    #[must_use]
    pub fn manifest_path(&self, sequence: u64) -> PathBuf {
        self.root
            .join(CHECKPOINTS)
            .join(format!("{sequence:016x}{MANIFEST_SUFFIX}"))
    }

    /// Where the chunk with `digest` is stored.
    #[must_use]
    pub fn chunk_path(&self, digest: &Sha256Digest) -> PathBuf {
        chunk_path_in(&self.root, digest)
    }

    /// Where `holder`'s pin of checkpoint `sequence` is.
    #[must_use]
    pub fn pin_path(&self, sequence: u64, holder: &Sha256Digest) -> PathBuf {
        self.root
            .join(PINS)
            .join(format!("{sequence:016x}.{}{PIN_SUFFIX}", holder.to_hex()))
    }

    /// Pin checkpoint `sequence` for `holder`: while pins are preserved,
    /// retention keeps it, and every chunk it names, however old it is.
    /// Returns whether the pin is new (`false` if `holder` already held
    /// it).
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] if the store holds no manifest
    /// `sequence`; [`CoreError::StorageFailed`] if the host refuses an
    /// operation.
    pub fn pin(&mut self, sequence: u64, holder: &Sha256Digest) -> Result<bool, CoreError> {
        let manifest = self.manifest_path(sequence);
        if !fs::symlink_metadata(&manifest).is_ok_and(|metadata| metadata.is_file()) {
            return Err(CoreError::StateConflict {
                operation: "pin checkpoint",
                detail: format!("{} holds no checkpoint {sequence}", self.root.display()),
            });
        }
        let path = self.pin_path(sequence, holder);
        let created = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                if self.durability.flushes() {
                    file.sync_all()
                        .map_err(|error| storage("flush pin", &path, &error))?;
                }
                true
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
            Err(error) => return Err(storage("create pin", &path, &error)),
        };
        if created && self.durability.flushes() {
            flush_directory(&self.root.join(PINS))?;
        }
        Ok(created)
    }

    /// Remove `holder`'s pin of checkpoint `sequence`. Returns whether it
    /// held one. The checkpoint stays pinned while another holder pins it.
    ///
    /// # Errors
    /// [`CoreError::StorageFailed`] if the host refuses an operation.
    pub fn unpin(&mut self, sequence: u64, holder: &Sha256Digest) -> Result<bool, CoreError> {
        let path = self.pin_path(sequence, holder);
        match fs::remove_file(&path) {
            Ok(()) => {
                if self.durability.flushes() {
                    flush_directory(&self.root.join(PINS))?;
                }
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(storage("remove pin", &path, &error)),
        }
    }

    /// Every pin: checkpoint sequence and holder, ascending. A pin may name
    /// a checkpoint retention removed while pins were not preserved.
    ///
    /// # Errors
    /// [`CoreError::StorageFailed`] if `pins/` cannot be listed.
    pub fn pins(&self) -> Result<Vec<(u64, Sha256Digest)>, CoreError> {
        let directory = self.root.join(PINS);
        let mut pins = Vec::new();
        for entry in
            fs::read_dir(&directory).map_err(|error| storage("list pins", &directory, &error))?
        {
            let entry = entry.map_err(|error| storage("list pins", &directory, &error))?;
            if let Some(pin) = entry.file_name().to_str().and_then(pin_of) {
                pins.push(pin);
            }
        }
        pins.sort_unstable_by_key(|(sequence, holder)| (*sequence, *holder.as_bytes()));
        Ok(pins)
    }

    /// Whether retention keeps pinned checkpoints (the default) or treats
    /// them as any other: the configuration's
    /// `persistence.preserve_pinned_discoveries`.
    pub const fn set_preserve_pinned(&mut self, preserve: bool) {
        self.preserve_pinned = preserve;
    }

    /// Whether retention keeps pinned checkpoints.
    #[must_use]
    pub const fn preserve_pinned(&self) -> bool {
        self.preserve_pinned
    }

    /// Start publishing a checkpoint as it is encoded ([`Stream`]): pass
    /// the stream to [`flife_core::checkpoint::CheckpointWriter::streaming`]
    /// (or `flife_runtime::checkpoint_into`), then the sealed prefix it
    /// returns to [`ChunkStore::commit`]. The stream takes the next sequence
    /// number now; a stream never committed leaves a gap.
    ///
    /// # Errors
    /// [`CoreError::Overflow`] if sequence numbers are exhausted.
    pub fn stream(&mut self) -> Result<Stream, CoreError> {
        self.stream_observed(Box::new(|_| Flow::Continue))
    }

    /// [`ChunkStore::stream`], linking each chunk this store lacks (or holds
    /// damaged) from `donor` when the donor holds it intact and the host
    /// can link it, as [`ChunkStore::publish_sharing`] does: a branch's
    /// founding checkpoint shares its parent's chunks without being held
    /// whole. A chunk's digest is known only at its end, so its pages are
    /// still written to a temporary file as they are encoded; the link then
    /// replaces it, and the publication counts the chunk linked, not its
    /// bytes written.
    ///
    /// # Errors
    /// As [`ChunkStore::stream`].
    pub fn stream_sharing(&mut self, donor: &Self) -> Result<Stream, CoreError> {
        self.stream_sharing_observed(donor, Box::new(|_| Flow::Continue))
    }

    /// [`ChunkStore::stream_sharing`], reporting each [`Step`] to `observe`
    /// as [`ChunkStore::stream_observed`] does.
    ///
    /// # Errors
    /// As [`ChunkStore::stream`].
    pub fn stream_sharing_observed(
        &mut self,
        donor: &Self,
        observe: Observer,
    ) -> Result<Stream, CoreError> {
        let mut stream = self.stream_observed(observe)?;
        stream.share_from(donor.root.clone());
        Ok(stream)
    }

    /// [`ChunkStore::stream`], reporting each [`Step`] to `observe`, which
    /// may cancel the publication until the manifest is published, as
    /// [`ChunkStore::publish_observed`] does.
    ///
    /// # Errors
    /// As [`ChunkStore::stream`].
    pub fn stream_observed(&mut self, observe: Observer) -> Result<Stream, CoreError> {
        let sequence = self.next;
        self.next = sequence.checked_add(1).ok_or(CoreError::Overflow {
            quantity: "checkpoint sequence",
        })?;
        Ok(Stream::new(
            self.root.clone(),
            self.durability,
            sequence,
            observe,
        ))
    }

    /// Publish the checkpoint `stream` stored, whose header, manifest and
    /// seal are `sealed` (see [`Stream`]). Before
    /// the manifest is written, checks that `sealed` verifies and names
    /// exactly the chunks the stream stored, in order and tiling the
    /// payload, and that each is still in place at its length.
    ///
    /// # Errors
    /// The stream's own refusal, if it had one; [`CoreError::StateConflict`]
    /// for a stream of another store or one with a chunk still open;
    /// [`CoreError::IncompatibleCheckpoint`] if `sealed` does not verify or
    /// does not match the stream; otherwise as
    /// [`ChunkStore::publish_observed`]. Unless the error is a failure to
    /// flush `checkpoints/` after [`Step::Published`], the store's
    /// published checkpoints are as they were.
    pub fn commit(&mut self, mut stream: Stream, sealed: &[u8]) -> Result<Publication, CoreError> {
        if let Some(error) = &stream.failed {
            return Err(error.clone());
        }
        if stream.root != self.root {
            return Err(CoreError::StateConflict {
                operation: "commit checkpoint",
                detail: format!(
                    "the stream writes {}, not this store",
                    stream.root.display()
                ),
            });
        }
        if stream.is_open() {
            return Err(CoreError::StateConflict {
                operation: "commit checkpoint",
                detail: "a chunk is still open".into(),
            });
        }
        let manifest = CheckpointManifest::parse(sealed)?;
        if manifest.sealed_length() != sealed.len() {
            return Err(refuse(format!(
                "the sealed prefix is followed by {} bytes",
                sealed.len().saturating_sub(manifest.sealed_length())
            )));
        }
        if manifest.entries().len() != stream.stored.len() {
            return Err(refuse(format!(
                "the manifest names {} chunks, the stream stored {}",
                manifest.entries().len(),
                stream.stored.len()
            )));
        }
        let mut offset = 0_u64;
        for (entry, (family, length, digest)) in manifest.entries().iter().zip(&stream.stored) {
            if entry.family != *family
                || entry.length != *length
                || entry.digest != *digest
                || entry.offset != offset
            {
                return Err(refuse(format!(
                    "the manifest's chunk {} is not the one the stream stored",
                    entry.family.name()
                )));
            }
            offset = offset.checked_add(entry.length).ok_or_else(|| {
                refuse(format!(
                    "chunk {} overflows payload offset",
                    entry.family.name()
                ))
            })?;
            let path = self.chunk_path(digest);
            let present = fs::symlink_metadata(&path)
                .is_ok_and(|metadata| metadata.is_file() && metadata.len() == *length);
            if !present {
                return Err(refuse(format!(
                    "chunk {} ({digest}) is no longer stored",
                    family.name()
                )));
            }
        }
        let sequence = stream.sequence;
        let mut temporaries = std::mem::take(&mut stream.temporaries);
        let outcome =
            self.publish_manifest(sealed, sequence, &mut *stream.observe, &mut temporaries);
        remove_all(&temporaries);
        outcome.map(|()| stream.publication.clone())
    }

    /// Publish `checkpoint` (see the [module documentation](self)).
    ///
    /// # Errors
    /// As [`ChunkStore::publish_observed`].
    pub fn publish(&mut self, checkpoint: &[u8]) -> Result<Publication, CoreError> {
        self.publish_observed(checkpoint, &mut |_| Flow::Continue)
    }

    /// Publish `checkpoint`, reporting each [`Step`] to `observe`, which may
    /// cancel the publication until the manifest is published.
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] if `checkpoint` does not verify;
    /// [`CoreError::Cancelled`] if `observe` cancelled;
    /// [`CoreError::StorageFailed`] if the host refused an operation;
    /// [`CoreError::Overflow`] if sequence numbers are exhausted. Unless the
    /// error is a failure to flush `checkpoints/` after
    /// [`Step::Published`], the store's published checkpoints are as they
    /// were.
    pub fn publish_observed(
        &mut self,
        checkpoint: &[u8],
        observe: &mut dyn FnMut(Step) -> Flow,
    ) -> Result<Publication, CoreError> {
        self.publish_from(checkpoint, None, observe)
    }

    /// Publish `checkpoint`, linking each chunk this store lacks (or holds
    /// damaged) from `donor` when the donor holds it intact and the host
    /// can link it, and writing it otherwise (see the
    /// [module documentation](self)).
    ///
    /// # Errors
    /// As [`ChunkStore::publish_observed`].
    pub fn publish_sharing(
        &mut self,
        checkpoint: &[u8],
        donor: &Self,
    ) -> Result<Publication, CoreError> {
        self.publish_sharing_observed(checkpoint, donor, &mut |_| Flow::Continue)
    }

    /// [`ChunkStore::publish_sharing`], reporting each [`Step`] to
    /// `observe` as [`ChunkStore::publish_observed`] does.
    ///
    /// # Errors
    /// As [`ChunkStore::publish_observed`].
    pub fn publish_sharing_observed(
        &mut self,
        checkpoint: &[u8],
        donor: &Self,
        observe: &mut dyn FnMut(Step) -> Flow,
    ) -> Result<Publication, CoreError> {
        self.publish_from(checkpoint, Some(donor), observe)
    }

    fn publish_from(
        &mut self,
        checkpoint: &[u8],
        donor: Option<&Self>,
        observe: &mut dyn FnMut(Step) -> Flow,
    ) -> Result<Publication, CoreError> {
        CheckpointReader::parse(checkpoint, &[])?;
        let manifest = CheckpointManifest::parse(checkpoint)?;
        let sequence = self.next;
        self.next = sequence.checked_add(1).ok_or(CoreError::Overflow {
            quantity: "checkpoint sequence",
        })?;
        let mut temporaries = Vec::new();
        let outcome = self.stage(
            checkpoint,
            &manifest,
            sequence,
            donor,
            observe,
            &mut temporaries,
        );
        remove_all(&temporaries);
        outcome
    }

    fn stage(
        &self,
        checkpoint: &[u8],
        manifest: &CheckpointManifest<'_>,
        sequence: u64,
        donor: Option<&Self>,
        observe: &mut dyn FnMut(Step) -> Flow,
        temporaries: &mut Vec<PathBuf>,
    ) -> Result<Publication, CoreError> {
        let flushes = self.durability.flushes();
        let payload = checkpoint
            .get(manifest.sealed_length()..)
            .unwrap_or_default();
        let mut publication = Publication {
            sequence,
            written: 0,
            linked: 0,
            shared: 0,
            repaired: 0,
            bytes_written: 0,
        };
        for (index, entry) in manifest.entries().iter().enumerate() {
            let bytes = usize::try_from(entry.offset)
                .ok()
                .zip(
                    entry
                        .offset
                        .checked_add(entry.length)
                        .and_then(|end| usize::try_from(end).ok()),
                )
                .and_then(|(start, end)| payload.get(start..end))
                .ok_or_else(|| {
                    refuse(format!(
                        "chunk {} is outside the payload",
                        entry.family.name()
                    ))
                })?;
            let path = self.chunk_path(&entry.digest);
            match stored(&path, entry)? {
                Stored::Intact => {
                    publication.shared += 1;
                    flow(observe(Step::ChunkShared { index }))?;
                    continue;
                }
                Stored::Absent => publication.written += 1,
                Stored::Damaged => publication.repaired += 1,
            }
            let directory = path.parent().unwrap_or(&self.root).to_path_buf();
            if !directory.is_dir() {
                create_directory(&directory)?;
                if flushes {
                    flush_directory(&self.root.join(CHUNKS))?;
                }
            }
            let temporary = self
                .root
                .join(TEMPORARY)
                .join(format!("{sequence:016x}-{index}.chunk"));
            // The donor's copy, verified, under a temporary name; or `None`
            // when there is no intact copy or the host will not link it.
            let linked = match donor {
                Some(donor) => {
                    let source = donor.chunk_path(&entry.digest);
                    if matches!(stored(&source, entry)?, Stored::Intact)
                        && fs::hard_link(&source, &temporary).is_ok()
                    {
                        temporaries.push(temporary.clone());
                        // Flushing the shared file covers a donor that
                        // published without flushing.
                        let file = File::open(&temporary)
                            .map_err(|error| storage("open linked chunk", &temporary, &error))?;
                        Some(file)
                    } else {
                        None
                    }
                }
                None => None,
            };
            let file = if let Some(file) = linked {
                publication.linked += 1;
                flow(observe(Step::ChunkLinked { index }))?;
                file
            } else {
                let file = create_new(&temporary, bytes, temporaries)?;
                publication.bytes_written = publication.bytes_written.saturating_add(entry.length);
                flow(observe(Step::ChunkWritten { index }))?;
                file
            };
            if flushes {
                file.sync_all()
                    .map_err(|error| storage("flush chunk", &temporary, &error))?;
                flow(observe(Step::ChunkFlushed { index }))?;
            }
            drop(file);
            fs::rename(&temporary, &path).map_err(|error| storage("store chunk", &path, &error))?;
            temporaries.retain(|candidate| candidate != &temporary);
            if flushes {
                flush_directory(&directory)?;
            }
            flow(observe(Step::ChunkStored { index }))?;
        }

        let sealed = checkpoint
            .get(..manifest.sealed_length())
            .ok_or_else(|| refuse("the sealed prefix is outside the checkpoint"))?;
        self.publish_manifest(sealed, sequence, observe, temporaries)?;
        Ok(publication)
    }

    /// Write `sealed` as checkpoint `sequence`'s manifest and link it into
    /// `checkpoints/`: the commit point.
    fn publish_manifest(
        &self,
        sealed: &[u8],
        sequence: u64,
        observe: &mut dyn FnMut(Step) -> Flow,
        temporaries: &mut Vec<PathBuf>,
    ) -> Result<(), CoreError> {
        let flushes = self.durability.flushes();
        let temporary = self
            .root
            .join(TEMPORARY)
            .join(format!("{sequence:016x}{MANIFEST_SUFFIX}"));
        let file = create_new(&temporary, sealed, temporaries)?;
        flow(observe(Step::ManifestWritten))?;
        if flushes {
            file.sync_all()
                .map_err(|error| storage("flush manifest", &temporary, &error))?;
            flow(observe(Step::ManifestFlushed))?;
        }
        drop(file);
        // A link, unlike a rename, never replaces an existing manifest.
        let target = self.manifest_path(sequence);
        fs::hard_link(&temporary, &target)
            .map_err(|error| storage("publish manifest", &target, &error))?;
        let _ignored = observe(Step::Published);
        if flushes {
            flush_directory(&self.root.join(CHECKPOINTS))?;
            let _ignored = observe(Step::DirectoryFlushed);
        }
        Ok(())
    }

    /// Checkpoint `sequence`, reassembled from its manifest and chunks.
    ///
    /// # Errors
    /// [`CoreError::IncompatibleCheckpoint`] for a missing, oversized or
    /// damaged manifest, or a missing, mis-sized or damaged chunk (each
    /// chunk's length is checked before anything is allocated for it);
    /// [`CoreError::StorageFailed`] if the host refuses a read.
    pub fn load(&self, sequence: u64) -> Result<Vec<u8>, CoreError> {
        self.assemble(sequence, None).map(|(bytes, _)| bytes)
    }

    /// Checkpoint `sequence` as [`ChunkStore::load`] reassembles it, but
    /// for `streamed`'s chunk, which stays in the store: the sealed prefix
    /// and every other chunk in manifest order, each verified, and that
    /// chunk's file opened to be read once, at its declared length (`None`
    /// if the manifest names no such chunk). Together they are what
    /// [`CheckpointReader::parse_streamed`]
    /// reads, which verifies the chunk as it is streamed, so a restore never
    /// holds it whole (FL-043).
    ///
    /// # Errors
    /// As [`ChunkStore::load`] for the held chunks; the streamed chunk's
    /// file is checked for its declared length, and opened.
    pub fn load_streaming(
        &self,
        sequence: u64,
        streamed: ChunkFamily,
    ) -> Result<(Vec<u8>, Option<io::Take<File>>), CoreError> {
        self.assemble(sequence, Some(streamed))
    }

    /// [`ChunkStore::load`] without `streamed`'s chunk, which is opened
    /// instead.
    fn assemble(
        &self,
        sequence: u64,
        streamed: Option<ChunkFamily>,
    ) -> Result<(Vec<u8>, Option<io::Take<File>>), CoreError> {
        let prefix = self.sealed_prefix(sequence)?;
        let manifest = CheckpointManifest::parse(&prefix)?;
        let left = manifest
            .entries()
            .iter()
            .filter(|entry| Some(entry.family) == streamed)
            .try_fold(0_u64, |sum, entry| sum.checked_add(entry.length))
            .ok_or_else(|| refuse(format!("checkpoint {sequence} is too large for this host")))?;
        let held_payload = manifest
            .payload_length()
            .checked_sub(left)
            .ok_or_else(|| refuse(format!("checkpoint {sequence} is too large for this host")))?;
        let total = u64::try_from(prefix.len())
            .ok()
            .and_then(|sealed| sealed.checked_add(held_payload))
            .and_then(|total| usize::try_from(total).ok())
            .ok_or_else(|| refuse(format!("checkpoint {sequence} is too large for this host")))?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(total)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "checkpoint",
                bytes: u64::try_from(total).unwrap_or(u64::MAX),
            })?;
        bytes.extend_from_slice(&prefix);

        let held_count = manifest
            .entries()
            .iter()
            .filter(|entry| Some(entry.family) != streamed)
            .count();
        let workers = store_workers();
        if workers >= 2 && held_count >= 2 && held_payload >= PARALLEL_LOAD_MIN_BYTES {
            bytes.resize(total, 0);
            let mut source = None;
            {
                let mut rest = bytes.get_mut(prefix.len()..).unwrap_or_default();
                let mut tasks = Vec::with_capacity(manifest.entries().len());
                for entry in manifest.entries() {
                    let path = self.chunk_path(&entry.digest);
                    if Some(entry.family) == streamed {
                        tasks.push(LoadTask::Streamed {
                            path,
                            length: entry.length,
                            output: None,
                        });
                        continue;
                    }
                    let len = usize::try_from(entry.length).map_err(|_| {
                        refuse(format!("checkpoint {sequence} is too large for this host"))
                    })?;
                    let (buf, tail) = rest.split_at_mut_checked(len).ok_or_else(|| {
                        refuse(format!("checkpoint {sequence} is too large for this host"))
                    })?;
                    rest = tail;
                    tasks.push(LoadTask::Held {
                        path,
                        family: entry.family,
                        digest: entry.digest,
                        buf,
                    });
                }
                flife_core::pool::try_parallel_for_each_mut(
                    workers,
                    "load checkpoint",
                    &mut tasks,
                    |_index, task| match task {
                        LoadTask::Streamed {
                            path,
                            length,
                            output,
                        } => {
                            let path = path.as_path();
                            let file = File::open(path)
                                .map_err(|error| storage("open chunk", path, &error))?
                                .take(*length);
                            *output = Some(file);
                            Ok(())
                        }
                        LoadTask::Held {
                            path,
                            family,
                            digest,
                            buf,
                        } => {
                            let path = path.as_path();
                            let mut file = File::open(path)
                                .map_err(|error| storage("read chunk", path, &error))?;
                            let read = read_full(&mut file, path, buf)?;
                            if read != buf.len() || Sha256Digest::of(buf) != *digest {
                                return Err(refuse(format!(
                                    "chunk {} ({}) hash mismatch",
                                    family.name(),
                                    digest
                                )));
                            }
                            Ok(())
                        }
                    },
                )?;
                for task in tasks {
                    if let LoadTask::Streamed {
                        output: Some(file), ..
                    } = task
                    {
                        source = Some(file);
                    }
                }
            }
            return Ok((bytes, source));
        }

        let mut source = None;
        for entry in manifest.entries() {
            let chunk = self.chunk_path(&entry.digest);
            if Some(entry.family) == streamed {
                source = Some(
                    File::open(&chunk)
                        .map_err(|error| storage("open chunk", &chunk, &error))?
                        .take(entry.length),
                );
                continue;
            }
            let start = bytes.len();
            File::open(&chunk)
                .and_then(|file| file.take(entry.length).read_to_end(&mut bytes))
                .map_err(|error| storage("read chunk", &chunk, &error))?;
            let read = bytes.get(start..).unwrap_or_default();
            if u64::try_from(read.len()).ok() != Some(entry.length)
                || Sha256Digest::of(read) != entry.digest
            {
                return Err(refuse(format!(
                    "chunk {} ({}) hash mismatch",
                    entry.family.name(),
                    entry.digest
                )));
            }
        }
        Ok((bytes, source))
    }

    /// Verify checkpoint `sequence` as [`ChunkStore::load`] does, without
    /// holding it: each chunk is hashed as it is read, a buffer at a time.
    /// Returns the SHA-256 of the whole container, the digest of the bytes
    /// [`ChunkStore::load`] would return.
    ///
    /// # Errors
    /// As [`ChunkStore::load`].
    pub fn verify(&self, sequence: u64) -> Result<Sha256Digest, CoreError> {
        let prefix = self.sealed_prefix(sequence)?;
        let manifest = CheckpointManifest::parse(&prefix)?;
        let max_len = manifest
            .entries()
            .iter()
            .map(|entry| entry.length)
            .max()
            .unwrap_or(0);
        let mut buffers = ReadBuffers::new();
        buffers.prepare_for(max_len, store_workers())?;
        let mut container = Sha256Stream::new();
        container.update(&prefix);
        for entry in manifest.entries() {
            let path = self.chunk_path(&entry.digest);
            let mut chunk = Sha256Stream::new();
            let whole = hash_file_with_buffers(
                &path,
                entry.length,
                &mut chunk,
                Some(&mut container),
                &mut buffers,
            )?;
            if !whole || chunk.finish() != entry.digest {
                return Err(refuse(format!(
                    "chunk {} ({}) hash mismatch",
                    entry.family.name(),
                    entry.digest
                )));
            }
        }
        Ok(container.finish())
    }

    /// Checkpoint `sequence`'s sealed prefix, verified, after checking that
    /// every chunk it names is a file of its declared length.
    fn sealed_prefix(&self, sequence: u64) -> Result<Vec<u8>, CoreError> {
        let path = self.manifest_path(sequence);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(refuse(format!("checkpoint {sequence} has no manifest")));
            }
            Err(error) => return Err(storage("inspect manifest", &path, &error)),
        };
        if !metadata.is_file() || metadata.len() > MANIFEST_FILE_MAX_BYTES {
            return Err(refuse(format!(
                "checkpoint {sequence}'s manifest is not a file of at most {MANIFEST_FILE_MAX_BYTES} bytes"
            )));
        }
        let prefix = fs::read(&path).map_err(|error| storage("read manifest", &path, &error))?;
        let manifest = CheckpointManifest::parse(&prefix)?;
        if manifest.sealed_length() != prefix.len() {
            return Err(refuse(format!(
                "checkpoint {sequence}'s manifest holds {} bytes after its seal",
                prefix.len().saturating_sub(manifest.sealed_length())
            )));
        }
        for entry in manifest.entries() {
            let chunk = self.chunk_path(&entry.digest);
            match fs::symlink_metadata(&chunk) {
                Ok(metadata) if metadata.is_file() && metadata.len() == entry.length => {}
                Ok(metadata) => {
                    return Err(refuse(format!(
                        "chunk {} ({}) is {} bytes, the manifest declares {}",
                        entry.family.name(),
                        entry.digest,
                        metadata.len(),
                        entry.length
                    )));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(refuse(format!(
                        "chunk {} ({}) is missing",
                        entry.family.name(),
                        entry.digest
                    )));
                }
                Err(error) => return Err(storage("inspect chunk", &chunk, &error)),
            }
        }
        Ok(prefix)
    }

    /// The newest checkpoint that loads and that `accept` accepts, with the
    /// reason each newer one was passed over.
    ///
    /// # Errors
    /// [`CoreError::StorageFailed`] if `checkpoints/` cannot be listed. A
    /// checkpoint that fails to load or is refused is skipped, not an error.
    pub fn latest_valid<T>(
        &self,
        mut accept: impl FnMut(u64, &[u8]) -> Result<T, CoreError>,
    ) -> Result<Recovered<T>, CoreError> {
        self.newest(|sequence| {
            self.load(sequence)
                .and_then(|bytes| accept(sequence, &bytes))
        })
    }

    /// [`ChunkStore::latest_valid`] over [`ChunkStore::load_streaming`]:
    /// `accept` is given each checkpoint without `streamed`'s chunk, and
    /// that chunk's opened file.
    ///
    /// # Errors
    /// As [`ChunkStore::latest_valid`].
    pub fn latest_valid_streaming<T>(
        &self,
        streamed: ChunkFamily,
        mut accept: impl FnMut(u64, &[u8], Option<io::Take<File>>) -> Result<T, CoreError>,
    ) -> Result<Recovered<T>, CoreError> {
        self.newest(|sequence| {
            self.load_streaming(sequence, streamed)
                .and_then(|(held, source)| accept(sequence, &held, source))
        })
    }

    /// The newest checkpoint for which `attempt` succeeds, with the reason
    /// each newer one was passed over.
    fn newest<T>(
        &self,
        mut attempt: impl FnMut(u64) -> Result<T, CoreError>,
    ) -> Result<Recovered<T>, CoreError> {
        let mut skipped = Vec::new();
        for sequence in self.sequences()?.into_iter().rev() {
            match attempt(sequence) {
                Ok(value) => {
                    return Ok(Recovered {
                        latest: Some((sequence, value)),
                        skipped,
                    });
                }
                Err(error) => skipped.push(Skipped {
                    sequence,
                    kind: error.kind(),
                    reason: error.to_string(),
                }),
            }
        }
        Ok(Recovered {
            latest: None,
            skipped,
        })
    }

    /// Keep the newest `keep` valid checkpoints (`accept` as in
    /// [`ChunkStore::latest_valid`]): remove every manifest older than the
    /// oldest of them, except pinned ones while pins are preserved, then
    /// every chunk no remaining manifest names. With fewer than `keep`
    /// valid checkpoints nothing is removed. Invalid manifests newer than
    /// the oldest kept one stay, for inspection.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for `keep == 0`;
    /// [`CoreError::StorageFailed`] if the host refuses an operation. A
    /// failure part way leaves every kept checkpoint whole: manifests go
    /// before the chunks they name.
    pub fn retain(
        &mut self,
        keep: usize,
        mut accept: impl FnMut(u64, &[u8]) -> Result<(), CoreError>,
    ) -> Result<Retention, CoreError> {
        let sequences = self.prepare_retain(keep)?;
        let mut valid = Vec::new();
        for &sequence in sequences.iter().rev() {
            if valid.len() == keep {
                break;
            }
            if self
                .load(sequence)
                .and_then(|bytes| accept(sequence, &bytes))
                .is_ok()
            {
                valid.push(sequence);
            }
        }
        self.finish_retention(keep, &sequences, valid)
    }

    /// [`ChunkStore::retain`], counting a checkpoint valid when it
    /// [verifies](ChunkStore::verify): its seal, and every chunk's length
    /// and hash, read a buffer at a time. It does not decode a checkpoint,
    /// does not compute the unused container digest, deduplicates chunks
    /// shared across candidate manifests, and stays strictly within the
    /// 1 MiB read-buffer budget (FL-038).
    ///
    /// # Errors
    /// As [`ChunkStore::retain`].
    pub fn retain_intact(&mut self, keep: usize) -> Result<Retention, CoreError> {
        let sequences = self.prepare_retain(keep)?;
        let mut valid = Vec::new();
        let mut chunk_status: BTreeMap<(Sha256Digest, u64), bool> = BTreeMap::new();
        let mut cursor = sequences.len();

        while valid.len() < keep && cursor > 0 {
            let batch_len = (keep - valid.len()).min(cursor);
            let batch_start = cursor - batch_len;
            let candidates = sequences.get(batch_start..cursor).unwrap_or_default();
            cursor = batch_start;

            let mut parsed: Vec<(u64, Option<Vec<(Sha256Digest, u64)>>)> =
                Vec::with_capacity(candidates.len());
            let mut seen = BTreeSet::new();
            let mut pending = Vec::new();

            for &sequence in candidates.iter().rev() {
                let entries = self.sealed_prefix(sequence).ok().and_then(|prefix| {
                    let manifest = CheckpointManifest::parse(&prefix).ok()?;
                    Some(
                        manifest
                            .entries()
                            .iter()
                            .map(|entry| (entry.digest, entry.length))
                            .collect::<Vec<_>>(),
                    )
                });
                if let Some(entries_ref) = &entries {
                    for &key in entries_ref {
                        if !chunk_status.contains_key(&key) && seen.insert(key) {
                            pending.push(key);
                        }
                    }
                }
                parsed.push((sequence, entries));
            }

            // Schedule largest chunks first (LPT) so dominant genome chunks
            // start immediately across concurrent workers/lanes.
            pending.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            self.verify_chunk_batch(&pending, &mut chunk_status)?;

            for (sequence, entries) in parsed {
                if let Some(entries) = entries
                    && entries
                        .iter()
                        .all(|key| chunk_status.get(key).copied().unwrap_or(false))
                {
                    valid.push(sequence);
                }
            }
        }

        self.finish_retention(keep, &sequences, valid)
    }

    fn prepare_retain(&self, keep: usize) -> Result<Vec<u64>, CoreError> {
        if keep == 0 {
            return Err(CoreError::StateConflict {
                operation: "retain checkpoints",
                detail: "keeping none would discard the last valid checkpoint".into(),
            });
        }
        self.sequences()
    }

    /// Verify every `(digest, length)` chunk in `pending` (sorted descending
    /// by `length`) against its stored file, recording results in
    /// `chunk_status`. Total simultaneous read-buffer heap stays strictly
    /// within [`READ_BUFFER_BYTES`].
    fn verify_chunk_batch(
        &self,
        pending: &[(Sha256Digest, u64)],
        chunk_status: &mut BTreeMap<(Sha256Digest, u64), bool>,
    ) -> Result<(), CoreError> {
        let Some(&(_, max_len)) = pending.first() else {
            return Ok(());
        };
        let workers = store_workers();

        // Small batches or single-core hosts run inline with an exact-sized
        // buffer and zero worker-pool overhead.
        if workers <= 1 || max_len <= PARALLEL_HASH_MIN_BYTES {
            let cap = usize::try_from(max_len.min(PIPELINE_HALF_BYTES as u64))
                .unwrap_or(PIPELINE_HALF_BYTES)
                .max(1);
            let mut buf = alloc_read_buffer(cap)?;
            for &(digest, length) in pending {
                let path = chunk_path_in(&self.root, &digest);
                let intact = verify_single_chunk_with_buf(&path, length, &digest, &mut buf);
                chunk_status.insert((digest, length), intact);
            }
            return Ok(());
        }

        // Single chunk larger than the inline threshold: pipeline disk reads
        // against SHA-256 on 2 threads inside 2 × 512 KiB = 1 MiB.
        if let &[(digest, length)] = pending {
            let path = chunk_path_in(&self.root, &digest);
            let mut buffers = ReadBuffers::new();
            let mut chunk = Sha256Stream::new();
            let intact = matches!(
                hash_file_with_buffers(&path, length, &mut chunk, None, &mut buffers),
                Ok(true)
            ) && chunk.finish() == digest;
            chunk_status.insert((digest, length), intact);
            return Ok(());
        }

        let dominant_floor = (max_len / 4).max(PARALLEL_HASH_MIN_BYTES + 1);
        let dominant = pending
            .iter()
            .take_while(|&&(_, length)| length >= dominant_floor)
            .count();

        // When 1 or 2 huge chunks dominate (e.g. two multi-GB genome chunks),
        // run two lockstep double-buffered lanes on 4 threads (`hash0 || read0
        // || hash1 || read1`) inside 4 × 256 KiB = 1 MiB total heap.
        if workers >= 4 && dominant <= 2 {
            let mut slot0 = PipelinedSlot::new(LANE_BUFFER_BYTES)?;
            let mut slot1 = PipelinedSlot::new(LANE_BUFFER_BYTES)?;
            let mut batch = PipelinedBatch::new(&self.root, pending);
            batch.refill(&mut slot0);
            batch.refill(&mut slot1);

            while slot0.active.is_some() || slot1.active.is_some() {
                let w0 = slot0.next_want();
                let w1 = slot1.next_want();
                let mut r0: Result<usize, CoreError> = Ok(0);
                let mut r1: Result<usize, CoreError> = Ok(0);
                {
                    let task1 = slot1.prepare_task(w1, &mut r1);
                    let task0 = slot0.prepare_task(w0, &mut r0);
                    flife_core::pool::scope_workers(workers, "retain checkpoints", |scope| {
                        if let Some(LaneTask {
                            hasher,
                            ready,
                            read,
                        }) = task1
                        {
                            scope.spawn(move || hasher.update(ready));
                            if let Some(LaneRead {
                                file,
                                path,
                                dst,
                                out,
                            }) = read
                            {
                                scope.spawn(move || {
                                    *out = read_full(file, path, dst);
                                });
                            }
                        }
                        if let Some(LaneTask {
                            hasher,
                            ready,
                            read,
                        }) = task0
                        {
                            if let Some(LaneRead {
                                file,
                                path,
                                dst,
                                out,
                            }) = read
                            {
                                scope.spawn(move || {
                                    *out = read_full(file, path, dst);
                                });
                            }
                            hasher.update(ready);
                        }
                    })?;
                }
                batch.advance(&mut slot0, w0, r0);
                batch.advance(&mut slot1, w1, r1);
                batch.refill(&mut slot0);
                batch.refill(&mut slot1);
            }

            for (&key, intact) in pending.iter().zip(batch.results) {
                chunk_status.insert(key, intact);
            }
            return Ok(());
        }

        // Three or more dominant chunks (or 2–3 core hosts): verify distinct
        // chunks concurrently across `worker_count` workers, dividing the
        // `READ_BUFFER_BYTES` ceiling evenly so total heap never exceeds 1 MiB.
        let worker_count = workers.min(pending.len());
        let buf_bytes = (READ_BUFFER_BYTES / worker_count)
            .min(usize::try_from(max_len).unwrap_or(READ_BUFFER_BYTES))
            .max(1);
        let mut worker_bufs = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            worker_bufs.push(alloc_read_buffer(buf_bytes)?);
        }
        let next_idx = AtomicUsize::new(0);
        let results: Vec<AtomicBool> = (0..pending.len()).map(|_| AtomicBool::new(false)).collect();
        let root = self.root.as_path();
        flife_core::pool::parallel_for_each_mut(
            workers,
            "retain checkpoints",
            &mut worker_bufs,
            |_worker_id, buf| {
                loop {
                    let index = next_idx.fetch_add(1, Ordering::Relaxed);
                    let Some(&(digest, length)) = pending.get(index) else {
                        break;
                    };
                    let path = chunk_path_in(root, &digest);
                    let intact = verify_single_chunk_with_buf(&path, length, &digest, buf);
                    if let Some(slot) = results.get(index) {
                        slot.store(intact, Ordering::Relaxed);
                    }
                }
            },
        )?;
        for (&key, result) in pending.iter().zip(&results) {
            chunk_status.insert(key, result.load(Ordering::Relaxed));
        }
        Ok(())
    }

    fn finish_retention(
        &self,
        keep: usize,
        sequences: &[u64],
        valid: Vec<u64>,
    ) -> Result<Retention, CoreError> {
        let oldest = if valid.len() == keep {
            valid.last().copied()
        } else {
            None
        };
        let held: BTreeSet<u64> = if self.preserve_pinned {
            self.pins()?
                .into_iter()
                .map(|(sequence, _)| sequence)
                .collect()
        } else {
            BTreeSet::new()
        };
        let (pinned, removed): (Vec<u64>, Vec<u64>) = oldest
            .map_or_else(Vec::new, |oldest| {
                sequences
                    .iter()
                    .copied()
                    .filter(|sequence| *sequence < oldest)
                    .collect()
            })
            .into_iter()
            .partition(|sequence| held.contains(sequence));
        for &sequence in &removed {
            let path = self.manifest_path(sequence);
            fs::remove_file(&path).map_err(|error| storage("remove manifest", &path, &error))?;
        }
        if self.durability.flushes() && !removed.is_empty() {
            flush_directory(&self.root.join(CHECKPOINTS))?;
        }

        // Every chunk a remaining manifest names. A manifest that does not
        // parse names none, and is not resumable either way.
        let mut referenced = BTreeSet::new();
        for sequence in self.sequences()? {
            let path = self.manifest_path(sequence);
            let small = fs::symlink_metadata(&path).is_ok_and(|metadata| {
                metadata.is_file() && metadata.len() <= MANIFEST_FILE_MAX_BYTES
            });
            if !small {
                continue;
            }
            let prefix =
                fs::read(&path).map_err(|error| storage("read manifest", &path, &error))?;
            if let Ok(manifest) = CheckpointManifest::parse(&prefix) {
                referenced.extend(manifest.entries().iter().map(|entry| entry.digest.to_hex()));
            }
        }
        let chunks = self.root.join(CHUNKS);
        let (mut chunks_removed, mut chunks_kept) = (0, 0);
        for shard in
            fs::read_dir(&chunks).map_err(|error| storage("list chunks", &chunks, &error))?
        {
            let shard = shard
                .map_err(|error| storage("list chunks", &chunks, &error))?
                .path();
            if !shard.is_dir() {
                continue;
            }
            for chunk in
                fs::read_dir(&shard).map_err(|error| storage("list chunks", &shard, &error))?
            {
                let chunk = chunk.map_err(|error| storage("list chunks", &shard, &error))?;
                let Some(name) = chunk.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if !is_digest_name(&name) {
                    continue;
                }
                if referenced.contains(&name) {
                    chunks_kept += 1;
                } else {
                    let path = chunk.path();
                    fs::remove_file(&path)
                        .map_err(|error| storage("remove chunk", &path, &error))?;
                    chunks_removed += 1;
                }
            }
        }
        Ok(Retention {
            valid,
            removed,
            pinned,
            chunks_removed,
            chunks_kept,
        })
    }
}

/// Whether `path` holds `entry`'s chunk intact.
fn stored(path: &Path, entry: &ManifestEntry) -> Result<Stored, CoreError> {
    stored_as(path, entry.length, &entry.digest)
}

/// Whether `path` holds the `length` bytes with `digest` intact, hashed a
/// buffer at a time.
fn stored_as(path: &Path, length: u64, digest: &Sha256Digest) -> Result<Stored, CoreError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Stored::Absent),
        Err(error) => Err(storage("inspect chunk", path, &error)),
        Ok(metadata) if !metadata.is_file() || metadata.len() != length => Ok(Stored::Damaged),
        Ok(_) => {
            let mut hash = Sha256Stream::new();
            let mut buffers = ReadBuffers::new();
            let whole = hash_file_with_buffers(path, length, &mut hash, None, &mut buffers)?;
            Ok(if whole && hash.finish() == *digest {
                Stored::Intact
            } else {
                Stored::Damaged
            })
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    reason = "unit test: an assertion failure here is a test failure"
)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Regression: `StoreLock::drop` must explicitly `unlock()` the lock file
    /// before closing the descriptor. Otherwise a duplicated descriptor on the
    /// same open file description (such as one transiently inherited by a
    /// concurrent `fork`/`posix_spawn` in another thread) keeps `flock` held
    /// after `ChunkStore` drops, causing an immediate reopen to fail with
    /// `"another writer holds ..."`.
    #[test]
    fn dropping_store_unlocks_even_with_duplicated_descriptor() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = TempDir(std::env::temp_dir().join(format!(
            "flife-store-lock-unlock-{}-{nanos}",
            std::process::id()
        )));

        let store = ChunkStore::open(&dir.0, Durability::ProcessCrash).expect("opens store");
        let duplicated = store._lock.0.try_clone().expect("duplicates lock fd");

        // While `store` is live, a second writer is still strictly refused.
        let busy = ChunkStore::open(&dir.0, Durability::ProcessCrash);
        assert!(
            matches!(
                busy,
                Err(CoreError::StateConflict {
                    operation: "open checkpoint store",
                    ..
                })
            ),
            "second writer while open must be refused, got {busy:?}"
        );

        // Dropping `store` explicitly unlocks the open file description even
        // though `duplicated` still holds a live descriptor to it.
        drop(store);
        let reopened = ChunkStore::open(&dir.0, Durability::ProcessCrash)
            .expect("reopens immediately after drop despite duplicated descriptor");
        drop(reopened);
        drop(duplicated);
    }

    fn fresh_dir(label: &str) -> TempDir {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        TempDir(std::env::temp_dir().join(format!(
            "flife-store-{label}-{}-{nanos}-{n}",
            std::process::id()
        )))
    }

    fn pattern_bytes(seed: u8, len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len);
        let mut state = u64::from(seed).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x517c_c1b7_2722_0a95;
        for index in 0..len {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407 ^ (index as u64));
            out.push((state >> 56) as u8);
        }
        out
    }

    fn build_checkpoint(seed: u8, sizes: &[(ChunkFamily, usize)]) -> Vec<u8> {
        let mut writer = flife_core::checkpoint::CheckpointWriter::new("field-ecology-v2");
        for &(family, len) in sizes {
            let bytes = pattern_bytes(seed.wrapping_add(family.code()), len);
            writer
                .chunk(family, 1, u64::try_from(len).expect("len"), bytes, &[])
                .expect("adds chunk");
        }
        writer.finish().expect("seals container")
    }

    /// Pipelined `verify`, parallel `load`/`load_streaming`, and `stored_as`
    /// must match naive whole-container SHA-256 and bytes bit-for-bit across
    /// every block-size boundary (0, 1, 64 KiB, 64 KiB+1, 256 KiB, 256 KiB+1,
    /// 512 KiB, 512 KiB+1, 1 MiB+137) and reject single-bit flips and
    /// single-byte truncations in both small and multi-block chunks.
    #[test]
    fn pipelined_verify_and_parallel_load_match_naive_across_boundaries() {
        let sizes = [
            (ChunkFamily::Configuration, 0),
            (ChunkFamily::Random, 1),
            (ChunkFamily::Topology, 137),
            (
                ChunkFamily::Environment,
                usize::try_from(PARALLEL_HASH_MIN_BYTES).expect("fits"),
            ),
            (
                ChunkFamily::Lifecycle,
                usize::try_from(PARALLEL_HASH_MIN_BYTES).expect("fits") + 1,
            ),
            (ChunkFamily::Field, LANE_BUFFER_BYTES),
            (ChunkFamily::Particles, LANE_BUFFER_BYTES + 1),
            (ChunkFamily::Held, PIPELINE_HALF_BYTES),
            (ChunkFamily::Ledger, PIPELINE_HALF_BYTES + 1),
            (ChunkFamily::Commands, LANE_BUFFER_BYTES * 3 + 19),
            (ChunkFamily::Genomes, READ_BUFFER_BYTES + 137),
        ];
        let whole = build_checkpoint(1, &sizes);
        let expected_digest = Sha256Digest::of(&whole);

        let dir = fresh_dir("verify-boundaries");
        let mut store = ChunkStore::open(&dir.0, Durability::ProcessCrash).expect("opens");
        let pub1 = store.publish(&whole).expect("publishes");
        assert_eq!(
            store.verify(pub1.sequence).expect("verifies"),
            expected_digest
        );
        assert_eq!(store.load(pub1.sequence).expect("loads"), whole);

        // Republishing shares every chunk (exercises `stored_as` across all sizes).
        let pub2 = store.publish(&whole).expect("republishes");
        assert_eq!(pub2.shared, sizes.len());
        assert_eq!(
            store.verify(pub2.sequence).expect("verifies"),
            expected_digest
        );

        // Parallel `load_streaming` matches `load` chunk-for-chunk.
        let (held, streamed) = store
            .load_streaming(pub1.sequence, ChunkFamily::Genomes)
            .expect("loads streaming");
        let mut streamed_bytes = Vec::new();
        streamed
            .expect("genomes handle")
            .read_to_end(&mut streamed_bytes)
            .expect("reads genomes");
        let manifest = CheckpointManifest::parse(&whole).expect("manifest");
        let genome_entry = manifest
            .entries()
            .iter()
            .find(|entry| entry.family == ChunkFamily::Genomes)
            .expect("genome entry");
        let g_start =
            manifest.sealed_length() + usize::try_from(genome_entry.offset).expect("fits");
        let g_end = g_start + usize::try_from(genome_entry.length).expect("fits");
        assert_eq!(streamed_bytes, &whole[g_start..g_end]);
        assert_eq!(held.len() + streamed_bytes.len(), whole.len());

        // Flip 1 bit in the tail of the multi-block `Genomes` chunk and check
        // rejection in `verify`, `load`, and `retain_intact`.
        let genome_path = store.chunk_path(&genome_entry.digest);
        let original = fs::read(&genome_path).expect("reads original");
        let mut flipped = original.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 0x80;
        fs::write(&genome_path, &flipped).expect("writes flipped");
        assert!(matches!(
            store.verify(pub1.sequence),
            Err(CoreError::IncompatibleCheckpoint { .. })
        ));
        assert!(matches!(
            store.load(pub1.sequence),
            Err(CoreError::IncompatibleCheckpoint { .. })
        ));
        let retained_damaged = store.retain_intact(1).expect("retains damaged");
        assert!(retained_damaged.valid.is_empty());

        // Truncate by 1 byte -> refused cleanly before restoring original.
        fs::write(&genome_path, &original[..original.len() - 1]).expect("writes truncated");
        assert!(matches!(
            store.verify(pub1.sequence),
            Err(CoreError::IncompatibleCheckpoint { .. })
        ));
        assert!(matches!(
            store.load(pub1.sequence),
            Err(CoreError::IncompatibleCheckpoint { .. })
        ));
        fs::write(&genome_path, &original).expect("restores");
        assert_eq!(
            store.verify(pub1.sequence).expect("restored"),
            expected_digest
        );
    }

    /// `retain_intact` must produce identical `Retention` results to serial
    /// `retain` across all parallel verification regimes (1-dominant, 2-lane
    /// lockstep pipelined, 3+/4-worker atomic pool, shared chunks, and damaged
    /// newest or middle checkpoints).
    #[test]
    fn retain_intact_matches_serial_across_all_parallel_regimes() {
        let shared_config = pattern_bytes(77, 1024);
        let shared_field = pattern_bytes(88, LANE_BUFFER_BYTES + 333);
        let make_ckpt = |seed: u8, genome_len: usize| -> Vec<u8> {
            let mut writer = flife_core::checkpoint::CheckpointWriter::new("field-ecology-v2");
            writer
                .chunk(
                    ChunkFamily::Configuration,
                    1,
                    u64::try_from(shared_config.len()).expect("len"),
                    shared_config.clone(),
                    &[],
                )
                .expect("config");
            writer
                .chunk(
                    ChunkFamily::Field,
                    1,
                    u64::try_from(shared_field.len()).expect("len"),
                    shared_field.clone(),
                    &[],
                )
                .expect("field");
            let particles = pattern_bytes(seed.wrapping_add(10), 100_000);
            writer
                .chunk(
                    ChunkFamily::Particles,
                    1,
                    u64::try_from(particles.len()).expect("len"),
                    particles,
                    &[],
                )
                .expect("particles");
            let genomes = pattern_bytes(seed.wrapping_add(20), genome_len);
            writer
                .chunk(
                    ChunkFamily::Genomes,
                    1,
                    u64::try_from(genomes.len()).expect("len"),
                    genomes,
                    &[],
                )
                .expect("genomes");
            writer.finish().expect("seals")
        };

        // 5 checkpoints with multi-block genomes so keep=1 (1-dominant),
        // keep=2 (2-lane lockstep pipelined), and keep=4 (4-worker atomic pool)
        // are all exercised against an identical serial `retain` twin.
        let checkpoints = [
            make_ckpt(1, 300_000),
            make_ckpt(2, READ_BUFFER_BYTES + 511),
            make_ckpt(3, READ_BUFFER_BYTES + 1023),
            make_ckpt(4, READ_BUFFER_BYTES + 2047),
            make_ckpt(5, READ_BUFFER_BYTES + 4093),
        ];

        for (case, damage_idx, keep) in [
            ("intact-keep-1", None, 1),
            ("intact-keep-2", None, 2),
            ("intact-keep-4", None, 4),
            ("damaged-newest-keep-2", Some(4_usize), 2),
            ("damaged-middle-keep-3", Some(2_usize), 3),
        ] {
            let intact_dir = fresh_dir(&format!("retain-intact-{case}"));
            let serial_dir = fresh_dir(&format!("retain-serial-{case}"));
            let mut by_intact =
                ChunkStore::open(&intact_dir.0, Durability::ProcessCrash).expect("opens");
            let mut by_serial =
                ChunkStore::open(&serial_dir.0, Durability::ProcessCrash).expect("opens");
            for bytes in &checkpoints {
                by_intact.publish(bytes).expect("publishes");
                by_serial.publish(bytes).expect("publishes");
            }
            if let Some(idx) = damage_idx {
                let manifest = CheckpointManifest::parse(&checkpoints[idx]).expect("manifest");
                let digest = manifest
                    .entries()
                    .iter()
                    .find(|entry| entry.family == ChunkFamily::Genomes)
                    .expect("genomes")
                    .digest;
                for store in [&by_intact, &by_serial] {
                    let path = store.chunk_path(&digest);
                    let mut raw = fs::read(&path).expect("reads");
                    let mid = raw.len() / 2;
                    raw[mid] ^= 0x01;
                    fs::write(&path, raw).expect("corrupts");
                }
            }
            let got = by_intact.retain_intact(keep).expect("retain_intact");
            let expected = by_serial.retain(keep, |_, _| Ok(())).expect("retain");
            assert_eq!(got, expected, "case {case} must match serial retain");
            assert_eq!(
                by_intact.sequences().expect("seqs"),
                by_serial.sequences().expect("seqs"),
                "case {case} kept sequences must match"
            );
        }
    }
}
