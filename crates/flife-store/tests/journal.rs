//! FL-036 evidence (plan §12.3, §12.4, §16.6, Appendix B.1 and C.14): the
//! FrankenSQLite run catalog and command journal, and
//! `write-ahead-before-apply` sessions over it.
//!
//! Sessions run on Asupersync's native runtime with the journal's writer
//! thread, against FL-013's 48-founder ecology. Every world a session
//! commits is compared with one stepped directly with the same
//! interventions at the same ticks, so "applied once" is checked by its
//! effect on the science, not only by the owner's report. The crash tests
//! kill (`SIGKILL`) a child process at each point C.14 names and recover in
//! the parent from what the journal and chunk store actually hold.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::fs;
use std::future::Future;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use asupersync::Cx;
use asupersync::runtime::{RuntimeBuilder, yield_now};
use flife_core::canonical::Sha256Digest;
use flife_core::ids::{CommandId, SessionId};
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_engine::{Arrangement, Intervention, NumericalProfile, Stepped, World};
use flife_runtime::journal::block_on_inline;
use flife_runtime::{
    Accepted, Admission, CancelReason, Cancellation, Command, CommandBook, CommandStatus, Journal,
    JournalContents, JournalRecord, JournalWriter, Outcome, Rejection, RunEnd, SessionClosed,
    SessionConfig, SessionHandle, SessionOwner, StepOutcome, Target, Watermarks, open_journaled,
    restore,
};
use flife_store::{CATALOG_FORMAT, ChunkStore, CommandJournal, Durability, IndexedCheckpoint};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");
const SESSION: SessionId = SessionId::new(7);
const CHILD: &str = "FLIFE_JOURNAL_CHILD";

/// Appendix A with `overrides`, each `(section, key, value)`.
fn config(overrides: &[(&str, &str, &str)]) -> ValidatedConfig {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    let mut lines: Vec<String> = appendix[start..start + length]
        .lines()
        .map(str::to_owned)
        .collect();
    for &(section, key, value) in overrides {
        let header = format!("[{section}]");
        let first = lines
            .iter()
            .position(|line| line.trim() == header)
            .expect("a section");
        let line = lines[first + 1..]
            .iter()
            .take_while(|line| !line.starts_with('['))
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .map(|offset| first + 1 + offset)
            .expect("a key");
        lines[line] = format!("{key} = {value}");
    }
    ValidatedConfig::from_toml(&lines.join("\n")).expect("a valid configuration")
}

/// FL-013's small ecology (32 × 32, 48 founders, four sources) with
/// Appendix A's `write-ahead-before-apply` durability.
fn ecology() -> ValidatedConfig {
    ecology_with(&[])
}

/// [`ecology`] with `extra` overrides.
fn ecology_with(extra: &[(&str, &str, &str)]) -> ValidatedConfig {
    let mut overrides = vec![
        ("world", "width", "32.0"),
        ("world", "height", "32.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", "48"),
        ("population", "maximum_count", "1024"),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "4"),
        ("sources", "power_per_source", "120.0"),
        ("sources", "sigma_world_units", "3.0"),
        ("sources", "path_direction_interval_seconds", "2.0"),
        ("chemistry", "uptake_capacity_per_second", "8.0"),
        ("costs", "basal_energy_per_second", "0.6"),
        (
            "reproduction",
            "minimum_parent_energy_before_attempt",
            "2.2",
        ),
        ("reproduction", "maximum_attempt_rate_per_second", "4.0"),
        ("reproduction", "cooldown_seconds", "0.25"),
    ];
    overrides.extend_from_slice(extra);
    let config = config(&overrides);
    assert_eq!(
        config.config().execution.command_durability,
        "write-ahead-before-apply"
    );
    config
}

fn profile() -> NumericalProfile {
    NumericalProfile::reference_f32().expect("profile")
}

fn found(config: &ValidatedConfig) -> World {
    World::found(config, Arrangement::FreeList, profile()).expect("the world founds")
}

fn book(config: &ValidatedConfig) -> CommandBook {
    let limits = SessionConfig::from_config(SESSION, config).expect("limits");
    CommandBook::new(SESSION, config.config_hash(), limits.command_capacity)
}

/// The world `config` founds, stepped directly to `ticks` with each
/// `(tick, intervention)` applied at its tick.
fn reference(config: &ValidatedConfig, ticks: u64, schedule: &[(u64, Intervention)]) -> World {
    let mut world = found(config);
    while world.tick().get() < ticks {
        let now = world.tick().get();
        let due: Vec<Intervention> = schedule
            .iter()
            .filter(|&&(tick, _)| tick == now)
            .map(|&(_, intervention)| intervention)
            .collect();
        match world.step_with(&due).expect("the tick advances") {
            Stepped::Published(report) => assert_eq!(report.interventions.len(), due.len()),
            Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
        }
    }
    world
}

const fn inject(cell: [u32; 2], amount: f64) -> Intervention {
    Intervention::InjectEnergy { cell, amount }
}

const fn at(tick: u64) -> Target {
    Target::At {
        tick: Tick::new(tick),
        retarget: false,
    }
}

const fn command(id: u64, target: Target, intervention: Intervention) -> Command {
    Command {
        session: SESSION,
        id: CommandId::new(id),
        target,
        expected_revision: None,
        intervention,
    }
}

/// C.14's energy brush: command 1, 6.0 into cell (5, 5) at tick 3.
const fn brush() -> Command {
    command(1, at(3), inject([5, 5], 6.0))
}

/// A path for one test, with nothing left from an earlier run.
fn fresh(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("flife-journal")
        .join(name);
    if path.is_dir() {
        fs::remove_dir_all(&path).expect("clears the last run's directory");
    } else if path.exists() {
        fs::remove_file(&path).expect("clears the last run's file");
    }
    fs::create_dir_all(&path).expect("creates the directory");
    path
}

fn cx() -> Cx {
    Cx::current().expect("a task context")
}

/// A journal opener for `catalog`, for [`JournalWriter::spawn`].
fn catalog_at(
    catalog: &Path,
) -> impl FnOnce() -> Result<Box<dyn Journal>, CoreError> + Send + use<> {
    let catalog = catalog.to_path_buf();
    move || Ok(Box::new(CommandJournal::open(&catalog)?) as Box<dyn Journal>)
}

/// Open a journaled session of `config` on `world` and `book`, its journal
/// made by `open`.
fn journaled(
    config: &ValidatedConfig,
    open: impl FnOnce() -> Result<Box<dyn Journal>, CoreError> + Send + 'static,
    world: World,
    book: CommandBook,
) -> (SessionOwner, SessionHandle) {
    let (writer, contents) =
        JournalWriter::spawn(open, SESSION, config.config_hash()).expect("the journal opens");
    let limits = SessionConfig::from_config(SESSION, config).expect("limits");
    open_journaled(world, book, limits, writer, &contents).expect("the session opens")
}

/// Run `owner` and `client` on the native runtime; the session closes when
/// the client's handle goes, if the client did not close it.
fn drive<T, F>(
    owner: SessionOwner,
    handle: SessionHandle,
    client: impl FnOnce(SessionHandle) -> F,
) -> (T, SessionClosed)
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let runtime = RuntimeBuilder::current_thread().build().expect("runtime");
    let spawner = runtime.handle();
    let client = client(handle);
    runtime.block_on(async move {
        let owner = spawner
            .try_spawn(async move { owner.run(&cx()).await })
            .expect("the owner spawns");
        let client = spawner.try_spawn(client).expect("the client spawns");
        let result = client.await;
        (result, owner.await)
    })
}

/// What `catalog` journals for the test session.
fn contents(config: &ValidatedConfig, catalog: &Path) -> JournalContents {
    CommandJournal::open(catalog)
        .expect("the catalog opens")
        .open_run(SESSION, config.config_hash())
        .expect("the run opens")
}

