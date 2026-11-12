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
//! - keeps the committed boundary at a client's held tick
//!   ([`SessionHandle::hold`]): a run stays running and a step waits there,
//!   with control live, until the hold moves or is released, and the
//!   hold's wait is answered once the boundary is there. A hold that
//!   arrives at the cancellation boundary takes effect after that
//!   candidate is decided.
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
//! returns the committed world. Suspending ([`SessionHandle::suspend`],
//! FL-037) closes the same way but leaves every unfinished command pending,
//! for a later session of the same run.
//!
//! Under `write-ahead-before-apply` ([`open_journaled`], FL-036) the owner
//! makes commands durable through a [`JournalWriter`] before relying on
//! them, and never waits on its I/O:
//! - an acceptance is answered only once the journal's receipt says it,
//!   and every acceptance before it, is durable. A retry of an identity
//!   still waiting shares that receipt;
//! - a boundary is held while an acceptance due there, or any
//!   cancellation, waits for its receipt (or while the journal's queue is
//!   at its limit). The control lane stays live meanwhile, and a step or
//!   checkpoint requested then waits with the boundary;
//! - a cancellation is answered once its record is durable. One that wins
//!   at the cancellation boundary discards the candidate first, and the
//!   candidate is rebuilt only after the record is durable;
//! - outcomes are journaled as they occur without holding the boundary:
//!   one lost in a crash is reproduced by replay;
//! - a journal failure is uncertain, not a refusal: every request waiting
//!   on it is answered with the error, the run stops, and new commands are
//!   refused, while control and close still work. A recovered session
//!   reconciles from what the journal actually holds.
//!
//! Opening a journaled session replays every journaled outcome the
//! starting book does not hold, advancing the world and checking that each
//! is reproduced exactly, so the session starts at the journal's horizon
//! with every later acceptance pending.

use std::collections::{BTreeSet, VecDeque};
use std::future::{Future, poll_fn};
use std::pin::pin;
use std::task::Poll;

use asupersync::Cx;
use asupersync::channel::{mpsc, oneshot};
use asupersync::runtime::yield_now;
use flife_core::canonical::Sha256Digest;
use flife_core::ids::{CommandId, OrganismId, ResourceEpoch, SessionId};
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_engine::{
    Advance, CapacityShortfall, ControllerWeights, Intervention, OrganismSnapshot,
    RegionMeasurement, TickReport, World,
};
use flife_field::FieldGrid;
use flife_render::{Feed, Publisher, RenderStats};

use crate::book::CommandBook;
use crate::command::{
    Admission, Cancellation, Command, CommandStatus, Durability, Outcome, Rejection,
    VOLATILE_IN_MEMORY,
};
use crate::journal::{JournalContents, JournalRecord, JournalWriter, Receipt, Watermarks};

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
    /// How acceptances are held: `execution.command_durability`.
    pub durability: Durability,
}

impl SessionConfig {
    /// The limits of `config` for `session`.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for a durability other than
    /// `volatile-in-memory` and `write-ahead-before-apply`;
    /// [`CoreError::NumericalInvalid`] for a zero capacity.
    pub fn from_config(session: SessionId, config: &ValidatedConfig) -> Result<Self, CoreError> {
        let execution = &config.config().execution;
        let durability = match execution.command_durability.as_str() {
            VOLATILE_IN_MEMORY => Durability::Volatile,
            WRITE_AHEAD_BEFORE_APPLY => Durability::WriteAhead,
            other => {
                return Err(CoreError::UnknownAlgorithm {
                    role: "execution.command_durability",
                    name: other.to_owned(),
                });
            }
        };
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
            durability,
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
    /// The tick no run or step passes ([`SessionHandle::hold`]), if any.
    pub hold: Option<Tick>,
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
    /// The journal's state, for a `write-ahead-before-apply` session.
    pub journal: Option<JournalState>,
}

/// A journaled session's durability, from the receipts it has had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalState {
    /// The journal's durable watermarks after its last receipt. The
    /// admission and outcome watermarks are distinct positions.
    pub durable: Watermarks,
    /// Records sent and not yet answered.
    pub waiting: usize,
    /// The failure that stopped the journal, if one did. It and every
    /// record sent after it are uncertain.
    pub failed: Option<CoreError>,
}

/// A checkpoint of a session's committed boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCheckpoint {
    /// The boundary it holds.
    pub boundary: Boundary,
    /// FL-016's container of the committed world and its command closure.
    pub bytes: Vec<u8>,
}

/// What the owner returns when the session closes.
#[derive(Debug)]
pub struct SessionClosed {
    /// The committed world.
    pub world: World,
    /// The book: closed, or for a session suspended
    /// ([`SessionHandle::suspend`]) with every pending command still
    /// pending.
    pub book: CommandBook,
    /// The committed boundary.
    pub boundary: Boundary,
    /// The render publisher's counters.
    pub render: RenderStats,
    /// The journal's state once every record sent was answered, for a
    /// `write-ahead-before-apply` session.
    pub journal: Option<JournalState>,
}

type AdmissionReply = oneshot::Sender<Result<Admission, CoreError>>;

/// A client's function of the committed boundary
/// ([`SessionHandle::checkpoint_with`]), or the reason it cannot run.
type Take = Box<dyn FnOnce(Result<(&World, &CommandBook, Boundary), CoreError>) + Send>;
type CancelReply = oneshot::Sender<Result<Cancellation, CoreError>>;

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
        reply: CancelReply,
    },
    Status {
        id: CommandId,
        reply: oneshot::Sender<Option<CommandStatus>>,
    },
    DurableOutcome {
        id: CommandId,
        reply: oneshot::Sender<Result<Option<Outcome>, CoreError>>,
    },
    Inspect(oneshot::Sender<Snapshot>),
    InspectOrganism {
        id: OrganismId,
        reply: oneshot::Sender<Result<Option<OrganismSnapshot>, CoreError>>,
    },
    OrganismWeights {
        id: OrganismId,
        reply: oneshot::Sender<Result<Option<ControllerWeights>, CoreError>>,
    },
    MeasureRegion {
        center: [f64; 2],
        radius: f64,
        reply: oneshot::Sender<Result<RegionMeasurement, CoreError>>,
    },
    Checkpoint(oneshot::Sender<Result<SessionCheckpoint, CoreError>>),
    Take(Take),
    /// Hold the committed boundary at `tick`, or release the hold (`None`).
    Hold {
        tick: Option<Tick>,
        set: oneshot::Sender<Result<Boundary, CoreError>>,
        reached: Option<HoldReply>,
    },
    Close(oneshot::Sender<Boundary>),
    Suspend(oneshot::Sender<Boundary>),
}

