//! `flife-studio`: the live world in a native window (FL-018; plan §13.6).
//!
//! Every line on standard output is one `frankenlife.studio.v1` record: an
//! `open`, then a `status` per session status, a `view` per style change,
//! a `selection` each time the selected organism changes (a click, or the
//! selected organism gone from a newer epoch), an `inspected` record for
//! each organism a click selects, and about once a second while it stays
//! selected and the world runs (FL-021: it at the session's committed
//! boundary `tick`, selected on the epoch of `seen_tick`), a `brush` each
//! time the armed brush changes, a `submitted` record for each brush click
//! the session accepted as a command (FL-022: the boundary of
//! `effective_tick` it applies at) and a `finished` record when it applied
//! (the tick, the published tick and the energy it moved) or was cancelled,
//! a `frame` summary about once a second and, while `[presentation]
//! show_clock_telemetry` is set, a `clock` record beside it (plan §12.2:
//! ticks committed per wall second, the simulation-time to wall-time
//! ratio, frames presented per second and the display's backlog in ticks),
//! then an `end` with the committed world's digest and the frames' times
//! (FL-024's G6 evidence: the median, 95th and 99th percentiles and the
//! longest of each frame's work, from draining input to the overlay set,
//! without the wait for the next frame). The window draws the
//! same state as text in its top-left corner: running or paused at the
//! shown tick, the clocks while they are reported, the colour mapping's key
//! (FL-019), the armed brush and the selected organism (FL-024), and under
//! them that organism's last inspection (FL-021's panel). An error
//! is one `frankenlife.error.v1` record on standard error. Exit status: 0
//! after a clean close, 2 for an unreadable or invalid configuration or a
//! window that cannot open, 1 for any later failure.
//!
//! `--offscreen SECONDS` runs the same loop with no window for that long
//! (FL-024's frame study): each frame is rendered through the same Metal
//! route and read back, at `--scale` pixels a point, and nothing posts
//! input but `--offscreen-zooms`' camera moves.
//!
//! `--timeline FILE` records the session's presentation timeline (FL-023,
//! plan §13.8) to FILE, which must not exist: one `frankenlife.timeline.v1`
//! record a line, written as the frames go, closing with an `end` record
//! when the Studio closes cleanly. `--export TIMELINE --out DIR` opens no
//! Studio and no window. It replays TIMELINE headless through the same
//! Metal route and writes each frame as a PNG into DIR, which must not
//! exist. It also writes the frames on a constant `--export-fps` clock (the
//! configuration's `[presentation] frames_per_second` by default) as y4m,
//! and a `frankenlife.export.v1` manifest. Then it prints one `export`
//! record.

#![forbid(unsafe_code)]

use std::io::{BufWriter, Write};
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::Parser;
use flife_backend_metal::ControllerChoice;
use flife_core::memory::MemoryAdmission;
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_engine::OrganismSnapshot;
use flife_render::Metrics;
use flife_runtime::Outcome;
use flife_studio::{
    Brush, BrushPreview, ClockMeter, ClockReading, ColorMapping, End, ExportManifest,
    ExportOptions, FieldMapping, FrameTimeSummary, FrameTimes, Inspected, Look, OverlayState,
    Particles, STUDIO_SCHEMA, Status, Stroke, Studio, StudioOptions, Style, Surface, Telemetry,
    TimelineRecord, legends, overlay_text, write_timeline_record,
};
use fmn_render::metal::{InputEvent, LogicalPoint, Modifiers, ScrollUnit};
use serde::Serialize;

const ERROR_SCHEMA: &str = "frankenlife.error.v1";

#[derive(Debug, Parser)]
#[command(
    name = "flife-studio",
    version,
    about = "FrankenLife's Studio: the live world in a native window"
)]
struct Cli {
    /// The configuration or preset, as TOML.
    config: PathBuf,
    /// Content width in points; `[presentation] width` by default.
    #[arg(long, requires = "height")]
    width: Option<u32>,
    /// Content height in points; `[presentation] height` by default.
    #[arg(long, requires = "width")]
    height: Option<u32>,
    /// Keep the window at its opening size.
    #[arg(long)]
    fixed: bool,
    /// Open paused; Space starts the world.
    #[arg(long)]
    paused: bool,
    /// Threads for the world's field and organism phases: a count, or
    /// `auto` (the default) for one per available core, up to 16. No result
    /// depends on it.
    #[arg(long, default_value = "auto", value_parser = parse_workers)]
    cpu_workers: NonZeroUsize,
    /// Where due controllers are evaluated: `metal`, `cpu` (the reference),
    /// `torch` (FrankenTorch's CPU kernel on the world's workers), or `auto`
    /// (the default): Metal where this host has it and the world uses at
    /// most 8 CPU workers (with 16 it measured no faster), else `torch` at
    /// most 4. No result depends on it.
    #[arg(long, default_value = "auto")]
    controllers: ControllerChoice,
    /// What the organisms' colour means: `lineage`, `energy`, `generation`,
    /// `motion`, `topology` or `affinity` (or a registered name such as
    /// `founder-lineage-v1`). The default is the configuration's
    /// `[presentation] color_mapping`. Keys 1 to 6 change it.
    #[arg(long)]
    color: Option<ColorMapping>,
    /// `glow` (the default) or `raw`: no halos and a linear grey field. R
    /// switches.
    #[arg(long, default_value = "glow")]
    look: Look,
    /// Draw organism density per screen bin instead of dots. D switches.
    #[arg(long)]
    density: bool,
    /// Hide the energy field. F switches.
    #[arg(long)]
    no_field: bool,
    /// What the field's colour means: `energy` (the default) or
    /// `composition`, its mean chemical composition coloured as the
    /// affinity mapping colours organisms, as bright as its energy. C
    /// switches.
    #[arg(long, default_value = "energy")]
    field_mapping: FieldMapping,
    /// Run for SECONDS wall seconds with no window, then close: each frame
    /// is rendered through the same Metal route and read back, and nothing
    /// posts input (FL-024's frame study).
    #[arg(long, value_name = "SECONDS", value_parser = parse_seconds)]
    offscreen: Option<f64>,
    /// Device pixels a point offscreen, as a screen's backing scale.
    #[arg(long, default_value = "2", requires = "offscreen")]
    scale: NonZeroU32,
    /// Offscreen, a camera move arrives in every third frame interval, at a
    /// moment drawn uniformly over it (a fixed seed): a scroll of 4 points
    /// at the content's centre, zooming in and out in turn. The `end`
    /// record's `camera_latency` is each one's time from its arrival to the
    /// end of the frame that drew it (G6). A move that arrives during a
    /// frame's work waits for the next frame, as a native queue holds it.
    #[arg(long, requires = "offscreen")]
    offscreen_zooms: bool,
    /// Record the session's presentation timeline (FL-023) to FILE, which
    /// must not exist.
    #[arg(long, value_name = "FILE", conflicts_with = "export")]
    timeline: Option<PathBuf>,
    /// Open no Studio: replay the timeline TIMELINE headless and export
    /// its frames, video and manifest to `--out` (FL-023).
    #[arg(
        long,
        value_name = "TIMELINE",
        requires = "out",
        conflicts_with = "offscreen"
    )]
    export: Option<PathBuf>,
    /// The export's directory, which must not exist.
    #[arg(long, value_name = "DIR", requires = "export")]
    out: Option<PathBuf>,
    /// The exported video's constant rate; the configuration's
    /// `[presentation] frames_per_second` by default.
    #[arg(long, value_name = "FPS", requires = "export")]
    export_fps: Option<NonZeroU32>,
}

