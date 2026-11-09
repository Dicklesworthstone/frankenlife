//! The world-session owner (plan §12.1–§12.4): the one task that owns the
//! committed [`World`] and its [`CommandBook`], and the only commit
//! authority.
//!
//! Clients reach it through a [`SessionHandle`] over two bounded Asupersync
//! channels: a control lane (pause, resume, step, cancel, status, inspect,
//! close) and a scientific lane (command submission). The control lane has
//! its own reserved capacity, so a full scientific lane never blocks
//! control. Admission never waits for a tick, so a future-tick command
//! cannot block anything behind it.
//!
//! The owner loop:
//! - drains both lanes, control first, at every committed boundary;
//! - while paused, waits for the next message, so pause, resume, step,
//!   inspection and close never need a future tick;
//! - while running, advances one candidate at a time. It seals the
//!   boundary's commands, prepares the candidate, then yields and drains
//!   the control lane at the cancellation boundary before publication. A
//!   cancellation of a sealed command that arrives there wins: the candidate
//!   is discarded and rebuilt from the same committed state without it. One
//!   that arrives after publication is too late. A pause there takes
//!   effect at the boundary publication reaches, and that boundary is
//!   reported. A close there discards the candidate.
//! - stops running on a capacity pause (the boundary stays sealed for the
//!   identical retry), and stops the run on a numerical fault under
//!   `stop-and-preserve`, failing every unfinished command.
//! - offers each committed boundary to its render feed (FL-017) before
//!   anyone hears of it. Viewers lease epochs from the feed directly,
//!   never through the lanes. When every slot is busy the boundary stays
//!   pending, and a lease release wakes a paused owner to retry it. A
//!   retained lease costs display freshness, never a tick, a command or a
//!   birth decision (§7.5, INV-09).
//!
//! Closing (a close request, every handle dropped, running or paused, or the
//! owner's context cancelled) discards any unpublished candidate, cancels every unfinished
//! command as `session-closed`, refuses whatever is still queued, and
//! returns the committed world.

use std::collections::VecDeque;
use std::future::{Future, poll_fn};
use std::pin::pin;
use std::task::Poll;

use asupersync::Cx;
use asupersync::channel::{mpsc, oneshot};
use asupersync::runtime::yield_now;
use flife_core::canonical::Sha256Digest;
use flife_core::ids::{CommandId, ResourceEpoch, SessionId};
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_engine::{Advance, CapacityShortfall, Intervention, TickReport, World};
use flife_field::FieldGrid;
use flife_render::{Feed, Publisher, RenderStats};

use crate::book::CommandBook;
use crate::command::{
    Admission, Cancellation, Command, CommandStatus, Rejection, VOLATILE_IN_MEMORY,
};

/// `write-ahead-before-apply`: Appendix A's durability, FL-036's journal.
pub const WRITE_AHEAD_BEFORE_APPLY: &str = "write-ahead-before-apply";

/// A committed boundary: the world's tick and epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Boundary {
    /// The committed tick.
    pub tick: Tick,
    /// The committed resource epoch.
    pub epoch: ResourceEpoch,
}

/// A session's admission limits and durability, from `[execution]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionConfig {
    /// The session's identity.
    pub session: SessionId,
    /// The control lane's reserved capacity.
    pub control_capacity: usize,
    /// The scientific lane's capacity, which is also the most unfinished
    /// commands the book holds.
    pub command_capacity: usize,
    /// Render epoch slots (`execution.render_queue_capacity`, at least 2).
    pub render_capacity: usize,
    /// The configuration hash the session runs.
    pub revision: Sha256Digest,
}

