//! The Studio's on-screen text (FL-024's metric overlay, plan §12.2): what
//! the session is doing, the honest clocks and what a click will do, as
//! lines the view draws in the drawable's top-left corner
//! ([`flife_manim::HudText`]). The persistent telemetry of plan §14.1
//! follows the clocks, from the shown epoch. The colour mapping's key is
//! one of the lines (FL-019's legend, plan §13.4), and while an organism is
//! selected its last inspection follows (FL-021's panel, plan §14.2).
//! While a brush is armed, a line says what a click at the pointer would
//! do (FL-022's preview, plan §14.3).

use flife_core::ids::OrganismId;
use flife_core::memory::MemoryAdmission;
use flife_core::{CoreError, Tick, ValidatedConfig};
use flife_engine::{Activations, OrganismSnapshot, Sensed};
use flife_genome::TopologyCatalog;
use flife_manim::{ColorMapping, Legend, Style};
use flife_render::Metrics;

use crate::{Brush, ClockReading, Preview, Stroke};

/// The field's key for a frame drawn in `style` (FL-019): the field
/// mapping's legend when the frame draws the field
/// ([`Style::field_drawn`]), none otherwise. The energy field, which the
/// field palette explains, has none.
#[must_use]
pub fn field_key(style: Style) -> Option<Legend> {
    style
        .field_drawn()
        .then(|| style.field_mapping.legend())
        .flatten()
}

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
    /// The field mapping's key ([`flife_manim::FieldMapping::legend`]),
    /// while the field is drawn in a mapping that has one: the line after
    /// the colour mapping's, its swatches after that key's.
    pub field_key: Option<&'a Legend>,
    /// The armed brush.
    pub brush: Option<Brush>,
    /// What a click of the armed brush at the pointer would do.
    pub preview: Option<BrushPreview>,
    /// The selected organism.
    pub selected: Option<OrganismId>,
    /// The selected organism's last inspection; `None` until one answers.
    pub inspected: Option<Inspected<'a>>,
    /// The persistent telemetry (plan §14.1); `None` shows none.
    pub telemetry: Option<Telemetry<'a>>,
}

/// What the persistent telemetry shows (FL-024, plan §14.1), besides the
/// tick and the clocks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Telemetry<'a> {
    /// The shown epoch's summary of its world.
    pub metrics: &'a Metrics,
    /// The run's model identity.
    pub model: &'a str,
    /// The run's seed.
    pub seed: u64,
    /// The configured neural cadence: `[controller]
    /// evaluation_interval_ticks`.
    pub evaluation_interval_ticks: u32,
    /// The peak-memory admission the Studio opened under (FL-043): the
    /// genome quota and the budget's headroom.
    pub memory: &'a MemoryAdmission,
    /// Capacity shortfalls the session reported: under
    /// `pause-before-commit`, the only capacity policy, a birth the quotas
    /// cannot cover pauses the run, so these are its capacity rejections.
    pub capacity_pauses: u64,
    /// The newest durable checkpoint's tick, if the session has one.
    pub checkpoint: Option<Tick>,
}

/// What a click of the armed brush at the pointer would do, as the overlay
/// says it (FL-022, plan §14.3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushPreview {
    /// The host's preview ([`crate::preview`]).
    pub preview: Preview,
    /// `[death] residual_energy_recycle_fraction`: the share of a removed
    /// organism's residual energy the removal brush, which recycles as a
    /// death does, returns to the field; the rest dissipates.
    pub recycle_fraction: f64,
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
/// - with a field key, the field's, such as `field composition ■ none
///   ■■■■■■ as affinity, darker is less energy`;
/// - with telemetry, its lines ([`telemetry_lines`]);
/// - `energy brush` while a brush is armed and `organism 42` while one is
///   selected, joined by ` · `;
/// - with a brush's preview, what a click would do ([`preview_line`]);
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
    if let Some(telemetry) = &state.telemetry {
        text.push('\n');
        text.push_str(&telemetry_lines(telemetry, state.clock));
    }
    text.push('\n');
    text.push_str(&state.legend.text);
    if let Some(key) = state.field_key {
        text.push('\n');
        text.push_str(&key.text);
    }
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
    if let Some(preview) = &state.preview {
        text.push('\n');
        text.push_str(&preview_line(preview));
    }
    if state.selected.is_some()
        && let Some(inspected) = state.inspected
    {
        text.push('\n');
        text.push_str(&inspector_lines(inspected));
    }
    text
}