/// A timeline file being written (`--timeline`).
struct TimelineFile {
    path: PathBuf,
    out: BufWriter<std::fs::File>,
}

impl TimelineFile {
    fn create(path: &Path) -> Result<Self, CoreError> {
        let file = std::fs::File::create_new(path)
            .map_err(|error| storage("create the timeline", path, &error))?;
        Ok(Self {
            path: path.to_owned(),
            out: BufWriter::new(file),
        })
    }

    fn write<'a>(
        &mut self,
        records: impl IntoIterator<Item = &'a TimelineRecord>,
    ) -> Result<(), CoreError> {
        for record in records {
            write_timeline_record(&mut self.out, record)
                .map_err(|error| storage("write the timeline", &self.path, &error))?;
        }
        Ok(())
    }

    /// Write the last `records` and make the file durable.
    fn finish<'a>(
        mut self,
        records: impl IntoIterator<Item = &'a TimelineRecord>,
    ) -> Result<(), CoreError> {
        self.write(records)?;
        let path = self.path;
        let file = self
            .out
            .into_inner()
            .map_err(|error| storage("write the timeline", &path, error.error()))?;
        file.sync_all()
            .map_err(|error| storage("write the timeline", &path, &error))
    }
}

fn storage(operation: &'static str, path: &Path, error: &std::io::Error) -> CoreError {
    CoreError::StorageFailed {
        operation,
        path: path.display().to_string(),
        reason: error.to_string(),
    }
}

/// `--export`: replay `timeline` and export it to `out`.
fn export_timeline(config: &ValidatedConfig, cli: &Cli, timeline: &Path, out: &Path) -> ExitCode {
    let text = match std::fs::read_to_string(timeline) {
        Ok(text) => text,
        Err(error) => {
            let error = storage("read the timeline", timeline, &error);
            return fail(error.kind(), error.to_string(), 2);
        }
    };
    let configured = config.config().presentation.frames_per_second;
    let options = ExportOptions {
        frames_per_second: cli
            .export_fps
            .unwrap_or_else(|| NonZeroU32::new(configured).unwrap_or(NonZeroU32::MIN)),
        cpu_workers: Some(cli.cpu_workers),
    };
    match flife_studio::export(config, &text, options, out) {
        Ok(manifest) => {
            print(&export_record(&manifest, out));
            ExitCode::SUCCESS
        }
        Err(error) => fail(error.kind(), error.to_string(), 1),
    }
}

fn export_record<'a>(manifest: &'a ExportManifest, out: &Path) -> Record<'a> {
    Record::Export {
        schema: STUDIO_SCHEMA,
        out: out.display().to_string(),
        timeline_sha256: &manifest.timeline_sha256,
        frames: manifest.frames.len(),
        ticks: manifest.ticks,
        end_tick: manifest.end_tick,
        digest: &manifest.digest,
        commands: manifest.commands,
        frames_per_second: manifest.video.as_ref().map(|video| video.frames_per_second),
        slots: manifest.video.as_ref().map(|video| video.slots),
        repeated: manifest.video.as_ref().map(|video| video.repeated),
        dropped: manifest.video.as_ref().map(|video| video.dropped),
        video_withheld: manifest.video_withheld.as_deref(),
    }
}

/// The fixed-seed draws of `--offscreen-zooms`' arrival moments.
struct Xorshift(u64);

impl Xorshift {
    /// A draw uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        #[expect(clippy::cast_precision_loss, reason = "53 bits fit an f64 exactly")]
        let unit = (self.0 >> 11) as f64 / (1_u64 << 53) as f64;
        unit
    }
}

/// A positive, finite number of seconds.
fn parse_seconds(text: &str) -> Result<f64, String> {
    match text.parse::<f64>() {
        Ok(seconds) if seconds.is_finite() && seconds > 0.0 => Ok(seconds),
        _ => Err(format!("`{text}` is not a positive number of seconds")),
    }
}

/// A worker count, or `auto`.
fn parse_workers(text: &str) -> Result<NonZeroUsize, String> {
    if text == "auto" {
        return Ok(std::thread::available_parallelism()
            .map_or(NonZeroUsize::MIN, |cores| cores.min(AUTO_WORKER_CEILING)));
    }
    text.parse()
        .map_err(|_| format!("`{text}` is not a positive worker count or `auto`"))
}

/// The most workers `auto` picks.
const AUTO_WORKER_CEILING: NonZeroUsize = match NonZeroUsize::new(16) {
    Some(ceiling) => ceiling,
    None => NonZeroUsize::MIN,
};

