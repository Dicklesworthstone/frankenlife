//! FL-024 evidence (plan §14.1's durable checkpoint tick, §16.3): a Studio
//! session kept in a run directory is a run the command line takes. Its
//! founding checkpoint holds tick 0, one follows as the committed tick
//! passes each multiple of the interval while it runs, and closing
//! publishes the closing boundary and suspends the session. Every
//! checkpoint `flife inspect` restores is the world stepped directly with
//! the strokes the Studio applied at their ticks, bit for bit, the last is
//! the world the session closed on, and `flife resume` continues the run to
//! exactly the uninterrupted world, applying a stroke still pending at the
//! close. Under `write-ahead-before-apply` the strokes are journaled too.
//!
//! No window opens: the session thread is driven through its intent lane,
//! as the Studio host drives it.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use flife_cli::{
    CHECKPOINT_FIXED_BYTES, ControllerChoice, InspectOptions, ResumeOptions, StoreDurability,
};
use flife_core::ids::CommandId;
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_engine::{Arrangement, Intervention, NumericalProfile, Stepped, World};
use flife_runtime::Outcome;
use flife_studio::{BRUSH_ENERGY, Intent, Keeping, STUDIO_SESSION, SessionThread, Status, Stroke};

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

    // The checkpoints: tick 0, then each at or past the next multiple after
    // the one before (multiples a run passes between two reads share one),
    // then the closing boundary; numbered in order, all pause-and-stream.
    let kept = checkpoints(&statuses);
    assert!(kept.len() >= 3, "{kept:?}");
    assert!(kept[kept.len() - 2].0.get() >= 2 * every, "{kept:?}");
    assert_eq!(kept[0].0, Tick::new(0));
    for pair in kept.windows(2) {
        let ((earlier, first, ..), (later, second, ..)) = (&pair[0], &pair[1]);
        assert!(second > first, "{kept:?}");
        assert!(later > earlier, "{kept:?}");
    }
    for pair in kept[..kept.len() - 1].windows(2) {
        let (earlier, later) = (pair[0].0.get(), pair[1].0.get());
        assert!(later >= (earlier / every + 1) * every, "{kept:?}");
    }
    assert_eq!(kept.last().expect("kept").0, closed_at, "{kept:?}");
    assert!(kept.iter().all(|(.., mode)| *mode == "pause-and-stream"));

    // Every checkpoint restores to the directly stepped world, and the last
    // is the world the session closed on.
    let (_, digests) = direct(&config, &applied_strokes, closed_at);
    for (tick, sequence, sha256, _) in &kept {
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