impl SessionConfig {
    /// The limits of `config` for `session`.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] for `write-ahead-before-apply`, whose
    /// journal is FL-036; [`CoreError::UnknownAlgorithm`] for any other
    /// durability but `volatile-in-memory`; [`CoreError::NumericalInvalid`]
    /// for a zero capacity.
    pub fn from_config(session: SessionId, config: &ValidatedConfig) -> Result<Self, CoreError> {
        let execution = &config.config().execution;
        match execution.command_durability.as_str() {
            VOLATILE_IN_MEMORY => {}
            WRITE_AHEAD_BEFORE_APPLY => {
                return Err(CoreError::MissingCapability {
                    capability: format!(
                        "{WRITE_AHEAD_BEFORE_APPLY} needs the command journal (FL-036); \
                         this session admits {VOLATILE_IN_MEMORY} only"
                    ),
                });
            }
            other => {
                return Err(CoreError::UnknownAlgorithm {
                    role: "execution.command_durability",
                    name: other.to_owned(),
                });
            }
        }
        let capacity = |name: &'static str, value: u32| {
            usize::try_from(value)
                .ok()
                .filter(|&value| value > 0)
                .ok_or_else(|| CoreError::NumericalInvalid {
                    quantity: name,
                    detail: format!("{value} is not a positive queue capacity"),
                })
        };
        Ok(Self {
            session,
            control_capacity: capacity(
                "execution.control_queue_capacity",
                execution.control_queue_capacity,
            )?,
            command_capacity: capacity(
                "execution.command_queue_capacity",
                execution.command_queue_capacity,
            )?,
            render_capacity: capacity(
                "execution.render_queue_capacity",
                execution.render_queue_capacity,
            )?,
            revision: config.config_hash(),
        })
    }
}

/// The result of a single step.
#[derive(Debug, Clone, PartialEq)]
pub enum StepOutcome {
    /// The candidate was published.
    Published {
        /// The boundary publication committed.
        boundary: Boundary,
        /// The commands it applied, in order.
        applied: Vec<CommandId>,
        /// The published tick's report.
        report: Box<TickReport>,
    },
    /// A capacity shortfall paused the tick; nothing changed, and its
    /// boundary stays sealed for the identical retry.
    Paused {
        /// The unchanged committed boundary.
        boundary: Boundary,
        /// What could not be covered.
        shortfall: CapacityShortfall,
    },
    /// The tick could not be prepared or published; nothing changed. A
    /// numerical fault also stopped the run.
    Failed {
        /// The unchanged committed boundary.
        boundary: Boundary,
        /// The error.
        error: CoreError,
        /// Whether the run stopped under `stop-and-preserve`.
        stopped: bool,
    },
}

/// Why a run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum RunEnd {
    /// It reached the requested tick.
    Reached(Boundary),
    /// A pause request stopped it at this boundary.
    Paused(Boundary),
    /// A capacity shortfall paused it.
    CapacityPaused {
        /// The unchanged committed boundary.
        boundary: Boundary,
        /// What could not be covered.
        shortfall: CapacityShortfall,
    },
    /// A tick failed; a numerical fault also stopped the run.
    Failed {
        /// The committed boundary.
        boundary: Boundary,
        /// The error.
        error: CoreError,
        /// Whether the run stopped under `stop-and-preserve`.
        stopped: bool,
    },
    /// The session closed.
    Closed(Boundary),
}

/// The owner's state, read at a committed boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The committed boundary.
    pub boundary: Boundary,
    /// The committed world's digest.
    pub digest: Sha256Digest,
    /// Live organisms.
    pub live: usize,
    /// Whether the owner is running.
    pub running: bool,
    /// The tick a running owner stops at, if any.
    pub until: Option<Tick>,
    /// Unfinished commands.
    pub unfinished: usize,
    /// Commands ever accepted.
    pub admitted: u64,
    /// The outcome stream's cursor.
    pub outcomes: u64,
    /// Whether the run stopped under `stop-and-preserve`.
    pub stopped: bool,
    /// The render publisher's counters.
    pub render: RenderStats,
}

/// What the owner returns when the session closes.
#[derive(Debug)]
pub struct SessionClosed {
    /// The committed world.
    pub world: World,
    /// The book, closed.
    pub book: CommandBook,
    /// The committed boundary.
    pub boundary: Boundary,
    /// The render publisher's counters.
    pub render: RenderStats,
}

