//! FL-024's metric overlay (plan §12.2) and persistent telemetry (plan
//! §14.1), FL-019's colour key in it (plan §13.4) and FL-021's inspector
//! panel (plan §14.2): the text the Studio draws over the world, exactly,
//! and that the bundled font sets all of it within the overlay's budget. No
//! window opens.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use flife_core::ids::{Generation, LineageId, OrganismId};
use flife_core::memory::MemoryAdmission;
use flife_core::{LedgerReport, Tick, ValidatedConfig};
use flife_engine::{
    Arrangement, MATERIAL_CHANNELS, NumericalProfile, OrganismSnapshot, Stepped, World,
};
use flife_manim::{
    HUD_MAX_SWATCHES, HUD_MAX_TRIANGLES, HUD_MAX_VERTICES, HUD_SWATCH, HUD_TEXT_POINTS, HudFont,
    Legend, linear,
};
use flife_render::{Metrics, Tally};
use flife_studio::{
    BRUSH_ENERGY, BRUSH_INTRODUCTION, Brush, BrushPreview, ClockReading, ColorMapping, Inspected,
    OverlayState, Preview, Stroke, Telemetry, inspector_lines, legends, overlay_text, preview_line,
    telemetry_lines,
};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

const READING: ClockReading = ClockReading {
    wall_seconds: 1.0,
    ticks: 128,
    ticks_per_second: 128.0,
    simulation_rate: 1.0,
    frames: 60,
    frames_per_second: 60.0,
    committed: Tick::new(1_025),
    shown: Some(Tick::new(1_024)),
    behind: Some(1),
    // 4,608 organisms evaluated on each of 128 ticks.
    evaluations: Some(589_824),
    evaluations_per_second: Some(589_824.0),
};

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

/// A run ledger's report with these cumulative terms.
const fn ledger(injected: f64, removed: f64, dissipated: f64, residual: f64) -> LedgerReport {
    LedgerReport {
        ticks: 1_024,
        initial: 100.0,
        current: 100.0,
        injected,
        removed,
        dissipated,
        turnover: 0.0,
        residual,
        normalized: None,
        versus_initial: None,
        versus_current: None,
        largest_tick_residual: 0.0,
        drift_per_tick: None,
        allowance: 1.0,
    }
}

/// An admission with this allocated genome quota, budget and peak.
const fn admission(genomes: u64, budget_bytes: u64, peak_bytes: u64) -> MemoryAdmission {
    MemoryAdmission {
        budget_bytes,
        logical_parameters: 0,
        logical_genome_bytes: 0,
        genomes,
        state: 0,
        field: 0,
        scratch: 0,
        births: 0,
        render: 0,
        checkpoint: 0,
        reserve: 0,
        backend: 0,
        peak_bytes,
    }
}

/// A shown epoch's metrics with every value the telemetry prints set.
const fn metrics() -> Metrics {
    Metrics {
        tick: Tick::new(1_024),
        live: 4_608,
        organism_energy: 0.0,
        field_energy: 0.0,
        max_generation: 7,
        events: 0,
        parameters: 9_124_000,
        genome_logical_bytes: 36 * MIB,
        // 38.2 MiB.
        genome_allocated_bytes: 40_055_603,
        ledger: ledger(1_234.5, 0.0, 789.06, 1.234e-12),
        faulted: false,
        tally: Some(Tally {
            since: Tick::ZERO,
            births: 18_586,
            introductions: 0,
            deaths: 18_255,
            evaluations: 0,
            rejected_births: 2,
            uptake: 456.74,
        }),
    }
}

/// The run's telemetry for `metrics` under `memory`.
const fn telemetry<'a>(metrics: &'a Metrics, memory: &'a MemoryAdmission) -> Telemetry<'a> {
    Telemetry {
        metrics,
        model: "field-ecology-v1",
        seed: 42,
        evaluation_interval_ticks: 1,
        memory,
        capacity_pauses: 0,
        checkpoint: None,
    }
}

