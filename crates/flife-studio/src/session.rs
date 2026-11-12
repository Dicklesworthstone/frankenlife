//! The session on its own thread, behind a bounded intent lane (plan §13.6).
//!
//! Native windows, event polling and presentation stay on the process main
//! thread. The FL-014 session owner and a small client task run on an
//! Asupersync current-thread runtime on a second thread. The two sides meet
//! at three places, and the main thread never blocks on any of them:
//!
//! - **Intents**: a bounded Asupersync lane of [`INTENT_CAPACITY`]. The main
//!   thread only ever `try_send`s; a full lane refuses the intent.
//! - **Statuses**: what each intent did, on a standard channel the main
//!   thread drains with `try_recv`.
//! - **The render feed**: FL-017's epochs, read through a viewer.
//!
//! The client is the session's only controller, and every intent it takes
//! gets exactly one status. A resume starts a run and hands its end to a
//! watcher task the client owns, so a run that stops on its own (a capacity
//! shortfall, a failed tick) still comes back, as [`Status::Ended`]. A
//! paused run reports both [`Status::Paused`] (the intent) and
//! [`Status::Ended`] (the run). An inspection is answered with
//! [`Status::Inspected`]: the organism read at the session's committed
//! boundary, whose tick the snapshot carries.
//!
//! A brush's stroke (FL-022) is submitted as a scientific command for the
//! next boundary not yet sealed, stating the session's configuration hash.
//! Its acceptance is answered with [`Status::Submitted`], which names the
//! boundary it will apply at; a rejection with [`Status::Refused`]. A
//! watcher the client owns then asks the command's status every
//! [`OUTCOME_POLL`] until it is terminal and reports [`Status::Finished`]:
//! for an applied command, the tick it applied at, the tick that published
//! it and the energy it moved. A paused session holds the command until a
//! step or a run reaches its boundary.
//!
//! A session kept in a run directory ([`SessionThread::spawn_kept`],
//! FL-024's `--run`; [`crate::keep`]) publishes its founding checkpoint
//! before the client takes any intent, then a checkpointer task the client
//! owns publishes one as the committed tick passes each due tick, each
//! reported as [`Status::Checkpointed`]. Closing it pauses it, publishes
//! the closing boundary and suspends the session instead of closing it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use asupersync::Cx;
use asupersync::channel::mpsc as lane;
use asupersync::runtime::{JoinHandle, RuntimeBuilder, RuntimeHandle};
use asupersync::time::{sleep, wall_now};
use flife_cli::Checkpoints;
use flife_core::ids::{CommandId, OrganismId};
use flife_core::{CoreError, Tick};
use flife_engine::{ControllerWeights, OrganismSnapshot};
use flife_render::Feed;
use flife_runtime::{
    Admission, Command, CommandStatus, Outcome, Rejection, RunEnd, SessionClosed, SessionHandle,
    SessionOwner, StepOutcome, Target,
};

use crate::{Intent, Stroke, keep};

/// Intents the lane holds before refusing more.
pub const INTENT_CAPACITY: usize = 16;

/// How often a command's watcher asks for its status.
pub const OUTCOME_POLL: Duration = Duration::from_millis(20);

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum End {
    /// A pause stopped it.
    Paused,
    /// It reached its tick limit.
    Reached,
    /// A capacity shortfall paused it, with the boundary sealed for retry.
    CapacityPaused {
        /// The shortfall.
        detail: String,
    },
    /// A tick failed.
    Failed {
        /// The error's class.
        kind: &'static str,
        /// The error.
        detail: String,
    },
    /// The session closed.
    Closed,
}

