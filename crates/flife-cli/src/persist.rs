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
//! Checkpoints are taken in the configuration's `checkpoint_mode`
//! ([`crate::checkpointing`], FL-038): `pause-and-stream` encodes the
//! committed world straight into the chunk store a page at a time while the
//! client takes no step; `bounded-background` hands the pages to a writer
//! thread through a queue bounded by `checkpoint_retained_bytes` and steps
//! on once every page is queued. Either way the session takes no step while
//! the world is encoded, so the committed world cannot move under it, and a
//! checkpoint is taken only once every journal record has its receipt.
//! Retention keeps the newest checkpoints the store verifies (seal, lengths
//! and hashes, read a buffer at a time); resuming restores them.

use std::fs::{self, File};
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;

use asupersync::Cx;

use flife_backend_metal::ControllerChoice;
use flife_core::canonical::Sha256Digest;
use flife_core::checkpoint::{CheckpointManifest, ChunkSink as _};
use flife_core::ids::{CommandId, SessionId};
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_engine::{Disposition, Intervention, MATERIAL_CHANNELS, NumericalProfile, World};
use flife_runtime::{
    Boundary, Command, CommandBook, Durability, Fork, Journal, JournalWriter, Restored,
    SessionCheckpoint, SessionConfig, SessionHandle, SessionOwner, Target, checkpoint_into,
    open_journaled, open_restored, restore,
};
use flife_store::{ChunkStore, CommandJournal, IndexedCheckpoint, Publication, Stream};

use crate::checkpointing::{
    CheckpointAdmission, CheckpointMode, Message, Producer, Queue, WRITER_STOPPED, admit,
};

use crate::run::{
    CheckpointAt, ControllerSummary, Finished, Plan, RUN_SCHEMA, RUN_SESSION, Record,
    SkippedCheckpoint, configure, drive, finish, found, start, tracker,
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
    /// How checkpoints are taken (FL-038). No result depends on it.
    pub mode: CheckpointMode,
    /// The most page bytes a `bounded-background` checkpoint retains.
    pub retained_bytes: u64,
}

impl Persistence {
    /// `config`'s persistence in `run_dir`: a checkpoint every
    /// `[persistence] checkpoint_interval_simulation_seconds`, rounded up
    /// to whole ticks, flushed against power loss, taken in
    /// `[persistence] checkpoint_mode` with `[execution]
    /// checkpoint_retained_bytes`.
    #[must_use]
    pub fn new(run_dir: impl Into<PathBuf>, config: &ValidatedConfig) -> Self {
        let config_values = config.config();
        Self {
            run_dir: run_dir.into(),
            checkpoint_every: config.report().checkpoint_interval_ticks.get(),
            durability: StoreDurability::PowerLoss,
            mode: CheckpointMode::from_name(&config_values.persistence.checkpoint_mode),
            retained_bytes: config_values.execution.checkpoint_retained_bytes,
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

/// A material brush scheduled from the command line: `amount` energy with
/// the signed composition `direction` into field cell `cell` at committed
/// tick `tick` (FL-022's `InjectMaterial`). Written `TICK:X,Y:AMOUNT:K=V,…`:
/// the direction's nonzero channels, each channel index `K` below
/// [`MATERIAL_CHANNELS`] named at most once, every other channel zero.
/// Admission checks the norm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Material {
    /// The committed tick whose boundary it applies at.
    pub tick: u64,
    /// The field cell, `[x, y]`.
    pub cell: [u32; 2],
    /// The energy to add.
    pub amount: f64,
    /// The composition per unit energy, channel by channel.
    pub direction: [f64; MATERIAL_CHANNELS],
}

impl FromStr for Material {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let bad = || {
            format!(
                "`{text}` is not TICK:X,Y:AMOUNT:K=V,… (channels K below {MATERIAL_CHANNELS}, \
                 each at most once)"
            )
        };
        let mut parts = text.split(':');
        let (Some(tick), Some(cell), Some(amount), Some(channels), None) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            return Err(bad());
        };
        let (x, y) = cell.split_once(',').ok_or_else(bad)?;
        let mut direction = [0.0; MATERIAL_CHANNELS];
        let mut named = [false; MATERIAL_CHANNELS];
        for entry in channels.split(',') {
            let (channel, value) = entry.split_once('=').ok_or_else(bad)?;
            let channel: usize = channel.trim().parse().map_err(|_| bad())?;
            let slot = named.get_mut(channel).ok_or_else(bad)?;
            if *slot {
                return Err(bad());
            }
            *slot = true;
            direction[channel] = value.trim().parse().map_err(|_| bad())?;
        }
        Ok(Self {
            tick: tick.trim().parse().map_err(|_| bad())?,
            cell: [
                x.trim().parse().map_err(|_| bad())?,
                y.trim().parse().map_err(|_| bad())?,
            ],
            amount: amount.trim().parse().map_err(|_| bad())?,
            direction,
        })
    }
}

