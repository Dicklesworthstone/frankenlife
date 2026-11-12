//! A Studio session kept in a run directory (FL-024; plan §14.1's durable
//! checkpoint tick, §16.3).
//!
//! `flife-studio --run DIR` keeps its session the way `flife run
//! --run-dir` keeps a run, with the command line's own library
//! ([`flife_cli::persist`]), so `flife resume`, `flife branch` and
//! `flife inspect` take the directory:
//!
//! - [`Keeping::new`] refuses a directory that holds anything before a
//!   window opens, and admits the checkpoint's memory under the
//!   configuration's budget; [`Keeping::checkpoint_bytes`] is charged with
//!   the Studio's observers before the controller backend is installed.
//! - [`Keeping::open`] writes the configuration, opens the chunk store and
//!   the catalog, and opens the session through the command journal under
//!   `write-ahead-before-apply` (in memory, its run still recorded, under
//!   `volatile-in-memory`). Every brush the Studio applies is a journaled
//!   command, as a scheduled one is for `flife run`.
//! - The session thread publishes the founding checkpoint before it takes
//!   any intent, when the interval is not zero, and holds the session
//!   ([`SessionHandle::hold`]) at the next multiple of the interval after
//!   the committed tick. A checkpointer waits for the committed boundary
//!   to reach the hold, takes a checkpoint there, and moves the hold to
//!   the next multiple, so no run or step passes a multiple unkept: every
//!   checkpoint a running session takes is at a multiple, each multiple
//!   it reaches has one, as `flife run`'s do, and the world waits at a
//!   multiple only while its checkpoint is taken. Holding changes no
//!   result. A world stopped by a numerical fault is not checkpointed
//!   again.
//! - A stroke offered while the session is paused applies at the
//!   committed boundary, so the session thread first publishes a
//!   checkpoint of it (plan §2.2: "save the exact pre-intervention
//!   checkpoint"), from which `flife branch --at` makes the control: it
//!   inherits every command applied before the stroke and none after.
//!   Strokes at one boundary share it. A running session's stroke applies
//!   at a boundary still to come and takes none.
//! - Closing pauses the session, then releases the hold, so the
//!   checkpointer ends (a checkpoint it is taking is of the paused
//!   boundary). It publishes a checkpoint of the closing boundary unless
//!   the newest already holds it, the tick and every command the session
//!   holds, or the world is stopped by a fault; publishes a background
//!   checkpoint still in flight; and suspends the session rather than
//!   closing it: a command still pending stays pending, in the final
//!   checkpoint and the journal alike, and applies when the run is resumed
//!   past its tick.
//!
//! The checkpointer and the session thread share the chain of checkpoints
//! behind an asynchronous lock. Each checkpoint is reported as
//! [`Status::Checkpointed`]. A checkpoint that fails, or a hold refused,
//! is reported as [`Status::Unkept`], and the session takes no more
//! checkpoints: the hold is released, the session runs on, and its journal
//! is still written.
//!
//! `flife-studio --resume DIR` ([`crate::Studio::resume`]) opens a kept run
//! again, as `flife resume` does, with [`flife_cli::recover`]: the
//! configuration and session the directory records, the newest checkpoint
//! that restores (the founding world when none does), and the session
//! opened through the run's command journal, which replays what it holds
//! past that checkpoint. Its checkpoints continue the run's, from the next
//! multiple after the tick recovered, and its strokes are numbered after
//! the commands the session holds.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use asupersync::Cx;
use asupersync::sync::{Mutex, OwnedMutexGuard};
use flife_cli::{CheckpointAdmission, Checkpoints, Persistence, Record};
use flife_core::ids::{CommandId, SessionId};
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_engine::World;
use flife_runtime::{
    Boundary, CommandBook, Durability, Holding, SessionConfig, SessionHandle, SessionOwner,
};

use crate::Status;

/// A run directory the Studio will keep its session in.
#[derive(Debug, Clone)]
pub struct Keeping {
    persistence: Persistence,
    admission: CheckpointAdmission,
}