#[derive(Serialize)]
#[serde(rename_all = "kebab-case", tag = "record")]
enum Record<'a> {
    Open {
        schema: &'static str,
        name: &'a str,
        config_hash: String,
        world: [f64; 2],
        capacity: usize,
        window: [u32; 2],
        frames_per_second: u32,
        cpu_workers: usize,
        controllers: &'a str,
        memory: &'a MemoryAdmission,
        mapping: &'static str,
        meaning: &'static str,
        look: &'static str,
        particles: &'static str,
        field: bool,
        field_mapping: &'static str,
        clock_telemetry: bool,
        /// `window` or `offscreen`.
        surface: &'static str,
    },
    View {
        schema: &'static str,
        mapping: &'static str,
        meaning: &'static str,
        look: &'static str,
        particles: &'static str,
        field: bool,
        field_mapping: &'static str,
        shown_tick: Option<u64>,
    },
    Selection {
        schema: &'static str,
        organism: Option<u64>,
        position: Option<[f64; 2]>,
        shown_tick: Option<u64>,
    },
    /// The armed brush changed (FL-022); `None` when disarmed.
    Brush {
        schema: &'static str,
        brush: Option<&'static str>,
    },
    /// A brush's command was accepted for the boundary of `effective_tick`.
    Submitted {
        schema: &'static str,
        command: u64,
        brush: &'static str,
        #[serde(flatten)]
        payload: Payload,
        seen_tick: u64,
        effective_tick: u64,
    },
    /// A command finished: `applied` at the boundary of `tick`, published
    /// as `published_tick`, moving `energy`; or `cancelled` or `failed`.
    Finished {
        schema: &'static str,
        command: u64,
        outcome: &'static str,
        tick: Option<u64>,
        published_tick: Option<u64>,
        energy: Option<f64>,
        detail: Option<String>,
    },
    Status {
        schema: &'static str,
        status: &'static str,
        run: Option<u64>,
        tick: Option<u64>,
        why: Option<&'static str>,
        detail: Option<String>,
    },
    Inspected {
        schema: &'static str,
        organism: u64,
        seen_tick: u64,
        /// The committed boundary it was read at; absent when the committed
        /// world no longer holds it.
        tick: Option<u64>,
        snapshot: Option<Inspection>,
    },
    /// The clocks over the last window (plan §12.2).
    Clock {
        schema: &'static str,
        wall_seconds: f64,
        ticks: u64,
        ticks_per_second: f64,
        simulation_rate: f64,
        frames: u64,
        frames_per_second: f64,
        committed_tick: u64,
        shown_tick: Option<u64>,
        behind_ticks: Option<u64>,
        evaluations: Option<u64>,
        evaluations_per_second: Option<f64>,
    },
    /// Plan §14.1's persistent telemetry, from the shown epoch.
    Telemetry {
        schema: &'static str,
        shown_tick: u64,
        live: u64,
        parameters: u64,
        genome_logical_bytes: u64,
        genome_allocated_bytes: u64,
        headroom_bytes: u64,
        counted_from_tick: Option<u64>,
        births: Option<u64>,
        introductions: Option<u64>,
        deaths: Option<u64>,
        evaluations: Option<u64>,
        rejected_births: Option<u64>,
        uptake: Option<f64>,
        injected: f64,
        removed: f64,
        dissipated: f64,
        residual: f64,
        faulted: bool,
        capacity_pauses: u64,
        full_evaluations_per_second: Option<f64>,
    },
    Frame {
        schema: &'static str,
        frames: u64,
        presented: u64,
        rendered: u64,
        occluded: u64,
        epochs: u64,
        shown_tick: Option<u64>,
        hover: Option<[f64; 2]>,
        center: [f64; 2],
        zoom: f64,
        input_dropped: u64,
        upload_bytes: u64,
        peak_upload_bytes: u64,
        readback_bytes: u64,
    },
    End {
        schema: &'static str,
        tick: u64,
        digest: String,
        frames: u64,
        presented: u64,
        rendered: u64,
        epochs: u64,
        refits: u64,
        strokes: u64,
        unstroked: u64,
        intents: u64,
        refused: u64,
        keys: u64,
        pointer_moves: u64,
        resizes: u64,
        input_dropped: u64,
        controller_evaluations: u64,
        deferred_evaluations: u64,
        controller_failure: Option<String>,
        upload_bytes: u64,
        peak_upload_bytes: u64,
        readback_bytes: u64,
        wall_seconds: f64,
        /// Each frame's work (FL-024's G6 evidence); absent before a frame.
        frame_times: Option<FrameTimeSummary>,
        /// With `--offscreen-zooms`: the moves posted, those whose frame
        /// did not move the view, and their latency.
        camera_moves: Option<u64>,
        camera_moves_unmoved: Option<u64>,
        camera_latency: Option<FrameTimeSummary>,
    },
    /// `--export`'s summary (FL-023); the manifest in `out` has the rest.
    Export {
        schema: &'static str,
        out: String,
        timeline_sha256: &'a str,
        frames: usize,
        /// The ticks of the first and last frames.
        ticks: [u64; 2],
        end_tick: u64,
        digest: &'a str,
        commands: usize,
        /// The video's rate, slots, repeats and drops; absent when withheld.
        frames_per_second: Option<u32>,
        slots: Option<u64>,
        repeated: Option<u64>,
        dropped: Option<u64>,
        video_withheld: Option<&'a str>,
    },
}

/// A stroke's payload, its absent fields null (FL-022).
#[derive(Serialize, Default)]
struct Payload {
    cell: Option<[u32; 2]>,
    amount: Option<f64>,
    /// The material's composition per unit energy.
    direction: Option<[f64; 32]>,
    center: Option<[f64; 2]>,
    radius: Option<f64>,
    /// The organisms introduced, each with `endowment`.
    count: Option<u32>,
    endowment: Option<f64>,
}

impl Payload {
    fn of(stroke: Stroke) -> Self {
        match stroke {
            Stroke::Energy { cell, amount } => Self {
                cell: Some(cell),
                amount: Some(amount),
                ..Self::default()
            },
            Stroke::Removal { center, radius } => Self {
                center: Some(center),
                radius: Some(radius),
                ..Self::default()
            },
            Stroke::Material {
                cell,
                amount,
                direction,
            } => Self {
                cell: Some(cell),
                amount: Some(amount),
                direction: Some(direction),
                ..Self::default()
            },
            Stroke::Introduction {
                cell,
                count,
                energy,
            } => Self {
                cell: Some(cell),
                count: Some(count),
                endowment: Some(energy),
                ..Self::default()
            },
        }
    }
}