/// What an intent or a run did.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// A run started from this boundary.
    Running {
        /// The run's number: the session's runs count from 1.
        run: u64,
        /// The boundary it started from.
        from: Tick,
    },
    /// A pause took effect at this boundary; also the answer for a session
    /// that was already paused.
    Paused {
        /// The committed boundary.
        at: Tick,
    },
    /// A run ended at this boundary. Its end can arrive after a later
    /// intent's status, so it names its run.
    Ended {
        /// The run's number.
        run: u64,
        /// The committed boundary.
        at: Tick,
        /// Why.
        why: End,
    },
    /// A step published this tick.
    Stepped {
        /// The tick it published.
        tick: Tick,
    },
    /// A step did not publish.
    StepHeld {
        /// The unchanged boundary.
        at: Tick,
        /// Why.
        why: End,
    },
    /// A brush's command was accepted (FL-022).
    Submitted {
        /// Its identity: the Studio's commands count on from the
        /// session's last command, from 1 in a fresh session.
        command: CommandId,
        /// What it asks.
        stroke: Stroke,
        /// The source tick of the epoch the brush was used on.
        seen: Tick,
        /// The committed tick whose boundary it applies at.
        effective: Tick,
    },
    /// An accepted command finished. This can arrive after a later
    /// intent's status, so it names its command.
    Finished {
        /// Its identity.
        command: CommandId,
        /// How it finished.
        outcome: Outcome,
    },
    /// An inspection's answer (FL-021).
    Inspected {
        /// The organism asked about.
        organism: OrganismId,
        /// The source tick of the epoch it was selected on.
        seen: Tick,
        /// It at the session's committed boundary, which its `tick` names;
        /// `None` when the committed world no longer holds it.
        snapshot: Option<Box<OrganismSnapshot>>,
    },
    /// A weight dump's answer (FL-021).
    Weights {
        /// The organism asked about.
        organism: OrganismId,
        /// The source tick of the epoch it was shown on.
        seen: Tick,
        /// Its whole controller at the session's committed boundary, which
        /// its `tick` names; `None` when the committed world no longer
        /// holds it.
        weights: Option<Box<ControllerWeights>>,
    },
    /// A checkpoint of a kept session was published to its run directory
    /// (FL-024): written, verified, indexed in the catalog and retained.
    Checkpointed {
        /// The committed tick it holds.
        tick: Tick,
        /// The committed epoch it holds.
        epoch: u64,
        /// Its chunk-store sequence number.
        sequence: u64,
        /// The container's SHA-256, in hex.
        sha256: String,
        /// The container's size.
        bytes: u64,
        /// Older checkpoints removed under `retain_recent_checkpoints`.
        retired: Vec<u64>,
        /// The mode it was taken in (FL-038).
        mode: &'static str,
        /// Wall-clock seconds the session took no step for it.
        paused_seconds: f64,
    },
    /// A kept session's checkpoint failed: it takes no more, and runs on
    /// with its journal.
    Unkept {
        /// The error's class.
        kind: &'static str,
        /// The error.
        detail: String,
    },
    /// The session refused an intent.
    Refused {
        /// The intent.
        intent: Intent,
        /// The error's class.
        kind: &'static str,
        /// The error.
        detail: String,
    },
    /// The session closed at this boundary.
    Closed {
        /// The committed boundary.
        at: Tick,
    },
}

/// The session owner and its controller on their own thread.
#[derive(Debug)]
pub struct SessionThread {
    intents: lane::Sender<Intent>,
    statuses: mpsc::Receiver<Status>,
    feed: Feed,
    thread: Option<std::thread::JoinHandle<Result<SessionClosed, CoreError>>>,
}

impl SessionThread {
    /// Start `owner` and a controller over `handle` on a new thread.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] when the thread cannot be spawned.
    pub fn spawn(owner: SessionOwner, handle: SessionHandle) -> Result<Self, CoreError> {
        Self::spawn_kept(owner, handle, None)
    }

