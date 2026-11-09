//! FL-014 evidence (plan §12.1–§12.4, §12.7, Appendix C.8 and C.14): the
//! Asupersync session owner. A responsive paused control loop, idempotent
//! command identities, exact effective ticks and cancellation boundaries.
//!
//! The owner and its clients run as separate tasks in Asupersync's Lab, so
//! each seed is one reproducible interleaving; one test runs the same
//! protocol on the native runtime. Every world a session commits is compared
//! with a world stepped directly with the same interventions at the same
//! ticks, so an applied command is checked by its effect on the science,
//! not only by the owner's own report.

mod common;

use std::collections::BTreeSet;

use asupersync::runtime::{RuntimeBuilder, yield_now};
use common::{
    Client, SESSION, at, bounded_lab, command, config, cx, ecology, found, inject, lab, publish,
    reference,
};
use flife_core::canonical::Sha256Digest;
use flife_core::ids::CommandId;
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_engine::{Advance, Intervention, Stepped};
use flife_runtime::{
    Admission, Boundary, CancelReason, Cancellation, CommandStatus, Durability, Outcome, Rejection,
    RunEnd, SessionConfig, StepOutcome, Target, open,
};

#[expect(
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
fn accepted(admission: &Admission) -> Tick {
    match admission {
        Admission::Accepted { accepted, .. } => accepted.effective,
        Admission::Rejected(rejection) => panic!("rejected: {rejection:?}"),
    }
}

#[expect(
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
fn published(outcome: &StepOutcome) -> (&Boundary, &[CommandId], Tick) {
    match outcome {
        StepOutcome::Published {
            boundary,
            applied,
            report,
        } => (boundary, applied, report.from),
        other => panic!("not published: {other:?}"),
    }
}

/// `volatile-in-memory` is admitted with `[execution]`'s capacities.
/// Appendix A's `write-ahead-before-apply` is a valid durability, but a
/// session holding it needs a command journal: `open` refuses it, and
/// FL-036's `open_journaled` (tested in `flife-store`) opens it.
#[test]
fn the_session_admits_volatile_in_memory_and_write_ahead_needs_a_journal() {
    let appendix = config(&[]);
    let limits = SessionConfig::from_config(SESSION, &appendix).expect("write-ahead");
    assert_eq!(limits.durability, Durability::WriteAhead);
    let error = open(found(&appendix), &appendix, SESSION).expect_err("needs a journal");
    assert!(
        matches!(&error, CoreError::MissingCapability { capability } if capability.contains("open_journaled")),
        "{error}"
    );
    let volatile = ecology(&[
        ("execution", "command_queue_capacity", "5"),
        ("execution", "control_queue_capacity", "3"),
    ]);
    let limits = SessionConfig::from_config(SESSION, &volatile).expect("volatile");
    assert_eq!(limits.durability, Durability::Volatile);
    assert_eq!(limits.session, SESSION);
    assert_eq!(limits.command_capacity, 5);
    assert_eq!(limits.control_capacity, 3);
    assert_eq!(limits.revision, volatile.config_hash());
    assert_ne!(limits.revision, appendix.config_hash());
}