/// One organism at a committed tick: its row, age, controller shape and
/// heritable traits, and its held output as the world decodes it.
#[derive(Serialize)]
struct Inspection {
    genome: u64,
    parent: Option<u64>,
    lineage: u64,
    generation: u32,
    birth_tick: u64,
    age_ticks: u64,
    position: [f64; 2],
    velocity: [f64; 2],
    energy: f64,
    energy_change_rate: f64,
    phase: u32,
    cooldown_until_tick: u64,
    topology: usize,
    hidden_widths: Vec<u32>,
    parameters: usize,
    sensing_scale: f64,
    motion_response: Vec<f64>,
    affinity: Vec<f64>,
    decoded: Option<Decoded>,
}

#[derive(Serialize)]
struct Decoded {
    attempt_rate: f64,
    attempt_probability: f64,
    child_fraction: f64,
    emission_gate: f64,
    emission_direction: Option<Vec<f64>>,
    emission_spend_fraction: Option<f64>,
}

#[derive(Serialize)]
struct ErrorRecord<'a> {
    schema: &'static str,
    kind: &'a str,
    detail: String,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let config = match load(&cli.config) {
        Ok(config) => config,
        Err(code) => return code,
    };
    if let (Some(timeline), Some(out)) = (&cli.export, &cli.out) {
        return export_timeline(&config, &cli, timeline, out);
    }
    let mapping = match cli
        .color
        .map_or_else(|| ColorMapping::configured(&config), Ok)
    {
        Ok(mapping) => mapping,
        Err(error) => return fail(error.kind(), error.to_string(), 2),
    };
    let options = StudioOptions {
        size: cli
            .width
            .zip(cli.height)
            .map(|(width, height)| [width, height]),
        resizable: !cli.fixed,
        cpu_workers: Some(cli.cpu_workers),
        run_on_open: !cli.paused,
        controllers: cli.controllers,
        style: Style {
            mapping,
            look: cli.look,
            particles: if cli.density {
                Particles::Density
            } else {
                Particles::Points
            },
            field: !cli.no_field,
            field_mapping: cli.field_mapping,
        },
        surface: cli
            .offscreen
            .map_or(Surface::Window, |_| Surface::Offscreen { scale: cli.scale }),
        record: cli.timeline.is_some(),
        ..StudioOptions::default()
    };
    // Refused before a window opens; creating it refuses a race as well.
    if let Some(path) = cli.timeline.as_deref().filter(|path| path.exists()) {
        let error = CoreError::StorageFailed {
            operation: "create the timeline",
            path: path.display().to_string(),
            reason: "it exists, and a timeline is never written over".to_owned(),
        };
        return fail(error.kind(), error.to_string(), 2);
    }
    let clock = Instant::now();
    let mut studio = match Studio::open(&config, &options) {
        Ok(studio) => studio,
        Err(error) => return fail(error.kind(), error.to_string(), 2),
    };
    let mut timeline = match cli
        .timeline
        .as_deref()
        .map(TimelineFile::create)
        .transpose()
    {
        Ok(timeline) => timeline,
        Err(error) => {
            let code = fail(error.kind(), error.to_string(), 2);
            let _ = studio.close();
            return code;
        }
    };
    // The session's statuses a frame took, for the timeline.
    let mut noted = Vec::new();
    let model = config.config();
    let fps = model.presentation.frames_per_second.max(1);
    print(&Record::Open {
        schema: STUDIO_SCHEMA,
        name: &model.name,
        config_hash: config.config_hash().to_hex(),
        world: studio.view().viewport().extent(),
        capacity: studio.view().cloud().capacity(),
        window: studio.view().viewport().drawable(),
        frames_per_second: fps,
        cpu_workers: cli.cpu_workers.get(),
        controllers: studio.controllers(),
        memory: studio.memory(),
        mapping: options.style.mapping.name(),
        meaning: options.style.mapping.meaning(),
        look: options.style.look.name(),
        particles: options.style.particles.name(),
        field: options.style.field,
        field_mapping: options.style.field_mapping.name(),
        clock_telemetry: model.presentation.show_clock_telemetry,
        surface: if studio.is_offscreen() {
            "offscreen"
        } else {
            "window"
        },
    });
    let deadline = cli
        .offscreen
        .map(|seconds| Instant::now() + Duration::from_secs_f64(seconds));

    let period = Duration::from_secs(1) / fps;
    // Frames start on a fixed schedule, `period` apart: a wait that
    // overshoots (macOS adds slack to a sleeping thread's timer) shortens
    // the next wait instead of delaying every later frame, and a frame that
    // runs past its slot starts the schedule again from its end.
    let mut slot = Instant::now();
    let mut summary = Instant::now();
    let mut clocks = ClockMeter::new(config.report().timestep);
    if model.presentation.show_clock_telemetry {
        let _ = clocks.read(
            summary,
            studio.committed().map_or(Tick::ZERO, |epoch| epoch.tick),
            studio.tally().drawn(),
            None,
            evaluated(&studio),
        );
    }
    let mut selected = None;
    let mut armed = None;
    // The overlay (FL-024): the session's last reported state, the tick
    // shown when it or the clocks last changed, and the last clock reading.
    let mut paused = None;
    let mut overlay_tick = None;
    let mut last_clock = None;
    // The shown epoch's metrics when the overlay last changed (plan §14.1's
    // telemetry), the current run, whose end leaves the world paused, and
    // the capacity shortfalls reported.
    let mut telemetry_metrics: Option<Metrics> = None;
    let mut current_run = None;
    let mut capacity_pauses = 0_u64;
    // The last inspection's answer and whom it was about (FL-021's panel).
    let mut inspected = None;
    // Every colour mapping's key (FL-019's legend).
    let keys = match legends(&config) {
        Ok(keys) => keys,
        Err(error) => {
            let code = fail(error.kind(), error.to_string(), 1);
            let _ = studio.close();
            return code;
        }
    };
    let (mut times, mut latency) = match FrameTimes::new().and_then(|a| Ok((a, FrameTimes::new()?)))
    {
        Ok(both) => both,
        Err(error) => {
            let code = fail(error.kind(), error.to_string(), 1);
            let _ = studio.close();
            return code;
        }
    };
    // `--offscreen-zooms`: the arrival of the move posted for the next
    // frame, the moves posted and those that did not move the view.
    let mut arrived: Option<Instant> = None;
    let (mut moves, mut unmoved) = (0_u64, 0_u64);
    let mut draws = Xorshift(0x9E37_79B9_7F4A_7C15);
    while !studio.should_close() && deadline.is_none_or(|deadline| Instant::now() < deadline) {
        let started = Instant::now();
        let frame = match studio.frame() {
            Ok(frame) => frame,
            Err(error) => {
                let code = fail(error.kind(), error.to_string(), 1);
                let _ = studio.close();
                return code;
            }
        };
        if let Some(at) = arrived.take() {
            // This frame drained it and drew the move.
            latency.record(at.elapsed());
            if !frame.moved {
                unmoved += 1;
            }
        }
        let mut reported = false;
        for status in studio.take_statuses() {
            if timeline.is_some() {
                noted.extend(TimelineRecord::status(&status));
            }
            match &status {
                Status::Running { run, .. } => {
                    paused = Some(false);
                    current_run = Some(*run);
                }
                Status::Paused { .. } => paused = Some(true),
                // The current run's end leaves the world paused, however it
                // ended; an earlier run's end can arrive after a later run
                // started.
                Status::Ended { run, why, .. } => {
                    if current_run == Some(*run) {
                        paused = Some(true);
                    }
                    if matches!(why, End::CapacityPaused { .. }) {
                        capacity_pauses += 1;
                    }
                }
                Status::StepHeld {
                    why: End::CapacityPaused { .. },
                    ..
                } => capacity_pauses += 1,
                Status::Inspected {
                    organism, snapshot, ..
                } => inspected = Some((*organism, snapshot.clone())),
                _ => {}
            }
            overlay_tick = frame.shown.map(|shown| shown.tick);
            reported = true;
            print(&status_record(&status));
        }
        if reported {
            telemetry_metrics = Some(*studio.display_pair().1.metrics());
        }
        if frame.restyled {
            let style = studio.view().style();
            print(&Record::View {
                schema: STUDIO_SCHEMA,
                mapping: style.mapping.name(),
                meaning: style.mapping.meaning(),
                look: style.look.name(),
                particles: style.particles.name(),
                field: style.field,
                field_mapping: style.field_mapping.name(),
                shown_tick: frame.shown.map(|shown| shown.tick.get()),
            });
        }
        if frame.brush != armed {
            armed = frame.brush;
            print(&Record::Brush {
                schema: STUDIO_SCHEMA,
                brush: armed.map(Brush::name),
            });
        }
        if frame.selected != selected {
            selected = frame.selected;
            print(&Record::Selection {
                schema: STUDIO_SCHEMA,
                organism: selected.map(|id| id.get()),
                position: studio.view().selected_position(),
                shown_tick: frame.shown.map(|shown| shown.tick.get()),
            });
        }
        if summary.elapsed() >= Duration::from_secs(1) {
            summary = Instant::now();
            overlay_tick = frame.shown.map(|shown| shown.tick);
            telemetry_metrics = Some(*studio.display_pair().1.metrics());
            let tally = studio.tally();
            let viewport = studio.view().viewport();
            print(&Record::Frame {
                schema: STUDIO_SCHEMA,
                frames: tally.frames,
                presented: tally.presented,
                rendered: tally.rendered,
                occluded: tally.occluded,
                epochs: tally.epochs,
                shown_tick: frame.shown.map(|shown| shown.tick.get()),
                hover: frame.hover,
                center: viewport.center(),
                zoom: viewport.zoom(),
                input_dropped: studio.input_dropped(),
                upload_bytes: tally.upload_bytes,
                peak_upload_bytes: tally.peak_upload_bytes,
                readback_bytes: tally.readback_bytes,
            });
            if model.presentation.show_clock_telemetry
                && let Some(reading) = clocks.read(
                    summary,
                    studio.committed().map_or(Tick::ZERO, |epoch| epoch.tick),
                    tally.drawn(),
                    frame.shown.map(|shown| shown.tick),
                    evaluated(&studio),
                )
            {
                print(&clock_record(&reading));
                last_clock = Some(reading);
            }
            if let Some(metrics) = &telemetry_metrics {
                print(&telemetry_record(
                    metrics,
                    last_clock.as_ref(),
                    studio.memory(),
                    capacity_pauses,
                ));
            }
            // A running world moves on: read the selected organism again.
            if selected.is_some() && paused == Some(false) {
                studio.inspect_selected();
            }
        }
        if overlay_tick.is_none() {
            overlay_tick = frame.shown.map(|shown| shown.tick);
        }
        if telemetry_metrics.is_none() {
            telemetry_metrics = Some(*studio.display_pair().1.metrics());
        }
        let style = studio.view().style();
        let mapping = style.mapping;
        let field_key = flife_studio::field_key(style);
        let Some(legend) = keys
            .iter()
            .find_map(|(key, legend)| (*key == mapping).then_some(legend))
        else {
            let code = fail("internal", format!("no key for the {mapping} mapping"), 1);
            let _ = studio.close();
            return code;
        };
        let text = overlay_text(&OverlayState {
            paused,
            shown: overlay_tick,
            clock: last_clock.as_ref(),
            legend,
            field_key: field_key.as_ref(),
            brush: armed,
            preview: frame.preview.map(|preview| BrushPreview {
                preview,
                recycle_fraction: model.death.residual_energy_recycle_fraction,
            }),
            selected,
            inspected: inspected
                .as_ref()
                .filter(|(organism, _)| selected == Some(*organism))
                .map(|(_, snapshot)| snapshot.as_deref().map_or(Inspected::Gone, Inspected::Read)),
            telemetry: telemetry_metrics.as_ref().map(|metrics| Telemetry {
                metrics,
                model: &model.model,
                seed: model.seed,
                evaluation_interval_ticks: model.controller.evaluation_interval_ticks,
                memory: studio.memory(),
                capacity_pauses,
                checkpoint: None,
            }),
        });
        let swatches: Vec<[u8; 3]> = legend
            .swatches
            .iter()
            .chain(field_key.iter().flat_map(|key| &key.swatches))
            .copied()
            .collect();
        if let Err(error) = studio.set_overlay(&text, &swatches) {
            let code = fail(error.kind(), error.to_string(), 1);
            let _ = studio.close();
            return code;
        }
        times.record(started.elapsed());
        if let Some(timeline) = &mut timeline {
            let records = studio.take_recorded();
            if let Err(error) = timeline.write(records.iter().chain(&noted)) {
                let code = fail(error.kind(), error.to_string(), 1);
                let _ = studio.close();
                return code;
            }
        }
        noted.clear();
        slot = next_slot(slot, period, Instant::now());
        let next = slot;
        if cli.offscreen_zooms && studio.tally().frames.is_multiple_of(3) {
            // Uniformly over this frame's interval; a moment already past
            // is posted now, as the queue would have held it.
            let at = started + (next - started).mul_f64(draws.unit());
            if let Some(wait) = at.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            let centre = studio.surface_metrics().map_or([0.0, 0.0], |metrics| {
                [metrics.logical_width / 2.0, metrics.logical_height / 2.0]
            });
            let delta = if moves.is_multiple_of(2) { 4.0 } else { -4.0 };
            if let Err(error) = studio.post_input(InputEvent::Scroll {
                delta: [0.0, delta],
                unit: ScrollUnit::Points,
                position: LogicalPoint {
                    x: centre[0],
                    y: centre[1],
                },
                modifiers: Modifiers::NONE,
            }) {
                let code = fail(error.kind(), error.to_string(), 1);
                let _ = studio.close();
                return code;
            }
            arrived = Some(at);
            moves += 1;
        }
        if let Some(rest) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(rest);
        }
    }

    let last = studio.take_recorded();
    match studio.close() {
        Ok(closed) => {
            if let Some(timeline) = timeline
                && let Err(error) =
                    timeline.finish(last.iter().chain([&TimelineRecord::end(&closed.session)]))
            {
                return fail(error.kind(), error.to_string(), 1);
            }
            print(&Record::End {
                schema: STUDIO_SCHEMA,
                tick: closed.session.boundary.tick.get(),
                digest: closed.session.world.digest().to_hex(),
                frames: closed.frames.frames,
                presented: closed.frames.presented,
                rendered: closed.frames.rendered,
                epochs: closed.frames.epochs,
                refits: closed.frames.refits,
                strokes: closed.frames.strokes,
                unstroked: closed.frames.unstroked,
                intents: closed.frames.intents,
                refused: closed.frames.refused,
                keys: closed.input.keys,
                pointer_moves: closed.input.pointer_moves,
                resizes: closed.input.resizes,
                input_dropped: closed.input_dropped,
                controller_evaluations: closed.session.world.controller_report().batched,
                deferred_evaluations: closed.session.world.controller_report().deferred,
                controller_failure: closed.session.world.controller_report().failure.clone(),
                upload_bytes: closed.frames.upload_bytes,
                peak_upload_bytes: closed.frames.peak_upload_bytes,
                readback_bytes: closed.frames.readback_bytes,
                wall_seconds: clock.elapsed().as_secs_f64(),
                frame_times: times.summary(),
                camera_moves: cli.offscreen_zooms.then_some(moves),
                camera_moves_unmoved: cli.offscreen_zooms.then_some(unmoved),
                camera_latency: latency.summary(),
            });
            ExitCode::SUCCESS
        }
        Err(error) => fail(error.kind(), error.to_string(), 1),
    }
}