/// The persistent telemetry's lines (plan §14.1), from the shown epoch's
/// [`Metrics`]:
///
/// - `4,612 live · 9,124,000 parameters`: the live population and its
///   logical neural parameters;
/// - `controllers every tick · 127.5 full evaluations/s`: the configured
///   neural cadence and, with a clock reading that counted evaluations, the
///   controller evaluations per wall second over the live population;
/// - `genomes 38.2 MiB of 1.13 GiB · 412 MiB headroom`: the bytes allocated
///   to live genomes' parameters, the allocated genome quota, and what the
///   memory budget leaves above the Studio's admitted peak;
/// - `18,586 births · 3 introduced · 18,255 deaths · no capacity pause · no
///   fault`, from the epoch's tally (the introductions only when there are
///   any); a tally from a later tick than the founding says
///   `from tick 512:` first, and without one the counts read `births and
///   deaths not counted`;
/// - `injected 1234.5 · uptake 456.7 · dissipated 789.1 · residual
///   1.2e-12`: the run ledger's cumulative injection (and `removed`, when
///   any was), the energy organisms gained by uptake from the tally
///   (`from tick 512` likewise, and left out without a tally), its
///   dissipation and its residual `Σ ε`;
/// - `field-ecology-v1 · seed 42 · no checkpoint`, or `checkpoint at tick
///   3,840`.
#[must_use]
pub fn telemetry_lines(telemetry: &Telemetry<'_>, clock: Option<&ClockReading>) -> String {
    let metrics = telemetry.metrics;
    let mut neural = match telemetry.evaluation_interval_ticks {
        1 => "controllers every tick".to_owned(),
        ticks => format!("controllers every {} ticks", grouped(u64::from(ticks))),
    };
    if let Some(rate) = clock.and_then(|clock| clock.evaluations_per_second)
        && metrics.live > 0
    {
        neural.push_str(&format!(
            " · {} full evaluations/s",
            decimal(rate / metrics.live as f64, 1)
        ));
    }
    let memory = telemetry.memory;
    let headroom = memory.budget_bytes.saturating_sub(memory.peak_bytes);
    let pauses = match telemetry.capacity_pauses {
        0 => "no capacity pause".to_owned(),
        1 => "1 capacity pause".to_owned(),
        pauses => format!("{} capacity pauses", grouped(pauses)),
    };
    let fault = if metrics.faulted {
        "a numerical fault stopped the run"
    } else {
        "no fault"
    };
    let from = |tally: &flife_render::Tally| {
        (tally.since != Tick::ZERO).then(|| format!("from tick {}", grouped(tally.since.get())))
    };
    let lifecycle = metrics.tally.as_ref().map_or_else(
        || "births and deaths not counted".to_owned(),
        |tally| {
            let mut counts = vec![format!("{} births", grouped(tally.births))];
            if tally.introductions > 0 {
                counts.push(format!("{} introduced", grouped(tally.introductions)));
            }
            counts.push(format!("{} deaths", grouped(tally.deaths)));
            let counts = counts.join(" · ");
            match from(tally) {
                Some(from) => format!("{from}: {counts}"),
                None => counts,
            }
        },
    );
    let ledger = &metrics.ledger;
    let mut energy = vec![format!("injected {}", decimal(ledger.injected, 1))];
    if ledger.removed > 0.0 {
        energy.push(format!("removed {}", decimal(ledger.removed, 1)));
    }
    if let Some(tally) = &metrics.tally {
        let uptake = format!("uptake {}", decimal(tally.uptake, 1));
        energy.push(match from(tally) {
            Some(from) => format!("{uptake} {from}"),
            None => uptake,
        });
    }
    energy.push(format!("dissipated {}", decimal(ledger.dissipated, 1)));
    energy.push(format!("residual {:.1e}", ledger.residual));
    let checkpoint = telemetry.checkpoint.map_or_else(
        || "no checkpoint".to_owned(),
        |tick| format!("checkpoint at tick {}", grouped(tick.get())),
    );
    format!(
        "{} live · {} parameters\n\
         {neural}\n\
         genomes {} of {} · {} headroom\n\
         {lifecycle} · {pauses} · {fault}\n\
         {}\n\
         {} · seed {} · {checkpoint}",
        grouped(metrics.live),
        grouped(metrics.parameters),
        bytes(metrics.genome_allocated_bytes),
        bytes(memory.genomes),
        bytes(headroom),
        energy.join(" · "),
        telemetry.model,
        telemetry.seed,
    )
}