impl Keeping {
    /// Keep a session of `config` in `run_dir`, with the configuration's
    /// checkpoint interval, mode and retained bytes, flushed against power
    /// loss ([`Persistence::new`]). Nothing is created yet.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when `run_dir` holds a run or other
    /// files ([`flife_cli::vacant`]); [`CoreError::BudgetExhausted`] when
    /// the configuration's memory cannot hold a checkpoint
    /// ([`flife_cli::admit`]).
    pub fn new(run_dir: impl Into<PathBuf>, config: &ValidatedConfig) -> Result<Self, CoreError> {
        let persistence = Persistence::new(run_dir, config);
        flife_cli::vacant(&persistence.run_dir)?;
        let admission = flife_cli::admit(config, persistence.mode, persistence.retained_bytes)?;
        Ok(Self {
            persistence,
            admission,
        })
    }

    /// The run directory.
    #[must_use]
    pub fn run_dir(&self) -> &std::path::Path {
        &self.persistence.run_dir
    }

    /// The checkpoint admission: the mode checkpoints are taken in and the
    /// most heap one holds beyond the live world.
    #[must_use]
    pub const fn admission(&self) -> &CheckpointAdmission {
        &self.admission
    }

    /// The bytes a checkpoint is charged in the memory admission
    /// ([`World::admit_memory`]'s checkpoint term).
    #[must_use]
    pub const fn checkpoint_bytes(&self) -> u64 {
        self.admission.peak_bytes
    }

    /// Keep a run recovered from `run_dir` there again
    /// ([`flife_cli::recover`]): the configuration's checkpoint settings,
    /// and the admission its checkpoints were opened under.
    pub(crate) fn recovered(
        run_dir: impl Into<PathBuf>,
        config: &ValidatedConfig,
        admission: CheckpointAdmission,
    ) -> Self {
        Self {
            persistence: Persistence::new(run_dir, config),
            admission,
        }
    }

    /// Make the run directory, open its checkpoints, and open `session`'s
    /// session on `world` (founded by `config`, its backend installed)
    /// through its catalog: journaled under `write-ahead-before-apply`.
    ///
    /// # Errors
    /// [`flife_cli::create`]'s, [`Checkpoints::open`]'s,
    /// [`SessionConfig::from_config`]'s and [`flife_cli::open_session`]'s.
    pub fn open(
        &self,
        world: World,
        config: &ValidatedConfig,
        session: SessionId,
    ) -> Result<(SessionOwner, SessionHandle, Checkpoints), CoreError> {
        let limits = SessionConfig::from_config(session, config)?;
        flife_cli::create(&self.persistence.run_dir, config)?;
        let checkpoints = Checkpoints::open(&self.persistence, config, session)?;
        let book = CommandBook::new(limits.session, limits.revision, limits.command_capacity);
        let (owner, handle) =
            flife_cli::open_session(world, book, limits, &self.persistence.run_dir)?;
        Ok((owner, handle, checkpoints))
    }
}

/// The first committed tick a checkpoint is due at after the newest:
/// the next multiple of the interval, or tick zero before any. `None` when
/// the interval is zero (only the last is taken).
fn next_due(checkpoints: &Checkpoints) -> Option<u64> {
    let every = checkpoints.every();
    (every > 0).then(|| {
        checkpoints.last_tick().map_or(0, |last| {
            (last / every).saturating_add(1).saturating_mul(every)
        })
    })
}

/// A kept session's checkpoints, as the checkpointer and the session
/// thread share them.
pub(crate) struct Chain {
    checkpoints: Checkpoints,
    /// The committed tick and the session's newest command read just
    /// before the newest checkpoint was taken: a boundary where both are
    /// still those holds nothing that checkpoint lacks. A command
    /// submitted between the read and the checkpoint only makes this
    /// claim less than the checkpoint holds. `None` when it was not read,
    /// or the newest is the checkpoint a resumed run started from.
    newest: Option<(Tick, Option<CommandId>)>,
}

impl Chain {
    /// Whether the newest checkpoint is of the committed boundary at
    /// `tick`: the first at a boundary is the one before any stroke made
    /// there, so a stroke, or the checkpointer, shares it.
    fn has(&self, tick: Tick) -> bool {
        self.checkpoints.last_tick() == Some(tick.get())
    }

    /// Whether the close needs no checkpoint of the boundary `at`: the
    /// newest is of it and, unless the journal holds every command, it
    /// already held every command the session holds now, so a stroke
    /// pending there is in the last checkpoint.
    fn closes(&self, at: &At) -> bool {
        self.has(at.tick) && (at.journaled || self.newest == Some((at.tick, at.last)))
    }
}

/// The chain the checkpointer and the session thread share: `None` once a
/// checkpoint or a hold failed, after which the session takes no more.
pub(crate) type Shared = Arc<Mutex<Option<Chain>>>;

