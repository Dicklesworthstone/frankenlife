//! FL-014 evidence (plan §12.3, §8.1 phase 1, Appendix C.14): the command
//! book's admission, schedule, sealing, cancellation and outcome stream,
//! driven directly against a real world. Expected values come from the
//! protocol's rules and from a world stepped with the same interventions
//! directly, never from the book itself.

mod common;

use common::{SESSION, advance, at, command, ecology, found, inject, publish, reference};
use flife_core::Tick;
use flife_core::canonical::Sha256Digest;
use flife_core::ids::{CommandId, SessionId};
use flife_core::ledger::Flow;
use flife_engine::Intervention;
use flife_runtime::{
    Admission, CancelReason, Cancellation, CommandBook, CommandStatus, Durability, Outcome,
    Rejection, Target,
};

fn book(capacity: usize) -> CommandBook {
    CommandBook::new(SESSION, ecology(&[]).config_hash(), capacity)
}

#[expect(
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
fn accepted(admission: &Admission) -> (u64, Tick, bool) {
    match admission {
        Admission::Accepted {
            accepted, retry, ..
        } => {
            assert_eq!(accepted.durability, Durability::Volatile);
            (accepted.order, accepted.effective, *retry)
        }
        Admission::Rejected(rejection) => panic!("rejected: {rejection:?}"),
    }
}

#[expect(
    clippy::panic,
    reason = "test helper: a failure here is a test failure"
)]
fn rejected(admission: Admission) -> Rejection {
    match admission {
        Admission::Rejected(rejection) => rejection,
        accepted @ Admission::Accepted { .. } => panic!("accepted: {accepted:?}"),
    }
}

const CELL: [u32; 2] = [5, 9];

/// Exact ticks: each command is assigned the boundary it named, or the next
/// unsealed one; a sealed or published boundary is late unless the command
/// permits retargeting, and then it is moved to the earliest open boundary.
#[test]
fn admission_assigns_exact_effective_ticks_and_refuses_late_targets() {
    let config = ecology(&[]);
    let mut world = found(&config);
    let grid = *world.field().grid();
    let mut book = book(16);
    let tick = world.tick();
    let next = book.admit(
        command(1, Target::NextAdmissible, inject(CELL, 1.0)),
        tick,
        &grid,
        false,
    );
    assert_eq!(accepted(&next), (0, Tick::new(0), false));
    let explicit = book.admit(command(2, at(4), inject(CELL, 1.0)), tick, &grid, false);
    assert_eq!(accepted(&explicit), (1, Tick::new(4), false));

    // Sealing tick 0 closes its boundary.
    let sealed = book.seal(tick).expect("seal");
    assert_eq!(sealed.len(), 1);
    assert_eq!(book.earliest(tick).expect("earliest"), Tick::new(1));
    assert_eq!(
        rejected(book.admit(command(3, at(0), inject(CELL, 1.0)), tick, &grid, false)),
        Rejection::Late {
            requested: Tick::new(0),
            earliest: Tick::new(1)
        }
    );
    let moved = book.admit(
        command(
            4,
            Target::At {
                tick: Tick::new(0),
                retarget: true,
            },
            inject(CELL, 1.0),
        ),
        tick,
        &grid,
        false,
    );
    let Admission::Accepted {
        accepted: record, ..
    } = moved
    else {
        panic!("retargeted");
    };
    assert!(record.retargeted);
    assert_eq!((record.order, record.effective), (2, Tick::new(1)));
    let next = book.admit(
        command(5, Target::NextAdmissible, inject(CELL, 1.0)),
        tick,
        &grid,
        false,
    );
    assert_eq!(accepted(&next).1, Tick::new(1));

    // After publication the published boundary is in the past.
    let interventions: Vec<Intervention> = sealed.iter().map(|&(_, i)| i).collect();
    let report = publish(&mut world, &interventions);
    book.published(&report, &sealed).expect("recorded");
    assert_eq!(book.sealed(), None);
    let tick = world.tick();
    assert_eq!(book.earliest(tick).expect("earliest"), Tick::new(1));
    assert_eq!(
        rejected(book.admit(command(6, at(0), inject(CELL, 1.0)), tick, &grid, false)),
        Rejection::Late {
            requested: Tick::new(0),
            earliest: Tick::new(1)
        }
    );
    // The schedule is ordered by (effective tick, admission order).
    let schedule: Vec<(u64, u64, u64)> = book
        .schedule()
        .map(|(tick, order, id)| (tick.get(), order, id.get()))
        .collect();
    assert_eq!(schedule, vec![(1, 2, 4), (1, 3, 5), (4, 1, 2)]);
}