/// A removal scheduled from the command line: every organism within
/// `radius` world units of `center` at committed tick `tick` dies, its
/// energy treated by `disposition` (FL-022's `RemoveOrganisms`). Written
/// `TICK:X,Y:RADIUS:DISPOSITION`, the disposition `remove`, `dissipate` or
/// `recycle`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Removal {
    /// The committed tick whose boundary it applies at.
    pub tick: u64,
    /// The disk's centre in world units, `[x, y]`.
    pub center: [f64; 2],
    /// The disk's radius in world units.
    pub radius: f64,
    /// What becomes of each removed organism's energy.
    pub disposition: Disposition,
}

impl FromStr for Removal {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let bad = || {
            format!("`{text}` is not TICK:X,Y:RADIUS:DISPOSITION (remove, dissipate or recycle)")
        };
        let mut parts = text.split(':');
        let (Some(tick), Some(center), Some(radius), Some(disposition), None) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            return Err(bad());
        };
        let (x, y) = center.split_once(',').ok_or_else(bad)?;
        Ok(Self {
            tick: tick.trim().parse().map_err(|_| bad())?,
            center: [
                x.trim().parse().map_err(|_| bad())?,
                y.trim().parse().map_err(|_| bad())?,
            ],
            radius: radius.trim().parse().map_err(|_| bad())?,
            disposition: Disposition::from_name(disposition.trim()).ok_or_else(bad)?,
        })
    }
}

/// A seeded introduction scheduled from the command line: `count` new
/// organisms, each endowed with `energy`, placed in field cell `cell` at the
/// end of committed tick `tick` (FL-022's `IntroduceOrganisms`). Written
/// `TICK:X,Y:COUNT:ENERGY`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Introduction {
    /// The committed tick whose boundary it applies at.
    pub tick: u64,
    /// The field cell, `[x, y]`.
    pub cell: [u32; 2],
    /// How many organisms.
    pub count: u32,
    /// Each one's endowment.
    pub energy: f64,
}

impl FromStr for Introduction {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        let bad = || format!("`{text}` is not TICK:X,Y:COUNT:ENERGY");
        let mut parts = text.split(':');
        let (Some(tick), Some(cell), Some(count), Some(energy), None) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            return Err(bad());
        };
        let (x, y) = cell.split_once(',').ok_or_else(bad)?;
        Ok(Self {
            tick: tick.trim().parse().map_err(|_| bad())?,
            cell: [
                x.trim().parse().map_err(|_| bad())?,
                y.trim().parse().map_err(|_| bad())?,
            ],
            count: count.trim().parse().map_err(|_| bad())?,
            energy: energy.trim().parse().map_err(|_| bad())?,
        })
    }
}

/// A command scheduled from the command line.
#[derive(Debug, Clone, Copy, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "a material's 32 components keep the command `Copy`; a command line schedules a handful"
)]
pub enum ScheduledCommand {
    /// Energy into a field cell.
    Inject(Injection),
    /// Energy with a composition into a field cell.
    Material(Material),
    /// Organisms out of a disk.
    Remove(Removal),
    /// New organisms into a field cell.
    Introduce(Introduction),
}

