//! FL-024's metric overlay (plan §12.2) and FL-021's inspector panel in it
//! (plan §14.2): the text the Studio draws over the world, exactly, and that
//! the bundled font sets all of it within the overlay's budget. No window
//! opens.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use flife_core::ids::{Generation, LineageId, OrganismId};
use flife_core::{Tick, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, OrganismSnapshot, Stepped, World};
use flife_manim::{HUD_MAX_TRIANGLES, HUD_MAX_VERTICES, HUD_TEXT_POINTS, HudFont};
use flife_studio::{Brush, ClockReading, Inspected, OverlayState, inspector_lines, overlay_text};

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
};

fn state() -> OverlayState<'static> {
    OverlayState {
        paused: None,
        shown: None,
        clock: None,
        mapping: "lineage",
        brush: None,
        selected: None,
        inspected: None,
    }
}

/// A real snapshot: an organism of the preset at 32 × 32 world units with
/// 96 founders, three ticks in, so it holds a controller output.
fn snapshot() -> OrganismSnapshot {
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
    let config = ValidatedConfig::from_toml(&lines.join("\n")).expect("a valid configuration");
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
    assert_eq!(overlay_text(&state()), "opening\nlineage");
    assert_eq!(
        overlay_text(&OverlayState {
            paused: Some(false),
            shown: Some(Tick::new(1_024)),
            clock: Some(&READING),
            ..state()
        }),
        "running · tick 1,024\n128 ticks/s · 1.00× real time · 60 fps\nlineage"
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
            mapping: "energy",
            brush: Some(Brush::Material),
            selected: Some(OrganismId::new(1_234_567)),
            inspected: None,
        }),
        "paused · tick 7\n30.0 ticks/s · 0.23× real time · 60 fps\nenergy · material brush · organism 1,234,567"
    );
    assert_eq!(
        overlay_text(&OverlayState {
            paused: Some(true),
            shown: Some(Tick::new(1_000_000)),
            brush: Some(Brush::Removal),
            ..state()
        }),
        "paused · tick 1,000,000\nlineage · removal brush"
    );
}

#[test]
fn the_inspector_panel_says_what_the_snapshot_holds() {
    let known = known();
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
            ..state()
        }),
        format!("paused · tick 5,120\nlineage · organism 42\n{panel}")
    );
    assert_eq!(
        overlay_text(&OverlayState {
            selected: Some(OrganismId::new(42)),
            inspected: Some(Inspected::Gone),
            ..state()
        }),
        "opening\nlineage · organism 42\nnot in the committed world"
    );
    // Only while an organism is selected.
    assert_eq!(
        overlay_text(&OverlayState {
            inspected: Some(Inspected::Read(&known)),
            ..state()
        }),
        "opening\nlineage"
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
        ticks_per_second: 123_456_789.0,
        simulation_rate: 99_999.99,
        frames_per_second: 1_000.0,
        ..READING
    };
    // Every count at its largest, every decimal one short of scientific
    // notation, and the catalog's largest controller.
    let mut longest = known();
    longest.tick = Tick::new(u64::MAX);
    longest.age_ticks = u64::MAX;
    longest.hidden_widths = vec![128, 96];
    longest.parameters = 29_316;
    let organism = &mut longest.organism;
    organism.lineage = LineageId::new(u64::MAX);
    organism.generation = Generation::new(u32::MAX);
    organism.energy = 99_999.994;
    organism.energy_change_rate = -99_999.999_4;
    organism.velocity = [70_710.67, 70_710.67];
    let decoded = longest.decoded.as_mut().expect("an output");
    decoded.attempt_probability = 0.999_96;
    decoded.emission_gate = 0.999;
    for brush in [
        Brush::Energy,
        Brush::Removal,
        Brush::Material,
        Brush::Introduction,
    ] {
        let text = overlay_text(&OverlayState {
            paused: Some(false),
            shown: Some(Tick::new(u64::MAX)),
            clock: Some(&reading),
            mapping: "chemical-affinity",
            brush: Some(brush),
            selected: Some(OrganismId::new(u64::MAX)),
            inspected: Some(Inspected::Read(&longest)),
        });
        assert_eq!(text.lines().count(), 8, "{text}");
        // At two drawable pixels a point, where the most vertices are.
        let laid = font
            .text(&text, HUD_TEXT_POINTS * 2.0)
            .unwrap_or_else(|error| panic!("{text:?}: {error}"));
        let glyphs = text.chars().filter(|c| !c.is_whitespace()).count();
        println!(
            "{brush:?}: {glyphs} glyphs, {} vertices, {} triangles",
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
            .text(&text, HUD_TEXT_POINTS * 3.0)
            .unwrap_or_else(|error| panic!("{text:?}: {error}"));
        println!(
            "{brush:?} at 3x: {} vertices, {} triangles",
            dense.vertices().len(),
            dense.triangles().len()
        );
        assert_eq!(dense.glyphs(), glyphs, "{text:?}");
        assert_eq!(dense.left_out(), 0, "{text:?}");
    }
}
