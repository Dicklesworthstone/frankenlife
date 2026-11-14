//! `flife`: FrankenLife's headless command line (FL-015; plan §14.6).
//!
//! Every line on standard output is one versioned JSON record. An error is
//! one `frankenlife.error.v1` record on standard error. Exit status: 0 when
//! the command did what it was asked, 1 when a run stopped early (a
//! capacity pause or a failed tick), 2 for an unreadable or invalid
//! configuration, a refused session or run directory.

#![forbid(unsafe_code)]

use std::io::Write;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use flife_cli::{
    BranchOptions, CheckpointMode, ColorMapping, ControllerChoice, DiscoverOptions, Discovery,
    ERROR_SCHEMA, Experiment, ExperimentOptions, FieldMapping, Injection, InspectOptions,
    Introduction, Look, Material, Particles, Persistence, Removal, ResumeOptions, RunOptions,
    ScheduledCommand, Selection, Stop, StoreDurability, Style, TransplantOptions, Validation,
};

/// `flife discover`'s one record: the [`Discovery`] its bundle's
/// `discovery.json` holds, tagged.
#[derive(Serialize)]
struct DiscoverRecord<'a> {
    record: &'static str,
    #[serde(flatten)]
    discovery: &'a Discovery,
}
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
        /// Print a metrics record (complete window, population and lineage
        /// statistics; spatial statistics sampled from the render epoch) at
        /// every multiple of this tick and at the end; 0 for none.
        #[arg(long, default_value_t = 0)]
        metrics_every: u64,
        /// Threads for the organism phases, replacing `[execution]
        /// cpu_workers`; `auto` for one per available core, up to 16. No
        /// result depends on it.
        #[arg(long, value_parser = parse_workers)]
        cpu_workers: Option<NonZeroUsize>,
        /// Where due controllers are evaluated: `metal`, `cpu` (the
        /// reference), `torch` (FrankenTorch's CPU kernel on the world's
        /// workers), or `auto` for Metal where this host has it and the
        /// world uses at most 8 CPU workers, else `torch` at most 4 (where
        /// each measured faster), else the reference. No result depends on
        /// it.
        #[arg(long, default_value = "auto")]
        controllers: ControllerChoice,
        /// Keep the run in this new (or empty) directory: its
        /// configuration, command journal and checkpoints, so `flife
        /// resume` can continue it. A `write-ahead-before-apply`
        /// configuration needs one.
        #[arg(long)]
        run_dir: Option<PathBuf>,
        #[command(flatten)]
        checkpoints: CheckpointFlags,
        /// Inject energy into a field cell at a committed tick, written
        /// `TICK:X,Y:AMOUNT`; repeatable. Commands 1, 2, … in order.
        #[arg(long = "inject", value_name = "TICK:X,Y:AMOUNT")]
        injections: Vec<Injection>,
        /// Inject energy with a signed chemical composition into a field
        /// cell at a committed tick (the material brush), written
        /// `TICK:X,Y:AMOUNT:K=V,…` with the direction's nonzero channels,
        /// its norm at most 1; repeatable. Numbered after the `--inject`
        /// commands, in order.
        #[arg(long = "inject-material", value_name = "TICK:X,Y:AMOUNT:K=V,...")]
        materials: Vec<Material>,
        /// Remove every organism within RADIUS world units of (X, Y) at a
        /// committed tick, its energy removed from the world, dissipated or
        /// recycled into the field, written
        /// `TICK:X,Y:RADIUS:DISPOSITION`; repeatable. Numbered after the
        /// `--inject` and `--inject-material` commands, in order.
        #[arg(long = "remove", value_name = "TICK:X,Y:RADIUS:DISPOSITION")]
        removals: Vec<Removal>,
        /// Introduce COUNT new organisms, each endowed with ENERGY, into a
        /// field cell at the end of a committed tick (seeded introduction:
        /// founder-drawn genomes, new lineages; all or none under the
        /// population cap), written `TICK:X,Y:COUNT:ENERGY`; repeatable.
        /// Numbered after the `--inject`, `--inject-material` and `--remove`
        /// commands, in order.
        #[arg(long = "introduce", value_name = "TICK:X,Y:COUNT:ENERGY")]
        introductions: Vec<Introduction>,
    },
    /// Continue the run in a run directory from its newest checkpoint that
    /// restores, replaying its command journal, to a committed tick.
    Resume {
        /// The run directory `flife run --run-dir` made.
        run_dir: PathBuf,
        /// The committed tick to reach.
        #[arg(long)]
        until: u64,
        /// Print a progress record at every multiple of this tick; 0 for
        /// none.
        #[arg(long, default_value_t = 128)]
        every: u64,
        /// Print a metrics record (complete window, population and lineage
        /// statistics; spatial statistics sampled from the render epoch) at
        /// every multiple of this tick and at the end; 0 for none.
        #[arg(long, default_value_t = 0)]
        metrics_every: u64,
        /// Threads for the organism phases: a count or `auto`. No result
        /// depends on it.
        #[arg(long, value_parser = parse_workers)]
        cpu_workers: Option<NonZeroUsize>,
        /// Where due controllers are evaluated: `metal`, `cpu`, `torch` or
        /// `auto`.
        /// No result depends on it.
        #[arg(long, default_value = "auto")]
        controllers: ControllerChoice,
        #[command(flatten)]
        checkpoints: CheckpointFlags,
    },
    /// Branch a run directory at one of its checkpoints into a new run
    /// directory: a control branch, or with `--inject`, `--inject-material`,
    /// `--remove` or `--introduce` an intervention branch. Continue it with `flife resume`.
    Branch {
        /// The run directory to branch.
        parent: PathBuf,
        /// The new (or empty) run directory.
        run_dir: PathBuf,
        /// The committed tick of the checkpoint to branch from; the newest
        /// that restores if omitted.
        #[arg(long)]
        at: Option<u64>,
        /// The branch's name, part of its identity; the new directory's
        /// name if omitted.
        #[arg(long)]
        name: Option<String>,
        /// The branch's own command: energy into a field cell at a
        /// committed tick at or after the checkpoint's, written
        /// `TICK:X,Y:AMOUNT`; repeatable.
        #[arg(long = "inject", value_name = "TICK:X,Y:AMOUNT")]
        injections: Vec<Injection>,
        /// The branch's own command: energy with a signed chemical
        /// composition into a field cell at a committed tick at or after the
        /// checkpoint's, written `TICK:X,Y:AMOUNT:K=V,…`; repeatable,
        /// numbered after the `--inject` commands.
        #[arg(long = "inject-material", value_name = "TICK:X,Y:AMOUNT:K=V,...")]
        materials: Vec<Material>,
        /// The branch's own command: every organism within RADIUS world
        /// units of (X, Y) removed at a committed tick at or after the
        /// checkpoint's, written `TICK:X,Y:RADIUS:DISPOSITION` (`remove`,
        /// `dissipate` or `recycle`); repeatable, numbered after the
        /// `--inject` and `--inject-material` commands.
        #[arg(long = "remove", value_name = "TICK:X,Y:RADIUS:DISPOSITION")]
        removals: Vec<Removal>,
        /// The branch's own command: COUNT new organisms, each endowed with
        /// ENERGY, introduced into a field cell at the end of a committed
        /// tick at or after the checkpoint's, written
        /// `TICK:X,Y:COUNT:ENERGY`; repeatable, numbered after the
        /// `--inject`, `--inject-material` and `--remove` commands.
        #[arg(long = "introduce", value_name = "TICK:X,Y:COUNT:ENERGY")]
        introductions: Vec<Introduction>,
        /// `power-loss` (the default) flushes the founding checkpoint to
        /// stable storage before publishing it; `process-crash` relies on
        /// atomic renames.
        #[arg(long, default_value = "power-loss", value_parser = parse_durability)]
        checkpoint_durability: StoreDurability,
    },
    /// Run an experiment specification's conditions over its discovery
    /// and held-out seeds into an experiment directory, cataloging every
    /// run, and write its report. Continues the experiment the directory
    /// already holds.
    Experiment {
        /// The experiment specification, as TOML.
        spec: PathBuf,
        /// The experiment directory: new, empty, or holding this
        /// experiment.
        #[arg(long)]
        out: PathBuf,
        /// Threads for each run's organism phases: a count or `auto`. No
        /// result depends on it.
        #[arg(long, value_parser = parse_workers)]
        cpu_workers: Option<NonZeroUsize>,
        /// Where due controllers are evaluated: `metal`, `cpu`, `torch` or
        /// `auto`.
        /// No result depends on it.
        #[arg(long, default_value = "auto")]
        controllers: ControllerChoice,
    },
    /// Print a run directory's committed state at one of its checkpoints
    /// as NDJSON: a header, every live organism in identity order, with
    /// `--field` the field's energy plane, and with `--weights` an
    /// organism's whole controller.
    Inspect {
        /// The run directory.
        run_dir: PathBuf,
        /// The committed tick of the checkpoint; the newest that restores
        /// if omitted.
        #[arg(long)]
        at: Option<u64>,
        /// Also print the field's energy plane, row-major.
        #[arg(long)]
        field: bool,
        /// Also print this organism's whole controller: each layer's
        /// weights and biases as stored. Repeatable; printed in the order
        /// given.
        #[arg(long = "weights", value_name = "ORGANISM")]
        weights: Vec<u64>,
    },
    /// Save a discovery (plan §14.5) of a run directory's checkpoint into a
    /// new bundle directory: `discovery.json` (source reference,
    /// selection, presentation, description), `thumbnail.png`, the
    /// full-world export `world/`, a run directory `flife resume`
    /// continues, and for a region the local transplant package `package/`,
    /// which `flife transplant` grafts. The run's store pins the checkpoint
    /// against retention.
    Discover {
        /// The run directory.
        run_dir: PathBuf,
        /// The new (or empty) bundle directory.
        out: PathBuf,
        /// The committed tick of the checkpoint; the newest that restores
        /// if omitted.
        #[arg(long)]
        at: Option<u64>,
        /// The discovery's name, part of its identity; the bundle
        /// directory's name if omitted.
        #[arg(long)]
        name: Option<String>,
        /// What the discovery is.
        #[arg(long, default_value = "")]
        description: String,
        /// Select the organisms and field cells within RADIUS world units
        /// of (X, Y); the thumbnail frames them.
        #[arg(long, value_name = "X,Y,RADIUS", conflicts_with = "lineage")]
        region: Option<flife_cli::Disk>,
        /// Select one founder lineage's live organisms.
        #[arg(long, value_name = "LINEAGE")]
        lineage: Option<u64>,
        /// Thumbnail width in pixels.
        #[arg(long, default_value_t = 640)]
        width: u32,
        /// Thumbnail height in pixels.
        #[arg(long, default_value_t = 360)]
        height: u32,
        /// Leave the full-world export out.
        #[arg(long)]
        no_world: bool,
        /// Leave a region's local transplant package out.
        #[arg(long)]
        no_package: bool,
        /// Threads for FrankenManim's CPU renderer: a count or `auto`.
        #[arg(long, default_value = "auto", value_parser = parse_workers)]
        render_threads: NonZeroUsize,
        /// What the organisms' colour means, as for `flife render`; the
        /// default is the configuration's `[presentation] color_mapping`.
        #[arg(long)]
        color: Option<ColorMapping>,
        /// `glow` (the default) or `raw`.
        #[arg(long, default_value = "glow")]
        look: Look,
        /// Draw organism density per screen bin instead of dots.
        #[arg(long)]
        density: bool,
        /// Leave the energy field out.
        #[arg(long)]
        no_field: bool,
        /// What the field's colour means, as for `flife render`.
        #[arg(long, default_value = "energy")]
        field_mapping: FieldMapping,
        /// `power-loss` (the default) flushes the export, the pin and the
        /// bundle's files to stable storage; `process-crash` relies on
        /// atomic renames.
        #[arg(long, default_value = "power-loss", value_parser = parse_durability)]
        checkpoint_durability: StoreDurability,
    },
    /// Graft a discovery's local transplant package into a host world as it
    /// is founded, into a new run directory whose founding checkpoint is the
    /// grafted world. Continue it with `flife resume`.
    Transplant {
        /// The package directory (a discovery bundle's `package/`).
        package: PathBuf,
        /// The new (or empty) run directory.
        run_dir: PathBuf,
        /// The host's configuration or preset, as TOML: the run's.
        #[arg(long)]
        config: PathBuf,
        /// Where the package's disk centre goes in the host, in world
        /// units; snapped to the nearest whole-cell move.
        #[arg(long, value_name = "X,Y", value_parser = parse_position)]
        at: [f64; 2],
        /// The transplant's name, part of its identity; the new directory's
        /// name if omitted.
        #[arg(long)]
        name: Option<String>,
        /// `power-loss` (the default) flushes the founding checkpoint and
        /// `transplant.json` to stable storage; `process-crash` relies on
        /// atomic renames.
        #[arg(long, default_value = "power-loss", value_parser = parse_durability)]
        checkpoint_durability: StoreDurability,
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
        /// Where due controllers are evaluated: `metal`, `cpu`, `torch`, or
        /// `auto` (Metal at most 8 CPU workers, else `torch` at most 4). No
        /// result depends on it.
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
        /// `generation`, `motion`, `topology` or `affinity` (or a registered
        /// name such as `founder-lineage-v1`). The default is the
        /// configuration's `[presentation] color_mapping`.
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
        /// What the field's colour means: `energy` (the default),
        /// `composition`, its mean chemical composition coloured as the
        /// affinity mapping colours organisms, or `component-C`, moment
        /// channel C (0 to 31) per unit energy from blue (-1) to red (+1);
        /// the last two as bright as its energy.
        #[arg(long, default_value = "energy")]
        field_mapping: FieldMapping,
    },
}