enum Control {
    Pause(oneshot::Sender<Boundary>),
    Resume {
        until: Option<Tick>,
        started: oneshot::Sender<Result<Boundary, CoreError>>,
        ended: Option<oneshot::Sender<RunEnd>>,
    },
    Step(oneshot::Sender<Result<StepOutcome, CoreError>>),
    Cancel {
        id: CommandId,
        reply: oneshot::Sender<Cancellation>,
    },
    Status {
        id: CommandId,
        reply: oneshot::Sender<Option<CommandStatus>>,
    },
    Inspect(oneshot::Sender<Snapshot>),
    Close(oneshot::Sender<Boundary>),
}

struct Submission {
    command: Command,
    reply: oneshot::Sender<Admission>,
}

enum Inbound {
    Control(Control),
    Science(Submission),
    Released,
    Ended,
}

/// A client's handle on a session. Cloning it shares the session.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    session: SessionId,
    revision: Sha256Digest,
    control: mpsc::Sender<Control>,
    science: mpsc::Sender<Submission>,
    feed: Feed,
}

impl std::fmt::Debug for Control {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Pause(_) => "Pause",
            Self::Resume { .. } => "Resume",
            Self::Step(_) => "Step",
            Self::Cancel { .. } => "Cancel",
            Self::Status { .. } => "Status",
            Self::Inspect(_) => "Inspect",
            Self::Close(_) => "Close",
        })
    }
}

impl std::fmt::Debug for Submission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Submission")
            .field("command", &self.command)
            .finish_non_exhaustive()
    }
}

fn closed() -> CoreError {
    CoreError::QueueClosed { queue: "session" }
}

async fn answer<T>(cx: &Cx, mut reply: oneshot::Receiver<T>) -> Result<T, CoreError> {
    reply.recv(cx).await.map_err(|error| match error {
        oneshot::RecvError::Cancelled => CoreError::Cancelled {
            reason: "waiting for the session owner".into(),
        },
        _ => closed(),
    })
}

impl SessionHandle {
    /// The session's identity.
    #[must_use]
    pub const fn session(&self) -> SessionId {
        self.session
    }

    /// The session's render epochs (FL-017). Reading them never goes
    /// through the owner's lanes, so no viewer can delay a command or a
    /// tick.
    #[must_use]
    pub fn feed(&self) -> Feed {
        self.feed.clone()
    }

    /// The configuration hash the session runs.
    #[must_use]
    pub const fn revision(&self) -> Sha256Digest {
        self.revision
    }

    async fn control<T>(
        &self,
        cx: &Cx,
        message: impl FnOnce(oneshot::Sender<T>) -> Control,
    ) -> Result<T, CoreError> {
        let (reply, answered) = oneshot::channel();
        self.control
            .send(cx, message(reply))
            .await
            .map_err(|_| closed())?;
        answer(cx, answered).await
    }

    /// Submit a scientific command and wait for its admission. Waits while
    /// the scientific lane is full.
    ///
    /// # Errors
    /// [`CoreError::QueueClosed`] once the session has closed;
    /// [`CoreError::Cancelled`] when `cx` is cancelled while waiting.
    pub async fn submit(&self, cx: &Cx, command: Command) -> Result<Admission, CoreError> {
        let (reply, answered) = oneshot::channel();
        self.science
            .send(cx, Submission { command, reply })
            .await
            .map_err(|_| closed())?;
        answer(cx, answered).await
    }

    /// Submit a scientific command only if the scientific lane has room now;
    /// otherwise it is refused with [`Rejection::QueueFull`] without
    /// waiting.
    ///
    /// # Errors
    /// As [`SessionHandle::submit`].
    pub async fn try_submit(&self, cx: &Cx, command: Command) -> Result<Admission, CoreError> {
        let (reply, answered) = oneshot::channel();
        match self.science.try_send(Submission { command, reply }) {
            Ok(()) => answer(cx, answered).await,
            Err(mpsc::SendError::Full(_)) => Ok(Admission::Rejected(Rejection::QueueFull {
                capacity: self.science.capacity(),
            })),
            Err(_) => Err(closed()),
        }
    }

