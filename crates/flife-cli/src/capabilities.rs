//! `flife capabilities`: what this machine and build can run, with explicit
//! refusals (plan §14.6, §20.5).
//!
//! [`capabilities()`] runs every controller route and reports what each
//! one did. It does not read what a route could do. Its one
//! [`CAPABILITIES_SCHEMA`] record holds:
//!
//! - `build`: this binary's version, target and profile.
//! - `host`: the CPU, logical cores, physical memory and operating system
//!   ([`Host::current`]).
//! - `world`: the world the routes ran.
//!   - Without a configuration, the reference preset
//!     (`presets/reference-small-world.toml`, compiled in). At
//!     [`PROBE_TICKS`] ticks its committed world is pinned: [`PRESET_DIGEST`],
//!     the world every qualified host reaches.
//!   - With one, that configuration's world.
//!   - Either way: the memory admission `flife run` would make with its
//!     default `auto` controllers, and whether the budget is within the
//!     host's physical memory.
//! - `controllers`: one entry per route, `cpu` (the reference), `torch` and
//!   `metal`. Each is founded afresh, configured as `flife run` configures
//!   it ([`configure`]), stepped, and compared with the reference's world at
//!   the same tick. A [`RouteStatus`] says what that established:
//!   - The reference is `verified` when it reaches the pinned world, and
//!     `reference` when nothing is pinned to compare it with.
//!   - Another route is `verified` only when it evaluated controllers
//!     itself and its world equals the reference's. It is `idle` when it
//!     agreed but deferred every evaluation to the reference: it did no
//!     work, so nothing about it is verified (plan §18.1: no placeholder
//!     backend may report success while doing no work).
//!   - `unavailable` (with the reason) when this host cannot open the
//!     route, such as Metal off macOS.
//!   - `refused` when the memory admission refuses the world on that route.
//!   - `disagrees` or `failed` for a defect to report.
//! - `auto`: the route `--controllers auto` takes for this world's workers
//!   on this host.
//! - `video`: whether `flife render --video`'s external encoder, ffmpeg
//!   with libx264, can be run ([`video_encoder`]).
//!
//! It opens no window: the Studio's window is not probed.

use std::num::NonZeroUsize;

use flife_backend_metal::ControllerChoice;
use flife_core::memory::MemoryAdmission;
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Stepped, World};
use serde::Serialize;

use crate::experiment::Host;
use crate::found;
use crate::render::video_encoder;
use crate::run::{ControllerSummary, configure};

/// The schema of the record `flife capabilities` prints.
pub const CAPABILITIES_SCHEMA: &str = "frankenlife.capabilities.v1";

/// Ticks each route runs unless told otherwise.
pub const PROBE_TICKS: u64 = 8;

/// The reference preset, the world probed without a configuration.
pub const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// The preset's committed world after [`PROBE_TICKS`] ticks: the world every
/// qualified host reaches, on every route and any number of workers.
pub const PRESET_DIGEST: &str = "82e0720d65135e2adaeae0ab58e26ce38a167d089d9f78e9cf8a29c8144df71e";

/// What to probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilitiesOptions {
    /// Ticks each route runs: at least one.
    pub ticks: u64,
    /// Threads for the world, replacing `[execution] cpu_workers`. No
    /// result depends on it.
    pub cpu_workers: Option<NonZeroUsize>,
}

impl Default for CapabilitiesOptions {
    fn default() -> Self {
        Self {
            ticks: PROBE_TICKS,
            cpu_workers: None,
        }
    }
}

/// This binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Build {
    /// The workspace version.
    pub version: &'static str,
    /// The target operating system.
    pub os: &'static str,
    /// The target architecture.
    pub arch: &'static str,
    /// `release` or `debug` (debug assertions on).
    pub profile: &'static str,
}

impl Build {
    /// This binary.
    #[must_use]
    pub const fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
        }
    }
}

/// A refusal or failure, as an error record gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Refusal {
    /// The error's class ([`CoreError::kind`]).
    pub kind: &'static str,
    /// The error.
    pub detail: String,
}

