//! The Studio's on-screen text (FL-024's metric overlay, plan §12.2): what
//! the session is doing, the honest clocks and what a click will do, as
//! lines the view draws in the drawable's top-left corner
//! ([`flife_manim::HudText`]). While an organism is selected, its last
//! inspection follows (FL-021's panel, plan §14.2).

use flife_core::Tick;
use flife_core::ids::OrganismId;
use flife_engine::OrganismSnapshot;

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
    /// The selected organism's last inspection; `None` until one answers.
    pub inspected: Option<Inspected<'a>>,
}

/// An inspection's answer, as the overlay shows it (FL-021).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Inspected<'a> {
    /// The organism at the session's committed boundary.
    Read(&'a OrganismSnapshot),
    /// The committed world no longer holds it.
    Gone,
}

/// The overlay's lines:
///
/// - `running · tick 1,024` (or `paused`, or `opening` before the session
///   has said);
/// - with a clock reading, `128 ticks/s · 1.00× real time · 60 fps`: the
///   ticks committed per wall second (one decimal below 100), simulated
///   seconds per wall second and frames presented per second;
/// - the colour mapping, then ` · energy brush` while a brush is armed and
///   ` · organism 42` while one is selected;
/// - while one is selected and an inspection has answered, the panel
///   ([`inspector_lines`]).
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
        if let Some(inspected) = state.inspected {
            text.push('\n');
            text.push_str(&inspector_lines(inspected));
        }
    }
    text
}

/// The inspector panel's lines for an answer:
///
/// - `read at tick 5,120 · lineage 17 · generation 42`: the committed
///   boundary it was read at, so a running world's panel says how old it is;
/// - `energy 3.21 · +0.012/s · age 1,024 ticks`: its energy, the energy's
///   change per simulated second over its last tick, and ticks since its
///   birth;
/// - `speed 0.84/s · heading 135°` in world units per simulated second, the
///   heading counter-clockwise on screen from east as the motion mapping's
///   wheel turns, or `still`;
/// - `hidden 64, 32 · 9,124 parameters`: its controller's hidden widths, in
///   evaluation order, and parameter count;
/// - `reproduction 3.1% a tick · emission gate 0.72` from the output it
///   holds, or `no controller output held`.
///
/// Or `not in the committed world` when it is gone. A magnitude of 100,000
/// or more is written in scientific notation, so no line grows without
/// bound.
#[must_use]
pub fn inspector_lines(inspected: Inspected<'_>) -> String {
    let Inspected::Read(snapshot) = inspected else {
        return "not in the committed world".to_owned();
    };
    let organism = &snapshot.organism;
    let [vx, vy] = organism.velocity;
    let speed = vx.hypot(vy);
    let motion = if speed > 0.0 && speed.is_finite() {
        // World y points down; the heading turns counter-clockwise on screen.
        // Adding zero turns a heading of -0 into 0.
        let degrees = (-vy).atan2(vx).to_degrees().rem_euclid(360.0).round() % 360.0 + 0.0;
        format!("speed {}/s · heading {degrees:.0}°", decimal(speed, 2))
    } else {
        "still".to_owned()
    };
    let hidden = snapshot
        .hidden_widths
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let actions = snapshot.decoded.map_or_else(
        || "no controller output held".to_owned(),
        |decoded| {
            format!(
                "reproduction {}% a tick · emission gate {}",
                decimal(100.0 * decoded.attempt_probability, 1),
                decimal(decoded.emission_gate, 2)
            )
        },
    );
    format!(
        "read at tick {} · lineage {} · generation {}\n\
         energy {} · {}/s · age {} ticks\n\
         {motion}\n\
         hidden {hidden} · {} parameters\n\
         {actions}",
        grouped(snapshot.tick.get()),
        grouped(organism.lineage.get()),
        grouped(u64::from(organism.generation.get())),
        decimal(organism.energy, 2),
        signed(organism.energy_change_rate, 3),
        grouped(snapshot.age_ticks),
        grouped(u64::try_from(snapshot.parameters).unwrap_or(u64::MAX)),
    )
}

/// `value` to `places` decimals, or in scientific notation from 100,000.
fn decimal(value: f64, places: usize) -> String {
    if value.abs() >= 1e5 {
        format!("{value:.2e}")
    } else {
        format!("{value:.places$}")
    }
}

/// [`decimal`] with its sign.
fn signed(value: f64, places: usize) -> String {
    let text = decimal(value, places);
    if text.starts_with('-') {
        text
    } else {
        format!("+{text}")
    }
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