/// C.14: while paused, fill the scientific queue with future commands; the
/// next command is refused, and inspection, step, run, pause, resume and
/// close all still progress through the reserved control lane without a
/// future tick having to occur.
#[test]
fn c14_paused_control_stays_responsive_with_a_full_scientific_queue() {
    let config = ecology(&[
        ("execution", "command_queue_capacity", "4"),
        ("execution", "control_queue_capacity", "2"),
    ]);
    let (results, closed) = lab(1, &config, |handle| {
        vec![Box::pin(async move {
            let cx = cx();
            for id in 0..4 {
                let admission = handle
                    .submit(&cx, command(id, at(100 + id), inject([1, 1], 1.0)))
                    .await
                    .expect("submitted");
                assert_eq!(accepted(&admission), Tick::new(100 + id));
            }
            let full = handle
                .submit(&cx, command(9, at(200), inject([1, 1], 1.0)))
                .await
                .expect("answered");
            assert_eq!(
                full,
                Admission::Rejected(Rejection::QueueFull { capacity: 4 })
            );
            let snapshot = handle.inspect(&cx).await.expect("inspect");
            assert!(!snapshot.running);
            assert_eq!(snapshot.boundary.tick, Tick::new(0));
            assert_eq!(snapshot.unfinished, 4);
            let step = handle.step(&cx).await.expect("step");
            assert_eq!(published(&step).0.tick, Tick::new(1));
            assert_eq!(
                handle.status(&cx, CommandId::new(0)).await.expect("status"),
                Some(CommandStatus::Pending)
            );
            let reached = handle.run_until(&cx, Tick::new(3)).await.expect("run");
            let RunEnd::Reached(boundary) = reached else {
                panic!("{reached:?}");
            };
            assert_eq!(boundary.tick, Tick::new(3));
            assert_eq!(boundary.epoch.get(), 3, "one epoch per tick");
            assert_eq!(handle.pause(&cx).await.expect("pause"), boundary);
            let resumed = handle.resume(&cx).await.expect("resume");
            assert_eq!(resumed, boundary);
            assert!(
                matches!(handle.step(&cx).await, Err(CoreError::StateConflict { .. })),
                "a running session refuses a step"
            );
            let paused = handle.pause(&cx).await.expect("pause");
            assert!(paused.tick >= boundary.tick);
            let snapshot = handle.inspect(&cx).await.expect("inspect");
            assert!(!snapshot.running);
            assert_eq!(snapshot.boundary, paused);
            let closed = handle.close(&cx).await.expect("close");
            assert_eq!(closed, paused);
            closed
        }) as Client<Boundary>]
    });
    assert_eq!(closed.boundary, results[0]);
    for id in 0..4 {
        assert!(matches!(
            closed.book.status(CommandId::new(id)),
            Some(CommandStatus::Terminal(Outcome::Cancelled {
                reason: CancelReason::SessionClosed,
                ..
            }))
        ));
    }
    let expected = reference(&config, closed.boundary.tick.get(), &[]);
    assert_eq!(closed.world.digest(), expected.digest());
}

/// Exact ticks: each command applies in the candidate of its effective
/// boundary, commands sharing a boundary apply in admission order, and the
/// committed world equals one stepped directly with the same interventions
/// at the same ticks.
#[test]
fn commands_apply_exactly_at_their_effective_tick_and_match_the_direct_world() {
    let config = ecology(&[]);
    let a = inject([3, 4], 10.0);
    let b = inject([20, 20], 5.0);
    let c = inject([3, 5], 1.0);
    let (results, closed) = lab(2, &config, |handle| {
        vec![Box::pin(async move {
            let cx = cx();
            assert_eq!(
                accepted(&handle.submit(&cx, command(1, at(2), a)).await.expect("a")),
                Tick::new(2)
            );
            assert_eq!(
                accepted(
                    &handle
                        .submit(&cx, command(2, Target::NextAdmissible, b))
                        .await
                        .expect("b")
                ),
                Tick::new(0)
            );
            assert_eq!(
                accepted(&handle.submit(&cx, command(3, at(2), c)).await.expect("c")),
                Tick::new(2)
            );
            let mut applied = Vec::new();
            for _ in 0..4 {
                let step = handle.step(&cx).await.expect("step");
                let (boundary, ids, from) = published(&step);
                assert_eq!(boundary.tick.get(), from.get() + 1);
                applied.push(ids.iter().map(|id| id.get()).collect::<Vec<_>>());
            }
            let status = handle.status(&cx, CommandId::new(3)).await.expect("status");
            assert!(matches!(
                status,
                Some(CommandStatus::Terminal(Outcome::Applied { tick, published, .. }))
                    if tick == Tick::new(2) && published == Tick::new(3)
            ));
            handle.close(&cx).await.expect("close");
            applied
        }) as Client<Vec<Vec<u64>>>]
    });
    assert_eq!(results[0], vec![vec![2], vec![], vec![1, 3], vec![]]);
    let expected = reference(&config, 4, &[(0, b), (2, a), (2, c)]);
    assert_eq!(closed.world.digest(), expected.digest());
    assert_eq!(closed.book.outcome_cursor(), 3);
}