fn state(legend: &Legend) -> OverlayState<'_> {
    OverlayState {
        paused: None,
        shown: None,
        clock: None,
        legend,
        brush: None,
        preview: None,
        selected: None,
        inspected: None,
        telemetry: None,
    }
}

/// The widest preview of `brush` the Studio can write: its brushes' own
/// amount and count, the largest cell indices, bounds and a founder's
/// endowment one short of scientific notation at the finest places, and
/// the finest radius.
fn widest_preview(brush: Brush) -> BrushPreview {
    let cell = [u32::MAX, u32::MAX];
    let stroke = match brush {
        Brush::Energy => Stroke::Energy {
            cell,
            amount: BRUSH_ENERGY,
        },
        Brush::Material => Stroke::Material {
            cell,
            amount: BRUSH_ENERGY,
            direction: [0.0; MATERIAL_CHANNELS],
        },
        Brush::Introduction => Stroke::Introduction {
            cell,
            count: BRUSH_INTRODUCTION,
            energy: 99_999.994,
        },
        Brush::Removal => Stroke::Removal {
            center: [0.0; 2],
            radius: 1.0e-7,
        },
    };
    BrushPreview {
        preview: Preview::Stroke {
            stroke,
            region: stroke
                .cell()
                .map(|_| [[99_999.999_999_8, 99_999.999_999_9]; 2]),
        },
        recycle_fraction: 0.999,
    }
}

/// The run's key for `mapping`.
fn key(mapping: ColorMapping) -> Legend {
    legends(&config())
        .expect("the keys")
        .into_iter()
        .find_map(|(key, legend)| (key == mapping).then_some(legend))
        .expect("every mapping's key")
}

/// The preset at 32 × 32 world units with 96 founders.
fn config() -> ValidatedConfig {
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
            .unwrap_or_else(|| panic!("no key {key} in {header}"));
        lines[line] = format!("{key} = {value}");
    }
    ValidatedConfig::from_toml(&lines.join("\n")).expect("a valid configuration")
}

/// A real snapshot: an organism of [`config`]'s world three ticks in, so it
/// holds a controller output.
fn snapshot() -> OrganismSnapshot {
    let config = config();
    let mut world = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds");
    for _ in 0..3 {
        assert!(matches!(
            world.step().expect("steps"),
            Stepped::Published(_)
        ));
    }
    let id = world.population().ids().next().expect("an organism");
    let snapshot = world
        .organism_snapshot(id)
        .expect("inspects")
        .expect("live");
    assert!(snapshot.decoded.is_some(), "it holds an output");
    snapshot
}

/// [`snapshot`] with every value the panel prints set.
fn known() -> OrganismSnapshot {
    let mut known = snapshot();
    known.tick = Tick::new(5_120);
    known.age_ticks = 1_024;
    known.hidden_widths = vec![64, 32];
    known.parameters = 9_124;
    let organism = &mut known.organism;
    organism.lineage = LineageId::new(17);
    organism.generation = Generation::new(42);
    organism.energy = 3.214;
    organism.energy_change_rate = 0.0123;
    // Up and to the left on screen: world y points down.
    organism.velocity = [-0.6, -0.6];
    let decoded = known.decoded.as_mut().expect("an output");
    decoded.attempt_probability = 0.0314;
    decoded.emission_gate = 0.72;
    known
}