fn status_record(status: &Status) -> Record<'static> {
    let record = |status, run, tick, why, detail| Record::Status {
        schema: STUDIO_SCHEMA,
        status,
        run,
        tick,
        why,
        detail,
    };
    match status {
        Status::Running { run, from } => {
            record("running", Some(*run), Some(from.get()), None, None)
        }
        Status::Paused { at } => record("paused", None, Some(at.get()), None, None),
        Status::Ended { run, at, why } => {
            let (why, detail) = end(why);
            record("ended", Some(*run), Some(at.get()), Some(why), detail)
        }
        Status::Stepped { tick } => record("stepped", None, Some(tick.get()), None, None),
        Status::StepHeld { at, why } => {
            let (why, detail) = end(why);
            record("step-held", None, Some(at.get()), Some(why), detail)
        }
        Status::Refused { kind, detail, .. } => {
            record("refused", None, None, Some(kind), Some(detail.clone()))
        }
        Status::Closed { at } => record("closed", None, Some(at.get()), None, None),
        Status::Submitted {
            command,
            stroke,
            seen,
            effective,
        } => Record::Submitted {
            schema: STUDIO_SCHEMA,
            command: command.get(),
            brush: stroke.brush().name(),
            payload: Payload::of(*stroke),
            seen_tick: seen.get(),
            effective_tick: effective.get(),
        },
        Status::Finished { command, outcome } => {
            let finished = |outcome, tick, published_tick, energy, detail| Record::Finished {
                schema: STUDIO_SCHEMA,
                command: command.get(),
                outcome,
                tick,
                published_tick,
                energy,
                detail,
            };
            match outcome {
                Outcome::Applied {
                    tick,
                    published,
                    applied,
                    ..
                } => finished(
                    "applied",
                    Some(tick.get()),
                    Some(published.get()),
                    Some(*applied),
                    None,
                ),
                Outcome::Cancelled {
                    boundary, reason, ..
                } => finished(
                    "cancelled",
                    Some(boundary.get()),
                    None,
                    None,
                    Some(format!("{reason:?}")),
                ),
                Outcome::Failed { detail, .. } => {
                    finished("failed", None, None, None, Some(detail.clone()))
                }
            }
        }
        Status::Inspected {
            organism,
            seen,
            snapshot,
        } => Record::Inspected {
            schema: STUDIO_SCHEMA,
            organism: organism.get(),
            seen_tick: seen.get(),
            tick: snapshot.as_ref().map(|snapshot| snapshot.tick.get()),
            snapshot: snapshot.as_deref().map(inspection),
        },
    }
}

