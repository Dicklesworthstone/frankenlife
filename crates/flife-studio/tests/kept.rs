//! FL-024 evidence (plan §14.1's durable checkpoint tick, §16.3): a Studio
//! session kept in a run directory is a run the command line takes. Its
//! founding checkpoint holds tick 0, one is taken at exactly each multiple
//! of the interval it reaches while it runs, as `flife run`'s are, and
//! closing publishes the closing boundary and suspends the session. Every
//! checkpoint `flife inspect` restores is the world stepped directly with
//! the strokes the Studio applied at their ticks, bit for bit, the last is
//! the world the session closed on, and `flife resume` continues the run to
//! exactly the uninterrupted world, applying a stroke still pending at the
//! close. Under `write-ahead-before-apply` the strokes are journaled too.
//! `Studio::resume` opens such a run again where it closed: the pending
//! stroke applies, a new stroke is numbered after the run's commands, the
//! checkpoints continue the run's, and the world it closes on is the
//! uninterrupted one.
//!
//! No window opens: the session thread is driven through its intent lane,
//! as the Studio host drives it, and the resumed Studio is offscreen on
//! headless Metal (without a Metal device that test returns early).

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::fs;
use std::num::{NonZeroU32, NonZeroUsize};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use flife_cli::{
    BranchOptions, CHECKPOINT_FIXED_BYTES, CheckpointAt, ControllerChoice, InspectOptions,
    ResumeOptions, StoreDurability,
};
use flife_core::ids::CommandId;
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_engine::{Arrangement, Intervention, NumericalProfile, Stepped, World};
use flife_runtime::Outcome;
use flife_studio::{
    BRUSH_ENERGY, Intent, KEY_E, KEY_PERIOD, Keeping, Resumed, STUDIO_SESSION, SessionThread,
    Status, Stroke, Studio, StudioOptions, Surface,
};
use fmn_render::metal::{InputEvent, LogicalPoint, MetalRenderer, Modifiers, PointerButton};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// The preset at 32 × 32 world units with 96 founders, a checkpoint every
/// 256 ticks (2 s at 128 ticks a second) all retained, under
/// `durability`, with `extra` overrides.
fn configured(durability: &str, extra: &[(&str, &str, &str)]) -> ValidatedConfig {
    let durability = format!("\"{durability}\"");
    let mut lines: Vec<String> = PRESET.lines().map(str::to_owned).collect();
    for &(section, key, value) in [
        ("world", "width", "32.0"),
        ("world", "height", "32.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", "96"),
        ("population", "maximum_count", "1024"),
        (
            "persistence",
            "checkpoint_interval_simulation_seconds",
            "2.0",
        ),
        ("persistence", "retain_recent_checkpoints", "1000"),
        ("execution", "command_durability", durability.as_str()),
    ]
    .iter()
    .chain(extra)
    {
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

/// A run directory path no run has used.
fn fresh(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("after the epoch")
        .as_nanos();
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("fl024-kept-{name}-{}-{nanos}", std::process::id()))
}

/// The ticks a kept run that runs from `from` to `to` checkpoints at:
/// every multiple of `every` from `from` on, then `to` unless it is one.
fn chain(from: u64, to: u64, every: u64) -> Vec<u64> {
    let mut ticks: Vec<u64> = (from..=to)
        .filter(|tick| tick.is_multiple_of(every))
        .collect();
    if !to.is_multiple_of(every) {
        ticks.push(to);
    }
    ticks
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

/// The checkpoints reported: tick, sequence, SHA-256 and mode.
fn checkpoints(statuses: &[Status]) -> Vec<(Tick, u64, String, &'static str)> {
    statuses
        .iter()
        .filter_map(|status| match status {
            Status::Checkpointed {
                tick,
                sequence,
                sha256,
                mode,
                ..
            } => Some((*tick, *sequence, sha256.clone(), *mode)),
            _ => None,
        })
        .collect()
}

fn finished(statuses: &[Status]) -> usize {
    statuses
        .iter()
        .filter(|status| matches!(status, Status::Finished { .. }))
        .count()
}

/// Each accepted stroke: its command, intervention and effective tick.
fn submitted(statuses: &[Status]) -> Vec<(CommandId, Intervention, Tick)> {
    statuses
        .iter()
        .filter_map(|status| match status {
            Status::Submitted {
                command,
                stroke,
                effective,
                ..
            } => Some((*command, stroke.intervention(), *effective)),
            _ => None,
        })
        .collect()
}

/// The tick each applied command applied at.
fn applied(statuses: &[Status]) -> Vec<(CommandId, Tick)> {
    statuses
        .iter()
        .filter_map(|status| match status {
            Status::Finished {
                command,
                outcome: Outcome::Applied { tick, .. },
            } => Some((*command, *tick)),
            _ => None,
        })
        .collect()
}

/// A world stepped directly to `until`, with `strokes` applied at the
/// boundaries of their ticks in command order, and its digest at each tick
/// from 0.
fn direct(
    config: &ValidatedConfig,
    strokes: &[(CommandId, Intervention, Tick)],
    until: Tick,
) -> (World, Vec<String>) {
    let mut world = found(config);
    let mut digests = vec![world.digest().to_hex()];
    while world.tick() < until {
        let at: Vec<Intervention> = strokes
            .iter()
            .filter(|(_, _, tick)| *tick == world.tick())
            .map(|(_, intervention, _)| *intervention)
            .collect();
        match world.step_with(&at).expect("steps") {
            Stepped::Published(_) => {}
            Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
        }
        digests.push(world.digest().to_hex());
    }
    (world, digests)
}

#[test]
fn a_kept_studio_session_is_a_run_the_command_line_resumes() {
    for durability in ["volatile-in-memory", "write-ahead-before-apply"] {
        kept_session_resumes(durability);
    }
}

fn kept_session_resumes(durability: &str) {
    let config = configured(durability, &[]);
    let every = config.report().checkpoint_interval_ticks.get();
    assert_eq!(every, 256);
    let dir = fresh(durability);
    let keeping = Keeping::new(&dir, &config).expect("kept");
    assert!(!dir.exists(), "nothing is created before the session opens");
    let (owner, handle, kept) = keeping
        .open(found(&config), &config, STUDIO_SESSION)
        .expect("the kept session opens");
    let session =
        SessionThread::spawn_kept(owner, handle, Some(kept)).expect("the session thread starts");

    // The founding checkpoint comes before any intent is taken.
    session.send(Intent::Resume).expect("offered");
    let statuses = gather(&session, Vec::new(), |statuses| {
        !checkpoints(statuses).is_empty()
    });
    assert_eq!(checkpoints(&statuses)[0].0, Tick::new(0), "{statuses:?}");

    // Running: a stroke applies, and checkpoints follow the run.
    let running = Stroke::Energy {
        cell: [5, 9],
        amount: BRUSH_ENERGY,
    };
    session
        .send(Intent::Intervene {
            stroke: running,
            seen: Tick::new(0),
        })
        .expect("offered");
    let statuses = gather(&session, statuses, |statuses| {
        finished(statuses) == 1
            && checkpoints(statuses)
                .last()
                .is_some_and(|(tick, ..)| tick.get() >= 2 * every)
    });

    // Paused, then two steps, then a stroke accepted for the next boundary
    // and left pending at the close.
    session.send(Intent::Pause).expect("offered");
    let statuses = gather(&session, statuses, |statuses| {
        statuses
            .iter()
            .any(|status| matches!(status, Status::Paused { .. }))
    });
    session.send(Intent::Step).expect("offered");
    session.send(Intent::Step).expect("offered");
    let statuses = gather(&session, statuses, |statuses| {
        statuses
            .iter()
            .filter(|status| matches!(status, Status::Stepped { .. }))
            .count()
            == 2
    });
    let pending = Stroke::Energy {
        cell: [20, 6],
        amount: BRUSH_ENERGY,
    };
    session
        .send(Intent::Intervene {
            stroke: pending,
            seen: Tick::new(0),
        })
        .expect("offered");
    let statuses = gather(&session, statuses, |statuses| {
        submitted(statuses).len() == 2
    });
    let (closed, closing) = session.close_reporting().expect("closes");
    let statuses: Vec<Status> = statuses.into_iter().chain(closing).collect();
    let closed_at = closed.boundary.tick;

    // The strokes: the first applied where the session reported, the
    // second accepted for the closing boundary and never applied.
    let strokes = submitted(&statuses);
    let applied = applied(&statuses);
    assert_eq!(applied.len(), 1, "{statuses:?}");
    assert_eq!(applied[0].0, strokes[0].0);
    assert_eq!(strokes[1].2, closed_at, "pending at the closing boundary");
    let applied_strokes = [(strokes[0].0, strokes[0].1, applied[0].1)];

    // The checkpoints: tick 0 and every multiple of the interval the
    // session reached, each exactly there, as `flife run`'s are; the
    // closing boundary, where the stroke offered while paused took its
    // pre-intervention checkpoint (unless a multiple's holds it); and, with
    // no journal to replay it, the closing boundary again, now holding that
    // stroke pending. Numbered in order, all pause-and-stream.
    let kept = checkpoints(&statuses);
    assert!(kept.len() >= 3, "{kept:?}");
    let ticks: Vec<u64> = kept.iter().map(|(tick, ..)| tick.get()).collect();
    let mut expected = chain(0, closed_at.get(), every);
    if durability == "volatile-in-memory" {
        expected.push(closed_at.get());
    }
    assert_eq!(ticks, expected, "{durability}");
    assert!(ticks.contains(&(2 * every)), "{ticks:?}");
    for pair in kept.windows(2) {
        assert!(pair[1].1 > pair[0].1, "{kept:?}");
    }
    assert!(kept.iter().all(|(.., mode)| *mode == "pause-and-stream"));

    // Every checkpoint restores to the directly stepped world, and the last
    // is the world the session closed on. `--at` takes the newest at a
    // tick: the closing one at the closing boundary.
    let (_, digests) = direct(&config, &applied_strokes, closed_at);
    let newest = kept
        .iter()
        .enumerate()
        .filter(|&(at, (tick, ..))| kept[at + 1..].iter().all(|(later, ..)| later != tick));
    for (_, (tick, sequence, sha256, _)) in newest {
        let inspection = flife_cli::inspect(
            &dir,
            InspectOptions {
                at: Some(tick.get()),
                field: false,
                weights: Vec::new(),
            },
        )
        .unwrap_or_else(|error| panic!("{durability}: tick {tick:?}: {error}"));
        let header = &inspection.inspected;
        assert_eq!(header.checkpoint.tick, tick.get());
        assert_eq!(header.checkpoint.sequence, *sequence);
        assert_eq!(&header.checkpoint.sha256, sha256);
        let at = usize::try_from(tick.get()).expect("a small tick");
        assert_eq!(header.digest, digests[at], "{durability}: tick {tick:?}");
    }
    assert_eq!(
        closed.world.digest().to_hex(),
        *digests.last().expect("digests")
    );

    // `flife resume` continues the run, applying the pending stroke at its
    // boundary: the uninterrupted world, bit for bit.
    let until = closed_at.get() + 6;
    let resumed = flife_cli::resume(
        &dir,
        &ResumeOptions {
            until,
            every: 0,
            cpu_workers: None,
            controllers: ControllerChoice::Cpu,
            checkpoint_every: None,
            durability: StoreDurability::PowerLoss,
            checkpoint_mode: None,
            retained_bytes: None,
            metrics_every: 0,
        },
        |_| {},
    )
    .unwrap_or_else(|error| panic!("{durability}: resume: {error}"));
    let all = [
        applied_strokes[0],
        (strokes[1].0, strokes[1].1, strokes[1].2),
    ];
    let (uninterrupted, _) = direct(&config, &all, Tick::new(until));
    assert_eq!(resumed.closed.boundary.tick, Tick::new(until));
    assert_eq!(
        resumed.closed.world.digest(),
        uninterrupted.digest(),
        "{durability}"
    );
    let pending_applied = resumed
        .closed
        .book
        .accepted(strokes[1].0)
        .map(|accepted| accepted.effective);
    assert_eq!(pending_applied, Some(closed_at));
}

/// Plan §2.2's journey: pause, stroke, resume, and compare the result with
/// a control branch from the exact pre-intervention checkpoint. A stroke
/// offered while a kept session is paused first publishes a checkpoint of
/// the boundary it will apply at; another stroke there shares it, as one at
/// a multiple shares the checkpointer's own, and a stroke while running
/// takes none. `flife branch --at` such a tick makes a control that
/// inherits every command applied before it and none of the strokes made
/// there: resumed, it is the world stepped directly without them, bit for
/// bit, while the session closes on the world with all of them.
#[test]
fn a_stroke_while_paused_keeps_the_exact_pre_intervention_checkpoint() {
    for durability in ["volatile-in-memory", "write-ahead-before-apply"] {
        pre_intervention(durability);
    }
}

fn pre_intervention(durability: &str) {
    // A checkpoint every 4 ticks.
    let config = configured(
        durability,
        &[(
            "persistence",
            "checkpoint_interval_simulation_seconds",
            "0.03125",
        )],
    );
    let every = config.report().checkpoint_interval_ticks.get();
    assert_eq!(every, 4);

    // Closed at once, the founding checkpoint holds the closing boundary
    // and every command (none): it is the only one.
    let dir = fresh(&format!("pre-{durability}-at-once"));
    let keeping = Keeping::new(&dir, &config).expect("kept");
    let (owner, handle, kept) = keeping
        .open(found(&config), &config, STUDIO_SESSION)
        .expect("the kept session opens");
    let session =
        SessionThread::spawn_kept(owner, handle, Some(kept)).expect("the session thread starts");
    let (closed, closing) = session.close_reporting().expect("closes");
    let ticks: Vec<u64> = checkpoints(&closing)
        .iter()
        .map(|(tick, ..)| tick.get())
        .collect();
    assert_eq!(
        (closed.boundary.tick, ticks),
        (Tick::ZERO, vec![0]),
        "{durability}"
    );

    let dir = fresh(&format!("pre-{durability}"));
    let keeping = Keeping::new(&dir, &config).expect("kept");
    let (owner, handle, kept) = keeping
        .open(found(&config), &config, STUDIO_SESSION)
        .expect("the kept session opens");
    let session =
        SessionThread::spawn_kept(owner, handle, Some(kept)).expect("the session thread starts");
    let stroke = |cell, seen| Intent::Intervene {
        stroke: Stroke::Energy {
            cell,
            amount: BRUSH_ENERGY,
        },
        seen: Tick::new(seen),
    };
    let checkpointed_at = |statuses: &[Status], tick: u64| {
        statuses.iter().position(
            |status| matches!(status, Status::Checkpointed { tick: at, .. } if at.get() == tick),
        )
    };
    let submitting = |statuses: &[Status], nth: usize| {
        statuses
            .iter()
            .enumerate()
            .filter(|(_, status)| matches!(status, Status::Submitted { .. }))
            .nth(nth)
            .map(|(at, _)| at)
    };

    // Paused at tick 1: stroke A, then its step.
    session.send(Intent::Step).expect("offered");
    let statuses = gather(&session, Vec::new(), |statuses| stepped(statuses) == 1);
    session.send(stroke([5, 9], 1)).expect("offered");
    let statuses = gather(&session, statuses, |statuses| {
        submitted(statuses).len() == 1
    });
    session.send(Intent::Step).expect("offered");
    session.send(Intent::Step).expect("offered");
    // Paused at tick 3: strokes B and C share one checkpoint.
    let statuses = gather(&session, statuses, |statuses| stepped(statuses) == 3);
    session.send(stroke([20, 6], 3)).expect("offered");
    session.send(stroke([12, 12], 3)).expect("offered");
    let statuses = gather(&session, statuses, |statuses| {
        submitted(statuses).len() == 3
    });
    // Stepped to the multiple 4: the checkpointer's checkpoint there is
    // stroke E's too.
    session.send(Intent::Step).expect("offered");
    let statuses = gather(&session, statuses, |statuses| {
        stepped(statuses) == 4 && checkpointed_at(statuses, 4).is_some()
    });
    session.send(stroke([7, 25], 4)).expect("offered");
    let statuses = gather(&session, statuses, |statuses| {
        submitted(statuses).len() == 4
    });
    // Running: stroke D takes no checkpoint of its own.
    session.send(Intent::Resume).expect("offered");
    let statuses = gather(&session, statuses, |statuses| {
        statuses
            .iter()
            .any(|status| matches!(status, Status::Running { .. }))
    });
    session.send(stroke([28, 2], 4)).expect("offered");
    let statuses = gather(&session, statuses, |statuses| {
        finished(statuses) == 5
            && checkpoints(statuses)
                .last()
                .is_some_and(|(tick, ..)| tick.get() >= 3 * every)
    });
    session.send(Intent::Pause).expect("offered");
    let statuses = gather(&session, statuses, |statuses| {
        statuses
            .iter()
            .any(|status| matches!(status, Status::Paused { .. }))
    });
    let (closed, closing) = session.close_reporting().expect("closes");
    let statuses: Vec<Status> = statuses.into_iter().chain(closing).collect();
    let closed_at = closed.boundary.tick;

    let strokes = submitted(&statuses);
    let effective: Vec<u64> = strokes.iter().map(|(.., tick)| tick.get()).collect();
    assert_eq!(effective[..4], [1, 3, 3, 4], "{durability}");
    assert!(effective[4] >= 4, "{effective:?}");
    let mut applied = applied(&statuses);
    applied.sort();
    assert_eq!(
        applied,
        strokes
            .iter()
            .map(|(command, _, tick)| (*command, *tick))
            .collect::<Vec<_>>(),
        "every stroke applied where it was accepted"
    );

    // Each paused stroke's checkpoint came before it was offered; the
    // checkpoints are tick 0, the two strokes' boundaries, then the
    // multiples and the closing boundary.
    for (tick, nth) in [(1, 0), (3, 1)] {
        let (Some(before), Some(offered)) =
            (checkpointed_at(&statuses, tick), submitting(&statuses, nth))
        else {
            panic!("{durability}: tick {tick}: {statuses:?}");
        };
        assert!(before < offered, "{durability}: tick {tick}");
    }
    let ticks: Vec<u64> = checkpoints(&statuses)
        .iter()
        .map(|(tick, ..)| tick.get())
        .collect();
    let mut expected = vec![0, 1, 3];
    expected.extend(chain(4, closed_at.get(), every));
    assert_eq!(ticks, expected, "{durability}");

    // The session closed on the world with every stroke.
    let (_, digests) = direct(&config, &strokes, closed_at);
    assert_eq!(
        closed.world.digest().to_hex(),
        *digests.last().expect("digests")
    );

    // Controls from tick 3 and tick 4: each inherits the strokes applied
    // before its tick and none made there, and resumed to tick 11 is the
    // world stepped directly with only those, while the session's world
    // there differs.
    let until = 11;
    for (at, inherited) in [(3, 1), (4, 3)] {
        let control = fresh(&format!("pre-{durability}-control-{at}"));
        flife_cli::branch(
            &dir,
            &control,
            &BranchOptions {
                at: Some(at),
                name: Some(format!("control-{at}")),
                commands: Vec::new(),
                durability: StoreDurability::PowerLoss,
            },
        )
        .unwrap_or_else(|error| panic!("{durability}: branch at {at}: {error}"));
        let resumed = flife_cli::resume(
            &control,
            &ResumeOptions {
                until,
                every: 0,
                cpu_workers: None,
                controllers: ControllerChoice::Cpu,
                checkpoint_every: None,
                durability: StoreDurability::PowerLoss,
                checkpoint_mode: None,
                retained_bytes: None,
                metrics_every: 0,
            },
            |_| {},
        )
        .unwrap_or_else(|error| panic!("{durability}: resume the control at {at}: {error}"));
        let (without, _) = direct(&config, &strokes[..inherited], Tick::new(until));
        assert_eq!(
            resumed.closed.world.digest(),
            without.digest(),
            "{durability}: the control from {at}"
        );
        let usize_until = usize::try_from(until).expect("a small tick");
        assert_ne!(
            without.digest().to_hex(),
            digests[usize_until],
            "{durability}: the strokes at {at} change the world"
        );
        for (nth, (command, ..)) in strokes.iter().enumerate() {
            assert_eq!(
                resumed.closed.book.accepted(*command).is_some(),
                nth < inherited,
                "{durability}: the control from {at}, stroke {nth}"
            );
        }
    }
}

/// An offscreen Studio: 320 x 200 points at one pixel a point, paused, CPU
/// controllers on two workers. No window opens.
fn offscreen() -> StudioOptions {
    StudioOptions {
        size: Some([320, 200]),
        cpu_workers: NonZeroUsize::new(2),
        controllers: ControllerChoice::Cpu,
        surface: Surface::Offscreen {
            scale: NonZeroU32::MIN,
        },
        ..StudioOptions::default()
    }
}

/// Press and release a key on `studio`.
fn tap(studio: &mut Studio, code: u16) {
    for pressed in [true, false] {
        studio
            .post_input(InputEvent::Key {
                code,
                text: String::new(),
                pressed,
                repeat: false,
                modifiers: Modifiers::NONE,
            })
            .expect("posts");
    }
}

/// Frames of `studio` until `done` holds for the statuses gathered, within
/// 120 seconds.
fn frames_until(
    studio: &mut Studio,
    mut statuses: Vec<Status>,
    done: impl Fn(&[Status]) -> bool,
) -> Vec<Status> {
    let deadline = Instant::now() + Duration::from_secs(120);
    while !done(&statuses) {
        assert!(Instant::now() < deadline, "only {statuses:?}");
        studio.frame().expect("a frame");
        statuses.extend(studio.take_statuses());
        std::thread::sleep(Duration::from_millis(2));
    }
    statuses
}

fn stepped(statuses: &[Status]) -> usize {
    statuses
        .iter()
        .filter(|status| matches!(status, Status::Stepped { .. }))
        .count()
}

#[test]
fn a_studio_resumes_its_kept_run_where_it_closed() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    for durability in ["volatile-in-memory", "write-ahead-before-apply"] {
        studio_resumes(durability);
    }
}

fn studio_resumes(durability: &str) {
    let config = configured(durability, &[]);
    let dir = fresh(&format!("resume-{durability}"));

    // A kept session, paused: two steps, a stroke applied at tick 2, a
    // step, and a stroke left pending at the closing boundary, tick 3.
    let keeping = Keeping::new(&dir, &config).expect("kept");
    let (owner, handle, kept) = keeping
        .open(found(&config), &config, STUDIO_SESSION)
        .expect("the kept session opens");
    let session =
        SessionThread::spawn_kept(owner, handle, Some(kept)).expect("the session thread starts");
    let stroke = |cell, seen| Intent::Intervene {
        stroke: Stroke::Energy {
            cell,
            amount: BRUSH_ENERGY,
        },
        seen: Tick::new(seen),
    };
    session.send(Intent::Step).expect("offered");
    session.send(Intent::Step).expect("offered");
    let statuses = gather(&session, Vec::new(), |statuses| stepped(statuses) == 2);
    session.send(stroke([5, 9], 2)).expect("offered");
    let statuses = gather(&session, statuses, |statuses| {
        submitted(statuses).len() == 1
    });
    session.send(Intent::Step).expect("offered");
    let statuses = gather(&session, statuses, |statuses| {
        stepped(statuses) == 3 && finished(statuses) == 1
    });
    session.send(stroke([20, 6], 3)).expect("offered");
    let statuses = gather(&session, statuses, |statuses| {
        submitted(statuses).len() == 2
    });
    let (closed, closing) = session.close_reporting().expect("closes");
    let statuses: Vec<Status> = statuses.into_iter().chain(closing).collect();
    let strokes = submitted(&statuses);
    assert_eq!(applied(&statuses), [(strokes[0].0, Tick::new(2))]);
    let closed_at = closed.boundary.tick;
    assert_eq!(closed_at, Tick::new(3));
    assert_eq!(strokes[1].2, closed_at, "pending at the closing boundary");
    let last = checkpoints(&statuses).last().expect("checkpoints").clone();
    assert_eq!(last.0, closed_at);

    // The Studio resumes it from the closing checkpoint.
    let mut studio = Studio::resume(&dir, &offscreen()).expect("resumes");
    assert_eq!(
        studio.resumed(),
        Some(&Resumed {
            checkpoint: Some(CheckpointAt {
                sequence: last.1,
                tick: closed_at.get(),
            }),
            skipped: Vec::new(),
            tick: closed_at.get(),
        })
    );
    assert_eq!(
        studio.kept().map(|(run_dir, _)| run_dir),
        Some(dir.as_path())
    );
    let shown = studio.frame().expect("a frame").shown.expect("an epoch");
    assert_eq!(shown.tick, closed_at);

    // Stepped, the pending stroke applies at its boundary, and the resumed
    // session reports it finishing as it reports its own; a new stroke is
    // numbered after the run's two and applies at the next.
    tap(&mut studio, KEY_PERIOD);
    let statuses = frames_until(&mut studio, Vec::new(), |statuses| {
        stepped(statuses) == 1 && finished(statuses) == 1
    });
    assert_eq!(
        applied(&statuses),
        [(strokes[1].0, closed_at)],
        "{statuses:?}"
    );
    tap(&mut studio, KEY_E);
    for pressed in [true, false] {
        studio
            .post_input(InputEvent::Button {
                button: PointerButton::Primary,
                pressed,
                position: LogicalPoint { x: 160.0, y: 100.0 },
                clicks: 1,
                modifiers: Modifiers::NONE,
            })
            .expect("posts");
    }
    let statuses = frames_until(&mut studio, statuses, |statuses| {
        submitted(statuses).len() == 1
    });
    let new = submitted(&statuses)[0];
    assert_eq!(
        new.0,
        CommandId::new(3),
        "numbered after the run's commands"
    );
    for _ in 0..3 {
        tap(&mut studio, KEY_PERIOD);
    }
    let statuses = frames_until(&mut studio, statuses, |statuses| {
        stepped(statuses) == 4 && finished(statuses) == 2
    });
    let after = applied(&statuses);
    assert_eq!(after.len(), 2, "{statuses:?}");
    assert_eq!(after[1], (new.0, new.2));
    let closed = studio.close().expect("closes");
    let until = closed.session.boundary.tick;
    assert_eq!(until, Tick::new(closed_at.get() + 4));

    // The world it closed on is the uninterrupted one, bit for bit; its
    // checkpoints continue the run's, the closing one there.
    let all = [
        (strokes[0].0, strokes[0].1, Tick::new(2)),
        (strokes[1].0, strokes[1].1, closed_at),
        (new.0, new.1, new.2),
    ];
    let (uninterrupted, _) = direct(&config, &all, until);
    assert_eq!(
        closed.session.world.digest(),
        uninterrupted.digest(),
        "{durability}"
    );
    let resumed_kept = checkpoints(&closed.statuses);
    assert_eq!(resumed_kept.len(), 1, "{resumed_kept:?}");
    assert_eq!(resumed_kept[0].0, until);
    assert!(
        resumed_kept[0].1 > last.1,
        "{resumed_kept:?} after {last:?}"
    );
    let inspection = flife_cli::inspect(&dir, InspectOptions::default()).expect("inspects");
    assert_eq!(inspection.inspected.checkpoint.tick, until.get());
    assert_eq!(inspection.inspected.digest, uninterrupted.digest().to_hex());
}

/// The directories under `root`, `root` first.
fn directories(root: &Path) -> Vec<PathBuf> {
    let mut found = vec![root.to_path_buf()];
    let mut at = 0;
    while at < found.len() {
        let entries = fs::read_dir(&found[at]).expect("a readable directory");
        for entry in entries {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                found.push(path);
            }
        }
        at += 1;
    }
    found
}