    /// Cancel a command.
    ///
    /// # Errors
    /// As [`SessionHandle::submit`].
    pub async fn cancel(&self, cx: &Cx, id: CommandId) -> Result<Cancellation, CoreError> {
        self.control(cx, |reply| Control::Cancel { id, reply })
            .await
    }

    /// A command's status.
    ///
    /// # Errors
    /// As [`SessionHandle::submit`].
    pub async fn status(&self, cx: &Cx, id: CommandId) -> Result<Option<CommandStatus>, CoreError> {
        self.control(cx, |reply| Control::Status { id, reply })
            .await
    }

    /// Pause at the next committed boundary, which is returned. Pausing a
    /// paused session returns its boundary.
    ///
    /// # Errors
    /// As [`SessionHandle::submit`].
    pub async fn pause(&self, cx: &Cx) -> Result<Boundary, CoreError> {
        self.control(cx, Control::Pause).await
    }

    /// Run until paused, returning the boundary it started from.
    ///
    /// # Errors
    /// As [`SessionHandle::submit`]; [`CoreError::StateConflict`] once the
    /// run has stopped.
    pub async fn resume(&self, cx: &Cx) -> Result<Boundary, CoreError> {
        self.control(cx, |started| Control::Resume {
            until: None,
            started,
            ended: None,
        })
        .await?
    }

    /// Run until the committed tick reaches `tick`, and wait for the run to
    /// end there or earlier.
    ///
    /// # Errors
    /// As [`SessionHandle::resume`].
    pub async fn run_until(&self, cx: &Cx, tick: Tick) -> Result<RunEnd, CoreError> {
        self.start(cx, Some(tick)).await?.ended(cx).await
    }

    /// Start a run, until the committed tick reaches `until` or until
    /// paused, and return at once with the boundary it started from. The
    /// returned [`Running`] waits for its end.
    ///
    /// # Errors
    /// As [`SessionHandle::resume`].
    pub async fn start(&self, cx: &Cx, until: Option<Tick>) -> Result<Running, CoreError> {
        let (ended, finished) = oneshot::channel();
        let started = self
            .control(cx, |started| Control::Resume {
                until,
                started,
                ended: Some(ended),
            })
            .await??;
        Ok(Running {
            started,
            ended: finished,
        })
    }

    /// Authorize exactly one candidate advancement of a paused session and
    /// report whether it committed.
    ///
    /// # Errors
    /// As [`SessionHandle::submit`]; [`CoreError::StateConflict`] while the
    /// session is running or once the run has stopped.
    pub async fn step(&self, cx: &Cx) -> Result<StepOutcome, CoreError> {
        self.control(cx, Control::Step).await?
    }

    /// The owner's state at its committed boundary.
    ///
    /// # Errors
    /// As [`SessionHandle::submit`].
    pub async fn inspect(&self, cx: &Cx) -> Result<Snapshot, CoreError> {
        self.control(cx, Control::Inspect).await
    }

    /// Close the session, returning the committed boundary it closed at.
    ///
    /// # Errors
    /// As [`SessionHandle::submit`].
    pub async fn close(&self, cx: &Cx) -> Result<Boundary, CoreError> {
        self.control(cx, Control::Close).await
    }
}

struct Run {
    until: Option<Tick>,
    ended: Option<oneshot::Sender<RunEnd>>,
}

/// A run started by [`SessionHandle::start`].
pub struct Running {
    started: Boundary,
    ended: oneshot::Receiver<RunEnd>,
}

impl std::fmt::Debug for Running {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Running")
            .field("started", &self.started)
            .finish_non_exhaustive()
    }
}

impl Running {
    /// The boundary the run started from.
    #[must_use]
    pub const fn started(&self) -> Boundary {
        self.started
    }

    /// Wait for the run to end: at its tick, on a pause, on a capacity
    /// shortfall, on a failed tick, or when the session closes.
    ///
    /// # Errors
    /// [`CoreError::Cancelled`] when the wait is cancelled, and
    /// [`CoreError::QueueClosed`] when the owner has gone.
    pub async fn ended(self, cx: &Cx) -> Result<RunEnd, CoreError> {
        answer(cx, self.ended).await
    }
}

