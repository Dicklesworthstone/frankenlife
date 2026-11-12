//! Plan §12.2 evidence: the Studio's clocks report the scientific ticks
//! committed per wall second and the simulation-time to wall-time ratio
//! the session achieved, beside the frames presented, and the display's
//! backlog, and the controller evaluations the shown epoch's tally gained
//! (plan §14.1). A slow world reads slow and a paused one reads zero.
//!
//! No window opens: the meter is read with chosen instants, and against
//! the render feed of a session thread driven through its intent lane.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    reason = "test helpers: a failure in one is a test failure; exact equality is the claim"
)]

use std::time::{Duration, Instant};

use flife_core::ids::SessionId;
use flife_core::{Tick, TickSpan, TimeStep, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, World};
use flife_studio::{ClockMeter, ClockReading, Intent, SessionThread, Status};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

fn reference_step() -> TimeStep {
    TimeStep::new(1, 128).expect("1/128 s")
}

fn read(
    meter: &mut ClockMeter,
    at: Instant,
    committed: u64,
    presented: u64,
    shown: Option<u64>,
) -> Option<ClockReading> {
    meter.read(
        at,
        Tick::new(committed),
        presented,
        shown.map(Tick::new),
        None,
    )
}

#[test]
fn the_first_read_opens_a_window() {
    let mut meter = ClockMeter::new(reference_step());
    assert_eq!(read(&mut meter, Instant::now(), 7, 3, Some(7)), None);
}

#[test]
fn a_second_of_128_ticks_at_one_128th_is_real_time() {
    let mut meter = ClockMeter::new(reference_step());
    let start = Instant::now();
    assert_eq!(read(&mut meter, start, 1_000, 40, Some(1_000)), None);
    let reading = read(
        &mut meter,
        start + Duration::from_secs(1),
        1_128,
        100,
        Some(1_120),
    )
    .expect("a window of one second");
    assert_eq!(
        reading,
        ClockReading {
            wall_seconds: 1.0,
            ticks: 128,
            ticks_per_second: 128.0,
            simulation_rate: 1.0,
            frames: 60,
            frames_per_second: 60.0,
            committed: Tick::new(1_128),
            shown: Some(Tick::new(1_120)),
            behind: Some(8),
            evaluations: None,
            evaluations_per_second: None,
        }
    );
}

#[test]
fn evaluations_are_read_between_two_counts() {
    let mut meter = ClockMeter::new(reference_step());
    let start = Instant::now();
    let at = |seconds| start + Duration::from_secs(seconds);
    assert_eq!(meter.read(start, Tick::new(0), 0, None, Some(1_000)), None);
    let reading = meter
        .read(at(2), Tick::new(256), 0, None, Some(5_096))
        .expect("a window");
    assert_eq!(reading.evaluations, Some(4_096));
    assert_eq!(reading.evaluations_per_second, Some(2_048.0));
    // Without a count at either end there is no reading of them.
    let reading = meter
        .read(at(3), Tick::new(384), 0, None, None)
        .expect("a window");
    assert_eq!(
        (reading.evaluations, reading.evaluations_per_second),
        (None, None)
    );
    let reading = meter
        .read(at(4), Tick::new(512), 0, None, Some(9_000))
        .expect("a window");
    assert_eq!(
        (reading.evaluations, reading.evaluations_per_second),
        (None, None)
    );
    let reading = meter
        .read(at(5), Tick::new(512), 0, None, Some(9_000))
        .expect("a window");
    assert_eq!(reading.evaluations, Some(0));
}

#[test]
fn each_window_starts_where_the_last_ended() {
    let mut meter = ClockMeter::new(reference_step());
    let start = Instant::now();
    let _ = read(&mut meter, start, 0, 0, None);
    let first = read(&mut meter, start + Duration::from_millis(500), 32, 30, None)
        .expect("the first window");
    assert_eq!(first.ticks, 32);
    assert_eq!(first.ticks_per_second, 64.0);
    assert_eq!(first.simulation_rate, 0.5);
    assert_eq!(first.frames_per_second, 60.0);
    assert_eq!(first.behind, None);
    let second = read(
        &mut meter,
        start + Duration::from_millis(750),
        288,
        45,
        Some(256),
    )
    .expect("the second window");
    assert_eq!(second.wall_seconds, 0.25);
    assert_eq!(second.ticks, 256);
    assert_eq!(second.ticks_per_second, 1_024.0);
    assert_eq!(second.simulation_rate, 8.0);
    assert_eq!(second.frames, 15);
    assert_eq!(second.behind, Some(32));
}

#[test]
fn the_ratio_uses_the_worlds_timestep() {
    // 3/40 s ticks: 40 ticks in 2 s are 3 simulated seconds, 1.5 per second.
    let mut meter = ClockMeter::new(TimeStep::new(3, 40).expect("3/40 s"));
    let start = Instant::now();
    let _ = read(&mut meter, start, 10, 0, None);
    let reading = read(&mut meter, start + Duration::from_secs(2), 50, 0, None).expect("a window");
    assert_eq!(reading.ticks_per_second, 20.0);
    assert_eq!(reading.simulation_rate, 1.5);
    assert_eq!(reading.frames_per_second, 0.0);
}

