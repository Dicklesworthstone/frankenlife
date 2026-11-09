//! `flife`: FrankenLife's headless command line (FL-015; plan §14.6).
//!
//! Every line on standard output is one versioned JSON record. An error is
//! one `frankenlife.error.v1` record on standard error. Exit status: 0 when
//! the command did what it was asked, 1 when a run stopped early (a
//! capacity pause or a failed tick), 2 for an unreadable or invalid
//! configuration or a refused session.

#![forbid(unsafe_code)]

use std::io::Write;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use flife_cli::{
    ColorMapping, ControllerChoice, ERROR_SCHEMA, Look, Particles, RunOptions, Stop, Style,
    Validation,
};
use flife_core::ValidatedConfig;
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(name = "flife", version, about = "FrankenLife's headless command line")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate a configuration and print its canonical summary.
    Validate {
        /// The configuration or preset, as TOML.
        config: PathBuf,
    },
    /// Run a configuration headlessly through the session owner, printing
    /// NDJSON records.
    Run {
        /// The configuration or preset, as TOML.
        config: PathBuf,
        /// Ticks to publish.
        #[arg(long, default_value_t = 1024)]
        ticks: u64,
        /// Print a progress record every this many ticks; 0 for none.
        #[arg(long, default_value_t = 128)]
        every: u64,
        /// Threads for the organism phases, replacing `[execution]
        /// cpu_workers`; `auto` for one per available core, up to 16. No
        /// result depends on it.
        #[arg(long, value_parser = parse_workers)]
        cpu_workers: Option<NonZeroUsize>,
        /// Where due controllers are evaluated: `metal`, `cpu`, or `auto`
        /// for Metal where this host has it and the world uses at most 8
        /// CPU workers (where it measured faster). No result depends on it.
        #[arg(long, default_value = "auto")]
        controllers: ControllerChoice,
    },
    /// Run a configuration headlessly and render it to PNG frames (and
    /// optionally an MP4) through the Studio's FrankenManim view.
    Render {
        /// The configuration or preset, as TOML.
        config: PathBuf,
        /// Ticks to run after the founding frame.
        #[arg(long, default_value_t = 1280)]
        ticks: u64,
        /// Draw a frame every this many ticks.
        #[arg(long, default_value_t = 4)]
        every: u64,
        /// Frame width in pixels.
        #[arg(long, default_value_t = 1280)]
        width: u32,
        /// Frame height in pixels.
        #[arg(long, default_value_t = 1280)]
        height: u32,
        /// The directory for the PNG frames.
        #[arg(long)]
        out: PathBuf,
        /// Threads for the world: a count or `auto`.
        #[arg(long, default_value = "auto", value_parser = parse_workers)]
        cpu_workers: NonZeroUsize,
        /// Where due controllers are evaluated: `metal`, `cpu`, or `auto`
        /// (Metal at most 8 CPU workers). No result depends on it.
        #[arg(long, default_value = "auto")]
        controllers: ControllerChoice,
        /// Threads for FrankenManim's CPU renderer: a count or `auto`.
        #[arg(long, default_value = "auto", value_parser = parse_workers)]
        render_threads: NonZeroUsize,
        /// Encode the frames into this MP4 with ffmpeg.
        #[arg(long)]
        video: Option<PathBuf>,
        /// The video's frames per second.
        #[arg(long, default_value_t = 30)]
        fps: u32,
        /// What the organisms' colour means: `lineage`, `energy`,
        /// `generation`, `motion` or `topology` (or a registered name such
        /// as `founder-lineage-v1`). The default is the configuration's
        /// `[presentation] color_mapping`.
        #[arg(long)]
        color: Option<ColorMapping>,
        /// `glow` (the default) or `raw`: no halos and a linear grey field.
        #[arg(long, default_value = "glow")]
        look: Look,
        /// Draw organism density per screen bin instead of dots.
        #[arg(long)]
        density: bool,
        /// Leave the energy field out.
        #[arg(long)]
        no_field: bool,
    },
}

#[derive(Serialize)]
struct ErrorRecord<'a> {
    schema: &'static str,
    kind: &'a str,
    detail: String,
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Validate { config } => match load(&config) {
            Ok(config) => {
                print(&Validation::of(&config));
                ExitCode::SUCCESS
            }
            Err(code) => code,
        },
        Command::Render {
            config,
            ticks,
            every,
            width,
            height,
            out,
            cpu_workers,
            controllers,
            render_threads,
            video,
            fps,
            color,
            look,
            density,
            no_field,
        } => render(
            &config,
            ticks,
            every,
            [width, height],
            out,
            (cpu_workers, controllers),
            render_threads,
            video,
            fps,
            (
                color,
                Style {
                    look,
                    particles: if density {
                        Particles::Density
                    } else {
                        Particles::Points
                    },
                    field: !no_field,
                    ..Style::default()
                },
            ),
        ),
        Command::Run {
            config,
            ticks,
            every,
            cpu_workers,
            controllers,
        } => {
            let config = match load(&config) {
                Ok(config) => config,
                Err(code) => return code,
            };
            match flife_cli::run(
                &config,
                RunOptions {
                    ticks,
                    every,
                    cpu_workers,
                    controllers,
                },
                print,
            ) {
                Ok(finished) if finished.stop == Stop::Reached => ExitCode::SUCCESS,
                Ok(_) => ExitCode::from(1),
                Err(error) => fail(error.kind(), error.to_string()),
            }
        }
    }
}

/// A worker count, or `auto`.
fn parse_workers(text: &str) -> Result<NonZeroUsize, String> {
    if text == "auto" {
        return Ok(flife_cli::auto_workers());
    }
    text.parse()
        .map_err(|_| format!("`{text}` is not a positive worker count or `auto`"))
}

#[expect(clippy::too_many_arguments, reason = "one argument per flag")]
fn render(
    config: &PathBuf,
    ticks: u64,
    every: u64,
    size: [u32; 2],
    out: PathBuf,
    (cpu_workers, controllers): (NonZeroUsize, ControllerChoice),
    render_threads: NonZeroUsize,
    video: Option<PathBuf>,
    fps: u32,
    (color, style): (Option<ColorMapping>, Style),
) -> ExitCode {
    let config = match load(config) {
        Ok(config) => config,
        Err(code) => return code,
    };
    let style = match color.map_or_else(|| ColorMapping::configured(&config), Ok) {
        Ok(mapping) => Style { mapping, ..style },
        Err(error) => return fail(error.kind(), error.to_string()),
    };
    let options = flife_cli::RenderOptions {
        ticks,
        every,
        size,
        out,
        cpu_workers: Some(cpu_workers),
        controllers,
        render_threads,
        video,
        fps,
        style,
    };
    match flife_cli::render(&config, &options, print) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            fail(error.kind(), error.to_string());
            ExitCode::from(1)
        }
    }
}

fn load(path: &PathBuf) -> Result<ValidatedConfig, ExitCode> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| fail("io", format!("{}: {error}", path.display())))?;
    ValidatedConfig::from_toml(&text).map_err(|error| fail(error.kind(), error.to_string()))
}

fn print(record: &impl Serialize) {
    match serde_json::to_string(record) {
        Ok(line) => {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
        }
        Err(error) => {
            fail("serialization", error.to_string());
        }
    }
}

fn fail(kind: &str, detail: String) -> ExitCode {
    let record = ErrorRecord {
        schema: ERROR_SCHEMA,
        kind,
        detail,
    };
    if let Ok(line) = serde_json::to_string(&record) {
        let _ = writeln!(std::io::stderr().lock(), "{line}");
    }
    ExitCode::from(2)
}