impl From<&CoreError> for Refusal {
    fn from(error: &CoreError) -> Self {
        Self {
            kind: error.kind(),
            detail: error.to_string(),
        }
    }
}

/// The world the routes ran.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProbedWorld {
    /// `preset` (the compiled-in reference preset) or `config`.
    pub source: &'static str,
    /// The configuration's name.
    pub name: String,
    /// Its canonical hash.
    pub config_hash: String,
    /// Founders.
    pub founders: u64,
    /// The world's CPU workers.
    pub cpu_workers: usize,
    /// Ticks each route was asked to run.
    pub ticks: u64,
    /// The world the reference must reach: [`PRESET_DIGEST`] for the
    /// preset at [`PROBE_TICKS`] ticks, otherwise none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned_digest: Option<&'static str>,
    /// The admission `flife run` makes with `auto` controllers and no run
    /// directory, or why it refuses.
    pub memory: Admitted,
    /// Whether `[execution] maximum_total_bytes` is within the host's
    /// physical memory; absent when the host's memory is unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_within_physical_memory: Option<bool>,
}

/// A memory admission, or its refusal.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Admitted {
    /// Admitted.
    Admitted(MemoryAdmission),
    /// Refused.
    Refused(Refusal),
}

/// What running one route established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouteStatus {
    /// The reference ran, with no pinned world to compare it with.
    Reference,
    /// The reference reached the pinned world; or another route evaluated
    /// controllers itself and reached the reference's world.
    Verified,
    /// It reached the reference's world but deferred every evaluation to
    /// the reference, so it did no work and nothing about it is verified.
    Idle,
    /// This host cannot open it.
    Unavailable,
    /// The memory admission refuses the world on it.
    Refused,
    /// It reached another world than the reference's (or the pinned one).
    Disagrees,
    /// It failed: an error while running, or a backend that stopped
    /// serving and left its ticks to the reference.
    Failed,
}

impl RouteStatus {
    /// Whether this is a defect to report: a route that disagrees or fails.
    #[must_use]
    pub const fn is_defect(self) -> bool {
        matches!(self, Self::Disagrees | Self::Failed)
    }
}

/// One controller route, run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Route {
    /// The `--controllers` value: `cpu`, `torch` or `metal`.
    pub route: &'static str,
    /// What running it established.
    pub status: RouteStatus,
    /// Why, for every status but `verified` and `reference`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// For a refusal or an error, its class.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'static str>,
    /// The committed tick it reached.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tick: Option<u64>,
    /// Its world's digest there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// What its controller backend did, as a run's end record gives it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub controllers: Option<ControllerSummary>,
}

/// What `--controllers auto` takes here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Auto {
    /// For a world on this many CPU workers.
    pub cpu_workers: usize,
    /// Whether this host opened a Metal device; absent when the Metal route
    /// was refused before it was opened.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metal_device: Option<bool>,
    /// The route it takes; absent when that is unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub takes: Option<&'static str>,
}

/// `flife render --video`'s external encoder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Video {
    /// `ffmpeg libx264`.
    pub encoder: &'static str,
    /// `available` or `unavailable`.
    pub status: &'static str,
    /// ffmpeg's version line, when it is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Why not, when it is unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The `flife capabilities` record.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Capabilities {
    /// [`CAPABILITIES_SCHEMA`].
    pub schema: &'static str,
    /// This binary.
    pub build: Build,
    /// This host.
    pub host: Host,
    /// The world the routes ran.
    pub world: ProbedWorld,
    /// `cpu`, `torch` and `metal`, run.
    pub controllers: Vec<Route>,
    /// What `--controllers auto` takes.
    pub auto: Auto,
    /// `flife render --video`'s encoder.
    pub video: Video,
}

impl Capabilities {
    /// Whether a route disagreed or failed: a defect to report.
    #[must_use]
    pub fn defect(&self) -> bool {
        self.controllers
            .iter()
            .any(|route| route.status.is_defect())
    }

