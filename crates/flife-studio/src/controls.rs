//! Typed native input to Studio intents (plan §13.6).
//!
//! [`Controls`] is a pure state machine over the native host's
//! [`InputEvent`]s. It turns keys into [`Intent`]s for the session and keeps
//! what the presentation owner needs itself: the pointer, held buttons,
//! focus and the surface geometry. It turns pointer gestures into
//! [`CameraMove`]s, in logical points, which the presentation owner applies
//! to its viewport and hit tests (FL-020). Nothing here reaches the world: a
//! click of an armed [`Brush`] is a [`CameraMove::Brush`], which the
//! presentation owner maps through its viewport to a world-space
//! [`Stroke`](crate::Stroke) and offers the session as
//! [`Intent::Intervene`] (FL-022).
//!
//! Pointer gestures:
//!
//! | Gesture | Move |
//! |---|---|
//! | primary drag (beyond [`CLICK_SLOP_POINTS`]) | pan: the world follows the pointer |
//! | primary click (press and release within the slop) | select what is drawn under the press, or with a brush armed, brush there |
//! | vertical scroll | zoom about the pointer by [`scroll_zoom`]; up zooms in |
//!
//! Keys, by virtual key code (`kVK_*`), so they do not depend on layout:
//!
//! | Key | Intent |
//! |---|---|
//! | Space | run or pause |
//! | → or `.` (held: repeats) | step one tick while paused |
//! | Esc, or ⌘W | close |
//! | 0 | the whole torus again (zoom 1, centred) |
//! | E | arm the energy brush, or disarm it (FL-022) |
//! | X | arm the removal brush, or disarm it (FL-022) |
//! | M | arm the material brush, or disarm it (FL-022) |
//! | I | arm the introduction brush, or disarm it (FL-022) |
//! | W | dump the selected organism's whole controller (FL-021) |
//! | T | track the selected organism, the camera keeping it centred, or stop (plan §14.3) |
//!
//! and, for the presentation owner alone (FL-019's [`Style`]):
//!
//! | Key | Style |
//! |---|---|
//! | 1 to 6 | colour by lineage, energy, generation, motion, topology or chemical affinity |
//! | R | glow or raw |
//! | D | points or density |
//! | F | field on or off |
//! | C | the field's energy, its chemical composition, or its chosen component, in turn ([`FieldMapping`](flife_manim::FieldMapping)) |
//! | `[` or `]` | the field's chosen component: shown, then the previous or next channel ([`FieldComponent`](flife_manim::FieldComponent)) |

use flife_core::Tick;
use flife_core::ids::OrganismId;
use flife_manim::{ColorMapping, FieldComponent, FieldMapping, Look, Particles, Style, Viewport};
use fmn_render::metal::{
    ButtonSet, InputEvent, LogicalPoint, PointerButton, ScrollUnit, SurfaceMetrics,
};

use crate::{Brush, Status, Stroke};

/// `kVK_Space`.
pub const KEY_SPACE: u16 = 49;
/// `kVK_RightArrow`.
pub const KEY_RIGHT: u16 = 124;
/// `kVK_ANSI_Period`.
pub const KEY_PERIOD: u16 = 47;
/// `kVK_Escape`.
pub const KEY_ESCAPE: u16 = 53;
/// `kVK_ANSI_W`.
pub const KEY_W: u16 = 13;
/// `kVK_ANSI_1` to `kVK_ANSI_6`, for [`ColorMapping::ALL`] in order.
pub const KEY_DIGITS: [u16; 6] = [18, 19, 20, 21, 23, 22];
/// `kVK_ANSI_R`.
pub const KEY_R: u16 = 15;
/// `kVK_ANSI_D`.
pub const KEY_D: u16 = 2;
/// `kVK_ANSI_F`.
pub const KEY_F: u16 = 3;
/// `kVK_ANSI_C`.
pub const KEY_C: u16 = 8;
/// `kVK_ANSI_0`.
pub const KEY_ZERO: u16 = 29;
/// `kVK_ANSI_E`.
pub const KEY_E: u16 = 14;
/// `kVK_ANSI_X`.
pub const KEY_X: u16 = 7;
/// `kVK_ANSI_M`.
pub const KEY_M: u16 = 46;
/// `kVK_ANSI_I`.
pub const KEY_I: u16 = 34;
/// `kVK_ANSI_T`.
pub const KEY_T: u16 = 17;
/// `kVK_ANSI_LeftBracket`.
pub const KEY_LEFT_BRACKET: u16 = 33;
/// `kVK_ANSI_RightBracket`.
pub const KEY_RIGHT_BRACKET: u16 = 30;

