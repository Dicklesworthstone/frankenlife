//! `flife`: FrankenLife's headless command line (FL-015; plan §14.6).
//!
//! Every line on standard output is one versioned JSON record. An error is
//! one `frankenlife.error.v1` record on standard error. Exit status: 0 when
//! the command did what it was asked, 1 when a run stopped early (a
//! capacity pause or a failed tick), 2 for an unreadable or invalid
//! configuration, a refused session or run directory.

#![forbid(unsafe_code)]

use std::io::Write;
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory as _, FromArgMatches as _, Parser, Subcommand};
use flife_cli::{
    BranchOptions, CheckpointMode, ColorMapping, ControllerChoice, DiscoverOptions, Discovery,
    ERROR_SCHEMA, Experiment, ExperimentOptions, FieldMapping, Injection, InspectOptions,
    Introduction, Look, Material, Particles, Persistence, Removal, ResumeOptions, RunOptions,
    ScheduledCommand, Selection, Stop, StoreDurability, Style, TransplantOptions, Until,
    Validation,
};

/// `flife discover`'s one record: the [`Discovery`] its bundle's
/// `discovery.json` holds, tagged.
#[derive(Serialize)]
struct DiscoverRecord<'a> {
    record: &'static str,
    #[serde(flatten)]
    discovery: &'a Discovery,
}
use flife_core::{FeatureOverride, ValidatedConfig};
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(
    name = "flife",
    version,
    about = "FrankenLife's headless command line",
    arg_required_else_help = true
)]
struct Cli {
    /// Print the canonical registry of all 21 `field-ecology-v2` feature
    /// toggles with bundle defaults, `INV-08` status, and ABI impact.
    #[arg(long)]
    list_features: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

/// Founding-only `field-ecology-v2` feature toggles (V2-01, INV-14).
/// Values are extracted via `ArgMatches` in [`ordered_overrides`] to preserve
/// exact left-to-right CLI ordering across interleaved `--disable`, `--enable`,
/// and `--v1-compat` flags.
#[derive(Debug, clap::Args)]
#[expect(
    dead_code,
    reason = "values read via ordered_overrides(ArgMatches) to preserve exact CLI flag order"
)]
struct FeatureFlags {
    /// Disable one or more `field-ecology-v2` feature slugs or aliases
    /// (`all-v2`, `bundle-a`, `bundle-b`), comma-separated or repeatable.
    #[arg(long = "disable", value_name = "SLUGS")]
    disable: Vec<String>,
    /// Enable one or more `field-ecology-v2` feature slugs or aliases
    /// (`all-v2`, `bundle-a`, `bundle-b`), comma-separated or repeatable.
    #[arg(long = "enable", value_name = "SLUGS")]
    enable: Vec<String>,
    /// Disable every `field-ecology-v2` feature (`--disable all-v2` alias)
    /// for exact v1 compatibility verification.
    #[arg(long = "v1-compat", action = clap::ArgAction::Count)]
    v1_compat: u8,
}

/// Hidden toggle traps on resuming commands (`resume`, `step`) so passing
/// `--disable`/`--enable`/`--v1-compat` returns a structured
/// `frankenlife.error.v1` refusal citing `INV-14` instead of silently ignoring
/// or mutating checkpoint provenance.
#[derive(Debug, clap::Args)]
struct RefusedFeatureFlags {
    #[arg(long = "disable", value_name = "SLUGS", hide = true)]
    disable: Vec<String>,
    #[arg(long = "enable", value_name = "SLUGS", hide = true)]
    enable: Vec<String>,
    #[arg(long = "v1-compat", action = clap::ArgAction::Count, hide = true)]
    v1_compat: u8,
}

