//! FL-024's metric overlay (plan §12.2) and persistent telemetry (plan
//! §14.1), FL-019's colour and field keys in it (plan §13.4) and FL-021's inspector
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
    Ancestor, Arrangement, Descendants, Disposition, ExchangeCell, LocalChemistry,
    MATERIAL_CHANNELS, NumericalProfile, OrganismSnapshot, RECENT_STEPS, RegionMeasurement,
    ReproductionOutcome, Split, Stepped, Unviable, World,
};
use flife_manim::{
    COMPONENT_PALETTE, FieldComponent, FieldMapping, HUD_MAX_SWATCHES, HUD_MAX_TRIANGLES,
    HUD_MAX_VERTICES, HUD_SWATCH, HUD_TEXT_POINTS, HudFont, Legend, affinity_rgb8,
    legend_affinities, linear,
};
use flife_render::{Metrics, Tally};
use flife_studio::{
    Brush, BrushPreview, BrushSettings, ClockReading, ColorMapping, Disk, Inspected,
    MeasurePreview, Measuring, OverlayState, Preview, Stroke, Telemetry, field_key,
    inspector_lines, legends, measuring_lines, overlay_text, preview_line, telemetry_lines,
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
        field_key: None,
        brush: None,
        preview: None,
        measuring: None,
        selected: None,
        inspected: None,
        telemetry: None,
    }
}

/// Every setting the keys can give `brush` (FL-022): from the default,
/// each amount or count down to the end of its range and up to the other,
/// or each disposition of the removal's. Each differs from the others in
/// what the brush applies.
fn every_setting(brush: Brush) -> Vec<BrushSettings> {
    if brush == Brush::Removal {
        let mut settings = vec![BrushSettings::default()];
        for _ in 1..3 {
            let next = settings[settings.len() - 1].next_disposition();
            settings.push(next);
        }
        return settings;
    }
    let mut lowest = BrushSettings::default();
    while lowest.scaled(brush, false) != lowest {
        lowest = lowest.scaled(brush, false);
    }
    let mut settings = vec![lowest];
    loop {
        let next = settings[settings.len() - 1].scaled(brush, true);
        if next == settings[settings.len() - 1] {
            return settings;
        }
        settings.push(next);
    }
}

