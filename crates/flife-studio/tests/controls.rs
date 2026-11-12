//! The Studio's controls: typed native input to intents and presentation
//! state, as a pure state machine (no window).

use flife_engine::Disposition;
use flife_manim::Viewport;
use flife_studio::{
    BRUSH_ENERGY, BRUSH_INTRODUCTION, Brush, BrushSettings, CameraMove, ColorMapping, Controls,
    ENERGY_STEPS, FieldComponent, FieldMapping, INTRODUCTION_STEPS, Intent, KEY_C, KEY_D,
    KEY_DIGITS, KEY_E, KEY_EQUAL, KEY_ESCAPE, KEY_F, KEY_G, KEY_I, KEY_LEFT_BRACKET, KEY_M,
    KEY_MINUS, KEY_PERIOD, KEY_R, KEY_RIGHT, KEY_RIGHT_BRACKET, KEY_SLASH, KEY_SPACE, KEY_T, KEY_W,
    KEY_X, KEY_ZERO, Look, Particles, Style, ZOOM_PER_LINE, ZOOM_PER_POINT, ZOOM_STEP_LIMIT,
    scroll_zoom,
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

/// FL-022: E arms the energy brush and X the removal brush; the armed key
/// again disarms, the other switches. Held (auto-repeat) or with ⌘ they do
/// nothing, and neither is an intent or a style key.
#[test]
fn e_and_x_arm_switch_and_disarm_the_brushes() {
    // `kVK_ANSI_E` is 0x0E, `kVK_ANSI_X` 0x07, `kVK_ANSI_M` 0x2E and
    // `kVK_ANSI_I` 0x22 (Carbon's `Events.h`).
    assert_eq!([KEY_E, KEY_X, KEY_M, KEY_I], [0x0E, 0x07, 0x2E, 0x22]);
    let mut controls = Controls::new();
    let style = controls.style();
    assert_eq!(controls.brush(), None);
    assert!(tap(&mut controls, KEY_E).is_empty());
    assert_eq!(controls.brush(), Some(Brush::Energy));
    assert!(tap(&mut controls, KEY_X).is_empty());
    assert_eq!(controls.brush(), Some(Brush::Removal));
    assert!(tap(&mut controls, KEY_X).is_empty());
    assert_eq!(controls.brush(), None);
    tap(&mut controls, KEY_X);
    tap(&mut controls, KEY_E);
    assert_eq!(controls.brush(), Some(Brush::Energy));
    tap(&mut controls, KEY_E);
    assert_eq!(controls.brush(), None);
    for (code, brush) in [(KEY_M, Brush::Material), (KEY_I, Brush::Introduction)] {
        assert!(tap(&mut controls, code).is_empty());
        assert_eq!(controls.brush(), Some(brush));
    }
    tap(&mut controls, KEY_M);
    assert_eq!(controls.brush(), Some(Brush::Material));
    tap(&mut controls, KEY_M);
    assert_eq!(controls.brush(), None);

    controls.apply(&key(KEY_E, true, true, Modifiers::NONE));
    controls.apply(&key(
        KEY_X,
        true,
        false,
        Modifiers {
            command: true,
            ..Modifiers::NONE
        },
    ));
    assert_eq!(controls.brush(), None);
    assert!(moves(&mut controls).is_empty());
    assert_eq!(controls.style(), style);
    assert_eq!(Brush::Energy.name(), "energy");
    assert_eq!(Brush::Removal.name(), "removal");
}

/// FL-022: with a brush armed a click brushes where it pressed instead of
/// selecting, with the brush armed at the release; with any brush but the
/// removal brush armed a drag still pans; once disarmed a click selects
/// again.
#[test]
fn an_armed_click_brushes_and_a_drag_still_pans() {
    let mut controls = Controls::new();
    tap(&mut controls, KEY_X);
    controls.apply(&button(true, point(40.0, 50.0)));
    controls.apply(&drag_to(point(41.0, 51.0)));
    controls.apply(&button(false, point(41.0, 51.0)));
    tap(&mut controls, KEY_E);
    controls.apply(&button(true, point(10.0, 10.0)));
    controls.apply(&drag_to(point(30.0, 10.0)));
    controls.apply(&button(false, point(30.0, 10.0)));
    // Switched while held: the release brushes with the brush armed then.
    controls.apply(&button(true, point(7.0, 8.0)));
    tap(&mut controls, KEY_X);
    controls.apply(&button(false, point(7.0, 8.0)));
    tap(&mut controls, KEY_X);
    controls.apply(&button(true, point(3.0, 4.0)));
    controls.apply(&button(false, point(3.0, 4.0)));
    assert_eq!(
        moves(&mut controls),
        [
            CameraMove::Brush {
                at: point(40.0, 50.0),
                brush: Brush::Removal,
                settings: BrushSettings::default(),
            },
            CameraMove::Pan { delta: [20.0, 0.0] },
            CameraMove::Brush {
                at: point(7.0, 8.0),
                brush: Brush::Removal,
                settings: BrushSettings::default(),
            },
            CameraMove::Select {
                at: point(3.0, 4.0)
            },
        ]
    );
}

/// FL-022 (plan §2.2's "draw an intervention region"): with the removal
/// brush armed, a drag draws its disk instead of panning, centred where it
/// pressed; the release applies it out to where it was released. While it
/// is drawn, `drawing` names its centre. Disarming the brush while held,
/// or losing the press, applies nothing; a drag that left the slop panning
/// keeps panning when the brush is armed during it.
#[test]
fn an_armed_removal_drag_draws_its_disk() {
    let mut controls = Controls::new();
    tap(&mut controls, KEY_X);
    controls.apply(&button(true, point(100.0, 100.0)));
    controls.apply(&drag_to(point(102.0, 103.0)));
    // Within the slop: still a click candidate.
    assert_eq!(controls.drawing(), None);
    controls.apply(&drag_to(point(110.0, 96.0)));
    assert_eq!(controls.drawing(), Some(point(100.0, 100.0)));
    assert!(controls.dragging());
    controls.apply(&drag_to(point(130.0, 60.0)));
    assert!(moves(&mut controls).is_empty(), "a drawing pans nothing");
    controls.apply(&button(false, point(124.0, 132.0)));
    assert_eq!(
        moves(&mut controls),
        [CameraMove::Drawn {
            center: point(100.0, 100.0),
            edge: point(124.0, 132.0),
            disposition: Disposition::Recycle,
        }]
    );
    assert_eq!(controls.drawing(), None);
    assert!(!controls.dragging());

    // Disarmed while held: nothing is drawn, and the release applies
    // nothing.
    controls.apply(&button(true, point(50.0, 50.0)));
    controls.apply(&drag_to(point(70.0, 50.0)));
    assert_eq!(controls.drawing(), Some(point(50.0, 50.0)));
    tap(&mut controls, KEY_X);
    assert_eq!(controls.drawing(), None);
    controls.apply(&drag_to(point(80.0, 50.0)));
    controls.apply(&button(false, point(80.0, 50.0)));
    // Switched to another brush while held: the same.
    tap(&mut controls, KEY_X);
    controls.apply(&button(true, point(50.0, 50.0)));
    controls.apply(&drag_to(point(70.0, 50.0)));
    tap(&mut controls, KEY_E);
    controls.apply(&button(false, point(70.0, 50.0)));
    // Lost to capture loss: the same.
    tap(&mut controls, KEY_X);
    controls.apply(&button(true, point(20.0, 20.0)));
    controls.apply(&drag_to(point(40.0, 40.0)));
    controls.apply(&InputEvent::PointerCaptureLost {
        released: ButtonSet::EMPTY.with(PointerButton::Primary),
    });
    assert_eq!(controls.drawing(), None);
    controls.apply(&button(false, point(40.0, 40.0)));
    assert!(moves(&mut controls).is_empty());

    // Disarmed, a drag pans; arming the brush during it keeps it panning,
    // and draws nothing.
    tap(&mut controls, KEY_X);
    controls.apply(&button(true, point(10.0, 10.0)));
    controls.apply(&drag_to(point(30.0, 10.0)));
    tap(&mut controls, KEY_X);
    controls.apply(&drag_to(point(40.0, 10.0)));
    assert_eq!(controls.drawing(), None);
    controls.apply(&button(false, point(40.0, 10.0)));
    // A click with it armed still brushes its default disk.
    controls.apply(&button(true, point(5.0, 5.0)));
    controls.apply(&button(false, point(5.0, 5.0)));
    assert_eq!(
        moves(&mut controls),
        [
            CameraMove::Pan { delta: [20.0, 0.0] },
            CameraMove::Pan { delta: [10.0, 0.0] },
            CameraMove::Brush {
                at: point(5.0, 5.0),
                brush: Brush::Removal,
                settings: BrushSettings::default(),
            },
        ]
    );
    assert_eq!(controls.tally().moves, 4);
}

/// FL-022's amount, count and disposition: `-` and `=` halve and double
/// the armed brush's amount or count, each brush its own, within its range
/// and kept while disarmed; `/` sends the armed removal's energy to the next
/// disposition. With no brush armed, or the wrong one, held, or with ⌘,
/// they change nothing; they are never an intent. A click carries the
/// settings in force when it was made, and a drawn disk its disposition at
/// the release.
#[test]
fn minus_equal_and_slash_set_the_armed_brushs_amount_and_disposition() {
    let mut controls = Controls::new();
    let defaults = BrushSettings::default();
    assert_eq!(controls.settings(), defaults);
    assert_eq!(defaults.amount(Brush::Energy), Some(BRUSH_ENERGY));
    assert_eq!(defaults.amount(Brush::Material), Some(BRUSH_ENERGY));
    assert_eq!(defaults.amount(Brush::Removal), None);
    assert_eq!(defaults.amount(Brush::Introduction), None);
    assert_eq!(defaults.count(), BRUSH_INTRODUCTION);
    assert_eq!(defaults.disposition(), Disposition::Recycle);

    // Unarmed: nothing changes, and nothing is an intent; each key alone,
    // so no two undo each other.
    for code in [KEY_EQUAL, KEY_EQUAL, KEY_MINUS, KEY_SLASH] {
        assert!(tap(&mut controls, code).is_empty());
        assert_eq!(controls.settings(), defaults, "{code}");
    }

    // The energy brush doubles twice and halves once: 128.
    tap(&mut controls, KEY_E);
    for code in [KEY_EQUAL, KEY_EQUAL, KEY_MINUS] {
        assert!(tap(&mut controls, code).is_empty());
    }
    assert_eq!(
        controls.settings().amount(Brush::Energy),
        Some(BRUSH_ENERGY * 2.0)
    );
    // The material brush's is its own; `/` is not the energy brush's.
    assert_eq!(
        controls.settings().amount(Brush::Material),
        Some(BRUSH_ENERGY)
    );
    tap(&mut controls, KEY_SLASH);
    assert_eq!(controls.settings().disposition(), Disposition::Recycle);
    // Held, or with ⌘: nothing.
    controls.apply(&key(KEY_EQUAL, true, true, Modifiers::NONE));
    let command = Modifiers {
        command: true,
        ..Modifiers::NONE
    };
    controls.apply(&key(KEY_EQUAL, true, false, command));
    assert_eq!(
        controls.settings().amount(Brush::Energy),
        Some(BRUSH_ENERGY * 2.0)
    );

    // Its range: down to 1 and up to 4,096, and no further.
    for _ in 0..20 {
        tap(&mut controls, KEY_MINUS);
    }
    assert_eq!(controls.settings().amount(Brush::Energy), Some(1.0));
    for _ in 0..20 {
        tap(&mut controls, KEY_EQUAL);
    }
    assert_eq!(controls.settings().amount(Brush::Energy), Some(4_096.0));
    assert_eq!(ENERGY_STEPS, [-6, 6]);

    // The material brush halves alone.
    tap(&mut controls, KEY_M);
    tap(&mut controls, KEY_MINUS);
    assert_eq!(
        controls.settings().amount(Brush::Material),
        Some(BRUSH_ENERGY / 2.0)
    );
    assert_eq!(controls.settings().amount(Brush::Energy), Some(4_096.0));

    // The introduction brush's count: 1 to 64.
    tap(&mut controls, KEY_I);
    tap(&mut controls, KEY_EQUAL);
    assert_eq!(controls.settings().count(), BRUSH_INTRODUCTION * 2);
    for _ in 0..10 {
        tap(&mut controls, KEY_EQUAL);
    }
    assert_eq!(controls.settings().count(), 64);
    for _ in 0..10 {
        tap(&mut controls, KEY_MINUS);
    }
    assert_eq!(controls.settings().count(), 1);
    assert_eq!(INTRODUCTION_STEPS, [-3, 3]);

    // The removal brush has no amount; `/` cycles its disposition.
    tap(&mut controls, KEY_X);
    let before = controls.settings();
    tap(&mut controls, KEY_EQUAL);
    tap(&mut controls, KEY_MINUS);
    assert_eq!(controls.settings(), before);
    let mut seen = Vec::new();
    for _ in 0..4 {
        assert!(tap(&mut controls, KEY_SLASH).is_empty());
        seen.push(controls.settings().disposition());
    }
    assert_eq!(
        seen,
        [
            Disposition::Dissipate,
            Disposition::Remove,
            Disposition::Recycle,
            Disposition::Dissipate,
        ]
    );
    // Kept while disarmed and rearmed.
    tap(&mut controls, KEY_X);
    tap(&mut controls, KEY_X);
    assert_eq!(controls.settings().disposition(), Disposition::Dissipate);
    assert_eq!(controls.settings().count(), 1);
    assert_eq!(controls.settings().amount(Brush::Energy), Some(4_096.0));

    // A click carries the settings then: the key after it does not change
    // it. A drawn disk carries its disposition at the release.
    moves(&mut controls);
    let clicked = controls.settings();
    controls.apply(&button(true, point(5.0, 5.0)));
    controls.apply(&button(false, point(5.0, 5.0)));
    // Pressed sending to `remove`, released sending to `dissipate`.
    tap(&mut controls, KEY_SLASH);
    controls.apply(&button(true, point(10.0, 10.0)));
    controls.apply(&drag_to(point(30.0, 10.0)));
    tap(&mut controls, KEY_SLASH);
    tap(&mut controls, KEY_SLASH);
    controls.apply(&button(false, point(30.0, 10.0)));
    tap(&mut controls, KEY_E);
    tap(&mut controls, KEY_MINUS);
    controls.apply(&button(true, point(6.0, 6.0)));
    controls.apply(&button(false, point(6.0, 6.0)));
    let energy = controls.settings();
    tap(&mut controls, KEY_MINUS);
    assert_eq!(
        moves(&mut controls),
        [
            CameraMove::Brush {
                at: point(5.0, 5.0),
                brush: Brush::Removal,
                settings: clicked,
            },
            CameraMove::Drawn {
                center: point(10.0, 10.0),
                edge: point(30.0, 10.0),
                disposition: Disposition::Dissipate,
            },
            CameraMove::Brush {
                at: point(6.0, 6.0),
                brush: Brush::Energy,
                settings: energy,
            },
        ]
    );
    assert_eq!(clicked.disposition(), Disposition::Dissipate);
    assert_eq!(energy.amount(Brush::Energy), Some(2_048.0));
    assert_eq!(controls.settings().amount(Brush::Energy), Some(1_024.0));
    // No key here is a style key.
    assert_eq!(controls.style(), Style::default());
}

/// Plan §14.3's temporary measurement region: G arms the measuring tool,
/// putting any brush away, and G again puts it away; arming a brush puts it
/// away too. Held (auto-repeat) or with ⌘, G does nothing, and it is never
/// an intent or a style key.
#[test]
fn g_arms_the_measuring_tool_apart_from_the_brushes() {
    // `kVK_ANSI_G` is 0x05 (Carbon's `Events.h`).
    assert_eq!(KEY_G, 0x05);
    let mut controls = Controls::new();
    let style = controls.style();
    assert!(!controls.measuring());
    assert!(tap(&mut controls, KEY_G).is_empty());
    assert!(controls.measuring());
    assert_eq!(controls.brush(), None);
    assert!(tap(&mut controls, KEY_G).is_empty());
    assert!(!controls.measuring());
    // Exclusive with every brush, both ways.
    for (code, brush) in [
        (KEY_E, Brush::Energy),
        (KEY_X, Brush::Removal),
        (KEY_M, Brush::Material),
        (KEY_I, Brush::Introduction),
    ] {
        tap(&mut controls, code);
        assert_eq!(controls.brush(), Some(brush));
        tap(&mut controls, KEY_G);
        assert!(controls.measuring());
        assert_eq!(controls.brush(), None, "{brush:?}");
        tap(&mut controls, code);
        assert_eq!(controls.brush(), Some(brush));
        assert!(!controls.measuring(), "{brush:?}");
        // Disarming that brush leaves the tool put away.
        tap(&mut controls, code);
        assert_eq!(controls.brush(), None);
        assert!(!controls.measuring());
    }
    controls.apply(&key(KEY_G, true, true, Modifiers::NONE));
    controls.apply(&key(
        KEY_G,
        true,
        false,
        Modifiers {
            command: true,
            ..Modifiers::NONE
        },
    ));
    assert!(!controls.measuring());
    assert!(moves(&mut controls).is_empty());
    assert_eq!(controls.style(), style);
}

/// With the measuring tool armed, a click measures where it pressed, and a
/// drag draws a disk centred where it pressed instead of panning, measured
/// at its release out to where it was released; `drawing` names its centre
/// meanwhile. Putting the tool away while a drag is held, or losing the
/// press, measures nothing; a drag that left the slop panning keeps
/// panning when the tool is armed during it; once put away a click selects.
#[test]
fn an_armed_measuring_click_or_drag_measures() {
    let mut controls = Controls::new();
    tap(&mut controls, KEY_G);
    controls.apply(&button(true, point(40.0, 50.0)));
    controls.apply(&drag_to(point(41.0, 51.0)));
    controls.apply(&button(false, point(41.0, 51.0)));
    controls.apply(&button(true, point(100.0, 100.0)));
    controls.apply(&drag_to(point(102.0, 103.0)));
    // Within the slop: still a click candidate.
    assert_eq!(controls.drawing(), None);
    controls.apply(&drag_to(point(110.0, 96.0)));
    assert_eq!(controls.drawing(), Some(point(100.0, 100.0)));
    controls.apply(&drag_to(point(130.0, 60.0)));
    controls.apply(&button(false, point(124.0, 132.0)));
    assert_eq!(controls.drawing(), None);
    assert_eq!(
        moves(&mut controls),
        [
            CameraMove::Measure {
                center: point(40.0, 50.0),
                edge: None
            },
            CameraMove::Measure {
                center: point(100.0, 100.0),
                edge: Some(point(124.0, 132.0))
            },
        ],
        "a drawing pans nothing"
    );

    // Put away while held: nothing is drawn, and the release measures
    // nothing.
    controls.apply(&button(true, point(50.0, 50.0)));
    controls.apply(&drag_to(point(70.0, 50.0)));
    tap(&mut controls, KEY_G);
    assert_eq!(controls.drawing(), None);
    controls.apply(&button(false, point(70.0, 50.0)));
    // The removal brush armed while a measuring drag is held: a drag draws
    // for the tool armed as it left the slop, and the removal brush's
    // release removes nothing drawn to be measured.
    tap(&mut controls, KEY_G);
    controls.apply(&button(true, point(50.0, 50.0)));
    controls.apply(&drag_to(point(70.0, 50.0)));
    tap(&mut controls, KEY_X);
    assert_eq!(controls.drawing(), None);
    controls.apply(&button(false, point(70.0, 50.0)));
    // Nor does the tool measure a removal's disk armed while it is drawn.
    controls.apply(&button(true, point(50.0, 50.0)));
    controls.apply(&drag_to(point(70.0, 50.0)));
    assert_eq!(controls.drawing(), Some(point(50.0, 50.0)));
    tap(&mut controls, KEY_G);
    assert_eq!(controls.drawing(), None);
    controls.apply(&button(false, point(70.0, 50.0)));
    tap(&mut controls, KEY_G);
    // Lost to capture loss: the same.
    tap(&mut controls, KEY_G);
    controls.apply(&button(true, point(20.0, 20.0)));
    controls.apply(&drag_to(point(40.0, 40.0)));
    controls.apply(&InputEvent::PointerCaptureLost {
        released: ButtonSet::EMPTY.with(PointerButton::Primary),
    });
    assert_eq!(controls.drawing(), None);
    controls.apply(&button(false, point(40.0, 40.0)));
    assert!(moves(&mut controls).is_empty());

    // Put away, a drag pans; arming the tool during it keeps it panning,
    // and draws nothing.
    tap(&mut controls, KEY_G);
    assert!(!controls.measuring());
    controls.apply(&button(true, point(10.0, 10.0)));
    controls.apply(&drag_to(point(30.0, 10.0)));
    tap(&mut controls, KEY_G);
    controls.apply(&drag_to(point(40.0, 10.0)));
    assert_eq!(controls.drawing(), None);
    controls.apply(&button(false, point(40.0, 10.0)));
    // Put away again, a click selects.
    tap(&mut controls, KEY_G);
    controls.apply(&button(true, point(5.0, 5.0)));
    controls.apply(&button(false, point(5.0, 5.0)));
    assert_eq!(
        moves(&mut controls),
        [
            CameraMove::Pan { delta: [20.0, 0.0] },
            CameraMove::Pan { delta: [10.0, 0.0] },
            CameraMove::Select {
                at: point(5.0, 5.0)
            },
        ]
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

/// FL-021: W asks for the selected organism's weight dump as a move, with
/// no intent of its own (the host knows the selection); auto-repeat asks
/// nothing more, ⌘W closes and asks nothing, and it is no style key.
#[test]
fn w_asks_for_a_weight_dump() {
    let mut controls = Controls::new();
    let style = controls.style();
    assert!(tap(&mut controls, KEY_W).is_empty());
    controls.apply(&key(KEY_W, true, true, Modifiers::NONE));
    assert_eq!(moves(&mut controls), [CameraMove::Weights]);
    assert_eq!(
        controls.apply(&key(
            KEY_W,
            true,
            false,
            Modifiers {
                command: true,
                ..Modifiers::NONE
            },
        )),
        Some(Intent::Close)
    );
    assert!(moves(&mut controls).is_empty());
    assert_eq!(controls.style(), style);
}

/// Plan §14.3: T asks the host to track the selected organism, or to stop,
/// as a move with no intent (the host knows the selection); each press is
/// one move, auto-repeat and ⌘T ask nothing, and it is no style key.
#[test]
fn t_asks_to_track_the_selection() {
    assert_eq!(KEY_T, 17);
    let mut controls = Controls::new();
    let style = controls.style();
    assert!(tap(&mut controls, KEY_T).is_empty());
    assert!(tap(&mut controls, KEY_T).is_empty());
    controls.apply(&key(KEY_T, true, true, Modifiers::NONE));
    assert_eq!(
        controls.apply(&key(
            KEY_T,
            true,
            false,
            Modifiers {
                command: true,
                ..Modifiers::NONE
            },
        )),
        None
    );
    assert_eq!(moves(&mut controls), [CameraMove::Track, CameraMove::Track]);
    assert_eq!(controls.style(), style);
}

/// FL-019: the digit keys 1 to 6 (`kVK_ANSI_1` … `kVK_ANSI_6`, whose codes
/// are not in order) choose the six mappings in order, R, D and F switch the
/// look, the particles and the field, C the field's mapping (energy,
/// composition, then the chosen component), none of them makes an intent,
/// and auto-repeat and ⌘ change nothing.
#[test]
fn style_keys_choose_the_mapping_look_particles_and_field() {
    assert_eq!(KEY_DIGITS, [18, 19, 20, 21, 23, 22]);
    assert_eq!(ColorMapping::ALL.len(), KEY_DIGITS.len());
    assert_eq!(ColorMapping::ALL[5], ColorMapping::Affinity);
    let mut controls = Controls::new();
    assert_eq!(controls.style(), Style::default());
    for (&code, mapping) in KEY_DIGITS.iter().zip(ColorMapping::ALL).rev() {
        assert!(tap(&mut controls, code).is_empty());
        assert_eq!(controls.style().mapping, mapping, "key {code}");
    }
    assert_eq!(controls.style().mapping, ColorMapping::Lineage);
    // Repeat and ⌘ leave the mapping.
    controls.apply(&key(22, true, true, Modifiers::NONE));
    controls.apply(&key(
        22,
        true,
        false,
        Modifiers {
            command: true,
            ..Modifiers::NONE
        },
    ));
    assert_eq!(controls.style().mapping, ColorMapping::Lineage);
    assert!(tap(&mut controls, 22).is_empty());
    assert_eq!(controls.style().mapping, ColorMapping::Affinity);
    // R, D and F switch, and switch back.
    for (code, changed) in [
        (
            KEY_R,
            Style {
                look: Look::Raw,
                ..controls.style()
            },
        ),
        (
            KEY_D,
            Style {
                look: Look::Raw,
                particles: Particles::Density,
                ..controls.style()
            },
        ),
        (
            KEY_F,
            Style {
                look: Look::Raw,
                particles: Particles::Density,
                field: false,
                ..controls.style()
            },
        ),
        (
            KEY_C,
            Style {
                look: Look::Raw,
                particles: Particles::Density,
                field: false,
                field_mapping: FieldMapping::Composition,
                ..controls.style()
            },
        ),
    ] {
        assert!(tap(&mut controls, code).is_empty());
        assert_eq!(controls.style(), changed, "key {code}");
    }
    assert_eq!(KEY_C, 8, "kVK_ANSI_C");
    for code in [KEY_R, KEY_D, KEY_F, KEY_C] {
        tap(&mut controls, code);
    }
    assert_eq!(
        controls.style().field_mapping,
        FieldMapping::Component(FieldComponent::default())
    );
    tap(&mut controls, KEY_C);
    assert_eq!(
        controls.style(),
        Style {
            mapping: ColorMapping::Affinity,
            ..Style::default()
        }
    );
    assert_eq!(controls.tally().keys, 2 * 16 + 2);
}

/// Plan §13.4's chosen components: `[` and `]` (`kVK_ANSI_LeftBracket`,
/// `kVK_ANSI_RightBracket`) first show the chosen field component, then
/// step to the previous or next channel, wrapping; C cycles energy,
/// composition and the component last chosen; a style the Studio opens in
/// that shows a component chooses it; no intent, and auto-repeat and ⌘
/// change nothing.
#[test]
fn bracket_keys_choose_the_field_component() {
    assert_eq!((KEY_LEFT_BRACKET, KEY_RIGHT_BRACKET), (33, 30));
    let component =
        |channel| FieldMapping::Component(FieldComponent::new(channel).expect("a channel"));
    let mut controls = Controls::new();
    for (code, shown) in [
        (KEY_RIGHT_BRACKET, component(0)),
        (KEY_RIGHT_BRACKET, component(1)),
        (KEY_RIGHT_BRACKET, component(2)),
        (KEY_LEFT_BRACKET, component(1)),
        (KEY_LEFT_BRACKET, component(0)),
        (KEY_LEFT_BRACKET, component(31)),
        (KEY_C, FieldMapping::Energy),
        (KEY_C, FieldMapping::Composition),
        (KEY_C, component(31)),
        (KEY_C, FieldMapping::Energy),
        (KEY_LEFT_BRACKET, component(31)),
        (KEY_RIGHT_BRACKET, component(0)),
    ] {
        assert!(tap(&mut controls, code).is_empty());
        assert_eq!(controls.style().field_mapping, shown, "key {code}");
    }
    // Repeat and ⌘ step nothing.
    controls.apply(&key(KEY_RIGHT_BRACKET, true, true, Modifiers::NONE));
    controls.apply(&key(
        KEY_LEFT_BRACKET,
        true,
        false,
        Modifiers {
            command: true,
            ..Modifiers::NONE
        },
    ));
    assert_eq!(controls.style().field_mapping, component(0));
    assert_eq!(
        controls.style(),
        Style {
            field_mapping: component(0),
            ..Style::default()
        },
        "only the field's mapping changed"
    );

    let mut opened = Controls::new();
    opened.set_style(Style {
        field_mapping: component(9),
        ..Style::default()
    });
    for shown in [
        FieldMapping::Energy,
        FieldMapping::Composition,
        component(9),
    ] {
        tap(&mut opened, KEY_C);
        assert_eq!(opened.style().field_mapping, shown);
    }
    tap(&mut opened, KEY_RIGHT_BRACKET);
    assert_eq!(opened.style().field_mapping, component(10));
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