/// The same identity and content return the existing acceptance and its
/// current status; the same identity with any other content is refused. A
/// refused admission records nothing, so its retry is admitted afresh.
#[test]
fn retries_are_idempotent_and_conflicting_reuse_is_refused() {
    let config = ecology(&[]);
    let mut world = found(&config);
    let grid = *world.field().grid();
    let mut book = book(1);
    let first = command(10, at(1), inject(CELL, 2.5));
    let admission = book.admit(first, world.tick(), &grid, false);
    assert_eq!(accepted(&admission), (0, Tick::new(1), false));
    let again = book.admit(first, world.tick(), &grid, false);
    assert_eq!(accepted(&again), (0, Tick::new(1), true));
    assert_eq!(book.admitted(), 1, "a retry is not a new acceptance");

    for conflicting in [
        command(10, at(1), inject(CELL, 2.0)),
        command(10, at(2), inject(CELL, 2.5)),
        command(10, at(1), inject([5, 8], 2.5)),
        flife_runtime::Command {
            expected_revision: Some(book.revision()),
            ..first
        },
    ] {
        assert_eq!(
            rejected(book.admit(conflicting, world.tick(), &grid, false)),
            Rejection::ConflictingReuse {
                accepted: first.content_hash()
            }
        );
    }

    // A full queue refuses without recording; the retry is admitted later.
    let second = command(11, at(3), inject(CELL, 1.0));
    assert_eq!(
        rejected(book.admit(second, world.tick(), &grid, false)),
        Rejection::QueueFull { capacity: 1 }
    );
    advance(&mut world, &mut book);
    let report = advance(&mut world, &mut book);
    assert_eq!(report.interventions.len(), 1);
    let applied = report.interventions[0].applied;
    assert_eq!(
        accepted(&book.admit(second, world.tick(), &grid, false)),
        (1, Tick::new(3), false)
    );

    // After application the retry returns the applied outcome, and nothing
    // applies twice.
    let Admission::Accepted { status, retry, .. } = book.admit(first, world.tick(), &grid, false)
    else {
        panic!("retry");
    };
    assert!(retry);
    assert_eq!(
        status,
        CommandStatus::Terminal(Outcome::Applied {
            tick: Tick::new(1),
            published: Tick::new(2),
            epoch: report.epoch,
            applied,
            cursor: 0,
        })
    );
    while world.tick().get() < 5 {
        advance(&mut world, &mut book);
    }
    let expected = reference(
        &config,
        5,
        &[(1, inject(CELL, 2.5)), (3, inject(CELL, 1.0))],
    );
    assert_eq!(world.digest(), expected.digest());
    assert_eq!(book.outcome_cursor(), 2);
}

/// C.14: A accepted for tick 1000 before B for tick 101. Applying B leaves A
/// pending and unapplied; the outcome cursor counts B alone and is not the
/// admission order.
#[test]
fn c14_admission_order_and_application_order_differ() {
    let config = ecology(&[("population", "initial_count", "8")]);
    let mut world = found(&config);
    let grid = *world.field().grid();
    let mut book = book(8);
    let a = command(1, at(1000), inject(CELL, 1.0));
    let b = command(2, at(101), inject([20, 3], 4.0));
    assert_eq!(accepted(&book.admit(a, world.tick(), &grid, false)).0, 0);
    assert_eq!(accepted(&book.admit(b, world.tick(), &grid, false)).0, 1);
    let mut applied_at = None;
    while world.tick().get() < 103 {
        let report = advance(&mut world, &mut book);
        if !report.interventions.is_empty() {
            applied_at = Some(report.from);
        }
    }
    assert_eq!(applied_at, Some(Tick::new(101)));
    assert_eq!(book.status(a.id), Some(&CommandStatus::Pending));
    assert!(matches!(
        book.status(b.id),
        Some(CommandStatus::Terminal(Outcome::Applied { tick, cursor: 0, .. })) if *tick == Tick::new(101)
    ));
    assert_eq!(book.outcome_cursor(), 1);
    assert_eq!(book.outcome_stream(), &[b.id]);
    assert_eq!(book.admitted(), 2);
    assert_eq!(
        book.schedule().collect::<Vec<_>>(),
        vec![(Tick::new(1000), 0, a.id)]
    );
    // Retrying either changes nothing.
    assert!(matches!(
        book.admit(a, world.tick(), &grid, false),
        Admission::Accepted {
            retry: true,
            status: CommandStatus::Pending,
            ..
        }
    ));
    assert!(matches!(
        book.admit(b, world.tick(), &grid, false),
        Admission::Accepted {
            retry: true,
            status: CommandStatus::Terminal(_),
            ..
        }
    ));
    assert_eq!(book.outcome_cursor(), 1);
    let expected = reference(&config, 103, &[(101, inject([20, 3], 4.0))]);
    assert_eq!(world.digest(), expected.digest());
}