/// The committed boundary, as the checkpointer reads it.
struct At {
    tick: Tick,
    /// Whether a numerical fault has stopped the world.
    stopped: bool,
    /// The newest command the session holds.
    last: Option<CommandId>,
    /// Whether the session journals every command before it applies
    /// (`write-ahead-before-apply`), so a resumed run replays one pending.
    journaled: bool,
}

async fn committed(cx: &Cx, handle: &SessionHandle) -> Result<At, CoreError> {
    handle
        .checkpoint_with(cx, |world, book, boundary: Boundary| At {
            tick: boundary.tick,
            stopped: world.fault().is_some(),
            last: book.last_command(),
            journaled: book.durability() == Durability::WriteAhead,
        })
        .await
}

/// Take a checkpoint and report what it published; `Err` once it failed
/// (reported).
async fn take(
    cx: &Cx,
    handle: &SessionHandle,
    chain: &mut Chain,
    report: &mpsc::Sender<Status>,
) -> Result<(), ()> {
    // Read first: see `Chain::newest`.
    let before = committed(cx, handle)
        .await
        .ok()
        .map(|at| (at.tick, at.last));
    match chain.checkpoints.take(handle, cx).await {
        Ok(records) => {
            chain.newest = before;
            published(records, report);
            Ok(())
        }
        Err(error) => {
            unkept(&error, report);
            Err(())
        }
    }
}

/// Publish a background checkpoint still in flight and report it; `Err`
/// once it failed (reported).
async fn drain(
    cx: &Cx,
    handle: &SessionHandle,
    checkpoints: &mut Checkpoints,
    report: &mpsc::Sender<Status>,
) -> Result<(), ()> {
    match checkpoints.drain(handle, cx).await {
        Ok(records) => {
            published(records, report);
            Ok(())
        }
        Err(error) => {
            unkept(&error, report);
            Err(())
        }
    }
}

fn published(records: Vec<Record>, report: &mpsc::Sender<Status>) {
    for record in records {
        if let Record::Checkpoint {
            tick,
            epoch,
            sequence,
            sha256,
            bytes,
            retired,
            mode,
            paused_seconds,
            ..
        } = record
        {
            let _ = report.send(Status::Checkpointed {
                tick: Tick::new(tick),
                epoch,
                sequence,
                sha256,
                bytes,
                retired,
                mode,
                paused_seconds,
            });
        }
    }
}

fn unkept(error: &CoreError, report: &mpsc::Sender<Status>) {
    let _ = report.send(Status::Unkept {
        kind: error.kind(),
        detail: error.to_string(),
    });
}

/// Hold `handle` at the multiple of the interval after `tick`; `None`,
/// with the refusal reported, when the session refuses it.
async fn hold_after(
    cx: &Cx,
    handle: &SessionHandle,
    every: u64,
    tick: u64,
    report: &mpsc::Sender<Status>,
) -> Option<Holding> {
    let next = (tick / every).saturating_add(1).saturating_mul(every);
    match handle.hold(cx, Tick::new(next)).await {
        Ok(holding) => Some(holding),
        Err(error) => {
            unkept(&error, report);
            None
        }
    }
}

/// The founding checkpoint, before the session takes any intent: when the
/// interval is not zero. Then the hold at the next multiple after the
/// committed tick, also before any intent, for the checkpointer
/// ([`keep`]); none when the interval is zero. Returns the chain to
/// share, or `None` once the checkpoint or the hold failed (reported).
pub(crate) async fn found(
    cx: &Cx,
    handle: &SessionHandle,
    checkpoints: Checkpoints,
    report: &mpsc::Sender<Status>,
) -> Option<(Shared, Option<Holding>)> {
    let mut chain = Chain {
        checkpoints,
        newest: None,
    };
    if next_due(&chain.checkpoints) == Some(0) {
        take(cx, handle, &mut chain, report).await.ok()?;
    }
    let every = chain.checkpoints.every();
    if every == 0 {
        return Some((Arc::new(Mutex::new(Some(chain))), None));
    }
    let tick = match committed(cx, handle).await {
        Ok(at) => at.tick,
        Err(error) => {
            unkept(&error, report);
            return None;
        }
    };
    let holding = hold_after(cx, handle, every, tick.get(), report).await?;
    Some((Arc::new(Mutex::new(Some(chain))), Some(holding)))
}

