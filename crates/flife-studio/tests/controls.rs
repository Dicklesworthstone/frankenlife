//! The Studio's controls: typed native input to intents and presentation
//! state, as a pure state machine (no window).

use flife_manim::Viewport;
use flife_studio::{Controls, Intent, KEY_ESCAPE, KEY_PERIOD, KEY_RIGHT, KEY_SPACE, KEY_W};
use fmn_render::metal::{
    ButtonSet, InputEvent, LogicalPoint, Modifiers, PointerButton, ScrollUnit, SurfaceMetrics,
};

fn key(code: u16, pressed: bool, repeat: bool, modifiers: Modifiers) -> InputEvent {
    InputEvent::Key {
        code,
        text: String::new(),
        pressed,
        repeat,
        modifiers,
    }
}

fn tap(controls: &mut Controls, code: u16) -> Vec<Intent> {
    [true, false]
        .into_iter()
        .filter_map(|pressed| controls.apply(&key(code, pressed, false, Modifiers::NONE)))
        .collect()
}

fn metrics(width: f64, height: f64) -> SurfaceMetrics {
    SurfaceMetrics {
        logical_width: width,
        logical_height: height,
        backing_scale: 2.0,
        drawable_width: 1,
        drawable_height: 1,
    }
}

fn point(x: f64, y: f64) -> LogicalPoint {
    LogicalPoint { x, y }
}

#[test]
fn space_toggles_and_ignores_auto_repeat() {
    let mut controls = Controls::new();
    assert!(!controls.running());
    assert_eq!(tap(&mut controls, KEY_SPACE), [Intent::Resume]);
    assert!(controls.running());
    assert_eq!(
        controls.apply(&key(KEY_SPACE, true, true, Modifiers::NONE)),
        None
    );
    assert_eq!(tap(&mut controls, KEY_SPACE), [Intent::Pause]);
    assert!(!controls.running());
}

#[test]
fn stepping_only_while_paused_and_held_keys_repeat_steps() {
    let mut controls = Controls::new();
    assert_eq!(tap(&mut controls, KEY_RIGHT), [Intent::Step]);
    assert_eq!(tap(&mut controls, KEY_PERIOD), [Intent::Step]);
    assert_eq!(
        controls.apply(&key(KEY_RIGHT, true, true, Modifiers::NONE)),
        Some(Intent::Step)
    );
    tap(&mut controls, KEY_SPACE);
    assert!(tap(&mut controls, KEY_RIGHT).is_empty());
    assert!(tap(&mut controls, KEY_PERIOD).is_empty());
    // The session stopping on its own makes stepping possible again.
    controls.set_running(false);
    assert_eq!(tap(&mut controls, KEY_RIGHT), [Intent::Step]);
}

#[test]
fn escape_or_command_w_closes_once() {
    let mut controls = Controls::new();
    assert!(tap(&mut controls, KEY_W).is_empty());
    let command = Modifiers {
        command: true,
        ..Modifiers::NONE
    };
    assert_eq!(
        controls.apply(&key(KEY_W, true, false, command)),
        Some(Intent::Close)
    );
    assert!(controls.close_requested());
    assert!(tap(&mut controls, KEY_ESCAPE).is_empty());
    let mut other = Controls::new();
    assert_eq!(tap(&mut other, KEY_ESCAPE), [Intent::Close]);
}

#[test]
fn buttons_follow_presses_motion_and_capture_loss() {
    let mut controls = Controls::new();
    let pressed = |button, pressed| InputEvent::Button {
        button,
        pressed,
        position: point(5.0, 6.0),
        clicks: 1,
        modifiers: Modifiers::NONE,
    };
    controls.apply(&pressed(PointerButton::Primary, true));
    controls.apply(&pressed(PointerButton::Secondary, true));
    assert!(controls.buttons().contains(PointerButton::Primary));
    assert!(controls.buttons().contains(PointerButton::Secondary));
    controls.apply(&pressed(PointerButton::Secondary, false));
    assert_eq!(
        controls.buttons(),
        ButtonSet::EMPTY.with(PointerButton::Primary)
    );
    controls.apply(&InputEvent::PointerCaptureLost {
        released: ButtonSet::EMPTY.with(PointerButton::Primary),
    });
    assert!(controls.buttons().is_empty());
    // Motion reports the buttons AppKit says are held.
    controls.apply(&InputEvent::PointerMoved {
        position: point(7.0, 8.0),
        buttons: ButtonSet::EMPTY.with(PointerButton::Middle),
        modifiers: Modifiers::NONE,
    });
    assert_eq!(controls.pointer(), Some(point(7.0, 8.0)));
    assert!(controls.buttons().contains(PointerButton::Middle));
    let tally = controls.tally();
    assert_eq!(
        (tally.buttons, tally.capture_lost, tally.pointer_moves),
        (3, 1, 1)
    );
}

