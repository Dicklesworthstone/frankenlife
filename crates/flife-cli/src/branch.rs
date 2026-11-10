//! Branches (FL-039; plan §14.4, §15.3, §16.3, §16.7, Appendix B.1).
//!
//! [`branch`] makes a new run directory that starts from a durable
//! checkpoint of another: a control branch with no commands of its own, or
//! an intervention branch with energy brushes scheduled at or after the
//! checkpoint's tick. Each then runs on with `flife resume`, independently
//! of the other and of its parent.
//!
//! - The branch is a new session. Its identity is derived from the parent
//!   checkpoint's SHA-256 and the branch's name ([`branch_session`]), so
//!   making the same branch again names the same session, and siblings with
//!   different names differ. Its command book inherits every command and
//!   outcome of the checkpoint and records the fork (the parent session,
//!   the admissions it had made, the tick and the checkpoint's SHA-256), so
//!   each later checkpoint carries the lineage (§16.3). The branch admits
//!   only commands of its own session.
//! - The founding checkpoint is the parent's world re-encoded with the
//!   branch's book. Its world chunks (configuration, field, genomes,
//!   particles and the rest) are byte-identical to the parent's, so the
//!   branch's store links the parent's verified chunk files instead of
//!   copying them (FL-035's [`ChunkStore::publish_sharing`]). Only the
//!   command closure is written.
//! - The branch's catalog records its lineage, its name, the parent run,
//!   the parent checkpoint's sequence and the creation time. For a
//!   `write-ahead-before-apply` run its journal starts with the history it
//!   inherited and its own commands; it never holds the parent's later
//!   commands, and the parent's journal never holds the branch's.
//!
//! The parent's store is opened as its writer, so a run another process
//! holds is refused, and nothing moves under the checkpoint while it is
//! read and linked.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use flife_core::canonical::Sha256Digest;
use flife_core::ids::SessionId;
use flife_core::{CoreError, Tick};
use flife_engine::NumericalProfile;
use flife_runtime::{
    Admission, Boundary, Durability, JournalContents, SessionCheckpoint, SessionConfig, checkpoint,
};
use flife_store::{BranchOrigin, ChunkStore, CommandJournal};
use serde::Serialize;

use crate::persist::{
    CATALOG_FILE, Checkpoints, Injection, Persistence, STORE_DIR, StoreDurability, create,
    on_plain_thread, read_config, restorable, run_identity, scheduled,
};
use crate::run::{CheckpointAt, RUN_SCHEMA, Record};

/// The longest branch name.
pub const BRANCH_NAME_MAX_BYTES: usize = 256;

/// What to branch.
#[derive(Debug, Clone, PartialEq)]
pub struct BranchOptions {
    /// The committed tick of the parent checkpoint to branch from; the
    /// newest that restores if `None`.
    pub at: Option<u64>,
    /// The branch's name; the new directory's name if `None`.
    pub name: Option<String>,
    /// The branch's own commands: energy brushes at or after the
    /// checkpoint's tick, numbered after every inherited command.
    pub injections: Vec<Injection>,
    /// How the founding checkpoint is published.
    pub durability: StoreDurability,
}

/// A fork, as a record reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ForkRecord {
    /// The session forked from.
    pub parent: u64,
    /// Commands it had admitted.
    pub admitted: u64,
    /// The checkpoint's committed tick.
    pub tick: u64,
    /// The checkpoint container's SHA-256.
    pub sha256: String,
}

/// The parent checkpoint a branch starts from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BranchParent {
    /// The parent run directory, as given.
    pub run_dir: String,
    /// The parent session.
    pub session: u64,
    /// The checkpoint's sequence number in the parent's store.
    pub sequence: u64,
    /// Its committed tick.
    pub tick: u64,
    /// Its container's SHA-256.
    pub sha256: String,
}

/// What a branch inherited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Inherited {
    /// Commands admitted before the fork, by the parent or its ancestors.
    pub admitted: u64,
    /// Outcomes in the inherited stream.
    pub outcomes: u64,
    /// Of the inherited commands, those still to apply.
    pub pending: usize,
}

/// The founding checkpoint's chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FoundingChunks {
    /// Chunks whose bytes were written.
    pub written: usize,
    /// Chunks linked from the parent's store.
    pub linked: usize,
}

/// The session a branch named `name` of the checkpoint whose container's
/// SHA-256 is `checkpoint` runs: the first eight bytes, little-endian, of
/// the SHA-256 of `frankenlife.branch-session.v1`, a zero byte, the
/// checkpoint's digest and the name.
#[must_use]
pub fn branch_session(checkpoint: Sha256Digest, name: &str) -> SessionId {
    let mut input = b"frankenlife.branch-session.v1\0".to_vec();
    input.extend_from_slice(checkpoint.as_bytes());
    input.extend_from_slice(name.as_bytes());
    let digest = Sha256Digest::of(&input);
    let mut first = [0_u8; 8];
    first.copy_from_slice(&digest.as_bytes()[..8]);
    SessionId::new(u64::from_le_bytes(first))
}

fn conflict(detail: String) -> CoreError {
    CoreError::StateConflict {
        operation: "branch run",
        detail,
    }
}