fn clock_record(reading: &ClockReading) -> Record<'static> {
    Record::Clock {
        schema: STUDIO_SCHEMA,
        wall_seconds: reading.wall_seconds,
        ticks: reading.ticks,
        ticks_per_second: reading.ticks_per_second,
        simulation_rate: reading.simulation_rate,
        frames: reading.frames,
        frames_per_second: reading.frames_per_second,
        committed_tick: reading.committed.get(),
        shown_tick: reading.shown.map(Tick::get),
        behind_ticks: reading.behind,
        evaluations: reading.evaluations,
        evaluations_per_second: reading.evaluations_per_second,
    }
}

/// The shown epoch's controller evaluations, if its tally has them.
fn evaluated(studio: &Studio) -> Option<u64> {
    studio
        .display_pair()
        .1
        .metrics()
        .tally
        .map(|tally| tally.evaluations)
}

fn telemetry_record(
    metrics: &Metrics,
    clock: Option<&ClockReading>,
    memory: &MemoryAdmission,
    capacity_pauses: u64,
) -> Record<'static> {
    let tally = metrics.tally;
    Record::Telemetry {
        schema: STUDIO_SCHEMA,
        shown_tick: metrics.tick.get(),
        live: metrics.live,
        parameters: metrics.parameters,
        genome_logical_bytes: metrics.genome_logical_bytes,
        genome_allocated_bytes: metrics.genome_allocated_bytes,
        headroom_bytes: memory.budget_bytes.saturating_sub(memory.peak_bytes),
        counted_from_tick: tally.map(|tally| tally.since.get()),
        births: tally.map(|tally| tally.births),
        introductions: tally.map(|tally| tally.introductions),
        deaths: tally.map(|tally| tally.deaths),
        evaluations: tally.map(|tally| tally.evaluations),
        rejected_births: tally.map(|tally| tally.rejected_births),
        uptake: tally.map(|tally| tally.uptake),
        injected: metrics.ledger.injected,
        removed: metrics.ledger.removed,
        dissipated: metrics.ledger.dissipated,
        residual: metrics.ledger.residual,
        faulted: metrics.faulted,
        capacity_pauses,
        full_evaluations_per_second: clock
            .and_then(|clock| clock.evaluations_per_second)
            .filter(|_| metrics.live > 0)
            .map(|rate| rate / metrics.live as f64),
    }
}