#[track_caller]
fn accepted(admission: &Admission) -> (Accepted, bool) {
    match admission {
        Admission::Accepted {
            accepted, retry, ..
        } => (*accepted, *retry),
        Admission::Rejected(rejection) => panic!("rejected: {rejection:?}"),
    }
}

#[track_caller]
fn applied_at(status: Option<&CommandStatus>) -> Tick {
    match status {
        Some(CommandStatus::Terminal(Outcome::Applied { tick, .. })) => *tick,
        other => panic!("not applied: {other:?}"),
    }
}

/// The catalog: WAL with `synchronous = FULL`; acceptances numbered by
/// admission order and outcomes by cursor, each the next of its kind;
/// appends idempotent by identity and refused on conflicting content; the
/// two watermarks distinct; everything read back after reopening; and a
/// checkpoint index that never stands in for the chunk store.
#[test]
fn the_catalog_journals_idempotently_with_distinct_watermarks() {
    let config = ecology();
    let revision = config.config_hash();
    let dir = fresh("catalog");
    let catalog = dir.join("catalog.db");
    let a = command(10, at(40), inject([3, 3], 2.0));
    let b = command(11, at(12), inject([4, 4], 3.0));
    let accept = |command: Command, order: u64| JournalRecord::Accepted {
        command,
        accepted: Accepted {
            order,
            effective: match command.target {
                Target::At { tick, .. } => tick,
                Target::NextAdmissible => Tick::new(0),
            },
            retargeted: false,
            durability: flife_runtime::Durability::WriteAhead,
        },
    };
    let applied = Outcome::Applied {
        tick: Tick::new(12),
        published: Tick::new(13),
        epoch: flife_core::ids::ResourceEpoch::new(14),
        applied: 2.75,
        cursor: 0,
    };
    {
        let mut journal = CommandJournal::open(&catalog).expect("creates");
        assert_eq!(
            journal.modes().expect("modes"),
            ("wal".to_owned(), 2),
            "WAL, synchronous = FULL"
        );
        assert!(matches!(
            journal.append(&accept(a, 0)),
            Err(CoreError::StateConflict { .. })
        ));
        assert_eq!(
            journal.open_run(SESSION, revision).expect("opens"),
            JournalContents::default()
        );
        let mark = |admitted, outcomes| Watermarks { admitted, outcomes };
        assert_eq!(journal.append(&accept(a, 0)).expect("a"), mark(1, 0));
        assert_eq!(journal.append(&accept(b, 1)).expect("b"), mark(2, 0));
        assert_eq!(journal.append(&accept(a, 0)).expect("again"), mark(2, 0));
        let reused = command(10, at(40), inject([3, 3], 2.5));
        for refused in [
            accept(reused, 0),
            accept(command(12, at(50), inject([1, 1], 1.0)), 5),
            accept(command(12, at(50), inject([1, 1], 1.0)), 1),
            accept(command(12, at(50), inject([1, 1], 1.0)), 0),
            accept(
                Command {
                    session: SessionId::new(8),
                    ..command(12, at(50), inject([1, 1], 1.0))
                },
                2,
            ),
            JournalRecord::Ended {
                id: CommandId::new(99),
                outcome: applied.clone(),
            },
            JournalRecord::Ended {
                id: b.id,
                outcome: Outcome::Cancelled {
                    boundary: Tick::new(3),
                    reason: CancelReason::Requested,
                    cursor: 4,
                },
            },
        ] {
            assert!(
                matches!(
                    journal.append(&refused),
                    Err(CoreError::StateConflict { .. })
                ),
                "{refused:?}"
            );
        }
        // b ends first: the outcome watermark moves while a, accepted first,
        // stays pending.
        let ended = JournalRecord::Ended {
            id: b.id,
            outcome: applied.clone(),
        };
        assert_eq!(journal.append(&ended).expect("b ends"), mark(2, 1));
        assert_eq!(journal.append(&ended).expect("again"), mark(2, 1));
        assert!(matches!(
            journal.append(&JournalRecord::Ended {
                id: b.id,
                outcome: Outcome::Failed {
                    detail: "other".into(),
                    cursor: 0,
                },
            }),
            Err(CoreError::StateConflict { .. })
        ));
        assert_eq!(journal.watermarks().expect("watermarks"), mark(2, 1));
        let indexed = IndexedCheckpoint {
            sequence: 1,
            tick: Tick::new(13),
            epoch: flife_core::ids::ResourceEpoch::new(14),
            digest: Sha256Digest::of(b"a checkpoint"),
        };
        journal.index_checkpoint(indexed).expect("indexes");
        journal.index_checkpoint(indexed).expect("again");
        assert!(matches!(
            journal.index_checkpoint(IndexedCheckpoint {
                tick: Tick::new(14),
                ..indexed
            }),
            Err(CoreError::StateConflict { .. })
        ));
        assert_eq!(journal.checkpoints().expect("lists"), [indexed]);
    }
    // Reopened: everything appended, in order, and the run's revision.
    let mut journal = CommandJournal::open(&catalog).expect("reopens");
    let held = journal.open_run(SESSION, revision).expect("opens");
    let JournalRecord::Accepted {
        accepted: accepted_a,
        ..
    } = accept(a, 0)
    else {
        panic!("an acceptance");
    };
    let JournalRecord::Accepted {
        accepted: accepted_b,
        ..
    } = accept(b, 1)
    else {
        panic!("an acceptance");
    };
    assert_eq!(held.accepted, [(a, accepted_a), (b, accepted_b)]);
    assert_eq!(held.outcomes, [(b.id, applied)]);
    assert_eq!(
        held.watermarks(),
        Watermarks {
            admitted: 2,
            outcomes: 1
        }
    );
    assert_eq!(journal.runs().expect("runs"), [(SESSION, revision)]);
    assert_eq!(journal.checkpoints().expect("lists").len(), 1);
    drop(journal);
    // Appendix A as written: another configuration hash for the session.
    let other = crate::config(&[]);
    assert_ne!(other.config_hash(), revision);
    assert!(matches!(
        CommandJournal::open(&catalog)
            .expect("opens")
            .open_run(SESSION, other.config_hash()),
        Err(CoreError::StateConflict { .. })
    ));
    // Another catalog format is refused.
    block_on_inline(async {
        let connection = fsqlite::Connection::open(catalog.display().to_string())
            .await
            .expect("opens");
        connection
            .execute("UPDATE catalog SET value = 'frankenlife.run-catalog.v9' WHERE key = 'format'")
            .await
            .expect("updates");
        connection.close().await.expect("closes");
    });
    assert!(matches!(
        CommandJournal::open(&catalog),
        Err(CoreError::SchemaMismatch { expected, .. }) if expected == CATALOG_FORMAT
    ));
}