#[test]
fn the_overlay_says_what_the_session_is_doing() {
    let lineage = key(ColorMapping::Lineage);
    let energy = key(ColorMapping::Energy);
    assert_eq!(
        overlay_text(&state(&lineage)),
        "opening\nlineage ■■■■■■■■■■■■ by founder"
    );
    assert_eq!(
        overlay_text(&OverlayState {
            paused: Some(false),
            shown: Some(Tick::new(1_024)),
            clock: Some(&READING),
            ..state(&lineage)
        }),
        "running · tick 1,024\n128 ticks/s · 1.00× real time · 60 fps\nlineage ■■■■■■■■■■■■ by founder"
    );
    let slow = ClockReading {
        ticks_per_second: 29.96,
        simulation_rate: 0.234_062_5,
        frames_per_second: 59.6,
        ..READING
    };
    assert_eq!(
        overlay_text(&OverlayState {
            paused: Some(true),
            shown: Some(Tick::new(7)),
            clock: Some(&slow),
            legend: &energy,
            brush: Some(Brush::Material),
            preview: None,
            selected: Some(OrganismId::new(1_234_567)),
            inspected: None,
            telemetry: None,
        }),
        "paused · tick 7\n30.0 ticks/s · 0.23× real time · 60 fps\n\
         energy ■ 0 ■ 1 ■ 2 ■ 4 ■ 8 ■ 16\nmaterial brush · organism 1,234,567"
    );
    assert_eq!(
        overlay_text(&OverlayState {
            paused: Some(true),
            shown: Some(Tick::new(1_000_000)),
            brush: Some(Brush::Removal),
            ..state(&lineage)
        }),
        "paused · tick 1,000,000\nlineage ■■■■■■■■■■■■ by founder\nremoval brush"
    );
    // A count from a trillion, and a rate from 100,000, in scientific
    // notation.
    for (tick, shown) in [
        (999_999_999_999, "999,999,999,999"),
        (1_000_000_000_000, "1.00e12"),
        (u64::MAX, "1.84e19"),
    ] {
        let text = overlay_text(&OverlayState {
            paused: Some(false),
            shown: Some(Tick::new(tick)),
            ..state(&lineage)
        });
        assert_eq!(
            text.lines().next(),
            Some(format!("running · tick {shown}").as_str())
        );
    }
    let fast = ClockReading {
        ticks_per_second: 123_456_789.0,
        simulation_rate: 250_000.0,
        ..READING
    };
    let text = overlay_text(&OverlayState {
        clock: Some(&fast),
        selected: Some(OrganismId::new(u64::MAX)),
        ..state(&lineage)
    });
    assert_eq!(
        text,
        "opening\n1.23e8 ticks/s · 2.50e5× real time · 60 fps\nlineage ■■■■■■■■■■■■ by founder\norganism 1.84e19"
    );
}