    /// Start `owner` and a controller over `handle` on a new thread,
    /// keeping its run directory through `checkpoints` when given
    /// ([`crate::Keeping`]; [`crate::Keeping::open`] opens the three).
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] when the thread cannot be spawned.
    pub fn spawn_kept(
        owner: SessionOwner,
        handle: SessionHandle,
        checkpoints: Option<Checkpoints>,
    ) -> Result<Self, CoreError> {
        let (intents, intake) = lane::channel(INTENT_CAPACITY);
        let (report, statuses) = mpsc::channel();
        let feed = handle.feed();
        let thread = std::thread::Builder::new()
            .name("flife-session".into())
            .spawn(move || serve(owner, handle, intake, report, checkpoints))
            .map_err(|error| missing(format!("the session thread: {error}")))?;
        Ok(Self {
            intents,
            statuses,
            feed,
            thread: Some(thread),
        })
    }

    /// The session's render feed.
    #[must_use]
    pub fn feed(&self) -> Feed {
        self.feed.clone()
    }

    /// Offer an intent without blocking.
    ///
    /// # Errors
    /// [`CoreError::BudgetExhausted`] when the lane is full, and
    /// [`CoreError::QueueClosed`] once the session thread has gone.
    pub fn send(&self, intent: Intent) -> Result<(), CoreError> {
        self.intents.try_send(intent).map_err(|error| match error {
            lane::SendError::Full(_) => CoreError::BudgetExhausted {
                budget: "studio intents",
                required: 1,
                available: 0,
            },
            _ => CoreError::QueueClosed {
                queue: "studio intents",
            },
        })
    }

    /// Every status reported since the last call, without blocking.
    pub fn statuses(&self) -> impl Iterator<Item = Status> + '_ {
        self.statuses.try_iter()
    }

    /// Close the session and wait for its thread, returning the committed
    /// world.
    ///
    /// # Errors
    /// The session's closing errors, or [`CoreError::MissingCapability`]
    /// when the thread panicked.
    pub fn close(self) -> Result<SessionClosed, CoreError> {
        self.close_reporting().map(|(closed, _)| closed)
    }

    /// [`SessionThread::close`], also returning every status not yet taken
    /// with [`SessionThread::statuses`], those reported while it closed
    /// last: a run's end, a kept session's last checkpoints, the close.
    ///
    /// # Errors
    /// As [`SessionThread::close`].
    pub fn close_reporting(self) -> Result<(SessionClosed, Vec<Status>), CoreError> {
        let Self {
            intents,
            statuses,
            feed,
            thread,
        } = self;
        // A full lane still drains: the controller handles intents in turn,
        // and dropping the sender below closes the lane once it is empty.
        let _ = intents.try_send(Intent::Close);
        drop(intents);
        drop(feed);
        let closed = match thread {
            Some(thread) => thread
                .join()
                .map_err(|_| missing("a session thread that did not panic".into()))?,
            None => Err(CoreError::StateConflict {
                operation: "close studio session",
                detail: "already closed".into(),
            }),
        }?;
        Ok((closed, statuses.try_iter().collect()))
    }
}

fn serve(
    owner: SessionOwner,
    handle: SessionHandle,
    mut intake: lane::Receiver<Intent>,
    report: mpsc::Sender<Status>,
    checkpoints: Option<Checkpoints>,
) -> Result<SessionClosed, CoreError> {
    let runtime = RuntimeBuilder::current_thread()
        .enable_time()
        .build()
        .map_err(|error| missing(format!("an Asupersync runtime: {error}")))?;
    let spawner = runtime.handle();
    runtime.block_on(async move {
        let owner = spawner
            .try_spawn(async move {
                let cx = Cx::current().ok_or_else(|| missing("a task context".into()))?;
                Ok::<_, CoreError>(owner.run(&cx).await)
            })
            .map_err(|error| missing(format!("the owner task: {error}")))?;
        let watchers = spawner.clone();
        let client = spawner
            .try_spawn(async move {
                let cx = Cx::current().ok_or_else(|| missing("a task context".into()))?;
                // Read before any intent: the largest command identity the
                // session holds and its unfinished commands (a resumed run's).
                // Only a session that has stopped already fails to answer,
                // and it then admits nothing.
                let (issued, unfinished) = handle
                    .checkpoint_with(&cx, |_, book, _| {
                        (
                            book.last_command().map_or(0, CommandId::get),
                            book.schedule().map(|(.., id)| id).collect::<Vec<_>>(),
                        )
                    })
                    .await
                    .unwrap_or_default();
                let mut client = Client {
                    issued,
                    ..Client::default()
                };
                // A resumed run's unfinished commands report how they finish
                // as the Studio's own strokes do.
                for command in unfinished {
                    let (session, report) = (handle.clone(), report.clone());
                    let watched = watchers.try_spawn(async move {
                        let Some(cx) = Cx::current() else {
                            return;
                        };
                        if let Ok(status) = finished(&cx, &session, command).await {
                            let _ = report.send(status);
                        }
                    });
                    if let Ok(watcher) = watched {
                        client.watchers.push(watcher);
                    }
                }
                // A kept session: its founding checkpoint before any intent,
                // then the checkpointer.
                let kept = checkpoints.is_some();
                let stop = Arc::new(AtomicBool::new(false));
                let mut keeper = None;
                if let Some(checkpoints) = checkpoints
                    && let Some(checkpoints) = keep::found(&cx, &handle, checkpoints, &report).await
                {
                    let (session, kept_report, kept_stop) =
                        (handle.clone(), report.clone(), Arc::clone(&stop));
                    let spawned = watchers.try_spawn(async move {
                        let cx = Cx::current()?;
                        keep::keep(&cx, &session, checkpoints, &kept_report, &kept_stop).await
                    });
                    match spawned {
                        Ok(task) => keeper = Some(task),
                        Err(error) => {
                            let _ = report.send(Status::Unkept {
                                kind: "missing-capability",
                                detail: format!("the checkpointer: {error}"),
                            });
                        }
                    }
                }
                loop {
                    // A closed lane (the Studio dropped its sender) closes.
                    let intent = intake.recv(&cx).await.unwrap_or(Intent::Close);
                    if intent == Intent::Close {
                        break;
                    }
                    client.watchers.retain(|watcher| !watcher.is_finished());
                    let status = act(&cx, &handle, &watchers, &report, &mut client, intent).await;
                    let _ = report.send(status);
                }
                let closed = if kept {
                    stop.store(true, Ordering::Release);
                    let checkpoints = match keeper {
                        Some(task) => task.await,
                        None => None,
                    };
                    keep::close(&cx, &handle, checkpoints, &report).await
                } else {
                    handle.close(&cx).await
                };
                // Closing ends every run and every command's wait (a status
                // after close is refused), so each watcher reports and
                // returns.
                for watcher in client.watchers {
                    watcher.await;
                }
                let boundary = closed?;
                let _ = report.send(Status::Closed { at: boundary.tick });
                Ok::<_, CoreError>(())
            })
            .map_err(|error| missing(format!("the controller task: {error}")))?;
        let client = client.await;
        let closed = owner.await?;
        client?;
        Ok(closed)
    })
}