/// `value` bytes in binary units to three significant figures: `512 B`,
/// `38.2 MiB`, `1.13 GiB`.
fn bytes(value: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    let mut scaled = value as f64;
    let mut unit = 0;
    while scaled >= 1024.0 && unit + 1 < UNITS.len() {
        scaled /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{value} B");
    }
    let places = if scaled >= 100.0 {
        0
    } else if scaled >= 10.0 {
        1
    } else {
        2
    };
    format!("{scaled:.places$} {}", UNITS[unit])
}

/// What a click would do (plan §14.3): the energy it adds or removes, in
/// the model's energy units, and the region it affects, in world units.
///
/// - `a click adds 64.0 energy to cell 12, 30 (x 12.00 to 13.00, y 30.00
///   to 31.00 world units)`;
/// - `a click adds 64.0 energy of the selected organism's material to cell
///   12, 30 (…)`;
/// - `a click adds 8 organisms of 2.00 energy, 16.0 in all, to cell 12, 30
///   (…)`;
/// - `a click removes the organisms within 1.23 world units of the
///   pointer: 50% of their energy to the field, the rest dissipated, none
///   out of the world`;
/// - `a click does nothing: the pointer is off the world`, or `…: select
///   an organism for its material`.
///
/// A cell's bounds have three significant figures of its width, and a
/// radius three of its own; from 100,000 a figure is in scientific
/// notation.
#[must_use]
pub fn preview_line(preview: &BrushPreview) -> String {
    let place = |cell: [u32; 2], region: Option<[[f64; 2]; 2]>| {
        let at = format!(
            "cell {}, {}",
            grouped(u64::from(cell[0])),
            grouped(u64::from(cell[1]))
        );
        match region {
            Some([[x0, x1], [y0, y1]]) => {
                let [x, y] = [x1 - x0, y1 - y0].map(places);
                format!(
                    "{at} (x {} to {}, y {} to {} world units)",
                    decimal(x0, x),
                    decimal(x1, x),
                    decimal(y0, y),
                    decimal(y1, y)
                )
            }
            None => at,
        }
    };
    match preview.preview {
        Preview::Stroke { stroke, region } => match stroke {
            Stroke::Energy { cell, amount } => format!(
                "a click adds {} energy to {}",
                decimal(amount, 1),
                place(cell, region)
            ),
            Stroke::Material { cell, amount, .. } => format!(
                "a click adds {} energy of the selected organism's material to {}",
                decimal(amount, 1),
                place(cell, region)
            ),
            Stroke::Introduction {
                cell,
                count,
                energy,
            } => format!(
                "a click adds {} organisms of {} energy, {} in all, to {}",
                grouped(u64::from(count)),
                decimal(energy, 2),
                decimal(f64::from(count) * energy, 1),
                place(cell, region)
            ),
            Stroke::Removal { radius, .. } => format!(
                "a click removes the organisms within {} world units of the pointer: {}% of \
                 their energy to the field, the rest dissipated, none out of the world",
                decimal(radius, places(radius)),
                decimal(preview.recycle_fraction * 100.0, 0)
            ),
        },
        Preview::OffWorld => "a click does nothing: the pointer is off the world".to_owned(),
        Preview::NoMaterial => {
            "a click does nothing: select an organism for its material".to_owned()
        }
    }
}

