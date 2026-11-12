//! Plan §2.2 ("Follow descendants of a selected organism") through the
//! Studio's session thread: a follow intent is answered with the line the
//! session's world follows from its committed boundary, and the session's
//! epochs carry that line, at the tick its organism dies passing to the
//! heir a directly stepped world names. The death is a removal brush's: in
//! the reference world's first 400 ticks no founder dies with a child
//! alive. A session refuses to follow an organism its world does not
//! hold alive, and a world that does not trace its ancestry follows
//! nothing. Following changes nothing the session commits.
//!
//! No window opens: the session thread is driven through its intent lane,
//! as the Studio host drives it after a selection.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "test helpers: a failure in one is a test failure; figures are printed"
)]

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use flife_core::ids::{OrganismId, SessionId};
use flife_core::{Tick, ValidatedConfig};
use flife_engine::{Arrangement, Followed, NumericalProfile, Stepped, TickReport, World};
use flife_studio::{Intent, SessionThread, Status, Stroke};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// How far the direct world looks for a parent.
const UNTIL: u64 = 400;

fn config() -> ValidatedConfig {
    ValidatedConfig::from_toml(PRESET).expect("the preset is valid")
}

/// The preset's world, tracing its ancestry when `traced`, as the Studio's
/// does.
fn found(config: &ValidatedConfig, traced: bool) -> World {
    let mut world = World::found(
        config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds");
    if traced {
        world.trace_ancestry().expect("traces");
    }
    world
}

fn step(world: &mut World) -> TickReport {
    match world.step_with(&[]).expect("steps") {
        Stepped::Published(report) => *report,
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    }
}

fn spawn(world: World, config: &ValidatedConfig) -> SessionThread {
    let (owner, handle) =
        flife_runtime::open(world, config, SessionId::new(1)).expect("the session opens");
    SessionThread::spawn(owner, handle).expect("the session thread starts")
}

/// Statuses until `done` holds for the ones gathered, within 120 seconds.
fn gather(
    session: &SessionThread,
    mut statuses: Vec<Status>,
    done: impl Fn(&[Status]) -> bool,
) -> Vec<Status> {
    let deadline = Instant::now() + Duration::from_secs(120);
    while !done(&statuses) {
        assert!(Instant::now() < deadline, "only {statuses:?}");
        statuses.extend(session.statuses());
        std::thread::sleep(Duration::from_millis(1));
    }
    statuses
}

/// Step the paused session to committed tick `to` from `from`, with at
/// most eight steps outstanding.
fn step_to(session: &SessionThread, from: u64, to: u64, statuses: Vec<Status>) -> Vec<Status> {
    let published = |statuses: &[Status]| {
        statuses
            .iter()
            .filter_map(|status| match status {
                Status::Stepped { tick } => Some(tick.get()),
                Status::StepHeld { .. } => panic!("a step was held: {status:?}"),
                _ => None,
            })
            .max()
            .unwrap_or(from)
            .max(from)
    };
    let mut statuses = statuses;
    let mut sent = from;
    while published(&statuses) < to {
        while sent < to && sent < published(&statuses) + 8 {
            session.send(Intent::Step).expect("offered");
            sent += 1;
        }
        let before = published(&statuses);
        statuses = gather(session, statuses, |statuses| published(statuses) > before);
    }
    assert_eq!(published(&statuses), to);
    statuses
}

/// The newest epoch's tick and the line it carries.
fn latest(session: &SessionThread) -> (Tick, Option<Followed>) {
    let lease = session
        .feed()
        .viewer(1)
        .expect("a viewer")
        .latest()
        .expect("an epoch");
    (lease.tick(), lease.followed())
}

/// The preset's traced world stepped directly to the first tick, before
/// [`UNTIL`], at which a live organism with a child alive stands alone
/// within `radius` world units, and that organism.
fn a_parent_alone(config: &ValidatedConfig, radius: f64) -> (World, OrganismId) {
    let mut world = found(config, true);
    let mut parents = BTreeSet::new();
    loop {
        let population = world.population();
        let alone = parents.iter().copied().find(|&parent| {
            population.get(parent).is_some_and(|organism| {
                population.iter().any(|child| child.parent == Some(parent))
                    && world
                        .measure_region(organism.position, radius)
                        .expect("measures")
                        .organisms
                        == 1
            })
        });
        if let Some(parent) = alone {
            return (world, parent);
        }
        assert!(world.tick().get() < UNTIL, "no parent alone by {UNTIL}");
        let report = step(&mut world);
        parents.extend(report.births.iter().map(|birth| birth.parent));
    }
}

/// The removal of the organisms within `radius` of `organism`.
fn removal(world: &World, organism: OrganismId, radius: f64) -> Stroke {
    Stroke::Removal {
        center: world.population().get(organism).expect("alive").position,
        radius,
    }
}

/// Plan §2.2: pause, follow an organism with a child alive, remove it with
/// the removal brush, and step. The follow is answered with the line begun
/// at the committed boundary (an identity the world does not hold is
/// refused); the epoch of the removal's tick carries the line passed to its
/// heir, the eldest child alive, exactly as a directly stepped world
/// following it names; and the session closes on that world's digest.
#[test]
fn a_studio_session_follows_a_line_its_epochs_carry() {
    let config = config();
    let (mut direct, parent) = a_parent_alone(&config, 0.01);
    let at = direct.tick();
    println!("{parent} has a child alive at {at:?}");
    let session = spawn(found(&config, true), &config);
    let statuses = step_to(&session, 0, at.get(), Vec::new());
    assert_eq!(latest(&session), (at, None), "nothing followed yet");
    let unknown = OrganismId::new(1 << 40);
    for organism in [unknown, parent] {
        session
            .send(Intent::Follow { organism, seen: at })
            .expect("offered");
    }
    let followed = |statuses: &[Status]| {
        statuses
            .iter()
            .filter(|status| {
                matches!(
                    status,
                    Status::Following { .. }
                        | Status::Refused {
                            intent: Intent::Follow { .. },
                            ..
                        }
                )
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    let statuses = gather(&session, statuses, |statuses| followed(statuses).len() == 2);
    let answers = followed(&statuses);
    assert!(
        matches!(&answers[0], Status::Refused { intent: Intent::Follow { organism, .. }, kind: "state-conflict", .. } if *organism == unknown),
        "{answers:?}"
    );
    let begun = Followed {
        origin: parent,
        organism: parent,
        since: at,
        handoffs: 0,
        alive: true,
    };
    assert_eq!(
        answers[1],
        Status::Following {
            seen: at,
            line: begun,
        }
    );

    // The removal takes the parent alone; the line passes to its heir.
    let stroke = removal(&direct, parent, 0.01);
    session
        .send(Intent::Intervene { stroke, seen: at })
        .expect("offered");
    let statuses = gather(&session, statuses, |statuses| {
        statuses
            .iter()
            .any(|status| matches!(status, Status::Submitted { effective, .. } if *effective == at))
    });
    assert_eq!(direct.follow(Some(parent)).expect("follows"), Some(begun));
    let report = match direct.step_with(&[stroke.intervention()]).expect("steps") {
        Stepped::Published(report) => report,
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    };
    assert_eq!(report.interventions[0].removed, 1);
    assert!(report.deaths.iter().any(|death| death.id == parent));
    step_to(&session, at.get(), at.get() + 1, statuses);
    let line = direct.followed().expect("a line");
    let eldest = direct
        .population()
        .iter()
        .filter(|organism| organism.parent == Some(parent))
        .map(|organism| organism.id)
        .min()
        .expect("a child alive");
    assert_eq!(
        line,
        Followed {
            organism: eldest,
            since: report.tick,
            handoffs: 1,
            ..begun
        }
    );
    assert_eq!(latest(&session), (report.tick, Some(line)));
    let closed = session.close().expect("closes");
    assert_eq!(closed.boundary.tick, report.tick);
    assert_eq!(closed.world.digest(), direct.digest());
    assert_eq!(closed.world.followed(), Some(line));
}

#[test]
fn a_session_whose_world_traces_no_ancestry_follows_nothing() {
    let config = config();
    let world = found(&config, false);
    let organism = world.population().ids().next().expect("an organism");
    let session = spawn(world, &config);
    session
        .send(Intent::Follow {
            organism,
            seen: Tick::ZERO,
        })
        .expect("offered");
    let statuses = gather(&session, Vec::new(), |statuses| !statuses.is_empty());
    assert!(
        matches!(&statuses[0], Status::Refused { kind: "state-conflict", detail, .. } if detail.contains("does not trace its ancestry")),
        "{statuses:?}"
    );
    session.send(Intent::Step).expect("offered");
    gather(&session, statuses, |statuses| {
        statuses.contains(&Status::Stepped { tick: Tick::new(1) })
    });
    assert_eq!(latest(&session), (Tick::new(1), None));
    session.close().expect("closes");
}