/// §12.3 under `write-ahead-before-apply`: an acceptance is answered as
/// durable only once the journal holds it; a retry returns it and a
/// conflicting reuse is refused; a cancellation is answered once durable;
/// commands apply at their ticks, and the journal holds each acceptance and
/// outcome exactly once. A session recovered from the journal alone, from
/// the founding world, replays to the same committed world.
#[test]
fn a_journaled_session_acknowledges_durable_records_and_replays_from_the_journal() {
    let config = ecology();
    let dir = fresh("session");
    let catalog = dir.join("catalog.db");
    let a = command(10, at(40), inject([3, 3], 2.0));
    let b = command(11, at(12), inject([4, 4], 3.0));
    let c = command(12, at(20), inject([6, 6], 1.5));
    let (owner, handle) = journaled(&config, catalog_at(&catalog), found(&config), book(&config));
    let (checkpoint, closed) = drive(owner, handle, move |handle| async move {
        let cx = cx();
        let (first, retry) = accepted(&handle.submit(&cx, a).await.expect("a"));
        assert!(!retry);
        assert_eq!(first.durability, flife_runtime::Durability::WriteAhead);
        assert_eq!((first.order, first.effective), (0, Tick::new(40)));
        let snapshot = handle.inspect(&cx).await.expect("inspect");
        let journal = snapshot.journal.expect("a journaled session");
        assert_eq!(journal.durable.admitted, 1, "answered once durable");
        accepted(&handle.submit(&cx, b).await.expect("b"));
        accepted(&handle.submit(&cx, c).await.expect("c"));
        let (again, retry) = accepted(&handle.submit(&cx, a).await.expect("retry"));
        assert_eq!((again, retry), (first, true));
        let reused = command(10, at(40), inject([3, 3], 9.0));
        assert!(matches!(
            handle.submit(&cx, reused).await.expect("answered"),
            Admission::Rejected(Rejection::ConflictingReuse { .. })
        ));
        let cancelled = handle.cancel(&cx, c.id).await.expect("cancel");
        assert!(matches!(
            cancelled,
            Cancellation::Cancelled {
                sealed: false,
                outcome: Outcome::Cancelled { cursor: 0, .. }
            }
        ));
        let snapshot = handle.inspect(&cx).await.expect("inspect");
        assert_eq!(
            snapshot.journal.expect("journaled").durable,
            Watermarks {
                admitted: 3,
                outcomes: 1
            }
        );
        let end = handle.run_until(&cx, Tick::new(13)).await.expect("runs");
        assert!(matches!(end, RunEnd::Reached(boundary) if boundary.tick == Tick::new(13)));
        assert_eq!(
            applied_at(handle.status(&cx, b.id).await.expect("status").as_ref()),
            Tick::new(12)
        );
        assert_eq!(
            handle.status(&cx, a.id).await.expect("status"),
            Some(CommandStatus::Pending)
        );
        assert!(matches!(
            handle.durable_outcome(&cx, b.id).await.expect("b"),
            Some(Outcome::Applied { cursor: 1, .. })
        ));
        assert_eq!(
            handle.durable_outcome(&cx, a.id).await.expect("a"),
            None,
            "a is unfinished"
        );
        let checkpoint = handle.checkpoint(&cx).await.expect("checkpoints");
        let snapshot = handle.inspect(&cx).await.expect("inspect");
        let journal = snapshot.journal.expect("journaled");
        assert_eq!(
            (journal.durable, journal.waiting),
            (
                Watermarks {
                    admitted: 3,
                    outcomes: 2
                },
                0
            ),
            "the checkpoint waited for every record; b's outcome is durable, a is pending"
        );
        handle.close(&cx).await.expect("closes");
        checkpoint
    });
    let expected = reference(&config, 13, &[(12, inject([4, 4], 3.0))]);
    assert_eq!(closed.world.digest(), expected.digest());
    assert_eq!(checkpoint.boundary.tick, Tick::new(13));
    let journal = closed.journal.expect("journaled");
    assert_eq!((journal.waiting, journal.failed), (0, None));
    assert_eq!(
        journal.durable,
        Watermarks {
            admitted: 3,
            outcomes: 3
        },
        "closing cancelled a, durably"
    );
    let held = contents(&config, &catalog);
    let ids: Vec<u64> = held.accepted.iter().map(|(c, _)| c.id.get()).collect();
    assert_eq!(
        ids,
        [10, 11, 12],
        "each acceptance once, in admission order"
    );
    let ended: Vec<(u64, &str)> = held
        .outcomes
        .iter()
        .map(|(id, outcome)| {
            (
                id.get(),
                match outcome {
                    Outcome::Applied { .. } => "applied",
                    Outcome::Cancelled {
                        reason: CancelReason::Requested,
                        ..
                    } => "cancelled",
                    Outcome::Cancelled { .. } => "closed",
                    Outcome::Failed { .. } => "failed",
                },
            )
        })
        .collect();
    assert_eq!(ended, [(12, "cancelled"), (11, "applied"), (10, "closed")]);

    // Recovered from the journal alone: the founding world replays to the
    // boundary of the last journaled outcome, a's session-closed
    // cancellation at tick 13, and reproduces every outcome.
    let (owner, handle) = journaled(&config, catalog_at(&catalog), found(&config), book(&config));
    let (snapshot, reopened) = drive(owner, handle, move |handle| async move {
        let cx = cx();
        let snapshot = handle.inspect(&cx).await.expect("inspect");
        let (_, retry) = accepted(&handle.submit(&cx, b).await.expect("retry"));
        assert!(retry, "b is recognised after recovery");
        handle.close(&cx).await.expect("closes");
        snapshot
    });
    assert_eq!(snapshot.boundary.tick, Tick::new(13));
    assert_eq!(snapshot.digest, expected.digest());
    assert_eq!((snapshot.admitted, snapshot.outcomes), (3, 3));
    assert_eq!(reopened.world.digest(), expected.digest());
    assert_eq!(contents(&config, &catalog), held, "nothing journaled twice");
    // The checkpoint's world is the same one.
    let restored = restore(&checkpoint.bytes, profile()).expect("restores");
    assert_eq!(restored.world.digest(), expected.digest());
}

/// A journal whose appends wait until the test lets each one through.
struct Gated {
    inner: CommandJournal,
    gate: std_mpsc::Receiver<()>,
}

impl Journal for Gated {
    fn open_run(
        &mut self,
        session: SessionId,
        revision: Sha256Digest,
    ) -> Result<JournalContents, CoreError> {
        self.inner.open_run(session, revision)
    }

    fn append(&mut self, record: &JournalRecord) -> Result<Watermarks, CoreError> {
        let _ = self.gate.recv();
        self.inner.append(record)
    }
}

/// §12.3–§12.4: while an acceptance waits for its receipt, the boundary it
/// is due at is held, a step requested there waits with it, and the control
/// lane (inspection, status, pause) answers meanwhile. Once the receipt
/// arrives the client hears of the acceptance and the step applies it.
#[test]
fn a_boundary_waits_for_its_receipt_while_control_stays_live() {
    let config = ecology();
    let dir = fresh("gated");
    let catalog = dir.join("catalog.db");
    let (open, gate) = std_mpsc::channel::<()>();
    let path = catalog.clone();
    let (owner, handle) = journaled(
        &config,
        move || {
            Ok(Box::new(Gated {
                inner: CommandJournal::open(&path)?,
                gate,
            }) as Box<dyn Journal>)
        },
        found(&config),
        book(&config),
    );
    let x = command(1, at(0), inject([7, 7], 4.0));
    let runtime = RuntimeBuilder::current_thread().build().expect("runtime");
    let spawner = runtime.handle();
    let closed = runtime.block_on(async move {
        let owner = spawner
            .try_spawn(async move { owner.run(&cx()).await })
            .expect("owner");
        let submitter = handle.clone();
        let submitted = spawner
            .try_spawn(async move { submitter.submit(&cx(), x).await })
            .expect("submitter");
        let stepper = handle.clone();
        let cx = cx();
        // The acceptance is in the book but not durable: the owner waits for
        // its receipt and still answers control.
        let waiting = loop {
            let snapshot = handle.inspect(&cx).await.expect("inspect");
            if snapshot.admitted == 1 {
                break snapshot;
            }
            yield_now().await;
        };
        let journal = waiting.journal.expect("journaled");
        assert_eq!((journal.durable.admitted, journal.waiting), (0, 1));
        let stepped = spawner
            .try_spawn(async move { stepper.step(&Cx::current().expect("a task context")).await })
            .expect("stepper");
        for _ in 0..64 {
            yield_now().await;
        }
        let held = handle.inspect(&cx).await.expect("inspect");
        assert_eq!(held.boundary.tick, Tick::new(0), "the boundary is held");
        assert_eq!(
            handle.status(&cx, x.id).await.expect("status"),
            Some(CommandStatus::Pending)
        );
        assert_eq!(handle.pause(&cx).await.expect("pause").tick, Tick::new(0));
        // A checkpoint waits too, so it never holds a record the journal
        // lacks.
        let taken = Arc::new(AtomicBool::new(false));
        let checkpointer = handle.clone();
        let flag = Arc::clone(&taken);
        let checkpointing = spawner
            .try_spawn(async move {
                let checkpoint = checkpointer
                    .checkpoint(&Cx::current().expect("a task context"))
                    .await;
                flag.store(true, Ordering::SeqCst);
                checkpoint
            })
            .expect("checkpointer");
        for _ in 0..64 {
            yield_now().await;
        }
        assert!(
            !taken.load(Ordering::SeqCst),
            "the checkpoint waits for the receipt"
        );
        // Let the acceptance through, then the applied outcome.
        open.send(()).expect("opens the gate");
        let (acceptance, retry) = accepted(&submitted.await.expect("durable"));
        assert!(!retry);
        assert_eq!(acceptance.effective, Tick::new(0));
        open.send(()).expect("opens the gate");
        let step = stepped.await.expect("steps");
        let StepOutcome::Published { applied, .. } = step else {
            panic!("not published: {step:?}");
        };
        assert_eq!(applied, [x.id]);
        let checkpoint = checkpointing.await.expect("checkpoints");
        drop(open);
        handle.close(&cx).await.expect("closes");
        (owner.await, checkpoint)
    });
    let (closed, checkpoint) = closed;
    let expected = reference(&config, 1, &[(0, inject([7, 7], 4.0))]);
    assert_eq!(closed.world.digest(), expected.digest());
    assert_eq!(
        closed.journal.expect("journaled").durable,
        Watermarks {
            admitted: 1,
            outcomes: 1
        }
    );
    let restored = restore(&checkpoint.bytes, profile()).expect("restores");
    assert_eq!(
        restored.world.digest(),
        reference(
            &config,
            checkpoint.boundary.tick.get(),
            &[(0, inject([7, 7], 4.0))]
        )
        .digest()
    );
}