#[test]
fn focus_geometry_and_hover_follow_the_window() {
    let mut controls = Controls::new();
    assert_eq!(controls.drawable_size(), None);
    controls.apply(&InputEvent::FocusGained);
    assert!(controls.focused());
    controls.apply(&InputEvent::Resized(metrics(800.4, 449.6)));
    assert_eq!(controls.drawable_size(), Some([800, 450]));
    let viewport = Viewport::fit([128.0, 128.0], [800, 450]).ok();
    let Some(viewport) = viewport else {
        return;
    };
    // Off the world: nothing hovered.
    controls.apply(&InputEvent::Scroll {
        delta: [0.0, -3.0],
        unit: ScrollUnit::Points,
        position: point(10.0, 10.0),
        modifiers: Modifiers::NONE,
    });
    assert_eq!(controls.hover(&viewport), None);
    let at = viewport.world_to_drawable([64.0, 32.0]);
    controls.apply(&InputEvent::PointerMoved {
        position: point(at[0] * 800.4 / 800.0, at[1] * 449.6 / 450.0),
        buttons: ButtonSet::EMPTY,
        modifiers: Modifiers::NONE,
    });
    let hover = controls.hover(&viewport).unwrap_or([f64::NAN; 2]);
    assert!(
        (hover[0] - 64.0).abs() < 1e-9 && (hover[1] - 32.0).abs() < 1e-9,
        "{hover:?}"
    );
    controls.apply(&InputEvent::FocusLost);
    assert!(!controls.focused());
    controls.apply(&InputEvent::Resized(metrics(0.4, 100.0)));
    assert_eq!(controls.drawable_size(), None);
    let tally = controls.tally();
    assert_eq!(
        (
            tally.focus_gained,
            tally.focus_lost,
            tally.resizes,
            tally.scrolls
        ),
        (1, 1, 2, 1)
    );
}

#[test]
fn only_the_current_runs_end_stops_the_controls() {
    use flife_core::Tick;
    use flife_studio::{End, Status};
    let mut controls = Controls::new();
    tap(&mut controls, KEY_SPACE);
    controls.follow(&Status::Running {
        run: 1,
        from: Tick::ZERO,
    });
    assert_eq!(controls.run(), Some(1));
    // Pause, then resume before run 1's end arrives.
    tap(&mut controls, KEY_SPACE);
    controls.follow(&Status::Paused { at: Tick::new(4) });
    assert!(!controls.running());
    tap(&mut controls, KEY_SPACE);
    controls.follow(&Status::Running {
        run: 2,
        from: Tick::new(4),
    });
    let late = Status::Ended {
        run: 1,
        at: Tick::new(4),
        why: End::Paused,
    };
    controls.follow(&late);
    assert!(
        controls.running(),
        "a late end of run 1 must not stop run 2"
    );
    assert_eq!(controls.run(), Some(2));
    // Run 2 stopping on its own stops the controls.
    controls.follow(&Status::Ended {
        run: 2,
        at: Tick::new(9),
        why: End::CapacityPaused {
            detail: "population".into(),
        },
    });
    assert!(!controls.running());
    assert_eq!(controls.run(), None);
    assert_eq!(tap(&mut controls, KEY_RIGHT), [Intent::Step]);
}

#[test]
fn a_refused_resume_leaves_the_controls_paused() {
    use flife_studio::Status;
    let mut controls = Controls::new();
    assert_eq!(tap(&mut controls, KEY_SPACE), [Intent::Resume]);
    controls.refused(Intent::Resume);
    assert!(!controls.running());
    // A refused pause or step changes nothing.
    controls.set_running(true);
    controls.refused(Intent::Pause);
    controls.refused(Intent::Step);
    assert!(controls.running());
    // The session refusing a resume stops them too.
    controls.follow(&Status::Refused {
        intent: Intent::Resume,
        kind: "state-conflict",
        detail: "the run stopped".into(),
    });
    assert!(!controls.running());
    assert_eq!(tap(&mut controls, KEY_SPACE), [Intent::Resume]);
}
