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

use std::sync::mpsc;

use asupersync::Cx;
use asupersync::channel::mpsc as lane;
use asupersync::runtime::{JoinHandle, RuntimeBuilder, RuntimeHandle};
use flife_core::ids::OrganismId;
use flife_core::{CoreError, Tick};
use flife_engine::OrganismSnapshot;
use flife_render::Feed;
use flife_runtime::{RunEnd, SessionClosed, SessionHandle, SessionOwner, StepOutcome};

use crate::Intent;

/// Intents the lane holds before refusing more.
pub const INTENT_CAPACITY: usize = 16;

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
        let (intents, intake) = lane::channel(INTENT_CAPACITY);
        let (report, statuses) = mpsc::channel();
        let feed = handle.feed();
        let thread = std::thread::Builder::new()
            .name("flife-session".into())
            .spawn(move || serve(owner, handle, intake, report))
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
    pub fn close(mut self) -> Result<SessionClosed, CoreError> {
        // A full lane still drains: the controller handles intents in turn,
        // and dropping the sender below closes the lane once it is empty.
        let _ = self.intents.try_send(Intent::Close);
        let thread = self.thread.take();
        drop(self);
        match thread {
            Some(thread) => thread
                .join()
                .map_err(|_| missing("a session thread that did not panic".into()))?,
            None => Err(CoreError::StateConflict {
                operation: "close studio session",
                detail: "already closed".into(),
            }),
        }
    }
}

fn serve(
    owner: SessionOwner,
    handle: SessionHandle,
    mut intake: lane::Receiver<Intent>,
    report: mpsc::Sender<Status>,
) -> Result<SessionClosed, CoreError> {
    let runtime = RuntimeBuilder::current_thread()
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
                let mut runs: Vec<JoinHandle<()>> = Vec::new();
                let mut started = 0;
                loop {
                    // A closed lane (the Studio dropped its sender) closes.
                    let intent = intake.recv(&cx).await.unwrap_or(Intent::Close);
                    if intent == Intent::Close {
                        break;
                    }
                    runs.retain(|run| !run.is_finished());
                    let status = act(
                        &cx,
                        &handle,
                        &watchers,
                        &report,
                        &mut runs,
                        &mut started,
                        intent,
                    )
                    .await;
                    let _ = report.send(status);
                }
                let closed = handle.close(&cx).await;
                // Closing ends every run, so each watcher reports and returns.
                for run in runs {
                    run.await;
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

async fn act(
    cx: &Cx,
    handle: &SessionHandle,
    watchers: &RuntimeHandle,
    report: &mpsc::Sender<Status>,
    runs: &mut Vec<JoinHandle<()>>,
    started: &mut u64,
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
            *started += 1;
            let run = *started;
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
                    runs.push(watcher);
                    Status::Running { run, from }
                }
                // The run goes on; only its end will not be reported.
                Err(error) => refused(missing(format!("the run watcher: {error}"))),
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
        // The client loop closes before it acts.
        Intent::Close => refused(CoreError::StateConflict {
            operation: "close studio session",
            detail: "close is handled by the controller loop".into(),
        }),
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
