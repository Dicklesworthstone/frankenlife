//! A headless run through the session owner (plan §12.1, §14.6; FL-015).
//!
//! [`run`] founds the configured world, opens an FL-014 session, and drives
//! it from a client task on Asupersync's native current-thread runtime. The
//! client steps the session one tick at a time over the control lane, so
//! every published tick's report reaches it, folds each report into a
//! [`Tally`], and emits a progress [`Record`] every `every` ticks. Then it
//! closes the session, and the committed world the owner returns is
//! [`Survey`]ed directly.
//!
//! The scientific content of the records (everything except
//! `wall_seconds`) depends only on the configuration and the tick count.
//!
//! [`run_with`] adds scheduled commands and, with a run directory
//! ([`crate::persist`], FL-037), checkpoints and the command journal.
//!
//! With `metrics_every`, a [`Tracker`] folds every report too and a
//! [`Record::Metrics`] follows at every multiple of it and at the end when
//! ticks remain ([`crate::metrics`], FL-040).

use std::num::NonZeroUsize;
use std::path::Path;
use std::time::Instant;

use asupersync::Cx;
use asupersync::runtime::RuntimeBuilder;
use flife_backend_metal::ControllerChoice;
use flife_core::ids::SessionId;
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Arrangement, CPU_REFERENCE, ControllerReport, NumericalProfile, World};
use flife_runtime::{
    Admission, Command, CommandBook, Durability, SessionClosed, SessionConfig, SessionHandle,
    SessionOwner, StepOutcome, VOLATILE_IN_MEMORY, WRITE_AHEAD_BEFORE_APPLY, open_with, restore,
};
use serde::Serialize;

use crate::branch::{BranchParent, ForkRecord, FoundingChunks, Inherited};
use crate::metrics::{Declaration, Metrics, Tracker};
use crate::observe::{Survey, Tally};
use crate::persist::{Checkpoints, Injection, Persistence};

/// The schema of every record a run emits.
pub const RUN_SCHEMA: &str = "frankenlife.run.v1";

/// One worker per available core, at most 16: the `auto` worker count.
#[must_use]
pub fn auto_workers() -> NonZeroUsize {
    std::thread::available_parallelism()
        .map_or(NonZeroUsize::MIN, |cores| cores.min(AUTO_WORKER_CEILING))
}

/// The most workers `auto` picks.
pub const AUTO_WORKER_CEILING: NonZeroUsize = match NonZeroUsize::new(16) {
    Some(ceiling) => ceiling,
    None => NonZeroUsize::MIN,
};

/// The session a headless run opens.
pub const RUN_SESSION: SessionId = SessionId::new(1);

/// What to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunOptions {
    /// Ticks to publish.
    pub ticks: u64,
    /// Emit a progress record every this many published ticks; zero for
    /// none.
    pub every: u64,
    /// Threads for the organism phases, replacing `[execution] cpu_workers`.
    /// No result depends on it.
    pub cpu_workers: Option<NonZeroUsize>,
    /// Where due controllers are evaluated. No result depends on it.
    pub controllers: ControllerChoice,
    /// Emit a metrics record at every multiple of this committed tick and
    /// at the end; zero for none.
    pub metrics_every: u64,
}

/// What the controller backend did over a run (FL-030). Not scientific
/// state: it says where the arithmetic ran, so a record shows whether the
/// GPU did the work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ControllerSummary {
    /// The backend, or `cpu-reference`.
    pub backend: String,
    /// Evaluations the backend returned.
    pub batched: u64,
    /// Evaluations it deferred to the reference.
    pub deferred: u64,
    /// Ticks the reference redid whole after an error on the batched path.
    pub reference_ticks: u64,
    /// Why the backend stopped serving, if it failed.
    pub failure: Option<String>,
}

impl ControllerSummary {
    /// `world`'s controller report.
    #[must_use]
    pub fn of(world: &World) -> Self {
        let ControllerReport {
            backend,
            batched,
            deferred,
            reference_ticks,
            failure,
        } = world.controller_report().clone();
        Self {
            backend: backend.unwrap_or_else(|| CPU_REFERENCE.to_owned()),
            batched,
            deferred,
            reference_ticks,
            failure,
        }
    }
}

