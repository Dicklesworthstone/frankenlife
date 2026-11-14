//! FL-024: plan §14.1's persistent telemetry through the binary.
//!
//! `flife-studio --offscreen 2.5 --timeline T` prints a `telemetry` record
//! a second, from the epoch it shows, and draws the same telemetry in its
//! overlay, which T records. It runs [`lively`], the preset on a small field
//! with founders able to reproduce at once and a basal cost that starves the
//! unfed, so births and deaths come within a debug build's few seconds. Each
//! is compared with that world stepped directly to the shown tick:
//! - each record's population, parameters, genome bytes, births, deaths,
//!   introductions, controller evaluations, rejected births, uptake and
//!   ledger terms, exactly, and its headroom with the `open` record's
//!   admission;
//! - each `clock` record's evaluations with the change in the telemetry's
//!   count since the last one, and the record's full evaluations a second
//!   with the clock's rate over the live population;
//! - each overlay T recorded with the telemetry lines of that world.
//!
//! [`crowded`] has room for no one past its founders, so the first birth
//! pauses the run before it commits: the binary's `ended` status names the
//! capacity pause at the tick the direct world pauses at, every telemetry
//! record after it counts one capacity pause and every one before none, and
//! the overlay says so, and that the world is paused.
//!
//! With `--field-mapping composition` (FL-019, plan §13.4's "Local field")
//! the binary says so in its `open` record, and every overlay carries the
//! field's key after the colour key, its swatches after that key's; without
//! it, the field shows energy and no overlay has a field key.
//!
//! The run passes `--offscreen`, so no window opens. Headless Metal: without
//! a Metal device the test returns early. Each run leaves its configuration
//! and timeline, well under 1 MB, under the target's temporary directory;
//! nothing deletes them.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::float_cmp,
    reason = "test: a failure is a test failure; figures are printed; exact equality is the claim"
)]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;
use std::time::SystemTime;

use flife_core::memory::MemoryAdmission;
use flife_core::{Tick, Transfer, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_render::{Epoch, Metrics, Tally};
use flife_studio::{
    ColorMapping, FieldMapping, Telemetry, TimelineRecord, legends, read_timeline, telemetry_lines,
};
use fmn_render::metal::MetalRenderer;
use serde_json::Value;

const PRESET: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../presets/reference-small-world.toml"
);

/// The preset on a 32 × 32 field with 384 founders that start above the
/// birth threshold (3.5 against 3.0) and pay 1.5 a second to live: each can
/// reproduce at once, and its children, endowed with part of its energy,
/// starve within a few seconds unless they feed.
fn lively() -> String {
    configured(&[("population", "maximum_count", "4096")])
}

/// [`lively`] with room for its 384 founders and no one more.
fn crowded() -> String {
    configured(&[("population", "maximum_count", "384")])
}