/// Branch the run in `parent` into the new (or empty) run directory
/// `run_dir` (see the [module documentation](self)), returning the
/// [`Record::Branch`] that describes it.
///
/// # Errors
/// [`CoreError::StateConflict`] when `parent` is not a run directory, holds
/// no checkpoint that restores (at `options.at`, if given), or is held by
/// another process; when `run_dir` already holds a run or other files; for
/// a name that is empty or too long, or whose session is already in the
/// lineage; or for a command that is not admitted (one due before the
/// checkpoint's tick); the store's and catalog's errors.
pub fn branch(parent: &Path, run_dir: &Path, options: &BranchOptions) -> Result<Record, CoreError> {
    let name = match &options.name {
        Some(name) => name.clone(),
        None => run_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    if name.is_empty() || name.len() > BRANCH_NAME_MAX_BYTES {
        return Err(conflict(format!(
            "a branch name is 1 to {BRANCH_NAME_MAX_BYTES} bytes, not {}",
            name.len()
        )));
    }
    let config = read_config(parent)?;
    let revision = config.config_hash();
    let (parent_session, _) = run_identity(parent, revision)?;
    let profile = NumericalProfile::reference_f32()?;
    let donor = ChunkStore::open(parent.join(STORE_DIR), options.durability)?;
    let recovered = donor.latest_valid(|_, bytes| {
        let restored = restorable(bytes, profile, revision, parent_session)?;
        let tick = restored.world.tick().get();
        if options.at.is_some_and(|at| at != tick) {
            return Err(conflict(format!("it holds tick {tick}")));
        }
        Ok((restored, Sha256Digest::of(bytes)))
    })?;
    let Some((sequence, (restored, digest))) = recovered.latest else {
        return Err(conflict(format!(
            "{} holds no checkpoint that restores{}",
            parent.display(),
            options
                .at
                .map_or_else(String::new, |at| format!(" at tick {at}"))
        )));
    };
    let world = restored.world;
    let tick = world.tick();
    let session = branch_session(digest, &name);
    let limits = SessionConfig::from_config(session, &config)?;
    let mut book = restored.book.branch(session, tick, digest)?;
    book.set_durability(limits.durability);
    let inherited = Inherited {
        admitted: book.lineage().last().map_or(0, |fork| fork.admitted),
        outcomes: book.outcome_cursor(),
        pending: book.unfinished(),
    };
    let first = book
        .last_command()
        .map_or(1, |id| id.get().saturating_add(1));
    let grid = *world.field().grid();
    let mut commands = Vec::new();
    for command in scheduled(session, first, &options.injections) {
        let admission = book.admit(command, tick, &grid, false);
        if !matches!(admission, Admission::Accepted { retry: false, .. }) {
            return Err(conflict(format!(
                "{} was not admitted to the branch at tick {}: {admission:?}",
                command.id,
                tick.get()
            )));
        }
        commands.push(command.id.get());
    }
    let bytes = checkpoint(&world, &book)?;

    create(run_dir, &config)?;
    let history = match limits.durability {
        Durability::WriteAhead => book.history(),
        Durability::Volatile => JournalContents::default(),
    };
    let origin = BranchOrigin {
        name: name.clone(),
        parent_run: parent.display().to_string(),
        parent_sequence: sequence,
        created_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs()),
    };
    let catalog = run_dir.join(CATALOG_FILE);
    let lineage = book.lineage().to_vec();
    on_plain_thread("record the branch", || {
        CommandJournal::open(&catalog)?.open_branch(session, revision, &lineage, &origin, &history)
    })?;
    let persistence = Persistence {
        checkpoint_every: 0,
        durability: options.durability,
        ..Persistence::new(run_dir, &config)
    };
    let mut checkpoints = Checkpoints::open(&persistence, &config, session)?;
    let taken = SessionCheckpoint {
        boundary: Boundary {
            tick,
            epoch: world.epoch(),
        },
        bytes,
    };
    let (published, linked) = checkpoints.publish_from(&taken, Some(&donor))?;
    let Record::Checkpoint {
        sequence: founding,
        sha256,
        bytes,
        chunks: [written, _],
        ..
    } = published
    else {
        return Err(conflict("the founding checkpoint was not published".into()));
    };
    Ok(Record::Branch {
        schema: RUN_SCHEMA,
        name,
        session: session.get(),
        parent: BranchParent {
            run_dir: parent.display().to_string(),
            session: parent_session.get(),
            sequence,
            tick: tick.get(),
            sha256: digest.to_hex(),
        },
        lineage: lineage
            .iter()
            .map(|fork| ForkRecord {
                parent: fork.parent.get(),
                admitted: fork.admitted,
                tick: fork.tick.get(),
                sha256: fork.checkpoint.to_hex(),
            })
            .collect(),
        inherited,
        commands,
        checkpoint: CheckpointAt {
            sequence: founding,
            tick: tick.get(),
        },
        sha256,
        bytes,
        chunks: FoundingChunks {
            written: written.saturating_sub(linked),
            linked,
        },
        digest: world.digest().to_hex(),
    })
}

/// The tick a branch's lineage forks at, for messages.
pub(crate) fn fork_tick(lineage: &[flife_runtime::Fork]) -> Option<Tick> {
    lineage.last().map(|fork| fork.tick)
}
