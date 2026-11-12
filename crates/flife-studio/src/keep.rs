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
//!   any intent, when the interval is not zero. While the Studio runs, a
//!   checkpointer reads the committed boundary every [`CHECKPOINT_POLL`]
//!   and, once it is at or past the next multiple of the interval after
//!   the newest checkpoint, takes one there. A running session's
//!   checkpoint can therefore hold a later tick than the multiple, and
//!   multiples a run passes between two reads share one checkpoint
//!   (`flife run` steps one tick at a time and lands on each). A
//!   checkpoint lands about the ticks run in one read's interval past its
//!   multiple, and multiples share one only above 20 intervals a second
//!   (76,800 ticks a second at the preset's 3,840-tick interval); in a
//!   debug build the 32 × 32 test world ran about 49 ticks between two
//!   reads. A world stopped by a numerical fault is not checkpointed
//!   again.
//! - Closing pauses the session, publishes a checkpoint of the closing
//!   boundary unless the newest already holds it or the world is stopped
//!   by a fault, publishes a background checkpoint still in flight, and
//!   suspends the session rather than closing it: a command still pending
//!   stays pending, in the final checkpoint and the journal alike, and
//!   applies when the run is resumed past its tick.
//!
//! Each checkpoint is reported as [`Status::Checkpointed`]. A checkpoint
//! that fails is reported as [`Status::Unkept`], and the session takes no
//! more of them; it runs on, and its journal is still written.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use asupersync::Cx;
use asupersync::time::{sleep, wall_now};
use flife_cli::{CheckpointAdmission, Checkpoints, Persistence, Record};
use flife_core::ids::SessionId;
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_engine::World;
use flife_runtime::{Boundary, CommandBook, SessionConfig, SessionHandle, SessionOwner};

use crate::Status;

/// How often a running kept session's committed boundary is read.
pub const CHECKPOINT_POLL: Duration = Duration::from_millis(50);

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

/// The founding checkpoint, before the session takes any intent: when the
/// interval is not zero. `None` once it failed.
pub(crate) async fn found(
    cx: &Cx,
    handle: &SessionHandle,
    mut checkpoints: Checkpoints,
    report: &mpsc::Sender<Status>,
) -> Option<Checkpoints> {
    if next_due(&checkpoints) == Some(0) {
        take(cx, handle, &mut checkpoints, report).await.ok()?;
    }
    Some(checkpoints)
}

/// The checkpointer: until `stop`, read the committed boundary every
/// [`CHECKPOINT_POLL`] and take a checkpoint once it reaches the next due
/// tick. Returns the checkpoints for the closing one, or `None` once one
/// failed.
pub(crate) async fn keep(
    cx: &Cx,
    handle: &SessionHandle,
    mut checkpoints: Checkpoints,
    report: &mpsc::Sender<Status>,
    stop: &AtomicBool,
) -> Option<Checkpoints> {
    let Some(mut due) = next_due(&checkpoints) else {
        return Some(checkpoints);
    };
    while !stop.load(Ordering::Acquire) && cx.checkpoint().is_ok() {
        sleep(wall_now(), CHECKPOINT_POLL).await;
        if stop.load(Ordering::Acquire) {
            break;
        }
        match committed(cx, handle).await {
            // A stopped world changes no more: nothing more to keep.
            Ok((_, true)) | Err(_) => break,
            Ok((tick, false)) if tick.get() < due => {}
            Ok(_) => {
                take(cx, handle, &mut checkpoints, report).await.ok()?;
                due = next_due(&checkpoints)?;
            }
        }
    }
    Some(checkpoints)
}

/// Close a kept session: pause it, publish a checkpoint of the closing
/// boundary unless the newest holds it or a fault stopped the world, then
/// any background checkpoint in flight, and suspend it.
///
/// # Errors
/// [`SessionHandle::suspend`]'s.
pub(crate) async fn close(
    cx: &Cx,
    handle: &SessionHandle,
    checkpoints: Option<Checkpoints>,
    report: &mpsc::Sender<Status>,
) -> Result<Boundary, CoreError> {
    if let Some(mut checkpoints) = checkpoints {
        match handle.pause(cx).await {
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