/// Idempotent identities: a retry after application returns the applied
/// outcome and applies nothing; reuse with other content is refused; a late
/// explicit target is refused unless it permits retargeting, and then the
/// chosen tick is reported.
#[test]
fn retries_return_the_first_outcome_and_apply_once() {
    let config = ecology(&[]);
    let x = command(1, at(1), inject([7, 7], 3.0));
    let (results, closed) = lab(3, &config, |handle| {
        vec![Box::pin(async move {
            let cx = cx();
            accepted(&handle.submit(&cx, x).await.expect("x"));
            for _ in 0..3 {
                handle.step(&cx).await.expect("step");
            }
            let retry = handle.submit(&cx, x).await.expect("retry");
            let Admission::Accepted {
                accepted,
                status,
                retry: true,
            } = retry
            else {
                panic!("{retry:?}");
            };
            assert_eq!(accepted.order, 0);
            assert!(matches!(
                status,
                CommandStatus::Terminal(Outcome::Applied { tick, cursor: 0, .. }) if tick == Tick::new(1)
            ));
            let reused = flife_runtime::Command {
                intervention: inject([7, 7], 4.0),
                ..x
            };
            assert_eq!(
                handle.submit(&cx, reused).await.expect("reuse"),
                Admission::Rejected(Rejection::ConflictingReuse {
                    accepted: x.content_hash()
                })
            );
            assert_eq!(
                handle
                    .submit(&cx, command(2, at(0), inject([1, 2], 1.0)))
                    .await
                    .expect("late"),
                Admission::Rejected(Rejection::Late {
                    requested: Tick::new(0),
                    earliest: Tick::new(3)
                })
            );
            let moved = handle
                .submit(
                    &cx,
                    command(
                        2,
                        Target::At {
                            tick: Tick::new(0),
                            retarget: true,
                        },
                        inject([1, 2], 1.0),
                    ),
                )
                .await
                .expect("retargeted");
            let Admission::Accepted { accepted, .. } = moved else {
                panic!("{moved:?}");
            };
            assert!(accepted.retargeted);
            assert_eq!(accepted.effective, Tick::new(3));
            let wrong = flife_runtime::Command {
                expected_revision: Some(Sha256Digest::of(b"another configuration")),
                ..command(3, at(9), inject([1, 2], 1.0))
            };
            assert!(matches!(
                handle.submit(&cx, wrong).await.expect("revision"),
                Admission::Rejected(Rejection::RevisionMismatch { .. })
            ));
            handle.step(&cx).await.expect("step");
            handle.close(&cx).await.expect("close")
        }) as Client<Boundary>]
    });
    assert_eq!(results[0].tick, Tick::new(4));
    let expected = reference(
        &config,
        4,
        &[(1, inject([7, 7], 3.0)), (3, inject([1, 2], 1.0))],
    );
    assert_eq!(closed.world.digest(), expected.digest());
}

/// What a cancellation decided, by kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Decided {
    BeforeSealing,
    AtTheCancellationBoundary,
    TooLate,
}