/// A pending command's cancellation wins; one already applied is too late;
/// a second cancellation reports the first outcome.
#[test]
fn cancellation_wins_before_application_and_is_too_late_after() {
    let config = ecology(&[]);
    let mut world = found(&config);
    let grid = *world.field().grid();
    let mut book = book(8);
    let early = command(1, at(0), inject(CELL, 1.0));
    let later = command(2, at(2), inject(CELL, 1.0));
    accepted(&book.admit(early, world.tick(), &grid, false));
    accepted(&book.admit(later, world.tick(), &grid, false));
    advance(&mut world, &mut book);
    let Cancellation::Cancelled { outcome, sealed } = book.cancel(later.id, world.tick()) else {
        panic!("a pending command's cancellation wins");
    };
    assert!(!sealed);
    assert_eq!(
        outcome,
        Outcome::Cancelled {
            boundary: Tick::new(1),
            reason: CancelReason::Requested,
            cursor: 1,
        }
    );
    assert!(matches!(
        book.cancel(early.id, world.tick()),
        Cancellation::TooLate(Outcome::Applied { .. })
    ));
    assert_eq!(
        book.cancel(later.id, world.tick()),
        Cancellation::AlreadyTerminal(outcome)
    );
    assert_eq!(
        book.cancel(CommandId::new(99), world.tick()),
        Cancellation::Unknown
    );
    while world.tick().get() < 4 {
        let report = advance(&mut world, &mut book);
        assert!(
            report.interventions.is_empty(),
            "a cancelled command never applies"
        );
    }
    let expected = reference(&config, 4, &[(0, inject(CELL, 1.0))]);
    assert_eq!(world.digest(), expected.digest());
}