/// §12.3–§12.4: while a cancellation waits for its receipt, the boundary
/// is held, so no epoch is published that a crash before the record is
/// durable could contradict; a step requested then waits with it, and the
/// control lane answers meanwhile. Once the receipt arrives the canceller
/// hears of the cancellation and the step publishes without the command.
#[test]
fn a_cancellation_holds_the_boundary_until_its_receipt() {
    let config = ecology();
    let dir = fresh("gated-cancel");
    let catalog = dir.join("catalog.db");
    let (open, gate) = std_mpsc::channel::<()>();
    let path = catalog.clone();
    let (owner, handle) = journaled(
        &config,
        move || {
            Ok(Box::new(Gated {
                inner: CommandJournal::open(&path)?,
                gate,
            }) as Box<dyn Journal>)
        },
        found(&config),
        book(&config),
    );
    let y = command(1, at(2), inject([7, 7], 4.0));
    let runtime = RuntimeBuilder::current_thread().build().expect("runtime");
    let spawner = runtime.handle();
    let closed = runtime.block_on(async move {
        let owner = spawner
            .try_spawn(async move { owner.run(&cx()).await })
            .expect("owner");
        let cx = cx();
        open.send(()).expect("opens the gate");
        accepted(&handle.submit(&cx, y).await.expect("durable"));
        let canceller = handle.clone();
        let cancelled = spawner
            .try_spawn(async move {
                canceller
                    .cancel(&Cx::current().expect("a task context"), y.id)
                    .await
            })
            .expect("canceller");
        // The cancellation is decided but not durable: the owner waits for
        // its receipt and still answers control.
        let waiting = loop {
            let snapshot = handle.inspect(&cx).await.expect("inspect");
            if snapshot.outcomes == 1 {
                break snapshot;
            }
            yield_now().await;
        };
        let journal = waiting.journal.expect("journaled");
        assert_eq!((journal.durable.outcomes, journal.waiting), (0, 1));
        let stepper = handle.clone();
        let stepped = spawner
            .try_spawn(async move { stepper.step(&Cx::current().expect("a task context")).await })
            .expect("stepper");
        for _ in 0..64 {
            yield_now().await;
        }
        let held = handle.inspect(&cx).await.expect("inspect");
        assert_eq!(held.boundary.tick, Tick::new(0), "the boundary is held");
        assert!(matches!(
            handle.status(&cx, y.id).await.expect("status"),
            Some(CommandStatus::Terminal(Outcome::Cancelled { .. }))
        ));
        // Let the cancellation through: the canceller hears, then the step.
        open.send(()).expect("opens the gate");
        let decision = cancelled.await.expect("durable");
        assert!(
            matches!(decision, Cancellation::Cancelled { sealed: false, .. }),
            "{decision:?}"
        );
        let step = stepped.await.expect("steps");
        let StepOutcome::Published { applied, .. } = step else {
            panic!("not published: {step:?}");
        };
        assert!(applied.is_empty());
        drop(open);
        handle.close(&cx).await.expect("closes");
        owner.await
    });
    assert_eq!(closed.world.digest(), reference(&config, 1, &[]).digest());
    assert_eq!(
        closed.journal.expect("journaled").durable,
        Watermarks {
            admitted: 1,
            outcomes: 1
        }
    );
}

/// A journal that persists a record and then reports a failure, as a lost
/// receipt: the first `lose` acceptances it appends.
struct LosesReceipts {
    inner: CommandJournal,
    lose: u32,
}

impl Journal for LosesReceipts {
    fn open_run(
        &mut self,
        session: SessionId,
        revision: Sha256Digest,
    ) -> Result<JournalContents, CoreError> {
        self.inner.open_run(session, revision)
    }

    fn append(&mut self, record: &JournalRecord) -> Result<Watermarks, CoreError> {
        let receipt = self.inner.append(record)?;
        if matches!(record, JournalRecord::Accepted { .. }) && self.lose > 0 {
            self.lose -= 1;
            return Err(CoreError::StorageFailed {
                operation: "append acceptance",
                path: self.inner.path().display().to_string(),
                reason: "the receipt was lost after the record persisted".into(),
            });
        }
        Ok(receipt)
    }
}

