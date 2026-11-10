//! Streamed publication (FL-038; plan §16.4, §16.5): a checkpoint written
//! to the store as it is encoded, never held whole.
//!
//! [`ChunkStore::stream`](super::ChunkStore::stream) opens a [`Stream`], a
//! [`ChunkSink`]: each chunk's pages go to a temporary file as they are
//! encoded; when the chunk ends, a chunk the store already holds intact is
//! shared and its temporary removed, and any other is flushed under the
//! store's [`Durability`] and renamed to its digest's path, exactly as
//! [`ChunkStore::publish`](super::ChunkStore::publish) stores it.
//! [`ChunkStore::commit`](super::ChunkStore::commit) then checks the sealed
//! prefix names exactly the chunks stored, in order, and publishes the
//! manifest by the same link. Until that link nothing is published: a
//! stream dropped, cancelled or failed removes its temporary files, and the
//! chunks it already stored are unreferenced until retention removes them.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use flife_core::checkpoint::{ChunkFamily, ChunkSink};
use flife_core::{CoreError, Sha256Digest};

use super::{
    CHUNKS, Durability, Flow, Publication, Step, Stored, TEMPORARY, chunk_path_in,
    create_directory, flow, flush_directory, remove_all, storage, stored_as,
};

/// A step observer, as [`ChunkStore::publish_observed`](super::ChunkStore::publish_observed) takes.
pub type Observer = Box<dyn FnMut(Step) -> Flow + Send>;

struct Open {
    index: usize,
    family: ChunkFamily,
    path: PathBuf,
    file: File,
}

/// A checkpoint being published as it is encoded: opened by
/// [`ChunkStore::stream`](super::ChunkStore::stream), committed by
/// [`ChunkStore::commit`](super::ChunkStore::commit).
pub struct Stream {
    pub(crate) root: PathBuf,
    durability: Durability,
    pub(crate) sequence: u64,
    open: Option<Open>,
    pub(crate) stored: Vec<(ChunkFamily, u64, Sha256Digest)>,
    pub(crate) temporaries: Vec<PathBuf>,
    pub(crate) publication: Publication,
    pub(crate) observe: Observer,
    /// The first refusal: once set, every later call refuses, so nothing
    /// more is written and [`ChunkStore::commit`](super::ChunkStore::commit)
    /// publishes nothing.
    pub(crate) failed: Option<CoreError>,
}

impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stream")
            .field("root", &self.root)
            .field("sequence", &self.sequence)
            .field("stored", &self.stored.len())
            .field("failed", &self.failed)
            .finish_non_exhaustive()
    }
}

impl Stream {
    pub(crate) fn new(
        root: PathBuf,
        durability: Durability,
        sequence: u64,
        observe: Observer,
    ) -> Self {
        Self {
            root,
            durability,
            sequence,
            open: None,
            stored: Vec::new(),
            temporaries: Vec::new(),
            publication: Publication {
                sequence,
                written: 0,
                linked: 0,
                shared: 0,
                repaired: 0,
                bytes_written: 0,
            },
            observe,
            failed: None,
        }
    }

    /// The sequence number this checkpoint takes when committed.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Chunks stored so far, in manifest order.
    #[must_use]
    pub fn chunks(&self) -> usize {
        self.stored.len()
    }

    /// Whether a chunk has begun and not ended.
    pub(crate) const fn is_open(&self) -> bool {
        self.open.is_some()
    }

