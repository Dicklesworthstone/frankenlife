//! `flife-studio`: the live world in a native window (FL-018; plan §13.6).
//!
//! Every line on standard output is one `frankenlife.studio.v1` record: an
//! `open`, then a `status` per session status, a `view` per style change,
//! a `selection` each time the selected organism changes (a click, or the
//! selected organism gone from a newer epoch), an `inspected` record for
//! each organism a click selects (FL-021: it at the session's committed
//! boundary `tick`, selected on the epoch of `seen_tick`), a `brush` each
//! time the armed brush changes, a `submitted` record for each brush click
//! the session accepted as a command (FL-022: the boundary of
//! `effective_tick` it applies at) and a `finished` record when it applied
//! (the tick, the published tick and the energy it moved) or was cancelled,
//! a `frame` summary about once a second and, while `[presentation]
//! show_clock_telemetry` is set, a `clock` record beside it (plan §12.2:
//! ticks committed per wall second, the simulation-time to wall-time
//! ratio, frames presented per second and the display's backlog in ticks),
//! then an `end` with the committed world's digest. The window draws the
//! same state as text in its top-left corner: running or paused at the
//! shown tick, the clocks while they are reported, the colour mapping, the
//! armed brush and the selected organism (FL-024). An error
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
use flife_core::memory::MemoryAdmission;
use flife_core::{Tick, ValidatedConfig};
use flife_engine::OrganismSnapshot;
use flife_runtime::Outcome;
use flife_studio::{
    Brush, ClockMeter, ClockReading, ColorMapping, End, Look, OverlayState, Particles,
    STUDIO_SCHEMA, Status, Stroke, Studio, StudioOptions, Style, overlay_text,
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
        clock_telemetry: bool,
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
        memory: studio.memory(),
        mapping: options.style.mapping.name(),
        meaning: options.style.mapping.meaning(),
        look: options.style.look.name(),
        particles: options.style.particles.name(),
        field: options.style.field,
        clock_telemetry: model.presentation.show_clock_telemetry,
    });

    let period = Duration::from_secs(1) / fps;
    let mut summary = Instant::now();
    let mut clocks = ClockMeter::new(config.report().timestep);
    if model.presentation.show_clock_telemetry {
        let _ = clocks.read(
            summary,
            studio.committed().map_or(Tick::ZERO, |epoch| epoch.tick),
            studio.tally().presented,
            None,
        );
    }
    let mut selected = None;
    let mut armed = None;
    // The overlay (FL-024): the session's last reported state, the tick
    // shown when it or the clocks last changed, and the last clock reading.
    let mut paused = None;
    let mut overlay_tick = None;
    let mut last_clock = None;
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
            match &status {
                Status::Running { .. } => paused = Some(false),
                Status::Paused { .. } => paused = Some(true),
                _ => {}
            }
            overlay_tick = frame.shown.map(|shown| shown.tick);
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
                upload_bytes: tally.upload_bytes,
                peak_upload_bytes: tally.peak_upload_bytes,
                readback_bytes: tally.readback_bytes,
            });
            if model.presentation.show_clock_telemetry
                && let Some(reading) = clocks.read(
                    summary,
                    studio.committed().map_or(Tick::ZERO, |epoch| epoch.tick),
                    tally.presented,
                    frame.shown.map(|shown| shown.tick),
                )
            {
                print(&clock_record(&reading));
                last_clock = Some(reading);
            }
        }
        if overlay_tick.is_none() {
            overlay_tick = frame.shown.map(|shown| shown.tick);
        }
        let text = overlay_text(&OverlayState {
            paused,
            shown: overlay_tick,
            clock: last_clock.as_ref(),
            mapping: studio.view().style().mapping.name(),
            brush: armed,
            selected,
        });
        if let Err(error) = studio.set_overlay(&text) {
            let code = fail(error.kind(), error.to_string(), 1);
            let _ = studio.close();
            return code;
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

    use flife_core::Tick;
    use flife_core::ids::{CommandId, ResourceEpoch};
    use flife_runtime::{CancelReason, Outcome};
    use flife_studio::{Status, Stroke};
    use serde_json::{Value, json};

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
                "behind_ticks": 1
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
