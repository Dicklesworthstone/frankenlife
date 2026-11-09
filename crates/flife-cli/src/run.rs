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

use std::time::Instant;

use asupersync::Cx;
use asupersync::runtime::RuntimeBuilder;
use flife_core::ids::SessionId;
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, World};
use flife_runtime::{SessionClosed, SessionConfig, StepOutcome, open_with};
use serde::Serialize;

use crate::observe::{Survey, Tally};

/// The schema of every record a run emits.
pub const RUN_SCHEMA: &str = "frankenlife.run.v1";

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
        /// Wall-clock seconds the run took. Not scientific state.
        wall_seconds: f64,
    },
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

/// The [`Record::Start`] for running `config` for `ticks`.
#[must_use]
pub fn start(config: &ValidatedConfig, ticks: u64) -> Record {
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
    }
}

/// Run `config` headlessly for `options.ticks` through a session owner,
/// passing every record to `emit` in order.
///
/// # Errors
/// The world's founding errors, the session's opening errors (such as a
/// refused command durability), and [`CoreError::MissingCapability`] when
/// the native runtime cannot be built or its tasks spawned.
pub fn run(
    config: &ValidatedConfig,
    options: RunOptions,
    mut emit: impl FnMut(&Record) + Send + 'static,
) -> Result<Finished, CoreError> {
    // The session's limits are checked before the world is founded, so an
    // unadmitted durability is refused without founding 4,096 organisms.
    let limits = SessionConfig::from_config(RUN_SESSION, config)?;
    let (owner, handle) = open_with(found(config)?, limits)?;
    emit(&start(config, options.ticks));
    let runtime = RuntimeBuilder::current_thread()
        .build()
        .map_err(|error| missing(format!("an Asupersync runtime: {error}")))?;
    let spawner = runtime.handle();
    let clock = Instant::now();
    let (stop, tally, closed, emit) = runtime.block_on(async move {
        let owner = spawner
            .try_spawn(async move {
                let cx = Cx::current().ok_or_else(|| missing("a task context".into()))?;
                Ok::<_, CoreError>(owner.run(&cx).await)
            })
            .map_err(|error| missing(format!("the owner task: {error}")))?;
        let client = spawner
            .try_spawn(async move {
                let cx = Cx::current().ok_or_else(|| missing("a task context".into()))?;
                let mut tally = Tally::default();
                let stop = loop {
                    if tally.ticks >= options.ticks {
                        break Stop::Reached;
                    }
                    match handle.step(&cx).await? {
                        StepOutcome::Published { report, .. } => {
                            tally.record(&report);
                            if options.every > 0 && tally.ticks % options.every == 0 {
                                emit(&Record::Progress {
                                    schema: RUN_SCHEMA,
                                    tick: report.tick.get(),
                                    live: report.live,
                                    tally: Box::new(tally.clone()),
                                    wall_seconds: clock.elapsed().as_secs_f64(),
                                });
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
                handle.close(&cx).await?;
                Ok::<_, CoreError>((stop, tally, emit))
            })
            .map_err(|error| missing(format!("the client task: {error}")))?;
        let client = client.await;
        let closed = owner.await?;
        let (stop, tally, emit) = client?;
        Ok::<_, CoreError>((stop, tally, closed, emit))
    })?;
    let mut emit = emit;
    let survey = Survey::of(&closed.world);
    emit(&Record::End {
        schema: RUN_SCHEMA,
        stop: stop.clone(),
        tick: closed.boundary.tick.get(),
        digest: closed.world.digest().to_hex(),
        tally: Box::new(tally.clone()),
        survey: Box::new(survey.clone()),
        wall_seconds: clock.elapsed().as_secs_f64(),
    });
    Ok(Finished {
        stop,
        tally,
        survey,
        closed,
    })
}

fn missing(capability: String) -> CoreError {
    CoreError::MissingCapability { capability }
}