/// End `shared`'s chain after a failed checkpoint or hold (reported): the
/// session takes no more, and the hold is released so it runs on.
async fn give_up(cx: &Cx, handle: &SessionHandle, chain: &mut Option<Chain>) {
    *chain = None;
    let _ = handle.release(cx).await;
}

/// The checkpointer: wait for the committed boundary to reach `holding`'s
/// multiple, take a checkpoint there unless the newest holds it, and hold
/// the session at the next multiple of the interval, until the hold's
/// wait ends (the close releases it, or the session closes), `stop` is
/// seen after a new hold, or the chain has ended. A checkpoint or a hold
/// that fails ends the chain ([`give_up`]).
pub(crate) async fn keep(
    cx: &Cx,
    handle: &SessionHandle,
    shared: &Shared,
    mut holding: Holding,
    report: &mpsc::Sender<Status>,
    stop: &AtomicBool,
) {
    loop {
        let at = holding.tick();
        if holding.reached(cx).await.is_err() {
            return;
        }
        let Ok(mut chain) = OwnedMutexGuard::lock(Arc::clone(shared), cx).await else {
            return;
        };
        let Some(kept) = chain.as_mut() else {
            return;
        };
        let every = kept.checkpoints.every();
        match committed(cx, handle).await {
            Ok(at) if !at.stopped => {
                // A stroke while paused here may have taken it already.
                if !kept.has(at.tick) && take(cx, handle, kept, report).await.is_err() {
                    give_up(cx, handle, &mut chain).await;
                    return;
                }
            }
            // A stopped world changes no more: nothing more to keep.
            _ => return,
        }
        let Some(next) = hold_after(cx, handle, every, at.get(), report).await else {
            give_up(cx, handle, &mut chain).await;
            return;
        };
        drop(chain);
        holding = next;
        // Seen after the new hold was set, so a close that released the
        // hold before it never leaves this waiting on it.
        if stop.load(Ordering::Acquire) {
            return;
        }
    }
}

/// Before the session thread offers a stroke (plan §2.2: "save the exact
/// pre-intervention checkpoint"): while the session is paused, the stroke
/// applies at the committed boundary, so publish a checkpoint of it
/// unless the newest is of it (taken before an earlier stroke there, or
/// by the checkpointer at a multiple) or a fault stopped the world. A
/// running session's stroke applies at a boundary still to come, and
/// takes none. A checkpoint that fails ends the chain ([`give_up`]).
pub(crate) async fn before_stroke(
    cx: &Cx,
    handle: &SessionHandle,
    shared: &Shared,
    report: &mpsc::Sender<Status>,
) {
    if !matches!(handle.inspect(cx).await, Ok(snapshot) if !snapshot.running) {
        return;
    }
    let Ok(mut chain) = OwnedMutexGuard::lock(Arc::clone(shared), cx).await else {
        return;
    };
    let Some(kept) = chain.as_mut() else {
        return;
    };
    if let Ok(at) = committed(cx, handle).await
        && !at.stopped
        && !kept.has(at.tick)
        && take(cx, handle, kept, report).await.is_err()
    {
        give_up(cx, handle, &mut chain).await;
    }
}

/// Close a kept session whose pause returned `paused`: publish a
/// checkpoint of the closing boundary unless the newest holds it
/// ([`Chain::closes`]: under `volatile-in-memory`, with every command, so
/// a stroke pending at the closing boundary is in the last checkpoint) or
/// a fault stopped the world, then any background checkpoint in flight,
/// and suspend it.
///
/// # Errors
/// [`SessionHandle::suspend`]'s.
pub(crate) async fn close(
    cx: &Cx,
    handle: &SessionHandle,
    shared: Option<&Shared>,
    paused: Result<Boundary, CoreError>,
    report: &mpsc::Sender<Status>,
) -> Result<Boundary, CoreError> {
    if let Some(shared) = shared
        && let Ok(mut chain) = OwnedMutexGuard::lock(Arc::clone(shared), cx).await
        && let Some(kept) = chain.as_mut()
    {
        match paused {
            Ok(_) => {
                if let Ok(at) = committed(cx, handle).await
                    && !at.stopped
                    && !kept.closes(&at)
                {
                    let _ = take(cx, handle, kept, report).await;
                }
            }
            Err(error) => unkept(&error, report),
        }
        let _ = drain(cx, handle, &mut kept.checkpoints, report).await;
    }
    handle.suspend(cx).await
}
