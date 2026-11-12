//! Plan §14.3 evidence (§21.2 INV-09): a session measures a disk of its
//! world at its committed boundary, paused or running, and the answer is
//! the measurement a world stepped directly to that tick gives. Measuring
//! changes nothing the session commits.
//!
//! The owner and its clients run as separate tasks in Asupersync's Lab, so
//! each seed is one reproducible interleaving.

mod common;

use std::future::Future;
use std::task::Poll;

use asupersync::runtime::yield_now;
use common::{Client, cx, ecology, found, lab, publish, reference};
use flife_core::{CoreError, Tick};
use flife_engine::{RegionMeasurement, World};
use flife_runtime::{Boundary, RunEnd};

/// The disks each test measures: across both seams, a small one, half the
/// world.
const DISKS: [([f64; 2], f64); 3] = [([0.5, 31.5], 6.0), ([20.0, 9.0], 2.5), ([16.0, 16.0], 16.0)];

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn measured(world: &World) -> Vec<RegionMeasurement> {
    DISKS
        .iter()
        .map(|&(center, radius)| world.measure_region(center, radius).expect("measures"))
        .collect()
}

/// Paused at tick 6, the session measures every disk as the direct world
/// at tick 6 does, refuses a disk wider than half the world as the world
/// does, moves nothing, and commits what a session never measured commits.
#[test]
fn a_paused_session_measures_a_disk_at_its_boundary() {
    let config = ecology(&[]);
    let direct = reference(&config, 6, &[]);
    let expected = measured(&direct);
    assert!(expected.iter().any(|measurement| measurement.organisms > 0));
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
                for (&(center, radius), expected) in DISKS.iter().zip(&expected) {
                    let answer = handle
                        .measure_region(&cx, center, radius)
                        .await
                        .expect("measures");
                    assert_eq!(&answer, expected, "{center:?} r {radius}");
                    assert_eq!(
                        answer.organism_energy.to_bits(),
                        expected.organism_energy.to_bits()
                    );
                    assert_eq!(answer.tick, boundary.tick);
                }
                let refused = handle.measure_region(&cx, [5.0, 5.0], 16.5).await;
                assert!(
                    matches!(
                        refused,
                        Err(CoreError::NumericalInvalid {
                            quantity: "measurement radius",
                            ..
                        })
                    ),
                    "{refused:?}"
                );
                // Measuring moved nothing.
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

/// While a bounded run is going, every measurement is answered between
/// ticks: it is the direct world's at the measurement's own tick, and the
/// run ends where an unmeasured run ends, at the same digest.
#[test]
fn a_running_session_answers_a_measurement_between_ticks() {
    const UNTIL: u64 = 24;
    let config = ecology(&[]);
    // The direct world's measurements at every tick to the run's end.
    let mut world = found(&config);
    let mut direct = vec![measured(&world)];
    while world.tick().get() < UNTIL {
        publish(&mut world, &[]);
        direct.push(measured(&world));
    }
    let mut ticks = Vec::new();
    for seed in 0..4 {
        let (mut results, closed) = lab(seed, &config, |handle| {
            vec![Box::pin(async move {
                let cx = cx();
                let running = handle
                    .start(&cx, Some(Tick::new(UNTIL)))
                    .await
                    .expect("start");
                let mut answers = Vec::new();
                for &(center, radius) in DISKS.iter().cycle() {
                    answers.push(
                        handle
                            .measure_region(&cx, center, radius)
                            .await
                            .expect("measures"),
                    );
                    if !handle.inspect(&cx).await.expect("inspect").running {
                        break;
                    }
                }
                let RunEnd::Reached(reached) = running.ended(&cx).await.expect("end") else {
                    panic!("the run did not reach its tick");
                };
                assert_eq!(reached.tick, Tick::new(UNTIL));
                handle.close(&cx).await.expect("close");
                answers
            }) as Client<Vec<RegionMeasurement>>]
        });
        let answers = results.remove(0);
        assert_eq!(closed.boundary.tick, Tick::new(UNTIL), "seed {seed}");
        assert_eq!(closed.world.digest(), world.digest(), "seed {seed}");
        for answer in &answers {
            let at = usize::try_from(answer.tick.get()).expect("a small tick");
            let disk = DISKS
                .iter()
                .position(|&(center, radius)| (center, radius) == (answer.center, answer.radius))
                .expect("one of the disks");
            assert_eq!(answer, &direct[at][disk], "seed {seed}");
            assert_eq!(
                answer.organism_energy.to_bits(),
                direct[at][disk].organism_energy.to_bits()
            );
        }
        ticks.extend(answers.iter().map(|answer| answer.tick.get()));
    }
    ticks.sort_unstable();
    ticks.dedup();
    assert!(
        ticks.len() >= 3,
        "measurements answered at several ticks: {ticks:?}"
    );
}

/// A measurement that reaches a running owner together with a close is
/// answered at the closed boundary, and a closed session refuses the next.
#[test]
fn a_closing_session_answers_a_queued_measurement_at_its_boundary() {
    const UNTIL: u64 = 30;
    let (center, radius) = DISKS[0];
    let config = ecology(&[]);
    let mut direct = found(&config);
    let mut at_tick = vec![(direct.digest(), measured(&direct)[0])];
    while direct.tick() < Tick::new(UNTIL) {
        publish(&mut direct, &[]);
        at_tick.push((direct.digest(), measured(&direct)[0]));
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
                let mut measure = std::pin::pin!(handle.measure_region(&cx, center, radius));
                let mut close = std::pin::pin!(closer.close(&cx));
                let (mut answered, mut closed) = (None, None);
                std::future::poll_fn(|context| {
                    if answered.is_none()
                        && let Poll::Ready(answer) = measure.as_mut().poll(context)
                    {
                        answered = Some(answer);
                    }
                    if closed.is_none()
                        && let Poll::Ready(answer) = close.as_mut().poll(context)
                    {
                        closed = Some(answer);
                    }
                    if answered.is_some() && closed.is_some() {
                        Poll::Ready(())
                    } else {
                        Poll::Pending
                    }
                })
                .await;
                let refused = handle.measure_region(&cx, center, radius).await;
                (
                    answered.expect("answered"),
                    closed.expect("answered"),
                    refused,
                )
            })
                as Client<(
                    Result<RegionMeasurement, CoreError>,
                    Result<Boundary, CoreError>,
                    Result<RegionMeasurement, CoreError>,
                )>]
        });
        let (answered, boundary, refused) = results.into_iter().next().expect("one client");
        let boundary = boundary.expect("closed");
        assert_eq!(closed.boundary, boundary);
        let (digest, expected) = &at_tick[usize::try_from(boundary.tick.get()).expect("small")];
        assert_eq!(closed.world.digest(), *digest, "seed {seed}");
        let answered = answered.expect("answered");
        assert_eq!(answered.tick, boundary.tick, "seed {seed}");
        assert_eq!(&answered, expected, "seed {seed}");
        assert!(refused.is_err(), "seed {seed}: {refused:?}");
    }
}