    /// Whether the reference itself was refused: this world cannot run here.
    #[must_use]
    pub fn refused(&self) -> bool {
        self.controllers
            .iter()
            .any(|route| route.route == "cpu" && route.status == RouteStatus::Refused)
    }
}

/// The `--controllers` name of a route.
const fn route_name(choice: ControllerChoice) -> &'static str {
    match choice {
        ControllerChoice::Cpu => "cpu",
        ControllerChoice::Torch => "torch",
        ControllerChoice::Metal => "metal",
        ControllerChoice::Auto => "auto",
    }
}

/// What one route did.
enum Outcome {
    Ran {
        tick: u64,
        digest: String,
        controllers: ControllerSummary,
    },
    Refused(CoreError),
    Unavailable(CoreError),
    Failed(CoreError),
}

/// Found `config`'s world with `options`' workers, read `before` from it,
/// configure it for `choice` as `flife run` does, and step it
/// `options.ticks` ticks or until it pauses.
///
/// # Errors
/// Founding's and the workers': they are the world's, not the route's.
fn run_route<T>(
    config: &ValidatedConfig,
    choice: ControllerChoice,
    options: &CapabilitiesOptions,
    before: impl FnOnce(&World) -> T,
) -> Result<(T, Outcome), CoreError> {
    let mut world = found(config)?;
    if let Some(workers) = options.cpu_workers {
        world.set_cpu_workers(workers)?;
    }
    let read = before(&world);
    let outcome = match configure(&mut world, None, choice, None) {
        Ok(_) => step(&mut world, options.ticks),
        Err(error @ CoreError::BudgetExhausted { .. }) => Outcome::Refused(error),
        Err(error @ CoreError::MissingCapability { .. }) => Outcome::Unavailable(error),
        Err(error) => Outcome::Failed(error),
    };
    Ok((read, outcome))
}

/// Step `world` `ticks` ticks or until it pauses.
fn step(world: &mut World, ticks: u64) -> Outcome {
    for _ in 0..ticks {
        match world.step() {
            Ok(Stepped::Published(_)) => {}
            Ok(Stepped::Paused(_)) => break,
            Err(error) => return Outcome::Failed(error),
        }
    }
    Outcome::Ran {
        tick: world.tick().get(),
        digest: world.digest().to_hex(),
        controllers: ControllerSummary::of(world),
    }
}

/// A route's entry with no run to show.
fn without_run(choice: ControllerChoice, status: RouteStatus, error: &CoreError) -> Route {
    Route {
        route: route_name(choice),
        status,
        detail: Some(error.to_string()),
        kind: Some(error.kind()),
        tick: None,
        digest: None,
        controllers: None,
    }
}

/// The reference's entry: `verified` against `pinned`, if any.
fn reference_route(outcome: &Outcome, pinned: Option<&'static str>) -> Route {
    let choice = ControllerChoice::Cpu;
    match outcome {
        Outcome::Ran {
            tick,
            digest,
            controllers,
        } => {
            let (status, detail) = match pinned {
                None => (RouteStatus::Reference, None),
                Some(pinned) if pinned == digest => (RouteStatus::Verified, None),
                Some(pinned) => (
                    RouteStatus::Disagrees,
                    Some(format!(
                        "this host's reference reached {digest} at tick {tick}; every \
                         qualified host reaches {pinned}"
                    )),
                ),
            };
            Route {
                route: route_name(choice),
                status,
                detail,
                kind: None,
                tick: Some(*tick),
                digest: Some(digest.clone()),
                controllers: Some(controllers.clone()),
            }
        }
        Outcome::Refused(error) => without_run(choice, RouteStatus::Refused, error),
        Outcome::Unavailable(error) | Outcome::Failed(error) => {
            without_run(choice, RouteStatus::Failed, error)
        }
    }
}

