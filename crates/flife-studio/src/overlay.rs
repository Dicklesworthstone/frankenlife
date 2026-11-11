//! The Studio's on-screen text (FL-024's metric overlay, plan §12.2): what
//! the session is doing, the honest clocks and what a click will do, as
//! lines the view draws in the drawable's top-left corner
//! ([`flife_manim::HudText`]). The colour mapping's key is one of them
//! (FL-019's legend, plan §13.4), and while an organism is selected its
//! last inspection follows (FL-021's panel, plan §14.2).

use flife_core::ids::OrganismId;
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_engine::OrganismSnapshot;
use flife_genome::TopologyCatalog;
use flife_manim::{ColorMapping, Legend};

use crate::{Brush, ClockReading};

/// Every colour mapping's key for the run `config` configures, in
/// [`ColorMapping::ALL`]'s order: the topology key labels the run's catalog
/// members with their hidden widths.
///
/// # Errors
/// As [`TopologyCatalog::from_config`].
pub fn legends(config: &ValidatedConfig) -> Result<Vec<(ColorMapping, Legend)>, CoreError> {
    let catalog = TopologyCatalog::from_config(config)?;
    let widths: Vec<&[u32]> = catalog
        .ids()
        .filter_map(|id| catalog.topology(id))
        .map(|topology| topology.hidden_widths())
        .collect();
    Ok(ColorMapping::ALL
        .into_iter()
        .map(|mapping| (mapping, mapping.legend(&widths)))
        .collect())
}

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
    /// The colour mapping's key: its swatches are the only
    /// [`flife_manim::HUD_SWATCH`]es in the text, so its `swatches` colour
    /// them in order.
    pub legend: &'a Legend,
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
/// - the colour mapping's key ([`ColorMapping::legend`]), such as
///   `energy ■ 0 ■ 1 ■ 2 ■ 4 ■ 8 ■ 16`;
/// - `energy brush` while a brush is armed and `organism 42` while one is
///   selected, joined by ` · `;
/// - while one is selected and an inspection has answered, the panel
///   ([`inspector_lines`]).
///
/// A count of a trillion or more, and a magnitude of 100,000 or more, is
/// written in scientific notation, so no line grows without bound.
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
        let places = if clock.ticks_per_second >= 100.0 {
            0
        } else {
            1
        };
        text.push_str(&format!(
            "\n{} ticks/s · {}× real time · {} fps",
            decimal(clock.ticks_per_second, places),
            decimal(clock.simulation_rate, 2),
            decimal(clock.frames_per_second, 0)
        ));
    }
    text.push('\n');
    text.push_str(&state.legend.text);
    let mut doing = Vec::new();
    if let Some(brush) = state.brush {
        doing.push(format!("{} brush", brush.name()));
    }
    if let Some(organism) = state.selected {
        doing.push(format!("organism {}", grouped(organism.get())));
    }
    if !doing.is_empty() {
        text.push('\n');
        text.push_str(&doing.join(" · "));
    }
    if state.selected.is_some()
        && let Some(inspected) = state.inspected
    {
        text.push('\n');
        text.push_str(&inspector_lines(inspected));
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

/// The smallest count [`grouped`] writes in scientific notation.
const TRILLION: u64 = 1_000_000_000_000;

/// `value` with its thousands separated by commas, or in scientific
/// notation from a trillion: no tick, identity or count of a run reaches
/// one, and the line stays bounded if one does.
fn grouped(value: u64) -> String {
    if value >= TRILLION {
        // Three significant digits are shown, so the cast loses nothing seen.
        return format!("{:.2e}", value as f64);
    }
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