impl RefusedFeatureFlags {
    fn any(&self) -> bool {
        !self.disable.is_empty() || !self.enable.is_empty() || self.v1_compat > 0
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Validate a configuration and print its canonical summary.
    Validate {
        /// The configuration or preset, as TOML.
        config: PathBuf,
        #[command(flatten)]
        features: FeatureFlags,
    },
    /// Report what this machine and build can run. Every controller route
    /// (`cpu`, `torch`, `metal`) runs a world and is compared with the
    /// reference, and the host, the memory admission and video encoding are
    /// reported, each with its reason where it is unavailable or refused.
    /// Exit 1 when a route disagrees with the reference or fails; 2 when
    /// the reference itself is refused.
    Capabilities {
        /// A configuration to run instead of the compiled-in reference
        /// preset, whose world after 8 ticks is pinned.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Ticks each route runs.
        #[arg(long, default_value_t = flife_cli::PROBE_TICKS,
              value_parser = clap::value_parser!(u64).range(1..))]
        ticks: u64,
        /// Threads for the world, replacing `[execution] cpu_workers`: a
        /// count or `auto`. No result depends on it; `auto`'s route does.
        #[arg(long, value_parser = parse_workers)]
        cpu_workers: Option<NonZeroUsize>,
        #[command(flatten)]
        features: FeatureFlags,
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
        /// every multiple of this tick and at the end; 0 for none. The
        /// configuration's `[persistence] metrics_interval_ticks` is not read.
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
        /// world uses at most 8 CPU workers, else `torch` at most 4 or, on
        /// Apple silicon, for a world of at least 25,000 organisms (where
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
        #[command(flatten)]
        features: FeatureFlags,
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
        /// every multiple of this tick and at the end; 0 for none. The
        /// configuration's `[persistence] metrics_interval_ticks` is not read.
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
        #[command(flatten)]
        refused_features: RefusedFeatureFlags,
    },
    /// Advance the run in a run directory TICKS committed ticks past its
    /// newest durable one: `flife resume` from its newest checkpoint that
    /// restores, its command journal replayed, to that tick plus TICKS. A
    /// progress record reports each committed tick (`--every 1`), and the
    /// end record whether the last one committed. Exit 1 when a tick did
    /// not commit (a capacity pause or a failed tick).
    Step {
        /// The run directory `flife run --run-dir` made.
        run_dir: PathBuf,
        /// Committed ticks to advance; at least 1.
        #[arg(long, default_value = "1")]
        ticks: NonZeroU64,
        /// Print a progress record at every multiple of this tick; 0 for
        /// none.
        #[arg(long, default_value_t = 1)]
        every: u64,
        /// Print a metrics record (complete window, population and lineage
        /// statistics; spatial statistics sampled from the render epoch) at
        /// every multiple of this tick and at the end; 0 for none. `[persistence] metrics_interval_ticks` is not read.
        #[arg(long, default_value_t = 0)]
        metrics_every: u64,
        /// Threads for the organism phases: a count or `auto`. No result
        /// depends on it.
        #[arg(long, value_parser = parse_workers)]
        cpu_workers: Option<NonZeroUsize>,
        /// Where due controllers are evaluated: `metal`, `cpu`, `torch` or
        /// `auto`. No result depends on it.
        #[arg(long, default_value = "auto")]
        controllers: ControllerChoice,
        #[command(flatten)]
        checkpoints: CheckpointFlags,
        #[command(flatten)]
        refused_features: RefusedFeatureFlags,
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
        #[command(flatten)]
        features: FeatureFlags,
    },
    /// Analyze experiment directories together, read-only: each condition's
    /// paired differences from the control pooled across the experiments
    /// that ran it, with the experiment report's statistics, beside each
    /// experiment's own. One `frankenlife.analysis.v1` record. Refused
    /// (exit 2) unless the experiments share their base configuration,
    /// ticks, metrics interval, measures and control, define each shared
    /// condition alike, keep each seed in one split, and pool each seed
    /// once.
    Analyze {
        /// Two or more experiment directories `flife experiment` made.
        #[arg(required = true, num_args = 2..)]
        experiments: Vec<PathBuf>,
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
        /// `glow` (the default), `raw`, or `sphere`: raw with each organism lit as a sphere.
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
        #[command(flatten)]
        features: FeatureFlags,
    },
    /// Run a configuration headlessly and render it to PNG frames (and
    /// optionally an MP4) through the Studio's FrankenManim view; or, with
    /// --run-dir, continue a run directory (a run, branch or transplant)
    /// from where `flife resume` would, writing nothing to it.
    Render {
        /// The configuration or preset, as TOML.
        #[arg(required_unless_present = "run_dir", conflicts_with = "run_dir")]
        config: Option<PathBuf>,
        /// A run directory to render from its newest restorable checkpoint
        /// (and journal), its commands applying at their ticks.
        #[arg(long)]
        run_dir: Option<PathBuf>,
        /// Ticks to run after the first frame.
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
        /// `auto` (Metal at most 8 CPU workers, else `torch` at most 4 or,
        /// on Apple silicon, from 25,000 organisms). No result depends on it.
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
        /// `generation`, `motion`, `topology`, `affinity`, `lineage-energy`,
        /// `cell-state` or `polarity` (or a registered name such as
        /// `founder-lineage-v1`, `cell-state-v1` or `polarity-v1`). The
        /// default is the configuration's `[presentation] color_mapping`.
        #[arg(long)]
        color: Option<ColorMapping>,
        /// `glow` (the default), `raw`: no halos and a linear grey field, or `sphere`: raw with
        /// each organism lit as a sphere at its body size.
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
        /// affinity mapping colours organisms, `component-C`, moment
        /// channel C (0 to 31) per unit energy from blue (-1) to red (+1)
        /// (the previous two as bright as its energy), `trail-carrier`,
        /// Carrier-1 slow trail energy density (`dual-carrier-trails`), or
        /// `vascular-conductance`, mean vascular edge conductance
        /// (`vascular-field`).
        #[arg(long, default_value = "energy")]
        field_mapping: FieldMapping,
        /// A camera path, as JSON: `{"keys": [{"tick", "center": [x, y],
        /// "zoom"}, …]}`, eased between keys, held before the first and
        /// after the last. Presentation only.
        #[arg(long)]
        camera: Option<PathBuf>,
        #[command(flatten)]
        features: FeatureFlags,
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

/// Extract `--disable`, `--enable`, and `--v1-compat` in exact command-line
/// order so chains like `--disable all-v2 --enable so2-motor` apply cleanly.
/// Returns an empty list on subcommands that do not define feature flags.
fn ordered_overrides(matches: &clap::ArgMatches) -> Vec<FeatureOverride> {
    let mut entries: Vec<(usize, FeatureOverride)> = Vec::new();
    if let Ok(Some(values)) = matches.try_get_many::<String>("disable")
        && let Some(indices) = matches.indices_of("disable")
    {
        for (idx, value) in indices.zip(values) {
            entries.push((idx, FeatureOverride::Disable(value.clone())));
        }
    }
    if let Ok(Some(values)) = matches.try_get_many::<String>("enable")
        && let Some(indices) = matches.indices_of("enable")
    {
        for (idx, value) in indices.zip(values) {
            entries.push((idx, FeatureOverride::Enable(value.clone())));
        }
    }
    if let Ok(Some(&count)) = matches.try_get_one::<u8>("v1_compat")
        && count > 0
        && matches.value_source("v1_compat") == Some(clap::parser::ValueSource::CommandLine)
        && let Some(indices) = matches.indices_of("v1_compat")
    {
        for idx in indices {
            entries.push((idx, FeatureOverride::V1Compat));
        }
    }
    entries.sort_by_key(|(idx, _)| *idx);
    entries.into_iter().map(|(_, item)| item).collect()
}

fn refuse_resuming_toggles(command_name: &str) -> ExitCode {
    fail(
        "incompatible_checkpoint",
        format!(
            "v2 feature toggles (--disable/--enable/--v1-compat) are refused on `{command_name}` \
             (INV-14): features are baked into the checkpoint configuration and cannot be mutated \
             on resume or replay"
        ),
    )
}

fn main() -> ExitCode {
    let matches = Cli::command().get_matches();
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(error) => error.exit(),
    };
    if cli.list_features {
        print(&flife_cli::list_features());
        return ExitCode::SUCCESS;
    }
    let overrides = matches
        .subcommand()
        .map_or_else(Vec::new, |(_, sub)| ordered_overrides(sub));
    let Some(command) = cli.command else {
        return fail(
            "usage",
            "a subcommand or --list-features is required".to_owned(),
        );
    };
    match command {
        Command::Validate {
            config,
            features: _,
        } => match load(&config, &overrides) {
            Ok(config) => {
                print(&Validation::of(&config));
                ExitCode::SUCCESS
            }
            Err(code) => code,
        },
        Command::Capabilities {
            config,
            ticks,
            cpu_workers,
            features: _,
        } => {
            if config.is_none() && !overrides.is_empty() {
                return fail(
                    "usage",
                    "v2 feature toggles (--disable/--enable/--v1-compat) require `--config <PATH>` \
                     on `flife capabilities`"
                        .to_owned(),
                );
            }
            capabilities(config.as_ref(), &overrides, ticks, cpu_workers)
        }
        Command::Render {
            config,
            run_dir,
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
            camera,
            features: _,
        } => {
            if run_dir.is_some() && !overrides.is_empty() {
                return refuse_resuming_toggles("flife render --run-dir");
            }
            render(
                (config.as_ref(), run_dir.as_ref()),
                &overrides,
                ticks,
                every,
                [width, height],
                out,
                (cpu_workers, controllers),
                render_threads,
                (video, camera),
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
            )
        }
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
            features: _,
        } => {
            let config = match load(&config, &overrides) {
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
            let mapping = match flife_cli::resolve_color_mapping(&config, color) {
                Ok(mapping) => mapping,
                Err(error) => return fail(error.kind(), error.to_string()),
            };
            if let Err(error) = flife_cli::validate_v2_visual_style(&config, mapping, field_mapping)
            {
                return fail(error.kind(), error.to_string());
            }
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
            features: _,
        } => {
            let host = match load(&config, &overrides) {
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
            features: _,
        } => {
            let experiment = match Experiment::load_with_overrides(&spec, &overrides) {
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
        Command::Analyze { experiments } => {
            let directories: Vec<&std::path::Path> =
                experiments.iter().map(PathBuf::as_path).collect();
            match flife_cli::analyze(&directories) {
                Ok(analysis) => {
                    print(&analysis);
                    ExitCode::SUCCESS
                }
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
            refused_features,
        } => {
            if refused_features.any() {
                return refuse_resuming_toggles("flife resume");
            }
            exit(flife_cli::resume(
                &run_dir,
                &ResumeOptions {
                    until: Until::Tick(until),
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
            ))
        }
        Command::Step {
            run_dir,
            ticks,
            every,
            metrics_every,
            cpu_workers,
            controllers,
            checkpoints,
            refused_features,
        } => {
            if refused_features.any() {
                return refuse_resuming_toggles("flife step");
            }
            exit(flife_cli::resume(
                &run_dir,
                &ResumeOptions {
                    until: Until::Past(ticks),
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
            ))
        }
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
/// Render a configuration, or with a run directory that run (its
/// configuration read from it for the default colour): 2 when the
/// configuration cannot be read or is invalid, 1 when the render fails.
fn render(
    (config, run_dir): (Option<&PathBuf>, Option<&PathBuf>),
    overrides: &[FeatureOverride],
    ticks: u64,
    every: u64,
    size: [u32; 2],
    out: PathBuf,
    (cpu_workers, controllers): (NonZeroUsize, ControllerChoice),
    render_threads: NonZeroUsize,
    (video, camera): (Option<PathBuf>, Option<PathBuf>),
    fps: u32,
    (color, style): (Option<ColorMapping>, Style),
) -> ExitCode {
    let camera = match camera {
        Some(path) => match std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))
            .and_then(|text| flife_cli::CameraPath::from_json(&text).map_err(|e| e.to_string()))
        {
            Ok(camera) => Some(camera),
            Err(error) => return fail("usage", format!("--camera: {error}")),
        },
        None => None,
    };
    let config = match (run_dir, config) {
        (Some(run_dir), _) => match flife_cli::persist::read_config(run_dir) {
            Ok(config) => config,
            Err(error) => return fail(error.kind(), error.to_string()),
        },
        (None, Some(config)) => match load(config, overrides) {
            Ok(config) => config,
            Err(code) => return code,
        },
        (None, None) => return fail("usage", "a configuration or --run-dir".to_owned()),
    };
    let mapping = match flife_cli::resolve_color_mapping(&config, color) {
        Ok(mapping) => mapping,
        Err(error) => return fail(error.kind(), error.to_string()),
    };
    if let Err(error) = flife_cli::validate_v2_visual_style(&config, mapping, style.field_mapping) {
        return fail(error.kind(), error.to_string());
    }
    let style = Style { mapping, ..style };
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
        camera,
    };
    let rendered = match run_dir {
        Some(run_dir) => flife_cli::render_run(run_dir, &options, print),
        None => flife_cli::render(&config, &options, print),
    };
    match rendered {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            fail(error.kind(), error.to_string());
            ExitCode::from(1)
        }
    }
}

/// The capability report: 0 when every route ran as the reference did or
/// says why it could not, 1 when one disagreed or failed, 2 when the
/// reference was refused (the report printed) or nothing could be probed
/// (an error record).
fn capabilities(
    config: Option<&PathBuf>,
    overrides: &[FeatureOverride],
    ticks: u64,
    cpu_workers: Option<NonZeroUsize>,
) -> ExitCode {
    let config = match config.map(|path| load(path, overrides)).transpose() {
        Ok(config) => config,
        Err(code) => return code,
    };
    let options = flife_cli::CapabilitiesOptions { ticks, cpu_workers };
    match flife_cli::capabilities(config.as_ref(), &options) {
        Ok(report) => {
            print(&report);
            if report.defect() {
                ExitCode::from(1)
            } else if report.refused() {
                ExitCode::from(2)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => fail(error.kind(), error.to_string()),
    }
}

fn load(path: &PathBuf, overrides: &[FeatureOverride]) -> Result<ValidatedConfig, ExitCode> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| fail("io", format!("{}: {error}", path.display())))?;
    ValidatedConfig::from_toml_with_overrides(&text, overrides)
        .map_err(|error| fail(error.kind(), error.to_string()))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_overrides(argv: &[&str]) -> Vec<FeatureOverride> {
        let matches = Cli::command()
            .try_get_matches_from(argv)
            .expect("valid CLI invocation");
        matches
            .subcommand()
            .map_or_else(Vec::new, |(_, sub)| ordered_overrides(sub))
    }

    #[test]
    fn ordered_overrides_never_panics_on_any_subcommand() {
        for argv in [
            &["flife", "inspect", "run_dir"][..],
            &["flife", "branch", "parent", "child"][..],
            &["flife", "analyze", "exp_a", "exp_b"][..],
            &["flife", "discover", "run_dir", "out_dir"][..],
            &["flife", "validate", "config.toml"][..],
            &["flife", "capabilities"][..],
            &["flife", "run", "config.toml"][..],
            &["flife", "resume", "run_dir", "--until", "10"][..],
            &["flife", "step", "run_dir"][..],
            &["flife", "experiment", "spec.toml", "--out", "out_dir"][..],
            &[
                "flife",
                "transplant",
                "pkg",
                "run_dir",
                "--config",
                "host.toml",
                "--at",
                "1,2",
            ][..],
            &["flife", "render", "config.toml", "--out", "frames"][..],
        ] {
            assert!(
                parse_overrides(argv).is_empty(),
                "expected empty overrides for {argv:?}"
            );
        }
    }

    #[test]
    fn ordered_overrides_preserves_interleaved_cli_order() {
        assert_eq!(
            parse_overrides(&[
                "flife",
                "validate",
                "v2.toml",
                "--disable",
                "all-v2",
                "--enable",
                "oases",
            ]),
            vec![
                FeatureOverride::Disable("all-v2".to_owned()),
                FeatureOverride::Enable("oases".to_owned()),
            ]
        );
        assert_eq!(
            parse_overrides(&[
                "flife",
                "run",
                "v2.toml",
                "--v1-compat",
                "--enable",
                "oases",
                "--disable",
                "oases",
            ]),
            vec![
                FeatureOverride::V1Compat,
                FeatureOverride::Enable("oases".to_owned()),
                FeatureOverride::Disable("oases".to_owned()),
            ]
        );
        assert_eq!(
            parse_overrides(&[
                "flife",
                "validate",
                "v2.toml",
                "--enable",
                "oases",
                "--v1-compat"
            ]),
            vec![
                FeatureOverride::Enable("oases".to_owned()),
                FeatureOverride::V1Compat,
            ]
        );
    }

    #[test]
    fn render_help_lists_v2_color_and_field_mappings() {
        let mut command = Cli::command();
        let render = command
            .find_subcommand_mut("render")
            .expect("render subcommand exists");
        let help = render.render_long_help().to_string();
        for token in [
            "cell-state",
            "polarity",
            "trail-carrier",
            "vascular-conductance",
        ] {
            assert!(
                help.contains(token),
                "flife render --help must document `{token}`:\n{help}"
            );
        }
        for argv in [
            &[
                "flife",
                "render",
                "config.toml",
                "--out",
                "frames",
                "--color",
                "cell-state",
                "--field-mapping",
                "trail-carrier",
            ][..],
            &[
                "flife",
                "render",
                "config.toml",
                "--out",
                "frames",
                "--color",
                "polarity",
                "--field-mapping",
                "vascular-conductance",
            ][..],
        ] {
            assert!(
                Cli::command().try_get_matches_from(argv).is_ok(),
                "flife render must accept {argv:?}"
            );
        }
    }

    #[test]
    fn render_color_mapping_falls_back_on_v2_compat_and_refuses_explicit_v2_flags() {
        let bundle_a = include_str!("../../../presets/morphogenesis-tissue-v2.toml");
        let v1_toml = include_str!("../../../presets/reference-small-world.toml");
        let default_cfg = ValidatedConfig::from_toml(bundle_a).expect("Bundle A loads");
        let v1_cfg = ValidatedConfig::from_toml(v1_toml).expect("v1 preset loads");

        let default_mapping =
            flife_cli::resolve_color_mapping(&default_cfg, None).expect("Bundle A default");
        assert_eq!(default_mapping, ColorMapping::CellState);
        flife_cli::validate_v2_visual_style(&default_cfg, default_mapping, FieldMapping::Energy)
            .expect("Bundle A default cell-state mapping is valid");

        for mapping in [ColorMapping::CellState, ColorMapping::Polarity] {
            flife_cli::validate_v2_visual_style(&default_cfg, mapping, FieldMapping::Energy)
                .expect("v2 color mapping allowed when v2-visual-mappings enabled");
            assert!(matches!(
                flife_cli::validate_v2_visual_style(&v1_cfg, mapping, FieldMapping::Energy),
                Err(flife_core::CoreError::InvalidConfig(_))
            ));
        }
        for field_mapping in [
            FieldMapping::TrailCarrier,
            FieldMapping::VascularConductance,
        ] {
            flife_cli::validate_v2_visual_style(&default_cfg, ColorMapping::Lineage, field_mapping)
                .expect("v2 field overlay allowed when v2-visual-mappings enabled");
            assert!(matches!(
                flife_cli::validate_v2_visual_style(&v1_cfg, ColorMapping::Lineage, field_mapping),
                Err(flife_core::CoreError::InvalidConfig(_))
            ));
        }

        for overrides in [
            [FeatureOverride::V1Compat],
            [FeatureOverride::Disable("v2-visual-mappings".to_owned())],
        ] {
            let disabled = ValidatedConfig::from_toml_with_overrides(bundle_a, &overrides)
                .expect("v2 preset with override loads");
            let fallback = flife_cli::resolve_color_mapping(&disabled, None)
                .expect("omitted --color falls back cleanly");
            assert_eq!(fallback, ColorMapping::Lineage);
            flife_cli::validate_v2_visual_style(&disabled, fallback, FieldMapping::Energy)
                .expect("fallback lineage passes v2 visual gate");

            for mapping in [ColorMapping::CellState, ColorMapping::Polarity] {
                let explicit = flife_cli::resolve_color_mapping(&disabled, Some(mapping))
                    .expect("explicit --color preserved");
                assert_eq!(explicit, mapping);
                assert!(matches!(
                    flife_cli::validate_v2_visual_style(&disabled, explicit, FieldMapping::Energy),
                    Err(flife_core::CoreError::InvalidConfig(_))
                ));
            }
            for field_mapping in [
                FieldMapping::TrailCarrier,
                FieldMapping::VascularConductance,
            ] {
                assert!(matches!(
                    flife_cli::validate_v2_visual_style(
                        &disabled,
                        ColorMapping::Lineage,
                        field_mapping
                    ),
                    Err(flife_core::CoreError::InvalidConfig(_))
                ));
            }
        }

        // A v1 TOML file configured with a v2 colour mapping must NOT fall back silently.
        let bad_v1_toml = v1_toml.replacen(
            "color_mapping = \"founder-lineage-v1\"",
            "color_mapping = \"cell-state-v1\"",
            1,
        );
        let bad_v1 = ValidatedConfig::from_toml(&bad_v1_toml).expect("v1 toml parses");
        let configured = flife_cli::resolve_color_mapping(&bad_v1, None).expect("reads v1 mapping");
        assert_eq!(configured, ColorMapping::CellState);
        assert!(matches!(
            flife_cli::validate_v2_visual_style(&bad_v1, configured, FieldMapping::Energy),
            Err(flife_core::CoreError::InvalidConfig(_))
        ));
    }
}
