//! FL-024's metric overlay (plan §12.2): the text the Studio draws over the
//! world, exactly, and that the bundled font sets all of it within the
//! overlay's budget. No window opens.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use flife_core::Tick;
use flife_core::ids::OrganismId;
use flife_manim::{HUD_MAX_TRIANGLES, HUD_MAX_VERTICES, HUD_TEXT_POINTS, HudFont};
use flife_studio::{Brush, ClockReading, OverlayState, overlay_text};

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
    }
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
fn the_longest_overlay_is_set_whole_within_its_budget() {
    let font = HudFont::bundled().expect("the bundled fonts");
    let reading = ClockReading {
        ticks_per_second: 123_456_789.0,
        simulation_rate: 99_999.99,
        frames_per_second: 1_000.0,
        ..READING
    };
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
        });
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
        // The budget is half as much again as the longest overlay.
        assert!(
            laid.vertices().len() * 3 <= HUD_MAX_VERTICES * 2,
            "{}",
            laid.vertices().len()
        );
        assert!(
            laid.triangles().len() * 3 <= HUD_MAX_TRIANGLES * 2,
            "{}",
            laid.triangles().len()
        );
    }
}