fn inspection(snapshot: &OrganismSnapshot) -> Inspection {
    let organism = &snapshot.organism;
    Inspection {
        genome: organism.genome.get(),
        parent: organism.parent.map(|parent| parent.get()),
        lineage: organism.lineage.get(),
        generation: organism.generation.get(),
        birth_tick: organism.birth_tick.get(),
        age_ticks: snapshot.age_ticks,
        position: organism.position,
        velocity: organism.velocity,
        energy: organism.energy,
        energy_change_rate: organism.energy_change_rate,
        phase: organism.phase,
        cooldown_until_tick: organism.cooldown_deadline.get(),
        topology: snapshot.topology,
        hidden_widths: snapshot.hidden_widths.clone(),
        parameters: snapshot.parameters,
        sensing_scale: snapshot.traits.sensing_scale(),
        motion_response: snapshot.traits.motion_response().to_vec(),
        affinity: snapshot.traits.affinity().to_vec(),
        decoded: snapshot.decoded.map(|decoded| Decoded {
            attempt_rate: decoded.attempt_rate,
            attempt_probability: decoded.attempt_probability,
            child_fraction: decoded.child_fraction,
            emission_gate: decoded.emission_gate,
            emission_direction: decoded.emission.map(|emission| emission.direction.to_vec()),
            emission_spend_fraction: decoded.emission.map(|emission| emission.spend_fraction),
        }),
    }
}

fn end(why: &End) -> (&'static str, Option<String>) {
    match why {
        End::Paused => ("paused", None),
        End::Reached => ("reached", None),
        End::CapacityPaused { detail } => ("capacity-paused", Some(detail.clone())),
        End::Failed { kind, detail } => (kind, Some(detail.clone())),
        End::Closed => ("closed", None),
    }
}

/// The next frame's start: one `period` after this frame's slot, so an
/// oversleep shortens the next wait instead of delaying every later frame,
/// or `now` when this frame ran past it, so a long frame is not followed by
/// a burst of frames catching up.
fn next_slot(slot: Instant, period: Duration, now: Instant) -> Instant {
    (slot + period).max(now)
}

fn load(path: &PathBuf) -> Result<ValidatedConfig, ExitCode> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| fail("io", format!("{}: {error}", path.display()), 2))?;
    ValidatedConfig::from_toml(&text).map_err(|error| fail(error.kind(), error.to_string(), 2))
}

fn print(record: &impl Serialize) {
    match serde_json::to_string(record) {
        Ok(line) => {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
        }
        Err(error) => {
            fail("serialization", error.to_string(), 1);
        }
    }
}

