//! FL-023 with FL-024: a resumed Studio records its timeline. The header
//! names where the session started: the run directory, the checkpoint it
//! restored (pinned, so retention keeps it) and the tick it opened at. The
//! replay restores that checkpoint and draws every frame the Studio drew,
//! byte for byte, at the tick it showed it, and closes on the world the
//! Studio closed on.
//!
//! - `volatile-in-memory`: resumed from the closing checkpoint. The run
//!   retains one checkpoint, so past the resumed session's own closing
//!   checkpoint only the pin keeps the one the timeline replays from.
//! - The same, a branch made at the run's closing tick, resumed from its
//!   founding checkpoint (the Studio resumes a branch as `flife resume`
//!   does).
//! - A transplant (FL-042), resumed from its founding checkpoint. Its world
//!   is not the one its configuration founds, so only a replay that
//!   restores the checkpoint reaches it. (A run's or a branch's book holds
//!   every command applied since the founding, so founding and stepping
//!   with them reaches the same world: for those the checkpoint saves the
//!   steps before it, and the refusals below show the replay reads it.)
//! - `write-ahead-before-apply`, its only checkpoint damaged: resumed from
//!   the founding world with the command journal replayed to tick 3. The
//!   timeline names no checkpoint, and nothing is pinned.
//!
//! A timeline whose start disagrees with its run directory is refused.
//! No window opens: headless Metal. Without a Metal device the test returns
//! early. Each run leaves its run directories under the target's temporary
//! directory; nothing deletes them.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    reason = "test: a failure is a test failure; figures are printed"
)]

use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use flife_cli::{
    BranchOptions, ControllerChoice, Disk, PackageSource, STORE_DIR, StoreDurability,
    TransplantOptions, transplant, write_package,
};
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_store::ChunkStore;
use flife_studio::{
    BRUSH_ENERGY, Intent, KEY_E, KEY_PERIOD, Keeping, STUDIO_SESSION, SessionThread, Status,
    Stroke, Studio, StudioOptions, Surface, TimelineCheckpoint, TimelineRecord, TimelineStart,
    read_timeline, replay, write_timeline_record,
};
use fmn_render::metal::{InputEvent, LogicalPoint, MetalRenderer, Modifiers, PointerButton};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// The preset at 32 × 32 world units with 96 founders, retaining one
/// checkpoint, under `durability`.
fn configured(durability: &str) -> ValidatedConfig {
    let durability = format!("\"{durability}\"");
    let mut lines: Vec<String> = PRESET.lines().map(str::to_owned).collect();
    for (section, key, value) in [
        ("world", "width", "32.0"),
        ("world", "height", "32.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", "96"),
        ("population", "maximum_count", "1024"),
        ("persistence", "retain_recent_checkpoints", "1"),
        ("execution", "command_durability", durability.as_str()),
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

/// A run directory path no run has used.
fn fresh(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("after the epoch")
        .as_nanos();
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "fl023-resumed-{name}-{}-{nanos}",
        std::process::id()
    ))
}

