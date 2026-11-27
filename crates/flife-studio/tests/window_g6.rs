//! FL-024 real-window G6 qualification (plan §13.7, §21.3, §23.1).
//!
//! Opens real native macOS AppKit/Metal windows on `presets/reference-small-world.toml`,
//! runs the live simulation with the full HUD telemetry/legend overlay and fixed 30 fps
//! pacing, injects real AppKit `SyntheticInput::Scroll` camera zooms on every third frame
//! interval, and measures:
//! - zero-readback Metal presentation rate (`presented / wall_seconds`);
//! - frame work percentiles (`FrameTimes`);
//! - both true arrival-to-drawn latency and `MoveLatencyBound` (p50/p95/p99/max);
//! - live simulation throughput (`ticks/s`, controller evaluations);
//! - bit-exact digest parity with the directly stepped reference world (`INV-09`).
//!
//! Also exercises the real-window interactive paths (`T` tracking, `G` measurement disk,
//! `W` weight dump, `C`/`[`/`]` field mappings) that previously ran only offscreen.
//!
//! Every case fails below 98% of the configuration's paced frame rate (G6's 30 fps
//! display) or above a `MoveLatencyBound` p95 of 100 ms.
//!
//! FL-034's profile cases (plan §19.5, §21.3) run when these name kept run directories:
//! `FLIFE_G6_INTERACTIVE_RUN` (the evolved 25k Interactive world) and
//! `FLIFE_G6_DESKTOP_RUN` (the evolved 100k Desktop ecology world). Each is resumed as
//! `flife-studio --resume` resumes it, at its `[presentation]` size, with Metal
//! controllers on 16 workers, for 20 s, and kept on there. Its digest is not compared
//! (that is G4's), and `experiments/fl034/README.md` says how to make the worlds.
//!
//! Run in release for G6 measurements:
//! ```text
//! cargo test --release --locked -p flife-studio --features synthetic-input --test window_g6
//! ```

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::cast_precision_loss,
    reason = "real-window G6 harness prints measurements and exits with ExitCode"
)]

use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use flife_core::{Tick, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_studio::{
    ClockMeter, ControllerChoice, FieldMapping, FrameTimes, KEY_C, KEY_G, KEY_LEFT_BRACKET,
    KEY_RIGHT_BRACKET, KEY_SPACE, KEY_T, KEY_W, KEY_ZERO, MoveLatencyBound, OverlayState, Status,
    Studio, StudioOptions, Style, Surface, Telemetry, field_key, legends, overlay_text,
};
use fmn_render::metal::{LogicalPoint, Modifiers, PointerButton, SyntheticInput};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// FL-034's profile cases: each one's label and the variable naming its run directory.
const PROFILE_RUNS: [(&str, &str); 2] = [
    ("interactive-25k-metal16", "FLIFE_G6_INTERACTIVE_RUN"),
    ("desktop-100k-metal16", "FLIFE_G6_DESKTOP_RUN"),
];

/// The least share of the paced frame rate a case must present (G6's 30 fps display): a
/// frame that overruns its slot is not made up, so this lets one slot's time in fifty be lost.
const PACED_SHARE: f64 = 0.98;

struct Lcg(u64);

impl Lcg {
    const fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 11) as f64) / ((1_u64 << 53) as f64)
    }
}

fn next_slot(last: Instant, period: Duration, now: Instant) -> Instant {
    let scheduled = last + period;
    if scheduled > now { scheduled } else { now }
}

struct WindowG6Result {
    label: &'static str,
    logical: [u32; 2],
    drawable: [u32; 2],
    backing_scale: f64,
    controllers_used: String,
    wall_seconds: f64,
    presented: u64,
    fps: f64,
    /// The configuration's `[presentation] frames_per_second`, which the loop paces to.
    paced_fps: f64,
    ticks: u64,
    ticks_per_second: f64,
    batched_evals: u64,
    readback_bytes: u64,
    input_dropped: u64,
    refused: u64,
    moves_posted: u64,
    moves_unmoved: u64,
    frame_p50_ms: f64,
    frame_p95_ms: f64,
    frame_p99_ms: f64,
    frame_max_ms: f64,
    lat_p50_ms: f64,
    lat_p95_ms: f64,
    lat_max_ms: f64,
    bound_p50_ms: f64,
    bound_p95_ms: f64,
    bound_max_ms: f64,
    digest_hex: String,
}

