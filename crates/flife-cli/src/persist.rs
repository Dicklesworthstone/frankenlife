//! Run directories: checkpointed, resumable runs (FL-037; plan §12.3,
//! §16.3–§16.7, Appendix C.13 and C.14).
//!
//! A run directory holds one headless run:
//! - `config.toml`: the configuration's canonical TOML, whose SHA-256 is
//!   the run's configuration hash;
//! - `catalog.db`: FL-036's run catalog and command journal, recording the
//!   run under that hash;
//! - `store/`: FL-035's chunk store of the run's checkpoints.
//!
//! [`crate::run_with`] with a [`Persistence`] creates one, opens a
//! `write-ahead-before-apply` session through the command journal (a
//! `volatile-in-memory` one in memory), and publishes a checkpoint at every
//! multiple of `checkpoint_every` (the founding tick included) and at the
//! end, each indexed in the catalog, keeping the configuration's
//! `retain_recent_checkpoints` newest. [`resume`] recovers the newest
//! checkpoint that loads and restores to this configuration (or the founding
//! world), replays the journal past it, and continues to the requested tick:
//! a resumed run reaches exactly the state of the uninterrupted one. A run
//! that reaches its tick suspends its session rather than closing it, so a
//! command still pending stays pending, in the final checkpoint and the
//! journal alike, and applies when the run is resumed past its tick.
//!
//! A run directory made by [`crate::branch()`] (FL-039) is a branch: its
//! catalog records a session other than [`RUN_SESSION`] and the fork it
//! descends from, and it resumes only from a checkpoint of that session.
//! It has no founding world of its own to fall back to.
//!
//! Checkpoints are `pause-and-stream`: the client takes no step while one is
//! taken and published, so the committed world cannot move under it. The
//! container is encoded whole in memory first; admitting that copy against
//! a memory budget is FL-038's. The catalog's index is written while the
//! journal is idle: a checkpoint is taken only once every record has its
//! receipt.

use std::fs::{self, File};
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Instant;

use flife_backend_metal::ControllerChoice;
use flife_core::canonical::Sha256Digest;
use flife_core::ids::{CommandId, SessionId};
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_engine::{Intervention, NumericalProfile, World};
use flife_runtime::{
    Command, CommandBook, Durability, Fork, Journal, JournalWriter, Restored, SessionCheckpoint,
    SessionConfig, SessionHandle, SessionOwner, Target, open_journaled, open_restored, restore,
};
use flife_store::{ChunkStore, CommandJournal, IndexedCheckpoint};

use crate::run::{
    CheckpointAt, ControllerSummary, Finished, Plan, RUN_SCHEMA, RUN_SESSION, Record,
    SkippedCheckpoint, configure, drive, finish, found, start,
};

pub use flife_store::Durability as StoreDurability;

/// The run's configuration, as canonical TOML.
pub const CONFIG_FILE: &str = "config.toml";
/// The run catalog and command journal.
pub const CATALOG_FILE: &str = "catalog.db";
/// The chunk store of the run's checkpoints.
pub const STORE_DIR: &str = "store";

/// Where and how a run keeps itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Persistence {
    /// The run directory.
    pub run_dir: PathBuf,
    /// Publish a checkpoint at every multiple of this tick; zero for only
    /// the last one.
    pub checkpoint_every: u64,
    /// How checkpoints are published: `power-loss` flushes before each
    /// commit point, `process-crash` relies on atomic renames.
    pub durability: StoreDurability,
}

impl Persistence {
    /// `config`'s persistence in `run_dir`: a checkpoint every
    /// `[persistence] checkpoint_interval_simulation_seconds`, rounded up
    /// to whole ticks, flushed against power loss.
    #[must_use]
    pub fn new(run_dir: impl Into<PathBuf>, config: &ValidatedConfig) -> Self {
        Self {
            run_dir: run_dir.into(),
            checkpoint_every: config.report().checkpoint_interval_ticks.get(),
            durability: StoreDurability::PowerLoss,
        }
    }
}

/// A command scheduled from the command line: `amount` energy into field
/// cell `cell` at committed tick `tick` (FL-014's `InjectEnergy`). Written
/// `TICK:X,Y:AMOUNT`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Injection {
    /// The committed tick whose boundary it applies at.
    pub tick: u64,
    /// The field cell, `[x, y]`.
    pub cell: [u32; 2],
    /// The energy added.
    pub amount: f64,
}

