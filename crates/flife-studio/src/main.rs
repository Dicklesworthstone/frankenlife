//! `flife-studio`: the live world in a native window (FL-018; plan §13.6).
//!
//! Every line on standard output is one `frankenlife.studio.v1` record: an
//! `open`, then a `status` per session status, a `view` per style change,
//! a `selection` each time the selected organism changes (a click, or the
//! selected organism gone from a newer epoch) and a `frame` summary about
//! once a second, then an `end` with the committed world's digest. An error
//! is one `frankenlife.error.v1` record on standard error. Exit status: 0
//! after a clean close, 2 for an unreadable or invalid configuration or a
//! window that cannot open, 1 for any later failure.

#![forbid(unsafe_code)]

use std::io::Write;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::Parser;
use flife_backend_metal::ControllerChoice;
use flife_core::ValidatedConfig;
use flife_studio::{
    ColorMapping, End, Look, Particles, STUDIO_SCHEMA, Status, Studio, StudioOptions, Style,
};
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
    /// `motion` or `topology` (or a registered name such as
    /// `founder-lineage-v1`). The default is the configuration's
    /// `[presentation] color_mapping`. Keys 1 to 5 change it.
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
        mapping: &'static str,
        meaning: &'static str,
        look: &'static str,
        particles: &'static str,
        field: bool,
    },
    View {
        schema: &'static str,
        mapping: &'static str,
        meaning: &'static str,
        look: &'static str,
        particles: &'static str,
        field: bool,
        shown_tick: Option<u64>,
    },
    Selection {
        schema: &'static str,
        organism: Option<u64>,
        position: Option<[f64; 2]>,
        shown_tick: Option<u64>,
    },
    Status {
        schema: &'static str,
        status: &'static str,
        run: Option<u64>,
        tick: Option<u64>,
        why: Option<&'static str>,
        detail: Option<String>,
    },
    Frame {
        schema: &'static str,
        frames: u64,
        presented: u64,
        occluded: u64,
        epochs: u64,
        shown_tick: Option<u64>,
        hover: Option<[f64; 2]>,
        center: [f64; 2],
        zoom: f64,
        input_dropped: u64,
    },
    End {
        schema: &'static str,
        tick: u64,
        digest: String,
        frames: u64,
        presented: u64,
        epochs: u64,
        refits: u64,
        intents: u64,
        refused: u64,
        keys: u64,
        pointer_moves: u64,
        resizes: u64,
        input_dropped: u64,
        controller_evaluations: u64,
        deferred_evaluations: u64,
        controller_failure: Option<String>,
        wall_seconds: f64,
    },
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
        },
        ..StudioOptions::default()
    };
    let clock = Instant::now();
    let mut studio = match Studio::open(&config, &options) {
        Ok(studio) => studio,
        Err(error) => return fail(error.kind(), error.to_string(), 2),
    };
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
        mapping: options.style.mapping.name(),
        meaning: options.style.mapping.meaning(),
        look: options.style.look.name(),
        particles: options.style.particles.name(),
        field: options.style.field,
    });

    let period = Duration::from_secs(1) / fps;
    let mut summary = Instant::now();
    let mut selected = None;
    while !studio.should_close() {
        let started = Instant::now();
        let frame = match studio.frame() {
            Ok(frame) => frame,
            Err(error) => {
                let code = fail(error.kind(), error.to_string(), 1);
                let _ = studio.close();
                return code;
            }
        };
        for status in studio.take_statuses() {
            print(&status_record(&status));
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
                shown_tick: frame.shown.map(|shown| shown.tick.get()),
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
            let tally = studio.tally();
            let viewport = studio.view().viewport();
            print(&Record::Frame {
                schema: STUDIO_SCHEMA,
                frames: tally.frames,
                presented: tally.presented,
                occluded: tally.occluded,
                epochs: tally.epochs,
                shown_tick: frame.shown.map(|shown| shown.tick.get()),
                hover: frame.hover,
                center: viewport.center(),
                zoom: viewport.zoom(),
                input_dropped: studio.input_dropped(),
            });
        }
        if let Some(rest) = period.checked_sub(started.elapsed()) {
            std::thread::sleep(rest);
        }
    }

    match studio.close() {
        Ok(closed) => {
            print(&Record::End {
                schema: STUDIO_SCHEMA,
                tick: closed.session.boundary.tick.get(),
                digest: closed.session.world.digest().to_hex(),
                frames: closed.frames.frames,
                presented: closed.frames.presented,
                epochs: closed.frames.epochs,
                refits: closed.frames.refits,
                intents: closed.frames.intents,
                refused: closed.frames.refused,
                keys: closed.input.keys,
                pointer_moves: closed.input.pointer_moves,
                resizes: closed.input.resizes,
                input_dropped: closed.input_dropped,
                controller_evaluations: closed.session.world.controller_report().batched,
                deferred_evaluations: closed.session.world.controller_report().deferred,
                controller_failure: closed.session.world.controller_report().failure.clone(),
                wall_seconds: clock.elapsed().as_secs_f64(),
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