/// The widest preview of `brush` the Studio can write with `settings`:
/// their amount and count, the largest cell indices, bounds and a
/// founder's endowment one short of scientific notation at the finest
/// places, and the finest radius.
fn widest_preview(brush: Brush, settings: BrushSettings) -> BrushPreview {
    let cell = [u32::MAX, u32::MAX];
    let stroke = match brush {
        Brush::Energy => Stroke::Energy {
            cell,
            amount: settings.amount(brush).expect("an amount"),
        },
        Brush::Material => Stroke::Material {
            cell,
            amount: settings.amount(brush).expect("an amount"),
            direction: [0.0; MATERIAL_CHANNELS],
        },
        Brush::Introduction => Stroke::Introduction {
            cell,
            count: settings.count(),
            energy: 99_999.994,
        },
        Brush::Removal => Stroke::Removal {
            center: [0.0; 2],
            radius: 1.0e-7,
            disposition: settings.disposition(),
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

/// The widest preview of the removal brush's disk being drawn, its energy
/// sent to `disposition`: the finest radius.
fn widest_drawing(disposition: Disposition) -> BrushPreview {
    BrushPreview {
        preview: Preview::Drawn {
            stroke: Some(Stroke::Removal {
                center: [0.0; 2],
                radius: 1.0e-7,
                disposition,
            }),
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

/// A real snapshot: an organism of [`config`]'s world three ticks in,
/// watched from the start, so it holds a controller output, an observation,
/// that output's activations and three steps' transactions.
fn snapshot() -> OrganismSnapshot {
    let config = config();
    let mut world = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds");
    let id = world.population().ids().next().expect("an organism");
    world.watch(Some(id));
    for _ in 0..3 {
        assert!(matches!(
            world.step().expect("steps"),
            Stepped::Published(_)
        ));
    }
    let snapshot = world
        .organism_snapshot(id)
        .expect("inspects")
        .expect("live");
    assert!(snapshot.decoded.is_some(), "it holds an output");
    assert!(snapshot.sensed.is_some(), "it was watched");
    assert!(snapshot.activations.is_some(), "it evaluated while watched");
    assert_eq!(snapshot.transactions.len(), 3, "three steps watched");
    snapshot
}

/// Set the composition inputs `Z` to `along` times the snapshot's own
/// affinity direction over its squared length, so `affinity·Z` is `along`.
fn compose(snapshot: &mut OrganismSnapshot, along: f64) {
    let affinity = *snapshot.traits.affinity();
    let length: f64 = affinity.iter().map(|a| a * a).sum();
    assert!(length > 0.0);
    let sensed = snapshot.sensed.as_mut().expect("sensed");
    for (z, a) in sensed.observation.inputs[1..=32].iter_mut().zip(affinity) {
        *z = along * a / length;
    }
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
    // Two footprint cells, half each: one harmful and one useful, so the
    // gain is half of 0.45 and the toxicity κ = 0.2 times half of 0.12.
    known.local = LocalChemistry {
        cells: vec![
            ExchangeCell {
                cell: 33,
                weight: 0.5,
                energy: 1.0,
                efficiency: -0.12,
            },
            ExchangeCell {
                cell: 34,
                weight: 0.5,
                energy: 2.0,
                efficiency: 0.45,
            },
        ],
        density: 1.5,
        request: 0.0123,
        gain: 0.225,
        toxicity: 0.012,
    };
    let decoded = known.decoded.as_mut().expect("an output");
    decoded.attempt_probability = 0.0314;
    decoded.emission_gate = 0.72;
    let sensed = known.sensed.as_mut().expect("sensed");
    sensed.tick = Tick::new(5_119);
    sensed.scale = 1.5;
    sensed.observation.inputs[0] = 0.412;
    // Up and to the right on screen.
    let side = 0.031 / 2.0_f64.sqrt();
    sensed.observation.gradients[0] = [side, -side];
    compose(&mut known, 0.112);
    let activations = known.activations.as_mut().expect("activations");
    activations.tick = Tick::new(5_120);
    activations.hidden = vec![vec![0.5, -0.98, 0.1, 0.02], vec![-0.31, 0.31, 0.97]];
    // Three steps: each gains 0.2 less 0.01 of toxicity and pays 0.004 of
    // maintenance and 0.001 of motion; the second emits 0.12 and bears a
    // child for 1.05; the newest is cooling down. It opened the first at
    // 2.0 and closed the newest at 3.214.
    let records = &mut known.transactions;
    for record in records.iter_mut() {
        record.uptake.gain = 0.2;
        record.uptake.toxicity.paid = 0.01;
        record.maintenance.as_mut().expect("maintained").cost.paid = 0.004;
        record.motion.as_mut().expect("moved").cost.paid = 0.001;
        record
            .emission
            .as_mut()
            .expect("reached emission")
            .cost
            .paid = 0.0;
        record.reproduction = Some(ReproductionOutcome::NoAttempt);
    }
    records[0].opening = 2.0;
    records[1]
        .emission
        .as_mut()
        .expect("reached emission")
        .cost
        .paid = 0.12;
    records[1].reproduction = Some(ReproductionOutcome::Born {
        child: OrganismId::new(4_321),
        split: Split {
            available: 2.6,
            child: 1.0,
            parent: 1.55,
        },
    });
    records[2].closing = 3.214;
    records[2].reproduction = Some(ReproductionOutcome::CoolingDown {
        deadline: Tick::new(5_200),
    });
    // Traced from the founding: three children, two alive, 41 descendants,
    // 17 alive; four ancestors on its line, the nearest alive, the next
    // dead where lines meet, and three more past the path.
    known.descendants = Some(Descendants {
        since: Tick::ZERO,
        children_born: 3,
        children_living: 2,
        born: 41,
        living: 17,
        path: vec![
            ancestor(455, 41, true, 204),
            ancestor(301, 38, false, 1_022),
            ancestor(120, 20, false, 2_000),
            ancestor(12, 0, false, 3_900),
        ],
        beyond: 3,
    });
    known
}

/// An ancestor on a [`Descendants`] path.
fn ancestor(organism: u64, generation: u32, alive: bool, living: u64) -> Ancestor {
    Ancestor {
        organism: OrganismId::new(organism),
        generation: Generation::new(generation),
        birth_tick: Tick::new(organism * 10),
        alive,
        born: living * 3,
        living,
    }
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
            field_key: None,
            brush: Some(Brush::Material),
            preview: None,
            measuring: None,
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

/// The field's key goes with a drawn, composed field (FL-019): the
/// composition's key in either look and either particles, none for the
/// energy field, none while the field is hidden, and none under raw
/// density, which draws no field.
#[test]
fn the_field_key_goes_with_a_drawn_composed_field() {
    use flife_manim::{Look, Particles, Style};
    let composed = Style {
        field_mapping: FieldMapping::Composition,
        ..Style::default()
    };
    let key = FieldMapping::Composition.legend();
    assert!(key.is_some());
    assert_eq!(field_key(composed), key);
    assert_eq!(
        field_key(Style {
            look: Look::Raw,
            ..composed
        }),
        key
    );
    assert_eq!(
        field_key(Style {
            particles: Particles::Density,
            ..composed
        }),
        key
    );
    assert_eq!(
        field_key(Style {
            field: false,
            ..composed
        }),
        None
    );
    assert_eq!(
        field_key(Style {
            look: Look::Raw,
            particles: Particles::Density,
            ..composed
        }),
        None
    );
    assert_eq!(field_key(Style::default()), None);
    // A chosen component's key, under the same rules.
    for channel in [0, 17, 31] {
        let mapping = FieldMapping::Component(FieldComponent::new(channel).expect("a channel"));
        let shown = Style {
            field_mapping: mapping,
            ..Style::default()
        };
        assert!(mapping.legend().is_some());
        assert_eq!(field_key(shown), mapping.legend());
        assert_eq!(
            field_key(Style {
                field: false,
                ..shown
            }),
            None
        );
    }
}

/// Plan §13.4's chosen components: a component's key names its channel and
/// shows the component palette at -1, -0.5, 0, 0.5 and 1, its labels at
/// the ends and the middle.
#[test]
fn a_component_key_names_its_channel_and_the_palette() {
    let seven = FieldMapping::Component(FieldComponent::new(7).expect("a channel"))
        .legend()
        .expect("a key");
    assert_eq!(
        seven.text,
        "field channel 7 / energy ■ -1 ■ ■ 0 ■ ■ +1, darker is less energy"
    );
    assert_eq!(seven.swatches, COMPONENT_PALETTE.to_vec());
    assert_eq!(
        seven.text.chars().filter(|&c| c == HUD_SWATCH).count(),
        seven.swatches.len()
    );
    let last = FieldMapping::Component(FieldComponent::new(31).expect("a channel"))
        .legend()
        .expect("a key");
    assert!(last.text.starts_with("field channel 31 / energy "));
    assert_eq!(last.swatches, seven.swatches);
}

/// Plan §13.4's "Local field": the composition mapping's key follows the
/// colour mapping's, a line of its own, its swatches the affinity key's
/// colours (`none`, then six directions) after the colour key's, set whole.
/// The energy mapping, which the field palette explains, has none.
#[test]
fn the_field_key_follows_the_colour_key() {
    assert_eq!(FieldMapping::Energy.legend(), None);
    let field = FieldMapping::Composition.legend().expect("a key");
    assert_eq!(
        field.text,
        "field composition ■ none ■■■■■■ as affinity, darker is less energy"
    );
    assert_eq!(
        field.swatches,
        legend_affinities().map(affinity_rgb8).to_vec(),
        "the affinity key's colours"
    );
    assert_eq!(
        field.swatches,
        key(ColorMapping::Affinity).swatches,
        "the colours the affinity mapping's key shows"
    );
    let lineage = key(ColorMapping::Lineage);
    let text = overlay_text(&OverlayState {
        paused: Some(false),
        shown: Some(Tick::new(1_024)),
        field_key: Some(&field),
        brush: Some(Brush::Energy),
        ..state(&lineage)
    });
    assert_eq!(
        text,
        "running · tick 1,024\nlineage ■■■■■■■■■■■■ by founder\n\
         field composition ■ none ■■■■■■ as affinity, darker is less energy\nenergy brush"
    );
    let swatches: Vec<_> = lineage.swatches.iter().chain(&field.swatches).collect();
    assert_eq!(
        text.chars().filter(|&c| c == HUD_SWATCH).count(),
        swatches.len()
    );
    assert!(swatches.len() <= HUD_MAX_SWATCHES);
    let colors: Vec<_> = swatches.iter().map(|&&rgb| linear(rgb)).collect();
    let font = HudFont::bundled().expect("the bundled fonts");
    let laid = font
        .text_with_swatches(&text, HUD_TEXT_POINTS * 2.0, &colors)
        .expect("laid out");
    assert_eq!(laid.refused(), 0);
    assert_eq!(laid.left_out(), 0);
    assert_eq!(
        laid.swatches()
            .iter()
            .map(|(_, color)| *color)
            .collect::<Vec<_>>(),
        colors,
        "the field key's swatches take the colours after the colour key's"
    );
}

#[test]
fn the_inspector_panel_says_what_the_snapshot_holds() {
    let known = known();
    let lineage = key(ColorMapping::Lineage);
    let panel = "read at tick 5,120 · lineage 17 · generation 42\n\
                 energy 3.21 · +0.012/s · age 1,024 ticks\n\
                 speed 0.85/s · heading 135°\n\
                 here: η -0.120 to +0.450 · gain 0.225 · toxicity 0.012 a unit · asks 0.0123 a tick\n\
                 hidden 64, 32 · 9,124 parameters\n\
                 reproduction 3.1% a tick · emission gate 0.72\n\
                 sensed at tick 5,119 · scale 1.50 · field 0.412\n\
                 gradient 0.031 toward 45° · affinity·Z +0.112\n\
                 activations at tick 5,120 · max |a| 0.98, 0.97 · mean |a| 0.40, 0.53\n\
                 last 3 steps: uptake +0.570 · maintenance -0.012 · motion -0.003\n\
                 emission -0.120 · 1 birth -1.050 · net +1.214 · cooling to tick 5,200\n\
                 children 3 born, 2 alive · descendants 41 born, 17 alive\n\
                 ancestors 455 (gen 41, 204 alive) ← 301 (gen 38, dead, 1,022 alive) ← 5 more";
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

    // One efficiency when the footprint's cells agree.
    let mut even = known.clone();
    for cell in &mut even.local.cells {
        cell.efficiency = 0.45;
    }
    let lines = inspector_lines(Inspected::Read(&even));
    assert_eq!(
        lines.lines().nth(3),
        Some("here: η +0.450 · gain 0.225 · toxicity 0.012 a unit · asks 0.0123 a tick")
    );

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
    let mut quiet = known.clone();
    quiet.decoded = None;
    let lines = inspector_lines(Inspected::Read(&quiet));
    assert_eq!(lines.lines().nth(5), Some("no controller output held"));

    // The gradient's direction as the heading's; a negative alignment; no
    // gradient; nothing recorded yet.
    let mut sensing = known.clone();
    sensing
        .sensed
        .as_mut()
        .expect("sensed")
        .observation
        .gradients[0] = [-0.5, 0.0];
    compose(&mut sensing, -0.05);
    let lines = inspector_lines(Inspected::Read(&sensing));
    assert_eq!(
        lines.lines().nth(7),
        Some("gradient 0.500 toward 180° · affinity·Z -0.050")
    );
    sensing
        .sensed
        .as_mut()
        .expect("sensed")
        .observation
        .gradients[0] = [0.0, 0.0];
    let lines = inspector_lines(Inspected::Read(&sensing));
    assert_eq!(
        lines.lines().nth(7),
        Some("no gradient · affinity·Z -0.050")
    );

    // One step; no births, and two; each reproduction outcome.
    let transactions = |edit: &dyn Fn(&mut OrganismSnapshot)| {
        let mut edited = known.clone();
        edit(&mut edited);
        inspector_lines(Inspected::Read(&edited))
            .lines()
            .skip(9)
            .take(2)
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        transactions(&|snapshot| {
            snapshot.transactions.truncate(1);
            snapshot.transactions[0].closing = 3.0;
        }),
        [
            "last step: uptake +0.190 · maintenance -0.004 · motion -0.001",
            "emission -0.000 · no births · net +1.000 · no attempt"
        ]
    );
    let born = known.transactions[1].reproduction;
    let outcomes = [
        (born, "2 births -2.100 · net +1.214 · bore organism 4,321"),
        (
            Some(ReproductionOutcome::Unviable(
                Unviable::BelowAttemptEnergy {
                    available: 1.0,
                    minimum: 2.0,
                },
            )),
            "1 birth -1.050 · net +1.214 · attempt unviable",
        ),
        (
            Some(ReproductionOutcome::Rejected {
                split: Split {
                    available: 2.6,
                    child: 1.0,
                    parent: 1.55,
                },
                candidates: 3,
            }),
            "1 birth -1.050 · net +1.214 · 3 candidates rejected",
        ),
        (None, "1 birth -1.050 · net +1.214 · no reproduction stage"),
    ];
    for (outcome, line) in outcomes {
        assert_eq!(
            transactions(&|snapshot| snapshot.transactions[2].reproduction = outcome)[1],
            format!("emission -0.120 · {line}"),
            "{outcome:?}"
        );
    }

    // Descendants: births counted since a later tick; no ancestor more than
    // the panel names; none at all; and a world that does not trace them.
    let descendants = |edit: &dyn Fn(&mut Descendants)| {
        let mut edited = known.clone();
        edit(edited.descendants.as_mut().expect("traced"));
        inspector_lines(Inspected::Read(&edited))
            .lines()
            .skip(11)
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        descendants(&|descendants| {
            descendants.since = Tick::new(1_024);
            descendants.path.truncate(2);
            descendants.beyond = 0;
        }),
        [
            "children 3 born, 2 alive · descendants 41 born, 17 alive since tick 1,024",
            "ancestors 455 (gen 41, 204 alive) ← 301 (gen 38, dead, 1,022 alive)"
        ]
    );
    assert_eq!(
        descendants(&|descendants| {
            descendants.path.truncate(1);
            descendants.beyond = 0;
        })[1],
        "ancestors 455 (gen 41, 204 alive)"
    );
    assert_eq!(
        descendants(&|descendants| {
            descendants.path.clear();
            descendants.beyond = 0;
        })[1],
        "ancestors: none alive or where lines meet"
    );

    let mut unrecorded = known;
    unrecorded.sensed = None;
    unrecorded.activations = None;
    unrecorded.transactions.clear();
    unrecorded.descendants = None;
    let lines = inspector_lines(Inspected::Read(&unrecorded));
    assert_eq!(
        lines.lines().skip(6).collect::<Vec<_>>(),
        [
            "sensed from its next tick",
            "activations at its next evaluation",
            "transactions from its next tick",
            "descendants not traced"
        ]
    );
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
    // What it sensed and its activations at their widest: every decimal one
    // short of scientific notation and negative, a three-digit direction,
    // and two layers each at the largest magnitude.
    let sensed = longest.sensed.as_mut().expect("sensed");
    sensed.tick = Tick::new(999_999_999_999);
    sensed.scale = 99_999.994;
    sensed.observation.inputs[0] = -99_999.999_4;
    sensed.observation.gradients[0] = [70_710.0, 70_710.0];
    compose(&mut longest, -99_999.999_4);
    let activations = longest.activations.as_mut().expect("activations");
    activations.tick = Tick::new(999_999_999_999);
    activations.hidden = vec![vec![-1.0; 128], vec![-1.0; 96]];
    // Every step the watch keeps, each sum one short of scientific notation
    // and negative, a birth in every step but the newest, and the newest
    // cooling down to the largest tick written in full.
    let template = longest.transactions[0];
    let mut records = vec![template; RECENT_STEPS];
    for record in &mut records {
        record.uptake.gain = 0.0;
        record.uptake.toxicity.paid = 0.0;
        record.maintenance.as_mut().expect("maintained").cost.paid = 0.0;
        record.motion.as_mut().expect("moved").cost.paid = 0.0;
        record
            .emission
            .as_mut()
            .expect("reached emission")
            .cost
            .paid = 0.0;
        record.reproduction = Some(ReproductionOutcome::Born {
            child: OrganismId::new(999_999_999_999),
            split: Split {
                available: 99_999.999_4 / 127.0,
                child: 0.0,
                parent: 0.0,
            },
        });
    }
    let first = &mut records[0];
    first.uptake.toxicity.paid = 99_999.999_4;
    first.maintenance.as_mut().expect("maintained").cost.paid = 99_999.999_4;
    first.motion.as_mut().expect("moved").cost.paid = 99_999.999_4;
    first.emission.as_mut().expect("reached emission").cost.paid = 99_999.999_4;
    first.opening = 99_999.999_4;
    let newest = &mut records[RECENT_STEPS - 1];
    newest.closing = 0.0;
    newest.reproduction = Some(ReproductionOutcome::CoolingDown {
        deadline: Tick::new(999_999_999_999),
    });
    longest.transactions = records;
    // What the field offers it at its widest: the two efficiencies at the
    // width of their bounds, and the toxicity and the request one short of
    // scientific notation.
    longest.local.cells[0].efficiency = -0.999_6;
    longest.local.cells[1].efficiency = 0.999_6;
    longest.local.gain = 0.999_6;
    longest.local.toxicity = 99_999.999_4;
    longest.local.request = 99_999.999_94;
    // Its descendants at their widest: every count and identity the
    // largest written in full, births counted from a later tick, two dead
    // ancestors at the largest generation, and more past them.
    let widest = |alive| Ancestor {
        organism: OrganismId::new(999_999_999_999),
        generation: Generation::new(u32::MAX),
        birth_tick: Tick::new(999_999_999_999),
        alive,
        born: 999_999_999_999,
        living: 999_999_999_999,
    };
    longest.descendants = Some(Descendants {
        since: Tick::new(999_999_999_999),
        children_born: 999_999_999_999,
        children_living: 999_999_999_999,
        born: 999_999_999_999,
        living: 999_999_999_999,
        path: vec![widest(false), widest(false)],
        beyond: 999_999_999_999,
    });
    let panel = inspector_lines(Inspected::Read(&longest));
    assert_eq!(
        panel.lines().nth(3),
        Some(
            "here: η -1.000 to +1.000 · gain 1.000 · toxicity 99999.999 a unit · asks 99999.9999 \
             a tick"
        )
    );
    assert_eq!(
        panel.lines().skip(9).collect::<Vec<_>>(),
        [
            "last 128 steps: uptake -99999.999 · maintenance -99999.999 · motion -99999.999",
            "emission -99999.999 · 127 births -99999.999 · net -99999.999 · cooling to tick \
             999,999,999,999",
            "children 999,999,999,999 born, 999,999,999,999 alive · descendants \
             999,999,999,999 born, 999,999,999,999 alive since tick 999,999,999,999",
            "ancestors 999,999,999,999 (gen 4,294,967,295, dead, 999,999,999,999 alive) ← \
             999,999,999,999 (gen 4,294,967,295, dead, 999,999,999,999 alive) ← 999,999,999,999 \
             more"
        ]
    );
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
    // The field key that lays out the most, of every field mapping's.
    let field = FieldMapping::ALL
        .into_iter()
        .filter_map(FieldMapping::legend)
        .max_by_key(|key| {
            let colors: Vec<_> = key.swatches.iter().map(|&rgb| linear(rgb)).collect();
            let laid = font
                .text_with_swatches(&key.text, HUD_TEXT_POINTS * 2.0, &colors)
                .unwrap_or_else(|error| panic!("{:?}: {error}", key.text));
            (laid.triangles().len(), laid.vertices().len())
        })
        .expect("a field key");
    println!("the widest field key: {}", field.text);
    // The measuring tool's widest (plan §14.3): the finest radius, and
    // every count and energy of what it holds at their widest.
    let disk = Disk {
        center: [99_999.999_999_999; 2],
        radius: 1.0e-7,
    };
    let held = RegionMeasurement {
        tick: Tick::new(999_999_999_999),
        center: disk.center,
        radius: disk.radius,
        organisms: 999_999_999_999,
        organism_energy: 99_999.94,
        lineages: 999_999_999_999,
        cells: 999_999_999_999,
        field_energy: 99_999.94,
    };
    let placed = Measuring {
        preview: MeasurePreview::Placed(disk),
        measurement: Some(&held),
    };
    assert_eq!(
        measuring_lines(&placed),
        "radius 0.000000100 world units at tick 999,999,999,999: 999,999,999,999 organisms of \
         99999.9 energy, 999,999,999,999 lineages\nfield 99999.9 energy in 999,999,999,999 cells"
    );
    let clicked = Measuring {
        preview: MeasurePreview::Click(Some(disk)),
        measurement: None,
    };
    // Each brush's widest click at every amount, count and disposition its
    // keys set, the widest removal disk being drawn at each disposition,
    // and the measuring tool's widest click and placed disk, which says a
    // line more: the brushes and the tool are never armed together. Each
    // with the lines the overlay then has.
    type Tool<'a> = (
        Option<Brush>,
        Option<BrushPreview>,
        Option<Measuring<'a>>,
        usize,
    );
    let tools: Vec<Tool<'_>> = [
        Brush::Energy,
        Brush::Removal,
        Brush::Material,
        Brush::Introduction,
    ]
    .into_iter()
    .flat_map(|brush| {
        every_setting(brush)
            .into_iter()
            .map(move |settings| (Some(brush), Some(widest_preview(brush, settings)), None, 25))
    })
    .chain(Disposition::ALL.map(|disposition| {
        (
            Some(Brush::Removal),
            Some(widest_drawing(disposition)),
            None,
            25,
        )
    }))
    .chain([
        (None, None, Some(clicked), 25),
        (None, None, Some(placed), 26),
    ])
    .collect();
    // 13 amounts of each energy brush, 7 counts, 3 dispositions clicked and
    // 3 drawn, and the measuring tool's 2.
    assert_eq!(tools.len(), 13 + 3 + 13 + 7 + 3 + 2);
    let mut most = (0, 0);
    for ((mapping, legend), (brush, preview, measuring, lines)) in keys
        .iter()
        .flat_map(|key| tools.iter().map(move |tool| (key, *tool)))
    {
        let text = overlay_text(&OverlayState {
            paused: Some(false),
            // The largest count a line writes out in full.
            shown: Some(Tick::new(999_999_999_999)),
            clock: Some(&reading),
            legend,
            field_key: Some(&field),
            brush,
            preview,
            measuring,
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
        assert_eq!(text.lines().count(), lines, "{text}");
        let colors: Vec<_> = legend
            .swatches
            .iter()
            .chain(&field.swatches)
            .map(|&rgb| linear(rgb))
            .collect();
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
            "{mapping} {brush:?} {:?}: {glyphs} glyphs, {} vertices, {} triangles",
            measuring.map(|measuring| measuring.preview),
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

/// A brush's preview (plan §14.3): what a click adds or removes, or the
/// release of a removal disk being drawn, in the model's energy units, and
/// the region it affects, in world units, with figures to the scale of the
/// cell or the radius.
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
    let removed = |disposition| {
        preview_line(&at(
            Stroke::Removal {
                center: [16.4, 12.0],
                radius: 1.234_5,
                disposition,
            },
            None,
        ))
    };
    assert_eq!(
        removed(Disposition::Recycle),
        "a click removes the organisms within 1.23 world units of the pointer: 50% of their \
         energy to the field, the rest dissipated, none out of the world"
    );
    // Dissipated, or taken out of the modeled world (FL-022's disposition):
    // none of it to the field, whatever the recycled share.
    assert_eq!(
        removed(Disposition::Dissipate),
        "a click removes the organisms within 1.23 world units of the pointer: all their \
         energy dissipated, none to the field, none out of the world"
    );
    assert_eq!(
        removed(Disposition::Remove),
        "a click removes the organisms within 1.23 world units of the pointer: all their \
         energy out of the world, none to the field, none dissipated"
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
                    disposition: Disposition::Recycle,
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
    // A removal disk being drawn: what its release does, about the press.
    assert_eq!(
        preview_line(&preview(Preview::Drawn {
            stroke: Some(Stroke::Removal {
                center: [16.4, 12.0],
                radius: 4.567_8,
                disposition: Disposition::Recycle,
            })
        })),
        "a release removes the organisms within 4.57 world units of the press: 50% of their \
         energy to the field, the rest dissipated, none out of the world"
    );
    assert_eq!(
        preview_line(&preview(Preview::Drawn {
            stroke: Some(Stroke::Removal {
                center: [16.4, 12.0],
                radius: 4.567_8,
                disposition: Disposition::Remove,
            })
        })),
        "a release removes the organisms within 4.57 world units of the press: all their \
         energy out of the world, none to the field, none dissipated"
    );
    assert_eq!(
        preview_line(&preview(Preview::Drawn { stroke: None })),
        "a release does nothing: the press was off the world"
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

/// The measuring tool (plan §14.3's temporary measurement region): what a
/// click or a release would measure, and what the placed disk holds, only
/// once the measurement of that disk is in.
#[test]
fn a_measurement_says_what_its_disk_holds() {
    fn lines(preview: MeasurePreview, measurement: Option<&RegionMeasurement>) -> String {
        measuring_lines(&Measuring {
            preview,
            measurement,
        })
    }
    let disk = Disk {
        center: [12.3, 45.6],
        radius: 4.56,
    };
    assert_eq!(
        lines(
            MeasurePreview::Click(Some(Disk {
                center: [16.4, 12.0],
                radius: 1.234_5
            })),
            None
        ),
        "a click measures within 1.23 world units of the pointer; a drag draws the disk"
    );
    assert_eq!(
        lines(MeasurePreview::Click(None), None),
        "a click measures nothing: the pointer is off the world"
    );
    assert_eq!(
        lines(MeasurePreview::Release(Some(disk)), None),
        "a release measures within 4.56 world units of the press"
    );
    assert_eq!(
        lines(MeasurePreview::Release(None), None),
        "a release measures nothing: the press was off the world"
    );
    assert_eq!(
        lines(MeasurePreview::Placed(disk), None),
        "radius 4.56 world units: measuring"
    );
    let held = RegionMeasurement {
        tick: Tick::new(5_120),
        center: disk.center,
        radius: disk.radius,
        organisms: 17,
        organism_energy: 41.23,
        lineages: 3,
        cells: 1_064,
        field_energy: 812.46,
    };
    let full = "radius 4.56 world units at tick 5,120: 17 organisms of 41.2 energy, 3 \
                lineages\nfield 812.5 energy in 1,064 cells";
    assert_eq!(lines(MeasurePreview::Placed(disk), Some(&held)), full);
    // The measurement of another disk, as a disk placed again has until its
    // own answers, is not shown: neither another centre nor another radius.
    for other in [
        RegionMeasurement {
            center: [12.3, 45.7],
            ..held
        },
        RegionMeasurement {
            radius: 4.57,
            ..held
        },
    ] {
        assert_eq!(
            lines(MeasurePreview::Placed(disk), Some(&other)),
            "radius 4.56 world units: measuring"
        );
    }
    // While a disk is drawn or a click is offered, no measurement shows.
    assert_eq!(
        lines(MeasurePreview::Release(Some(disk)), Some(&held)),
        "a release measures within 4.56 world units of the press"
    );
    // A finer radius keeps three significant figures; a wide one goes to
    // scientific notation from 100,000.
    assert_eq!(
        lines(
            MeasurePreview::Placed(Disk {
                center: [0.25, 1.5],
                radius: 0.012_34
            }),
            None
        ),
        "radius 0.0123 world units: measuring"
    );
    assert_eq!(
        lines(
            MeasurePreview::Click(Some(Disk {
                center: [123_456.0, 7.0],
                radius: 123_456.0
            })),
            None
        ),
        "a click measures within 1.23e5 world units of the pointer; a drag draws the disk"
    );
    // In the overlay: `measuring` before the selection, as a brush's name
    // is, and its lines where a brush's preview would be.
    let energy = key(ColorMapping::Energy);
    assert_eq!(
        overlay_text(&OverlayState {
            paused: Some(true),
            shown: Some(Tick::new(7)),
            measuring: Some(Measuring {
                preview: MeasurePreview::Placed(disk),
                measurement: Some(&held),
            }),
            selected: Some(OrganismId::new(42)),
            ..state(&energy)
        }),
        format!(
            "paused · tick 7\nenergy ■ 0 ■ 1 ■ 2 ■ 4 ■ 8 ■ 16\nmeasuring · organism 42\n{full}"
        )
    );
    // Put away, it says nothing.
    assert_eq!(
        overlay_text(&OverlayState {
            paused: Some(true),
            shown: Some(Tick::new(7)),
            ..state(&energy)
        }),
        "paused · tick 7\nenergy ■ 0 ■ 1 ■ 2 ■ 4 ■ 8 ■ 16"
    );
}