/// How far, in logical points, a primary press may move and still be a
/// click; beyond it the press is a drag.
pub const CLICK_SLOP_POINTS: f64 = 4.0;
/// The zoom per point of precise (trackpad) vertical scroll, as a natural
/// logarithm: 100 points zoom by e.
pub const ZOOM_PER_POINT: f64 = 0.01;
/// The zoom per line of notched-wheel vertical scroll, as a natural
/// logarithm: one line is about 10.5 %.
pub const ZOOM_PER_LINE: f64 = 0.1;
/// The largest zoom one scroll event may make, as a natural logarithm.
pub const ZOOM_STEP_LIMIT: f64 = 8.0;

/// The zoom factor of a vertical scroll of `delta_y` in `unit`: positive
/// (up) zooms in, negative out; `exp(rate · delta_y)`, the exponent limited
/// to ±[`ZOOM_STEP_LIMIT`]. `None` for no movement or a non-finite delta.
#[must_use]
pub fn scroll_zoom(delta_y: f64, unit: ScrollUnit) -> Option<f64> {
    if !delta_y.is_finite() || delta_y == 0.0 {
        return None;
    }
    let rate = match unit {
        ScrollUnit::Points => ZOOM_PER_POINT,
        ScrollUnit::Lines => ZOOM_PER_LINE,
    };
    Some(
        (rate * delta_y)
            .clamp(-ZOOM_STEP_LIMIT, ZOOM_STEP_LIMIT)
            .exp(),
    )
}

/// A camera or selection change for the presentation owner, in the logical
/// points the window reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CameraMove {
    /// Zoom by `factor` about the logical point `anchor`, keeping the world
    /// under it.
    Zoom {
        /// The pointer.
        anchor: LogicalPoint,
        /// Above 1 zooms in.
        factor: f64,
    },
    /// The pointer dragged by `delta` logical points: the world follows.
    Pan {
        /// Right and down are positive.
        delta: [f64; 2],
    },
    /// The whole torus again.
    Reset,
    /// Select what is drawn under the logical point `at`, or nothing.
    Select {
        /// Where the click pressed.
        at: LogicalPoint,
    },
    /// Apply `brush` under the logical point `at` (FL-022).
    Brush {
        /// Where the click pressed.
        at: LogicalPoint,
        /// The brush armed then.
        brush: Brush,
    },
    /// Dump the selected organism's whole controller (FL-021, plan §14.2's
    /// on-demand weight dump): the presentation owner asks the session for
    /// whatever is selected when it takes the move.
    Weights,
    /// Track the selected organism, or stop tracking (plan §14.3's
    /// camera-follow target): while tracking, the presentation owner centres
    /// its view on the selected organism where each frame draws it, at the
    /// zoom it has. A pan, a reset or losing the selection stops it.
    Track,
}

/// What the presentation owner asks of the session.
#[derive(Debug, Clone, Copy, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "a material stroke's 32 components keep the intent `Copy` and unboxed; the lane holds 16"
)]
pub enum Intent {
    /// Run until paused.
    Resume,
    /// Pause at the next committed boundary.
    Pause,
    /// Advance one tick while paused.
    Step,
    /// Inspect an organism selected on screen (FL-021): a bounded snapshot
    /// of it at the session's committed boundary. The presentation owner
    /// sends this on a selection, not on a key.
    Inspect {
        /// The selected organism.
        organism: OrganismId,
        /// The source tick of the epoch it was selected on. A running
        /// session's committed boundary can be past it.
        seen: Tick,
    },
    /// Dump an organism's whole controller (FL-021, plan §14.2): its
    /// genome's parameters at the session's committed boundary. The
    /// presentation owner sends this for the selected organism on a
    /// [`CameraMove::Weights`].
    DumpWeights {
        /// The selected organism.
        organism: OrganismId,
        /// The source tick of the epoch it was shown on.
        seen: Tick,
    },
    /// Apply a brush's stroke (FL-022): submitted as a command for the next
    /// boundary not yet sealed. The presentation owner sends this on a
    /// brush click, not on a key.
    Intervene {
        /// The world-space stroke.
        stroke: Stroke,
        /// The source tick of the epoch it was made on.
        seen: Tick,
    },
    /// Close the session.
    Close,
}

