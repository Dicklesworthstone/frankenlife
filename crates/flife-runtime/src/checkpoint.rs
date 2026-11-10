//! Session checkpoints (FL-016; plan §16.3, Appendix B.2, INV-14): a
//! committed world and its complete command closure as one verified byte
//! string.
//!
//! [`checkpoint`] writes the world's chunks
//! ([`WORLD_CHECKPOINT_CHUNKS`]) and
//! the book's [`ChunkFamily::Commands`]. [`restore`] verifies the
//! container, then rebuilds the world and the book and checks them against
//! each other. A restored world continues exactly as the uninterrupted one:
//! pending future commands apply at their boundaries, once, and a retry of
//! an accepted command is recognised as one.
//!
//! [`checkpoint_into`] writes the same bytes without holding them: each
//! chunk streams to a [`ChunkSink`] a page at a time as it is encoded, and
//! only the sealed prefix comes back (FL-038).
//!
//! This is the encoding. Writing chunks durably and publishing a manifest
//! atomically is FL-035's chunk store.

use flife_core::CoreError;
use flife_core::checkpoint::{CheckpointReader, CheckpointWriter, ChunkFamily, ChunkSink};
use flife_engine::{NumericalProfile, WORLD_CHECKPOINT_CHUNKS, World};

use crate::book::CommandBook;

/// Every family a session checkpoint holds.
pub const SESSION_CHECKPOINT_CHUNKS: [ChunkFamily; 11] = [
    WORLD_CHECKPOINT_CHUNKS[0],
    WORLD_CHECKPOINT_CHUNKS[1],
    WORLD_CHECKPOINT_CHUNKS[2],
    WORLD_CHECKPOINT_CHUNKS[3],
    WORLD_CHECKPOINT_CHUNKS[4],
    WORLD_CHECKPOINT_CHUNKS[5],
    WORLD_CHECKPOINT_CHUNKS[6],
    WORLD_CHECKPOINT_CHUNKS[7],
    WORLD_CHECKPOINT_CHUNKS[8],
    WORLD_CHECKPOINT_CHUNKS[9],
    ChunkFamily::Commands,
];

/// A world and command book read back from a checkpoint.
#[derive(Debug)]
pub struct Restored {
    /// The committed world.
    pub world: World,
    /// Its command closure.
    pub book: CommandBook,
}

/// Encode committed `world` and its `book`.
///
/// # Errors
/// [`CoreError::StateConflict`] when the book belongs to another
/// configuration, or the run stopped (see
/// [`World::write_checkpoint`]); [`CoreError::IncompatibleCheckpoint`]
/// when the container's bounds are exceeded.
pub fn checkpoint(world: &World, book: &CommandBook) -> Result<Vec<u8>, CoreError> {
    agree(world, book)?;
    let mut writer = CheckpointWriter::new(&world.config().config().model);
    world.write_checkpoint(&mut writer)?;
    book.write_checkpoint(&mut writer)?;
    writer.finish()
}

/// Encode committed `world` and its `book` as [`checkpoint`] does, streaming
/// each chunk to `sink` page by page ([`ChunkSink`]) rather than holding
/// the container (FL-038's pause-and-stream). Returns the sealed prefix:
/// followed by the pages `sink` received, it is [`checkpoint`]'s bytes.
///
/// # Errors
/// As [`checkpoint`], and `sink`'s own.
pub fn checkpoint_into(
    world: &World,
    book: &CommandBook,
    sink: &mut dyn ChunkSink,
) -> Result<Vec<u8>, CoreError> {
    agree(world, book)?;
    let mut writer = CheckpointWriter::streaming(&world.config().config().model, sink);
    world.write_checkpoint(&mut writer)?;
    book.write_checkpoint(&mut writer)?;
    writer.seal()
}

fn agree(world: &World, book: &CommandBook) -> Result<(), CoreError> {
    if book.revision() == world.config().config_hash() {
        return Ok(());
    }
    Err(CoreError::StateConflict {
        operation: "checkpoint session",
        detail: format!(
            "the book runs configuration {}, the world {}",
            book.revision().to_hex(),
            world.config().config_hash().to_hex()
        ),
    })
}

/// Verify and decode `bytes` under `profile`.
///
/// # Errors
/// [`CoreError::IncompatibleCheckpoint`] naming the first defect: a
/// damaged or truncated container, a missing chunk, or chunks that do not
/// describe one consistent world and closure; and as
/// [`World::read_checkpoint`].
pub fn restore(bytes: &[u8], profile: NumericalProfile) -> Result<Restored, CoreError> {
    let reader = CheckpointReader::parse(bytes, &SESSION_CHECKPOINT_CHUNKS)?;
    let world = World::read_checkpoint(&reader, profile)?;
    let book = CommandBook::read_checkpoint(&reader, world.tick(), world.config().config_hash())?;
    Ok(Restored { world, book })
}