/// C.14: a receipt lost after the record actually persisted. Reconciling by
/// the same identity finds the record instead of applying it twice: once
/// within the writer's retries, and once across a restart after every
/// retry failed and the client was told the outcome is uncertain.
#[test]
fn c14_a_lost_receipt_is_reconciled_by_identity_and_applies_once() {
    let config = ecology();
    let schedule = [(3, inject([5, 5], 6.0))];
    let expected = reference(&config, 5, &schedule);

    // One lost receipt: the writer's retry finds the persisted record.
    let dir = fresh("lost-once");
    let catalog = dir.join("catalog.db");
    let path = catalog.clone();
    let (owner, handle) = journaled(
        &config,
        move || {
            Ok(Box::new(LosesReceipts {
                inner: CommandJournal::open(&path)?,
                lose: 1,
            }) as Box<dyn Journal>)
        },
        found(&config),
        book(&config),
    );
    let ((), closed) = drive(owner, handle, |handle| async move {
        let cx = cx();
        let (_, retry) = accepted(&handle.submit(&cx, brush()).await.expect("durable"));
        assert!(!retry);
        handle.run_until(&cx, Tick::new(5)).await.expect("runs");
        handle.close(&cx).await.expect("closes");
    });
    assert_eq!(closed.world.digest(), expected.digest());
    let held = contents(&config, &catalog);
    assert_eq!((held.accepted.len(), held.outcomes.len()), (1, 1));

    // Every attempt loses its receipt: the client hears an uncertain
    // failure, the run stops, control and close still answer.
    let dir = fresh("lost-always");
    let catalog = dir.join("catalog.db");
    let path = catalog.clone();
    let (owner, handle) = journaled(
        &config,
        move || {
            Ok(Box::new(LosesReceipts {
                inner: CommandJournal::open(&path)?,
                lose: flife_runtime::JOURNAL_ATTEMPTS,
            }) as Box<dyn Journal>)
        },
        found(&config),
        book(&config),
    );
    let ((), closed) = drive(owner, handle, |handle| async move {
        let cx = cx();
        let uncertain = handle.submit(&cx, brush()).await;
        assert!(
            matches!(uncertain, Err(CoreError::StorageFailed { .. })),
            "{uncertain:?}"
        );
        let snapshot = handle.inspect(&cx).await.expect("inspect");
        assert!(snapshot.journal.expect("journaled").failed.is_some());
        let step = handle.step(&cx).await.expect("answered");
        assert!(
            matches!(
                step,
                StepOutcome::Failed {
                    error: CoreError::StorageFailed { .. },
                    ..
                }
            ),
            "{step:?}"
        );
        assert!(
            handle.submit(&cx, brush()).await.is_err(),
            "the uncertain identity stays uncertain"
        );
        assert!(
            handle
                .submit(&cx, command(2, at(9), inject([1, 1], 1.0)))
                .await
                .is_err(),
            "nothing new is admitted"
        );
        assert_eq!(
            handle.status(&cx, CommandId::new(2)).await.expect("status"),
            None,
            "a refused command is not recorded"
        );
        handle.close(&cx).await.expect("closes");
    });
    assert_eq!(closed.boundary.tick, Tick::new(0));
    // The record did persist; recovery finds it, and the retry applies once.
    let held = contents(&config, &catalog);
    assert_eq!((held.accepted.len(), held.outcomes.len()), (1, 0));
    let (owner, handle) = journaled(&config, catalog_at(&catalog), found(&config), book(&config));
    let ((), closed) = drive(owner, handle, |handle| async move {
        let cx = cx();
        let (acceptance, retry) = accepted(&handle.submit(&cx, brush()).await.expect("retry"));
        assert!(retry, "reconciled by identity, not admitted again");
        assert_eq!(acceptance.order, 0);
        handle.run_until(&cx, Tick::new(5)).await.expect("runs");
        handle.close(&cx).await.expect("closes");
    });
    assert_eq!(closed.world.digest(), expected.digest());
    assert_eq!(
        applied_at(closed.book.status(brush().id)),
        Tick::new(3),
        "applied once, at its tick"
    );
    let held = contents(&config, &catalog);
    assert_eq!((held.accepted.len(), held.outcomes.len()), (1, 1));
}

/// C.14: a cancellation racing a sealed candidate and publication has one
/// winner, and the journal records that one. Cancelled at the cancellation
/// boundary, the candidate is discarded and rebuilt without the command
/// once the cancellation is durable; cancelled after publication, it is too
/// late and the journal holds its application.
#[test]
fn c14_a_cancellation_racing_publication_has_one_durable_winner() {
    let config = ecology();
    let x = command(1, at(0), inject([5, 5], 6.0));
    for wins in [true, false] {
        let dir = fresh(&format!("race-{wins}"));
        let catalog = dir.join("catalog.db");
        let (owner, handle) =
            journaled(&config, catalog_at(&catalog), found(&config), book(&config));
        let (decision, closed) = drive(owner, handle, move |handle| async move {
            let cx = cx();
            accepted(&handle.submit(&cx, x).await.expect("durable"));
            let decision = if wins {
                // The owner seals x, prepares the candidate and yields at the
                // cancellation boundary, where this cancellation arrives.
                let running = handle.start(&cx, Some(Tick::new(1))).await.expect("starts");
                let decision = handle.cancel(&cx, x.id).await.expect("cancel");
                running.ended(&cx).await.expect("ends");
                decision
            } else {
                handle.run_until(&cx, Tick::new(1)).await.expect("runs");
                handle.cancel(&cx, x.id).await.expect("cancel")
            };
            handle.close(&cx).await.expect("closes");
            decision
        });
        let held = contents(&config, &catalog);
        assert_eq!(held.outcomes.len(), 1, "one outcome, journaled once");
        if wins {
            assert!(
                matches!(decision, Cancellation::Cancelled { sealed: true, .. }),
                "{decision:?}"
            );
            assert!(matches!(held.outcomes[0].1, Outcome::Cancelled { .. }));
            assert_eq!(
                closed.world.digest(),
                reference(&config, 1, &[]).digest(),
                "the published world never held x"
            );
        } else {
            assert!(matches!(decision, Cancellation::TooLate(_)), "{decision:?}");
            assert!(matches!(held.outcomes[0].1, Outcome::Applied { .. }));
            assert_eq!(
                closed.world.digest(),
                reference(&config, 1, &[(0, inject([5, 5], 6.0))]).digest()
            );
        }
    }
}

/// §12.4's bound on records awaiting receipts: with a command capacity of 4
/// and a control capacity of 1, at most 5 records wait. Four applied
/// outcomes and one acceptance waiting stop the owner taking submissions
/// (the rest queue on the scientific lane) and hold the boundary, while
/// control answers. Once the receipts arrive, the queued commands are
/// admitted and everything applies at its tick.
#[test]
fn records_awaiting_receipts_are_bounded() {
    let config = ecology_with(&[
        ("execution", "command_queue_capacity", "4"),
        ("execution", "control_queue_capacity", "1"),
    ]);
    let dir = fresh("bounded");
    let catalog = dir.join("catalog.db");
    let (open, gate) = std_mpsc::channel::<()>();
    let path = catalog.clone();
    let (owner, handle) = journaled(
        &config,
        move || {
            Ok(Box::new(Gated {
                inner: CommandJournal::open(&path)?,
                gate,
            }) as Box<dyn Journal>)
        },
        found(&config),
        book(&config),
    );
    let early = [
        command(1, at(1), inject([1, 1], 1.0)),
        command(2, at(1), inject([2, 1], 1.0)),
        command(3, at(1), inject([3, 1], 1.0)),
        command(4, at(1), inject([4, 1], 1.0)),
    ];
    let late = [
        command(5, at(9), inject([5, 1], 1.0)),
        command(6, at(9), inject([6, 1], 1.0)),
        command(7, at(9), inject([7, 1], 1.0)),
        command(8, at(9), inject([8, 1], 1.0)),
    ];
    let runtime = RuntimeBuilder::current_thread().build().expect("runtime");
    let spawner = runtime.handle();
    let closed = runtime.block_on(async move {
        let owner = spawner
            .try_spawn(async move { owner.run(&cx()).await })
            .expect("owner");
        let cx = cx();
        for command in early {
            open.send(()).expect("lets one append through");
            accepted(&handle.submit(&cx, command).await.expect("durable"));
        }
        // The four apply at tick 1; their outcomes wait for receipts, which
        // never holds a boundary below the limit.
        let end = handle.run_until(&cx, Tick::new(2)).await.expect("runs");
        assert!(matches!(end, RunEnd::Reached(boundary) if boundary.tick == Tick::new(2)));
        let mut submitted = Vec::new();
        for command in late {
            let submitter = handle.clone();
            submitted.push(
                spawner
                    .try_spawn(async move {
                        submitter
                            .submit(&Cx::current().expect("a task context"), command)
                            .await
                    })
                    .expect("submitter"),
            );
        }
        // The control lane is served first, so wait for the limit before
        // asking for a step.
        while handle.inspect(&cx).await.expect("inspect").admitted < 5 {
            yield_now().await;
        }
        for _ in 0..64 {
            yield_now().await;
        }
        let held = handle.inspect(&cx).await.expect("inspect");
        let journal = held.journal.expect("journaled");
        assert_eq!(
            (held.admitted, journal.waiting),
            (5, 5),
            "one more acceptance reaches the limit; the rest wait unadmitted"
        );
        assert_eq!(
            journal.durable,
            Watermarks {
                admitted: 4,
                outcomes: 0
            }
        );
        let stepper = handle.clone();
        let stepped = spawner
            .try_spawn(async move { stepper.step(&Cx::current().expect("a task context")).await })
            .expect("stepper");
        for _ in 0..64 {
            yield_now().await;
        }
        let held = handle.inspect(&cx).await.expect("inspect");
        assert_eq!(held.boundary.tick, Tick::new(2), "the boundary is held");
        drop(open);
        for submission in submitted {
            accepted(&submission.await.expect("durable"));
        }
        let step = stepped.await.expect("steps");
        assert!(matches!(step, StepOutcome::Published { .. }), "{step:?}");
        handle.run_until(&cx, Tick::new(10)).await.expect("runs");
        handle.close(&cx).await.expect("closes");
        owner.await
    });
    let schedule: Vec<(u64, Intervention)> = early
        .iter()
        .map(|sent| (1, sent.intervention))
        .chain(late.iter().map(|sent| (9, sent.intervention)))
        .collect();
    assert_eq!(
        closed.world.digest(),
        reference(&config, 10, &schedule).digest()
    );
    assert_eq!(
        closed.journal.expect("journaled").durable,
        Watermarks {
            admitted: 8,
            outcomes: 8
        }
    );
}