/// Plan §14.1's persistent telemetry, exactly: the population and its
/// parameters, the cadence and the full evaluations a wall second, the
/// genomes' bytes and the memory headroom, the lifecycle counts, capacity
/// pauses and faults, the energy terms, and the run's identity.
#[test]
fn the_telemetry_says_what_plan_14_1_lists() {
    let lineage = key(ColorMapping::Lineage);
    let base = metrics();
    let memory = admission(GIB, 2 * GIB, 2 * GIB - 412 * MIB);
    assert_eq!(
        telemetry_lines(&telemetry(&base, &memory), Some(&READING)),
        "4,608 live · 9,124,000 parameters\n\
         controllers every tick · 128.0 full evaluations/s\n\
         genomes 38.2 MiB of 1.00 GiB · 412 MiB headroom\n\
         18,586 births · 18,255 deaths · no capacity pause · no fault\n\
         injected 1234.5 · uptake 456.7 · dissipated 789.1 · residual 1.2e-12\n\
         field-ecology-v1 · seed 42 · no checkpoint"
    );
    // In the overlay: after the clocks, before the key.
    assert_eq!(
        overlay_text(&OverlayState {
            paused: Some(false),
            shown: Some(Tick::new(1_024)),
            clock: Some(&READING),
            telemetry: Some(telemetry(&base, &memory)),
            ..state(&lineage)
        }),
        "running · tick 1,024\n128 ticks/s · 1.00× real time · 60 fps\n\
         4,608 live · 9,124,000 parameters\n\
         controllers every tick · 128.0 full evaluations/s\n\
         genomes 38.2 MiB of 1.00 GiB · 412 MiB headroom\n\
         18,586 births · 18,255 deaths · no capacity pause · no fault\n\
         injected 1234.5 · uptake 456.7 · dissipated 789.1 · residual 1.2e-12\n\
         field-ecology-v1 · seed 42 · no checkpoint\n\
         lineage ■■■■■■■■■■■■ by founder"
    );
    let line = |metrics: &Metrics,
                memory: &MemoryAdmission,
                edit: &dyn Fn(&mut Telemetry<'_>),
                clock: Option<&ClockReading>,
                at: usize| {
        let mut shown = telemetry(metrics, memory);
        edit(&mut shown);
        telemetry_lines(&shown, clock)
            .lines()
            .nth(at)
            .unwrap_or_else(|| panic!("no line {at}"))
            .to_owned()
    };
    let same = |_: &mut Telemetry<'_>| {};

    // The cadence, and no rate without a clock that counted evaluations or
    // without a live population.
    let every_four = |shown: &mut Telemetry<'_>| shown.evaluation_interval_ticks = 4;
    assert_eq!(
        line(&base, &memory, &every_four, Some(&READING), 1),
        "controllers every 4 ticks · 128.0 full evaluations/s"
    );
    assert_eq!(
        line(&base, &memory, &same, None, 1),
        "controllers every tick"
    );
    let uncounted = ClockReading {
        evaluations: None,
        evaluations_per_second: None,
        ..READING
    };
    assert_eq!(
        line(&base, &memory, &same, Some(&uncounted), 1),
        "controllers every tick"
    );
    let empty = Metrics { live: 0, ..base };
    assert_eq!(
        line(&empty, &memory, &same, Some(&READING), 0),
        "0 live · 9,124,000 parameters"
    );
    assert_eq!(
        line(&empty, &memory, &same, Some(&READING), 1),
        "controllers every tick"
    );

    // Bytes in binary units to three figures, and no headroom below zero.
    for (allocated, shown) in [
        (512, "512 B"),
        (1_023, "1023 B"),
        (1_024, "1.00 KiB"),
        (150 * MIB, "150 MiB"),
        (u64::MAX, "16.0 EiB"),
    ] {
        let sized = Metrics {
            genome_allocated_bytes: allocated,
            ..base
        };
        assert_eq!(
            line(&sized, &memory, &same, None, 2),
            format!("genomes {shown} of 1.00 GiB · 412 MiB headroom")
        );
    }
    let over = admission(GIB, GIB, 2 * GIB);
    assert_eq!(
        line(&base, &over, &same, None, 2),
        "genomes 38.2 MiB of 1.00 GiB · 0 B headroom"
    );

    // Introductions only when there are any; removal only when any was.
    let introduced = Metrics {
        tally: base.tally.map(|tally| Tally {
            introductions: 3,
            ..tally
        }),
        ledger: ledger(1_234.5, 12.0, 789.06, -3.4e-13),
        ..base
    };
    assert_eq!(
        line(&introduced, &memory, &same, None, 3),
        "18,586 births · 3 introduced · 18,255 deaths · no capacity pause · no fault"
    );
    assert_eq!(
        line(&introduced, &memory, &same, None, 4),
        "injected 1234.5 · removed 12.0 · uptake 456.7 · dissipated 789.1 · residual -3.4e-13"
    );
    // A tally from a later tick says so; without one, nothing is counted.
    let later = Metrics {
        tally: base.tally.map(|tally| Tally {
            since: Tick::new(512),
            ..tally
        }),
        ..base
    };
    assert_eq!(
        line(&later, &memory, &same, None, 3),
        "from tick 512: 18,586 births · 18,255 deaths · no capacity pause · no fault"
    );
    assert_eq!(
        line(&later, &memory, &same, None, 4),
        "injected 1234.5 · uptake 456.7 from tick 512 · dissipated 789.1 · residual 1.2e-12"
    );
    let none = Metrics {
        tally: None,
        ..base
    };
    assert_eq!(
        line(&none, &memory, &same, None, 3),
        "births and deaths not counted · no capacity pause · no fault"
    );
    assert_eq!(
        line(&none, &memory, &same, None, 4),
        "injected 1234.5 · dissipated 789.1 · residual 1.2e-12"
    );
    // Capacity pauses, a fault, a large or a zero energy.
    let once = |shown: &mut Telemetry<'_>| shown.capacity_pauses = 1;
    let often = |shown: &mut Telemetry<'_>| shown.capacity_pauses = 2_345;
    let faulted = Metrics {
        faulted: true,
        ledger: ledger(123_456.0, 0.0, 0.0, 0.0),
        ..base
    };
    assert_eq!(
        line(&base, &memory, &once, None, 3),
        "18,586 births · 18,255 deaths · 1 capacity pause · no fault"
    );
    assert_eq!(
        line(&faulted, &memory, &often, None, 3),
        "18,586 births · 18,255 deaths · 2,345 capacity pauses · a numerical fault stopped the run"
    );
    assert_eq!(
        line(&faulted, &memory, &same, None, 4),
        "injected 1.23e5 · uptake 456.7 · dissipated 0.0 · residual 0.0e0"
    );
    // A durable checkpoint's tick.
    let checkpointed = |shown: &mut Telemetry<'_>| shown.checkpoint = Some(Tick::new(3_840));
    assert_eq!(
        line(&base, &memory, &checkpointed, None, 5),
        "field-ecology-v1 · seed 42 · checkpoint at tick 3,840"
    );
}

/// FL-019's legend for the run: one key per mapping in the Studio's key
/// order, the topology key naming the run's catalog, and every key set
/// whole with a colour for each swatch.
#[test]
fn every_key_is_the_runs_and_set_whole() {
    let keys = legends(&config()).expect("the keys");
    assert_eq!(
        keys.iter().map(|(mapping, _)| *mapping).collect::<Vec<_>>(),
        ColorMapping::ALL
    );
    let font = HudFont::bundled().expect("the bundled fonts");
    for (mapping, legend) in &keys {
        assert!(legend.text.starts_with(mapping.name()), "{legend:?}");
        let squares = legend.text.chars().filter(|&c| c == HUD_SWATCH).count();
        assert_eq!(squares, legend.swatches.len(), "{legend:?}");
        assert!(squares <= HUD_MAX_SWATCHES);
        let colors: Vec<_> = legend.swatches.iter().map(|&rgb| linear(rgb)).collect();
        let laid = font
            .text_with_swatches(&legend.text, HUD_TEXT_POINTS * 2.0, &colors)
            .unwrap_or_else(|error| panic!("{legend:?}: {error}"));
        let glyphs = legend.text.chars().filter(|c| !c.is_whitespace()).count();
        assert_eq!(laid.glyphs(), glyphs, "{legend:?}");
        assert_eq!(laid.refused(), 0, "{legend:?}");
        assert_eq!(laid.swatches().len(), squares, "{legend:?}");
        assert_eq!(
            laid.swatches()
                .iter()
                .map(|(_, color)| *color)
                .collect::<Vec<_>>(),
            colors
        );
    }
    assert_eq!(
        key(ColorMapping::Topology).text,
        "topology ■ 8 ■ 32 ■ 64, 32 ■ 96, 64 ■ 128, 96"
    );
}

#[test]
fn the_inspector_panel_says_what_the_snapshot_holds() {
    let known = known();
    let lineage = key(ColorMapping::Lineage);
    let panel = "read at tick 5,120 · lineage 17 · generation 42\n\
                 energy 3.21 · +0.012/s · age 1,024 ticks\n\
                 speed 0.85/s · heading 135°\n\
                 hidden 64, 32 · 9,124 parameters\n\
                 reproduction 3.1% a tick · emission gate 0.72";
    assert_eq!(inspector_lines(Inspected::Read(&known)), panel);
    // Under the line naming the selection.
    assert_eq!(
        overlay_text(&OverlayState {
            paused: Some(true),
            shown: Some(Tick::new(5_120)),
            selected: Some(OrganismId::new(42)),
            inspected: Some(Inspected::Read(&known)),
            ..state(&lineage)
        }),
        format!("paused · tick 5,120\nlineage ■■■■■■■■■■■■ by founder\norganism 42\n{panel}")
    );
    assert_eq!(
        overlay_text(&OverlayState {
            selected: Some(OrganismId::new(42)),
            inspected: Some(Inspected::Gone),
            ..state(&lineage)
        }),
        "opening\nlineage ■■■■■■■■■■■■ by founder\norganism 42\nnot in the committed world"
    );
    // Only while an organism is selected.
    assert_eq!(
        overlay_text(&OverlayState {
            inspected: Some(Inspected::Read(&known)),
            ..state(&lineage)
        }),
        "opening\nlineage ■■■■■■■■■■■■ by founder"
    );

    // The heading turns counter-clockwise on screen from east, as the motion
    // wheel does; no heading reads -0 or 360.
    for (velocity, motion) in [
        ([2.0, 0.0], "speed 2.00/s · heading 0°"),
        ([1.0, 1e-12], "speed 1.00/s · heading 0°"),
        ([0.0, -1.0], "speed 1.00/s · heading 90°"),
        ([-1.0, 0.0], "speed 1.00/s · heading 180°"),
        ([0.0, 0.5], "speed 0.50/s · heading 270°"),
        ([0.0, 0.0], "still"),
    ] {
        let mut moving = known.clone();
        moving.organism.velocity = velocity;
        let lines = inspector_lines(Inspected::Read(&moving));
        assert_eq!(lines.lines().nth(2), Some(motion), "{velocity:?}");
    }

    // Large magnitudes in scientific notation; a sign on every rate.
    let mut large = known.clone();
    large.organism.energy = 123_456.7;
    large.organism.energy_change_rate = -250_000.0;
    let lines = inspector_lines(Inspected::Read(&large));
    assert_eq!(
        lines.lines().nth(1),
        Some("energy 1.23e5 · -2.50e5/s · age 1,024 ticks")
    );
    large.organism.energy_change_rate = 0.0;
    let lines = inspector_lines(Inspected::Read(&large));
    assert_eq!(
        lines.lines().nth(1),
        Some("energy 1.23e5 · +0.000/s · age 1,024 ticks")
    );
    let mut quiet = known;
    quiet.decoded = None;
    let lines = inspector_lines(Inspected::Read(&quiet));
    assert_eq!(lines.lines().nth(4), Some("no controller output held"));
}

#[test]
fn the_longest_overlay_is_set_whole_within_its_budget() {
    let font = HudFont::bundled().expect("the bundled fonts");
    let reading = ClockReading {
        ticks_per_second: 99_999.0,
        simulation_rate: 99_999.99,
        frames_per_second: 99_999.0,
        ..READING
    };
    // Every count at the largest written in full, every decimal one short
    // of scientific notation, and the catalog's largest controller.
    let mut longest = known();
    longest.tick = Tick::new(999_999_999_999);
    longest.age_ticks = 999_999_999_999;
    longest.hidden_widths = vec![128, 96];
    longest.parameters = 29_316;
    let organism = &mut longest.organism;
    organism.lineage = LineageId::new(999_999_999_999);
    organism.generation = Generation::new(u32::MAX);
    organism.energy = 99_999.994;
    organism.energy_change_rate = -99_999.999_4;
    organism.velocity = [70_710.67, 70_710.67];
    let decoded = longest.decoded.as_mut().expect("an output");
    decoded.attempt_probability = 0.999_96;
    decoded.emission_gate = 0.999;
    // The telemetry at its widest: each count at the largest written in
    // full, each byte count four digits wide, each energy one short of
    // scientific notation, the widest residual, a tally from a later tick,
    // a fault, and the run's identity at its longest.
    let widest = Metrics {
        live: 999_999_999_999,
        parameters: 999_999_999_999,
        genome_allocated_bytes: GIB - 1,
        ledger: ledger(99_999.94, 99_999.94, 99_999.94, -9.94e-300),
        faulted: true,
        tally: Some(Tally {
            since: Tick::new(999_999_999_999),
            births: 999_999_999_999,
            introductions: 999_999_999_999,
            deaths: 999_999_999_999,
            evaluations: 0,
            rejected_births: 0,
            uptake: 99_999.94,
        }),
        ..metrics()
    };
    let memory = admission(GIB - 1, 2 * GIB - 1, GIB);
    let reading = ClockReading {
        evaluations_per_second: Some(99_999.94 * 999_999_999_999.0),
        ..reading
    };
    let keys = legends(&config()).expect("the keys");
    let brushes = [
        Brush::Energy,
        Brush::Removal,
        Brush::Material,
        Brush::Introduction,
    ];
    let mut most = (0, 0);
    for ((mapping, legend), brush) in keys
        .iter()
        .flat_map(|key| brushes.iter().map(move |brush| (key, *brush)))
    {
        let text = overlay_text(&OverlayState {
            paused: Some(false),
            // The largest count a line writes out in full.
            shown: Some(Tick::new(999_999_999_999)),
            clock: Some(&reading),
            legend,
            brush: Some(brush),
            preview: Some(widest_preview(brush)),
            selected: Some(OrganismId::new(999_999_999_999)),
            inspected: Some(Inspected::Read(&longest)),
            telemetry: Some(Telemetry {
                metrics: &widest,
                model: "field-ecology-v1",
                seed: u64::MAX,
                evaluation_interval_ticks: u32::MAX,
                memory: &memory,
                capacity_pauses: 999_999_999_999,
                checkpoint: Some(Tick::new(999_999_999_999)),
            }),
        });
        assert_eq!(text.lines().count(), 16, "{text}");
        let colors: Vec<_> = legend.swatches.iter().map(|&rgb| linear(rgb)).collect();
        // At two drawable pixels a point, where the most vertices are.
        let laid = font
            .text_with_swatches(&text, HUD_TEXT_POINTS * 2.0, &colors)
            .unwrap_or_else(|error| panic!("{text:?}: {error}"));
        let glyphs = text.chars().filter(|c| !c.is_whitespace()).count();
        most = (
            most.0.max(laid.vertices().len()),
            most.1.max(laid.triangles().len()),
        );
        println!(
            "{mapping} {brush:?}: {glyphs} glyphs, {} vertices, {} triangles",
            laid.vertices().len(),
            laid.triangles().len()
        );
        assert_eq!(laid.glyphs(), glyphs, "{text:?}");
        assert_eq!(laid.refused(), 0, "{text:?}");
        assert_eq!(laid.left_out(), 0, "{text:?}");
        // The budget is a quarter as much again as the longest overlay.
        assert!(
            laid.vertices().len() * 5 <= HUD_MAX_VERTICES * 4,
            "{}",
            laid.vertices().len()
        );
        assert!(
            laid.triangles().len() * 5 <= HUD_MAX_TRIANGLES * 4,
            "{}",
            laid.triangles().len()
        );
        // A denser screen's finer flattening still sets all of it.
        let dense = font
            .text_with_swatches(&text, HUD_TEXT_POINTS * 3.0, &colors)
            .unwrap_or_else(|error| panic!("{text:?}: {error}"));
        assert_eq!(dense.glyphs(), glyphs, "{text:?}");
        assert_eq!(dense.left_out(), 0, "{text:?}");
    }
    println!("the longest: {} vertices, {} triangles", most.0, most.1);
}

/// A brush's preview (plan §14.3): what a click adds or removes, in the
/// model's energy units, and the region it affects, in world units, with
/// figures to the scale of the cell or the radius.
#[test]
fn a_preview_says_what_a_click_does() {
    let preview = |preview| BrushPreview {
        preview,
        recycle_fraction: 0.5,
    };
    let at = |stroke, region| preview(Preview::Stroke { stroke, region });
    let cell = [12, 30];
    let region = Some([[12.0, 13.0], [30.0, 31.0]]);
    assert_eq!(
        preview_line(&at(Stroke::Energy { cell, amount: 64.0 }, region)),
        "a click adds 64.0 energy to cell 12, 30 (x 12.00 to 13.00, y 30.00 to 31.00 world units)"
    );
    assert_eq!(
        preview_line(&at(
            Stroke::Material {
                cell,
                amount: 64.0,
                direction: [0.0; MATERIAL_CHANNELS]
            },
            region
        )),
        "a click adds 64.0 energy of the selected organism's material to cell 12, 30 \
         (x 12.00 to 13.00, y 30.00 to 31.00 world units)"
    );
    assert_eq!(
        preview_line(&at(
            Stroke::Introduction {
                cell,
                count: 8,
                energy: 2.0
            },
            region
        )),
        "a click adds 8 organisms of 2.00 energy, 16.0 in all, to cell 12, 30 \
         (x 12.00 to 13.00, y 30.00 to 31.00 world units)"
    );
    assert_eq!(
        preview_line(&at(
            Stroke::Removal {
                center: [16.4, 12.0],
                radius: 1.234_5
            },
            None
        )),
        "a click removes the organisms within 1.23 world units of the pointer: 50% of their \
         energy to the field, the rest dissipated, none out of the world"
    );
    // A finer grid's bounds, a deeper zoom's radius and a wider one keep
    // three significant figures of their scale; a large count is grouped.
    assert_eq!(
        preview_line(&at(
            Stroke::Energy {
                cell: [1_000, 3],
                amount: 0.5
            },
            Some([[250.0, 250.25], [0.75, 1.0]])
        )),
        "a click adds 0.5 energy to cell 1,000, 3 (x 250.000 to 250.250, y 0.750 to 1.000 world units)"
    );
    let removal = |radius, recycle_fraction| {
        preview_line(&BrushPreview {
            preview: Preview::Stroke {
                stroke: Stroke::Removal {
                    center: [0.0; 2],
                    radius,
                },
                region: None,
            },
            recycle_fraction,
        })
    };
    assert_eq!(
        removal(0.000_123_4, 1.0),
        "a click removes the organisms within 0.000123 world units of the pointer: 100% of \
         their energy to the field, the rest dissipated, none out of the world"
    );
    assert_eq!(
        removal(123_456.0, 0.0),
        "a click removes the organisms within 1.23e5 world units of the pointer: 0% of their \
         energy to the field, the rest dissipated, none out of the world"
    );
    assert_eq!(
        preview_line(&preview(Preview::OffWorld)),
        "a click does nothing: the pointer is off the world"
    );
    assert_eq!(
        preview_line(&preview(Preview::NoMaterial)),
        "a click does nothing: select an organism for its material"
    );
    // In the overlay: after the brush and the selection.
    let energy = key(ColorMapping::Energy);
    assert_eq!(
        overlay_text(&OverlayState {
            paused: Some(true),
            shown: Some(Tick::new(7)),
            brush: Some(Brush::Material),
            preview: Some(preview(Preview::NoMaterial)),
            selected: Some(OrganismId::new(42)),
            ..state(&energy)
        }),
        "paused · tick 7\nenergy ■ 0 ■ 1 ■ 2 ■ 4 ■ 8 ■ 16\nmaterial brush · organism 42\n\
         a click does nothing: select an organism for its material"
    );
}