/// Give `world` the backend `choice` selects on this host.
///
/// # Errors
/// [`ControllerChoice::backend`]'s.
pub fn install(world: &mut World, choice: ControllerChoice) -> Result<(), CoreError> {
    world.set_controller_backend(choice.backend(world.cpu_workers())?);
    Ok(())
}

/// Why the run stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "end")]
pub enum Stop {
    /// It published every requested tick.
    Reached,
    /// A capacity shortfall paused it at an unchanged boundary.
    CapacityPaused {
        /// What could not be covered.
        detail: String,
    },
    /// A tick failed; `stopped` when a numerical fault stopped the run.
    Failed {
        /// The error's class.
        kind: &'static str,
        /// The error.
        detail: String,
        /// Whether the run stopped under `stop-and-preserve`.
        stopped: bool,
    },
}

/// One line of a run's NDJSON output.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "record")]
pub enum Record {
    /// What is about to run.
    Start {
        /// [`RUN_SCHEMA`].
        schema: &'static str,
        /// The configuration's name.
        name: String,
        /// Its canonical hash.
        config_hash: String,
        /// Its world seed.
        seed: u64,
        /// Founders.
        founders: u64,
        /// The field grid, `[x, y]` cells.
        grid: [u64; 2],
        /// Δt as `[numerator, denominator]` simulation-seconds.
        timestep: [u64; 2],
        /// Ticks requested.
        ticks: u64,
        /// Threads the organism phases use.
        cpu_workers: usize,
        /// Where due controllers are evaluated: a backend and device, or
        /// `cpu-reference`.
        controllers: String,
        /// The metrics records to follow and how they are computed.
        #[serde(skip_serializing_if = "Option::is_none")]
        metrics: Option<Declaration>,
    },
    /// The run so far.
    Progress {
        /// [`RUN_SCHEMA`].
        schema: &'static str,
        /// The committed tick.
        tick: u64,
        /// Live organisms.
        live: usize,
        /// The run's tally so far.
        tally: Box<Tally>,
        /// Wall-clock seconds since the run started. Not scientific state.
        wall_seconds: f64,
    },
    /// Incremental metrics and lineage summaries (FL-040).
    Metrics(Box<Metrics>),
    /// How the run ended, and the committed world's raw state.
    End {
        /// [`RUN_SCHEMA`].
        schema: &'static str,
        /// Why it stopped.
        stop: Stop,
        /// The committed tick.
        tick: u64,
        /// The committed world's digest.
        digest: String,
        /// The whole run's tally.
        tally: Box<Tally>,
        /// The committed world, surveyed directly.
        survey: Box<Survey>,
        /// What the controller backend did. Not scientific state.
        controllers: ControllerSummary,
        /// Wall-clock seconds the run took. Not scientific state.
        wall_seconds: f64,
        /// For a run directory: what is durable, reported apart from the
        /// committed tick (§8.3).
        #[serde(skip_serializing_if = "Option::is_none")]
        persisted: Option<Persisted>,
    },
    /// A checkpoint published to the run directory's chunk store and
    /// indexed in its catalog (FL-037).
    Checkpoint {
        /// [`RUN_SCHEMA`].
        schema: &'static str,
        /// The committed tick it holds.
        tick: u64,
        /// The committed epoch it holds.
        epoch: u64,
        /// Its chunk-store sequence number.
        sequence: u64,
        /// The container's SHA-256.
        sha256: String,
        /// The container's size.
        bytes: u64,
        /// Chunks written, and chunks already stored and shared.
        chunks: [usize; 2],
        /// Older checkpoints removed under `retain_recent_checkpoints`.
        retired: Vec<u64>,
    },
    /// A branch made from a parent run's checkpoint (FL-039).
    Branch {
        /// [`RUN_SCHEMA`].
        schema: &'static str,
        /// The branch's name.
        name: String,
        /// Its session.
        session: u64,
        /// The checkpoint it starts from.
        parent: BranchParent,
        /// Every fork it descends through, root first; the last is its own.
        lineage: Vec<ForkRecord>,
        /// The commands and outcomes it inherited.
        inherited: Inherited,
        /// Its own commands' identities.
        commands: Vec<u64>,
        /// Its founding checkpoint.
        checkpoint: CheckpointAt,
        /// The founding checkpoint container's SHA-256.
        sha256: String,
        /// The container's size.
        bytes: u64,
        /// The founding checkpoint's chunks written and linked from the
        /// parent's store.
        chunks: FoundingChunks,
        /// The committed world's digest, the parent's at the checkpoint.
        digest: String,
    },
    /// Where a resumed run starts (FL-037).
    Resume {
        /// [`RUN_SCHEMA`].
        schema: &'static str,
        /// The checkpoint it recovered, or none for the founding world.
        checkpoint: Option<CheckpointAt>,
        /// Newer checkpoints passed over, newest first.
        skipped: Vec<SkippedCheckpoint>,
        /// The committed tick after replaying the command journal past the
        /// checkpoint.
        tick: u64,
        /// The committed world's digest there.
        digest: String,
        /// The journal's durable watermarks, for a write-ahead run.
        journal: Option<JournalMarks>,
    },
}