/// A journal that fails its first `fails` appends, once the test lets the
/// first through, and records every later append.
struct FailsFirst {
    gate: std_mpsc::Receiver<()>,
    fails: u32,
    appended: Arc<Mutex<Vec<JournalRecord>>>,
}

impl Journal for FailsFirst {
    fn open_run(
        &mut self,
        _session: SessionId,
        _revision: Sha256Digest,
    ) -> Result<JournalContents, CoreError> {
        Ok(JournalContents::default())
    }

    fn append(&mut self, record: &JournalRecord) -> Result<Watermarks, CoreError> {
        if self.fails > 0 {
            let _ = self.gate.recv();
            self.fails -= 1;
            return Err(CoreError::StorageFailed {
                operation: "append",
                path: "a test journal".into(),
                reason: "refused".into(),
            });
        }
        let mut appended = self.appended.lock().expect("the record list");
        appended.push(record.clone());
        Ok(Watermarks {
            admitted: u64::try_from(appended.len()).expect("a count"),
            outcomes: 0,
        })
    }
}

/// §12.3: once an append fails every retry, nothing sent after it becomes
/// durable, even where the journal would take it, so the journal never
/// holds a record without every earlier one. Both clients hear the failure.
#[test]
fn a_failed_append_fails_every_record_behind_it() {
    let config = ecology();
    let (open, gate) = std_mpsc::channel::<()>();
    let appended = Arc::new(Mutex::new(Vec::new()));
    let records = Arc::clone(&appended);
    let (owner, handle) = journaled(
        &config,
        move || {
            Ok(Box::new(FailsFirst {
                gate,
                fails: flife_runtime::JOURNAL_ATTEMPTS,
                appended: records,
            }) as Box<dyn Journal>)
        },
        found(&config),
        book(&config),
    );
    let a = command(1, at(5), inject([1, 1], 1.0));
    let b = command(2, at(6), inject([2, 2], 1.0));
    let runtime = RuntimeBuilder::current_thread().build().expect("runtime");
    let spawner = runtime.handle();
    let closed = runtime.block_on(async move {
        let owner = spawner
            .try_spawn(async move { owner.run(&cx()).await })
            .expect("owner");
        let cx = cx();
        let mut submitted = Vec::new();
        for command in [a, b] {
            let submitter = handle.clone();
            submitted.push(
                spawner
                    .try_spawn(async move {
                        submitter
                            .submit(&Cx::current().expect("a task context"), command)
                            .await
                    })
                    .expect("submitter"),
            );
        }
        // Both records are with the writer before the first is tried.
        loop {
            if handle.inspect(&cx).await.expect("inspect").admitted == 2 {
                break;
            }
            yield_now().await;
        }
        drop(open);
        for submission in submitted {
            let answer = submission.await;
            assert!(
                matches!(answer, Err(CoreError::StorageFailed { .. })),
                "{answer:?}"
            );
        }
        handle.close(&cx).await.expect("closes");
        owner.await
    });
    assert!(
        appended.lock().expect("the record list").is_empty(),
        "nothing after the failed record was appended"
    );
    assert!(closed.journal.expect("journaled").failed.is_some());
}

/// `open_journaled` refuses a journal that disagrees with its starting
/// point, naming the first disagreement, as a damaged, foreign or edited
/// journal could: one behind the checkpoint; an acceptance with other
/// content than the checkpoint's; a new command due before the checkpoint;
/// an outcome at a boundary already passed; an outcome the replay does not
/// reproduce; and a cancellation the book cannot make. The unaltered
/// journal opens from the checkpoint and from the founding world.
#[test]
fn recovery_refuses_a_journal_that_disagrees_with_its_starting_point() {
    let config = ecology();
    let dir = fresh("refusals");
    let catalog = dir.join("catalog.db");
    let a = command(10, at(40), inject([3, 3], 2.0));
    let b = command(11, at(12), inject([4, 4], 3.0));
    let (owner, handle) = journaled(&config, catalog_at(&catalog), found(&config), book(&config));
    let (checkpoint, _) = drive(owner, handle, move |handle| async move {
        let cx = cx();
        accepted(&handle.submit(&cx, a).await.expect("a"));
        accepted(&handle.submit(&cx, b).await.expect("b"));
        handle.run_until(&cx, Tick::new(13)).await.expect("runs");
        let checkpoint = handle.checkpoint(&cx).await.expect("checkpoints");
        handle.close(&cx).await.expect("closes");
        checkpoint
    });
    // a and b accepted; b applied at 12, a cancelled by the close at 13.
    let held = contents(&config, &catalog);
    assert_eq!((held.accepted.len(), held.outcomes.len()), (2, 2));
    let checkpointed = || {
        let restored = restore(&checkpoint.bytes, profile()).expect("restores");
        (restored.world, restored.book)
    };
    let open = |(world, book): (World, CommandBook), journal: &JournalContents| {
        let spare = fresh("refusals-writer").join("catalog.db");
        let (writer, _) = JournalWriter::spawn(catalog_at(&spare), SESSION, config.config_hash())
            .expect("the journal opens");
        let limits = SessionConfig::from_config(SESSION, &config).expect("limits");
        open_journaled(world, book, limits, writer, journal).map(|_| ())
    };
    open(checkpointed(), &held).expect("the journal opens from its checkpoint");
    open((found(&config), book(&config)), &held).expect("and from the founding world");

    conflict(
        open(checkpointed(), &JournalContents::default()),
        "recover command book",
        "acceptances",
    );
    let mut other = held.clone();
    other.accepted[0].0 = command(10, at(40), inject([3, 3], 9.0));
    conflict(
        open(checkpointed(), &other),
        "recover command book",
        "differs",
    );
    let mut early = held.clone();
    early.accepted.push((
        command(12, at(5), inject([5, 5], 1.0)),
        Accepted {
            order: 2,
            effective: Tick::new(5),
            retargeted: false,
            durability: flife_runtime::Durability::WriteAhead,
        },
    ));
    conflict(
        open(checkpointed(), &early),
        "recover command book",
        "before the committed tick",
    );
    let mut passed = held.clone();
    passed.outcomes[1].1 = Outcome::Cancelled {
        boundary: Tick::new(5),
        reason: CancelReason::Requested,
        cursor: 1,
    };
    conflict(
        open(checkpointed(), &passed),
        "recover command book",
        "cannot be reached",
    );
    let mut diverged = held.clone();
    let Outcome::Applied {
        tick,
        published,
        epoch,
        applied,
        cursor,
    } = diverged.outcomes[0].1.clone()
    else {
        panic!("b applied first");
    };
    diverged.outcomes[0].1 = Outcome::Applied {
        tick,
        published,
        epoch,
        applied: applied * 2.0,
        cursor,
    };
    conflict(
        open((found(&config), book(&config)), &diverged),
        "replay command journal",
        "the replay recorded",
    );
    let mut impossible = held.clone();
    impossible.outcomes[1] = (
        b.id,
        Outcome::Cancelled {
            boundary: Tick::new(13),
            reason: CancelReason::Requested,
            cursor: 1,
        },
    );
    conflict(
        open(checkpointed(), &impossible),
        "replay command journal",
        "not reproduced",
    );
}