/// A sealed boundary admits nothing new. A discarded candidate leaves it
/// sealed, so the retry is the identical set, less a command whose
/// cancellation won; publication applies exactly that set.
#[test]
fn a_sealed_boundary_survives_a_discarded_candidate_and_loses_only_cancelled_commands() {
    let config = ecology(&[]);
    let mut world = found(&config);
    let grid = *world.field().grid();
    let mut book = book(8);
    for (id, cell) in [(1, [1, 1]), (2, [2, 2]), (3, [3, 3])] {
        accepted(&book.admit(
            command(id, at(0), inject(cell, 1.0)),
            world.tick(),
            &grid,
            false,
        ));
    }
    let sealed = book.seal(world.tick()).expect("seal");
    assert_eq!(
        sealed.iter().map(|(id, _)| id.get()).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(book.status(CommandId::new(2)), Some(&CommandStatus::Sealed));
    let interventions: Vec<Intervention> = sealed.iter().map(|&(_, i)| i).collect();
    let digest = world.digest();
    if let flife_engine::Advance::Ready(candidate) =
        world.advance_with(&interventions).expect("advance")
    {
        candidate.cancel();
    }
    assert_eq!(world.digest(), digest);
    assert_eq!(
        book.seal(world.tick()).expect("reseal"),
        sealed,
        "the identical retry"
    );
    assert!(matches!(
        book.cancel(CommandId::new(2), world.tick()),
        Cancellation::Cancelled { sealed: true, .. }
    ));
    let resealed = book.seal(world.tick()).expect("reseal");
    assert_eq!(
        resealed.iter().map(|(id, _)| id.get()).collect::<Vec<_>>(),
        [1, 3]
    );
    // The stale set is refused; the current one is recorded.
    let interventions: Vec<Intervention> = resealed.iter().map(|&(_, i)| i).collect();
    let report = publish(&mut world, &interventions);
    assert!(book.published(&report, &sealed).is_err());
    // So are the same interventions under another identity.
    let impostor = vec![(CommandId::new(9), resealed[0].1), resealed[1]];
    assert!(book.published(&report, &impostor).is_err());
    book.published(&report, &resealed).expect("recorded");
    assert_eq!(
        report
            .interventions
            .iter()
            .map(|applied| (applied.position, applied.intervention))
            .collect::<Vec<_>>(),
        vec![(0, inject([1, 1], 1.0)), (1, inject([3, 3], 1.0))]
    );
    // Another boundary cannot be sealed while one is.
    let mut other = self::book(8);
    other.seal(Tick::new(5)).expect("seal");
    assert!(other.seal(Tick::new(6)).is_err());
}

/// The applied energy reconciles: the ledger's `command-injection` is the
/// energy that entered the field, never more than declared and within one
/// `f32` step of the cell, and the closure accounts for it.
#[test]
fn an_injection_reconciles_with_the_ledger_at_its_tick() {
    let config = ecology(&[]);
    let mut world = found(&config);
    let mut control = found(&config);
    let grid = *world.field().grid();
    let mut book = book(8);
    let amount = 37.0;
    accepted(&book.admit(
        command(1, at(3), inject(CELL, amount)),
        world.tick(),
        &grid,
        false,
    ));
    for _ in 0..3 {
        let report = advance(&mut world, &mut book);
        assert!(report.interventions.is_empty());
        publish(&mut control, &[]);
        assert_eq!(
            world.digest(),
            control.digest(),
            "nothing applies before tick 3"
        );
    }
    let report = advance(&mut world, &mut book);
    let reference = publish(&mut control, &[]);
    assert_eq!(report.from, Tick::new(3));
    let [applied] = report.interventions.as_slice() else {
        panic!("one intervention");
    };
    assert_eq!(applied.tick, Tick::new(3));
    assert!(applied.applied <= amount);
    // Rounded down by less than one f32 ulp of the cell (about 4e-6 here).
    assert!(amount - applied.applied < 1.0e-5, "{applied:?}");
    assert_eq!(
        report.closure.flow(Flow::CommandInjection).to_bits(),
        applied.applied.to_bits()
    );
    assert_eq!(reference.closure.flow(Flow::CommandInjection), 0.0);
    let extra = report.closure.injected() - reference.closure.injected();
    assert!((extra - applied.applied).abs() <= 1.0e-12 * report.closure.injected());
    assert!(report.closure.residual().abs() <= report.closure.allowance());
    assert_ne!(world.digest(), control.digest());
}

/// Admission refuses another session, another configuration, an invalid
/// payload and new work after the run stopped; a closed book cancels every
/// unfinished command and still answers retries of accepted identities.
#[test]
fn admission_refusals_and_closing() {
    let config = ecology(&[]);
    let world = found(&config);
    let grid = *world.field().grid();
    let tick = world.tick();
    let mut book = book(8);
    let other = flife_runtime::Command {
        session: SessionId::new(8),
        ..command(1, at(0), inject(CELL, 1.0))
    };
    assert_eq!(
        rejected(book.admit(other, tick, &grid, false)),
        Rejection::WrongSession { session: SESSION }
    );
    let stale = flife_runtime::Command {
        expected_revision: Some(Sha256Digest::of(b"another configuration")),
        ..command(1, at(0), inject(CELL, 1.0))
    };
    assert!(matches!(
        rejected(book.admit(stale, tick, &grid, false)),
        Rejection::RevisionMismatch { .. }
    ));
    for invalid in [
        inject([32, 0], 1.0),
        inject([0, 32], 1.0),
        inject(CELL, 0.0),
        inject(CELL, -1.0),
        inject(CELL, f64::NAN),
        inject(CELL, f64::INFINITY),
        inject(CELL, 1.0e39),
    ] {
        assert!(matches!(
            rejected(book.admit(command(1, at(0), invalid), tick, &grid, false)),
            Rejection::Invalid(_)
        ));
    }
    assert_eq!(
        rejected(book.admit(command(1, at(0), inject(CELL, 1.0)), tick, &grid, true)),
        Rejection::Stopped
    );
    let kept = command(2, at(5), inject(CELL, 1.0));
    let sealed = command(3, at(0), inject(CELL, 1.0));
    accepted(&book.admit(kept, tick, &grid, false));
    accepted(&book.admit(sealed, tick, &grid, false));
    book.seal(tick).expect("seal");
    assert_eq!(book.close(tick), 2);
    assert!(book.is_closed());
    assert_eq!(book.unfinished(), 0);
    for id in [kept.id, sealed.id] {
        assert!(matches!(
            book.status(id),
            Some(CommandStatus::Terminal(Outcome::Cancelled {
                reason: CancelReason::SessionClosed,
                ..
            }))
        ));
    }
    assert_eq!(
        rejected(book.admit(command(4, at(9), inject(CELL, 1.0)), tick, &grid, false)),
        Rejection::Closed
    );
    assert!(matches!(
        book.admit(kept, tick, &grid, false),
        Admission::Accepted {
            retry: true,
            status: CommandStatus::Terminal(_),
            ..
        }
    ));

    // A stopped run fails every unfinished command.
    let mut stopped = self::book(8);
    accepted(&stopped.admit(command(5, at(3), inject(CELL, 1.0)), tick, &grid, false));
    assert_eq!(stopped.fail_unfinished("the run stopped"), 1);
    assert!(matches!(
        stopped.status(CommandId::new(5)),
        Some(CommandStatus::Terminal(Outcome::Failed { cursor: 0, .. }))
    ));
}
