//! FL-021 evidence (plan §14.2, §21.2 INV-09): a session inspects one
//! organism at its committed boundary, paused, running or closing, and the
//! answer is the snapshot a world stepped directly to that tick gives.
//! Inspecting changes nothing the session commits.
//!
//! The owner and its clients run as separate tasks in Asupersync's Lab, so
//! each seed is one reproducible interleaving.

mod common;

use std::future::Future;
use std::task::Poll;

use asupersync::runtime::yield_now;
use common::{Client, cx, ecology, found, lab, publish, reference};
use flife_core::ids::OrganismId;
use flife_core::{CoreError, Tick};
use flife_engine::{OrganismSnapshot, World};
use flife_runtime::{Boundary, RunEnd};

/// An identity no world here holds.
const UNKNOWN: OrganismId = OrganismId::new(1 << 40);

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn snapshot(world: &World, id: OrganismId) -> Option<OrganismSnapshot> {
    world.organism_snapshot(id).expect("inspects")
}

/// Paused at tick 6, the session answers every live organism with the
/// direct world's snapshot at tick 6, an unknown identity with none, and
/// commits what a session never inspected commits.
#[test]
fn a_paused_session_inspects_an_organism_at_its_boundary() {
    let config = ecology(&[]);
    let direct = reference(&config, 6, &[]);
    let expected: Vec<(OrganismId, OrganismSnapshot)> = direct
        .population()
        .ids()
        .filter_map(|id| snapshot(&direct, id).map(|snapshot| (id, snapshot)))
        .collect();
    assert!(expected.len() >= 40, "{}", expected.len());
    assert!(expected.iter().any(|(_, s)| s.decoded.is_some()));
    for seed in 0..4 {
        let expected = expected.clone();
        let (results, closed) = lab(seed, &config, |handle| {
            vec![Box::pin(async move {
                let cx = cx();
                for _ in 0..6 {
                    handle.step(&cx).await.expect("step");
                }
                let boundary = handle.inspect(&cx).await.expect("inspect").boundary;
                assert_eq!(boundary.tick, Tick::new(6));
                for (id, snapshot) in &expected {
                    let answer = handle.inspect_organism(&cx, *id).await.expect("inspects");
                    assert_eq!(answer.as_ref(), Some(snapshot), "organism {id:?}");
                    assert_eq!(answer.map(|s| s.tick), Some(boundary.tick));
                }
                assert_eq!(
                    handle
                        .inspect_organism(&cx, UNKNOWN)
                        .await
                        .expect("inspects"),
                    None
                );
                // Inspecting moved nothing.
                assert_eq!(
                    handle.inspect(&cx).await.expect("inspect").boundary,
                    boundary
                );
                handle.close(&cx).await.expect("close")
            }) as Client<Boundary>]
        });
        assert_eq!(closed.boundary, results[0]);
        assert_eq!(closed.boundary.tick, Tick::new(6));
        assert_eq!(closed.world.digest(), direct.digest(), "seed {seed}");
    }
}

/// While a bounded run is going, every inspection is answered between
/// ticks: a snapshot is the direct world's at the snapshot's own tick, an
/// organism the session answers with none is dead by the run's end, and
/// the run ends where an uninspected run ends, at the same digest.
#[test]
fn a_running_session_answers_an_inspection_between_ticks() {
    const UNTIL: u64 = 24;
    let config = ecology(&[]);
    let founders: Vec<OrganismId> = found(&config).population().ids().collect();
    let end = reference(&config, UNTIL, &[]);
    let mut answered: Vec<OrganismSnapshot> = Vec::new();
    for seed in 0..4 {
        let founders = founders.clone();
        let (mut results, closed) = lab(seed, &config, |handle| {
            vec![Box::pin(async move {
                let cx = cx();
                let running = handle
                    .start(&cx, Some(Tick::new(UNTIL)))
                    .await
                    .expect("start");
                let (mut snapshots, mut gone) = (Vec::new(), Vec::new());
                for id in founders.iter().cycle() {
                    match handle.inspect_organism(&cx, *id).await.expect("inspects") {
                        Some(snapshot) => {
                            assert_eq!(snapshot.organism.id, *id);
                            snapshots.push(snapshot);
                        }
                        None => gone.push(*id),
                    }
                    if !handle.inspect(&cx).await.expect("inspect").running {
                        break;
                    }
                }
                let RunEnd::Reached(reached) = running.ended(&cx).await.expect("end") else {
                    panic!("the run did not reach its tick");
                };
                assert_eq!(reached.tick, Tick::new(UNTIL));
                handle.close(&cx).await.expect("close");
                (snapshots, gone)
            })
                as Client<(Vec<OrganismSnapshot>, Vec<OrganismId>)>]
        });
        let (snapshots, gone) = results.remove(0);
        assert_eq!(closed.boundary.tick, Tick::new(UNTIL), "seed {seed}");
        assert_eq!(closed.world.digest(), end.digest(), "seed {seed}");
        for id in gone {
            assert_eq!(
                snapshot(&end, id),
                None,
                "seed {seed}: {id:?} answered none"
            );
        }
        answered.extend(snapshots);
    }
    // Every snapshot against the direct world at its own tick.
    answered.sort_by_key(|snapshot| snapshot.tick);
    let mut direct = found(&config);
    for answer in &answered {
        while direct.tick() < answer.tick {
            publish(&mut direct, &[]);
        }
        assert_eq!(
            snapshot(&direct, answer.organism.id).as_ref(),
            Some(answer),
            "tick {:?}",
            answer.tick
        );
    }
    let ticks: std::collections::BTreeSet<Tick> =
        answered.iter().map(|snapshot| snapshot.tick).collect();
    assert!(
        ticks
            .iter()
            .any(|tick| Tick::new(0) < *tick && *tick < Tick::new(UNTIL)),
        "answered only at {ticks:?}"
    );
}

