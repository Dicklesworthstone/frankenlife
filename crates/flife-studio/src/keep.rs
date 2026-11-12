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
//! - Closing pauses the session, then releases the hold, so the
//!   checkpointer ends (a checkpoint it is taking is of the paused
//!   boundary). It publishes a checkpoint of the closing boundary unless
//!   the newest already holds it or the world is stopped by a fault,
//!   publishes a background checkpoint still in flight, and suspends the
//!   session rather than closing it: a command still pending stays
//!   pending, in the final checkpoint and the journal alike, and applies
//!   when the run is resumed past its tick.
//!
//! Each checkpoint is reported as [`Status::Checkpointed`]. A checkpoint
//! that fails, or a hold refused, is reported as [`Status::Unkept`], and
//! the session takes no more checkpoints: the hold is released, the
//! session runs on, and its journal is still written.
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
use std::sync::mpsc;

use asupersync::Cx;
use flife_cli::{CheckpointAdmission, Checkpoints, Persistence, Record};
use flife_core::ids::SessionId;
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_engine::World;
use flife_runtime::{Boundary, CommandBook, Holding, SessionConfig, SessionHandle, SessionOwner};

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

/// The committed boundary's tick, and whether a numerical fault has
/// stopped the world.
async fn committed(cx: &Cx, handle: &SessionHandle) -> Result<(Tick, bool), CoreError> {
    handle
        .checkpoint_with(cx, |world, _, boundary: Boundary| {
            (boundary.tick, world.fault().is_some())
        })
        .await
}

/// Take a checkpoint and report what it published; `Err` once it failed
/// (reported).
async fn take(
    cx: &Cx,
    handle: &SessionHandle,
    checkpoints: &mut Checkpoints,
    report: &mpsc::Sender<Status>,
) -> Result<(), ()> {
    match checkpoints.take(handle, cx).await {
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
/// ([`keep`]); none when the interval is zero. `None` once the checkpoint
/// or the hold failed (reported).
pub(crate) async fn found(
    cx: &Cx,
    handle: &SessionHandle,
    mut checkpoints: Checkpoints,
    report: &mpsc::Sender<Status>,
) -> Option<(Checkpoints, Option<Holding>)> {
    if next_due(&checkpoints) == Some(0) {
        take(cx, handle, &mut checkpoints, report).await.ok()?;
    }
    let every = checkpoints.every();
    if every == 0 {
        return Some((checkpoints, None));
    }
    let (tick, _) = match committed(cx, handle).await {
        Ok(committed) => committed,
        Err(error) => {
            unkept(&error, report);
            return None;
        }
    };
    let holding = hold_after(cx, handle, every, tick.get(), report).await?;
    Some((checkpoints, Some(holding)))
}

/// The checkpointer: wait for the committed boundary to reach `holding`'s
/// multiple, take a checkpoint there, and hold the session at the next,
/// until the hold's wait ends (the close releases it, or the session
/// closes) or `stop` is seen after a new hold. Returns the checkpoints for
/// the closing one, or `None` once a checkpoint or a hold failed
/// (reported), the hold then released so the session runs on.
pub(crate) async fn keep(
    cx: &Cx,
    handle: &SessionHandle,
    mut checkpoints: Checkpoints,
    mut holding: Holding,
    report: &mpsc::Sender<Status>,
    stop: &AtomicBool,
) -> Option<Checkpoints> {
    let every = checkpoints.every();
    loop {
        let at = holding.tick();
        if holding.reached(cx).await.is_err() {
            return Some(checkpoints);
        }
        match committed(cx, handle).await {
            // A stopped world changes no more: nothing more to keep.
            Ok((_, true)) | Err(_) => return Some(checkpoints),
            Ok(_) => {}
        }
        if take(cx, handle, &mut checkpoints, report).await.is_err() {
            let _ = handle.release(cx).await;
            return None;
        }
        let Some(next) = hold_after(cx, handle, every, at.get(), report).await else {
            let _ = handle.release(cx).await;
            return None;
        };
        holding = next;
        // Seen after the new hold was set, so a close that released the
        // hold before it never leaves this waiting on it.
        if stop.load(Ordering::Acquire) {
            return Some(checkpoints);
        }
    }
}

/// Close a kept session whose pause returned `paused`: publish a
/// checkpoint of the closing boundary unless the newest holds it or a
/// fault stopped the world, then any background checkpoint in flight, and
/// suspend it.
///
/// # Errors
/// [`SessionHandle::suspend`]'s.
pub(crate) async fn close(
    cx: &Cx,
    handle: &SessionHandle,
    checkpoints: Option<Checkpoints>,
    paused: Result<Boundary, CoreError>,
    report: &mpsc::Sender<Status>,
) -> Result<Boundary, CoreError> {
    if let Some(mut checkpoints) = checkpoints {
        match paused {
            Ok(_) => {
                if let Ok((tick, false)) = committed(cx, handle).await
                    && checkpoints.last_tick() != Some(tick.get())
                {
                    let _ = take(cx, handle, &mut checkpoints, report).await;
                }
            }
            Err(error) => unkept(&error, report),
        }
        let _ = drain(cx, handle, &mut checkpoints, report).await;
    }
    handle.suspend(cx).await
}
