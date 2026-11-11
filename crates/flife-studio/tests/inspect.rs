//! FL-021 evidence (plan §14.2): the Studio's session thread answers an
//! inspection intent with the organism at the session's committed boundary,
//! the snapshot a world stepped directly to that tick gives, and keeps the
//! tick it was selected at beside it. Inspecting changes nothing the session
//! commits.
//!
//! No window opens: the session thread is driven through its intent lane,
//! as the Studio host drives it after a selection.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::time::{Duration, Instant};

use flife_core::ids::{OrganismId, SessionId};
use flife_core::{Tick, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, OrganismSnapshot, Stepped, World};
use flife_studio::{Intent, SessionThread, Status};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// An identity no world here holds.
const UNKNOWN: OrganismId = OrganismId::new(1 << 40);

/// The preset at 32 × 32 world units with 96 founders.
fn small() -> ValidatedConfig {
    let mut lines: Vec<String> = PRESET.lines().map(str::to_owned).collect();
    for (section, key, value) in [
        ("world", "width", "32.0"),
        ("world", "height", "32.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", "96"),
        ("population", "maximum_count", "1024"),
    ] {
        let header = format!("[{section}]");
        let at = lines
            .iter()
            .position(|line| line.trim() == header)
            .unwrap_or_else(|| panic!("no section {header}"));
        let line = lines[at + 1..]
            .iter()
            .take_while(|line| !line.starts_with('['))
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .map(|offset| at + 1 + offset)
            .unwrap_or_else(|| panic!("no key {key} in {header}"));
        lines[line] = format!("{key} = {value}");
    }
    ValidatedConfig::from_toml(&lines.join("\n")).expect("a valid configuration")
}

fn found(config: &ValidatedConfig) -> World {
    World::found(
        config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds")
}

fn step(world: &mut World) {
    assert!(matches!(
        world.step().expect("steps"),
        Stepped::Published(_)
    ));
}

fn spawn(config: &ValidatedConfig) -> SessionThread {
    let (owner, handle) =
        flife_runtime::open(found(config), config, SessionId::new(1)).expect("the session opens");
    SessionThread::spawn(owner, handle).expect("the session thread starts")
}

/// Statuses until `done` holds for the ones gathered, within 60 seconds.
fn gather(session: &SessionThread, done: impl Fn(&[Status]) -> bool) -> Vec<Status> {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut statuses = Vec::new();
    while !done(&statuses) {
        assert!(Instant::now() < deadline, "only {statuses:?}");
        statuses.extend(session.statuses());
        std::thread::sleep(Duration::from_millis(1));
    }
    statuses
}

/// Each inspection's organism, the tick it was seen at and its snapshot.
fn inspections(statuses: &[Status]) -> Vec<(OrganismId, Tick, Option<OrganismSnapshot>)> {
    statuses
        .iter()
        .filter_map(|status| match status {
            Status::Inspected {
                organism,
                seen,
                snapshot,
            } => Some((*organism, *seen, snapshot.as_deref().cloned())),
            _ => None,
        })
        .collect()
}

/// Paused at tick 4, every inspection is answered at tick 4 with the direct
/// world's snapshot, and the selection tick comes back unchanged.
#[test]
fn a_paused_studio_session_answers_an_inspection_at_its_boundary() {
    let config = small();
    let mut direct = found(&config);
    for _ in 0..4 {
        step(&mut direct);
    }
    let session = spawn(&config);
    for _ in 0..4 {
        session.send(Intent::Step).expect("offered");
    }
    let stepped = gather(&session, |statuses| {
        statuses
            .iter()
            .filter(|status| matches!(status, Status::Stepped { .. }))
            .count()
            == 4
    });
    assert!(
        stepped.contains(&Status::Stepped { tick: Tick::new(4) }),
        "{stepped:?}"
    );
    let mut asked: Vec<OrganismId> = direct.population().ids().take(8).collect();
    asked.push(UNKNOWN);
    // The epoch it was "seen" on is the host's to report; the session keeps it.
    let seen = Tick::new(3);
    for &organism in &asked {
        session
            .send(Intent::Inspect { organism, seen })
            .expect("offered");
    }
    let answered = inspections(&gather(&session, |statuses| {
        inspections(statuses).len() == asked.len()
    }));
    for ((organism, at, snapshot), asked) in answered.iter().zip(&asked) {
        assert_eq!(organism, asked);
        assert_eq!(*at, seen);
        assert_eq!(
            *snapshot,
            direct.organism_snapshot(*asked).expect("inspects"),
            "{asked:?}"
        );
    }
    assert!(answered[..8].iter().all(|(_, _, snapshot)| {
        snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.tick == Tick::new(4))
    }));
    assert_eq!(answered[8].2, None);
    let closed = session.close().expect("closes");
    assert_eq!(closed.boundary.tick, Tick::new(4));
    assert_eq!(closed.world.digest(), direct.digest());
}

/// While the session runs, every answer is the direct world's snapshot at
/// the answer's own tick; an organism answered with none is dead by the
/// end; and the session ends where an uninspected world ends.
#[test]
fn a_running_studio_session_answers_inspections_between_ticks() {
    let config = small();
    let founders: Vec<OrganismId> = found(&config).population().ids().collect();
    let session = spawn(&config);
    session.send(Intent::Resume).expect("offered");
    let mut offered = 0;
    for organism in founders.iter().cycle().take(48) {
        if session
            .send(Intent::Inspect {
                organism: *organism,
                seen: Tick::new(0),
            })
            .is_ok()
        {
            offered += 1;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    session.send(Intent::Pause).expect("offered");
    let statuses = gather(&session, |statuses| {
        inspections(statuses).len() == offered
            && statuses
                .iter()
                .any(|status| matches!(status, Status::Paused { .. }))
    });
    let closed = session.close().expect("closes");
    let mut answered = inspections(&statuses);
    assert!(!answered.is_empty());
    answered.sort_by_key(|(_, _, snapshot)| snapshot.as_ref().map(|snapshot| snapshot.tick));
    let mut direct = found(&config);
    for (organism, _, snapshot) in &answered {
        if let Some(snapshot) = snapshot {
            while direct.tick() < snapshot.tick {
                step(&mut direct);
            }
            assert_eq!(snapshot.organism.id, *organism);
            assert_eq!(
                direct
                    .organism_snapshot(*organism)
                    .expect("inspects")
                    .as_ref(),
                Some(snapshot)
            );
        }
    }
    while direct.tick() < closed.boundary.tick {
        step(&mut direct);
    }
    assert_eq!(closed.world.digest(), direct.digest());
    for (organism, _, snapshot) in &answered {
        if snapshot.is_none() {
            assert_eq!(
                direct.organism_snapshot(*organism),
                Ok(None),
                "{organism:?}"
            );
        }
    }
}