impl FromStr for Injection {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let bad = || format!("`{text}` is not TICK:X,Y:AMOUNT");
        let mut parts = text.split(':');
        let (Some(tick), Some(cell), Some(amount), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(bad());
        };
        let (x, y) = cell.split_once(',').ok_or_else(bad)?;
        Ok(Self {
            tick: tick.trim().parse().map_err(|_| bad())?,
            cell: [
                x.trim().parse().map_err(|_| bad())?,
                y.trim().parse().map_err(|_| bad())?,
            ],
            amount: amount.trim().parse().map_err(|_| bad())?,
        })
    }
}

/// The run session's commands for `injections`: identities 1, 2, … in
/// order, each for exactly its tick.
#[must_use]
pub fn commands(injections: &[Injection]) -> Vec<Command> {
    scheduled(RUN_SESSION, 1, injections)
}

/// `session`'s commands for `injections`: identities `first`, `first + 1`,
/// … in order, each for exactly its tick.
pub(crate) fn scheduled(session: SessionId, first: u64, injections: &[Injection]) -> Vec<Command> {
    injections
        .iter()
        .zip(first..)
        .map(|(injection, id)| Command {
            session,
            id: CommandId::new(id),
            target: Target::At {
                tick: Tick::new(injection.tick),
                retarget: false,
            },
            expected_revision: None,
            intervention: Intervention::InjectEnergy {
                cell: injection.cell,
                amount: injection.amount,
            },
        })
        .collect()
}

fn storage(operation: &'static str, path: &Path, error: &std::io::Error) -> CoreError {
    CoreError::StorageFailed {
        operation,
        path: path.display().to_string(),
        reason: error.to_string(),
    }
}

/// Make `run_dir` a new run directory for `config`: create it, or take an
/// empty one, and write `config.toml` durably (written, flushed, renamed
/// into place, and the directory flushed).
///
/// # Errors
/// [`CoreError::StateConflict`] when `run_dir` already holds a run or
/// other files; [`CoreError::StorageFailed`] when the host refuses an
/// operation.
pub fn create(run_dir: &Path, config: &ValidatedConfig) -> Result<(), CoreError> {
    if run_dir.join(CONFIG_FILE).exists() {
        return Err(CoreError::StateConflict {
            operation: "create run directory",
            detail: format!(
                "{} already holds a run: continue it with flife resume",
                run_dir.display()
            ),
        });
    }
    match fs::read_dir(run_dir) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Err(CoreError::StateConflict {
                    operation: "create run directory",
                    detail: format!("{} is not empty", run_dir.display()),
                });
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(run_dir)
                .map_err(|error| storage("create run directory", run_dir, &error))?;
        }
        Err(error) => return Err(storage("read run directory", run_dir, &error)),
    }
    let temporary = run_dir.join(format!("{CONFIG_FILE}.tmp"));
    let path = run_dir.join(CONFIG_FILE);
    let mut file = File::create(&temporary)
        .map_err(|error| storage("write run configuration", &temporary, &error))?;
    file.write_all(config.canonical_toml().as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| storage("write run configuration", &temporary, &error))?;
    drop(file);
    fs::rename(&temporary, &path)
        .map_err(|error| storage("write run configuration", &path, &error))?;
    File::open(run_dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| storage("flush run directory", run_dir, &error))
}

/// The configuration `run_dir` holds.
///
/// # Errors
/// [`CoreError::StateConflict`] when `run_dir` is not a run directory;
/// [`CoreError::StorageFailed`] when its configuration cannot be read; the
/// configuration's validation errors.
pub fn read_config(run_dir: &Path) -> Result<ValidatedConfig, CoreError> {
    let path = run_dir.join(CONFIG_FILE);
    if !path.is_file() || !run_dir.join(CATALOG_FILE).is_file() {
        return Err(CoreError::StateConflict {
            operation: "resume run",
            detail: format!(
                "{} is not a run directory (it needs {CONFIG_FILE} and {CATALOG_FILE})",
                run_dir.display()
            ),
        });
    }
    let text = fs::read_to_string(&path)
        .map_err(|error| storage("read run configuration", &path, &error))?;
    ValidatedConfig::from_toml(&text)
}