/// An inspection that reaches a running owner together with a close is
/// answered at the closed boundary, and a closed session refuses the next.
#[test]
fn a_closing_session_answers_a_queued_inspection_at_its_boundary() {
    const UNTIL: u64 = 30;
    let config = ecology(&[]);
    // A founder alive at every tick the run can close at, and the direct
    // world's digest and snapshot of it at each of those ticks.
    let mut direct = found(&config);
    let mut alive: Vec<OrganismId> = direct.population().ids().collect();
    while direct.tick() < Tick::new(UNTIL) {
        publish(&mut direct, &[]);
        alive.retain(|id| direct.population().get(*id).is_some());
    }
    let id = *alive.first().expect("a founder lives through the run");
    let mut direct = found(&config);
    let mut at_tick = vec![(direct.digest(), snapshot(&direct, id))];
    while direct.tick() < Tick::new(UNTIL) {
        publish(&mut direct, &[]);
        at_tick.push((direct.digest(), snapshot(&direct, id)));
    }
    for seed in 0..6 {
        let (results, closed) = lab(seed, &config, |handle| {
            vec![Box::pin(async move {
                let cx = cx();
                handle
                    .start(&cx, Some(Tick::new(UNTIL)))
                    .await
                    .expect("start");
                while !handle.inspect(&cx).await.expect("inspect").running {
                    yield_now().await;
                }
                for _ in 0..(2 * seed) {
                    yield_now().await;
                }
                // Both requests are queued before the owner runs.
                let closer = handle.clone();
                let mut inspect = std::pin::pin!(handle.inspect_organism(&cx, id));
                let mut close = std::pin::pin!(closer.close(&cx));
                let (mut inspected, mut closed) = (None, None);
                std::future::poll_fn(|context| {
                    if inspected.is_none()
                        && let Poll::Ready(answer) = inspect.as_mut().poll(context)
                    {
                        inspected = Some(answer);
                    }
                    if closed.is_none()
                        && let Poll::Ready(answer) = close.as_mut().poll(context)
                    {
                        closed = Some(answer);
                    }
                    if inspected.is_some() && closed.is_some() {
                        Poll::Ready(())
                    } else {
                        Poll::Pending
                    }
                })
                .await;
                let refused = handle.inspect_organism(&cx, id).await;
                (
                    inspected.expect("answered"),
                    closed.expect("answered"),
                    refused,
                )
            })
                as Client<(
                    Result<Option<OrganismSnapshot>, CoreError>,
                    Result<Boundary, CoreError>,
                    Result<Option<OrganismSnapshot>, CoreError>,
                )>]
        });
        let (inspected, boundary, refused) = results.into_iter().next().expect("one client");
        let boundary = boundary.expect("closed");
        assert_eq!(closed.boundary, boundary);
        let (digest, expected) = &at_tick[usize::try_from(boundary.tick.get()).expect("small")];
        assert_eq!(closed.world.digest(), *digest, "seed {seed}");
        let inspected = inspected.expect("answered").expect("alive");
        assert_eq!(inspected.tick, boundary.tick, "seed {seed}");
        assert_eq!(Some(&inspected), expected.as_ref(), "seed {seed}");
        assert!(refused.is_err(), "seed {seed}: {refused:?}");
    }
}