/// C.8 and C.14: a cancellation races sealing, the candidate and its
/// publication. Exactly one outcome wins, and the committed world shows it:
/// without the command when the cancellation won, before or after the
/// command was sealed into a candidate, and with it exactly once when
/// publication won. The canceller waits a varying number of yields, so the
/// sweep reaches every side of the race; the same seed decides the same way
/// again.
#[test]
fn c8_cancellation_races_publication_and_exactly_one_outcome_wins() {
    let config = ecology(&[]);
    let x = command(1, at(2), inject([10, 12], 8.0));
    let without = reference(&config, 5, &[]).digest();
    let with = reference(&config, 5, &[(2, x.intervention)]).digest();
    let run = |seed: u64, yields: u64| -> Decided {
        let (results, closed) = lab(seed, &config, |handle| {
            let canceller = handle.clone();
            vec![
                Box::pin(async move {
                    let cx = cx();
                    accepted(&handle.submit(&cx, x).await.expect("x"));
                    let end = handle.run_until(&cx, Tick::new(5)).await.expect("run");
                    assert!(
                        matches!(end, RunEnd::Reached(boundary) if boundary.tick == Tick::new(5))
                    );
                    None
                }) as Client<Option<Cancellation>>,
                Box::pin(async move {
                    let cx = cx();
                    while canceller.status(&cx, x.id).await.expect("status").is_none() {
                        yield_now().await;
                    }
                    for _ in 0..yields {
                        yield_now().await;
                    }
                    Some(canceller.cancel(&cx, x.id).await.expect("cancel"))
                }),
            ]
        });
        let decision = results[1].clone().expect("the canceller decided");
        let digest = closed.world.digest();
        assert_eq!(closed.boundary.tick, Tick::new(5));
        match decision {
            Cancellation::Cancelled { outcome, sealed } => {
                assert!(matches!(
                    outcome,
                    Outcome::Cancelled {
                        reason: CancelReason::Requested,
                        ..
                    }
                ));
                assert_eq!(digest, without, "a cancelled command never applies");
                assert!(closed.book.status(x.id) == Some(&CommandStatus::Terminal(outcome)));
                if sealed {
                    Decided::AtTheCancellationBoundary
                } else {
                    Decided::BeforeSealing
                }
            }
            Cancellation::TooLate(outcome) => {
                assert!(matches!(outcome, Outcome::Applied { tick, .. } if tick == Tick::new(2)));
                assert_eq!(digest, with, "a published command applied exactly once");
                Decided::TooLate
            }
            other => panic!("seed {seed}: {other:?}"),
        }
    };
    let mut seen = BTreeSet::new();
    for seed in 0..3 {
        for yields in 0..8 {
            let decided = run(seed, yields);
            assert_eq!(run(seed, yields), decided, "seed {seed} replays");
            seen.insert(decided);
        }
    }
    assert_eq!(
        seen,
        BTreeSet::from([
            Decided::BeforeSealing,
            Decided::AtTheCancellationBoundary,
            Decided::TooLate
        ]),
        "every side of the race was exercised"
    );
}

/// A pause while running takes effect at the boundary publication reaches,
/// and that boundary is reported: inspection sees it unchanged, and the next
/// step advances exactly one tick from it.
#[test]
fn a_pause_while_running_takes_effect_at_a_reported_boundary() {
    let config = ecology(&[]);
    for seed in 0..6 {
        let (results, closed) = lab(seed, &config, |handle| {
            let pauser = handle.clone();
            vec![
                Box::pin(async move {
                    let cx = cx();
                    let started = handle.resume(&cx).await.expect("resume");
                    assert_eq!(started.tick, Tick::new(0));
                    None
                }) as Client<Option<(Boundary, Boundary)>>,
                Box::pin(async move {
                    let cx = cx();
                    while !pauser.inspect(&cx).await.expect("inspect").running {
                        yield_now().await;
                    }
                    for _ in 0..(3 * seed) {
                        yield_now().await;
                    }
                    let paused = pauser.pause(&cx).await.expect("pause");
                    let snapshot = pauser.inspect(&cx).await.expect("inspect");
                    assert!(!snapshot.running);
                    assert_eq!(snapshot.boundary, paused);
                    let step = pauser.step(&cx).await.expect("step");
                    let stepped = *published(&step).0;
                    assert_eq!(stepped.tick.get(), paused.tick.get() + 1);
                    pauser.close(&cx).await.expect("close");
                    Some((paused, stepped))
                }),
            ]
        });
        let (paused, stepped) = results[1].expect("paused");
        assert_eq!(paused.epoch.get(), paused.tick.get());
        assert_eq!(closed.boundary, stepped);
        let expected = reference(&config, stepped.tick.get(), &[]);
        assert_eq!(closed.world.digest(), expected.digest());
    }
}