/// Run `work` on a plain thread and wait for it: FrankenSQLite's calls must
/// not run on an Asupersync runtime's thread (`block_on_inline`).
pub(crate) fn on_plain_thread<T: Send>(
    operation: &'static str,
    work: impl FnOnce() -> Result<T, CoreError> + Send,
) -> Result<T, CoreError> {
    std::thread::scope(|scope| match scope.spawn(work).join() {
        Ok(result) => result,
        Err(_) => Err(CoreError::StorageFailed {
            operation,
            path: "a catalog thread".into(),
            reason: "the thread panicked".into(),
        }),
    })
}

/// Open the run's session on committed `world` and its `book`: through the
/// command journal in `run_dir` for `write-ahead-before-apply` (replaying
/// what it holds past the book), in memory otherwise. Either way the
/// catalog records the run under the configuration hash, and refuses one
/// recorded under another.
///
/// # Errors
/// The catalog's and the session's opening errors.
pub fn open_session(
    world: World,
    book: CommandBook,
    limits: SessionConfig,
    run_dir: &Path,
) -> Result<(SessionOwner, SessionHandle), CoreError> {
    let catalog = run_dir.join(CATALOG_FILE);
    match limits.durability {
        Durability::WriteAhead => {
            let (writer, contents) = JournalWriter::spawn(
                move || Ok(Box::new(CommandJournal::open(&catalog)?) as Box<dyn Journal>),
                limits.session,
                limits.revision,
            )?;
            open_journaled(world, book, limits, writer, &contents)
        }
        Durability::Volatile => {
            on_plain_thread("record the run", || {
                CommandJournal::open(&catalog)?
                    .open_run(limits.session, limits.revision)
                    .map(|_| ())
            })?;
            open_restored(world, book, limits)
        }
    }
}

/// Restore `bytes` if they hold a world of the configuration `revision`
/// names and the commands of `session`: what makes a stored checkpoint one
/// this run can resume from.
pub(crate) fn restorable(
    bytes: &[u8],
    profile: NumericalProfile,
    revision: Sha256Digest,
    session: SessionId,
) -> Result<Restored, CoreError> {
    let restored = restore(bytes, profile)?;
    if restored.world.config().config_hash() != revision {
        return Err(CoreError::StateConflict {
            operation: "resume run",
            detail: "the checkpoint holds another configuration's world".into(),
        });
    }
    if restored.book.session() != session {
        return Err(CoreError::StateConflict {
            operation: "resume run",
            detail: format!(
                "the checkpoint holds the commands of {}, not {session}",
                restored.book.session()
            ),
        });
    }
    Ok(restored)
}

/// The session the run in `run_dir` records, running configuration
/// `revision`, and its lineage (empty unless it is a branch).
///
/// # Errors
/// [`CoreError::StateConflict`] when the catalog records no run or more
/// than one, or the run under another configuration; the catalog's
/// errors.
pub(crate) fn run_identity(
    run_dir: &Path,
    revision: Sha256Digest,
) -> Result<(SessionId, Vec<Fork>), CoreError> {
    let catalog = run_dir.join(CATALOG_FILE);
    on_plain_thread("read the run catalog", || {
        let mut journal = CommandJournal::open(&catalog)?;
        let runs = journal.runs()?;
        let [(session, _)] = runs.as_slice() else {
            return Err(CoreError::StateConflict {
                operation: "resume run",
                detail: format!(
                    "{} records {} runs; a run directory records one",
                    catalog.display(),
                    runs.len()
                ),
            });
        };
        journal.open_run(*session, revision)?;
        Ok((*session, journal.lineage()?))
    })
}

/// The run's checkpoints: its chunk store, and the catalog that indexes
/// them.
pub(crate) struct Checkpoints {
    store: ChunkStore,
    catalog: PathBuf,
    session: SessionId,
    revision: Sha256Digest,
    profile: NumericalProfile,
    every: u64,
    keep: usize,
    last: Option<CheckpointAt>,
}