impl ScheduledCommand {
    /// The committed tick whose boundary it applies at.
    #[must_use]
    pub const fn tick(&self) -> u64 {
        match self {
            Self::Inject(injection) => injection.tick,
            Self::Material(material) => material.tick,
            Self::Remove(removal) => removal.tick,
            Self::Introduce(introduction) => introduction.tick,
        }
    }

    /// Its canonical intervention.
    #[must_use]
    pub const fn intervention(&self) -> Intervention {
        match *self {
            Self::Inject(Injection { cell, amount, .. }) => {
                Intervention::InjectEnergy { cell, amount }
            }
            Self::Material(Material {
                cell,
                amount,
                direction,
                ..
            }) => Intervention::InjectMaterial {
                cell,
                amount,
                direction,
            },
            Self::Remove(Removal {
                center,
                radius,
                disposition,
                ..
            }) => Intervention::RemoveOrganisms {
                center,
                radius,
                disposition,
            },
            Self::Introduce(Introduction {
                cell,
                count,
                energy,
                ..
            }) => Intervention::IntroduceOrganisms {
                cell,
                count,
                energy,
            },
        }
    }

    /// `injections`, then `materials`, then `removals`, then
    /// `introductions`: the command line's order of identities.
    #[must_use]
    pub fn listed(
        injections: &[Injection],
        materials: &[Material],
        removals: &[Removal],
        introductions: &[Introduction],
    ) -> Vec<Self> {
        injections
            .iter()
            .copied()
            .map(Self::Inject)
            .chain(materials.iter().copied().map(Self::Material))
            .chain(removals.iter().copied().map(Self::Remove))
            .chain(introductions.iter().copied().map(Self::Introduce))
            .collect()
    }
}

impl From<Injection> for ScheduledCommand {
    fn from(injection: Injection) -> Self {
        Self::Inject(injection)
    }
}

impl From<Material> for ScheduledCommand {
    fn from(material: Material) -> Self {
        Self::Material(material)
    }
}

impl From<Removal> for ScheduledCommand {
    fn from(removal: Removal) -> Self {
        Self::Remove(removal)
    }
}

impl From<Introduction> for ScheduledCommand {
    fn from(introduction: Introduction) -> Self {
        Self::Introduce(introduction)
    }
}

/// The run session's commands for `scheduled`: identities 1, 2, … in
/// order, each for exactly its tick.
#[must_use]
pub fn commands(scheduled: &[ScheduledCommand]) -> Vec<Command> {
    self::scheduled(RUN_SESSION, 1, scheduled)
}