/// What the controller keeps between intents.
#[derive(Default)]
struct Client {
    /// The tasks watching runs and commands, awaited at close.
    watchers: Vec<JoinHandle<()>>,
    /// Runs started.
    started: u64,
    /// Commands submitted.
    submitted: u64,
    /// The largest command identity the session held when it opened: a
    /// resumed run's commands keep theirs, and the Studio's follow them.
    issued: u64,
}

async fn act(
    cx: &Cx,
    handle: &SessionHandle,
    watchers: &RuntimeHandle,
    report: &mpsc::Sender<Status>,
    client: &mut Client,
    intent: Intent,
) -> Status {
    let refused = |error: CoreError| Status::Refused {
        intent,
        kind: error.kind(),
        detail: error.to_string(),
    };
    match intent {
        Intent::Resume => {
            let running = match handle.start(cx, None).await {
                Ok(running) => running,
                Err(error) => return refused(error),
            };
            client.started += 1;
            let run = client.started;
            let from = running.started().tick;
            let report = report.clone();
            let watched = watchers.try_spawn(async move {
                let Some(cx) = Cx::current() else {
                    return;
                };
                let status = match running.ended(&cx).await {
                    Ok(end) => ended(run, end),
                    Err(error) => Status::Refused {
                        intent: Intent::Resume,
                        kind: error.kind(),
                        detail: error.to_string(),
                    },
                };
                let _ = report.send(status);
            });
            match watched {
                Ok(watcher) => {
                    client.watchers.push(watcher);
                    Status::Running { run, from }
                }
                // The run goes on; only its end will not be reported.
                Err(error) => refused(missing(format!("the run watcher: {error}"))),
            }
        }
        Intent::Intervene { stroke, seen } => {
            // After every command the session held when it opened (a
            // resumed run's), so its identities never meet an earlier
            // client's.
            let command = CommandId::new(client.issued + client.submitted + 1);
            let submitted = handle
                .submit(
                    cx,
                    Command {
                        session: handle.session(),
                        id: command,
                        target: Target::NextAdmissible,
                        expected_revision: Some(handle.revision()),
                        intervention: stroke.intervention(),
                    },
                )
                .await;
            let accepted = match submitted {
                Ok(Admission::Accepted { accepted, .. }) => accepted,
                Ok(Admission::Rejected(rejection)) => {
                    let (kind, detail) = rejected(&rejection);
                    return Status::Refused {
                        intent,
                        kind,
                        detail,
                    };
                }
                Err(error) => return refused(error),
            };
            client.submitted += 1;
            let report = report.clone();
            let session = handle.clone();
            let watched = watchers.try_spawn(async move {
                let Some(cx) = Cx::current() else {
                    return;
                };
                let status = finished(&cx, &session, command).await;
                let _ = report.send(status.unwrap_or_else(|error| Status::Refused {
                    intent,
                    kind: error.kind(),
                    detail: error.to_string(),
                }));
            });
            match watched {
                Ok(watcher) => {
                    client.watchers.push(watcher);
                    Status::Submitted {
                        command,
                        stroke,
                        seen,
                        effective: accepted.effective,
                    }
                }
                // The command still applies; only its outcome will not be
                // reported.
                Err(error) => refused(missing(format!("the command watcher: {error}"))),
            }
        }
        Intent::Pause => match handle.pause(cx).await {
            Ok(boundary) => Status::Paused { at: boundary.tick },
            Err(error) => refused(error),
        },
        Intent::Step => match handle.step(cx).await {
            Ok(StepOutcome::Published { boundary, .. }) => Status::Stepped {
                tick: boundary.tick,
            },
            Ok(StepOutcome::Paused {
                boundary,
                shortfall,
            }) => Status::StepHeld {
                at: boundary.tick,
                why: End::CapacityPaused {
                    detail: format!("{shortfall:?}"),
                },
            },
            Ok(StepOutcome::Failed {
                boundary, error, ..
            }) => Status::StepHeld {
                at: boundary.tick,
                why: End::Failed {
                    kind: error.kind(),
                    detail: error.to_string(),
                },
            },
            Err(error) => refused(error),
        },
        Intent::Inspect { organism, seen } => match handle.inspect_organism(cx, organism).await {
            Ok(snapshot) => Status::Inspected {
                organism,
                seen,
                snapshot: snapshot.map(Box::new),
            },
            Err(error) => refused(error),
        },
        Intent::DumpWeights { organism, seen } => {
            match handle.organism_weights(cx, organism).await {
                Ok(weights) => Status::Weights {
                    organism,
                    seen,
                    weights: weights.map(Box::new),
                },
                Err(error) => refused(error),
            }
        }
        // The client loop closes before it acts.
        Intent::Close => refused(CoreError::StateConflict {
            operation: "close studio session",
            detail: "close is handled by the controller loop".into(),
        }),
    }
}