impl Checkpoints {
    /// Open the run directory's chunk store under `persistence`'s profile,
    /// for `session`'s run of `config`.
    pub(crate) fn open(
        persistence: &Persistence,
        config: &ValidatedConfig,
        session: SessionId,
    ) -> Result<Self, CoreError> {
        Ok(Self {
            store: ChunkStore::open(persistence.run_dir.join(STORE_DIR), persistence.durability)?,
            catalog: persistence.run_dir.join(CATALOG_FILE),
            session,
            revision: config.config_hash(),
            profile: NumericalProfile::reference_f32()?,
            every: persistence.checkpoint_every,
            keep: usize::try_from(config.config().persistence.retain_recent_checkpoints)
                .unwrap_or(usize::MAX),
            last: None,
        })
    }

    /// The newest stored checkpoint this run can resume from, and those
    /// passed over, newest first.
    fn recover(&mut self) -> Result<(Option<Restored>, Vec<SkippedCheckpoint>), CoreError> {
        let (profile, revision, session) = (self.profile, self.revision, self.session);
        let recovered = self
            .store
            .latest_valid(|_, bytes| restorable(bytes, profile, revision, session))?;
        let skipped = recovered
            .skipped
            .into_iter()
            .map(|skipped| SkippedCheckpoint {
                sequence: skipped.sequence,
                kind: skipped.kind,
                reason: skipped.reason,
            })
            .collect();
        let restored = recovered.latest.map(|(sequence, restored)| {
            self.last = Some(CheckpointAt {
                sequence,
                tick: restored.world.tick().get(),
            });
            restored
        });
        Ok((restored, skipped))
    }

    /// Whether a checkpoint is due at committed tick `tick`.
    pub(crate) fn due(&self, tick: u64) -> bool {
        self.every > 0 && tick.is_multiple_of(self.every)
    }

    /// The newest checkpoint published, or the one the run started from.
    pub(crate) const fn last(&self) -> Option<CheckpointAt> {
        self.last
    }

    /// The tick of [`Checkpoints::last`].
    pub(crate) fn last_tick(&self) -> Option<u64> {
        self.last.map(|last| last.tick)
    }

    /// Publish `taken` to the chunk store, index it in the catalog, then
    /// keep the newest `retain_recent_checkpoints` this run can resume
    /// from.
    pub(crate) fn publish(&mut self, taken: &SessionCheckpoint) -> Result<Record, CoreError> {
        self.publish_from(taken, None).map(|(record, _)| record)
    }

    /// [`Checkpoints::publish`], linking the chunks `donor` holds intact
    /// rather than writing them. Returns the record and how many chunks
    /// were linked.
    pub(crate) fn publish_from(
        &mut self,
        taken: &SessionCheckpoint,
        donor: Option<&ChunkStore>,
    ) -> Result<(Record, usize), CoreError> {
        let publication = match donor {
            Some(donor) => self.store.publish_sharing(&taken.bytes, donor)?,
            None => self.store.publish(&taken.bytes)?,
        };
        let digest = Sha256Digest::of(&taken.bytes);
        let indexed = IndexedCheckpoint {
            sequence: publication.sequence,
            tick: taken.boundary.tick,
            epoch: taken.boundary.epoch,
            digest,
        };
        let (catalog, session, revision) = (&self.catalog, self.session, self.revision);
        on_plain_thread("index checkpoint", move || {
            let mut journal = CommandJournal::open(catalog)?;
            journal.open_run(session, revision)?;
            journal.index_checkpoint(indexed)
        })?;
        let at = CheckpointAt {
            sequence: publication.sequence,
            tick: taken.boundary.tick.get(),
        };
        self.last = Some(at);
        let profile = self.profile;
        let retention = self.store.retain(self.keep, |_, bytes| {
            restorable(bytes, profile, revision, session).map(|_| ())
        })?;
        let record = Record::Checkpoint {
            schema: RUN_SCHEMA,
            tick: at.tick,
            epoch: taken.boundary.epoch.get(),
            sequence: at.sequence,
            sha256: digest.to_hex(),
            bytes: u64::try_from(taken.bytes.len()).unwrap_or(u64::MAX),
            chunks: [publication.written, publication.shared],
            retired: retention.removed,
        };
        Ok((record, publication.linked))
    }
}