#[test]
fn a_started_run_reports_its_start_at_once_and_its_end_later() {
    let config = ecology(&[]);
    for seed in 0..6 {
        let (results, closed) = lab(seed, &config, |handle| {
            vec![Box::pin(async move {
                let cx = cx();
                // An open-ended run returns its start at once and ends on a
                // pause, at the boundary the pause reports.
                let running = handle.start(&cx, None).await.expect("start");
                assert_eq!(running.started().tick, Tick::new(0));
                while handle.inspect(&cx).await.expect("inspect").boundary.tick < Tick::new(2) {
                    yield_now().await;
                }
                let paused = handle.pause(&cx).await.expect("pause");
                let end = running.ended(&cx).await.expect("end");
                assert_eq!(end, RunEnd::Paused(paused));
                // A bounded run ends at its tick, and the next starts there.
                let running = handle
                    .start(&cx, Some(Tick::new(paused.tick.get() + 3)))
                    .await
                    .expect("start");
                assert_eq!(running.started(), paused);
                let reached = running.ended(&cx).await.expect("end");
                let RunEnd::Reached(reached) = reached else {
                    panic!("not reached: {reached:?}");
                };
                assert_eq!(reached.tick.get(), paused.tick.get() + 3);
                // A run that is open when the session closes ends closed.
                let running = handle.start(&cx, None).await.expect("start");
                let boundary = handle.close(&cx).await.expect("close");
                assert_eq!(
                    running.ended(&cx).await.expect("end"),
                    RunEnd::Closed(boundary)
                );
                Some(boundary)
            }) as Client<Option<Boundary>>]
        });
        let boundary = results[0].expect("closed");
        assert_eq!(closed.boundary, boundary);
        let expected = reference(&config, boundary.tick.get(), &[]);
        assert_eq!(closed.world.digest(), expected.digest());
    }
}

/// The tick at which the population cap first pauses this world.
#[expect(
    clippy::panic,
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn first_capacity_pause(config: &ValidatedConfig) -> u64 {
    let mut world = found(config);
    for _ in 0..512 {
        let tick = world.tick().get();
        match world.advance().expect("advance") {
            Advance::Ready(candidate) => {
                candidate.publish().expect("publish");
            }
            Advance::Paused(_) => return tick,
        }
    }
    panic!("no capacity pause in 512 ticks");
}

/// A capacity shortfall pauses the run at an unchanged boundary that stays
/// sealed: a step retries the identical candidate and pauses again, a new
/// command cannot enter that boundary, and a cancellation of the sealed
/// command still wins.
#[test]
fn a_capacity_pause_keeps_its_boundary_sealed_for_the_identical_retry() {
    let config = ecology(&[("population", "maximum_count", "48")]);
    let pause = first_capacity_pause(&config);
    let x = command(1, at(pause), inject([4, 4], 2.0));
    let (results, closed) = lab(4, &config, |handle| {
        vec![Box::pin(async move {
            let cx = cx();
            accepted(&handle.submit(&cx, x).await.expect("x"));
            let end = handle
                .run_until(&cx, Tick::new(pause + 8))
                .await
                .expect("run");
            let RunEnd::CapacityPaused {
                boundary,
                shortfall,
            } = end
            else {
                panic!("{end:?}");
            };
            assert_eq!(boundary.tick, Tick::new(pause));
            assert_eq!(
                handle.status(&cx, x.id).await.expect("status"),
                Some(CommandStatus::Sealed)
            );
            let again = handle.step(&cx).await.expect("step");
            assert_eq!(
                again,
                StepOutcome::Paused {
                    boundary,
                    shortfall: shortfall.clone()
                },
                "the identical retry pauses identically"
            );
            assert_eq!(
                handle
                    .submit(&cx, command(2, at(pause), inject([4, 4], 1.0)))
                    .await
                    .expect("late"),
                Admission::Rejected(Rejection::Late {
                    requested: Tick::new(pause),
                    earliest: Tick::new(pause + 1)
                })
            );
            let cancelled = handle.cancel(&cx, x.id).await.expect("cancel");
            assert!(matches!(
                cancelled,
                Cancellation::Cancelled { sealed: true, .. }
            ));
            assert!(matches!(
                handle.step(&cx).await.expect("step"),
                StepOutcome::Paused { .. }
            ));
            handle.close(&cx).await.expect("close")
        }) as Client<Boundary>]
    });
    assert_eq!(results[0].tick, Tick::new(pause));
    let expected = reference(&config, pause, &[]);
    assert_eq!(closed.world.digest(), expected.digest());
}