/// Another route's entry, against the reference's run.
fn compared_route(choice: ControllerChoice, outcome: Outcome, reference: &Outcome) -> Route {
    let (tick, digest, controllers) = match outcome {
        Outcome::Ran {
            tick,
            digest,
            controllers,
        } => (tick, digest, controllers),
        Outcome::Refused(error) => return without_run(choice, RouteStatus::Refused, &error),
        Outcome::Unavailable(error) => {
            return without_run(choice, RouteStatus::Unavailable, &error);
        }
        Outcome::Failed(error) => return without_run(choice, RouteStatus::Failed, &error),
    };
    let Outcome::Ran {
        tick: reference_tick,
        digest: reference_digest,
        ..
    } = reference
    else {
        return Route {
            route: route_name(choice),
            status: RouteStatus::Failed,
            detail: Some("the reference did not run, so nothing compares this route".to_owned()),
            kind: None,
            tick: Some(tick),
            digest: Some(digest),
            controllers: Some(controllers),
        };
    };
    let (status, detail) = if (*reference_tick, reference_digest) != (tick, &digest) {
        (
            RouteStatus::Disagrees,
            Some(format!(
                "it reached {digest} at tick {tick}; the reference reached \
                 {reference_digest} at tick {reference_tick}"
            )),
        )
    } else if let Some(failure) = &controllers.failure {
        (
            RouteStatus::Failed,
            Some(format!(
                "the backend stopped serving, and the reference redid its ticks: {failure}"
            )),
        )
    } else if controllers.batched == 0 {
        (
            RouteStatus::Idle,
            Some(format!(
                "it deferred all {} evaluations to the reference, so it did no work",
                controllers.deferred
            )),
        )
    } else {
        (RouteStatus::Verified, None)
    };
    Route {
        route: route_name(choice),
        status,
        detail,
        kind: None,
        tick: Some(tick),
        digest: Some(digest),
        controllers: Some(controllers),
    }
}

/// Run every controller route on `config`'s world, or on the compiled-in
/// reference preset without one, and report this host and build (see the
/// [module documentation](mod@crate::capabilities)).
///
/// # Errors
/// The world's own: the preset or `config` failing to found, or
/// `options.cpu_workers` refused. A route's refusal, absence or failure is
/// reported in its entry, not returned.
pub fn capabilities(
    config: Option<&ValidatedConfig>,
    options: &CapabilitiesOptions,
) -> Result<Capabilities, CoreError> {
    let preset;
    let (config, source) = match config {
        Some(config) => (config, "config"),
        None => {
            preset = ValidatedConfig::from_toml(PRESET)?;
            (&preset, "preset")
        }
    };
    let pinned = (source == "preset" && options.ticks == PROBE_TICKS).then_some(PRESET_DIGEST);
    let host = Host::current();

    let ((cpu_workers, memory), reference) =
        run_route(config, ControllerChoice::Cpu, options, |world| {
            (
                world.cpu_workers(),
                crate::memory::admit(world, ControllerChoice::Auto, None, 0),
            )
        })?;
    let ((), torch) = run_route(config, ControllerChoice::Torch, options, |_| ())?;
    let ((), metal) = run_route(config, ControllerChoice::Metal, options, |_| ())?;

    let metal_device = match &metal {
        Outcome::Ran { .. } | Outcome::Failed(_) => Some(true),
        Outcome::Unavailable(_) => Some(false),
        Outcome::Refused(_) => None,
    };
    let auto = Auto {
        cpu_workers,
        metal_device,
        takes: metal_device
            .map(|device| route_name(ControllerChoice::auto_for(cpu_workers, device))),
    };
    let controllers = vec![
        reference_route(&reference, pinned),
        compared_route(ControllerChoice::Torch, torch, &reference),
        compared_route(ControllerChoice::Metal, metal, &reference),
    ];

    let model = config.config();
    let budget = model.execution.maximum_total_bytes;
    let world = ProbedWorld {
        source,
        name: model.name.clone(),
        config_hash: config.config_hash().to_hex(),
        founders: u64::from(model.population.initial_count),
        cpu_workers,
        ticks: options.ticks,
        pinned_digest: pinned,
        memory: match memory {
            Ok(admission) => Admitted::Admitted(admission),
            Err(error) => Admitted::Refused(Refusal::from(&error)),
        },
        budget_within_physical_memory: host.memory_bytes.map(|physical| budget <= physical),
    };
    let video = match video_encoder() {
        Ok(version) => Video {
            encoder: "ffmpeg libx264",
            status: "available",
            version: Some(version),
            detail: None,
        },
        Err(error) => Video {
            encoder: "ffmpeg libx264",
            status: "unavailable",
            version: None,
            detail: Some(error.to_string()),
        },
    };
    Ok(Capabilities {
        schema: CAPABILITIES_SCHEMA,
        build: Build::current(),
        host,
        world,
        controllers,
        auto,
        video,
    })
}