type HoldReply = oneshot::Sender<Result<Boundary, CoreError>>;

struct Submission {
    command: Command,
    reply: AdmissionReply,
}

#[expect(
    clippy::large_enum_variant,
    reason = "a material command's direction makes submissions large; one message per request"
)]
enum Inbound {
    Control(Control),
    Science(Submission),
    Receipt(Receipt),
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
            Self::DurableOutcome { .. } => "DurableOutcome",
            Self::Inspect(_) => "Inspect",
            Self::InspectOrganism { .. } => "InspectOrganism",
            Self::OrganismWeights { .. } => "OrganismWeights",
            Self::MeasureRegion { .. } => "MeasureRegion",
            Self::Checkpoint(_) => "Checkpoint",
            Self::Take(_) => "Take",
            Self::Hold { .. } => "Hold",
            Self::Close(_) => "Close",
            Self::Suspend(_) => "Suspend",
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
    /// the scientific lane is full. Under `write-ahead-before-apply` an
    /// acceptance is answered once it is durable.
    ///
    /// # Errors
    /// [`CoreError::QueueClosed`] once the session has closed;
    /// [`CoreError::Cancelled`] when `cx` is cancelled while waiting; the
    /// journal's error when its record is uncertain (it may or may not have
    /// persisted: retry the same identity after recovery).
    pub async fn submit(&self, cx: &Cx, command: Command) -> Result<Admission, CoreError> {
        let (reply, answered) = oneshot::channel();
        self.science
            .send(cx, Submission { command, reply })
            .await
            .map_err(|_| closed())?;
        answer(cx, answered).await?
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
            Ok(()) => answer(cx, answered).await?,
            Err(mpsc::SendError::Full(_)) => Ok(Admission::Rejected(Rejection::QueueFull {
                capacity: self.science.capacity(),
            })),
            Err(_) => Err(closed()),
        }
    }

    /// Cancel a command. Under `write-ahead-before-apply` a cancellation
    /// that wins is answered once its record is durable.
    ///
    /// # Errors
    /// As [`SessionHandle::submit`].
    pub async fn cancel(&self, cx: &Cx, id: CommandId) -> Result<Cancellation, CoreError> {
        self.control(cx, |reply| Control::Cancel { id, reply })
            .await?
    }

    /// A checkpoint of the committed boundary (FL-016's container), taken
    /// without a future tick. Under `write-ahead-before-apply` it waits
    /// until every record sent is durable, so the journal holds everything
    /// the checkpoint's command closure does.
    ///
    /// # Errors
    /// As [`SessionHandle::submit`], and as
    /// [`checkpoint`](crate::checkpoint::checkpoint).
    pub async fn checkpoint(&self, cx: &Cx) -> Result<SessionCheckpoint, CoreError> {
        self.control(cx, Control::Checkpoint).await?
    }

    /// Run `take` on the committed boundary exactly where
    /// [`SessionHandle::checkpoint`] would encode it: the world, its command
    /// book and the boundary, with no future tick, and under
    /// `write-ahead-before-apply` once every record sent is durable. The
    /// session takes no step while `take` runs, so a checkpoint `take`
    /// streams elsewhere (FL-038's pause-and-stream) holds one committed
    /// state. `take` runs on the owner's task: it blocks the session, not
    /// the client.
    ///
    /// # Errors
    /// As [`SessionHandle::submit`].
    pub async fn checkpoint_with<T: Send + 'static>(
        &self,
        cx: &Cx,
        take: impl FnOnce(&World, &CommandBook, Boundary) -> T + Send + 'static,
    ) -> Result<T, CoreError> {
        self.control(cx, |reply| {
            Control::Take(Box::new(move |state| {
                let _ = reply.send_blocking(
                    state.map(|(world, book, boundary)| take(world, book, boundary)),
                );
            }))
        })
        .await?
    }

    /// A command's status.
    ///
    /// # Errors
    /// As [`SessionHandle::submit`].
    pub async fn status(&self, cx: &Cx, id: CommandId) -> Result<Option<CommandStatus>, CoreError> {
        self.control(cx, |reply| Control::Status { id, reply })
            .await
    }

    /// A `write-ahead-before-apply` command's terminal outcome once the
    /// journal holds it: the explicitly durable response (§12.3). `None`
    /// at once while the command is unfinished or unknown; otherwise it
    /// waits for the outcome's receipt if that has not arrived.
    ///
    /// # Errors
    /// As [`SessionHandle::submit`]; [`CoreError::StateConflict`] for a
    /// `volatile-in-memory` session, whose outcomes are never durable; the
    /// journal's error once it has failed.
    pub async fn durable_outcome(
        &self,
        cx: &Cx,
        id: CommandId,
    ) -> Result<Option<Outcome>, CoreError> {
        self.control(cx, |reply| Control::DurableOutcome { id, reply })
            .await?
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

    /// One organism at the owner's committed boundary (FL-021, plan §14.2),
    /// or `None` for an identity the world does not hold there: its row,
    /// its own genome's traits and shape, and its held output decoded
    /// ([`World::organism_snapshot`]). A running owner answers between
    /// ticks, so the snapshot is of one whole committed tick, named by its
    /// `tick`; nothing else of the world is copied and no result changes.
    /// The owner's world then watches that organism ([`World::watch`]): its
    /// observation is recorded in every tick from the next one on, so a
    /// later inspection of it carries what it sensed and the activations of
    /// its newest evaluation. A world watches one organism, the one most
    /// recently inspected.
    ///
    /// # Errors
    /// As [`SessionHandle::submit`] and [`World::organism_snapshot`].
    pub async fn inspect_organism(
        &self,
        cx: &Cx,
        id: OrganismId,
    ) -> Result<Option<OrganismSnapshot>, CoreError> {
        self.control(cx, |reply| Control::InspectOrganism { id, reply })
            .await?
    }

    /// One organism's whole controller at the owner's committed boundary
    /// (FL-021, plan §14.2's on-demand weight dump), or `None` for an
    /// identity the world does not hold there: its genome's parameters
    /// copied whole, with the layer shapes that lay them out
    /// ([`World::organism_weights`]). A running owner answers between
    /// ticks, so the dump is of one whole committed tick, named by its
    /// `tick`. It copies that one genome and nothing else, watches nothing,
    /// and changes no result.
    ///
    /// # Errors
    /// As [`SessionHandle::submit`] and [`World::organism_weights`].
    pub async fn organism_weights(
        &self,
        cx: &Cx,
        id: OrganismId,
    ) -> Result<Option<ControllerWeights>, CoreError> {
        self.control(cx, |reply| Control::OrganismWeights { id, reply })
            .await?
    }

    /// What the disk of `radius` world units about `center` holds at the
    /// owner's committed boundary (plan §14.3's temporary measurement
    /// region; [`World::measure_region`]): the organisms a removal of that
    /// disk applied there would take, their energy and lineages, and the
    /// field cells whose centres lie in it and their energy. A running
    /// owner answers between ticks, so the measurement is of one whole
    /// committed tick, named by its `tick`. It copies no genome, watches
    /// nothing and changes no result.
    ///
    /// # Errors
    /// As [`SessionHandle::submit`] and [`World::measure_region`].
    pub async fn measure_region(
        &self,
        cx: &Cx,
        center: [f64; 2],
        radius: f64,
    ) -> Result<RegionMeasurement, CoreError> {
        self.control(cx, |reply| Control::MeasureRegion {
            center,
            radius,
            reply,
        })
        .await?
    }

    /// Close the session, returning the committed boundary it closed at.
    ///
    /// # Errors
    /// As [`SessionHandle::submit`].
    pub async fn close(&self, cx: &Cx) -> Result<Boundary, CoreError> {
        self.control(cx, Control::Close).await
    }

    /// Close the session without ending its commands (FL-037): every
    /// pending command stays pending in the book the owner returns and, for
    /// a `write-ahead-before-apply` session, unended in its journal, so a
    /// later session of the same run, restored from a checkpoint with the
    /// journal replayed, applies it at its tick. [`SessionHandle::close`],
    /// and every handle dropped, cancel them as `session-closed`; a close
    /// requested alongside a suspension wins.
    ///
    /// # Errors
    /// As [`SessionHandle::submit`].
    pub async fn suspend(&self, cx: &Cx) -> Result<Boundary, CoreError> {
        self.control(cx, Control::Suspend).await
    }

    /// Hold the session at committed tick `tick` (FL-024: a kept Studio
    /// session's checkpoints on their multiples): no run and no step takes
    /// the committed boundary past it until the hold moves or is released.
    /// A run stays running while held, and ends as it would have; a step
    /// requested at the hold waits there with it. Control stays live, so a
    /// checkpoint, an inspection, a pause or a close is answered at the
    /// held boundary. A hold replaces the session's earlier one, whose
    /// wait is answered with [`CoreError::StateConflict`]. Holding changes
    /// no result: the world a held session commits is the world it commits
    /// unheld.
    ///
    /// Returns at once with the committed boundary; the returned
    /// [`Holding`] waits for the boundary to be at `tick`.
    ///
    /// # Errors
    /// As [`SessionHandle::submit`]; [`CoreError::StateConflict`] when the
    /// committed tick is already past `tick` (a running session may pass it
    /// before the request arrives), and the earlier hold stays.
    pub async fn hold(&self, cx: &Cx, tick: Tick) -> Result<Holding, CoreError> {
        let (reached, waiting) = oneshot::channel();
        let set = self
            .control(cx, |set| Control::Hold {
                tick: Some(tick),
                set,
                reached: Some(reached),
            })
            .await??;
        Ok(Holding { tick, set, waiting })
    }

    /// Release the session's hold, if any, returning the committed
    /// boundary. A wait on the hold is answered with
    /// [`CoreError::StateConflict`].
    ///
    /// # Errors
    /// As [`SessionHandle::submit`].
    pub async fn release(&self, cx: &Cx) -> Result<Boundary, CoreError> {
        self.control(cx, |set| Control::Hold {
            tick: None,
            set,
            reached: None,
        })
        .await?
    }
}