/// `session`'s commands for `scheduled`: identities `first`, `first + 1`,
/// … in order, each for exactly its tick.
pub(crate) fn scheduled(
    session: SessionId,
    first: u64,
    scheduled: &[ScheduledCommand],
) -> Vec<Command> {
    scheduled
        .iter()
        .zip(first..)
        .map(|(command, id)| Command {
            session,
            id: CommandId::new(id),
            target: Target::At {
                tick: Tick::new(command.tick()),
                retarget: false,
            },
            expected_revision: None,
            intervention: command.intervention(),
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

/// The run's checkpoints: its chunk store, the catalog that indexes them,
/// and the mode they are taken in (FL-038).
pub(crate) struct Checkpoints {
    /// The store; with the writer while a background checkpoint is in
    /// flight.
    store: Option<ChunkStore>,
    filing: Filing,
    profile: NumericalProfile,
    every: u64,
    admission: CheckpointAdmission,
    last: Option<CheckpointAt>,
    in_flight: Option<InFlight>,
}

/// Where and how a published checkpoint is indexed and retained.
#[derive(Clone)]
struct Filing {
    catalog: PathBuf,
    session: SessionId,
    revision: Sha256Digest,
    keep: usize,
}

/// What a published checkpoint's record needs besides the publication.
struct Taken {
    boundary: Boundary,
    mode: CheckpointMode,
    retained_peak: u64,
    paused_seconds: f64,
}

/// A background checkpoint's writer, and what its record needs.
struct InFlight {
    writer: std::thread::JoinHandle<(ChunkStore, Result<Committed, CoreError>)>,
    queue: Arc<Queue>,
    paused_seconds: f64,
}

/// A checkpoint published and verified, not yet indexed.
struct Committed {
    boundary: Boundary,
    publication: Publication,
    digest: Sha256Digest,
    bytes: u64,
}

/// A checkpoint published, verified, indexed and retained.
struct Filed {
    committed: Committed,
    retired: Vec<u64>,
}

impl Filing {
    /// Index `committed` in the catalog. Called only while the session's
    /// journal is idle: every record sent has its receipt, and the client
    /// sends no other until this returns.
    fn index(&self, committed: &Committed) -> Result<(), CoreError> {
        let indexed = IndexedCheckpoint {
            sequence: committed.publication.sequence,
            tick: committed.boundary.tick,
            epoch: committed.boundary.epoch,
            digest: committed.digest,
        };
        let (catalog, session, revision) = (&self.catalog, self.session, self.revision);
        on_plain_thread("index checkpoint", move || {
            let mut journal = CommandJournal::open(catalog)?;
            journal.open_run(session, revision)?;
            journal.index_checkpoint(indexed)
        })
    }

    /// Index `committed`, then keep the newest `keep` checkpoints the store
    /// verifies.
    fn file(&self, store: &mut ChunkStore, committed: Committed) -> Result<Filed, CoreError> {
        self.index(&committed)?;
        Ok(Filed {
            committed,
            retired: store.retain_intact(self.keep)?.removed,
        })
    }
}

/// A streamed checkpoint's container size: its sealed prefix and payload.
fn container_bytes(sealed: &[u8]) -> Result<u64, CoreError> {
    let manifest = CheckpointManifest::parse(sealed)?;
    Ok(u64::try_from(sealed.len())
        .unwrap_or(u64::MAX)
        .saturating_add(manifest.payload_length()))
}

/// The background writer: store each chunk as its pages arrive, then
/// commit and verify. Indexing and retention wait for
/// [`Checkpoints::drain`], where the journal is idle. Returns the store
/// either way; on a failure it stops the queue, so the encoder's next call
/// refuses and every queued page is released.
fn write_background(
    mut store: ChunkStore,
    stream: Stream,
    queue: &Queue,
) -> (ChunkStore, Result<Committed, CoreError>) {
    let outcome = store_streamed(&mut store, stream, queue);
    if outcome.is_err() {
        queue.stop();
    }
    (store, outcome)
}

fn store_streamed(
    store: &mut ChunkStore,
    mut stream: Stream,
    queue: &Queue,
) -> Result<Committed, CoreError> {
    let (sealed, boundary) = loop {
        match queue.next() {
            None => {
                return Err(CoreError::Cancelled {
                    reason: "the checkpoint was abandoned before it was sealed".into(),
                });
            }
            Some(Message::Begin(index, family)) => stream.begin(index, family)?,
            Some(Message::Page(page)) => {
                let written = stream.page(&page);
                queue.written(page);
                written?;
            }
            Some(Message::End(length, digest)) => stream.end(length, digest)?,
            Some(Message::Seal(sealed, boundary)) => break (sealed, boundary),
        }
    };
    let publication = store.commit(stream, &sealed)?;
    let digest = store.verify(publication.sequence)?;
    Ok(Committed {
        boundary,
        publication,
        digest,
        bytes: container_bytes(&sealed)?,
    })
}

fn unavailable() -> CoreError {
    CoreError::StateConflict {
        operation: "checkpoint run",
        detail: "the chunk store was lost with a failed checkpoint writer".into(),
    }
}

impl Checkpoints {
    /// Open the run directory's chunk store under `persistence`'s profile,
    /// for `session`'s run of `config`, admitting its checkpoint mode.
    pub(crate) fn open(
        persistence: &Persistence,
        config: &ValidatedConfig,
        session: SessionId,
    ) -> Result<Self, CoreError> {
        let admission = admit(config, persistence.mode, persistence.retained_bytes)?;
        Ok(Self {
            store: Some(ChunkStore::open(
                persistence.run_dir.join(STORE_DIR),
                persistence.durability,
            )?),
            filing: Filing {
                catalog: persistence.run_dir.join(CATALOG_FILE),
                session,
                revision: config.config_hash(),
                keep: usize::try_from(config.config().persistence.retain_recent_checkpoints)
                    .unwrap_or(usize::MAX),
            },
            profile: NumericalProfile::reference_f32()?,
            every: persistence.checkpoint_every,
            admission,
            last: None,
            in_flight: None,
        })
    }

    /// The checkpoint admission the run's start record reports.
    pub(crate) const fn admission(&self) -> &CheckpointAdmission {
        &self.admission
    }

    fn store(&mut self) -> Result<&mut ChunkStore, CoreError> {
        self.store.as_mut().ok_or_else(unavailable)
    }

    /// The newest stored checkpoint this run can resume from, and those
    /// passed over, newest first.
    fn recover(&mut self) -> Result<(Option<Restored>, Vec<SkippedCheckpoint>), CoreError> {
        let (profile, revision, session) =
            (self.profile, self.filing.revision, self.filing.session);
        let recovered = self
            .store()?
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

    /// The newest checkpoint taken, or the one the run started from.
    pub(crate) const fn last(&self) -> Option<CheckpointAt> {
        self.last
    }

    /// The tick of [`Checkpoints::last`].
    pub(crate) fn last_tick(&self) -> Option<u64> {
        self.last.map(|last| last.tick)
    }

    /// Take a checkpoint of `handle`'s committed boundary in the admitted
    /// mode. Returns the records of checkpoints published meanwhile: a
    /// background checkpoint still in flight first (this waits for it),
    /// then, for `pause-and-stream`, this one. A background checkpoint's
    /// record follows when it is drained.
    pub(crate) async fn take(
        &mut self,
        handle: &SessionHandle,
        cx: &Cx,
    ) -> Result<Vec<Record>, CoreError> {
        let mut records = self.drain(handle, cx).await?;
        let started = Instant::now();
        match self.admission.mode() {
            CheckpointMode::PauseAndStream => {
                let stream = self.store()?.stream()?;
                let (stream, sealed, boundary) = handle
                    .checkpoint_with(cx, move |world, book, boundary| {
                        let mut stream = stream;
                        let sealed = checkpoint_into(world, book, &mut stream);
                        (stream, sealed, boundary)
                    })
                    .await?;
                let sealed = sealed?;
                let store = self.store()?;
                let publication = store.commit(stream, &sealed)?;
                let digest = store.verify(publication.sequence)?;
                let committed = Committed {
                    boundary,
                    publication,
                    digest,
                    bytes: container_bytes(&sealed)?,
                };
                // The session answered the take once every journal record
                // had its receipt, and the client sends none until this
                // returns: the journal is idle.
                let filing = self.filing.clone();
                let filed = filing.file(self.store()?, committed)?;
                let taken = Taken {
                    boundary,
                    mode: CheckpointMode::PauseAndStream,
                    retained_peak: 0,
                    paused_seconds: started.elapsed().as_secs_f64(),
                };
                records.push(self.record(&filed, &taken));
            }
            CheckpointMode::BoundedBackground => {
                let mut store = self.store.take().ok_or_else(unavailable)?;
                let stream = match store.stream() {
                    Ok(stream) => stream,
                    Err(error) => {
                        self.store = Some(store);
                        return Err(error);
                    }
                };
                let sequence = stream.sequence();
                let queue = Queue::new(self.admission.retained_bytes);
                let producer = Producer::new(Arc::clone(&queue));
                let writer_queue = Arc::clone(&queue);
                let writer = std::thread::Builder::new()
                    .name("flife-checkpoint-writer".into())
                    .spawn(move || write_background(store, stream, &writer_queue))
                    .map_err(|error| CoreError::StorageFailed {
                        operation: "start checkpoint writer",
                        path: self.filing.catalog.display().to_string(),
                        reason: error.to_string(),
                    })?;
                let queued = handle
                    .checkpoint_with(cx, move |world, book, boundary| {
                        let mut producer = producer;
                        checkpoint_into(world, book, &mut producer)
                            .and_then(|sealed| producer.seal(sealed, boundary))
                            .map(|()| boundary)
                    })
                    .await;
                self.in_flight = Some(InFlight {
                    writer,
                    queue,
                    paused_seconds: started.elapsed().as_secs_f64(),
                });
                match queued.and_then(|queued| queued) {
                    Ok(boundary) => {
                        self.last = Some(CheckpointAt {
                            sequence,
                            tick: boundary.tick.get(),
                        });
                    }
                    Err(error) => {
                        // Join the writer. When it stopped first, its own
                        // error says more than the encoder's refusal.
                        let writer = self.join().map(|_| ());
                        let refused = matches!(
                            &error,
                            CoreError::Cancelled { reason } if reason == WRITER_STOPPED
                        );
                        return Err(match writer {
                            Err(writer) if refused => writer,
                            _ => error,
                        });
                    }
                }
            }
        }
        Ok(records)
    }

    /// Wait for a background checkpoint in flight, if any; index it while
    /// `handle`'s session holds its journal idle, retain, and return its
    /// record. The run takes no step meanwhile, and that wait counts in the
    /// record's `paused_seconds`.
    pub(crate) async fn drain(
        &mut self,
        handle: &SessionHandle,
        cx: &Cx,
    ) -> Result<Vec<Record>, CoreError> {
        let started = Instant::now();
        let Some((committed, queue, paused_seconds)) = self.join()? else {
            return Ok(Vec::new());
        };
        // Index on the session's task, which answers once every journal
        // record has its receipt and takes no step until this returns: the
        // writer ran while the run stepped and its records were written.
        let filing = self.filing.clone();
        let committed = handle
            .checkpoint_with(cx, move |_, _, _| {
                filing.index(&committed).map(|()| committed)
            })
            .await??;
        let keep = self.filing.keep;
        let filed = Filed {
            retired: self.store()?.retain_intact(keep)?.removed,
            committed,
        };
        let taken = Taken {
            boundary: filed.committed.boundary,
            mode: CheckpointMode::BoundedBackground,
            retained_peak: queue.peak(),
            paused_seconds: paused_seconds + started.elapsed().as_secs_f64(),
        };
        Ok(vec![self.record(&filed, &taken)])
    }

    /// Join the writer of a background checkpoint in flight, if any, and
    /// take its store back: the checkpoint it committed and verified, its
    /// queue and how long the session paused for it.
    fn join(&mut self) -> Result<Option<(Committed, Arc<Queue>, f64)>, CoreError> {
        let Some(InFlight {
            writer,
            queue,
            paused_seconds,
        }) = self.in_flight.take()
        else {
            return Ok(None);
        };
        let (store, committed) = writer.join().map_err(|_| CoreError::StorageFailed {
            operation: "write checkpoint",
            path: self.filing.catalog.display().to_string(),
            reason: "the checkpoint writer panicked".into(),
        })?;
        self.store = Some(store);
        Ok(Some((committed?, queue, paused_seconds)))
    }

    fn record(&mut self, filed: &Filed, taken: &Taken) -> Record {
        let at = CheckpointAt {
            sequence: filed.committed.publication.sequence,
            tick: taken.boundary.tick.get(),
        };
        if self.last.is_none_or(|last| last.sequence <= at.sequence) {
            self.last = Some(at);
        }
        Record::Checkpoint {
            schema: RUN_SCHEMA,
            tick: at.tick,
            epoch: taken.boundary.epoch.get(),
            sequence: at.sequence,
            sha256: filed.committed.digest.to_hex(),
            bytes: filed.committed.bytes,
            chunks: [
                filed.committed.publication.written,
                filed.committed.publication.shared,
            ],
            retired: filed.retired.clone(),
            mode: taken.mode.name(),
            retained_peak: taken.retained_peak,
            paused_seconds: taken.paused_seconds,
        }
    }

    /// Publish `taken`, a checkpoint encoded in memory (a branch's first),
    /// to the chunk store, linking the chunks `donor` holds intact rather
    /// than writing them; index it in the catalog, then keep the newest
    /// `retain_recent_checkpoints` the store verifies. Returns the record
    /// and how many chunks were linked.
    pub(crate) fn publish_from(
        &mut self,
        taken: &SessionCheckpoint,
        donor: Option<&ChunkStore>,
    ) -> Result<(Record, usize), CoreError> {
        if self.in_flight.is_some() {
            return Err(CoreError::StateConflict {
                operation: "publish checkpoint",
                detail: "a background checkpoint is in flight".into(),
            });
        }
        let started = Instant::now();
        let store = self.store()?;
        let publication = match donor {
            Some(donor) => store.publish_sharing(&taken.bytes, donor)?,
            None => store.publish(&taken.bytes)?,
        };
        let linked = publication.linked;
        let committed = Committed {
            boundary: taken.boundary,
            publication,
            digest: Sha256Digest::of(&taken.bytes),
            bytes: u64::try_from(taken.bytes.len()).unwrap_or(u64::MAX),
        };
        let filing = self.filing.clone();
        let filed = filing.file(self.store()?, committed)?;
        let record = self.record(
            &filed,
            &Taken {
                boundary: taken.boundary,
                mode: CheckpointMode::PauseAndStream,
                retained_peak: 0,
                paused_seconds: started.elapsed().as_secs_f64(),
            },
        );
        Ok((record, linked))
    }
}

impl Drop for Checkpoints {
    fn drop(&mut self) {
        // Never leave the writer detached: let it finish (or fail) and
        // join it. Its result was not asked for.
        if let Some(flight) = self.in_flight.take() {
            let _ignored = flight.writer.join();
        }
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
    /// How checkpoints are taken; `None` for the configuration's mode.
    pub checkpoint_mode: Option<CheckpointMode>,
    /// The `bounded-background` page budget; `None` for the
    /// configuration's `checkpoint_retained_bytes`.
    pub retained_bytes: Option<u64>,
    /// Emit a metrics record at every multiple of this committed tick and
    /// at the end; zero for none. Its lineage counts start where the
    /// resumed run does.
    pub metrics_every: u64,
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
    if let Some(mode) = options.checkpoint_mode {
        persistence.mode = mode;
    }
    if let Some(retained) = options.retained_bytes {
        persistence.retained_bytes = retained;
    }
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
    let metrics = tracker(&world, options.metrics_every)?;
    let (owner, handle) = open_session(world, book, limits, run_dir)?;
    let clock = Instant::now();
    let plan = Plan {
        start: start(
            &config,
            options.until,
            cpu_workers,
            controllers,
            options.metrics_every,
        ),
        until: options.until,
        every: options.every,
        commands: Vec::new(),
        resumed: Some((from, skipped)),
        metrics,
    };
    let driven = drive(owner, handle, plan, Some(checkpoints), emit, clock)?;
    Ok(finish(driven, Some(run_dir), limits.durability, clock))
}

#[cfg(test)]
mod tests {
    use super::{Injection, Introduction, Material, Removal, ScheduledCommand};
    use flife_engine::{Disposition, MATERIAL_CHANNELS};

    /// `TICK:X,Y:COUNT:ENERGY`, in that order, and nothing else.
    #[test]
    fn an_introduction_is_tick_cell_count_and_energy() {
        assert_eq!(
            "40:3,7:12:2.5".parse::<Introduction>(),
            Ok(Introduction {
                tick: 40,
                cell: [3, 7],
                count: 12,
                energy: 2.5,
            })
        );
        assert_eq!(
            " 1 : 0 , 31 : 1 : 1e-3 ".parse::<Introduction>(),
            Ok(Introduction {
                tick: 1,
                cell: [0, 31],
                count: 1,
                energy: 0.001,
            })
        );
        for bad in [
            "",
            "40:3,7:12",
            "40:3,7:12:2.5:1",
            "40:3:12:2.5",
            "40:3,7:-1:2.5",
            "40:3,7:1.5:2.5",
            "40:3,7:4294967296:2.5",
            "40:3,7:12:x",
            "-1:3,7:12:2.5",
        ] {
            assert!(bad.parse::<Introduction>().is_err(), "{bad}");
        }
    }

    /// `TICK:X,Y:AMOUNT:K=V,…`: named channels below the channel count,
    /// each once, the rest zero; nothing else.
    #[test]
    fn a_material_is_tick_cell_amount_and_named_channels() {
        let mut direction = [0.0; MATERIAL_CHANNELS];
        direction[0] = 0.6;
        direction[31] = -0.8;
        assert_eq!(
            "40:3,7:2.5:0=0.6,31=-0.8".parse::<Material>(),
            Ok(Material {
                tick: 40,
                cell: [3, 7],
                amount: 2.5,
                direction,
            })
        );
        let mut direction = [0.0; MATERIAL_CHANNELS];
        direction[4] = 1.0;
        assert_eq!(
            " 1 : 0 , 0 : 1 : 4 = 1 ".parse::<Material>(),
            Ok(Material {
                tick: 1,
                cell: [0, 0],
                amount: 1.0,
                direction,
            })
        );
        for bad in [
            "",
            "40:3,7:2.5",
            "40:3,7:2.5:",
            "40:3:2.5:0=1",
            "40:3,7:2.5:32=0.5",
            "40:3,7:2.5:1=0.5,1=0.2",
            "40:3,7:2.5:1",
            "40:3,7:2.5:x=0.5",
            "40:3,7:2.5:1=y",
            "40:3,7:2.5:1=0.5:9",
        ] {
            assert!(bad.parse::<Material>().is_err(), "{bad}");
        }
    }

    /// Injections, then materials, then removals, then introductions, each
    /// in the order given: the identities 1, 2, … the command line assigns.
    #[test]
    fn listed_commands_are_injections_then_materials_then_removals_then_introductions() {
        let injection = |tick| Injection {
            tick,
            cell: [1, 2],
            amount: 1.0,
        };
        let material = |tick| Material {
            tick,
            cell: [3, 4],
            amount: 2.0,
            direction: [0.0; MATERIAL_CHANNELS],
        };
        let removal = Removal {
            tick: 1,
            center: [5.0, 6.0],
            radius: 1.0,
            disposition: Disposition::Remove,
        };
        let introduction = |tick| Introduction {
            tick,
            cell: [7, 8],
            count: 3,
            energy: 1.0,
        };
        assert_eq!(
            ScheduledCommand::listed(
                &[injection(9), injection(8)],
                &[material(7), material(6)],
                &[removal],
                &[introduction(5), introduction(4)]
            ),
            vec![
                ScheduledCommand::Inject(injection(9)),
                ScheduledCommand::Inject(injection(8)),
                ScheduledCommand::Material(material(7)),
                ScheduledCommand::Material(material(6)),
                ScheduledCommand::Remove(removal),
                ScheduledCommand::Introduce(introduction(5)),
                ScheduledCommand::Introduce(introduction(4)),
            ]
        );
    }

    /// `TICK:X,Y:RADIUS:DISPOSITION`, in that order, and nothing else.
    #[test]
    fn a_removal_is_tick_centre_radius_and_disposition() {
        assert_eq!(
            "66:9.5,2:3.25:recycle".parse::<Removal>(),
            Ok(Removal {
                tick: 66,
                center: [9.5, 2.0],
                radius: 3.25,
                disposition: Disposition::Recycle,
            })
        );
        assert_eq!(
            " 3 : 5 , 6 : 2 : remove ".parse::<Removal>(),
            Ok(Removal {
                tick: 3,
                center: [5.0, 6.0],
                radius: 2.0,
                disposition: Disposition::Remove,
            })
        );
        for bad in [
            "",
            "3:5,6:2",
            "3:5:2:remove",
            "3:5,6:2:erase",
            "3:5,6:2:remove:1",
            "x:5,6:2:dissipate",
            "3:5,6:r:dissipate",
        ] {
            assert!(bad.parse::<Removal>().is_err(), "{bad}");
        }
    }

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