fn digest(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

fn count(statuses: &[Status], which: fn(&Status) -> bool) -> usize {
    statuses.iter().filter(|status| which(status)).count()
}

fn stepped(status: &Status) -> bool {
    matches!(status, Status::Stepped { .. })
}

fn submitted(status: &Status) -> bool {
    matches!(status, Status::Submitted { .. })
}

fn finished(status: &Status) -> bool {
    matches!(status, Status::Finished { .. })
}

/// A kept run of the Studio's session, paused: two steps, a stroke applied
/// at tick 2, a step, and a stroke left pending at tick 3, where it closes.
/// Returns the closing checkpoint's sequence.
fn kept_run(config: &ValidatedConfig, dir: &Path) -> u64 {
    let world = World::found(
        config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds");
    let keeping = Keeping::new(dir, config).expect("kept");
    let (owner, handle, kept) = keeping
        .open(world, config, STUDIO_SESSION)
        .expect("the kept session opens");
    let session =
        SessionThread::spawn_kept(owner, handle, Some(kept)).expect("the session thread starts");
    let gather = |mut statuses: Vec<Status>, done: &dyn Fn(&[Status]) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(120);
        while !done(&statuses) {
            assert!(Instant::now() < deadline, "only {statuses:?}");
            statuses.extend(session.statuses());
            std::thread::sleep(Duration::from_millis(1));
        }
        statuses
    };
    let stroke = |cell, seen| Intent::Intervene {
        stroke: Stroke::Energy {
            cell,
            amount: BRUSH_ENERGY,
        },
        seen: Tick::new(seen),
    };
    session.send(Intent::Step).expect("offered");
    session.send(Intent::Step).expect("offered");
    let statuses = gather(Vec::new(), &|statuses| count(statuses, stepped) == 2);
    session.send(stroke([5, 9], 2)).expect("offered");
    let statuses = gather(statuses, &|statuses| count(statuses, submitted) == 1);
    session.send(Intent::Step).expect("offered");
    let statuses = gather(statuses, &|statuses| {
        count(statuses, stepped) == 3 && count(statuses, finished) == 1
    });
    session.send(stroke([20, 6], 3)).expect("offered");
    let statuses = gather(statuses, &|statuses| count(statuses, submitted) == 2);
    let (closed, closing) = session.close_reporting().expect("closes");
    assert_eq!(closed.boundary.tick, Tick::new(3));
    let last = statuses
        .iter()
        .chain(&closing)
        .filter_map(|status| match status {
            Status::Checkpointed { tick, sequence, .. } => Some((*tick, *sequence)),
            _ => None,
        })
        .next_back()
        .expect("checkpoints");
    assert_eq!(last.0, Tick::new(3));
    last.1
}

/// A recording Studio and what it has recorded: its timeline, with the
/// session's statuses in, and each drawn frame's tick and pixels' hash.
struct Recording {
    studio: Studio,
    records: Vec<TimelineRecord>,
    drawn: Vec<(u64, u64)>,
    statuses: Vec<Status>,
}

impl Recording {
    fn tap(&mut self, code: u16) {
        for pressed in [true, false] {
            self.studio
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

    fn frame(&mut self) {
        let frame = self.studio.frame().expect("a frame");
        self.records.extend(self.studio.take_recorded());
        for status in self.studio.take_statuses() {
            self.records.extend(TimelineRecord::status(&status));
            self.statuses.push(status);
        }
        if frame.drawn {
            let tick = frame.shown.expect("an epoch is shown").tick.get();
            let pixels = self.studio.last_frame().expect("the frame read back");
            self.drawn.push((tick, digest(pixels.as_bytes())));
        }
    }

    /// Frames until `done` holds for the statuses so far, within 120 s.
    fn until(&mut self, done: impl Fn(&[Status]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(120);
        while !done(&self.statuses) {
            assert!(Instant::now() < deadline, "only {:?}", self.statuses);
            self.frame();
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// An offscreen recording Studio: 320 x 200 points at one pixel a point,
/// paused, CPU controllers on two workers.
fn recording() -> StudioOptions {
    StudioOptions {
        size: Some([320, 200]),
        cpu_workers: NonZeroUsize::new(2),
        controllers: ControllerChoice::Cpu,
        surface: Surface::Offscreen {
            scale: NonZeroU32::MIN,
        },
        record: true,
        ..StudioOptions::default()
    }
}

fn store(dir: &Path) -> ChunkStore {
    ChunkStore::open(dir.join(STORE_DIR), StoreDurability::ProcessCrash).expect("the store opens")
}

/// A transplant (FL-042) of a disk of `config`'s world stepped 8 ticks into
/// a host of `config`, kept in `dir`: a run whose world is not the one its
/// configuration founds, with no commands.
fn transplanted(config: &ValidatedConfig, dir: &Path) {
    let mut world = World::found(
        config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds");
    for _ in 0..8 {
        assert!(matches!(
            world.step().expect("steps"),
            Stepped::Published(_)
        ));
    }
    let package = fresh("package");
    write_package(
        &package,
        &world,
        Disk {
            center: [16.0, 16.0],
            radius: 8.0,
        },
        PackageSource {
            run_dir: "resumed-timeline".into(),
            session: 1,
            sequence: 1,
            tick: world.tick().get(),
            checkpoint_sha256: String::new(),
            config_sha256: config.config_hash().to_hex(),
        },
        StoreDurability::ProcessCrash,
    )
    .expect("writes the package");
    transplant(
        &package,
        config,
        dir,
        &TransplantOptions {
            at: [10.0, 22.0],
            name: None,
            durability: StoreDurability::ProcessCrash,
        },
    )
    .expect("transplants");
}

/// Where the resumed Studio's session starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Start {
    /// The run's closing checkpoint.
    Closing,
    /// The founding checkpoint of a branch made at the run's closing tick.
    Branch,
    /// A transplant's founding checkpoint.
    Transplant,
    /// The founding world, the run's only checkpoint damaged, with the
    /// command journal replayed past it.
    Founding,
}

#[test]
fn a_resumed_studio_records_a_timeline_that_replays_from_where_it_started() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    // The transplant first: its world is the one founding cannot reach.
    resumed_replays("volatile-in-memory", Start::Transplant);
    resumed_replays("volatile-in-memory", Start::Branch);
    resumed_replays("volatile-in-memory", Start::Closing);
    resumed_replays("write-ahead-before-apply", Start::Founding);
}

fn resumed_replays(durability: &str, from: Start) {
    let config = configured(durability);
    let dir = fresh(&format!("{durability}-{from:?}"));
    let closing = match from {
        Start::Transplant => {
            transplanted(&config, &dir);
            None
        }
        Start::Branch => {
            let parent = fresh(&format!("{durability}-{from:?}-parent"));
            kept_run(&config, &parent);
            flife_cli::branch(
                &parent,
                &dir,
                &BranchOptions {
                    at: Some(3),
                    name: None,
                    commands: Vec::new(),
                    durability: StoreDurability::ProcessCrash,
                },
            )
            .expect("branches at the run's close");
            None
        }
        Start::Closing | Start::Founding => Some(kept_run(&config, &dir)),
    };
    // The one checkpoint the run directory holds: the closing one, the
    // branch's founding one or the transplant's.
    let stored = store(&dir).sequences().expect("lists");
    let [restored] = stored[..] else {
        panic!("{stored:?}");
    };
    if let Some(closing) = closing {
        assert_eq!(restored, closing);
    }
    let damaged = from == Start::Founding;
    if damaged {
        let store = store(&dir);
        fs::write(store.manifest_path(restored), b"damaged").expect("damages the manifest");
    }

    // Resumed and recording, through a path the header names canonically:
    // a pending stroke (all but the transplant's run hold one) applies at
    // the first step, a new stroke is made, and three more steps apply it.
    let studio =
        Studio::resume(dir.join(STORE_DIR).join(".."), &recording()).expect("resumes, recording");
    let resumed = studio.resumed().expect("resumed").clone();
    let opened = resumed.tick;
    assert_eq!(resumed.checkpoint.is_some(), !damaged);
    let (restored_tick, pending) = match resumed.checkpoint {
        Some(checkpoint) if from == Start::Transplant => (checkpoint.tick, 0),
        _ => (3, 1),
    };
    assert_eq!(opened, restored_tick, "{from:?}");
    let mut recording = Recording {
        studio,
        records: Vec::new(),
        drawn: Vec::new(),
        statuses: Vec::new(),
    };
    recording.frame();
    recording.tap(KEY_PERIOD);
    recording
        .until(|statuses| count(statuses, stepped) == 1 && count(statuses, finished) == pending);
    recording.tap(KEY_E);
    for pressed in [true, false] {
        recording
            .studio
            .post_input(InputEvent::Button {
                button: PointerButton::Primary,
                pressed,
                position: LogicalPoint { x: 160.0, y: 100.0 },
                clicks: 1,
                modifiers: Modifiers::NONE,
            })
            .expect("posts");
    }
    recording.until(|statuses| count(statuses, submitted) == 1);
    for _ in 0..3 {
        recording.tap(KEY_PERIOD);
    }
    recording.until(|statuses| {
        count(statuses, stepped) == 4 && count(statuses, finished) == pending + 1
    });
    for _ in 0..3 {
        recording.frame();
    }
    let Recording {
        mut studio,
        mut records,
        drawn,
        ..
    } = recording;
    records.extend(studio.take_recorded());
    let closed = studio.close().expect("closes");
    records.push(TimelineRecord::end(&closed.session));
    let until = closed.session.boundary.tick.get();
    assert_eq!(until, opened + 4);

    // The header names where the session started; the checkpoint is
    // pinned for it, and retention kept it past the resumed session's
    // closing checkpoint.
    let Some(TimelineRecord::Header {
        from: Some(start), ..
    }) = records.first()
    else {
        panic!("{:?}", records.first());
    };
    let run_dir = fs::canonicalize(&dir).expect("the run directory");
    let store_now = store(&dir);
    let pins = store_now.pins().expect("pins");
    let sequences = store_now.sequences().expect("lists");
    drop(store_now);
    if damaged {
        assert_eq!(
            *start,
            TimelineStart {
                run_dir: run_dir.display().to_string(),
                checkpoint: None,
                tick: opened,
            }
        );
        assert!(pins.is_empty(), "{pins:?}");
    } else {
        let Some(TimelineCheckpoint { pin, .. }) = &start.checkpoint else {
            panic!("{start:?}");
        };
        assert_eq!(
            *start,
            TimelineStart {
                run_dir: run_dir.display().to_string(),
                checkpoint: Some(TimelineCheckpoint {
                    sequence: restored,
                    tick: restored_tick,
                    pin: pin.clone(),
                }),
                tick: opened,
            }
        );
        assert_eq!(
            pins.iter()
                .map(|(sequence, holder)| (*sequence, holder.to_hex()))
                .collect::<Vec<_>>(),
            [(restored, pin.clone())]
        );
        assert_eq!(sequences.len(), 2, "{sequences:?}");
        assert_eq!(sequences[0], restored, "{sequences:?}");
    }

    // Written as NDJSON and read back, it replays the Studio's frames byte
    // for byte from where the session started, to the world it closed on.
    let mut text = Vec::new();
    for record in &records {
        write_timeline_record(&mut text, record).expect("written");
    }
    let read = read_timeline(std::str::from_utf8(&text).expect("UTF-8")).expect("reads");
    assert_eq!(read, records);
    let mut index = 0;
    let replayed = replay(&config, &read, NonZeroUsize::new(2), |frame| {
        let at = drawn.get(index).copied();
        assert_eq!(
            Some((frame.tick.get(), digest(frame.pixels.as_bytes()))),
            at,
            "{from:?}: frame {index}"
        );
        index += 1;
        Ok(())
    })
    .expect("replays");
    assert_eq!(index, drawn.len());
    assert!(drawn.len() >= 5, "{drawn:?}");
    assert_eq!(replayed.frames, drawn.len() as u64);
    assert_eq!(replayed.tick, Tick::new(until));
    assert_eq!(replayed.digest, closed.session.world.digest().to_hex());
    assert_eq!(
        drawn[0].0, opened,
        "the first frame shows the tick opened at"
    );

    // A start its run directory disagrees with is refused.
    let refused = |change: &dyn Fn(&mut TimelineStart), expected: &str| {
        let mut changed = read.clone();
        if let Some(TimelineRecord::Header {
            from: Some(start), ..
        }) = changed.first_mut()
        {
            change(start);
        }
        match replay(&config, &changed, NonZeroUsize::new(2), |_| Ok(())) {
            Err(CoreError::StateConflict { detail, .. }) => {
                assert!(detail.contains(expected), "{detail}");
            }
            other => panic!("{other:?}"),
        }
    };
    let later = opened + 2;
    refused(
        &|start| start.tick = later,
        &format!("but the view shows tick {later}"),
    );
    if !damaged {
        let held = store(&dir);
        refused(&|_| {}, "another writer holds");
        drop(held);
        refused(
            &|start| start.run_dir = fresh("moved").display().to_string(),
            "is not a run directory",
        );
        let other = restored_tick + 1;
        refused(
            &|start| {
                if let Some(checkpoint) = &mut start.checkpoint {
                    checkpoint.tick = other;
                }
            },
            &format!("where the Studio restored tick {other}"),
        );
    }

    // A header with no start writes no `from`, and reads back as one.
    let mut founded = read[0].clone();
    if let TimelineRecord::Header { from, .. } = &mut founded {
        *from = None;
    }
    let line = serde_json::to_string(&founded).expect("a record");
    assert!(!line.contains("\"from\""), "{line}");
    assert_eq!(
        read_timeline(&format!("{line}\n")).expect("reads"),
        [founded]
    );
}
