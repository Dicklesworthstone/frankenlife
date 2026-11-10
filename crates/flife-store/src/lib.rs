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

pub mod experiment;
pub mod journal;
mod stream;

pub use experiment::{CatalogedRun, EXPERIMENT_CATALOG_FORMAT, ExperimentCatalog};
pub use journal::{BranchOrigin, CATALOG_FORMAT, CommandJournal, IndexedCheckpoint};
pub use stream::{Observer, Stream};

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use flife_core::checkpoint::{
    CheckpointManifest, CheckpointReader, MANIFEST_MAX_BYTES, MODEL_MAX_BYTES, ManifestEntry,
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
const MANIFEST_SUFFIX: &str = ".manifest";

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

/// An open checkpoint store, held by its one writer.
#[derive(Debug)]
pub struct ChunkStore {
    root: PathBuf,
    durability: Durability,
    next: u64,
    swept: usize,
    _lock: File,
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

/// The bytes a streamed read holds at once.
const READ_BUFFER_BYTES: usize = 1 << 20;

/// Feed `length` bytes of the file at `path` to `chunk` and `container`,
/// a buffer at a time. False when the file ends early.
fn hash_file(
    path: &Path,
    length: u64,
    chunk: &mut Sha256Stream,
    mut container: Option<&mut Sha256Stream>,
) -> Result<bool, CoreError> {
    let mut file = File::open(path).map_err(|error| storage("read chunk", path, &error))?;
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(READ_BUFFER_BYTES)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: "chunk read buffer",
            bytes: READ_BUFFER_BYTES as u64,
        })?;
    buffer.resize(READ_BUFFER_BYTES, 0);
    let mut remaining = length;
    while remaining > 0 {
        let want =
            usize::try_from(remaining.min(READ_BUFFER_BYTES as u64)).unwrap_or(READ_BUFFER_BYTES);
        let read = file
            .read(&mut buffer[..want])
            .map_err(|error| storage("read chunk", path, &error))?;
        if read == 0 {
            return Ok(false);
        }
        let bytes = &buffer[..read];
        chunk.update(bytes);
        if let Some(container) = container.as_deref_mut() {
            container.update(bytes);
        }
        remaining -= u64::try_from(read).unwrap_or(remaining);
    }
    Ok(true)
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
        match lock.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => {
                return Err(CoreError::StateConflict {
                    operation: "open checkpoint store",
                    detail: format!("another writer holds {}", root.display()),
                });
            }
            Err(fs::TryLockError::Error(error)) => {
                return Err(storage("lock store", &lock_path, &error));
            }
        }

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
                let foreign = fs::read_dir(&root)
                    .map_err(|error| storage("list store", &root, &error))?
                    .filter_map(Result::ok)
                    .any(|entry| entry.file_name() != LOCK);
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
        for directory in [CHUNKS, CHECKPOINTS, TEMPORARY] {
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
            offset = offset.saturating_add(entry.length);
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
                .zip(usize::try_from(entry.offset.saturating_add(entry.length)).ok())
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
        let prefix = self.sealed_prefix(sequence)?;
        let manifest = CheckpointManifest::parse(&prefix)?;
        let total = u64::try_from(prefix.len())
            .ok()
            .and_then(|sealed| sealed.checked_add(manifest.payload_length()))
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
        for entry in manifest.entries() {
            let chunk = self.chunk_path(&entry.digest);
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
        Ok(bytes)
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
        let mut container = Sha256Stream::new();
        container.update(&prefix);
        for entry in manifest.entries() {
            let path = self.chunk_path(&entry.digest);
            let mut chunk = Sha256Stream::new();
            let whole = hash_file(&path, entry.length, &mut chunk, Some(&mut container))?;
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
        let mut skipped = Vec::new();
        for sequence in self.sequences()?.into_iter().rev() {
            match self
                .load(sequence)
                .and_then(|bytes| accept(sequence, &bytes))
            {
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
    /// oldest of them, then every chunk no remaining manifest names. With
    /// fewer than `keep` valid checkpoints nothing is removed. Invalid
    /// manifests newer than the oldest kept one stay, for inspection.
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
        self.retain_by(keep, |store, sequence| {
            store
                .load(sequence)
                .and_then(|bytes| accept(sequence, &bytes))
                .is_ok()
        })
    }

    /// [`ChunkStore::retain`], counting a checkpoint valid when it
    /// [verifies](ChunkStore::verify): its seal, and every chunk's length
    /// and hash, read a buffer at a time. It does not decode a checkpoint,
    /// and holds at most one read buffer (FL-038).
    ///
    /// # Errors
    /// As [`ChunkStore::retain`].
    pub fn retain_intact(&mut self, keep: usize) -> Result<Retention, CoreError> {
        self.retain_by(keep, |store, sequence| store.verify(sequence).is_ok())
    }

    fn retain_by(
        &mut self,
        keep: usize,
        mut valid_checkpoint: impl FnMut(&Self, u64) -> bool,
    ) -> Result<Retention, CoreError> {
        if keep == 0 {
            return Err(CoreError::StateConflict {
                operation: "retain checkpoints",
                detail: "keeping none would discard the last valid checkpoint".into(),
            });
        }
        let sequences = self.sequences()?;
        let mut valid = Vec::new();
        for &sequence in sequences.iter().rev() {
            if valid.len() == keep {
                break;
            }
            if valid_checkpoint(self, sequence) {
                valid.push(sequence);
            }
        }
        let oldest = if valid.len() == keep {
            valid.last().copied()
        } else {
            None
        };
        let removed: Vec<u64> = oldest.map_or_else(Vec::new, |oldest| {
            sequences
                .iter()
                .copied()
                .filter(|sequence| *sequence < oldest)
                .collect()
        });
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
            let whole = hash_file(path, length, &mut hash, None)?;
            Ok(if whole && hash.finish() == *digest {
                Stored::Intact
            } else {
                Stored::Damaged
            })
        }
    }
}