/// The preset with [`lively`]'s small field and founders, and `extra`.
fn configured(extra: &[(&str, &str, &str)]) -> String {
    let preset = std::fs::read_to_string(PRESET).expect("the preset");
    let mut lines: Vec<String> = preset.lines().map(str::to_owned).collect();
    let base = [
        ("world", "width", "32.0"),
        ("world", "height", "32.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", "384"),
        ("population", "initial_energy", "3.5"),
        ("costs", "basal_energy_per_second", "1.5"),
    ];
    for &(section, key, value) in base.iter().chain(extra) {
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
    lines.join("\n")
}

fn number(record: &Value, key: &str) -> u64 {
    record[key]
        .as_u64()
        .unwrap_or_else(|| panic!("`{key}` in {record}"))
}

fn float(record: &Value, key: &str) -> f64 {
    record[key]
        .as_f64()
        .unwrap_or_else(|| panic!("`{key}` in {record}"))
}

/// The admission an `open` record reports.
fn admission(memory: &Value) -> MemoryAdmission {
    MemoryAdmission {
        budget_bytes: number(memory, "budget_bytes"),
        logical_parameters: number(memory, "logical_parameters"),
        logical_genome_bytes: number(memory, "logical_genome_bytes"),
        genomes: number(memory, "genomes"),
        state: number(memory, "state"),
        field: number(memory, "field"),
        scratch: number(memory, "scratch"),
        births: number(memory, "births"),
        render: number(memory, "render"),
        checkpoint: number(memory, "checkpoint"),
        package: number(memory, "package"),
        reserve: number(memory, "reserve"),
        backend: number(memory, "backend"),
        peak_bytes: number(memory, "peak_bytes"),
    }
}

/// The tick an overlay's first line shows, as `running · tick 1,024`.
fn overlay_tick(text: &str) -> Option<u64> {
    let first = text.lines().next()?;
    let (_, tick) = first.split_once(" · tick ")?;
    tick.replace(',', "").parse().ok()
}

/// The configuration's world stepped directly, with its tally.
struct Direct {
    world: World,
    tally: Tally,
}

impl Direct {
    fn to(&mut self, tick: u64) {
        while self.world.tick().get() < tick {
            match self.world.step().expect("steps") {
                Stepped::Published(report) => {
                    let tally = &mut self.tally;
                    tally.births += report.births.len() as u64;
                    tally.introductions += report.introductions.len() as u64;
                    tally.deaths += report.deaths.len() as u64;
                    tally.evaluations += report.evaluated as u64;
                    tally.rejected_births += report.rejected.len() as u64;
                    tally.uptake += report.closure.credited(Transfer::Uptake);
                }
                Stepped::Paused(shortfall) => panic!("the direct world paused: {shortfall:?}"),
            }
        }
        assert_eq!(self.world.tick().get(), tick);
    }

    /// The metrics an epoch of the world at its tick carries, with the
    /// tally from founding.
    fn metrics(&self) -> Metrics {
        let epoch = Epoch::standalone(&self.world).expect("an epoch");
        Metrics {
            tally: Some(self.tally),
            ..*epoch.metrics()
        }
    }
}

/// The kind of a record the binary prints.
fn kind(record: &Value) -> &str {
    record["record"].as_str().unwrap_or("")
}

/// The binary run offscreen for 2.5 s on configuration `text`, named
/// `name`: the records it printed and the timeline it wrote.
fn run_offscreen(text: &str, name: &str) -> (Vec<Value>, PathBuf) {
    run_offscreen_with(text, name, &[])
}

/// [`run_offscreen`] with `extra` arguments.
fn run_offscreen_with(text: &str, name: &str, extra: &[&str]) -> (Vec<Value>, PathBuf) {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("after the epoch")
        .as_nanos();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("fl024-telemetry-{}-{nanos}", std::process::id()));
    std::fs::create_dir(&dir).expect("a new directory");
    let path = dir.join(format!("{name}.toml"));
    std::fs::write(&path, text).expect("writes the configuration");
    let timeline = dir.join("session.ndjson");
    let output = Command::new(env!("CARGO_BIN_EXE_flife-studio"))
        .arg(&path)
        .args([
            "--offscreen",
            "2.5",
            "--scale",
            "1",
            "--width",
            "320",
            "--height",
            "200",
            "--controllers",
            "cpu",
            "--cpu-workers",
            "2",
            "--timeline",
        ])
        .arg(&timeline)
        .args(extra)
        .output()
        .expect("the binary runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "exit {:?}: {stderr}",
        output.status
    );
    let records = String::from_utf8(output.stdout)
        .expect("UTF-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON record"))
        .collect();
    (records, timeline)
}

/// The overlays a timeline recorded, in order, with their swatches.
fn overlays_with_swatches(timeline: &PathBuf) -> Vec<(String, Vec<[u8; 3]>)> {
    let text = std::fs::read_to_string(timeline).expect("the timeline");
    read_timeline(&text)
        .expect("reads")
        .into_iter()
        .filter_map(|record| match record {
            TimelineRecord::Overlay { text, swatches } if !text.is_empty() => {
                Some((text, swatches))
            }
            _ => None,
        })
        .collect()
}

/// The overlays a timeline recorded, in order.
fn overlays(timeline: &PathBuf) -> Vec<String> {
    let text = std::fs::read_to_string(timeline).expect("the timeline");
    read_timeline(&text)
        .expect("reads")
        .into_iter()
        .filter_map(|record| match record {
            TimelineRecord::Overlay { text, .. } if !text.is_empty() => Some(text),
            _ => None,
        })
        .collect()
}