/// Counts of every typed input event the controls have seen, by kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputTally {
    /// Pointer motion.
    pub pointer_moves: u64,
    /// Button presses and releases.
    pub buttons: u64,
    /// Scrolls.
    pub scrolls: u64,
    /// Key downs and ups.
    pub keys: u64,
    /// Modifier changes.
    pub modifier_changes: u64,
    /// Focus gained.
    pub focus_gained: u64,
    /// Focus lost.
    pub focus_lost: u64,
    /// Pointer capture lost.
    pub capture_lost: u64,
    /// Surface geometry changes.
    pub resizes: u64,
    /// Camera and selection moves made.
    pub moves: u64,
}

/// The presentation owner's input state.
#[derive(Debug, Clone, Default)]
pub struct Controls {
    running: bool,
    run: Option<u64>,
    close: bool,
    focused: bool,
    pointer: Option<LogicalPoint>,
    buttons: ButtonSet,
    metrics: Option<SurfaceMetrics>,
    tally: InputTally,
    style: Style,
    /// Where the primary button went down, while it is held.
    press: Option<LogicalPoint>,
    /// Whether the held press has moved beyond the click slop.
    dragging: bool,
    moves: Vec<CameraMove>,
    /// The armed brush: a click brushes instead of selecting.
    brush: Option<Brush>,
    /// The field component C and the bracket keys show: the last chosen.
    component: FieldComponent,
}

impl Controls {
    /// Paused, unfocused, with no geometry yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply one event in delivery order, returning the intent it makes.
    pub fn apply(&mut self, event: &InputEvent) -> Option<Intent> {
        match event {
            InputEvent::PointerMoved {
                position, buttons, ..
            } => {
                self.tally.pointer_moves += 1;
                let last = self.pointer.replace(*position);
                self.buttons = *buttons;
                self.drag(*position, last);
                None
            }
            InputEvent::Button {
                button,
                pressed,
                position,
                ..
            } => {
                self.tally.buttons += 1;
                self.pointer = Some(*position);
                self.buttons = if *pressed {
                    self.buttons.with(*button)
                } else {
                    self.buttons.without(*button)
                };
                if *button == PointerButton::Primary {
                    self.press_or_release(*pressed, *position);
                }
                None
            }
            InputEvent::Scroll {
                delta,
                unit,
                position,
                ..
            } => {
                self.tally.scrolls += 1;
                self.pointer = Some(*position);
                if let Some(factor) = scroll_zoom(delta[1], *unit) {
                    self.push(CameraMove::Zoom {
                        anchor: *position,
                        factor,
                    });
                }
                None
            }
            InputEvent::Key {
                code,
                pressed,
                repeat,
                modifiers,
                ..
            } => {
                self.tally.keys += 1;
                if !*pressed {
                    return None;
                }
                match *code {
                    KEY_SPACE if !*repeat => {
                        self.running = !self.running;
                        Some(if self.running {
                            Intent::Resume
                        } else {
                            Intent::Pause
                        })
                    }
                    KEY_RIGHT | KEY_PERIOD if !self.running => Some(Intent::Step),
                    KEY_ESCAPE => self.request_close(),
                    KEY_W if modifiers.command => self.request_close(),
                    _ if *repeat || modifiers.command => None,
                    KEY_ZERO => {
                        self.push(CameraMove::Reset);
                        None
                    }
                    KEY_E => {
                        self.arm(Brush::Energy);
                        None
                    }
                    KEY_X => {
                        self.arm(Brush::Removal);
                        None
                    }
                    KEY_M => {
                        self.arm(Brush::Material);
                        None
                    }
                    KEY_I => {
                        self.arm(Brush::Introduction);
                        None
                    }
                    KEY_W => {
                        self.push(CameraMove::Weights);
                        None
                    }
                    KEY_T => {
                        self.push(CameraMove::Track);
                        None
                    }
                    code => {
                        self.restyle(code);
                        None
                    }
                }
            }
            InputEvent::ModifiersChanged { .. } => {
                self.tally.modifier_changes += 1;
                None
            }
            InputEvent::FocusGained => {
                self.tally.focus_gained += 1;
                self.focused = true;
                None
            }
            InputEvent::FocusLost => {
                self.tally.focus_lost += 1;
                self.focused = false;
                None
            }
            InputEvent::PointerCaptureLost { .. } => {
                self.tally.capture_lost += 1;
                self.buttons = ButtonSet::EMPTY;
                // A lost press is neither a click nor the rest of a drag.
                self.press = None;
                self.dragging = false;
                None
            }
            InputEvent::Resized(metrics) => {
                self.tally.resizes += 1;
                self.metrics = Some(*metrics);
                None
            }
        }
    }

