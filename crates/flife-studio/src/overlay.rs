//! The Studio's on-screen text (FL-024's metric overlay, plan §12.2): what
//! the session is doing, the honest clocks and what a click will do, as
//! lines the view draws in the drawable's top-left corner
//! ([`flife_manim::HudText`]).

use flife_core::Tick;
use flife_core::ids::OrganismId;

use crate::{Brush, ClockReading};

/// What the overlay shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayState<'a> {
    /// Whether the session last reported itself paused (`Some(true)`) or
    /// running; `None` before it has reported either.
    pub paused: Option<bool>,
    /// The shown epoch's tick.
    pub shown: Option<Tick>,
    /// The last clock reading, while `[presentation] show_clock_telemetry`
    /// is set.
    pub clock: Option<&'a ClockReading>,
    /// The colour mapping's name.
    pub mapping: &'static str,
    /// The armed brush.
    pub brush: Option<Brush>,
    /// The selected organism.
    pub selected: Option<OrganismId>,
}

/// The overlay's lines:
///
/// - `running · tick 1,024` (or `paused`, or `opening` before the session
///   has said);
/// - with a clock reading, `128 ticks/s · 1.00× real time · 60 fps`: the
///   ticks committed per wall second (one decimal below 100), simulated
///   seconds per wall second and frames presented per second;
/// - the colour mapping, then ` · energy brush` while a brush is armed and
///   ` · organism 42` while one is selected.
#[must_use]
pub fn overlay_text(state: &OverlayState<'_>) -> String {
    let run = match state.paused {
        Some(true) => "paused",
        Some(false) => "running",
        None => "opening",
    };
    let mut text = match state.shown {
        Some(tick) => format!("{run} · tick {}", grouped(tick.get())),
        None => run.to_owned(),
    };
    if let Some(clock) = state.clock {
        let rate = if clock.ticks_per_second >= 100.0 {
            format!("{:.0}", clock.ticks_per_second)
        } else {
            format!("{:.1}", clock.ticks_per_second)
        };
        text.push_str(&format!(
            "\n{rate} ticks/s · {:.2}× real time · {:.0} fps",
            clock.simulation_rate, clock.frames_per_second
        ));
    }
    text.push('\n');
    text.push_str(state.mapping);
    if let Some(brush) = state.brush {
        text.push_str(&format!(" · {} brush", brush.name()));
    }
    if let Some(organism) = state.selected {
        text.push_str(&format!(" · organism {}", grouped(organism.get())));
    }
    text
}

/// `value` with its thousands separated by commas.
fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}