#[test]
fn a_paused_world_reads_zero_and_an_empty_window_waits() {
    let mut meter = ClockMeter::new(reference_step());
    let start = Instant::now();
    let _ = read(&mut meter, start, 640, 10, Some(640));
    // No wall time has passed: no reading, and the window stays open.
    assert_eq!(read(&mut meter, start, 640, 10, Some(640)), None);
    let paused = read(
        &mut meter,
        start + Duration::from_secs(1),
        640,
        70,
        Some(640),
    )
    .expect("a window");
    assert_eq!(paused.ticks, 0);
    assert_eq!(paused.ticks_per_second, 0.0);
    assert_eq!(paused.simulation_rate, 0.0);
    assert_eq!(paused.frames_per_second, 60.0);
    assert_eq!(paused.behind, Some(0));
}

#[test]
fn a_read_before_the_window_opened_is_no_reading() {
    let mut meter = ClockMeter::new(reference_step());
    let start = Instant::now() + Duration::from_secs(5);
    let _ = read(&mut meter, start, 0, 0, None);
    assert_eq!(
        read(&mut meter, start - Duration::from_secs(1), 9, 9, None),
        None
    );
    // The window kept its start.
    let reading = read(&mut meter, start + Duration::from_secs(1), 128, 0, None).expect("a window");
    assert_eq!(reading.ticks, 128);
    assert_eq!(reading.wall_seconds, 1.0);
}

/// The preset at 32 × 32 world units on a 32 × 32 grid with 96 founders.
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
            .unwrap_or_else(|| panic!("no {section}.{key}"));
        lines[line] = format!("{key} = {value}");
    }
    ValidatedConfig::from_toml(&lines.join("\n")).expect("a valid configuration")
}

/// The newest epoch's tick and its tally's controller evaluations, from one
/// lease.
fn latest(session: &SessionThread) -> (Tick, Option<u64>) {
    let lease = session
        .feed()
        .viewer(1)
        .expect("a viewer")
        .latest()
        .expect("an epoch");
    (
        lease.tick(),
        lease.metrics().tally.map(|tally| tally.evaluations),
    )
}

/// Statuses until one satisfies `done`, within 60 seconds.
fn until(session: &SessionThread, done: impl Fn(&Status) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if session.statuses().any(|status| done(&status)) {
            return;
        }
        assert!(Instant::now() < deadline, "no such status");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn a_running_session_reads_the_ticks_its_feed_committed() {
    let config = small();
    let step = config.report().timestep;
    let world = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("the world founds");
    let (owner, handle) =
        flife_runtime::open(world, &config, SessionId::new(1)).expect("the session opens");
    let session = SessionThread::spawn(owner, handle).expect("the session thread starts");
    let mut meter = ClockMeter::new(step);

    session.send(Intent::Resume).expect("offered");
    until(&session, |status| matches!(status, Status::Running { .. }));
    let (opened, opened_evaluations) = latest(&session);
    assert_eq!(
        meter.read(Instant::now(), opened, 0, None, opened_evaluations),
        None
    );
    std::thread::sleep(Duration::from_millis(300));
    let (now, now_evaluations) = latest(&session);
    let reading = meter
        .read(Instant::now(), now, 0, Some(opened), now_evaluations)
        .expect("a window");
    let ticks = now.get() - opened.get();
    // The preset evaluates every live organism every tick.
    let evaluations = now_evaluations
        .zip(opened_evaluations)
        .map(|(now, then)| now - then)
        .expect("the session's epochs carry tallies");
    assert!(evaluations >= ticks, "{evaluations} in {ticks} ticks");
    assert_eq!(reading.evaluations, Some(evaluations));
    assert_eq!(
        reading.evaluations_per_second,
        Some(evaluations as f64 / reading.wall_seconds)
    );
    assert!(ticks > 0, "the running world committed nothing in 300 ms");
    assert_eq!(reading.ticks, ticks);
    assert_eq!(reading.committed, now);
    assert_eq!(reading.behind, Some(ticks));
    assert!(reading.wall_seconds >= 0.3, "{reading:?}");
    assert_eq!(
        reading.ticks_per_second,
        ticks as f64 / reading.wall_seconds
    );
    assert_eq!(
        reading.simulation_rate,
        step.seconds(TickSpan::new(ticks)) / reading.wall_seconds
    );

    session.send(Intent::Pause).expect("offered");
    until(&session, |status| matches!(status, Status::Paused { .. }));
    std::thread::sleep(Duration::from_millis(50));
    let (held, held_evaluations) = latest(&session);
    let _ = meter.read(Instant::now(), held, 0, Some(held), held_evaluations);
    std::thread::sleep(Duration::from_millis(100));
    let (still, still_evaluations) = latest(&session);
    let paused = meter
        .read(Instant::now(), still, 0, Some(held), still_evaluations)
        .expect("a window");
    assert_eq!(paused.ticks, 0, "{paused:?}");
    assert_eq!(paused.evaluations, Some(0));
    assert_eq!(paused.simulation_rate, 0.0);
    assert_eq!(paused.behind, Some(0));
    session.close().expect("closes");
}