/// Wait for `command` to finish, asking every [`OUTCOME_POLL`].
async fn finished(
    cx: &Cx,
    handle: &SessionHandle,
    command: CommandId,
) -> Result<Status, CoreError> {
    loop {
        cx.checkpoint().map_err(|_| CoreError::Cancelled {
            reason: "the studio's command watcher was cancelled".into(),
        })?;
        match handle.status(cx, command).await? {
            Some(CommandStatus::Terminal(outcome)) => {
                return Ok(Status::Finished { command, outcome });
            }
            Some(CommandStatus::Pending | CommandStatus::Sealed) => {
                sleep(wall_now(), OUTCOME_POLL).await;
            }
            None => {
                return Err(CoreError::StateConflict {
                    operation: "watch a studio command",
                    detail: format!("the session does not know command {}", command.get()),
                });
            }
        }
    }
}

/// A rejection's class and detail.
fn rejected(rejection: &Rejection) -> (&'static str, String) {
    match rejection {
        Rejection::WrongSession { session } => (
            "wrong-session",
            format!("addressed to another session than {}", session.get()),
        ),
        Rejection::RevisionMismatch { revision } => (
            "revision-mismatch",
            format!("the session runs configuration {revision:?}"),
        ),
        Rejection::Invalid(error) => (error.kind(), error.to_string()),
        Rejection::Late {
            requested,
            earliest,
        } => (
            "late",
            format!(
                "boundary {} is sealed; the earliest open is {}",
                requested.get(),
                earliest.get()
            ),
        ),
        Rejection::QueueFull { capacity } => (
            "queue-full",
            format!("the scientific queue holds its {capacity} unfinished commands"),
        ),
        Rejection::ConflictingReuse { .. } => (
            "conflicting-reuse",
            "the identity was accepted earlier with different content".into(),
        ),
        Rejection::Stopped => ("stopped", "the run stopped; nothing more applies".into()),
        Rejection::Closed => ("closed", "the session is closing".into()),
    }
}

fn ended(run: u64, end: RunEnd) -> Status {
    match end {
        RunEnd::Reached(boundary) => Status::Ended {
            run,
            at: boundary.tick,
            why: End::Reached,
        },
        RunEnd::Paused(boundary) => Status::Ended {
            run,
            at: boundary.tick,
            why: End::Paused,
        },
        RunEnd::CapacityPaused {
            boundary,
            shortfall,
        } => Status::Ended {
            run,
            at: boundary.tick,
            why: End::CapacityPaused {
                detail: format!("{shortfall:?}"),
            },
        },
        RunEnd::Failed {
            boundary, error, ..
        } => Status::Ended {
            run,
            at: boundary.tick,
            why: End::Failed {
                kind: error.kind(),
                detail: error.to_string(),
            },
        },
        RunEnd::Closed(boundary) => Status::Ended {
            run,
            at: boundary.tick,
            why: End::Closed,
        },
    }
}

fn missing(capability: String) -> CoreError {
    CoreError::MissingCapability { capability }
}