#[test]
fn the_binary_reports_and_draws_the_shown_worlds_telemetry() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let text = lively();
    let config = ValidatedConfig::from_toml(&text).expect("valid");
    let (records, timeline) = run_offscreen(&text, "lively");
    let open = &records[0];
    assert_eq!(kind(open), "open");
    let memory = admission(&open["memory"]);
    // The field shows energy, which has no key of its own.
    assert_eq!(open["field_mapping"], "energy");
    assert!(
        overlays(&timeline)
            .iter()
            .all(|overlay| !overlay.contains("field composition")),
        "an energy field has no field key"
    );

    let model = config.config();
    let mut direct = Direct {
        world: World::found(
            &config,
            Arrangement::FreeList,
            NumericalProfile::reference_f32().expect("the profile"),
        )
        .expect("founds"),
        tally: Tally::from(Tick::ZERO),
    };

    // Every telemetry record is the shown world's.
    let mut checked = 0;
    let mut paired = 0;
    let mut evaluated_before = 0_u64;
    let mut shown_ticks = Vec::new();
    for (at, record) in records.iter().enumerate() {
        if kind(record) != "telemetry" {
            continue;
        }
        let shown = number(record, "shown_tick");
        // The direct world only steps forward: a record that showed an
        // earlier tick than the last would be compared with a later world.
        assert!(
            shown_ticks.last().is_none_or(|&last| shown >= last),
            "shown ticks {shown_ticks:?} then {shown}"
        );
        shown_ticks.push(shown);
        direct.to(shown);
        let metrics = direct.metrics();
        let tally = direct.tally;
        assert_eq!(number(record, "live"), metrics.live, "{record}");
        assert_eq!(number(record, "parameters"), metrics.parameters);
        assert_eq!(
            number(record, "genome_logical_bytes"),
            metrics.genome_logical_bytes
        );
        assert_eq!(
            number(record, "genome_allocated_bytes"),
            metrics.genome_allocated_bytes
        );
        assert_eq!(
            number(record, "headroom_bytes"),
            memory.budget_bytes - memory.peak_bytes
        );
        assert_eq!(number(record, "counted_from_tick"), 0);
        assert_eq!(number(record, "births"), tally.births, "{record}");
        assert_eq!(number(record, "introductions"), tally.introductions);
        assert_eq!(number(record, "deaths"), tally.deaths, "{record}");
        assert_eq!(number(record, "evaluations"), tally.evaluations);
        assert_eq!(number(record, "rejected_births"), tally.rejected_births);
        assert_eq!(float(record, "uptake").to_bits(), tally.uptake.to_bits());
        let ledger = direct.world.ledger().report();
        assert_eq!(
            float(record, "injected").to_bits(),
            ledger.injected.to_bits()
        );
        assert_eq!(float(record, "removed").to_bits(), ledger.removed.to_bits());
        assert_eq!(
            float(record, "dissipated").to_bits(),
            ledger.dissipated.to_bits()
        );
        assert_eq!(
            float(record, "residual").to_bits(),
            ledger.residual.to_bits()
        );
        assert_eq!(record["faulted"], false);
        assert_eq!(number(record, "capacity_pauses"), 0);
        // The clock read in the same second: the evaluations since the last
        // reading, and the full evaluations a second over the population.
        if at > 0 && kind(&records[at - 1]) == "clock" {
            let clock = &records[at - 1];
            assert_eq!(
                number(clock, "evaluations"),
                tally.evaluations - evaluated_before,
                "{clock}"
            );
            let rate = float(clock, "evaluations_per_second");
            if metrics.live == 0 {
                assert_eq!(record["full_evaluations_per_second"], Value::Null);
            } else {
                assert_eq!(
                    float(record, "full_evaluations_per_second"),
                    rate / metrics.live as f64
                );
            }
            paired += 1;
        }
        evaluated_before = tally.evaluations;
        checked += 1;
    }
    assert!(
        checked >= 2 && paired >= 2,
        "{checked} records, {paired} with clocks"
    );
    assert!(
        shown_ticks.first() < shown_ticks.last(),
        "the run advanced between records: {shown_ticks:?}"
    );
    let last = direct.tally;
    assert!(
        last.births > 0 && last.deaths > 0 && last.evaluations > 0 && last.uptake > 0.0,
        "the run was born, died, evaluated and fed: {last:?}"
    );
    assert!(
        !direct.world.population().is_empty(),
        "the population lives at the last record"
    );

    // Every overlay drawn says the shown world's telemetry.
    let overlays = overlays(&timeline);
    let ticks: BTreeSet<u64> = overlays
        .iter()
        .filter_map(|text| overlay_tick(text))
        .collect();
    assert!(ticks.len() >= 2, "{ticks:?}");
    let mut direct = Direct {
        world: World::found(
            &config,
            Arrangement::FreeList,
            NumericalProfile::reference_f32().expect("the profile"),
        )
        .expect("founds"),
        tally: Tally::from(Tick::ZERO),
    };
    let mut drawn = 0;
    for tick in ticks {
        direct.to(tick);
        let metrics = direct.metrics();
        let expected = telemetry_lines(
            &Telemetry {
                metrics: &metrics,
                model: &model.model,
                seed: model.seed,
                evaluation_interval_ticks: model.controller.evaluation_interval_ticks,
                memory: &memory,
                capacity_pauses: 0,
                checkpoint: None,
            },
            None,
        );
        for overlay in overlays
            .iter()
            .filter(|text| overlay_tick(text) == Some(tick))
        {
            let lines: Vec<&str> = overlay.lines().collect();
            for (index, line) in expected.lines().enumerate() {
                if index == 1 {
                    // The cadence; the rate depends on the clock's window.
                    assert!(
                        lines.iter().any(|drawn| drawn.starts_with(line)),
                        "{line:?} in {overlay}"
                    );
                } else {
                    assert!(lines.contains(&line), "{line:?} in {overlay}");
                }
            }
            drawn += 1;
        }
    }
    println!(
        "{checked} telemetry records ({paired} with a clock) and {drawn} overlays matched the direct \
         world; through tick {}: {last:?}",
        direct.world.tick().get()
    );
}