/// `stop-and-preserve`: a numerical fault stops the run on its committed
/// state, fails every unfinished command, and refuses new work, steps and
/// runs; a retry of an accepted identity still returns its outcome.
#[test]
fn a_numerical_fault_stops_the_run_and_fails_every_unfinished_command() {
    let config = ecology(&[
        ("sources", "count", "1"),
        ("sources", "power_per_source", "1.0e40"),
        ("sources", "sigma_world_units", "0.5"),
    ]);
    let x = command(1, at(60), inject([2, 2], 1.0));
    let y = command(2, at(70), inject([2, 2], 1.0));
    let (results, closed) = lab(5, &config, |handle| {
        vec![Box::pin(async move {
            let cx = cx();
            accepted(&handle.submit(&cx, x).await.expect("x"));
            accepted(&handle.submit(&cx, y).await.expect("y"));
            let stopped_at = loop {
                match handle.step(&cx).await.expect("step") {
                    StepOutcome::Published { boundary, .. } => {
                        assert!(boundary.tick < Tick::new(60), "the fault comes first");
                    }
                    StepOutcome::Failed {
                        stopped: true,
                        error: CoreError::NumericalInvalid { .. },
                        boundary,
                    } => break boundary,
                    other => panic!("{other:?}"),
                }
            };
            for id in [x.id, y.id] {
                assert!(matches!(
                    handle.status(&cx, id).await.expect("status"),
                    Some(CommandStatus::Terminal(Outcome::Failed { .. }))
                ));
            }
            assert_eq!(
                handle
                    .submit(&cx, command(3, Target::NextAdmissible, inject([2, 2], 1.0)))
                    .await
                    .expect("refused"),
                Admission::Rejected(Rejection::Stopped)
            );
            assert!(matches!(
                handle.submit(&cx, y).await.expect("retry"),
                Admission::Accepted {
                    retry: true,
                    status: CommandStatus::Terminal(Outcome::Failed { .. }),
                    ..
                }
            ));
            assert!(matches!(
                handle.step(&cx).await,
                Err(CoreError::StateConflict { .. })
            ));
            assert!(matches!(
                handle.resume(&cx).await,
                Err(CoreError::StateConflict { .. })
            ));
            let snapshot = handle.inspect(&cx).await.expect("inspect");
            assert!(snapshot.stopped);
            assert_eq!(snapshot.boundary, stopped_at);
            stopped_at
        }) as Client<Boundary>]
    });
    let stopped_at = results[0];
    assert!(closed.world.fault().is_some());
    assert_eq!(closed.boundary, stopped_at);
    let preserved = reference(&config, stopped_at.tick.get(), &[]);
    assert_eq!(
        closed.world.digest(),
        preserved.digest(),
        "the committed state is the last published epoch"
    );
}

/// Closing while running discards any unpublished candidate, cancels every
/// unfinished command as `session-closed`, and refuses later requests; the
/// world is the last published epoch.
#[test]
fn closing_discards_an_unpublished_candidate_and_cancels_unfinished_commands() {
    let config = ecology(&[]);
    for seed in 0..4 {
        let (results, closed) = lab(seed, &config, |handle| {
            let closer = handle.clone();
            let late = handle.clone();
            vec![
                Box::pin(async move {
                    let cx = cx();
                    accepted(
                        &handle
                            .submit(&cx, command(1, at(1000), inject([1, 1], 1.0)))
                            .await
                            .expect("x"),
                    );
                    handle.resume(&cx).await.expect("resume");
                    None
                }) as Client<Option<Boundary>>,
                Box::pin(async move {
                    let cx = cx();
                    while !closer.inspect(&cx).await.expect("inspect").running {
                        yield_now().await;
                    }
                    for _ in 0..(3 * seed) {
                        yield_now().await;
                    }
                    let boundary = closer.close(&cx).await.expect("close");
                    let refused = late
                        .submit(&cx, command(2, Target::NextAdmissible, inject([1, 1], 1.0)))
                        .await;
                    assert!(
                        matches!(
                            refused,
                            Err(CoreError::QueueClosed { .. })
                                | Ok(Admission::Rejected(Rejection::Closed))
                        ),
                        "{refused:?}"
                    );
                    assert!(late.step(&cx).await.is_err());
                    Some(boundary)
                }),
            ]
        });
        let boundary = results[1].expect("closed");
        assert_eq!(closed.boundary, boundary);
        assert!(matches!(
            closed.book.status(CommandId::new(1)),
            Some(CommandStatus::Terminal(Outcome::Cancelled {
                reason: CancelReason::SessionClosed,
                boundary: at,
                ..
            })) if *at == boundary.tick
        ));
        assert!(closed.book.is_closed());
        let expected = reference(&config, boundary.tick.get(), &[]);
        assert_eq!(closed.world.digest(), expected.digest());
    }
}