/// How a run is classified, for outcomes no healthy host produces: a route
/// that reaches another world, a backend that stops serving, a reference
/// that misses its pinned world. `tests/capabilities.rs` runs the routes
/// themselves.
#[cfg(test)]
mod tests {
    use flife_backend_metal::ControllerChoice;
    use flife_core::CoreError;

    use super::{Outcome, RouteStatus, compared_route, reference_route};
    use crate::run::ControllerSummary;

    const A: &str = "aa";
    const B: &str = "bb";

    fn summary(batched: u64, deferred: u64, failure: Option<&str>) -> ControllerSummary {
        ControllerSummary {
            backend: "test".to_owned(),
            batched,
            deferred,
            reference_ticks: u64::from(failure.is_some()),
            failure: failure.map(str::to_owned),
            resident_peak_bytes: 0,
        }
    }

    fn ran(tick: u64, digest: &str, controllers: ControllerSummary) -> Outcome {
        Outcome::Ran {
            tick,
            digest: digest.to_owned(),
            controllers,
        }
    }

    fn compared(outcome: Outcome, reference: &Outcome) -> RouteStatus {
        compared_route(ControllerChoice::Torch, outcome, reference).status
    }

    #[test]
    fn a_route_is_verified_only_when_it_worked_and_agreed() {
        let reference = ran(8, A, summary(0, 0, None));
        assert_eq!(
            compared(ran(8, A, summary(10, 2, None)), &reference),
            RouteStatus::Verified
        );
        assert_eq!(
            compared(ran(8, A, summary(0, 12, None)), &reference),
            RouteStatus::Idle
        );
        assert_eq!(
            compared(ran(8, B, summary(10, 0, None)), &reference),
            RouteStatus::Disagrees
        );
        assert_eq!(
            compared(ran(7, A, summary(10, 0, None)), &reference),
            RouteStatus::Disagrees,
            "the same digest at another tick"
        );
        // A backend that stopped serving left its ticks to the reference,
        // which reached the same world: a failure all the same.
        assert_eq!(
            compared(ran(8, A, summary(10, 0, Some("device lost"))), &reference),
            RouteStatus::Failed
        );
        let error = || CoreError::MissingCapability {
            capability: "x".to_owned(),
        };
        let refused = Outcome::Refused(CoreError::BudgetExhausted {
            budget: "b",
            required: 2,
            available: 1,
        });
        assert_eq!(
            compared(ran(8, A, summary(10, 0, None)), &refused),
            RouteStatus::Failed,
            "nothing compares it"
        );
        assert_eq!(
            compared(Outcome::Unavailable(error()), &reference),
            RouteStatus::Unavailable
        );
        assert_eq!(
            compared(Outcome::Failed(error()), &reference),
            RouteStatus::Failed
        );
        assert!(RouteStatus::Disagrees.is_defect() && RouteStatus::Failed.is_defect());
        assert!(!RouteStatus::Idle.is_defect() && !RouteStatus::Refused.is_defect());
    }

    #[test]
    fn the_reference_is_verified_only_against_its_pinned_world() {
        let run = ran(8, A, summary(0, 0, None));
        assert_eq!(reference_route(&run, None).status, RouteStatus::Reference);
        assert_eq!(reference_route(&run, Some(A)).status, RouteStatus::Verified);
        let missed = reference_route(&run, Some(B));
        assert_eq!(missed.status, RouteStatus::Disagrees);
        assert!(missed.detail.is_some_and(|detail| detail.contains(B)));
    }
}
