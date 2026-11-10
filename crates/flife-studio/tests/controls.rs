//! The Studio's controls: typed native input to intents and presentation
//! state, as a pure state machine (no window).

use flife_manim::Viewport;
use flife_studio::{
    CameraMove, Controls, Intent, KEY_ESCAPE, KEY_PERIOD, KEY_RIGHT, KEY_SPACE, KEY_W, KEY_ZERO,
    ZOOM_PER_LINE, ZOOM_PER_POINT, ZOOM_STEP_LIMIT, scroll_zoom,
};
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
    // At one pixel per point the drawable is the content area in whole
    // points; on the 2x screen of `metrics`, twice that, rounded.
    controls.apply(&InputEvent::Resized(SurfaceMetrics {
        backing_scale: 1.0,
        ..metrics(800.4, 449.6)
    }));
    assert_eq!(controls.drawable_size(), Some([800, 450]));
    controls.apply(&InputEvent::Resized(metrics(800.4, 449.6)));
    assert_eq!(controls.drawable_size(), Some([1601, 899]));
    let viewport = Viewport::fit([128.0, 128.0], [1601, 899]).ok();
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
        position: point(at[0] * 800.4 / 1601.0, at[1] * 449.6 / 899.0),
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
    // Under half a pixel wide at 2x: empty.
    controls.apply(&InputEvent::Resized(metrics(0.2, 100.0)));
    assert_eq!(controls.drawable_size(), None);
    let tally = controls.tally();
    assert_eq!(
        (
            tally.focus_gained,
            tally.focus_lost,
            tally.resizes,
            tally.scrolls
        ),
        (1, 1, 3, 1)
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

fn button(pressed: bool, at: LogicalPoint) -> InputEvent {
    InputEvent::Button {
        button: PointerButton::Primary,
        pressed,
        position: at,
        clicks: 1,
        modifiers: Modifiers::NONE,
    }
}

fn drag_to(at: LogicalPoint) -> InputEvent {
    InputEvent::PointerMoved {
        position: at,
        buttons: ButtonSet::EMPTY.with(PointerButton::Primary),
        modifiers: Modifiers::NONE,
    }
}

fn moves(controls: &mut Controls) -> Vec<CameraMove> {
    controls.take_moves().collect()
}

/// FL-020: a press that moves beyond the slop is a drag. Its first pan is
/// the whole motion since the press, then each move; the pans sum to the
/// pointer's whole motion, and the release selects nothing.
#[test]
fn a_drag_pans_by_the_whole_pointer_motion_and_selects_nothing() {
    let mut controls = Controls::new();
    controls.apply(&button(true, point(100.0, 100.0)));
    // Within the slop: still a click candidate, no pan.
    controls.apply(&drag_to(point(102.0, 103.0)));
    assert!(moves(&mut controls).is_empty());
    assert!(!controls.dragging());
    controls.apply(&drag_to(point(110.0, 96.0)));
    controls.apply(&drag_to(point(130.5, 80.0)));
    controls.apply(&drag_to(point(130.5, 80.0)));
    controls.apply(&button(false, point(131.0, 79.0)));
    let made = moves(&mut controls);
    assert_eq!(
        made,
        [
            CameraMove::Pan {
                delta: [10.0, -4.0]
            },
            CameraMove::Pan {
                delta: [20.5, -16.0]
            },
        ]
    );
    let total = made.iter().fold([0.0, 0.0], |sum, change| match change {
        CameraMove::Pan { delta } => [sum[0] + delta[0], sum[1] + delta[1]],
        _ => sum,
    });
    assert_eq!(total, [30.5, -20.0]);
    assert!(!controls.dragging());
    assert_eq!(controls.tally().moves, 2);
}

/// A press released within the slop is a click: one selection at the
/// press. A secondary press, a press lost to capture loss, and motion with
/// no press make no move.
#[test]
fn a_click_selects_where_it_pressed_and_lost_presses_select_nothing() {
    let mut controls = Controls::new();
    controls.apply(&button(true, point(40.0, 50.0)));
    controls.apply(&drag_to(point(42.0, 52.0)));
    controls.apply(&button(false, point(42.0, 52.0)));
    assert_eq!(
        moves(&mut controls),
        [CameraMove::Select {
            at: point(40.0, 50.0)
        }]
    );

    controls.apply(&InputEvent::Button {
        button: PointerButton::Secondary,
        pressed: true,
        position: point(1.0, 1.0),
        clicks: 1,
        modifiers: Modifiers::NONE,
    });
    controls.apply(&InputEvent::Button {
        button: PointerButton::Secondary,
        pressed: false,
        position: point(1.0, 1.0),
        clicks: 1,
        modifiers: Modifiers::NONE,
    });
    controls.apply(&InputEvent::PointerMoved {
        position: point(300.0, 300.0),
        buttons: ButtonSet::EMPTY,
        modifiers: Modifiers::NONE,
    });
    assert!(moves(&mut controls).is_empty());

    controls.apply(&button(true, point(10.0, 10.0)));
    controls.apply(&InputEvent::PointerCaptureLost {
        released: ButtonSet::EMPTY.with(PointerButton::Primary),
    });
    controls.apply(&button(false, point(10.0, 10.0)));
    controls.apply(&drag_to(point(90.0, 90.0)));
    assert!(moves(&mut controls).is_empty(), "a lost press is no click");

    // A drag lost to capture loss pans no further.
    controls.apply(&button(true, point(0.0, 0.0)));
    controls.apply(&drag_to(point(20.0, 0.0)));
    controls.apply(&InputEvent::PointerCaptureLost {
        released: ButtonSet::EMPTY.with(PointerButton::Primary),
    });
    controls.apply(&drag_to(point(40.0, 0.0)));
    controls.apply(&button(false, point(40.0, 0.0)));
    assert_eq!(
        moves(&mut controls),
        [CameraMove::Pan { delta: [20.0, 0.0] }]
    );
}

/// A vertical scroll zooms about the pointer by `exp(rate · dy)`: up in,
/// down out, by points or lines; no movement, a horizontal scroll, and
/// non-finite deltas make no move; a huge scroll is limited.
#[test]
fn a_scroll_zooms_about_the_pointer() {
    assert_eq!(scroll_zoom(50.0, ScrollUnit::Points), Some((0.5_f64).exp()));
    assert_eq!(scroll_zoom(-3.0, ScrollUnit::Lines), Some((-0.3_f64).exp()));
    assert_eq!(scroll_zoom(0.0, ScrollUnit::Points), None);
    assert_eq!(scroll_zoom(f64::NAN, ScrollUnit::Points), None);
    assert_eq!(
        scroll_zoom(1e9, ScrollUnit::Points),
        Some(ZOOM_STEP_LIMIT.exp())
    );
    assert_eq!(
        scroll_zoom(-1e9, ScrollUnit::Lines),
        Some((-ZOOM_STEP_LIMIT).exp())
    );
    const { assert!(ZOOM_PER_POINT > 0.0 && ZOOM_PER_LINE > ZOOM_PER_POINT) };

    let mut controls = Controls::new();
    let scroll = |dy, unit| InputEvent::Scroll {
        delta: [7.0, dy],
        unit,
        position: point(12.0, 34.0),
        modifiers: Modifiers::NONE,
    };
    controls.apply(&scroll(50.0, ScrollUnit::Points));
    controls.apply(&scroll(0.0, ScrollUnit::Points));
    controls.apply(&scroll(-2.0, ScrollUnit::Lines));
    assert_eq!(
        moves(&mut controls),
        [
            CameraMove::Zoom {
                anchor: point(12.0, 34.0),
                factor: (0.5_f64).exp()
            },
            CameraMove::Zoom {
                anchor: point(12.0, 34.0),
                factor: (-0.2_f64).exp()
            },
        ]
    );
    assert_eq!(controls.tally().scrolls, 3);
}

/// 0 asks for the whole torus again; held (auto-repeat) or with ⌘ it does
/// nothing, and it is no style key.
#[test]
fn zero_resets_the_view() {
    let mut controls = Controls::new();
    let style = controls.style();
    assert!(tap(&mut controls, KEY_ZERO).is_empty());
    controls.apply(&key(KEY_ZERO, true, true, Modifiers::NONE));
    controls.apply(&key(
        KEY_ZERO,
        true,
        false,
        Modifiers {
            command: true,
            ..Modifiers::NONE
        },
    ));
    assert_eq!(moves(&mut controls), [CameraMove::Reset]);
    assert_eq!(controls.style(), style);
}

/// Moves wait, in order, until taken; reserving room for one poll's events
/// lets that many accumulate.
#[test]
fn moves_are_kept_in_order_until_taken() {
    let mut controls = Controls::new();
    controls.reserve_moves(256).expect("reserved");
    for _ in 0..256 {
        tap(&mut controls, KEY_ZERO);
    }
    assert_eq!(moves(&mut controls).len(), 256);
    assert!(moves(&mut controls).is_empty());
}