/// Directories made read-only, made writable again when dropped.
struct ReadOnly(Vec<PathBuf>);

impl ReadOnly {
    fn new(directories: Vec<PathBuf>) -> Self {
        set_mode(&directories, 0o555);
        Self(directories)
    }
}

impl Drop for ReadOnly {
    fn drop(&mut self) {
        set_mode(&self.0, 0o755);
    }
}

fn set_mode(directories: &[PathBuf], mode: u32) {
    for directory in directories {
        let mut permissions = fs::metadata(directory).expect("metadata").permissions();
        permissions.set_mode(mode);
        fs::set_permissions(directory, permissions).expect("permissions set");
    }
}

/// A checkpoint that fails is reported unkept, and the session is not held
/// at its multiple: it runs on past it, takes no more checkpoints, and
/// closes without a closing one.
#[test]
fn a_failed_checkpoint_lets_the_session_run_on() {
    let config = configured("volatile-in-memory", &[]);
    let every = config.report().checkpoint_interval_ticks.get();
    let dir = fresh("unkept");
    let keeping = Keeping::new(&dir, &config).expect("kept");
    let (owner, handle, kept) = keeping
        .open(found(&config), &config, STUDIO_SESSION)
        .expect("the kept session opens");
    let session =
        SessionThread::spawn_kept(owner, handle, Some(kept)).expect("the session thread starts");
    let statuses = gather(&session, Vec::new(), |statuses| {
        !checkpoints(statuses).is_empty()
    });

    // The store takes no more chunks, so the checkpoint at the first
    // multiple fails.
    let store = ReadOnly::new(directories(&dir.join(flife_cli::STORE_DIR)));
    session.send(Intent::Resume).expect("offered");
    let unkept = |statuses: &[Status]| {
        statuses
            .iter()
            .filter(|status| matches!(status, Status::Unkept { .. }))
            .count()
    };
    let mut statuses = gather(&session, statuses, |statuses| unkept(statuses) == 1);

    // Released, the session runs on past the multiple.
    let paused = |statuses: &[Status]| -> Vec<Tick> {
        statuses
            .iter()
            .filter_map(|status| match status {
                Status::Paused { at } => Some(*at),
                _ => None,
            })
            .collect()
    };
    let mut attempts = 0;
    loop {
        std::thread::sleep(Duration::from_millis(200));
        session.send(Intent::Pause).expect("offered");
        let before = paused(&statuses).len();
        statuses = gather(&session, statuses, |statuses| {
            paused(statuses).len() > before
        });
        let at = *paused(&statuses).last().expect("paused");
        if at.get() > every {
            break;
        }
        attempts += 1;
        assert!(attempts < 25, "held at {at:?}: {statuses:?}");
        session.send(Intent::Resume).expect("offered");
    }
    let (closed, closing) = session.close_reporting().expect("closes");
    drop(store);
    let statuses: Vec<Status> = statuses.into_iter().chain(closing).collect();
    assert!(closed.boundary.tick.get() > every, "{:?}", closed.boundary);
    assert_eq!(unkept(&statuses), 1, "{statuses:?}");
    let ticks: Vec<Tick> = checkpoints(&statuses)
        .iter()
        .map(|(tick, ..)| *tick)
        .collect();
    assert_eq!(ticks, [Tick::new(0)], "only the founding checkpoint");
}