/// A hold set by [`SessionHandle::hold`].
pub struct Holding {
    tick: Tick,
    set: Boundary,
    waiting: oneshot::Receiver<Result<Boundary, CoreError>>,
}

impl std::fmt::Debug for Holding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Holding")
            .field("tick", &self.tick)
            .field("set", &self.set)
            .finish_non_exhaustive()
    }
}

impl Holding {
    /// The tick held.
    #[must_use]
    pub const fn tick(&self) -> Tick {
        self.tick
    }

    /// The committed boundary when the hold was set.
    #[must_use]
    pub const fn set(&self) -> Boundary {
        self.set
    }

    /// Wait until the committed boundary is at the held tick, and return
    /// it; at once when it was there already.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] when the hold is moved or released
    /// first; [`CoreError::QueueClosed`] when the session closes first;
    /// [`CoreError::Cancelled`] when the wait is cancelled.
    pub async fn reached(self, cx: &Cx) -> Result<Boundary, CoreError> {
        answer(cx, self.waiting).await?
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

/// What waits for one journal receipt.
enum Waiter {
    /// An acceptance, answered to every submission of its identity once
    /// durable. Its boundary is held until then.
    Acceptance {
        id: CommandId,
        effective: Tick,
        replies: Vec<(AdmissionReply, bool)>,
    },
    /// A cancellation, answered once durable. Every boundary is held until
    /// then, so no candidate is built without knowing it.
    Cancellation {
        reply: CancelReply,
        decision: Cancellation,
    },
    /// An outcome nobody waits for.
    Outcome,
}

/// A journaled session's link to its journal.
struct Durable {
    writer: JournalWriter,
    waiting: VecDeque<Waiter>,
    limit: usize,
    durable: Watermarks,
    failed: Option<CoreError>,
    uncertain: BTreeSet<CommandId>,
}

impl Durable {
    fn state(&self) -> JournalState {
        JournalState {
            durable: self.durable,
            waiting: self.waiting.len(),
            failed: self.failed.clone(),
        }
    }

    /// Whether the boundary of committed tick `committed` must wait for a
    /// receipt before a candidate is built there.
    fn holds(&self, committed: Tick) -> bool {
        self.waiting.len() >= self.limit
            || self.waiting.iter().any(|waiter| match waiter {
                Waiter::Acceptance { effective, .. } => *effective == committed,
                Waiter::Cancellation { .. } => true,
                Waiter::Outcome => false,
            })
    }

    /// Whether the owner may take another submission. A failed journal
    /// takes them, to refuse them.
    fn room(&self) -> bool {
        self.failed.is_some() || self.waiting.len() < self.limit
    }

    fn refuse(waiter: Waiter, error: &CoreError) {
        match waiter {
            Waiter::Acceptance { replies, .. } => {
                for (reply, _) in replies {
                    let _ = reply.send_blocking(Err(error.clone()));
                }
            }
            Waiter::Cancellation { reply, .. } => {
                let _ = reply.send_blocking(Err(error.clone()));
            }
            Waiter::Outcome => {}
        }
    }

    fn uncertain(&mut self, waiter: Waiter, error: CoreError) {
        if let Waiter::Acceptance { id, .. } = &waiter {
            self.uncertain.insert(*id);
        }
        Self::refuse(waiter, &error);
        self.failed.get_or_insert(error);
    }

    fn send(&mut self, record: JournalRecord, waiter: Waiter) {
        let sent = match &self.failed {
            Some(error) => Err(error.clone()),
            None => self.writer.send(record),
        };
        match sent {
            Ok(()) => self.waiting.push_back(waiter),
            Err(error) => self.uncertain(waiter, error),
        }
    }

    /// Settle the oldest waiting record with its receipt.
    fn settle(&mut self, receipt: Receipt, book: &CommandBook) {
        let Some(waiter) = self.waiting.pop_front() else {
            return;
        };
        match receipt {
            Ok(durable) => {
                self.durable = durable;
                match waiter {
                    Waiter::Acceptance { id, replies, .. } => {
                        if let (Some(accepted), Some(status)) = (book.accepted(id), book.status(id))
                        {
                            for (reply, retry) in replies {
                                let _ = reply.send_blocking(Ok(Admission::Accepted {
                                    accepted,
                                    status: status.clone(),
                                    retry,
                                }));
                            }
                        }
                    }
                    Waiter::Cancellation { reply, decision } => {
                        let _ = reply.send_blocking(Ok(decision));
                    }
                    Waiter::Outcome => {}
                }
            }
            Err(error) => self.uncertain(waiter, error),
        }
    }

    /// Settle every receipt that has arrived.
    fn settle_ready(&mut self, book: &CommandBook) {
        while !self.waiting.is_empty() {
            let Some(receipt) = self.writer.try_receipt() else {
                return;
            };
            self.settle(receipt, book);
        }
    }

    /// Journal the outcomes `book` recorded from cursor `from` on.
    fn outcomes(&mut self, book: &CommandBook, from: u64) {
        for cursor in from..book.outcome_cursor() {
            if let Some((id, outcome)) = book.outcome_at(cursor) {
                self.send(
                    JournalRecord::Ended {
                        id,
                        outcome: outcome.clone(),
                    },
                    Waiter::Outcome,
                );
            }
        }
    }

    /// Answer a cancellation `decision` made with the outcome cursor at
    /// `from`: once its record is durable if it ended the command, at once
    /// otherwise.
    fn cancelled(
        &mut self,
        book: &CommandBook,
        from: u64,
        decision: Cancellation,
        reply: CancelReply,
    ) {
        match (&decision, book.outcome_at(from)) {
            (Cancellation::Cancelled { .. }, Some((id, outcome))) => self.send(
                JournalRecord::Ended {
                    id,
                    outcome: outcome.clone(),
                },
                Waiter::Cancellation { reply, decision },
            ),
            _ => {
                let _ = reply.send_blocking(Ok(decision));
            }
        }
    }

    /// Admit `command` through `book` and journal a new acceptance, whose
    /// client is answered with its receipt.
    fn admit(
        &mut self,
        book: &mut CommandBook,
        grid: &FieldGrid,
        submission: Submission,
        committed: Tick,
        stopped: bool,
    ) {
        let Submission { command, reply } = submission;
        if let Some(error) = &self.failed
            && book.status(command.id).is_none()
        {
            let _ = reply.send_blocking(Err(error.clone()));
            return;
        }
        let admission = book.admit(command, committed, grid, stopped);
        match admission {
            Admission::Accepted {
                accepted,
                retry: false,
                ..
            } => self.send(
                JournalRecord::Accepted { command, accepted },
                Waiter::Acceptance {
                    id: command.id,
                    effective: accepted.effective,
                    replies: vec![(reply, false)],
                },
            ),
            Admission::Accepted { retry: true, .. } if self.uncertain.contains(&command.id) => {
                let error = self
                    .failed
                    .clone()
                    .unwrap_or_else(|| CoreError::StateConflict {
                        operation: "admit command",
                        detail: format!("{}'s journal record is uncertain", command.id),
                    });
                let _ = reply.send_blocking(Err(error));
            }
            Admission::Accepted { retry: true, .. } => {
                let waiting = self.waiting.iter_mut().find_map(|waiter| match waiter {
                    Waiter::Acceptance { id, replies, .. } if *id == command.id => Some(replies),
                    _ => None,
                });
                match waiting {
                    Some(replies) => replies.push((reply, true)),
                    None => {
                        let _ = reply.send_blocking(Ok(admission));
                    }
                }
            }
            Admission::Rejected(_) => {
                let _ = reply.send_blocking(Ok(admission));
            }
        }
    }

    /// Wait for every record sent to be answered, then stop the writer. The
    /// session is closing, so the wait cannot be cancelled.
    async fn drain(&mut self, book: &CommandBook) {
        let cx = Cx::detached_cancel_context();
        while !self.waiting.is_empty() {
            let receipt = match self.writer.receipts().recv(&cx).await {
                Ok(receipt) => receipt,
                Err(_) => Err(CoreError::StorageFailed {
                    operation: "run the journal writer",
                    path: "flife-journal thread".into(),
                    reason: "the writer thread has ended".into(),
                }),
            };
            self.settle(receipt, book);
        }
        self.writer.finish();
    }
}

/// Admit `submission` and answer it: through the journal for a journaled
/// session, at once otherwise.
fn admit_into(
    book: &mut CommandBook,
    journal: Option<&mut Durable>,
    grid: &FieldGrid,
    submission: Submission,
    committed: Tick,
    stopped: bool,
    closing: bool,
) {
    if closing {
        let _ = submission
            .reply
            .send_blocking(Ok(Admission::Rejected(Rejection::Closed)));
        return;
    }
    match journal {
        Some(journal) => journal.admit(book, grid, submission, committed, stopped),
        None => {
            let Submission { command, reply } = submission;
            let _ = reply.send_blocking(Ok(book.admit(command, committed, grid, stopped)));
        }
    }
}

/// Cancel `id` at `committed` and answer: through the journal for a
/// journaled session, at once otherwise. Returns the decision's kind for
/// the caller: whether a sealed command was cancelled.
fn cancel_into(
    book: &mut CommandBook,
    journal: Option<&mut Durable>,
    id: CommandId,
    committed: Tick,
    reply: CancelReply,
) -> bool {
    match journal {
        Some(journal) => {
            if let Some(error) = &journal.failed {
                let _ = reply.send_blocking(Err(error.clone()));
                return false;
            }
            let from = book.outcome_cursor();
            let decision = book.cancel(id, committed);
            let sealed = matches!(decision, Cancellation::Cancelled { sealed: true, .. });
            journal.cancelled(book, from, decision, reply);
            sealed
        }
        None => {
            let decision = book.cancel(id, committed);
            let sealed = matches!(decision, Cancellation::Cancelled { sealed: true, .. });
            let _ = reply.send_blocking(Ok(decision));
            sealed
        }
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
    /// Whether every close requested is a suspension.
    suspending: bool,
    deferred: VecDeque<Control>,
    journal: Option<Durable>,
    held: VecDeque<Control>,
    /// The tick no run or step passes ([`SessionHandle::hold`]), and the
    /// wait to answer when the committed boundary is there.
    hold: Option<(Tick, Option<HoldReply>)>,
}

impl std::fmt::Debug for Durable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Durable")
            .field("state", &self.state())
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Run {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Run")
            .field("until", &self.until)
            .finish_non_exhaustive()
    }
}

/// Open a `volatile-in-memory` session on `world`, which `config` founded,
/// with the limits `config` declares.
///
/// # Errors
/// As [`SessionConfig::from_config`] and [`open_with`].
pub fn open(
    world: World,
    config: &ValidatedConfig,
    session: SessionId,
) -> Result<(SessionOwner, SessionHandle), CoreError> {
    open_with(world, SessionConfig::from_config(session, config)?)
}

/// Open a `volatile-in-memory` session on `world` with explicit limits.
/// Its founding state is render epoch 1.
///
/// # Errors
/// [`CoreError::MissingCapability`] when `limits` declares
/// `write-ahead-before-apply`, which needs [`open_journaled`];
/// [`Publisher::new`]'s: fewer than two render slots, or a slot that cannot
/// be allocated.
pub fn open_with(
    world: World,
    limits: SessionConfig,
) -> Result<(SessionOwner, SessionHandle), CoreError> {
    let book = CommandBook::new(limits.session, limits.revision, limits.command_capacity);
    open_restored(world, book, limits)
}

/// Open a `volatile-in-memory` session on committed `world` and its `book`,
/// as [`crate::restore`] returns them from a checkpoint (FL-037). The
/// session continues as the uninterrupted one: pending commands apply at
/// their ticks, a retry returns its first outcome, and a sealed boundary
/// stays sealed. Its founding state is render epoch 1.
///
/// # Errors
/// As [`open_with`]; [`CoreError::StateConflict`] when the book, world and
/// limits disagree on the session or configuration.
pub fn open_restored(
    world: World,
    book: CommandBook,
    limits: SessionConfig,
) -> Result<(SessionOwner, SessionHandle), CoreError> {
    if limits.durability != Durability::Volatile {
        return Err(CoreError::MissingCapability {
            capability: format!(
                "{WRITE_AHEAD_BEFORE_APPLY} needs a command journal: open the session with \
                 open_journaled"
            ),
        });
    }
    agree(&world, &book, &limits, "open restored session")?;
    build(world, book, limits, None)
}

/// Refuse a `book`, `world` and `limits` that disagree on the session or the
/// configuration.
fn agree(
    world: &World,
    book: &CommandBook,
    limits: &SessionConfig,
    operation: &'static str,
) -> Result<(), CoreError> {
    if book.session() != limits.session
        || book.revision() != limits.revision
        || world.config().config_hash() != limits.revision
    {
        return Err(CoreError::StateConflict {
            operation,
            detail: format!(
                "the book is session {} running {}, the world runs {}, the limits name session \
                 {} running {}",
                book.session(),
                book.revision().to_hex(),
                world.config().config_hash().to_hex(),
                limits.session,
                limits.revision.to_hex()
            ),
        });
    }
    Ok(())
}

/// Open a `write-ahead-before-apply` session (FL-036) on committed `world`
/// and its `book`: a checkpoint's, or [`CommandBook::new`] for a founding
/// world. `writer` appends to the session's journal, which held `journal`
/// when it was opened ([`JournalWriter::spawn`]).
///
/// The journal is merged into the book ([`CommandBook::recover`]), and
/// every journaled outcome the book does not hold is replayed: the world
/// advances through the same boundaries with the same commands, each
/// journaled cancellation is applied at its boundary, and each outcome
/// must be reproduced exactly. The session then starts at the boundary the
/// replay reached, with every later acceptance pending and nothing
/// journaled twice. Its founding state is render epoch 1.
///
/// # Errors
/// [`CoreError::StateConflict`] when `limits` is not
/// `write-ahead-before-apply`, the book, world and limits disagree on the
/// session or configuration, the journal disagrees with the book, or the
/// replay does not reproduce the journal; the world's errors while
/// replaying; and as [`open_with`].
pub fn open_journaled(
    mut world: World,
    mut book: CommandBook,
    limits: SessionConfig,
    writer: JournalWriter,
    journal: &JournalContents,
) -> Result<(SessionOwner, SessionHandle), CoreError> {
    let conflict = |detail: String| CoreError::StateConflict {
        operation: "open journaled session",
        detail,
    };
    if limits.durability != Durability::WriteAhead {
        return Err(conflict(format!(
            "the session's durability is {:?}, not {WRITE_AHEAD_BEFORE_APPLY}",
            limits.durability
        )));
    }
    agree(&world, &book, &limits, "open journaled session")?;
    book.set_durability(Durability::WriteAhead);
    let replay = book.recover(journal, world.tick())?;
    replay_journal(&mut world, &mut book, replay)?;
    let durable = Durable {
        writer,
        waiting: VecDeque::new(),
        limit: limits
            .command_capacity
            .saturating_add(limits.control_capacity)
            .max(2),
        durable: journal.watermarks(),
        failed: None,
        uncertain: BTreeSet::new(),
    };
    build(world, book, limits, Some(durable))
}

/// The detail every unfinished command fails with when a numerical fault
/// stops `world`'s run.
fn stopped_detail(world: &World) -> Option<String> {
    world.fault().map(|fault| {
        format!(
            "the run stopped under stop-and-preserve advancing tick {}: {}",
            fault.tick.get(),
            fault.error
        )
    })
}

/// Advance `world` and `book` until they have reproduced `outcomes`, the
/// journal's outcomes after the book's, in order.
fn replay_journal(
    world: &mut World,
    book: &mut CommandBook,
    outcomes: Vec<(CommandId, Outcome)>,
) -> Result<(), CoreError> {
    let diverged = |detail: String| CoreError::StateConflict {
        operation: "replay command journal",
        detail,
    };
    let mut expected = outcomes.into_iter();
    let mut next = expected.next();
    // Check the outcomes `book` recorded from `from` on against the
    // journal's, in order.
    let mut check = |book: &CommandBook, from: u64, next: &mut Option<(CommandId, Outcome)>| {
        for cursor in from..book.outcome_cursor() {
            let produced = book.outcome_at(cursor);
            let journaled = next.take();
            if produced != journaled.as_ref().map(|(id, outcome)| (*id, outcome)) {
                return Err(diverged(format!(
                    "outcome {cursor}: the replay recorded {produced:?}, the journal holds \
                     {journaled:?}"
                )));
            }
            *next = expected.next();
        }
        Ok(())
    };
    while let Some((id, outcome)) = next.clone() {
        let committed = world.tick();
        let due = match &outcome {
            Outcome::Applied { tick, .. } => *tick,
            Outcome::Cancelled { boundary, .. } => *boundary,
            // A command fails when a fault stops the run with it unfinished,
            // at or before its effective tick.
            Outcome::Failed { .. } => book.accepted(id).map_or(committed, |a| a.effective),
        };
        if committed > due {
            return Err(diverged(format!(
                "the journal's outcome {} for {id} is due by tick {}, but the replay reached \
                 tick {} without it",
                outcome.cursor(),
                due.get(),
                committed.get()
            )));
        }
        if let Outcome::Cancelled {
            boundary, reason, ..
        } = outcome
            && boundary == committed
        {
            let from = book.outcome_cursor();
            let _ = book.cancel_for(id, committed, reason);
            // A cancellation the book cannot make (the command already
            // ended) records nothing; refuse it rather than wait for it.
            if book.outcome_cursor() == from {
                return Err(diverged(format!(
                    "the journal's cancellation of {id} at tick {} is not reproduced: the                      command already ended",
                    committed.get()
                )));
            }
            check(book, from, &mut next)?;
            continue;
        }
        if world.fault().is_some() {
            return Err(diverged(format!(
                "the run stopped at tick {} before the journal's outcome {}",
                committed.get(),
                outcome.cursor()
            )));
        }
        let sealed = book.seal(committed)?;
        let interventions: Vec<Intervention> = sealed.iter().map(|&(_, i)| i).collect();
        match world.advance_with(&interventions) {
            Ok(Advance::Ready(candidate)) => {
                let report = candidate.publish()?;
                let from = book.outcome_cursor();
                book.published(&report, &sealed)?;
                check(book, from, &mut next)?;
            }
            Ok(Advance::Paused(shortfall)) => {
                return Err(diverged(format!(
                    "tick {} paused on a capacity shortfall while replaying: {shortfall:?}",
                    committed.get()
                )));
            }
            Err(error) => {
                let Some(detail) = stopped_detail(world) else {
                    return Err(error);
                };
                let from = book.outcome_cursor();
                book.fail_unfinished(&detail);
                check(book, from, &mut next)?;
            }
        }
    }
    Ok(())
}

fn build(
    world: World,
    book: CommandBook,
    limits: SessionConfig,
    journal: Option<Durable>,
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
        book,
        grid,
        render,
        control,
        science,
        released,
        run: None,
        closing: Vec::new(),
        suspending: false,
        deferred: VecDeque::new(),
        journal,
        held: VecDeque::new(),
        hold: None,
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
    /// The boundary waits for a journal receipt or a hold; nothing changed.
    Held,
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
            hold: self.hold.as_ref().map(|&(tick, _)| tick),
            unfinished: self.book.unfinished(),
            admitted: self.book.admitted(),
            outcomes: self.book.outcome_cursor(),
            stopped: self.stopped(),
            render: self.render.stats().clone(),
            journal: self.journal.as_ref().map(Durable::state),
        }
    }