/// Assert `refusal` is a state conflict from `operation` naming `detail`.
#[track_caller]
fn conflict(refusal: Result<(), CoreError>, operation: &str, detail: &str) {
    match refusal {
        Err(CoreError::StateConflict {
            operation: refused,
            detail: why,
        }) if refused == operation && why.contains(detail) => {}
        other => panic!("expected {operation} ({detail}), got {other:?}"),
    }
}

/// A journal that reports each append of the child's scenario and, at its
/// planned point, waits to be killed.
struct Traced {
    inner: CommandJournal,
    stop: String,
}

/// Report `name` and wait to be killed if it is the planned point.
fn point(stop: &str, name: &str) {
    if stop == name {
        let mut out = std::io::stdout();
        writeln!(out, "FLIFE-POINT {name}").expect("reports");
        out.flush().expect("flushes");
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }
}

impl Journal for Traced {
    fn open_run(
        &mut self,
        session: SessionId,
        revision: Sha256Digest,
    ) -> Result<JournalContents, CoreError> {
        self.inner.open_run(session, revision)
    }

    fn append(&mut self, record: &JournalRecord) -> Result<Watermarks, CoreError> {
        let kind = match record {
            JournalRecord::Accepted { .. } => "accept",
            JournalRecord::Ended {
                outcome: Outcome::Applied { .. },
                ..
            } => "outcome",
            JournalRecord::Ended { .. } => "end",
        };
        point(&self.stop, &format!("before-{kind}"));
        let receipt = self.inner.append(record);
        point(&self.stop, &format!("after-{kind}"));
        receipt
    }
}

/// The points C.14 interrupts a brush at, in the order they occur: before
/// durable acceptance, after it (before the client hears), after the
/// client's acknowledgement (before publication), after publication (before
/// its outcome is durable), after the outcome is durable (before the
/// terminal response reaches the client), and after the client has the
/// explicitly durable applied response.
const POINTS: [&str; 6] = [
    "before-accept",
    "after-accept",
    "acknowledged",
    "before-outcome",
    "after-outcome",
    "published",
];

/// Run by [`c14_a_crash_at_each_point_recovers_and_a_retry_applies_once`]
/// and [`c14_a_crash_keeps_a_durable_cancellation_and_commands_beyond_the_checkpoint`]
/// in a child process. Does nothing in an ordinary test run.
#[test]
fn child_runs_a_journaled_session_until_its_planned_crash() {
    let Ok(plan) = std::env::var(CHILD) else {
        return;
    };
    let parts: Vec<&str> = plan.split('|').collect();
    let [scenario, dir, stop] = parts[..] else {
        panic!("a plan of three parts: {plan}");
    };
    let (scenario, dir, stop) = (scenario.to_owned(), PathBuf::from(dir), stop.to_owned());
    let config = ecology();
    let catalog = dir.join("catalog.db");
    let traced = stop.clone();
    let path = catalog.clone();
    let (owner, handle) = journaled(
        &config,
        move || {
            Ok(Box::new(Traced {
                inner: CommandJournal::open(&path)?,
                stop: traced,
            }) as Box<dyn Journal>)
        },
        found(&config),
        book(&config),
    );
    let revision = config.config_hash();
    let ((), _) = drive(owner, handle, move |handle| async move {
        let cx = cx();
        match scenario.as_str() {
            "brush" => {
                accepted(&handle.submit(&cx, brush()).await.expect("durable"));
                point(&stop, "acknowledged");
                handle.run_until(&cx, Tick::new(5)).await.expect("runs");
                // The explicitly durable applied response.
                let outcome = handle
                    .durable_outcome(&cx, brush().id)
                    .await
                    .expect("answered");
                assert!(
                    matches!(outcome, Some(Outcome::Applied { .. })),
                    "{outcome:?}"
                );
                point(&stop, "published");
            }
            "schedule" => {
                for command in schedule() {
                    accepted(&handle.submit(&cx, command).await.expect("durable"));
                }
                let cancelled = handle
                    .cancel(&cx, CommandId::new(22))
                    .await
                    .expect("cancel");
                assert!(matches!(cancelled, Cancellation::Cancelled { .. }));
                handle.run_until(&cx, Tick::new(13)).await.expect("runs");
                let checkpoint = handle.checkpoint(&cx).await.expect("checkpoints");
                let mut store =
                    ChunkStore::open(dir.join("store"), Durability::ProcessCrash).expect("store");
                let published = store.publish(&checkpoint.bytes).expect("publishes");
                // A second connection, on its own thread: a journal connection
                // blocks its thread, so it never runs on a runtime worker.
                let indexed = IndexedCheckpoint {
                    sequence: published.sequence,
                    tick: checkpoint.boundary.tick,
                    epoch: checkpoint.boundary.epoch,
                    digest: Sha256Digest::of(&checkpoint.bytes),
                };
                std::thread::spawn(move || {
                    let mut index = CommandJournal::open(&catalog).expect("a second connection");
                    index.open_run(SESSION, revision).expect("opens");
                    index.index_checkpoint(indexed).expect("indexes");
                })
                .join()
                .expect("the index thread");
                point(&stop, "checkpointed");
            }
            other => panic!("no scenario {other}"),
        }
        handle.close(&cx).await.expect("closes");
    });
    println!("FLIFE-DONE");
}