    fn push(&mut self, change: CameraMove) {
        self.tally.moves += 1;
        self.moves.push(change);
    }

    /// The primary button went down (start a press) or up (a press that
    /// never left the slop is a click).
    fn press_or_release(&mut self, pressed: bool, position: LogicalPoint) {
        if pressed {
            self.press = Some(position);
            self.dragging = false;
            return;
        }
        if let Some(at) = self.press.take()
            && !self.dragging
        {
            self.push(match self.brush {
                Some(brush) => CameraMove::Brush { at, brush },
                None => CameraMove::Select { at },
            });
        }
        self.dragging = false;
    }

    /// Arm `brush`, or disarm it if it is the one armed.
    fn arm(&mut self, brush: Brush) {
        self.brush = if self.brush == Some(brush) {
            None
        } else {
            Some(brush)
        };
    }

    /// The armed brush, if any.
    #[must_use]
    pub const fn brush(&self) -> Option<Brush> {
        self.brush
    }

    /// Pointer motion while the primary press is held: once beyond the
    /// slop it is a drag, panning by the whole motion since the press and
    /// then by each move.
    fn drag(&mut self, position: LogicalPoint, last: Option<LogicalPoint>) {
        let Some(press) = self.press else {
            return;
        };
        if !self.buttons.contains(PointerButton::Primary) {
            return;
        }
        let from = if self.dragging {
            match last {
                Some(last) => last,
                None => return,
            }
        } else {
            let (dx, dy) = (position.x - press.x, position.y - press.y);
            if dx.hypot(dy) <= CLICK_SLOP_POINTS {
                return;
            }
            self.dragging = true;
            press
        };
        let delta = [position.x - from.x, position.y - from.y];
        if delta.iter().all(|value| value.is_finite()) && delta != [0.0, 0.0] {
            self.push(CameraMove::Pan { delta });
        }
    }