/// `--field-mapping composition` opens the field's composition view: the
/// `open` record says so, and every overlay carries the field's key on the
/// line after the colour key's, its swatches after that key's.
#[test]
fn the_binary_keys_the_fields_composition() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let text = lively();
    let config = ValidatedConfig::from_toml(&text).expect("valid");
    let mapping: ColorMapping = config
        .config()
        .presentation
        .color_mapping
        .parse()
        .expect("a colour mapping");
    let keys = legends(&config).expect("the keys");
    let legend = keys
        .iter()
        .find_map(|(key, legend)| (*key == mapping).then_some(legend))
        .expect("the mapping's key");
    let field = FieldMapping::Composition.legend().expect("the field's key");
    let (records, timeline) =
        run_offscreen_with(&text, "composition", &["--field-mapping", "composition"]);
    assert_eq!(kind(&records[0]), "open");
    assert_eq!(records[0]["field_mapping"], "composition");
    let laid = overlays_with_swatches(&timeline);
    assert!(!laid.is_empty(), "no overlay was recorded");
    let swatches: Vec<[u8; 3]> = legend
        .swatches
        .iter()
        .chain(&field.swatches)
        .copied()
        .collect();
    for (text, drawn) in &laid {
        let lines: Vec<&str> = text.lines().collect();
        let at = lines
            .iter()
            .position(|line| *line == legend.text)
            .unwrap_or_else(|| panic!("no colour key in {text:?}"));
        assert_eq!(lines.get(at + 1), Some(&field.text.as_str()), "{text:?}");
        assert_eq!(drawn, &swatches, "{text:?}");
    }
}

#[test]
fn a_capacity_pause_is_counted_reported_and_drawn() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let text = crowded();
    let config = ValidatedConfig::from_toml(&text).expect("valid");
    // The tick the world stepped directly pauses at: its first birth has
    // no room.
    let mut world = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("the profile"),
    )
    .expect("founds");
    let paused_at = loop {
        match world.step().expect("steps") {
            Stepped::Published(report) => {
                assert!(report.births.is_empty(), "a birth past the ceiling");
                assert!(
                    world.tick().get() < 1_000,
                    "no capacity pause by tick 1,000"
                );
            }
            Stepped::Paused(_) => break world.tick().get(),
        }
    };

    let (records, timeline) = run_offscreen(&text, "crowded");
    let ended: Vec<&Value> = records
        .iter()
        .filter(|record| kind(record) == "status" && record["status"] == "ended")
        .collect();
    assert_eq!(ended.len(), 1, "{ended:?}");
    assert_eq!(ended[0]["why"], "capacity-paused", "{}", ended[0]);
    assert_eq!(number(ended[0], "tick"), paused_at);
    // None counted before the pause; one in every record after it.
    let mut after = false;
    let mut counts = (0, 0);
    let mut last = None;
    for record in &records {
        if std::ptr::eq(record, ended[0]) {
            after = true;
        }
        if kind(record) == "telemetry" {
            assert_eq!(
                number(record, "capacity_pauses"),
                u64::from(after),
                "{record}"
            );
            if after {
                counts.1 += 1;
            } else {
                counts.0 += 1;
            }
            last = Some(record);
        }
    }
    assert!(
        counts.1 >= 1,
        "{counts:?} telemetry records before and after the pause"
    );
    let last = last.expect("a telemetry record");
    assert_eq!(number(last, "shown_tick"), paused_at, "{last}");
    assert_eq!(number(last, "births"), 0, "{last}");

    // Every overlay counts the pauses so far; the last shows the one.
    let overlays = overlays(&timeline);
    assert!(
        overlays.iter().all(|overlay| {
            overlay.contains(" · no capacity pause · ")
                || overlay.contains(" · 1 capacity pause · ")
        }),
        "{overlays:?}"
    );
    let shown = overlays.last().expect("an overlay");
    assert!(shown.contains(" · 1 capacity pause · "), "{shown}");
    assert_eq!(overlay_tick(shown), Some(paused_at), "{shown}");
    // The run's end left the world paused, and the overlay says so.
    assert!(shown.starts_with("paused · tick "), "{shown}");
    println!(
        "paused at tick {paused_at}; {} telemetry records before, {} after; {} overlays",
        counts.0,
        counts.1,
        overlays.len()
    );
}