/// The decimal places that give `scale` three significant figures, from 0
/// to 9; 2 for a scale that is not positive and finite.
fn places(scale: f64) -> usize {
    if !(scale.is_finite() && scale > 0.0) {
        return 2;
    }
    // A finite positive scale's floor(log10) is within ±400.
    (2.0 - scale.log10().floor()).clamp(0.0, 9.0) as usize
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
///   holds, or `no controller output held`;
/// - `sensed at tick 5,119 · scale 1.50 · field 0.412` and `gradient 0.031
///   toward 45° · affinity·Z +0.112`: what it observed in the newest tick
///   the world recorded for it ([`Sensed`]), at its sensing scale: the
///   field input `q = R/(R + R_scale)`, the length of its unsaturated
///   gradient and its direction as the heading's, and its affinity
///   direction's dot product with the composition inputs `Z`; or `sensed
///   from its next tick` before the world has recorded one;
/// - `activations at tick 5,120 · max |a| 0.98, 0.97 · mean |a| 0.31, 0.44`:
///   each hidden layer's largest and mean activation magnitude, in
///   evaluation order, of the evaluation its held output came from
///   ([`Activations`]); or `activations at its next evaluation`.
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
        format!(
            "speed {}/s · heading {:.0}°",
            decimal(speed, 2),
            degrees([vx, vy])
        )
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
    let sensed = snapshot.sensed.as_ref().map_or_else(
        || "sensed from its next tick".to_owned(),
        |sensed| sensed_lines(sensed, snapshot.traits.affinity()),
    );
    let activations = snapshot.activations.as_ref().map_or_else(
        || "activations at its next evaluation".to_owned(),
        activation_line,
    );
    format!(
        "read at tick {} · lineage {} · generation {}\n\
         energy {} · {}/s · age {} ticks\n\
         {motion}\n\
         hidden {hidden} · {} parameters\n\
         {actions}\n\
         {sensed}\n\
         {activations}",
        grouped(snapshot.tick.get()),
        grouped(organism.lineage.get()),
        grouped(u64::from(organism.generation.get())),
        decimal(organism.energy, 2),
        signed(organism.energy_change_rate, 3),
        grouped(snapshot.age_ticks),
        grouped(u64::try_from(snapshot.parameters).unwrap_or(u64::MAX)),
    )
}

/// A direction in degrees counter-clockwise on screen from east, as a
/// heading is written; world y points down.
fn degrees([x, y]: [f64; 2]) -> f64 {
    // Adding zero turns a direction of -0 into 0.
    (-y).atan2(x).to_degrees().rem_euclid(360.0).round() % 360.0 + 0.0
}

/// [`inspector_lines`]' two lines for what an organism sensed.
fn sensed_lines(sensed: &Sensed, affinity: &[f64]) -> String {
    let inputs = &sensed.observation.inputs;
    let field = inputs.first().copied().unwrap_or(0.0);
    let [gx, gy] = sensed
        .observation
        .gradients
        .first()
        .copied()
        .unwrap_or([0.0; 2]);
    let length = gx.hypot(gy);
    let gradient = if length > 0.0 && length.is_finite() {
        format!(
            "gradient {} toward {:.0}°",
            decimal(length, 3),
            degrees([gx, gy])
        )
    } else {
        "no gradient".to_owned()
    };
    // The composition inputs Z_0, …, Z_31 follow the field input.
    let along: f64 = inputs
        .iter()
        .skip(1)
        .zip(affinity)
        .map(|(z, a)| z * a)
        .sum();
    format!(
        "sensed at tick {} · scale {} · field {}\n{gradient} · affinity·Z {}",
        grouped(sensed.tick.get()),
        decimal(sensed.scale, 2),
        decimal(field, 3),
        signed(along, 3),
    )
}

/// [`inspector_lines`]' line for a held output's hidden activations.
fn activation_line(activations: &Activations) -> String {
    let magnitudes = |summary: fn(&[f32]) -> f64| {
        activations
            .hidden
            .iter()
            .map(|layer| decimal(summary(layer), 2))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let largest = magnitudes(|layer| {
        layer
            .iter()
            .map(|value| f64::from(value.abs()))
            .fold(0.0, f64::max)
    });
    let mean = magnitudes(|layer| {
        let total: f64 = layer.iter().map(|value| f64::from(value.abs())).sum();
        // A layer's width is a few hundred at most: the count is exact.
        total / layer.len().max(1) as f64
    });
    format!(
        "activations at tick {} · max |a| {largest} · mean |a| {mean}",
        grouped(activations.tick.get())
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