fn fail(kind: &str, detail: String, code: u8) -> ExitCode {
    let record = ErrorRecord {
        schema: ERROR_SCHEMA,
        kind,
        detail,
    };
    if let Ok(line) = serde_json::to_string(&record) {
        let _ = writeln!(std::io::stderr().lock(), "{line}");
    }
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    //! The FL-022 records as the binary prints them: what the window-run
    //! paths print is `status_record` serialized, which needs no window.
    //! And FL-024's offscreen arguments, on the parser alone.

    use clap::Parser;
    use flife_core::Tick;
    use flife_core::ids::{CommandId, ResourceEpoch};
    use flife_runtime::{CancelReason, Outcome};
    use flife_studio::{Status, Stroke};
    use serde_json::{Value, json};

    #[test]
    fn offscreen_arguments_are_parsed_and_refused() {
        let parse = |args: &[&str]| {
            super::Cli::try_parse_from(["flife-studio", "preset.toml"].iter().chain(args).copied())
        };
        let cli = parse(&["--offscreen", "2.5"]).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(cli.offscreen, Some(2.5));
        assert_eq!(cli.scale.get(), 2, "two pixels a point by default");
        let cli = parse(&["--offscreen", "1", "--scale", "3"]).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!((cli.offscreen, cli.scale.get()), (Some(1.0), 3));
        let cli = parse(&[]).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(cli.offscreen, None, "a window by default");
        assert!(!cli.offscreen_zooms);
        let cli =
            parse(&["--offscreen", "1", "--offscreen-zooms"]).unwrap_or_else(|e| panic!("{e}"));
        assert!(cli.offscreen_zooms);
        for args in [
            &["--offscreen", "0"][..],
            &["--offscreen", "-1"],
            &["--offscreen", "NaN"],
            &["--offscreen", "inf"],
            &["--offscreen", "soon"],
            &["--offscreen", "1", "--scale", "0"],
            // A scale and posted moves are an offscreen surface's.
            &["--scale", "2"],
            &["--offscreen-zooms"],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn timeline_and_export_arguments_are_parsed_and_refused() {
        let parse = |args: &[&str]| {
            super::Cli::try_parse_from(["flife-studio", "preset.toml"].iter().chain(args).copied())
        };
        let cli = parse(&["--timeline", "t.ndjson"]).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(cli.timeline.as_deref(), Some("t.ndjson".as_ref()));
        let cli = parse(&["--offscreen", "1", "--timeline", "t"]).unwrap_or_else(|e| panic!("{e}"));
        assert!(cli.timeline.is_some(), "an offscreen session records too");
        let cli = parse(&["--export", "t", "--out", "d"]).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            (cli.export.as_deref(), cli.out.as_deref(), cli.export_fps),
            (Some("t".as_ref()), Some("d".as_ref()), None)
        );
        let cli = parse(&["--export", "t", "--out", "d", "--export-fps", "60"])
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(cli.export_fps.map(std::num::NonZeroU32::get), Some(60));
        for args in [
            // An export needs its directory, and the directory its export.
            &["--export", "t"][..],
            &["--out", "d"],
            &["--export-fps", "30"],
            &["--export", "t", "--out", "d", "--export-fps", "0"],
            // An export opens no Studio: it neither records nor runs one.
            &["--export", "t", "--out", "d", "--timeline", "u"],
            &["--export", "t", "--out", "d", "--offscreen", "1"],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn frames_keep_their_schedule_through_oversleeping() {
        use std::time::{Duration, Instant};
        let period = Duration::from_secs(1) / 30;
        let work = Duration::from_millis(15);
        let base = Instant::now();
        // Every wait overshoots by 6 ms, as macOS' timer slack did (the
        // median for a 16.7 ms sleep): frames still start `period` apart.
        let (mut slot, mut start) = (base, base);
        for frame in 1..=300 {
            slot = super::next_slot(slot, period, start + work);
            assert_eq!(slot, base + period * frame, "frame {frame} keeps its slot");
            start = slot + Duration::from_millis(6);
        }
        // A frame that runs past its slot: the next starts at its end, and
        // the schedule goes on from there, one frame a period.
        let end = start + period * 3;
        let next = super::next_slot(slot, period, end);
        assert_eq!(next, end, "no frame starts before the long one ends");
        assert_eq!(super::next_slot(next, period, next + work), end + period);
    }

    fn line(status: &Status) -> Value {
        serde_json::to_value(super::status_record(status)).unwrap_or(Value::Null)
    }

    #[test]
    fn submitted_records_carry_each_strokes_payload() {
        let mut direction = [0.0; 32];
        direction[5] = -0.25;
        let submitted = |stroke| Status::Submitted {
            command: CommandId::new(7),
            stroke,
            seen: Tick::new(40),
            effective: Tick::new(41),
        };
        let material = line(&submitted(Stroke::Material {
            cell: [3, 4],
            amount: 64.0,
            direction,
        }));
        let mut expected = json!({
            "record": "submitted",
            "schema": "frankenlife.studio.v1",
            "command": 7,
            "brush": "material",
            "cell": [3, 4],
            "amount": 64.0,
            "direction": direction.to_vec(),
            "center": null,
            "radius": null,
            "count": null,
            "endowment": null,
            "seen_tick": 40,
            "effective_tick": 41
        });
        assert_eq!(material, expected);
        let removal = line(&submitted(Stroke::Removal {
            center: [1.5, 2.5],
            radius: 3.0,
        }));
        for (key, value) in [
            ("brush", json!("removal")),
            ("cell", Value::Null),
            ("amount", Value::Null),
            ("direction", Value::Null),
            ("center", json!([1.5, 2.5])),
            ("radius", json!(3.0)),
        ] {
            expected[key] = value;
        }
        assert_eq!(removal, expected);
        let introduction = line(&submitted(Stroke::Introduction {
            cell: [9, 1],
            count: 8,
            energy: 2.0,
        }));
        assert_eq!(introduction["brush"], "introduction");
        assert_eq!(introduction["cell"], json!([9, 1]));
        assert_eq!(introduction["count"], 8);
        assert_eq!(introduction["endowment"], 2.0);
        assert_eq!(introduction["amount"], Value::Null);
        let energy = line(&submitted(Stroke::Energy {
            cell: [0, 31],
            amount: 64.0,
        }));
        assert_eq!(energy["brush"], "energy");
        assert_eq!(
            (&energy["cell"], &energy["amount"]),
            (&json!([0, 31]), &json!(64.0))
        );
    }

    #[test]
    fn clock_records_carry_the_reading() {
        let reading = flife_studio::ClockReading {
            wall_seconds: 1.25,
            ticks: 160,
            ticks_per_second: 128.0,
            simulation_rate: 1.0,
            frames: 75,
            frames_per_second: 60.0,
            committed: Tick::new(1_000),
            shown: Some(Tick::new(999)),
            behind: Some(1),
            evaluations: Some(4_096),
            evaluations_per_second: Some(3_276.8),
        };
        let record = serde_json::to_value(super::clock_record(&reading)).unwrap_or(Value::Null);
        assert_eq!(
            record,
            json!({
                "record": "clock",
                "schema": "frankenlife.studio.v1",
                "wall_seconds": 1.25,
                "ticks": 160,
                "ticks_per_second": 128.0,
                "simulation_rate": 1.0,
                "frames": 75,
                "frames_per_second": 60.0,
                "committed_tick": 1000,
                "shown_tick": 999,
                "behind_ticks": 1,
                "evaluations": 4096,
                "evaluations_per_second": 3276.8
            })
        );
    }

    #[test]
    fn finished_records_reconcile_the_tick_and_energy() {
        let applied = line(&Status::Finished {
            command: CommandId::new(2),
            outcome: Outcome::Applied {
                tick: Tick::new(10),
                published: Tick::new(11),
                epoch: ResourceEpoch::new(12),
                applied: 63.5,
                cursor: 0,
            },
        });
        assert_eq!(
            applied,
            json!({
                "record": "finished",
                "schema": "frankenlife.studio.v1",
                "command": 2,
                "outcome": "applied",
                "tick": 10,
                "published_tick": 11,
                "energy": 63.5,
                "detail": null
            })
        );
        let cancelled = line(&Status::Finished {
            command: CommandId::new(3),
            outcome: Outcome::Cancelled {
                boundary: Tick::new(5),
                reason: CancelReason::SessionClosed,
                cursor: 1,
            },
        });
        assert_eq!(cancelled["outcome"], "cancelled");
        assert_eq!(cancelled["tick"], 5);
        assert_eq!(cancelled["energy"], Value::Null);
        assert_eq!(cancelled["detail"], "SessionClosed");
    }
}