/// A checkpoint by its sequence number and tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CheckpointAt {
    /// Its chunk-store sequence number.
    pub sequence: u64,
    /// The committed tick it holds.
    pub tick: u64,
}

/// A checkpoint recovery passed over, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkippedCheckpoint {
    /// Its sequence number.
    pub sequence: u64,
    /// The error's class.
    pub kind: &'static str,
    /// The error.
    pub reason: String,
}

/// The command journal's durable watermarks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct JournalMarks {
    /// Acceptances durable, by admission order.
    pub admitted: u64,
    /// Terminal outcomes durable, by cursor.
    pub outcomes: u64,
}

/// What a run directory holds durably when a run ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Persisted {
    /// The run directory.
    pub run_dir: String,
    /// The command durability: `volatile-in-memory` or
    /// `write-ahead-before-apply`.
    pub durability: &'static str,
    /// The newest checkpoint published, the tick a crash now recovers from
    /// before replaying the journal.
    pub checkpoint: Option<CheckpointAt>,
    /// The journal's durable watermarks, for a write-ahead run.
    pub journal: Option<JournalMarks>,
}

/// What a run returns besides its records.
#[derive(Debug)]
pub struct Finished {
    /// Why it stopped.
    pub stop: Stop,
    /// The whole run's tally.
    pub tally: Tally,
    /// The committed world, surveyed.
    pub survey: Survey,
    /// What the session owner returned: the committed world and its book.
    pub closed: SessionClosed,
}

/// Found `config`'s world in the free-list arrangement with the reference
/// F32 profile.
///
/// # Errors
/// [`World::found`]'s errors.
pub fn found(config: &ValidatedConfig) -> Result<World, CoreError> {
    World::found(
        config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32()?,
    )
}

/// The [`Record::Start`] for running `config` for `ticks`, declaring a
/// metrics record every `metrics_every` ticks unless it is zero.
#[must_use]
pub fn start(
    config: &ValidatedConfig,
    ticks: u64,
    cpu_workers: usize,
    controllers: String,
    metrics_every: u64,
) -> Record {
    let model = config.config();
    let timestep = config.report().timestep;
    Record::Start {
        schema: RUN_SCHEMA,
        name: model.name.clone(),
        config_hash: config.config_hash().to_hex(),
        seed: model.seed,
        founders: u64::from(model.population.initial_count),
        grid: [
            u64::from(model.world.grid_width),
            u64::from(model.world.grid_height),
        ],
        timestep: [
            u64::from(timestep.numerator()),
            u64::from(timestep.denominator()),
        ],
        ticks,
        cpu_workers,
        controllers,
        metrics: (metrics_every > 0).then(|| Declaration::new(metrics_every)),
    }
}

/// Run `config` headlessly for `options.ticks` through a session owner,
/// passing every record to `emit` in order.
///
/// # Errors
/// As [`run_with`].
pub fn run(
    config: &ValidatedConfig,
    options: RunOptions,
    emit: impl FnMut(&Record) + Send + 'static,
) -> Result<Finished, CoreError> {
    run_with(config, options, &[], None, emit)
}