#[derive(Debug, clap::Args)]
struct CheckpointFlags {
    /// With a run directory, publish a checkpoint at every multiple of
    /// this tick and at the end; 0 for only the end. The default is the
    /// configuration's `[persistence]
    /// checkpoint_interval_simulation_seconds`.
    #[arg(long)]
    checkpoint_every: Option<u64>,
    /// `power-loss` (the default) flushes each checkpoint to stable storage
    /// before publishing it; `process-crash` relies on atomic renames.
    #[arg(long, default_value = "power-loss", value_parser = parse_durability)]
    checkpoint_durability: StoreDurability,
    /// `pause-and-stream` waits while each checkpoint streams into the
    /// store a page at a time; `bounded-background` steps on while a
    /// writer drains at most `--checkpoint-retained-bytes` of its pages.
    /// The default is the configuration's `[persistence] checkpoint_mode`.
    /// No result depends on it.
    #[arg(long, value_parser = parse_mode)]
    checkpoint_mode: Option<CheckpointMode>,
    /// The most page bytes a `bounded-background` checkpoint retains. The
    /// default is the configuration's `[execution]
    /// checkpoint_retained_bytes`.
    #[arg(long)]
    checkpoint_retained_bytes: Option<u64>,
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
            field_mapping,
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
                    field_mapping,
                    ..Style::default()
                },
            ),
        ),
        Command::Run {
            config,
            ticks,
            every,
            metrics_every,
            cpu_workers,
            controllers,
            run_dir,
            checkpoints,
            injections,
            materials,
            removals,
            introductions,
        } => {
            let config = match load(&config) {
                Ok(config) => config,
                Err(code) => return code,
            };
            let persistence = run_dir.map(|run_dir| {
                let defaults = Persistence::new(run_dir, &config);
                Persistence {
                    checkpoint_every: checkpoints
                        .checkpoint_every
                        .unwrap_or(defaults.checkpoint_every),
                    durability: checkpoints.checkpoint_durability,
                    mode: checkpoints.checkpoint_mode.unwrap_or(defaults.mode),
                    retained_bytes: checkpoints
                        .checkpoint_retained_bytes
                        .unwrap_or(defaults.retained_bytes),
                    ..defaults
                }
            });
            exit(flife_cli::run_with(
                &config,
                RunOptions {
                    ticks,
                    every,
                    cpu_workers,
                    controllers,
                    metrics_every,
                },
                &ScheduledCommand::listed(&injections, &materials, &removals, &introductions),
                persistence.as_ref(),
                print,
            ))
        }
        Command::Branch {
            parent,
            run_dir,
            at,
            name,
            injections,
            materials,
            removals,
            introductions,
            checkpoint_durability,
        } => match flife_cli::branch(
            &parent,
            &run_dir,
            &BranchOptions {
                at,
                name,
                commands: ScheduledCommand::listed(
                    &injections,
                    &materials,
                    &removals,
                    &introductions,
                ),
                durability: checkpoint_durability,
            },
        ) {
            Ok(record) => {
                print(&record);
                ExitCode::SUCCESS
            }
            Err(error) => fail(error.kind(), error.to_string()),
        },
        Command::Discover {
            run_dir,
            out,
            at,
            name,
            description,
            region,
            lineage,
            width,
            height,
            no_world,
            no_package,
            render_threads,
            color,
            look,
            density,
            no_field,
            field_mapping,
            checkpoint_durability,
        } => {
            let config = match flife_cli::persist::read_config(&run_dir) {
                Ok(config) => config,
                Err(error) => return fail(error.kind(), error.to_string()),
            };
            let mapping = match color.map_or_else(|| ColorMapping::configured(&config), Ok) {
                Ok(mapping) => mapping,
                Err(error) => return fail(error.kind(), error.to_string()),
            };
            let selection = match (region, lineage) {
                (Some(disk), _) => Selection::Region(disk),
                (None, Some(lineage)) => Selection::Lineage(lineage),
                (None, None) => Selection::World,
            };
            let options = DiscoverOptions {
                at,
                name,
                description,
                selection,
                size: [width, height],
                style: Style {
                    mapping,
                    look,
                    particles: if density {
                        Particles::Density
                    } else {
                        Particles::Points
                    },
                    field: !no_field,
                    field_mapping,
                },
                render_threads,
                full_world: !no_world,
                package: !no_package,
                durability: checkpoint_durability,
            };
            match flife_cli::discover(&run_dir, &out, &options) {
                Ok(discovery) => {
                    print(&DiscoverRecord {
                        record: "discovery",
                        discovery: &discovery,
                    });
                    ExitCode::SUCCESS
                }
                Err(error) => fail(error.kind(), error.to_string()),
            }
        }
        Command::Transplant {
            package,
            run_dir,
            config,
            at,
            name,
            checkpoint_durability,
        } => {
            let host = match load(&config) {
                Ok(host) => host,
                Err(code) => return code,
            };
            match flife_cli::transplant(
                &package,
                &host,
                &run_dir,
                &TransplantOptions {
                    at,
                    name,
                    durability: checkpoint_durability,
                },
            ) {
                Ok(record) => {
                    print(&record);
                    ExitCode::SUCCESS
                }
                Err(error) => fail(error.kind(), error.to_string()),
            }
        }
        Command::Inspect {
            run_dir,
            at,
            field,
            weights,
        } => match flife_cli::inspect(&run_dir, InspectOptions { at, field, weights }) {
            Ok(inspection) => {
                print(&inspection.inspected);
                for organism in &inspection.organisms {
                    print(organism);
                }
                if let Some(field) = &inspection.field {
                    print(field);
                }
                for controller in &inspection.weights {
                    print(controller);
                }
                ExitCode::SUCCESS
            }
            Err(error) => fail(error.kind(), error.to_string()),
        },
        Command::Experiment {
            spec,
            out,
            cpu_workers,
            controllers,
        } => {
            let experiment = match Experiment::load(&spec) {
                Ok(experiment) => experiment,
                Err(error) => return fail(error.kind(), error.to_string()),
            };
            match flife_cli::experiment(
                &experiment,
                &out,
                ExperimentOptions {
                    cpu_workers,
                    controllers,
                },
                print,
            ) {
                Ok(_) => ExitCode::SUCCESS,
                Err(error) => fail(error.kind(), error.to_string()),
            }
        }
        Command::Resume {
            run_dir,
            until,
            every,
            metrics_every,
            cpu_workers,
            controllers,
            checkpoints,
        } => exit(flife_cli::resume(
            &run_dir,
            &ResumeOptions {
                until,
                every,
                cpu_workers,
                controllers,
                checkpoint_every: checkpoints.checkpoint_every,
                durability: checkpoints.checkpoint_durability,
                checkpoint_mode: checkpoints.checkpoint_mode,
                retained_bytes: checkpoints.checkpoint_retained_bytes,
                metrics_every,
            },
            print,
        )),
    }
}