    /// Whether a hold keeps the committed boundary where it is.
    fn held_here(&self) -> bool {
        self.hold
            .as_ref()
            .is_some_and(|&(tick, _)| tick == self.world.tick())
    }

    /// Answer the hold's wait once the committed boundary is at its tick.
    fn reach_hold(&mut self) {
        if !self.held_here() {
            return;
        }
        let boundary = self.boundary();
        if let Some((_, waiting)) = &mut self.hold
            && let Some(waiting) = waiting.take()
        {
            let _ = waiting.send_blocking(Ok(boundary));
        }
    }

    /// Whether the committed boundary waits for a journal receipt or is
    /// held.
    fn holding(&self) -> bool {
        let committed = self.world.tick();
        self.held_here()
            || self
                .journal
                .as_ref()
                .is_some_and(|journal| journal.holds(committed))
    }

    /// Whether the owner may take another submission now.
    fn room(&self) -> bool {
        self.journal.as_ref().is_none_or(Durable::room)
    }

    fn settle_receipts(&mut self) {
        if let Some(journal) = &mut self.journal {
            journal.settle_ready(&self.book);
        }
    }

    /// Offer the committed state to the render feed. A busy feed leaves the
    /// boundary pending, and a failure is recorded in its counters; neither
    /// touches the world.
    fn offer_render(&mut self) {
        let _ = self.render.offer(&self.world);
    }