/// Run `config` headlessly for `options.ticks` through a session owner,
/// first submitting `injections` (commands 1, 2, … in order) and, with
/// `persistence`, keeping the run in a new run directory: the command
/// journal for a `write-ahead-before-apply` configuration, and a
/// checkpoint at every multiple of `persistence.checkpoint_every` and at
/// the end ([`crate::persist`]).
///
/// # Errors
/// The world's founding errors; the session's opening errors;
/// [`CoreError::MissingCapability`] for a `write-ahead-before-apply`
/// configuration without a run directory, or when the native runtime
/// cannot be built or its tasks spawned; [`CoreError::StateConflict`] for a
/// scheduled command that is not admitted, or a run directory that already
/// holds a run or other files; the store's and catalog's errors.
pub fn run_with(
    config: &ValidatedConfig,
    options: RunOptions,
    injections: &[Injection],
    persistence: Option<&Persistence>,
    emit: impl FnMut(&Record) + Send + 'static,
) -> Result<Finished, CoreError> {
    // The session's limits are checked before the world is founded, so an
    // unadmitted durability is refused without founding 4,096 organisms.
    let limits = SessionConfig::from_config(RUN_SESSION, config)?;
    if persistence.is_none() && limits.durability != Durability::Volatile {
        return Err(CoreError::MissingCapability {
            capability: format!(
                "a {WRITE_AHEAD_BEFORE_APPLY} run needs a command journal: give it a run \
                 directory (flife run --run-dir), or run {VOLATILE_IN_MEMORY}"
            ),
        });
    }
    let commands = crate::persist::commands(injections);
    if let Some(persistence) = persistence {
        crate::persist::create(&persistence.run_dir, config)?;
    }
    let mut world = found(config)?;
    configure(&mut world, options.cpu_workers, options.controllers)?;
    let cpu_workers = world.cpu_workers();
    let controllers = ControllerSummary::of(&world).backend;
    let metrics = tracker(&world, options.metrics_every)?;
    let (owner, handle, checkpoints) = match persistence {
        Some(persistence) => {
            let checkpoints = Checkpoints::open(persistence, config, RUN_SESSION)?;
            let book = CommandBook::new(limits.session, limits.revision, limits.command_capacity);
            let (owner, handle) =
                crate::persist::open_session(world, book, limits, &persistence.run_dir)?;
            (owner, handle, Some(checkpoints))
        }
        None => {
            let (owner, handle) = open_with(world, limits)?;
            (owner, handle, None)
        }
    };
    let clock = Instant::now();
    let plan = Plan {
        start: start(
            config,
            options.ticks,
            cpu_workers,
            controllers,
            options.metrics_every,
        ),
        until: options.ticks,
        every: options.every,
        commands,
        resumed: None,
        metrics,
    };
    let driven = drive(owner, handle, plan, checkpoints, emit, clock)?;
    Ok(finish(
        driven,
        persistence.map(|persistence| persistence.run_dir.as_path()),
        limits.durability,
        clock,
    ))
}

/// A metrics tracker over `world` for a record every `every` ticks; none
/// for zero.
///
/// # Errors
/// [`Tracker::new`]'s.
pub(crate) fn tracker(world: &World, every: u64) -> Result<Option<Tracker>, CoreError> {
    (every > 0).then(|| Tracker::new(world, every)).transpose()
}

/// Give `world` `cpu_workers`, if set, and the controller backend `choice`
/// selects on this host.
///
/// # Errors
/// [`World::set_cpu_workers`]'s and [`install`]'s.
pub fn configure(
    world: &mut World,
    cpu_workers: Option<NonZeroUsize>,
    choice: ControllerChoice,
) -> Result<(), CoreError> {
    if let Some(workers) = cpu_workers {
        world.set_cpu_workers(workers)?;
    }
    install(world, choice)
}