    fn usable(&self) -> Result<(), CoreError> {
        match &self.failed {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn step(&mut self, step: Step) -> Result<(), CoreError> {
        flow((self.observe)(step))
    }

    /// Record `outcome`'s error, if any, as the stream's refusal.
    fn hold<T>(&mut self, outcome: Result<T, CoreError>) -> Result<T, CoreError> {
        if let Err(error) = &outcome
            && self.failed.is_none()
        {
            self.failed = Some(error.clone());
        }
        outcome
    }

    fn open_chunk(&mut self, index: usize, family: ChunkFamily) -> Result<(), CoreError> {
        self.usable()?;
        if self.open.is_some() || index != self.stored.len() {
            return Err(CoreError::StateConflict {
                operation: "stream checkpoint",
                detail: format!(
                    "chunk {index} ({}) began with {} chunks stored and one {}open",
                    family.name(),
                    self.stored.len(),
                    if self.open.is_some() { "" } else { "not " }
                ),
            });
        }
        let path = self
            .root
            .join(TEMPORARY)
            .join(format!("{:016x}-{index}.chunk", self.sequence));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| storage("create temporary file", &path, &error))?;
        self.temporaries.push(path.clone());
        self.open = Some(Open {
            index,
            family,
            path,
            file,
        });
        Ok(())
    }

    fn write_page(&mut self, bytes: &[u8]) -> Result<(), CoreError> {
        self.usable()?;
        let open = self.open.as_mut().ok_or_else(|| CoreError::StateConflict {
            operation: "stream checkpoint",
            detail: "a page arrived outside a chunk".into(),
        })?;
        open.file
            .write_all(bytes)
            .map_err(|error| storage("write temporary file", &open.path, &error))
    }

    fn close_chunk(&mut self, length: u64, digest: Sha256Digest) -> Result<(), CoreError> {
        self.usable()?;
        let Open {
            index,
            family,
            path: temporary,
            file,
        } = self.open.take().ok_or_else(|| CoreError::StateConflict {
            operation: "stream checkpoint",
            detail: "a chunk ended that never began".into(),
        })?;
        let flushes = matches!(self.durability, Durability::PowerLoss);
        let target = chunk_path_in(&self.root, &digest);
        match stored_as(&target, length, &digest)? {
            Stored::Intact => {
                drop(file);
                fs::remove_file(&temporary)
                    .map_err(|error| storage("remove temporary file", &temporary, &error))?;
                self.temporaries.retain(|candidate| candidate != &temporary);
                self.publication.shared += 1;
                self.stored.push((family, length, digest));
                return self.step(Step::ChunkShared { index });
            }
            Stored::Absent => self.publication.written += 1,
            Stored::Damaged => self.publication.repaired += 1,
        }
        let written = file
            .metadata()
            .map_err(|error| storage("inspect temporary file", &temporary, &error))?
            .len();
        if written != length {
            return Err(CoreError::StateConflict {
                operation: "stream checkpoint",
                detail: format!(
                    "chunk {} ended at {length} bytes, its file holds {written}",
                    family.name()
                ),
            });
        }
        self.publication.bytes_written = self.publication.bytes_written.saturating_add(length);
        self.step(Step::ChunkWritten { index })?;
        if flushes {
            file.sync_all()
                .map_err(|error| storage("flush chunk", &temporary, &error))?;
            self.step(Step::ChunkFlushed { index })?;
        }
        drop(file);
        let directory = target.parent().unwrap_or(&self.root).to_path_buf();
        if !directory.is_dir() {
            create_directory(&directory)?;
            if flushes {
                flush_directory(&self.root.join(CHUNKS))?;
            }
        }
        fs::rename(&temporary, &target).map_err(|error| storage("store chunk", &target, &error))?;
        self.temporaries.retain(|candidate| candidate != &temporary);
        if flushes {
            flush_directory(&directory)?;
        }
        self.stored.push((family, length, digest));
        self.step(Step::ChunkStored { index })
    }
}

impl ChunkSink for Stream {
    fn begin(&mut self, index: usize, family: ChunkFamily) -> Result<(), CoreError> {
        let outcome = self.open_chunk(index, family);
        self.hold(outcome)
    }

    fn page(&mut self, bytes: &[u8]) -> Result<(), CoreError> {
        let outcome = self.write_page(bytes);
        self.hold(outcome)
    }

    fn end(&mut self, length: u64, digest: Sha256Digest) -> Result<(), CoreError> {
        let outcome = self.close_chunk(length, digest);
        self.hold(outcome)
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        drop(self.open.take());
        remove_all(&self.temporaries);
    }
}