/// The session owner. Run it on its own task with [`SessionOwner::run`].
#[derive(Debug)]
pub struct SessionOwner {
    world: World,
    book: CommandBook,
    grid: FieldGrid,
    render: Publisher,
    control: mpsc::Receiver<Control>,
    science: mpsc::Receiver<Submission>,
    released: mpsc::Receiver<()>,
    run: Option<Run>,
    closing: Vec<oneshot::Sender<Boundary>>,
    deferred: VecDeque<Control>,
}

impl std::fmt::Debug for Run {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Run")
            .field("until", &self.until)
            .finish_non_exhaustive()
    }
}

/// Open a session on `world`, which `config` founded, with the limits
/// `config` declares.
///
/// # Errors
/// As [`SessionConfig::from_config`].
pub fn open(
    world: World,
    config: &ValidatedConfig,
    session: SessionId,
) -> Result<(SessionOwner, SessionHandle), CoreError> {
    open_with(world, SessionConfig::from_config(session, config)?)
}

/// Open a session on `world` with explicit limits. Its founding state is
/// render epoch 1.
///
/// # Errors
/// [`Publisher::new`]'s: fewer than two render slots, or a slot that cannot
/// be allocated.
pub fn open_with(
    world: World,
    limits: SessionConfig,
) -> Result<(SessionOwner, SessionHandle), CoreError> {
    let (control_sender, control) = mpsc::channel(limits.control_capacity);
    let (science_sender, science) = mpsc::channel(limits.command_capacity);
    // One pending release signal is enough: the owner retries the newest
    // boundary, not one per lease.
    let (release_sender, released) = mpsc::channel(1);
    let render = Publisher::new(&world, limits.render_capacity, move || {
        let _ = release_sender.try_send(());
    })?;
    let feed = render.feed();
    let grid = *world.field().grid();
    let owner = SessionOwner {
        world,
        book: CommandBook::new(limits.session, limits.revision, limits.command_capacity),
        grid,
        render,
        control,
        science,
        released,
        run: None,
        closing: Vec::new(),
        deferred: VecDeque::new(),
    };
    let handle = SessionHandle {
        session: limits.session,
        revision: limits.revision,
        control: control_sender,
        science: science_sender,
        feed,
    };
    Ok((owner, handle))
}

enum Ticked {
    Published(Box<TickReport>, Vec<CommandId>),
    Paused(CapacityShortfall),
    Failed(CoreError),
    Closing,
}

impl SessionOwner {
    fn boundary(&self) -> Boundary {
        Boundary {
            tick: self.world.tick(),
            epoch: self.world.epoch(),
        }
    }