/// What a session is driven to do.
pub(crate) struct Plan {
    /// The [`Record::Start`] emitted first.
    pub start: Record,
    /// The committed tick to reach.
    pub until: u64,
    /// Emit a progress record at every multiple of this tick; zero for
    /// none.
    pub every: u64,
    /// Commands to submit, in order, before the first step. Each must be
    /// admitted afresh.
    pub commands: Vec<Command>,
    /// For a resumed run: the checkpoint it recovered and those it passed
    /// over, reported in a [`Record::Resume`] before anything else.
    pub resumed: Option<(Option<CheckpointAt>, Vec<SkippedCheckpoint>)>,
    /// The metrics tracker, from the world the session was opened with.
    pub metrics: Option<Tracker>,
}

/// What driving a session returned.
pub(crate) struct Driven<E> {
    pub stop: Stop,
    pub tally: Tally,
    pub closed: SessionClosed,
    pub emit: E,
    pub checkpoints: Option<Checkpoints>,
}

/// Drive the session from a client task on Asupersync's native
/// current-thread runtime: submit the plan's commands, then step one tick at
/// a time until the committed tick reaches `plan.until`, a capacity pause
/// or a failure, emitting a progress record at every multiple of
/// `plan.every`. With `checkpoints`, publish one at each tick it is due (the
/// starting tick included, once the commands are admitted) and at the end
/// unless the run stopped on a fault, then suspend the session; without
/// them, close it. With a metrics tracker, fold every report into it and
/// emit a metrics record at each tick it is due and at the end when ticks
/// remain; a tracker behind the session's starting tick (a resumed session
/// replayed its journal) starts again from a checkpoint of it.
pub(crate) fn drive<E>(
    owner: SessionOwner,
    handle: SessionHandle,
    plan: Plan,
    checkpoints: Option<Checkpoints>,
    mut emit: E,
    clock: Instant,
) -> Result<Driven<E>, CoreError>
where
    E: FnMut(&Record) + Send + 'static,
{
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
        let client = spawner
            .try_spawn(async move {
                let cx = Cx::current().ok_or_else(|| missing("a task context".into()))?;
                let mut checkpoints = checkpoints;
                let mut metrics = plan.metrics;
                let viewer = match metrics {
                    Some(_) => Some(handle.feed().viewer(1)?),
                    None => None,
                };
                let snapshot = handle.inspect(&cx).await?;
                let mut committed = snapshot.boundary.tick.get();
                if plan.until < committed {
                    return Err(CoreError::StateConflict {
                        operation: "continue the run",
                        detail: format!(
                            "the run is already at tick {committed}, past tick {}",
                            plan.until
                        ),
                    });
                }
                emit(&plan.start);
                if let Some((checkpoint, skipped)) = plan.resumed {
                    emit(&Record::Resume {
                        schema: RUN_SCHEMA,
                        checkpoint,
                        skipped,
                        tick: committed,
                        digest: snapshot.digest.to_hex(),
                        journal: snapshot.journal.map(|journal| JournalMarks {
                            admitted: journal.durable.admitted,
                            outcomes: journal.durable.outcomes,
                        }),
                    });
                }
                if let Some(tracker) = &mut metrics
                    && tracker.tick() != committed
                {
                    let taken = handle.checkpoint(&cx).await?;
                    let restored = restore(&taken.bytes, NumericalProfile::reference_f32()?)?;
                    tracker.refound(&restored.world)?;
                }
                for command in plan.commands {
                    let admission = handle.submit(&cx, command).await?;
                    if !matches!(admission, Admission::Accepted { retry: false, .. }) {
                        return Err(CoreError::StateConflict {
                            operation: "submit a scheduled command",
                            detail: format!(
                                "{} was not admitted afresh: {admission:?}",
                                command.id
                            ),
                        });
                    }
                }
                // The starting state, commands included, when it is due and
                // not already stored.
                if let Some(checkpoints) = &mut checkpoints
                    && checkpoints.due(committed)
                    && checkpoints.last_tick() != Some(committed)
                {
                    let taken = handle.checkpoint(&cx).await?;
                    emit(&checkpoints.publish(&taken)?);
                }
                let mut tally = Tally::default();
                let stop = loop {
                    if committed >= plan.until {
                        break Stop::Reached;
                    }
                    match handle.step(&cx).await? {
                        StepOutcome::Published { report, .. } => {
                            tally.record(&report);
                            committed = report.tick.get();
                            if let Some(tracker) = &mut metrics {
                                tracker.record(&report)?;
                                if tracker.due(committed)
                                    && let Some(viewer) = &viewer
                                {
                                    let epoch = viewer.latest()?;
                                    emit(&Record::Metrics(Box::new(tracker.sample(&epoch)?)));
                                }
                            }
                            if plan.every > 0 && committed.is_multiple_of(plan.every) {
                                emit(&Record::Progress {
                                    schema: RUN_SCHEMA,
                                    tick: committed,
                                    live: report.live,
                                    tally: Box::new(tally.clone()),
                                    wall_seconds: clock.elapsed().as_secs_f64(),
                                });
                            }
                            if let Some(checkpoints) = &mut checkpoints
                                && checkpoints.due(committed)
                            {
                                let taken = handle.checkpoint(&cx).await?;
                                emit(&checkpoints.publish(&taken)?);
                            }
                        }
                        StepOutcome::Paused { shortfall, .. } => {
                            break Stop::CapacityPaused {
                                detail: format!("{shortfall:?}"),
                            };
                        }
                        StepOutcome::Failed { error, stopped, .. } => {
                            break Stop::Failed {
                                kind: error.kind(),
                                detail: error.to_string(),
                                stopped,
                            };
                        }
                    }
                };
                // The last window, when ticks remain in it.
                if let Some(tracker) = &mut metrics
                    && tracker.pending() > 0
                    && let Some(viewer) = &viewer
                {
                    let epoch = viewer.latest()?;
                    emit(&Record::Metrics(Box::new(tracker.sample(&epoch)?)));
                }
                // The last committed state, unless a fault stopped the run.
                if let Some(checkpoints) = &mut checkpoints
                    && !matches!(stop, Stop::Failed { stopped: true, .. })
                    && checkpoints.last_tick() != Some(committed)
                {
                    let taken = handle.checkpoint(&cx).await?;
                    emit(&checkpoints.publish(&taken)?);
                }
                // A kept run is suspended, not closed: commands still
                // pending stay pending in its checkpoint and journal, to
                // apply when it is resumed past their ticks.
                if checkpoints.is_some() {
                    handle.suspend(&cx).await?;
                } else {
                    handle.close(&cx).await?;
                }
                Ok::<_, CoreError>((stop, tally, emit, checkpoints))
            })
            .map_err(|error| missing(format!("the client task: {error}")))?;
        let client = client.await;
        let closed = owner.await?;
        let (stop, tally, emit, checkpoints) = client?;
        Ok(Driven {
            stop,
            tally,
            closed,
            emit,
            checkpoints,
        })
    })
}