/// Start a child running `scenario` in `dir` and kill it (`SIGKILL`) at
/// `stop`. Whether it reached the point.
fn crash_at(scenario: &str, dir: &Path, stop: &str) -> bool {
    let mut child = Process::new(std::env::current_exe().expect("this test binary"))
        .args([
            "--exact",
            "child_runs_a_journaled_session_until_its_planned_crash",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env(CHILD, format!("{scenario}|{}|{stop}", dir.display()))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawns");
    let stdout = child.stdout.take().expect("a pipe");
    for line in BufReader::new(stdout).lines() {
        let line = line.expect("a line");
        if line.contains(&format!("FLIFE-POINT {stop}")) {
            child.kill().expect("kills");
            child.wait().expect("reaps");
            return true;
        }
        if line.contains("FLIFE-DONE") {
            assert!(child.wait().expect("waits").success());
            return false;
        }
    }
    panic!("the child ended without a report: {:?}", child.wait());
}

/// C.14: one energy brush, killed at each point. Recovery from the journal
/// (and the founding world) holds the acceptance exactly when it was
/// durable; the client's retry with the same identity and payload is then
/// recognised (or, if it never became durable, admitted afresh), a
/// conflicting reuse is refused, and the brush applies once, at its tick:
/// the committed world equals one stepped directly with it applied once.
#[test]
fn c14_a_crash_at_each_point_recovers_and_a_retry_applies_once() {
    let config = ecology();
    let expected = reference(&config, 6, &[(3, inject([5, 5], 6.0))]);
    for stop in POINTS {
        let dir = fresh(&format!("brush-{stop}"));
        assert!(crash_at("brush", &dir, stop), "{stop} was reached");
        let catalog = dir.join("catalog.db");
        let held = contents(&config, &catalog);
        let durable = stop != "before-accept";
        let outcome = matches!(stop, "after-outcome" | "published");
        assert_eq!(
            (held.accepted.len(), held.outcomes.len()),
            (usize::from(durable), usize::from(outcome)),
            "{stop}: what the journal holds"
        );
        let (owner, handle) =
            journaled(&config, catalog_at(&catalog), found(&config), book(&config));
        let (start, closed) = drive(owner, handle, move |handle| async move {
            let cx = cx();
            let start = handle.inspect(&cx).await.expect("inspect").boundary.tick;
            let (acceptance, retry) =
                accepted(&handle.submit(&cx, brush()).await.expect("the retry"));
            assert_eq!(retry, durable, "{stop}: recognised iff it was durable");
            assert_eq!((acceptance.order, acceptance.effective), (0, Tick::new(3)));
            let reused = command(1, at(3), inject([5, 5], 7.0));
            assert!(matches!(
                handle.submit(&cx, reused).await.expect("answered"),
                Admission::Rejected(Rejection::ConflictingReuse { .. })
            ));
            handle.run_until(&cx, Tick::new(6)).await.expect("runs");
            handle.close(&cx).await.expect("closes");
            start
        });
        // Replay reaches the journaled outcome's publication, and no further.
        assert_eq!(
            start,
            Tick::new(if outcome { 4 } else { 0 }),
            "{stop}: the recovered boundary"
        );
        assert_eq!(
            closed.world.digest(),
            expected.digest(),
            "{stop}: applied once"
        );
        assert_eq!(applied_at(closed.book.status(brush().id)), Tick::new(3));
        let held = contents(&config, &catalog);
        assert_eq!(
            (held.accepted.len(), held.outcomes.len()),
            (1, 1),
            "{stop}: one acceptance and one outcome"
        );
    }
}

/// A for tick 40 before B for tick 12, then C for tick 20, which is
/// cancelled.
fn schedule() -> [Command; 3] {
    [
        command(20, at(40), inject([3, 3], 2.0)),
        command(21, at(12), inject([4, 4], 3.0)),
        command(22, at(20), inject([6, 6], 1.5)),
    ]
}

/// C.14 and §16.6: accept A for tick 40 before B for tick 12, cancel C
/// durably, run past B, checkpoint into the chunk store, index it, and
/// crash. Recovered from the checkpoint and the journal, A is pending, B
/// applied once and C cancelled; neither A nor B is duplicated, and A
/// applies at tick 40. With the checkpoint's manifest damaged, its catalog
/// row does not make it resumable: recovery falls back to the founding
/// world and the journal, and replays to the same committed world.
#[test]
fn c14_a_crash_keeps_a_durable_cancellation_and_commands_beyond_the_checkpoint() {
    let config = ecology();
    let dir = fresh("schedule");
    assert!(crash_at("schedule", &dir, "checkpointed"));
    let catalog = dir.join("catalog.db");
    let [a, b, c] = schedule();
    let held = contents(&config, &catalog);
    assert_eq!(held.accepted.len(), 3);
    assert_eq!(
        held.outcomes.len(),
        2,
        "c's cancellation and b's application"
    );
    let mut index = CommandJournal::open(&catalog).expect("opens");
    index
        .open_run(SESSION, config.config_hash())
        .expect("opens the run");
    let indexed = index.checkpoints().expect("lists");
    drop(index);
    assert_eq!(indexed.len(), 1);
    assert_eq!(indexed[0].tick, Tick::new(13));

    let store_dir = dir.join("store");
    let recover = |config: &ValidatedConfig| {
        let store = ChunkStore::open(&store_dir, Durability::ProcessCrash).expect("store");
        let recovered = store
            .latest_valid(|_, bytes| restore(bytes, profile()))
            .expect("lists");
        let (world, book, sequence) = match recovered.latest {
            Some((sequence, session)) => (session.world, session.book, Some(sequence)),
            None => (found(config), book(config), None),
        };
        (world, book, sequence, recovered.skipped.len())
    };
    let expected_13 = reference(&config, 13, &[(12, inject([4, 4], 3.0))]);
    let (world, book_13, sequence, skipped) = recover(&config);
    assert_eq!((sequence, skipped), (Some(indexed[0].sequence), 0));
    assert_eq!(world.tick(), Tick::new(13));
    assert_eq!(world.digest(), expected_13.digest());
    let (owner, handle) = journaled(&config, catalog_at(&catalog), world, book_13);
    let (statuses, closed) = drive(owner, handle, move |handle| async move {
        let cx = cx();
        let start = handle.inspect(&cx).await.expect("inspect");
        assert_eq!(start.boundary.tick, Tick::new(13));
        assert_eq!((start.admitted, start.outcomes), (3, 2));
        let statuses = [
            handle.status(&cx, a.id).await.expect("a"),
            handle.status(&cx, b.id).await.expect("b"),
            handle.status(&cx, c.id).await.expect("c"),
        ];
        for command in [a, b, c] {
            let (_, retry) = accepted(&handle.submit(&cx, command).await.expect("retry"));
            assert!(retry, "{} is recognised", command.id);
        }
        handle.run_until(&cx, Tick::new(41)).await.expect("runs");
        handle.close(&cx).await.expect("closes");
        statuses
    });
    assert_eq!(
        statuses[0],
        Some(CommandStatus::Pending),
        "A is still pending"
    );
    assert_eq!(applied_at(statuses[1].as_ref()), Tick::new(12));
    assert!(matches!(
        statuses[2],
        Some(CommandStatus::Terminal(Outcome::Cancelled {
            reason: CancelReason::Requested,
            ..
        }))
    ));
    let expected_41 = reference(
        &config,
        41,
        &[(12, inject([4, 4], 3.0)), (40, inject([3, 3], 2.0))],
    );
    assert_eq!(closed.world.digest(), expected_41.digest());
    assert_eq!(applied_at(closed.book.status(a.id)), Tick::new(40));
    let held = contents(&config, &catalog);
    let ends: Vec<u64> = held.outcomes.iter().map(|(id, _)| id.get()).collect();
    assert_eq!(ends, [22, 21, 20], "each outcome once: C, B, then A");
    assert_eq!(held.accepted.len(), 3, "no acceptance twice");

    // Damage the checkpoint's manifest: the catalog still lists it, but it
    // is not resumable.
    let crash = fresh("schedule-damaged");
    assert!(crash_at("schedule", &crash, "checkpointed"));
    let manifest = fs::read_dir(crash.join("store").join("checkpoints"))
        .expect("checkpoints")
        .next()
        .expect("one manifest")
        .expect("an entry")
        .path();
    let mut bytes = fs::read(&manifest).expect("reads");
    bytes[20] ^= 0x01;
    fs::write(&manifest, bytes).expect("writes");
    let damaged_catalog = crash.join("catalog.db");
    let mut index = CommandJournal::open(&damaged_catalog).expect("opens");
    index
        .open_run(SESSION, config.config_hash())
        .expect("opens the run");
    assert_eq!(index.checkpoints().expect("lists").len(), 1);
    drop(index);
    let store = ChunkStore::open(crash.join("store"), Durability::ProcessCrash).expect("store");
    let recovered = store
        .latest_valid(|_, bytes| restore(bytes, profile()))
        .expect("lists");
    assert!(
        recovered.latest.is_none(),
        "the indexed checkpoint is not resumable"
    );
    assert_eq!(recovered.skipped.len(), 1);
    drop(store);
    let (owner, handle) = journaled(
        &config,
        catalog_at(&damaged_catalog),
        found(&config),
        book(&config),
    );
    let (start, _) = drive(owner, handle, |handle| async move {
        let cx = cx();
        let start = handle.inspect(&cx).await.expect("inspect");
        handle.close(&cx).await.expect("closes");
        start
    });
    assert_eq!(
        (start.boundary.tick, start.digest),
        (Tick::new(13), expected_13.digest()),
        "the journal alone replays to the checkpoint's world"
    );
}