/// When every handle is dropped the owner closes on its own.
#[test]
fn dropping_every_handle_closes_the_session() {
    let config = ecology(&[]);
    let (_, closed) = lab(6, &config, |handle| {
        vec![Box::pin(async move {
            let cx = cx();
            handle.step(&cx).await.expect("step");
        }) as Client<()>]
    });
    assert_eq!(closed.boundary.tick, Tick::new(1));
    assert!(closed.book.is_closed());
}

/// When every handle is dropped while the owner is running, it closes at
/// its next boundary instead of running on unobserved.
#[test]
fn dropping_every_handle_while_running_closes_the_session() {
    let config = ecology(&[]);
    let (_, closed) = bounded_lab(7, &config, 2_000, |handle| {
        vec![Box::pin(async move {
            let cx = cx();
            handle.resume(&cx).await.expect("resume");
            for _ in 0..6 {
                yield_now().await;
            }
        }) as Client<()>]
    });
    assert!(closed.book.is_closed());
    assert!(closed.boundary.tick.get() < 64, "{:?}", closed.boundary);
    let expected = reference(&config, closed.boundary.tick.get(), &[]);
    assert_eq!(closed.world.digest(), expected.digest());
}

/// The same protocol on Asupersync's native current-thread runtime: the
/// owner and the client are real spawned tasks, and the committed world
/// equals the directly stepped one.
#[test]
fn a_native_runtime_session_runs_the_same_protocol() {
    let config = ecology(&[]);
    let (owner, handle) = open(found(&config), &config, SESSION).expect("open");
    let runtime = RuntimeBuilder::current_thread().build().expect("runtime");
    let spawner = runtime.handle();
    let closed = runtime.block_on(async move {
        let owner = spawner
            .try_spawn(async move {
                let cx = cx();
                owner.run(&cx).await
            })
            .expect("owner");
        let client = spawner
            .try_spawn(async move {
                let cx = cx();
                accepted(
                    &handle
                        .submit(&cx, command(1, at(1), inject([9, 9], 6.0)))
                        .await
                        .expect("x"),
                );
                let end = handle.run_until(&cx, Tick::new(3)).await.expect("run");
                assert!(matches!(end, RunEnd::Reached(boundary) if boundary.tick == Tick::new(3)));
                handle.close(&cx).await.expect("close")
            })
            .expect("client");
        let boundary = client.await;
        let closed = owner.await;
        assert_eq!(closed.boundary, boundary);
        closed
    });
    let expected = reference(&config, 3, &[(1, inject([9, 9], 6.0))]);
    assert_eq!(closed.world.digest(), expected.digest());
    let mut direct = found(&config);
    for tick in 0..3 {
        let due: Vec<Intervention> = if tick == 1 {
            vec![inject([9, 9], 6.0)]
        } else {
            Vec::new()
        };
        let report = publish(&mut direct, &due);
        assert_eq!(report.interventions.len(), due.len());
    }
    assert_eq!(direct.digest(), closed.world.digest());
    assert!(matches!(found(&config).step(), Ok(Stepped::Published(_))));
}