/// 0 for a run that reached its tick, 1 for one that stopped early, 2 (and
/// an error record) for one refused.
fn exit(finished: Result<flife_cli::Finished, flife_core::CoreError>) -> ExitCode {
    match finished {
        Ok(finished) if finished.stop == Stop::Reached => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(1),
        Err(error) => fail(error.kind(), error.to_string()),
    }
}

/// A checkpoint durability profile by name.
fn parse_mode(text: &str) -> Result<CheckpointMode, String> {
    [
        CheckpointMode::PauseAndStream,
        CheckpointMode::BoundedBackground,
    ]
    .into_iter()
    .find(|mode| mode.name() == text)
    .ok_or_else(|| format!("`{text}` is not `pause-and-stream` or `bounded-background`"))
}

/// A world position, `X,Y`.
fn parse_position(text: &str) -> Result<[f64; 2], String> {
    let parts: Vec<&str> = text.split(',').map(str::trim).collect();
    let [x, y] = parts.as_slice() else {
        return Err(format!("`{text}` is not X,Y"));
    };
    let value = |part: &str| {
        part.parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .ok_or_else(|| format!("`{part}` in `{text}` is not a finite number"))
    };
    Ok([value(x)?, value(y)?])
}

fn parse_durability(text: &str) -> Result<StoreDurability, String> {
    [StoreDurability::PowerLoss, StoreDurability::ProcessCrash]
        .into_iter()
        .find(|durability| durability.name() == text)
        .ok_or_else(|| format!("`{text}` is not `power-loss` or `process-crash`"))
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