/// `Studio::resume` refuses, before anything is read or any surface opens,
/// a run directory to keep it in elsewhere and a timeline to record; and a
/// directory that holds no run.
#[test]
fn what_cannot_be_resumed_is_refused() {
    let unmade = fresh("unmade");
    let refused =
        |options: &StudioOptions, dir: &Path, expected: &str| match Studio::resume(dir, options) {
            Err(CoreError::StateConflict { detail, .. }) => {
                assert!(detail.contains(expected), "{detail}");
            }
            other => panic!("{other:?}"),
        };
    let elsewhere = StudioOptions {
        run: Some(fresh("elsewhere")),
        ..offscreen()
    };
    refused(&elsewhere, &unmade, "kept where it is");
    let recorded = StudioOptions {
        record: true,
        ..offscreen()
    };
    refused(&recorded, &unmade, "replays from the founding world");
    refused(&offscreen(), &unmade, "is not a run directory");
    assert!(!unmade.exists());
}

/// A directory that holds a run, or anything else, and a budget that
/// cannot hold a checkpoint are refused before anything is created.
#[test]
fn what_cannot_be_kept_is_refused_before_anything_is_created() {
    let config = configured("volatile-in-memory", &[]);
    let holding = fresh("holding");
    std::fs::create_dir_all(&holding).expect("creates");
    std::fs::write(holding.join("notes.txt"), "mine").expect("writes");
    match Keeping::new(&holding, &config) {
        Err(CoreError::StateConflict { detail, .. }) => {
            assert!(detail.contains("is not empty"), "{detail}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(listing(&holding), ["notes.txt"]);

    let run = fresh("run");
    let keeping = Keeping::new(&run, &config).expect("kept");
    let (owner, handle, kept) = keeping
        .open(found(&config), &config, STUDIO_SESSION)
        .expect("opens");
    let session = SessionThread::spawn_kept(owner, handle, Some(kept)).expect("starts");
    session.close().expect("closes");
    let before = listing(&run);
    match Keeping::new(&run, &config) {
        Err(CoreError::StateConflict { detail, .. }) => {
            assert!(detail.contains("already holds a run"), "{detail}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(listing(&run), before);

    // `maximum_total_bytes` leaves one mebibyte past the genome bytes: less
    // than a checkpoint's fixed heap.
    let total = 1_207_959_552_u64 + (1 << 20);
    assert!(total - 1_207_959_552 < CHECKPOINT_FIXED_BYTES);
    let total = total.to_string();
    let tight = configured(
        "volatile-in-memory",
        &[("execution", "maximum_total_bytes", total.as_str())],
    );
    let unmade = fresh("tight");
    match Keeping::new(&unmade, &tight) {
        Err(CoreError::BudgetExhausted { budget, .. }) => {
            assert_eq!(budget, "checkpoint memory");
        }
        other => panic!("{other:?}"),
    }
    assert!(!unmade.exists());
}

fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("reads")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}