/// One case on `config`'s world, founded, or with `resume` the run kept there, which must
/// be `config`'s.
#[allow(
    clippy::too_many_arguments,
    reason = "one case's settings, as main lists them"
)]
fn run_window_g6(
    config: &ValidatedConfig,
    resume: Option<&Path>,
    label: &'static str,
    logical: [u32; 2],
    workers: usize,
    controllers: ControllerChoice,
    duration: Duration,
    verify_digest: bool,
) -> Result<WindowG6Result, String> {
    if resume.is_some() && verify_digest {
        return Err(format!(
            "{label}: a resumed world has no founding to step directly"
        ));
    }
    let model = config.config();
    let paced_fps = f64::from(model.presentation.frames_per_second);
    let period = Duration::from_secs_f64(1.0 / paced_fps);
    let options = StudioOptions {
        title: format!("FrankenLife G6 ({label})"),
        size: Some(logical),
        resizable: true,
        cpu_workers: NonZeroUsize::new(workers),
        run_on_open: true,
        controllers,
        style: Style::default(),
        surface: Surface::Window,
        record: false,
        run: None,
    };
    let mut studio = match resume {
        Some(run) => Studio::resume(run, &options),
        None => Studio::open(config, &options),
    }
    .map_err(|e| format!("{label} open: {e}"))?;
    let keys = legends(config).map_err(|e| format!("{label} legends: {e}"))?;
    let mut clocks = ClockMeter::new(config.report().timestep);

    // Wait for initial Retina geometry and focus before timing starts.
    let warmup_deadline = Instant::now() + Duration::from_secs(10);
    let mut focus_sent = false;
    loop {
        let frame = studio.frame().map_err(|e| format!("{label} warmup: {e}"))?;
        let _ = studio.take_statuses();
        if !focus_sent && studio.controls().metrics().is_some() && !studio.controls().focused() {
            studio
                .post(&SyntheticInput::Focus(true))
                .map_err(|e| format!("{label} focus: {e}"))?;
            focus_sent = true;
        }
        if frame.outcome.is_some()
            && studio.controls().metrics().is_some()
            && studio.controls().focused()
            && studio
                .surface_metrics()
                .ok()
                .and_then(|m| {
                    m.backing_size()
                        .map(|b| [m.drawable_width, m.drawable_height] == b)
                })
                .unwrap_or(false)
        {
            break;
        }
        if Instant::now() > warmup_deadline {
            return Err(format!("{label}: timed out waiting for Retina focus"));
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    let opened_metrics = studio
        .surface_metrics()
        .map_err(|e| format!("{label} metrics: {e}"))?;
    let drawable = [
        opened_metrics.drawable_width,
        opened_metrics.drawable_height,
    ];
    let backing_scale = opened_metrics.backing_scale;
    let controllers_used = studio.controllers().to_owned();

    let start_presented = studio.tally().presented;
    let start_tick = studio.committed().map_or(0, |e| e.tick.get());
    let start_wall = Instant::now();
    let deadline = start_wall + duration;
    let mut slot = start_wall;
    let mut last_summary = start_wall;
    let mut last_clock = None;
    let mut times = FrameTimes::new().map_err(|e| format!("{label} times: {e}"))?;
    let mut latency = FrameTimes::new().map_err(|e| format!("{label} latency: {e}"))?;
    let mut bound_times = FrameTimes::new().map_err(|e| format!("{label} bound_times: {e}"))?;
    let mut bounds = MoveLatencyBound::new();
    let mut draws = Lcg::new(0xF1_1F_E0_24);
    let mut arrived: Option<Instant> = None;
    let mut moves_posted = 0_u64;
    let mut moves_unmoved = 0_u64;
    let mut frame_index = 0_u64;

    while Instant::now() < deadline {
        let started = Instant::now();
        let frame = studio.frame().map_err(|e| format!("{label} frame: {e}"))?;
        let _ = studio.take_statuses();
        let finished = Instant::now();
        frame_index += 1;

        if frame.moved
            && let Some(at) = arrived.take()
        {
            latency.record(finished.saturating_duration_since(at));
        }
        if let Some(bound) = bounds.frame(started, finished, frame.moved, frame.drawn) {
            bound_times.record(bound);
        }

        if finished.duration_since(last_summary) >= Duration::from_secs(1) {
            let tally = studio.tally();
            let evals = studio
                .display_pair()
                .1
                .metrics()
                .tally
                .map(|t| t.evaluations);
            if let Some(reading) = clocks.read(
                finished,
                studio.committed().map_or(Tick::ZERO, |e| e.tick),
                tally.drawn(),
                frame.shown.map(|s| s.tick),
                evals,
            ) {
                last_clock = Some(reading);
            }
            last_summary = finished;
        }

        let style = studio.view().style();
        let fk = field_key(style);
        let legend = keys
            .iter()
            .find_map(|(k, l)| (*k == style.mapping).then_some(l))
            .ok_or_else(|| format!("{label}: missing legend for {}", style.mapping))?;
        let metrics = *studio.display_pair().1.metrics();
        let text = overlay_text(&OverlayState {
            paused: Some(false),
            shown: frame.shown.map(|s| s.tick),
            clock: last_clock.as_ref(),
            legend,
            field_key: fk.as_ref(),
            brush: None,
            preview: None,
            measuring: None,
            selected: frame.selected,
            inspected: None,
            telemetry: Some(Telemetry {
                metrics: &metrics,
                model: &model.model,
                seed: model.seed,
                evaluation_interval_ticks: model.controller.evaluation_interval_ticks,
                memory: studio.memory(),
                capacity_pauses: 0,
                checkpoint: None,
            }),
        });
        let swatches: Vec<[u8; 3]> = legend
            .swatches
            .iter()
            .chain(fk.iter().flat_map(|k| &k.swatches))
            .copied()
            .collect();
        studio
            .set_overlay(&text, &swatches)
            .map_err(|e| format!("{label} overlay: {e}"))?;
        times.record(started.elapsed());

        slot = next_slot(slot, period, Instant::now());
        if frame_index.is_multiple_of(3) {
            if arrived.take().is_some() {
                moves_unmoved += 1;
            }
            let at = started + (slot.saturating_duration_since(started)).mul_f64(draws.unit());
            if let Some(wait) = at.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            let centre = LogicalPoint {
                x: f64::from(logical[0]) / 2.0,
                y: f64::from(logical[1]) / 2.0,
            };
            let delta = if moves_posted.is_multiple_of(2) {
                4
            } else {
                -4
            };
            let post_time = Instant::now();
            studio
                .post(&SyntheticInput::Scroll {
                    delta: [0, delta],
                    position: centre,
                })
                .map_err(|e| format!("{label} scroll: {e}"))?;
            arrived = Some(post_time);
            moves_posted += 1;
        }
        if let Some(rest) = slot.checked_duration_since(Instant::now()) {
            std::thread::sleep(rest);
        }
    }

    let wall_seconds = start_wall.elapsed().as_secs_f64();
    let end_presented = studio.tally().presented;
    let presented = end_presented.saturating_sub(start_presented);
    let end_tick = studio.committed().map_or(start_tick, |e| e.tick.get());
    let ticks = end_tick.saturating_sub(start_tick);
    let input_dropped = studio.input_dropped();
    let refused = studio.tally().refused;
    let readback_bytes = studio.tally().readback_bytes;

    let closed = studio.close().map_err(|e| format!("{label} close: {e}"))?;
    let final_tick = closed.session.boundary.tick.get();
    let digest = closed.session.world.digest();
    let batched_evals = closed.session.world.controller_report().batched;

    if verify_digest {
        let mut direct = World::found(
            config,
            Arrangement::FreeList,
            NumericalProfile::reference_f32().map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        direct
            .set_cpu_workers(NonZeroUsize::new(workers).ok_or("workers")?)
            .map_err(|e| e.to_string())?;
        for _ in 0..final_tick {
            match direct.step().map_err(|e| e.to_string())? {
                Stepped::Published(_) => {}
                Stepped::Paused(shortfall) => {
                    return Err(format!("{label}: direct world paused: {shortfall:?}"));
                }
            }
        }
        if direct.digest() != digest {
            return Err(format!(
                "{label}: live digest {} != direct digest {} at tick {final_tick}",
                digest.to_hex(),
                direct.digest().to_hex()
            ));
        }
    }

    let fs = times
        .summary()
        .ok_or_else(|| format!("{label}: no frames"))?;
    let ls = latency
        .summary()
        .ok_or_else(|| format!("{label}: no camera latency samples"))?;
    let bs = bound_times
        .summary()
        .ok_or_else(|| format!("{label}: no MoveLatencyBound samples"))?;

    Ok(WindowG6Result {
        label,
        logical,
        drawable,
        backing_scale,
        controllers_used,
        wall_seconds,
        presented,
        fps: presented as f64 / wall_seconds,
        paced_fps,
        ticks,
        ticks_per_second: ticks as f64 / wall_seconds,
        batched_evals,
        readback_bytes,
        input_dropped,
        refused,
        moves_posted,
        moves_unmoved,
        frame_p50_ms: fs.p50_ms,
        frame_p95_ms: fs.p95_ms,
        frame_p99_ms: fs.p99_ms,
        frame_max_ms: fs.max_ms,
        lat_p50_ms: ls.p50_ms,
        lat_p95_ms: ls.p95_ms,
        lat_max_ms: ls.max_ms,
        bound_p50_ms: bs.p50_ms,
        bound_p95_ms: bs.p95_ms,
        bound_max_ms: bs.max_ms,
        digest_hex: digest.to_hex(),
    })
}

fn wait_for(
    studio: &mut Studio,
    statuses: &mut Vec<Status>,
    what: &str,
    mut done: impl FnMut(&Studio, &flife_studio::Frame, &[Status]) -> bool,
) -> Result<flife_studio::Frame, String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let frame = studio.frame().map_err(|e| format!("{what}: {e}"))?;
        statuses.extend(studio.take_statuses());
        if done(studio, &frame, statuses) {
            return Ok(frame);
        }
        if Instant::now() > deadline {
            return Err(format!("timed out waiting for {what}"));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Exercise real-window `T` tracking, `G` measurement disk, `W` weight dump,
/// and `C` / `[` / `]` field-mapping cycling through real AppKit NSEvents.
fn verify_window_interactive_tools(config: &ValidatedConfig) -> Result<(), String> {
    let options = StudioOptions {
        title: "FrankenLife Studio (tools window check)".into(),
        size: Some([640, 400]),
        resizable: true,
        cpu_workers: NonZeroUsize::new(4),
        run_on_open: false,
        controllers: ControllerChoice::Auto,
        style: Style::default(),
        surface: Surface::Window,
        record: false,
        run: None,
    };
    let mut studio = Studio::open(config, &options).map_err(|e| format!("tools open: {e}"))?;
    let mut statuses = Vec::new();

    let key = |studio: &mut Studio, code: u16, text: &str| -> Result<(), String> {
        for pressed in [true, false] {
            studio
                .post(&SyntheticInput::Key {
                    code,
                    text: text.into(),
                    pressed,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                })
                .map_err(|e| format!("key {text}: {e}"))?;
        }
        Ok(())
    };

    wait_for(&mut studio, &mut statuses, "metrics", |s, _, _| {
        s.controls().metrics().is_some()
    })?;
    if !studio.controls().focused() {
        studio
            .post(&SyntheticInput::Focus(true))
            .map_err(|e| e.to_string())?;
    }
    wait_for(&mut studio, &mut statuses, "focus", |s, f, _| {
        s.controls().focused() && f.outcome.is_some()
    })?;
    let density = studio
        .controls()
        .metrics()
        .ok_or("no metrics")?
        .backing_scale;
    let fitted = *studio.view().viewport();

    // 1. Field mapping keys: C (energy -> composition), `]` (component 0), `[` (component 31), C (energy).
    key(&mut studio, KEY_C, "c")?;
    wait_for(&mut studio, &mut statuses, "composition", |s, f, _| {
        f.outcome.is_some() && s.view().style().field_mapping == FieldMapping::Composition
    })?;
    key(&mut studio, KEY_RIGHT_BRACKET, "]")?;
    wait_for(&mut studio, &mut statuses, "component 0", |s, f, _| {
        f.outcome.is_some() && matches!(s.view().style().field_mapping, FieldMapping::Component(_))
    })?;
    let c0 = studio.view().style().field_mapping;
    key(&mut studio, KEY_LEFT_BRACKET, "[")?;
    wait_for(&mut studio, &mut statuses, "component wrap", |s, f, _| {
        f.outcome.is_some() && s.view().style().field_mapping != c0
    })?;
    key(&mut studio, KEY_C, "c")?;
    wait_for(&mut studio, &mut statuses, "back to energy", |s, f, _| {
        f.outcome.is_some() && s.view().style().field_mapping == FieldMapping::Energy
    })?;

    // 2. Click an organism, verify inspection + W weight dump + T camera tracking while running.
    let centre = [320.0 * density, 200.0 * density];
    let (_id, spot) = studio
        .view()
        .shown_rows()
        .map(|(id, p)| (id, fitted.world_to_drawable(p)))
        .min_by(|a, b| {
            let da = (a.1[0] - centre[0]).hypot(a.1[1] - centre[1]);
            let db = (b.1[0] - centre[0]).hypot(b.1[1] - centre[1]);
            da.total_cmp(&db)
        })
        .ok_or("no organism")?;
    let pt = LogicalPoint {
        x: spot[0] / density,
        y: spot[1] / density,
    };
    for pressed in [true, false] {
        studio
            .post(&SyntheticInput::Button {
                button: PointerButton::Primary,
                pressed,
                position: pt,
                clicks: 1,
                modifiers: Modifiers::NONE,
            })
            .map_err(|e| e.to_string())?;
    }
    wait_for(
        &mut studio,
        &mut statuses,
        "selection + inspect",
        |_, f, st| {
            f.selected.is_some()
                && st.iter().any(|s| {
                    matches!(
                        s,
                        Status::Inspected {
                            snapshot: Some(_),
                            ..
                        }
                    )
                })
        },
    )?;

    // W dumps the selected organism's weights in the real window.
    key(&mut studio, KEY_W, "w")?;
    wait_for(&mut studio, &mut statuses, "weights dump", |_, _, st| {
        st.iter().any(|s| {
            matches!(
                s,
                Status::Weights {
                    weights: Some(_),
                    ..
                }
            )
        })
    })?;

    // Zoom in and press T to track while running for a few ticks.
    studio
        .post(&SyntheticInput::Scroll {
            delta: [0, 80],
            position: pt,
        })
        .map_err(|e| e.to_string())?;
    wait_for(&mut studio, &mut statuses, "zoom", |s, f, _| {
        f.outcome.is_some() && s.view().viewport().zoom() > 1.5
    })?;
    key(&mut studio, KEY_T, "t")?;
    wait_for(&mut studio, &mut statuses, "tracking on", |_, f, _| {
        f.tracking && f.outcome.is_some()
    })?;
    key(&mut studio, KEY_SPACE, " ")?;
    wait_for(
        &mut studio,
        &mut statuses,
        "tracked live tick",
        |_, f, _| f.tracking && f.tracked && f.shown.is_some_and(|s| s.tick.get() >= 3),
    )?;
    key(&mut studio, KEY_SPACE, " ")?;
    wait_for(&mut studio, &mut statuses, "paused", |s, _, _| {
        !s.controls().running()
    })?;
    key(&mut studio, KEY_ZERO, "0")?;
    wait_for(&mut studio, &mut statuses, "reset", |s, f, _| {
        f.outcome.is_some() && !f.tracking && s.view().viewport().zoom() == 1.0
    })?;

    // 3. G arms the measuring tool and clicks to measure a real disk.
    key(&mut studio, KEY_G, "g")?;
    let click_pt = LogicalPoint { x: 320.0, y: 200.0 };
    for pressed in [true, false] {
        studio
            .post(&SyntheticInput::Button {
                button: PointerButton::Primary,
                pressed,
                position: click_pt,
                clicks: 1,
                modifiers: Modifiers::NONE,
            })
            .map_err(|e| e.to_string())?;
    }
    wait_for(&mut studio, &mut statuses, "measured disk", |_, _, st| {
        st.iter().any(|s| matches!(s, Status::Measured { .. }))
    })?;
    key(&mut studio, KEY_G, "g")?;

    let tally = studio.tally();
    if tally.refused != 0 || studio.input_dropped() != 0 {
        return Err(format!(
            "tools window had refused={} dropped={}",
            tally.refused,
            studio.input_dropped()
        ));
    }
    studio.close().map_err(|e| format!("tools close: {e}"))?;
    println!(
        "ok: real-window interactive tools (C/[/], click+inspect, W weights, T live tracking, G measurement disk) verified"
    );
    Ok(())
}

fn main() -> ExitCode {
    if !cfg!(target_os = "macos") {
        println!("window_g6: skipped off macOS");
        return ExitCode::SUCCESS;
    }
    let config = match ValidatedConfig::from_toml(PRESET) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("window_g6: config error: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = verify_window_interactive_tools(&config) {
        eprintln!("window_g6: FAILED tools: {e}");
        return ExitCode::FAILURE;
    }

    struct G6Case<'a> {
        label: &'static str,
        config: &'a ValidatedConfig,
        resume: Option<&'a Path>,
        logical: [u32; 2],
        workers: usize,
        controllers: ControllerChoice,
        secs: u64,
        verify_digest: bool,
    }

    // FL-034's evolved worlds, where their variables name them.
    let mut profiles: Vec<(&'static str, PathBuf, ValidatedConfig)> = Vec::new();
    for (label, variable) in PROFILE_RUNS {
        let Some(run) = std::env::var_os(variable).map(PathBuf::from) else {
            println!("window_g6: [{label}] skipped: {variable} is not set");
            continue;
        };
        let loaded = std::fs::read_to_string(run.join("config.toml"))
            .map_err(|e| e.to_string())
            .and_then(|text| ValidatedConfig::from_toml(&text).map_err(|e| e.to_string()));
        match loaded {
            Ok(profile) => profiles.push((label, run, profile)),
            Err(e) => {
                eprintln!(
                    "window_g6: [{label}] {variable}={}: no configuration: {e}",
                    run.display()
                );
                return ExitCode::FAILURE;
            }
        }
    }

    let small = [
        G6Case {
            label: "1080p-retina-cpu16",
            config: &config,
            resume: None,
            logical: [960, 540],
            workers: 16,
            controllers: ControllerChoice::Cpu,
            secs: 10,
            verify_digest: true,
        },
        G6Case {
            label: "1080p-retina-metal8",
            config: &config,
            resume: None,
            logical: [960, 540],
            workers: 8,
            controllers: ControllerChoice::Metal,
            secs: 10,
            verify_digest: false,
        },
        G6Case {
            label: "4k-retina-cpu16",
            config: &config,
            resume: None,
            logical: [1920, 1080],
            workers: 16,
            controllers: ControllerChoice::Cpu,
            secs: 10,
            verify_digest: false,
        },
    ];
    let profile_cases = profiles.iter().map(|&(label, ref run, ref profile)| {
        let presentation = &profile.config().presentation;
        G6Case {
            label,
            config: profile,
            resume: Some(run.as_path()),
            logical: [presentation.width, presentation.height],
            workers: 16,
            controllers: ControllerChoice::Metal,
            secs: 20,
            verify_digest: false,
        }
    });

    for case in small.into_iter().chain(profile_cases) {
        let label = case.label;
        match run_window_g6(
            case.config,
            case.resume,
            case.label,
            case.logical,
            case.workers,
            case.controllers,
            Duration::from_secs(case.secs),
            case.verify_digest,
        ) {
            Ok(r) => {
                println!(
                    "G6 [{}] logical={}x{} drawable={}x{} ({:.0}x) controllers=\"{}\" wall={:.2}s presented={} fps={:.2} ticks={} ({:.1} t/s) batched_evals={} readback={}B dropped={} refused={} moves={} (unmoved={}) frame_ms=[p50={:.1} p95={:.1} p99={:.1} max={:.1}] lat_ms=[p50={:.1} p95={:.1} max={:.1}] bound_ms=[p50={:.1} p95={:.1} max={:.1}] digest={}",
                    r.label,
                    r.logical[0],
                    r.logical[1],
                    r.drawable[0],
                    r.drawable[1],
                    r.backing_scale,
                    r.controllers_used,
                    r.wall_seconds,
                    r.presented,
                    r.fps,
                    r.ticks,
                    r.ticks_per_second,
                    r.batched_evals,
                    r.readback_bytes,
                    r.input_dropped,
                    r.refused,
                    r.moves_posted,
                    r.moves_unmoved,
                    r.frame_p50_ms,
                    r.frame_p95_ms,
                    r.frame_p99_ms,
                    r.frame_max_ms,
                    r.lat_p50_ms,
                    r.lat_p95_ms,
                    r.lat_max_ms,
                    r.bound_p50_ms,
                    r.bound_p95_ms,
                    r.bound_max_ms,
                    &r.digest_hex[..16],
                );
                if r.readback_bytes != 0 {
                    eprintln!(
                        "window_g6: [{label}] unexpected readback_bytes={}",
                        r.readback_bytes
                    );
                    return ExitCode::FAILURE;
                }
                if r.input_dropped != 0 || r.refused != 0 {
                    eprintln!(
                        "window_g6: [{label}] dropped={} refused={}",
                        r.input_dropped, r.refused
                    );
                    return ExitCode::FAILURE;
                }
                if r.moves_unmoved != 0 {
                    eprintln!("window_g6: [{label}] moves_unmoved={}", r.moves_unmoved);
                    return ExitCode::FAILURE;
                }
                if r.fps < PACED_SHARE * r.paced_fps {
                    eprintln!(
                        "window_g6: [{label}] presented {:.2} fps < {:.0}% of the paced {:.0} fps G6 display",
                        r.fps,
                        100.0 * PACED_SHARE,
                        r.paced_fps
                    );
                    return ExitCode::FAILURE;
                }
                if r.bound_p95_ms > 100.0 {
                    eprintln!(
                        "window_g6: [{label}] MoveLatencyBound p95 {:.1} ms > 100.0 ms G6 limit",
                        r.bound_p95_ms
                    );
                    return ExitCode::FAILURE;
                }
            }
            Err(e) => {
                eprintln!("window_g6: FAILED [{label}]: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    println!(
        "window_g6: all real-window G6 cases passed ({} of FL-034's {} profiles run)",
        profiles.len(),
        PROFILE_RUNS.len()
    );
    ExitCode::SUCCESS
}