    fn stopped(&self) -> bool {
        self.world.fault().is_some()
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            boundary: self.boundary(),
            digest: self.world.digest(),
            live: self.world.population().len(),
            running: self.run.is_some(),
            until: self.run.as_ref().and_then(|run| run.until),
            unfinished: self.book.unfinished(),
            admitted: self.book.admitted(),
            outcomes: self.book.outcome_cursor(),
            stopped: self.stopped(),
            render: self.render.stats().clone(),
        }
    }

    /// Offer the committed state to the render feed. A busy feed leaves the
    /// boundary pending, and a failure is recorded in its counters; neither
    /// touches the world.
    fn offer_render(&mut self) {
        let _ = self.render.offer(&self.world);
    }

    fn admit(&mut self, submission: Submission, committed: Tick, stopped: bool) {
        let Submission { command, reply } = submission;
        let admission = if self.closing.is_empty() {
            self.book.admit(command, committed, &self.grid, stopped)
        } else {
            Admission::Rejected(Rejection::Closed)
        };
        let _ = reply.send_blocking(admission);
    }

    fn end_run(&mut self, end: RunEnd) {
        if let Some(run) = self.run.take()
            && let Some(ended) = run.ended
        {
            let _ = ended.send_blocking(end);
        }
    }

    /// Handle a control message at a committed boundary, where the world is
    /// not borrowed by a candidate.
    async fn handle(&mut self, cx: &Cx, message: Control) {
        match message {
            Control::Pause(reply) => {
                let boundary = self.boundary();
                self.end_run(RunEnd::Paused(boundary));
                let _ = reply.send_blocking(boundary);
            }
            Control::Resume {
                until,
                started,
                ended,
            } => {
                let boundary = self.boundary();
                if self.stopped() {
                    let _ = started.send_blocking(Err(stopped_error(&self.world)));
                    return;
                }
                self.end_run(RunEnd::Paused(boundary));
                let _ = started.send_blocking(Ok(boundary));
                if until.is_some_and(|tick| tick <= boundary.tick) {
                    if let Some(ended) = ended {
                        let _ = ended.send_blocking(RunEnd::Reached(boundary));
                    }
                    return;
                }
                self.run = Some(Run { until, ended });
            }
            Control::Step(reply) => {
                if self.run.is_some() {
                    let _ = reply.send_blocking(Err(CoreError::StateConflict {
                        operation: "step session",
                        detail: "the session is running; pause it first".into(),
                    }));
                    return;
                }
                if self.stopped() {
                    let _ = reply.send_blocking(Err(stopped_error(&self.world)));
                    return;
                }
                let outcome = match self.tick(cx).await {
                    Ticked::Published(report, applied) => StepOutcome::Published {
                        boundary: self.boundary(),
                        applied,
                        report,
                    },
                    Ticked::Paused(shortfall) => StepOutcome::Paused {
                        boundary: self.boundary(),
                        shortfall,
                    },
                    Ticked::Failed(error) => StepOutcome::Failed {
                        boundary: self.boundary(),
                        error,
                        stopped: self.stopped(),
                    },
                    Ticked::Closing => StepOutcome::Failed {
                        boundary: self.boundary(),
                        error: closed(),
                        stopped: false,
                    },
                };
                let _ = reply.send_blocking(Ok(outcome));
            }
            Control::Cancel { id, reply } => {
                let _ = reply.send_blocking(self.book.cancel(id, self.world.tick()));
            }
            Control::Status { id, reply } => {
                let _ = reply.send_blocking(self.book.status(id).cloned());
            }
            Control::Inspect(reply) => {
                let _ = reply.send_blocking(self.snapshot());
            }
            Control::Close(reply) => self.closing.push(reply),
        }
    }

    /// Prepare, check and publish one candidate. Between preparation and
    /// publication it yields and drains the control lane: the cancellation
    /// boundary.
    async fn tick(&mut self, cx: &Cx) -> Ticked {
        loop {
            if cx.checkpoint().is_err() {
                return Ticked::Closing;
            }
            let committed = self.world.tick();
            let sealed = match self.book.seal(committed) {
                Ok(sealed) => sealed,
                Err(error) => return Ticked::Failed(error),
            };
            let interventions: Vec<Intervention> = sealed.iter().map(|&(_, i)| i).collect();
            let Self {
                world,
                book,
                grid,
                render,
                control,
                science,
                closing,
                deferred,
                run,
                ..
            } = self;
            let candidate = match world.advance_with(&interventions) {
                Ok(Advance::Ready(candidate)) => candidate,
                Ok(Advance::Paused(shortfall)) => return Ticked::Paused(shortfall),
                Err(error) => {
                    if let Some(fault) = world.fault() {
                        let detail = format!(
                            "the run stopped under stop-and-preserve advancing tick {}: {}",
                            fault.tick.get(),
                            fault.error
                        );
                        book.fail_unfinished(&detail);
                    }
                    return Ticked::Failed(error);
                }
            };

            // The cancellation boundary: other tasks run, then every control
            // message that arrived is decided before publication.
            yield_now().await;
            let mut discard = false;
            let mut pause = Vec::new();
            while let Ok(message) = control.try_recv() {
                match message {
                    Control::Cancel { id, reply } => {
                        let decision = book.cancel(id, committed);
                        if matches!(decision, Cancellation::Cancelled { sealed: true, .. }) {
                            discard = true;
                        }
                        let _ = reply.send_blocking(decision);
                    }
                    Control::Status { id, reply } => {
                        let _ = reply.send_blocking(book.status(id).cloned());
                    }
                    Control::Pause(reply) => pause.push(reply),
                    Control::Close(reply) => {
                        closing.push(reply);
                        discard = true;
                    }
                    other => deferred.push_back(other),
                }
            }
            while let Ok(submission) = science.try_recv() {
                let Submission { command, reply } = submission;
                let admission = if closing.is_empty() {
                    book.admit(command, committed, grid, false)
                } else {
                    Admission::Rejected(Rejection::Closed)
                };
                let _ = reply.send_blocking(admission);
            }
            if discard {
                candidate.cancel();
                if !closing.is_empty() {
                    return Ticked::Closing;
                }
                for reply in pause {
                    deferred.push_front(Control::Pause(reply));
                }
                continue;
            }
            let report = match candidate.publish() {
                Ok(report) => report,
                Err(error) => return Ticked::Failed(error),
            };
            let applied = sealed.iter().map(|&(id, _)| id).collect();
            if let Err(error) = book.published(&report, &sealed) {
                return Ticked::Failed(error);
            }
            // Before anyone hears of the boundary: a viewer that is told of
            // a published tick finds its epoch unless every slot is busy.
            let _ = render.offer(world);
            if !pause.is_empty() {
                let boundary = Boundary {
                    tick: world.tick(),
                    epoch: world.epoch(),
                };
                if let Some(current) = run.take()
                    && let Some(ended) = current.ended
                {
                    let _ = ended.send_blocking(RunEnd::Paused(boundary));
                }
                for reply in pause {
                    let _ = reply.send_blocking(boundary);
                }
            }
            return Ticked::Published(Box::new(report), applied);
        }
    }

    /// Wait for the next message on either lane, control first. A receive
    /// takes a message only when it completes, so the lane that loses keeps
    /// its message for the next call.
    async fn next_inbound(
        cx: &Cx,
        control: &mut mpsc::Receiver<Control>,
        science: &mut mpsc::Receiver<Submission>,
        released: &mut mpsc::Receiver<()>,
    ) -> Inbound {
        let mut control = pin!(control.recv(cx));
        let mut science = pin!(science.recv(cx));
        let mut released = pin!(released.recv(cx));
        poll_fn(|task| {
            match control.as_mut().poll(task) {
                Poll::Ready(Ok(message)) => return Poll::Ready(Inbound::Control(message)),
                Poll::Ready(Err(_)) => return Poll::Ready(Inbound::Ended),
                Poll::Pending => {}
            }
            match science.as_mut().poll(task) {
                Poll::Ready(Ok(submission)) => return Poll::Ready(Inbound::Science(submission)),
                Poll::Ready(Err(_)) => return Poll::Ready(Inbound::Ended),
                Poll::Pending => {}
            }
            // The publisher holds the release sender, so this lane never
            // closes while the owner runs; a lease release only wakes it.
            match released.as_mut().poll(task) {
                Poll::Ready(Ok(())) => Poll::Ready(Inbound::Released),
                Poll::Ready(Err(_)) | Poll::Pending => Poll::Pending,
            }
        })
        .await
    }

    /// Own the world until the session closes, then return it.
    pub async fn run(mut self, cx: &Cx) -> SessionClosed {
        'session: loop {
            // Every committed boundary: the render offer (a no-op unless a
            // boundary is pending), deferred control, then both lanes,
            // control first.
            self.offer_render();
            while let Some(message) = self.deferred.pop_front() {
                self.handle(cx, message).await;
            }
            let mut abandoned = false;
            loop {
                match self.control.try_recv() {
                    Ok(message) => self.handle(cx, message).await,
                    Err(mpsc::RecvError::Disconnected) => {
                        abandoned = true;
                        break;
                    }
                    Err(_) => break,
                }
            }
            let (committed, stopped) = (self.world.tick(), self.stopped());
            while let Ok(submission) = self.science.try_recv() {
                self.admit(submission, committed, stopped);
            }
            // Every handle is gone, so nothing can pause or close a run: the
            // session closes, running or not.
            if abandoned || !self.closing.is_empty() || cx.checkpoint().is_err() {
                break 'session;
            }
            if let Some(until) = self.run.as_ref().map(|run| run.until) {
                if until.is_some_and(|tick| self.world.tick() >= tick) {
                    let boundary = self.boundary();
                    self.end_run(RunEnd::Reached(boundary));
                    continue;
                }
                match self.tick(cx).await {
                    Ticked::Published(..) => {}
                    Ticked::Paused(shortfall) => {
                        let boundary = self.boundary();
                        self.end_run(RunEnd::CapacityPaused {
                            boundary,
                            shortfall,
                        });
                    }
                    Ticked::Failed(error) => {
                        let boundary = self.boundary();
                        let stopped = self.stopped();
                        self.end_run(RunEnd::Failed {
                            boundary,
                            error,
                            stopped,
                        });
                    }
                    Ticked::Closing => break 'session,
                }
                yield_now().await;
                continue;
            }
            let inbound =
                Self::next_inbound(cx, &mut self.control, &mut self.science, &mut self.released)
                    .await;
            match inbound {
                Inbound::Control(message) => self.handle(cx, message).await,
                Inbound::Science(submission) => {
                    let (committed, stopped) = (self.world.tick(), self.stopped());
                    self.admit(submission, committed, stopped);
                }
                // A slot may be free: the loop's next offer retries.
                Inbound::Released => {}
                Inbound::Ended => break 'session,
            }
        }
        self.shut_down()
    }

    fn shut_down(mut self) -> SessionClosed {
        let boundary = self.boundary();
        self.offer_render();
        self.render.close();
        self.end_run(RunEnd::Closed(boundary));
        self.book.close(boundary.tick);
        self.control.close();
        self.science.close();
        let mut queued: Vec<Control> = self.deferred.drain(..).collect();
        while let Ok(message) = self.control.try_recv() {
            queued.push(message);
        }
        let mut submissions = Vec::new();
        while let Ok(submission) = self.science.try_recv() {
            submissions.push(submission);
        }
        let snapshot = Snapshot {
            running: false,
            until: None,
            ..self.snapshot()
        };
        for message in queued {
            match message {
                Control::Pause(reply) | Control::Close(reply) => {
                    let _ = reply.send_blocking(boundary);
                }
                Control::Resume { started, ended, .. } => {
                    let _ = started.send_blocking(Err(closed()));
                    if let Some(ended) = ended {
                        let _ = ended.send_blocking(RunEnd::Closed(boundary));
                    }
                }
                Control::Step(reply) => {
                    let _ = reply.send_blocking(Err(closed()));
                }
                Control::Cancel { id, reply } => {
                    let decision = match self.book.status(id) {
                        Some(CommandStatus::Terminal(outcome)) => {
                            Cancellation::AlreadyTerminal(outcome.clone())
                        }
                        _ => Cancellation::Unknown,
                    };
                    let _ = reply.send_blocking(decision);
                }
                Control::Status { id, reply } => {
                    let _ = reply.send_blocking(self.book.status(id).cloned());
                }
                Control::Inspect(reply) => {
                    let _ = reply.send_blocking(snapshot.clone());
                }
            }
        }
        // A closed book answers a retry of an accepted identity with its
        // outcome and refuses everything else.
        for Submission { command, reply } in submissions {
            let admission = self.book.admit(command, boundary.tick, &self.grid, true);
            let _ = reply.send_blocking(admission);
        }
        for reply in self.closing.drain(..) {
            let _ = reply.send_blocking(boundary);
        }
        SessionClosed {
            render: self.render.stats().clone(),
            world: self.world,
            book: self.book,
            boundary,
        }
    }
}

fn stopped_error(world: &World) -> CoreError {
    CoreError::StateConflict {
        operation: "advance session",
        detail: world.fault().map_or_else(
            || "the run stopped".to_owned(),
            |fault| {
                format!(
                    "the run stopped under stop-and-preserve advancing tick {}: {}",
                    fault.tick.get(),
                    fault.error
                )
            },
        ),
    }
}
