//! Typed native input to Studio intents (plan §13.6).
//!
//! [`Controls`] is a pure state machine over the native host's
//! [`InputEvent`]s. It turns keys into [`Intent`]s for the session and keeps
//! what the presentation owner needs itself: the pointer, held buttons,
//! focus and the surface geometry. Camera changes and hit testing stay with
//! the presentation owner. Nothing here reaches the world; a completed
//! brush would become a canonical command through the session (FL-022).
//!
//! Keys, by virtual key code (`kVK_*`), so they do not depend on layout:
//!
//! | Key | Intent |
//! |---|---|
//! | Space | run or pause |
//! | → or `.` (held: repeats) | step one tick while paused |
//! | Esc, or ⌘W | close |
//!
//! and, for the presentation owner alone (FL-019's [`Style`]):
//!
//! | Key | Style |
//! |---|---|
//! | 1 to 5 | colour by lineage, energy, generation, motion or topology |
//! | R | glow or raw |
//! | D | points or density |
//! | F | field on or off |

use flife_manim::{ColorMapping, Look, Particles, Style, Viewport};
use fmn_render::metal::{ButtonSet, InputEvent, LogicalPoint, SurfaceMetrics};

use crate::Status;

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
/// `kVK_ANSI_1` to `kVK_ANSI_5`, for [`ColorMapping::ALL`] in order.
pub const KEY_DIGITS: [u16; 5] = [18, 19, 20, 21, 23];
/// `kVK_ANSI_R`.
pub const KEY_R: u16 = 15;
/// `kVK_ANSI_D`.
pub const KEY_D: u16 = 2;
/// `kVK_ANSI_F`.
pub const KEY_F: u16 = 3;

/// What the presentation owner asks of the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// Run until paused.
    Resume,
    /// Pause at the next committed boundary.
    Pause,
    /// Advance one tick while paused.
    Step,
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
                self.pointer = Some(*position);
                self.buttons = *buttons;
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
                None
            }
            InputEvent::Scroll { position, .. } => {
                // Zoom is FL-020's.
                self.tally.scrolls += 1;
                self.pointer = Some(*position);
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
                None
            }
            InputEvent::Resized(metrics) => {
                self.tally.resizes += 1;
                self.metrics = Some(*metrics);
                None
            }
        }
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
            _ => {}
        }
    }

    /// The style the presentation owner should draw in.
    #[must_use]
    pub const fn style(&self) -> Style {
        self.style
    }

    /// Start from `style`, as the Studio opens.
    pub const fn set_style(&mut self, style: Style) {
        self.style = style;
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

    /// The drawable size the next frame should have: the content area in
    /// whole points. The presenter draws one drawable pixel per point.
    #[must_use]
    pub fn drawable_size(&self) -> Option<[u32; 2]> {
        let metrics = self.metrics?;
        let whole = |points: f64| {
            if points.is_finite() && points >= 1.0 {
                // Bounded by the screen; the conversion saturates.
                Some(points.round() as u32)
            } else {
                None
            }
        };
        Some([
            whole(metrics.logical_width)?,
            whole(metrics.logical_height)?,
        ])
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