/// What to resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeOptions {
    /// The committed tick to reach.
    pub until: u64,
    /// Emit a progress record at every multiple of this tick; zero for
    /// none.
    pub every: u64,
    /// Threads for the organism phases. No result depends on it.
    pub cpu_workers: Option<NonZeroUsize>,
    /// Where due controllers are evaluated. No result depends on it.
    pub controllers: ControllerChoice,
    /// Publish a checkpoint at every multiple of this tick and at the end;
    /// `None` for the configuration's interval.
    pub checkpoint_every: Option<u64>,
    /// How checkpoints are published.
    pub durability: StoreDurability,
}

/// Resume the run in `run_dir` to committed tick `options.until`, passing
/// every record to `emit` in order: the start, a [`Record::Resume`] naming
/// the checkpoint recovered (the newest that loads and restores to the
/// run's configuration; the founding world if none does) and those passed
/// over, then progress and checkpoint records, then the end. A
/// `write-ahead-before-apply` run first replays every journaled outcome
/// past the checkpoint, and its pending commands apply at their ticks.
///
/// The tally covers the ticks this call publishes. A branch resumes the
/// same way, as its own session, from its own checkpoints.
///
/// # Errors
/// [`CoreError::StateConflict`] when `run_dir` is not a run directory, its
/// catalog records the run under another configuration, the journal
/// disagrees with the checkpoint, the run is already past `options.until`,
/// or it is a branch none of whose checkpoints restores; the store's,
/// catalog's and session's errors.
pub fn resume(
    run_dir: &Path,
    options: &ResumeOptions,
    emit: impl FnMut(&Record) + Send + 'static,
) -> Result<Finished, CoreError> {
    let config = read_config(run_dir)?;
    let (session, lineage) = run_identity(run_dir, config.config_hash())?;
    let limits = SessionConfig::from_config(session, &config)?;
    let mut persistence = Persistence::new(run_dir, &config);
    if let Some(every) = options.checkpoint_every {
        persistence.checkpoint_every = every;
    }
    persistence.durability = options.durability;
    let mut checkpoints = Checkpoints::open(&persistence, &config, session)?;
    let (restored, skipped) = checkpoints.recover()?;
    let (mut world, book) = match restored {
        Some(restored) => (restored.world, restored.book),
        None if !lineage.is_empty() => {
            return Err(CoreError::StateConflict {
                operation: "resume run",
                detail: format!(
                    "{} is a branch from tick {}, and none of its checkpoints restores; a branch \
                     has no founding world to fall back to",
                    run_dir.display(),
                    crate::branch::fork_tick(&lineage).map_or(0, Tick::get)
                ),
            });
        }
        None => (
            found(&config)?,
            CommandBook::new(limits.session, limits.revision, limits.command_capacity),
        ),
    };
    configure(&mut world, options.cpu_workers, options.controllers)?;
    let cpu_workers = world.cpu_workers();
    let controllers = ControllerSummary::of(&world).backend;
    let from = checkpoints.last();
    let (owner, handle) = open_session(world, book, limits, run_dir)?;
    let clock = Instant::now();
    let plan = Plan {
        start: start(&config, options.until, cpu_workers, controllers),
        until: options.until,
        every: options.every,
        commands: Vec::new(),
        resumed: Some((from, skipped)),
    };
    let driven = drive(owner, handle, plan, Some(checkpoints), emit, clock)?;
    Ok(finish(driven, Some(run_dir), limits.durability, clock))
}

#[cfg(test)]
mod tests {
    use super::Injection;

    /// `TICK:X,Y:AMOUNT`, in that order, and nothing else.
    #[test]
    fn an_injection_is_tick_cell_and_amount() {
        assert_eq!(
            "66:9,2:3.5".parse::<Injection>(),
            Ok(Injection {
                tick: 66,
                cell: [9, 2],
                amount: 3.5,
            })
        );
        assert_eq!(
            " 3 : 5 , 6 : 2 ".parse::<Injection>(),
            Ok(Injection {
                tick: 3,
                cell: [5, 6],
                amount: 2.0,
            })
        );
        for bad in [
            "",
            "3:5,6",
            "3:5:2.0",
            "3:5,6:2.0:1",
            "x:5,6:2.0",
            "3:5,-6:2.0",
            "3:5,6:y",
        ] {
            assert!(bad.parse::<Injection>().is_err(), "{bad}");
        }
    }
}