    fn admit(&mut self, submission: Submission, committed: Tick, stopped: bool) {
        admit_into(
            &mut self.book,
            self.journal.as_mut(),
            &self.grid,
            submission,
            committed,
            stopped,
            !self.closing.is_empty(),
        );
    }

    fn end_run(&mut self, end: RunEnd) {
        if let Some(run) = self.run.take()
            && let Some(ended) = run.ended
        {
            let _ = ended.send_blocking(end);
        }
    }

    /// Handle again every step and checkpoint that waited for the journal;
    /// one still blocked waits again.
    async fn release_held(&mut self, cx: &Cx) {
        for _ in 0..self.held.len() {
            let Some(message) = self.held.pop_front() else {
                break;
            };
            self.handle(cx, message).await;
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
                    Ticked::Held => {
                        self.held.push_back(Control::Step(reply));
                        return;
                    }
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
                let committed = self.world.tick();
                let _ = cancel_into(&mut self.book, self.journal.as_mut(), id, committed, reply);
            }
            Control::Status { id, reply } => {
                let _ = reply.send_blocking(self.book.status(id).cloned());
            }
            Control::DurableOutcome { id, reply } => {
                self.settle_receipts();
                let Some(journal) = &self.journal else {
                    let _ = reply.send_blocking(Err(CoreError::StateConflict {
                        operation: "read durable outcome",
                        detail: format!(
                            "a {VOLATILE_IN_MEMORY} session's outcomes are never durable"
                        ),
                    }));
                    return;
                };
                match self.book.status(id) {
                    Some(CommandStatus::Terminal(outcome))
                        if outcome.cursor() < journal.durable.outcomes =>
                    {
                        let _ = reply.send_blocking(Ok(Some(outcome.clone())));
                    }
                    Some(CommandStatus::Terminal(_)) => match &journal.failed {
                        Some(error) => {
                            let _ = reply.send_blocking(Err(error.clone()));
                        }
                        None => self.held.push_back(Control::DurableOutcome { id, reply }),
                    },
                    _ => {
                        let _ = reply.send_blocking(Ok(None));
                    }
                }
            }
            Control::Inspect(reply) => {
                let _ = reply.send_blocking(self.snapshot());
            }
            Control::InspectOrganism { id, reply } => {
                self.world.watch(Some(id));
                let _ = reply.send_blocking(self.world.organism_snapshot(id));
            }
            Control::OrganismWeights { id, reply } => {
                let _ = reply.send_blocking(self.world.organism_weights(id));
            }
            Control::MeasureRegion {
                center,
                radius,
                reply,
            } => {
                let _ = reply.send_blocking(self.world.measure_region(center, radius));
            }
            Control::Checkpoint(reply) => {
                self.settle_receipts();
                if self
                    .journal
                    .as_ref()
                    .is_some_and(|journal| !journal.waiting.is_empty())
                {
                    self.held.push_back(Control::Checkpoint(reply));
                    return;
                }
                let taken = crate::checkpoint::checkpoint(&self.world, &self.book).map(|bytes| {
                    SessionCheckpoint {
                        boundary: self.boundary(),
                        bytes,
                    }
                });
                let _ = reply.send_blocking(taken);
            }
            Control::Take(take) => {
                self.settle_receipts();
                if self
                    .journal
                    .as_ref()
                    .is_some_and(|journal| !journal.waiting.is_empty())
                {
                    self.held.push_back(Control::Take(take));
                    return;
                }
                take(Ok((&self.world, &self.book, self.boundary())));
            }
            Control::Hold { tick, set, reached } => {
                let boundary = self.boundary();
                if let Some(tick) = tick
                    && tick < boundary.tick
                {
                    let _ = set.send_blocking(Err(CoreError::StateConflict {
                        operation: "hold session",
                        detail: format!(
                            "the committed tick {} is past {}",
                            boundary.tick.get(),
                            tick.get()
                        ),
                    }));
                    return;
                }
                if let Some((_, Some(waiting))) = self.hold.take() {
                    let _ = waiting.send_blocking(Err(CoreError::StateConflict {
                        operation: "hold session",
                        detail: match tick {
                            Some(tick) => format!("the hold moved to tick {}", tick.get()),
                            None => "the hold was released".into(),
                        },
                    }));
                }
                self.hold = tick.map(|tick| (tick, reached));
                let _ = set.send_blocking(Ok(boundary));
                self.reach_hold();
            }
            Control::Close(reply) => {
                self.suspending = false;
                self.closing.push(reply);
            }
            Control::Suspend(reply) => {
                self.suspending = self.closing.is_empty() || self.suspending;
                self.closing.push(reply);
            }
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
            if self.held_here() {
                return Ticked::Held;
            }
            let committed = self.world.tick();
            if let Some(journal) = &mut self.journal {
                journal.settle_ready(&self.book);
                if let Some(error) = &journal.failed {
                    return Ticked::Failed(error.clone());
                }
                if journal.holds(committed) {
                    return Ticked::Held;
                }
            }
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
                suspending,
                deferred,
                run,
                journal,
                ..
            } = self;
            let candidate = match world.advance_with(&interventions) {
                Ok(Advance::Ready(candidate)) => candidate,
                Ok(Advance::Paused(shortfall)) => return Ticked::Paused(shortfall),
                Err(error) => {
                    if let Some(detail) = stopped_detail(world) {
                        let from = book.outcome_cursor();
                        book.fail_unfinished(&detail);
                        if let Some(journal) = journal {
                            journal.outcomes(book, from);
                        }
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
                        if cancel_into(book, journal.as_mut(), id, committed, reply) {
                            discard = true;
                        }
                    }
                    Control::Status { id, reply } => {
                        let _ = reply.send_blocking(book.status(id).cloned());
                    }
                    Control::Pause(reply) => pause.push(reply),
                    Control::Close(reply) => {
                        *suspending = false;
                        closing.push(reply);
                        discard = true;
                    }
                    Control::Suspend(reply) => {
                        *suspending = closing.is_empty() || *suspending;
                        closing.push(reply);
                        discard = true;
                    }
                    other => deferred.push_back(other),
                }
            }
            while journal.as_ref().is_none_or(Durable::room)
                && let Ok(submission) = science.try_recv()
            {
                admit_into(
                    book,
                    journal.as_mut(),
                    grid,
                    submission,
                    committed,
                    false,
                    !closing.is_empty(),
                );
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
            let from = book.outcome_cursor();
            if let Err(error) = book.published(&report, &sealed) {
                return Ticked::Failed(error);
            }
            if let Some(journal) = journal {
                journal.outcomes(book, from);
            }
            // Before anyone hears of the boundary: a viewer that is told of
            // a published tick finds its epoch unless every slot is busy,
            // and every epoch carries the tally through its tick.
            render.count(&report);
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

    /// Wait for the next message, control first, then a journal receipt,
    /// then a submission (when `science` is offered), then a lease release.
    /// A receive takes a message only when it completes, so a lane that
    /// loses keeps its message for the next call.
    async fn next_inbound(
        cx: &Cx,
        control: &mut mpsc::Receiver<Control>,
        science: Option<&mut mpsc::Receiver<Submission>>,
        released: &mut mpsc::Receiver<()>,
        receipts: Option<&mut mpsc::UnboundedReceiver<Receipt>>,
    ) -> Inbound {
        let mut control = pin!(control.recv(cx));
        let mut receipts = pin!(receipts.map(|lane| lane.recv(cx)));
        let mut science = pin!(science.map(|lane| lane.recv(cx)));
        let mut released = pin!(released.recv(cx));
        poll_fn(|task| {
            match control.as_mut().poll(task) {
                Poll::Ready(Ok(message)) => return Poll::Ready(Inbound::Control(message)),
                Poll::Ready(Err(_)) => return Poll::Ready(Inbound::Ended),
                Poll::Pending => {}
            }
            if let Some(receipts) = receipts.as_mut().as_pin_mut() {
                match receipts.poll(task) {
                    Poll::Ready(Ok(receipt)) => return Poll::Ready(Inbound::Receipt(receipt)),
                    Poll::Ready(Err(_)) => {
                        return Poll::Ready(Inbound::Receipt(Err(CoreError::StorageFailed {
                            operation: "run the journal writer",
                            path: "flife-journal thread".into(),
                            reason: "the writer thread has ended".into(),
                        })));
                    }
                    Poll::Pending => {}
                }
            }
            if let Some(science) = science.as_mut().as_pin_mut() {
                match science.poll(task) {
                    Poll::Ready(Ok(submission)) => {
                        return Poll::Ready(Inbound::Science(submission));
                    }
                    Poll::Ready(Err(_)) => return Poll::Ready(Inbound::Ended),
                    Poll::Pending => {}
                }
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
            // boundary is pending), journal receipts, steps and checkpoints
            // that waited for them, deferred control, then both lanes,
            // control first.
            self.offer_render();
            self.reach_hold();
            self.settle_receipts();
            self.release_held(cx).await;
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
            while self.room()
                && let Ok(submission) = self.science.try_recv()
            {
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
                // A held boundary waits below for its receipt, with control
                // live.
                if !self.holding() {
                    match self.tick(cx).await {
                        Ticked::Published(..) | Ticked::Held => {}
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
            }
            // A step handled above may have reached the hold.
            self.reach_hold();
            let room = self.room();
            let Self {
                control,
                science,
                released,
                journal,
                ..
            } = &mut self;
            let receipts = journal
                .as_mut()
                .filter(|journal| !journal.waiting.is_empty())
                .map(|journal| journal.writer.receipts());
            let inbound =
                Self::next_inbound(cx, control, room.then_some(science), released, receipts).await;
            match inbound {
                Inbound::Control(message) => self.handle(cx, message).await,
                Inbound::Science(submission) => {
                    let (committed, stopped) = (self.world.tick(), self.stopped());
                    self.admit(submission, committed, stopped);
                }
                Inbound::Receipt(receipt) => {
                    if let Some(journal) = &mut self.journal {
                        journal.settle(receipt, &self.book);
                    }
                }
                // A slot may be free: the loop's next offer retries.
                Inbound::Released => {}
                Inbound::Ended => break 'session,
            }
        }
        self.shut_down().await
    }

    async fn shut_down(mut self) -> SessionClosed {
        let boundary = self.boundary();
        self.offer_render();
        self.render.close();
        self.end_run(RunEnd::Closed(boundary));
        if !self.suspending {
            let from = self.book.outcome_cursor();
            self.book.close(boundary.tick);
            if let Some(journal) = &mut self.journal {
                journal.outcomes(&self.book, from);
            }
        }
        self.control.close();
        self.science.close();
        let mut queued: Vec<Control> = self.held.drain(..).collect();
        queued.extend(self.deferred.drain(..));
        while let Ok(message) = self.control.try_recv() {
            queued.push(message);
        }
        let mut durable = Vec::new();
        let mut submissions = Vec::new();
        while let Ok(submission) = self.science.try_recv() {
            submissions.push(submission);
        }
        // A closed session holds nothing: a wait on its hold ends.
        if let Some((_, Some(waiting))) = self.hold.take() {
            let _ = waiting.send_blocking(Err(closed()));
        }
        let snapshot = Snapshot {
            running: false,
            until: None,
            ..self.snapshot()
        };
        for message in queued {
            match message {
                Control::Pause(reply) | Control::Close(reply) | Control::Suspend(reply) => {
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
                Control::Checkpoint(reply) => {
                    let _ = reply.send_blocking(Err(closed()));
                }
                Control::Take(take) => take(Err(closed())),
                Control::Hold { set, reached, .. } => {
                    let _ = set.send_blocking(Err(closed()));
                    if let Some(reached) = reached {
                        let _ = reached.send_blocking(Err(closed()));
                    }
                }
                Control::Cancel { id, reply } => {
                    let decision = match self.book.status(id) {
                        Some(CommandStatus::Terminal(outcome)) => {
                            Cancellation::AlreadyTerminal(outcome.clone())
                        }
                        _ => Cancellation::Unknown,
                    };
                    let _ = reply.send_blocking(Ok(decision));
                }
                Control::Status { id, reply } => {
                    let _ = reply.send_blocking(self.book.status(id).cloned());
                }
                Control::DurableOutcome { id, reply } => durable.push((id, reply)),
                Control::Inspect(reply) => {
                    let _ = reply.send_blocking(snapshot.clone());
                }
                Control::InspectOrganism { id, reply } => {
                    let _ = reply.send_blocking(self.world.organism_snapshot(id));
                }
                Control::OrganismWeights { id, reply } => {
                    let _ = reply.send_blocking(self.world.organism_weights(id));
                }
                Control::MeasureRegion {
                    center,
                    radius,
                    reply,
                } => {
                    let _ = reply.send_blocking(self.world.measure_region(center, radius));
                }
            }
        }
        // A closed book answers a retry of an accepted identity with its
        // outcome and refuses everything else.
        for submission in submissions {
            admit_into(
                &mut self.book,
                self.journal.as_mut(),
                &self.grid,
                submission,
                boundary.tick,
                true,
                false,
            );
        }
        // Every record sent is answered before anyone hears the session
        // closed, so a closed session's journal holds all it will.
        if let Some(journal) = &mut self.journal {
            journal.drain(&self.book).await;
        }
        for (id, reply) in durable {
            let answer = match (&self.journal, self.book.status(id)) {
                (Some(journal), Some(CommandStatus::Terminal(outcome)))
                    if outcome.cursor() < journal.durable.outcomes =>
                {
                    Ok(Some(outcome.clone()))
                }
                (Some(journal), Some(CommandStatus::Terminal(_))) => {
                    Err(journal.failed.clone().unwrap_or_else(closed))
                }
                _ => Ok(None),
            };
            let _ = reply.send_blocking(answer);
        }
        for reply in self.closing.drain(..) {
            let _ = reply.send_blocking(boundary);
        }
        SessionClosed {
            render: self.render.stats().clone(),
            journal: self.journal.as_ref().map(Durable::state),
            world: self.world,
            book: self.book,
            boundary,
        }
    }
}

fn stopped_error(world: &World) -> CoreError {
    CoreError::StateConflict {
        operation: "advance session",
        detail: stopped_detail(world).unwrap_or_else(|| "the run stopped".to_owned()),
    }
}