/// Emit the [`Record::End`] for `driven` and return what the run did.
pub(crate) fn finish<E: FnMut(&Record)>(
    driven: Driven<E>,
    run_dir: Option<&Path>,
    durability: Durability,
    clock: Instant,
) -> Finished {
    let Driven {
        stop,
        tally,
        closed,
        mut emit,
        checkpoints,
    } = driven;
    let survey = Survey::of(&closed.world);
    let persisted = run_dir.map(|run_dir| Persisted {
        run_dir: run_dir.display().to_string(),
        durability: match durability {
            Durability::Volatile => VOLATILE_IN_MEMORY,
            Durability::WriteAhead => WRITE_AHEAD_BEFORE_APPLY,
        },
        checkpoint: checkpoints.as_ref().and_then(Checkpoints::last),
        journal: closed.journal.as_ref().map(|journal| JournalMarks {
            admitted: journal.durable.admitted,
            outcomes: journal.durable.outcomes,
        }),
    });
    emit(&Record::End {
        schema: RUN_SCHEMA,
        stop: stop.clone(),
        tick: closed.boundary.tick.get(),
        digest: closed.world.digest().to_hex(),
        tally: Box::new(tally.clone()),
        survey: Box::new(survey.clone()),
        controllers: ControllerSummary::of(&closed.world),
        wall_seconds: clock.elapsed().as_secs_f64(),
        persisted,
    });
    Finished {
        stop,
        tally,
        survey,
        closed,
    }
}

fn missing(capability: String) -> CoreError {
    CoreError::MissingCapability { capability }
}