    /// Take the camera and selection moves made since the last take, in
    /// the order they were made.
    pub fn take_moves(&mut self) -> std::vec::Drain<'_, CameraMove> {
        self.moves.drain(..)
    }

    /// Reserve room for `capacity` moves, so draining one poll's events
    /// (each makes at most one move) never allocates.
    ///
    /// # Errors
    /// [`flife_core::CoreError::AllocationFailed`] when it cannot be
    /// reserved.
    pub fn reserve_moves(&mut self, capacity: usize) -> Result<(), flife_core::CoreError> {
        let wanted = capacity.saturating_sub(self.moves.len());
        self.moves
            .try_reserve_exact(wanted)
            .map_err(|_| flife_core::CoreError::AllocationFailed {
                buffer: "studio camera moves",
                bytes: u64::try_from(capacity.saturating_mul(size_of::<CameraMove>()))
                    .unwrap_or(u64::MAX),
            })
    }

    /// Whether a primary press is dragging.
    #[must_use]
    pub const fn dragging(&self) -> bool {
        self.dragging
    }

    /// A style key's change, if `code` is one.
    fn restyle(&mut self, code: u16) {
        if let Some(slot) = KEY_DIGITS.iter().position(|&digit| digit == code) {
            self.style.mapping = ColorMapping::ALL[slot];
            return;
        }
        match code {
            KEY_R => {
                self.style.look = match self.style.look {
                    Look::Glow => Look::Raw,
                    Look::Raw => Look::Glow,
                };
            }
            KEY_D => {
                self.style.particles = match self.style.particles {
                    Particles::Points => Particles::Density,
                    Particles::Density => Particles::Points,
                };
            }
            KEY_F => self.style.field = !self.style.field,
            KEY_C => self.style.field_mapping = self.style.field_mapping.next(self.component),
            KEY_LEFT_BRACKET | KEY_RIGHT_BRACKET => {
                // The first press shows the chosen component; later ones
                // step through the channels.
                if self.style.field_mapping.component().is_some() {
                    self.component = if code == KEY_RIGHT_BRACKET {
                        self.component.next()
                    } else {
                        self.component.previous()
                    };
                }
                self.style.field_mapping = FieldMapping::Component(self.component);
            }
            _ => {}
        }
    }

    /// The style the presentation owner should draw in.
    #[must_use]
    pub const fn style(&self) -> Style {
        self.style
    }

    /// Start from `style`, as the Studio opens; a component it shows is
    /// the chosen one.
    pub const fn set_style(&mut self, style: Style) {
        self.style = style;
        if let Some(component) = style.field_mapping.component() {
            self.component = component;
        }
    }

    fn request_close(&mut self) -> Option<Intent> {
        if self.close {
            return None;
        }
        self.close = true;
        Some(Intent::Close)
    }

    /// Follow the session: it runs, or it stopped (paused on request, by a
    /// capacity shortfall, or a failed tick).
    pub fn set_running(&mut self, running: bool) {
        self.running = running;
    }

    /// Follow one status from the session, in the order it arrived. A run
    /// that starts makes the controls run. Only the end of the current run
    /// stops them: an older run's end can arrive after a newer run started.
    /// A refused resume stops them too.
    pub fn follow(&mut self, status: &Status) {
        match status {
            Status::Running { run, .. } => {
                self.run = Some(*run);
                self.running = true;
            }
            Status::Ended { run, .. } if self.run == Some(*run) => {
                self.run = None;
                self.running = false;
            }
            Status::Refused {
                intent: Intent::Resume,
                ..
            } => self.running = false,
            _ => {}
        }
    }

    /// An intent the session never received, because its lane was full or
    /// gone. A refused resume leaves the controls paused.
    pub fn refused(&mut self, intent: Intent) {
        if intent == Intent::Resume {
            self.running = false;
        }
    }

    /// The run the controls last saw start and not yet end.
    #[must_use]
    pub const fn run(&self) -> Option<u64> {
        self.run
    }

    /// Whether the controls last asked the session to run.
    #[must_use]
    pub const fn running(&self) -> bool {
        self.running
    }

    /// Whether a close was asked for.
    #[must_use]
    pub const fn close_requested(&self) -> bool {
        self.close
    }

    /// Whether the window has keyboard focus.
    #[must_use]
    pub const fn focused(&self) -> bool {
        self.focused
    }

    /// The last pointer position, in logical points.
    #[must_use]
    pub const fn pointer(&self) -> Option<LogicalPoint> {
        self.pointer
    }

    /// The buttons held.
    #[must_use]
    pub const fn buttons(&self) -> ButtonSet {
        self.buttons
    }

    /// The last reported surface geometry.
    #[must_use]
    pub const fn metrics(&self) -> Option<SurfaceMetrics> {
        self.metrics
    }

    /// Every event seen, by kind.
    #[must_use]
    pub const fn tally(&self) -> InputTally {
        self.tally
    }

    /// The drawable size the next frame should have: the content area at
    /// the screen's density ([`SurfaceMetrics::backing_size`]), one drawable
    /// pixel per device pixel. `None` before the geometry is known or while
    /// it is empty.
    #[must_use]
    pub fn drawable_size(&self) -> Option<[u32; 2]> {
        self.metrics?.backing_size()
    }

    /// The world point under the pointer in `viewport`, or `None` off the
    /// world or before the geometry is known.
    #[must_use]
    pub fn hover(&self, viewport: &Viewport) -> Option<[f64; 2]> {
        let pointer = self.pointer?;
        let metrics = self.metrics?;
        viewport.logical_to_world(
            [pointer.x, pointer.y],
            [metrics.logical_width, metrics.logical_height],
        )
    }
}
