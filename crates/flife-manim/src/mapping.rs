//! FL-019's visual mappings (plan §2.3, §13.4): what a particle's colour
//! means, and how a frame is drawn.
//!
//! Every mapping is a fixed function of one epoch row and the world's own
//! constants. It never reads the other rows, earlier frames or a fitted
//! statistic, so an unchanged organism keeps its colour, and one epoch
//! draws the same colours in every frame. Colours are for display only: no
//! metric reads them back.
//!
//! | Mapping | Reads | Colour |
//! |---|---|---|
//! | lineage | lineage id | [`LINEAGE_PALETTE`] slot [`lineage_slot`] (categorical) |
//! | energy | energy `e` | [`ENERGY_RAMP`] at `e / (e + ENERGY_HALF)` (monotone lightness) |
//! | generation | generation `g` | [`GENERATION_RAMP`] at `ln(1 + g) / ln(1 + GENERATION_SCALE)` |
//! | motion | velocity | [`MOTION_WHEEL`] hue for the heading, mixed from [`MOTION_STILL`] by `√(speed / speed limit)` |
//! | topology | catalog index `k` | [`TOPOLOGY_PALETTE`]`[k mod 8]`, catalog members in size order |
//! | affinity | affinity coordinates `q` ([`flife_render::affinity_projection`]) | [`affinity_rgb8`]: OKLab hue and chroma from `q₁, q₂`, lightness from `q₃` |
//! | lineage-energy | lineage id, energy `e`, the world's attempt energy `θ` | [`lineage_energy_rgb8`]: the lineage's palette hue and chroma at OKLab lightness [`LINEAGE_ENERGY_LIGHTNESS`]` × e / θ` |
//!
//! The field under the organisms has its own mapping ([`FieldMapping`],
//! plan §13.4's "Local field"): its energy density through the look's
//! palette; its mean chemical composition by the affinity mapping's
//! colours (the fixed projection); or one chosen moment channel per unit
//! energy through the diverging [`COMPONENT_PALETTE`](crate::COMPONENT_PALETTE) (chosen components);
//! the last two as bright as its energy.

use std::fmt;
use std::str::FromStr;
use std::sync::LazyLock;

use flife_core::registry::{self, Role};
use flife_core::{CoreError, ValidatedConfig};
use flife_render::Epoch;
use fmn_core::color::{LinearRgba, Oklab, Srgb, oklab_to_srgb, srgb_to_oklab};

/// What a particle's colour means (plan §13.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ColorMapping {
    /// Founder lineage: one stable categorical colour per lineage.
    #[default]
    Lineage,
    /// Energy: a monotone dark-to-bright ramp.
    Energy,
    /// Generation: an explicit logarithmic scale from the founders.
    Generation,
    /// Motion: hue for the heading, saturation for the speed.
    Motion,
    /// Neural architecture: one colour per catalog topology.
    Topology,
    /// Chemical affinity: a fixed projection of the heritable affinity
    /// direction (plan §13.4).
    Affinity,
    /// Founder lineage and energy: the lineage's colour, as light as the
    /// organism's energy against the energy it needs to attempt a birth.
    LineageEnergy,
}

impl ColorMapping {
    /// Every mapping, in the Studio's key order (1 to 7).
    pub const ALL: [Self; 7] = [
        Self::Lineage,
        Self::Energy,
        Self::Generation,
        Self::Motion,
        Self::Topology,
        Self::Affinity,
        Self::LineageEnergy,
    ];

    /// The mapping's name, as flags and records spell it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Lineage => "lineage",
            Self::Energy => "energy",
            Self::Generation => "generation",
            Self::Motion => "motion",
            Self::Topology => "topology",
            Self::Affinity => "affinity",
            Self::LineageEnergy => "lineage-energy",
        }
    }

    /// The registered name a configuration's `[presentation]
    /// color_mapping` uses for it.
    #[must_use]
    pub const fn registry_name(self) -> &'static str {
        match self {
            Self::Lineage => "founder-lineage-v1",
            Self::Energy => "energy-lightness-v1",
            Self::Generation => "generation-log-v1",
            Self::Motion => "motion-heading-v1",
            Self::Topology => "topology-catalog-v1",
            Self::Affinity => "affinity-projection-v1",
            Self::LineageEnergy => "founder-lineage-energy-v1",
        }
    }

    /// The mapping a registered name stands for.
    #[must_use]
    pub fn from_registry(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mapping| mapping.registry_name() == name)
    }

    /// The mapping `config`'s `[presentation] color_mapping` names.
    ///
    /// # Errors
    /// [`CoreError::UnknownAlgorithm`] for a name that is not a registered
    /// colour mapping this crate draws.
    pub fn configured(config: &ValidatedConfig) -> Result<Self, CoreError> {
        let name = &config.config().presentation.color_mapping;
        registry::lookup(Role::ColorMapping, name)?;
        Self::from_registry(name).ok_or_else(|| CoreError::UnknownAlgorithm {
            role: Role::ColorMapping.key(),
            name: name.clone(),
        })
    }

    /// What the colour means, for records and legends.
    #[must_use]
    pub const fn meaning(self) -> &'static str {
        match self {
            Self::Lineage => "founder lineage: a stable categorical colour per lineage",
            Self::Energy => "energy: dark violet (none) to pale yellow, half way at 2.0",
            Self::Generation => {
                "generation: purple (founders) to yellow (256 or more), logarithmic"
            }
            Self::Motion => {
                "motion: hue is the heading, grey to full colour the square root of the speed"
            }
            Self::Topology => "neural topology: cyan, teal, lime, amber, magenta from smallest",
            Self::Affinity => {
                "chemical affinity: a fixed projection of the 32 affinity terms; grey is none, \
                 like colours like chemistry, opposite directions opposite hues"
            }
            Self::LineageEnergy => {
                "founder lineage lit by energy: the lineage's colour (lineage-energy), lighter as \
                 the organism holds more of the energy it needs to divide, black with none, white \
                 with 1.8 times it"
            }
        }
    }
}

/// The energies the lineage-energy mapping's key shows, as fractions of the
/// energy an organism needs to attempt a birth.
pub const LEGEND_ATTEMPT_FRACTIONS: [f32; 4] = [0.25, 0.5, 1.0, 1.5];

/// The energies the energy mapping's key shows.
pub const LEGEND_ENERGIES: [f32; 6] = [0.0, 1.0, 2.0, 4.0, 8.0, 16.0];

/// The generations the generation mapping's key shows: the last is
/// [`GENERATION_SCALE`], where the ramp ends.
pub const LEGEND_GENERATIONS: [u32; 6] = [0, 1, 3, 15, 63, GENERATION_SCALE];

/// The headings the motion mapping's key shows, as fractions of a turn
/// counter-clockwise on screen from east, at full speed.
pub const LEGEND_HEADINGS: [(&str, f64); 4] = [
    ("east", 0.0),
    ("north", 0.25),
    ("west", 0.5),
    ("south", 0.75),
];

/// The affinity mapping's key: no affinity, then six directions in its
/// first two coordinates, `k · 60°` from the first, at the radius a
/// founder's direction typically takes (`2 / AFFINITY_GAIN`).
#[must_use]
pub fn legend_affinities() -> [[f32; 3]; 7] {
    let radius = 2.0 / AFFINITY_GAIN;
    let mut out = [[0.0_f32; 3]; 7];
    for (k, q) in out.iter_mut().skip(1).enumerate() {
        let angle = (k as f64) * std::f64::consts::TAU / 6.0;
        *q = [
            (radius * angle.cos()) as f32,
            (radius * angle.sin()) as f32,
            0.0,
        ];
    }
    out
}

/// A colour mapping's key (FL-019, plan §13.4): one line of text in which
/// each [`HUD_SWATCH`](crate::HUD_SWATCH) is drawn in the next of
/// `swatches`, the colour the mapping gives what follows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Legend {
    /// The line: the mapping's name, then the swatches and their labels.
    pub text: String,
    /// Each swatch's sRGB colour, in order.
    pub swatches: Vec<[u8; 3]>,
}

impl ColorMapping {
    /// The mapping's key. `topologies` are the hidden widths of the run's
    /// catalog members in catalog order, which the topology key labels.
    ///
    /// - lineage: the 12 palette colours, `by founder` (a lineage's colour is
    ///   a fixed hash of its identity, so lineages share them);
    /// - energy: [`LEGEND_ENERGIES`] on the ramp;
    /// - generation: [`LEGEND_GENERATIONS`], the last `256+`;
    /// - motion: [`LEGEND_HEADINGS`] at full speed, then `still`, `greyer is
    ///   slower`;
    /// - topology: each member's colour and hidden widths;
    /// - affinity: [`legend_affinities`]: `none`, then the six directions,
    ///   `like hue, like chemistry`;
    /// - lineage-energy: the first palette colour at [`LEGEND_ATTEMPT_FRACTIONS`]
    ///   of the energy to divide, then `x energy to divide`.
    #[must_use]
    pub fn legend(self, topologies: &[&[u32]]) -> Legend {
        let swatch = crate::HUD_SWATCH;
        let mut text = self.name().to_owned();
        let mut swatches = Vec::new();
        let mut labelled = |text: &mut String, color: [u8; 3], label: &str| {
            text.push_str(&format!(" {swatch} {label}"));
            swatches.push(color);
        };
        match self {
            Self::Lineage => {
                text.push(' ');
                for color in LINEAGE_PALETTE {
                    text.push(swatch);
                    swatches.push(color);
                }
                text.push_str(" by founder");
            }
            Self::Energy => {
                for energy in LEGEND_ENERGIES {
                    let color = ramp(&ENERGY_RAMP, energy_level(energy));
                    labelled(&mut text, color, &energy.to_string());
                }
            }
            Self::Generation => {
                for generation in LEGEND_GENERATIONS {
                    let color = ramp(&GENERATION_RAMP, generation_level(generation));
                    let label = if generation == GENERATION_SCALE {
                        format!("{generation}+")
                    } else {
                        generation.to_string()
                    };
                    labelled(&mut text, color, &label);
                }
            }
            Self::Motion => {
                for (label, turn) in LEGEND_HEADINGS {
                    labelled(&mut text, wheel(turn), label);
                }
                labelled(&mut text, MOTION_STILL, "still, greyer is slower");
            }
            Self::Topology => {
                for (index, widths) in topologies.iter().enumerate() {
                    let label = widths
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(", ");
                    labelled(
                        &mut text,
                        TOPOLOGY_PALETTE[index % TOPOLOGY_PALETTE.len()],
                        &label,
                    );
                }
            }
            Self::Affinity => {
                let [none, directions @ ..] = legend_affinities();
                labelled(&mut text, affinity_rgb8(none), "none");
                text.push(' ');
                for q in directions {
                    text.push(swatch);
                    swatches.push(affinity_rgb8(q));
                }
                text.push_str(" like hue, like chemistry");
            }
            Self::LineageEnergy => {
                for fraction in LEGEND_ATTEMPT_FRACTIONS {
                    let color = palette_shade(0, fraction, 1.0);
                    labelled(&mut text, color, &fraction.to_string());
                }
                text.push_str(" x energy to divide");
            }
        }
        Legend { text, swatches }
    }
}

impl fmt::Display for ColorMapping {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl FromStr for ColorMapping {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|mapping| mapping.name() == text || mapping.registry_name() == text)
            .ok_or_else(|| {
                format!(
                    "`{text}` is not a colour mapping: lineage, energy, generation, motion, topology, \
                     affinity or lineage-energy"
                )
            })
    }
}

/// How a frame is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Look {
    /// Glow halos sized by energy; the field through a square-root palette.
    #[default]
    Glow,
    /// The raw diagnostic view (plan §2.3): no halos or other blending
    /// around the dots, and the field in linear grey.
    Raw,
    /// Raw, with each dot lit as a sphere (FrankenManim's true-dot shading)
    /// at the organism's body size: no halos, no bloom, the field in linear
    /// grey.
    Sphere,
}

impl Look {
    /// The look's name, as flags and records spell it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Glow => "glow",
            Self::Raw => "raw",
            Self::Sphere => "sphere",
        }
    }
}

impl FromStr for Look {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "glow" => Ok(Self::Glow),
            "raw" => Ok(Self::Raw),
            "sphere" => Ok(Self::Sphere),
            _ => Err(format!("`{text}` is not a look: glow, raw or sphere")),
        }
    }
}

/// How organisms are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Particles {
    /// One dot per organism.
    #[default]
    Points,
    /// Organism counts and mean colours per screen bin (plan §2.3, §13.3).
    Density,
}

impl Particles {
    /// The name, as flags and records spell it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Points => "points",
            Self::Density => "density",
        }
    }
}

/// The moment channels a field component can name:
/// [`flife_core::model::MOMENT_CHANNELS`].
pub const FIELD_COMPONENTS: u8 = 32;

/// The component mappings' names, as flags and records spell them.
const COMPONENT_NAMES: [&str; FIELD_COMPONENTS as usize] = [
    "component-0",
    "component-1",
    "component-2",
    "component-3",
    "component-4",
    "component-5",
    "component-6",
    "component-7",
    "component-8",
    "component-9",
    "component-10",
    "component-11",
    "component-12",
    "component-13",
    "component-14",
    "component-15",
    "component-16",
    "component-17",
    "component-18",
    "component-19",
    "component-20",
    "component-21",
    "component-22",
    "component-23",
    "component-24",
    "component-25",
    "component-26",
    "component-27",
    "component-28",
    "component-29",
    "component-30",
    "component-31",
];

/// One of the field's moment channels, `0` to `31` (plan §13.4, "Local
/// field": chosen components).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct FieldComponent(u8);

impl FieldComponent {
    /// Channel `channel`, or `None` past the last.
    #[must_use]
    pub const fn new(channel: u8) -> Option<Self> {
        if channel < FIELD_COMPONENTS {
            Some(Self(channel))
        } else {
            None
        }
    }

    /// The channel's index.
    #[must_use]
    pub const fn channel(self) -> u8 {
        self.0
    }

    /// The next channel, the first after the last.
    #[must_use]
    pub const fn next(self) -> Self {
        Self((self.0 + 1) % FIELD_COMPONENTS)
    }

    /// The previous channel, the last before the first.
    #[must_use]
    pub const fn previous(self) -> Self {
        Self((self.0 + FIELD_COMPONENTS - 1) % FIELD_COMPONENTS)
    }
}

/// What the field's colour means (plan §13.4, "Local field").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FieldMapping {
    /// The texels' mean energy density: the glow look's palette, or the
    /// raw look's linear grey.
    #[default]
    Energy,
    /// The texels' mean chemical composition
    /// ([`flife_render::FieldTexture::composition`]), coloured as the
    /// affinity mapping colours an organism ([`affinity_rgb8`]) and as
    /// bright as the energy density shows: like hue, like chemistry, so an
    /// organism at its norm limit shares the hue of the material it
    /// converts best.
    Composition,
    /// One moment channel per unit energy
    /// ([`flife_render::FieldTexture::components`]): a corner the
    /// [`COMPONENT_PALETTE`](crate::COMPONENT_PALETTE) colour of its texels' density-weighted mean,
    /// from −1 (the channel wholly against the energy) through 0 to +1
    /// (wholly along it), and as bright as the energy density shows.
    Component(FieldComponent),
}

impl FieldMapping {
    /// Every field mapping: energy, composition, then each component in
    /// channel order.
    pub const ALL: [Self; 2 + FIELD_COMPONENTS as usize] = {
        let mut all = [Self::Energy; 2 + FIELD_COMPONENTS as usize];
        all[1] = Self::Composition;
        let mut channel = 0;
        while channel < FIELD_COMPONENTS {
            all[2 + channel as usize] = Self::Component(FieldComponent(channel));
            channel += 1;
        }
        all
    };

    /// The mapping's name, as flags and records spell it: `energy`,
    /// `composition` or `component-C` for channel `C`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Energy => "energy",
            Self::Composition => "composition",
            Self::Component(component) => {
                COMPONENT_NAMES[(component.0 % FIELD_COMPONENTS) as usize]
            }
        }
    }

    /// What the Studio's C key switches to: energy, composition, then
    /// `component`, then energy again.
    #[must_use]
    pub const fn next(self, component: FieldComponent) -> Self {
        match self {
            Self::Energy => Self::Composition,
            Self::Composition => Self::Component(component),
            Self::Component(_) => Self::Energy,
        }
    }

    /// The component shown, in a component mapping.
    #[must_use]
    pub const fn component(self) -> Option<FieldComponent> {
        match self {
            Self::Component(component) => Some(component),
            Self::Energy | Self::Composition => None,
        }
    }

    /// The mapping's key, for a mapping the energy palette does not
    /// explain: the composition's is the affinity key's colours
    /// ([`legend_affinities`]), `none` and then the six directions, at full
    /// brightness; a component's is [`COMPONENT_PALETTE`](crate::COMPONENT_PALETTE) at −1, −½, 0, ½
    /// and 1; a texel shows its colour darkened as its energy is lower.
    /// `None` for energy.
    #[must_use]
    pub fn legend(self) -> Option<Legend> {
        match self {
            Self::Energy => None,
            Self::Composition => {
                let swatch = crate::HUD_SWATCH;
                let [none, directions @ ..] = legend_affinities();
                let mut text = format!("field composition {swatch} none ");
                let mut swatches = vec![affinity_rgb8(none)];
                for q in directions {
                    text.push(swatch);
                    swatches.push(affinity_rgb8(q));
                }
                text.push_str(" as affinity, darker is less energy");
                Some(Legend { text, swatches })
            }
            Self::Component(component) => {
                let swatch = crate::HUD_SWATCH;
                let text = format!(
                    "field channel {} / energy {swatch} -1 {swatch} {swatch} 0 {swatch} \
                     {swatch} +1, darker is less energy",
                    component.0
                );
                let swatches = [-1.0, -0.5, 0.0, 0.5, 1.0]
                    .map(crate::component_rgb8)
                    .to_vec();
                Some(Legend { text, swatches })
            }
        }
    }
}

impl fmt::Display for FieldMapping {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl FromStr for FieldMapping {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|mapping| mapping.name() == text)
            .ok_or_else(|| {
                format!(
                    "`{text}` is not a field mapping: energy, composition or component-C for a \
                     channel C from 0 to {}",
                    FIELD_COMPONENTS - 1
                )
            })
    }
}

/// A whole frame's drawing choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Style {
    /// What a particle's colour means.
    pub mapping: ColorMapping,
    /// Glow or raw.
    pub look: Look,
    /// Points or density.
    pub particles: Particles,
    /// Whether the field is drawn under the organisms.
    pub field: bool,
    /// What the field's colour means.
    pub field_mapping: FieldMapping,
}

impl Style {
    /// Whether a frame in this style draws the field: when it is on, except
    /// under raw or sphere density, which is opaque.
    #[must_use]
    pub fn field_drawn(self) -> bool {
        self.field
            && !(self.particles == Particles::Density
                && matches!(self.look, Look::Raw | Look::Sphere))
    }
}

impl Default for Style {
    /// Lineage colours, glowing points over the field.
    fn default() -> Self {
        Self {
            mapping: ColorMapping::Lineage,
            look: Look::Glow,
            particles: Particles::Points,
            field: true,
            field_mapping: FieldMapping::Energy,
        }
    }
}

/// The epoch columns one organism's colour may read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Traits {
    /// The founder lineage.
    pub lineage: u64,
    /// Energy.
    pub energy: f32,
    /// Generations since the founders.
    pub generation: u32,
    /// The controller topology's catalog index.
    pub topology: u8,
    /// Velocity in world units per simulated second, y down.
    pub velocity: [f32; 2],
    /// The affinity direction's display coordinates.
    pub affinity: [f32; 3],
    /// The least energy an organism of the world attempts a birth with
    /// ([`Epoch::attempt_energy`]), the same for every row.
    pub attempt_energy: f64,
}

/// Every row of `epoch` as [`Traits`], in identity order.
pub fn traits(epoch: &Epoch) -> impl Iterator<Item = Traits> + '_ {
    let attempt_energy = epoch.attempt_energy();
    epoch
        .lineages()
        .iter()
        .zip(epoch.energies())
        .zip(epoch.generations())
        .zip(epoch.topologies())
        .zip(epoch.velocities())
        .zip(epoch.affinities())
        .map(
            move |(((((&lineage, &energy), &generation), &topology), &velocity), &affinity)| {
                Traits {
                    lineage,
                    energy,
                    generation,
                    topology,
                    velocity,
                    affinity,
                    attempt_energy,
                }
            },
        )
}

/// The lineage palette (sRGB): the luminous hues of plan §2.3 (teal,
/// violet, magenta, pale cyan, amber, lime) and six more like them, for a
/// dark canvas. Indexed by [`lineage_slot`].
pub const LINEAGE_PALETTE: [[u8; 3]; 12] = [
    [0x2E, 0xC4, 0xB6], // teal
    [0x9B, 0x5D, 0xE5], // violet
    [0xF1, 0x5B, 0xB5], // magenta
    [0x9B, 0xF6, 0xFF], // pale cyan
    [0xFF, 0xB6, 0x27], // amber
    [0xB5, 0xE6, 0x1D], // lime
    [0xFF, 0x6F, 0x59], // coral
    [0x4E, 0xA8, 0xDE], // sky
    [0xFE, 0xE4, 0x40], // gold
    [0xFF, 0x99, 0xC8], // rose
    [0x7A, 0xE5, 0x82], // mint
    [0xC8, 0xB6, 0xFF], // lavender
];

/// The palette slot of a lineage: a fixed SplitMix64 mix of its identity,
/// so neighbouring founder identities get unrelated colours and a lineage
/// keeps its colour for the whole run.
#[must_use]
pub fn lineage_slot(lineage: u64) -> usize {
    let mut z = lineage.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    let slots = LINEAGE_PALETTE.len() as u64;
    usize::try_from(z % slots).unwrap_or(0)
}

/// The linear-light colour of a lineage.
#[must_use]
pub fn lineage_color(lineage: u64) -> LinearRgba {
    linear(LINEAGE_PALETTE[lineage_slot(lineage)])
}

/// The energy ramp (sRGB stops, evenly spaced): magma from 0.2 to 1.0, so
/// lightness rises monotonically and even an organism with no energy shows
/// on the dark canvas.
pub const ENERGY_RAMP: [[u8; 3]; 9] = [
    [0x3B, 0x0F, 0x70],
    [0x64, 0x1A, 0x80],
    [0x8C, 0x29, 0x81],
    [0xB7, 0x37, 0x79],
    [0xDE, 0x49, 0x68],
    [0xF7, 0x70, 0x5C],
    [0xFE, 0x9F, 0x6D],
    [0xFE, 0xCF, 0x92],
    [0xFC, 0xFD, 0xBF],
];

/// The energy shown half way along [`ENERGY_RAMP`].
pub const ENERGY_HALF: f64 = 2.0;

/// An energy's position on [`ENERGY_RAMP`]: `e / (e + ENERGY_HALF)`, in
/// `[0, 1)`; `0` for a negative or non-finite energy.
#[must_use]
pub fn energy_level(energy: f32) -> f64 {
    let energy = f64::from(energy);
    if energy.is_finite() && energy > 0.0 {
        energy / (energy + ENERGY_HALF)
    } else {
        0.0
    }
}

/// The generation ramp (sRGB stops, evenly spaced): plasma from 0.33 to
/// 1.0, lightness rising monotonically. Warm and bright, so even the
/// founders stand out on the blue and teal field.
pub const GENERATION_RAMP: [[u8; 3]; 7] = [
    [0x9C, 0x17, 0x9E],
    [0xBD, 0x37, 0x86],
    [0xD8, 0x57, 0x6B],
    [0xED, 0x79, 0x53],
    [0xFB, 0x9F, 0x3A],
    [0xFD, 0xCA, 0x26],
    [0xF0, 0xF9, 0x21],
];

/// The generation at the bright end of [`GENERATION_RAMP`].
pub const GENERATION_SCALE: u32 = 256;

/// A generation's position on [`GENERATION_RAMP`]:
/// `ln(1 + g) / ln(1 + GENERATION_SCALE)`, clamped to `[0, 1]`, so the
/// first generations separate and the scale never refits.
#[must_use]
pub fn generation_level(generation: u32) -> f64 {
    (f64::from(generation).ln_1p() / f64::from(GENERATION_SCALE).ln_1p()).clamp(0.0, 1.0)
}

/// The heading wheel (sRGB stops at 0°, 60°, … 300°, repeating): east is
/// coral, and the hues turn counter-clockwise on screen through amber,
/// lime, teal, sky and violet.
pub const MOTION_WHEEL: [[u8; 3]; 6] = [
    [0xFF, 0x6F, 0x59],
    [0xFF, 0xB6, 0x27],
    [0xB5, 0xE6, 0x1D],
    [0x2E, 0xC4, 0xB6],
    [0x4E, 0xA8, 0xDE],
    [0x9B, 0x5D, 0xE5],
];

/// A motionless organism's colour (sRGB): a dim grey.
pub const MOTION_STILL: [u8; 3] = [0x46, 0x4B, 0x5A];

/// A velocity's heading as a fraction of a turn in `[0, 1)`, counted
/// counter-clockwise on screen from east (world y points down), or `None`
/// for a zero or non-finite velocity.
#[must_use]
pub fn heading(velocity: [f32; 2]) -> Option<f64> {
    let [x, y] = velocity.map(f64::from);
    if !(x.is_finite() && y.is_finite()) || (x == 0.0 && y == 0.0) {
        return None;
    }
    let turn = (-y).atan2(x) / std::f64::consts::TAU;
    let turn = if turn < 0.0 { turn + 1.0 } else { turn };
    Some(if turn >= 1.0 { 0.0 } else { turn })
}

/// A velocity's speed as a fraction of the world's speed limit, in
/// `[0, 1]`; `0` when either is zero or not finite.
#[must_use]
pub fn speed_level(velocity: [f32; 2], maximum_speed: f64) -> f64 {
    let [x, y] = velocity.map(f64::from);
    let speed = x.hypot(y);
    if !(speed.is_finite() && maximum_speed.is_finite()) || maximum_speed <= 0.0 {
        return 0.0;
    }
    (speed / maximum_speed).clamp(0.0, 1.0)
}

/// The topology palette (sRGB), indexed by catalog index mod 8. The first
/// five are the catalog's members in size order, small to large.
pub const TOPOLOGY_PALETTE: [[u8; 3]; 8] = [
    [0x9B, 0xF6, 0xFF], // pale cyan: 104 → 8 → 36
    [0x2E, 0xC4, 0xB6], // teal: 104 → 32 → 36
    [0xB5, 0xE6, 0x1D], // lime: 104 → 64 → 32 → 36
    [0xFF, 0xB6, 0x27], // amber: 104 → 96 → 64 → 36
    [0xF1, 0x5B, 0xB5], // magenta: 104 → 128 → 96 → 36
    [0x9B, 0x5D, 0xE5],
    [0xFF, 0x6F, 0x59],
    [0xFE, 0xE4, 0x40],
];

/// The gain on the affinity coordinates: `√(128/3)`. A founder's direction
/// is isotropic at the norm limit, so each of its three coordinates has a
/// mean square of 1/32, and this puts it at a radius of about 2: most
/// colours near full strength, the squash still keeping them below it.
/// About 9% of isotropic directions then leave the sRGB gamut and are
/// clipped per channel; at half this gain under 1% did, with half the
/// typical chroma.
pub const AFFINITY_GAIN: f64 = 6.531_972_647_421_808;

/// The OKLab lightness of an organism with no affinity, and how far the
/// third coordinate moves it either way. With [`AFFINITY_CHROMA`], chosen
/// so that a world drawn as lit spheres ([`crate::SPHERE_SHADING`], which
/// lifts lightness by about 0.05) spans the reference screenshots' lit
/// pixels: median OKLab lightness 0.40 to 0.57, chroma to 0.20 at the 90th
/// percentile. Dark olives and maroons are among them, as there.
pub const AFFINITY_LIGHTNESS: [f64; 2] = [0.46, 0.12];

/// The OKLab chroma the first two coordinates approach at full strength.
pub const AFFINITY_CHROMA: f64 = 0.26;

/// The colour of affinity coordinates `q`. They are scaled by
/// [`AFFINITY_GAIN`] to `g` and squashed radially to `s = g / √(1 + |g|²)`,
/// so `|s| < 1` and nothing refits. Then OKLab
/// `L = 0.46 + 0.12 s₃`, `a = 0.26 s₁`, `b = 0.26 s₂` ([`AFFINITY_LIGHTNESS`],
/// [`AFFINITY_CHROMA`]) is converted to sRGB;
/// a colour outside the gamut keeps its lightness and hue and takes the most
/// chroma that fits. No affinity is a neutral grey; the
/// hue turns with the direction in the first two coordinates, and opposite
/// directions take opposite hues. A non-finite coordinate counts as 0.
#[must_use]
pub fn affinity_rgb8(q: [f32; 3]) -> [u8; 3] {
    let g = q.map(|c| {
        let c = f64::from(c);
        if c.is_finite() {
            c * AFFINITY_GAIN
        } else {
            0.0
        }
    });
    let squash = (1.0 + g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
    let s = g.map(|c| c / squash);
    let [lightness, swing] = AFFINITY_LIGHTNESS;
    let l = lightness + swing * s[2];
    let at = |k: f64| {
        oklab_to_srgb(Oklab {
            l,
            a: k * AFFINITY_CHROMA * s[0],
            b: k * AFFINITY_CHROMA * s[1],
        })
    };
    let inside = |c: Srgb| [c.r, c.g, c.b].iter().all(|v| (0.0..=1.0).contains(v));
    let full = at(1.0);
    if inside(full) {
        return full.to_rgb8();
    }
    // Outside the gamut: the most chroma that fits at this lightness and
    // hue, by bisection (the grey at k = 0 always fits).
    let (mut low, mut high) = (0.0_f64, 1.0_f64);
    for _ in 0..24 {
        let middle = 0.5 * (low + high);
        if inside(at(middle)) {
            low = middle;
        } else {
            high = middle;
        }
    }
    at(low).to_rgb8()
}

/// The lineage-energy mapping's OKLab lightness for an organism holding
/// exactly the energy it needs to attempt a birth. Lightness is
/// proportional to energy, so an organism with none is black and one with
/// `1 / 0.55 ≈ 1.8` times that energy is white. Chosen against the
/// reference screenshots (`experiments/showcase/scale.py`): on the film's
/// world it scored best of the lightness rules tried, and it keeps the
/// world's common energies (about 0.5 to 1.5 times the attempt energy)
/// between dark and pale.
pub const LINEAGE_ENERGY_LIGHTNESS: f64 = 0.55;

/// The lightness steps of [`lineage_energy_rgb8`]: `0` to `255`, so each
/// palette colour's 256 shades are computed once.
const LINEAGE_ENERGY_STEPS: usize = 256;

/// Each [`LINEAGE_PALETTE`] colour at each of its lightness steps.
static LINEAGE_ENERGY_SHADES: LazyLock<[[[u8; 3]; LINEAGE_ENERGY_STEPS]; LINEAGE_PALETTE.len()]> =
    LazyLock::new(|| {
        let mut shades = [[[0_u8; 3]; LINEAGE_ENERGY_STEPS]; LINEAGE_PALETTE.len()];
        for (palette, row) in LINEAGE_PALETTE.iter().zip(shades.iter_mut()) {
            let [r, g, b] = *palette;
            let base = srgb_to_oklab(Srgb::from_rgb8(r, g, b));
            for (step, shade) in row.iter_mut().enumerate() {
                let l = step as f64 / (LINEAGE_ENERGY_STEPS - 1) as f64;
                *shade = most_chroma(l, base.a, base.b);
            }
        }
        shades
    });

/// The sRGB colour at OKLab lightness `l` with hue `(a, b)`: the full
/// chroma if it fits the gamut, otherwise the most of it that fits, by
/// bisection (no chroma always fits).
fn most_chroma(l: f64, a: f64, b: f64) -> [u8; 3] {
    let at = |k: f64| {
        oklab_to_srgb(Oklab {
            l,
            a: k * a,
            b: k * b,
        })
    };
    let inside = |c: Srgb| [c.r, c.g, c.b].iter().all(|v| (0.0..=1.0).contains(v));
    let full = at(1.0);
    if inside(full) {
        return full.to_rgb8();
    }
    let (mut low, mut high) = (0.0_f64, 1.0_f64);
    for _ in 0..24 {
        let middle = 0.5 * (low + high);
        if inside(at(middle)) {
            low = middle;
        } else {
            high = middle;
        }
    }
    at(low).to_rgb8()
}

/// The lineage-energy colour of an organism of `lineage` holding `energy`
/// in a world whose organisms attempt a birth from `attempt_energy`: the
/// OKLab hue and chroma of the lineage's [`LINEAGE_PALETTE`] colour at
/// lightness [`LINEAGE_ENERGY_LIGHTNESS`]` × energy / attempt_energy`,
/// clamped to `[0, 1]` and rounded to one of 256 steps. A shade outside
/// the gamut keeps its lightness and hue and takes the most chroma that
/// fits, so the best fed go towards white. A negative or non-finite energy
/// counts as 0; for an attempt energy that is not finite and positive the
/// lineage's palette colour is unchanged.
#[must_use]
pub fn lineage_energy_rgb8(lineage: u64, energy: f32, attempt_energy: f64) -> [u8; 3] {
    palette_shade(lineage_slot(lineage), energy, attempt_energy)
}

/// [`LINEAGE_PALETTE`]`[slot]` shaded as [`lineage_energy_rgb8`] shades a
/// lineage's colour.
fn palette_shade(slot: usize, energy: f32, attempt_energy: f64) -> [u8; 3] {
    if !(attempt_energy.is_finite() && attempt_energy > 0.0) {
        return LINEAGE_PALETTE[slot];
    }
    let energy = f64::from(energy);
    let energy = if energy.is_finite() {
        energy.max(0.0)
    } else {
        0.0
    };
    let lightness = (LINEAGE_ENERGY_LIGHTNESS * energy / attempt_energy).clamp(0.0, 1.0);
    let step = (lightness * (LINEAGE_ENERGY_STEPS - 1) as f64).round() as usize;
    LINEAGE_ENERGY_SHADES[slot][step.min(LINEAGE_ENERGY_STEPS - 1)]
}

/// `t ∈ [0, 1]` along evenly spaced sRGB `stops`, interpolated in sRGB
/// bytes; a non-finite `t` is `0`.
#[must_use]
pub fn ramp(stops: &[[u8; 3]], t: f64) -> [u8; 3] {
    let (Some(&first), Some(&last)) = (stops.first(), stops.last()) else {
        return [0; 3];
    };
    if stops.len() == 1 {
        return first;
    }
    let t = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if t >= 1.0 {
        return last;
    }
    let position = t * (stops.len() - 1) as f64;
    let low = (position.floor() as usize).min(stops.len() - 2);
    mix(stops[low], stops[low + 1], position - low as f64)
}

/// `turn ∈ [0, 1)` around the cyclic [`MOTION_WHEEL`].
#[must_use]
pub fn wheel(turn: f64) -> [u8; 3] {
    let turn = if turn.is_finite() {
        turn.rem_euclid(1.0)
    } else {
        0.0
    };
    let count = MOTION_WHEEL.len();
    let position = turn * count as f64;
    let low = (position.floor() as usize).min(count - 1);
    mix(
        MOTION_WHEEL[low],
        MOTION_WHEEL[(low + 1) % count],
        position - low as f64,
    )
}

/// `a` to `b` by `fraction ∈ [0, 1]`, per sRGB byte, rounded.
#[must_use]
pub fn mix(a: [u8; 3], b: [u8; 3], fraction: f64) -> [u8; 3] {
    let fraction = fraction.clamp(0.0, 1.0);
    let channel =
        |x: u8, y: u8| (f64::from(x) + (f64::from(y) - f64::from(x)) * fraction).round() as u8;
    [
        channel(a[0], b[0]),
        channel(a[1], b[1]),
        channel(a[2], b[2]),
    ]
}

/// The sRGB colour `mapping` gives an organism with `traits` in a world
/// whose speed limit is `maximum_speed`.
#[must_use]
pub fn mapped_rgb8(mapping: ColorMapping, traits: &Traits, maximum_speed: f64) -> [u8; 3] {
    match mapping {
        ColorMapping::Lineage => LINEAGE_PALETTE[lineage_slot(traits.lineage)],
        ColorMapping::Energy => ramp(&ENERGY_RAMP, energy_level(traits.energy)),
        ColorMapping::Generation => ramp(&GENERATION_RAMP, generation_level(traits.generation)),
        // The square root lifts the common slow movers (the preset's mean
        // speed is about a quarter of its limit) without refitting.
        ColorMapping::Motion => heading(traits.velocity).map_or(MOTION_STILL, |turn| {
            mix(
                MOTION_STILL,
                wheel(turn),
                speed_level(traits.velocity, maximum_speed).sqrt(),
            )
        }),
        ColorMapping::Topology => {
            TOPOLOGY_PALETTE[usize::from(traits.topology) % TOPOLOGY_PALETTE.len()]
        }
        ColorMapping::Affinity => affinity_rgb8(traits.affinity),
        ColorMapping::LineageEnergy => {
            lineage_energy_rgb8(traits.lineage, traits.energy, traits.attempt_energy)
        }
    }
}

/// [`mapped_rgb8`] as opaque linear light.
#[must_use]
pub fn mapped_color(mapping: ColorMapping, traits: &Traits, maximum_speed: f64) -> LinearRgba {
    linear(mapped_rgb8(mapping, traits, maximum_speed))
}

/// Opaque linear light from sRGB bytes: exactly
/// `Srgb::from_rgb8(r, g, b).to_linear(1.0)`.
///
/// The transfer acts on each channel alone, so each byte's linear value is
/// decoded once for the process and looked up, instead of a power per
/// channel per call (a frame decodes a colour for every
/// organism, and a field update one for every texel).
#[must_use]
pub fn linear([r, g, b]: [u8; 3]) -> LinearRgba {
    let table = &*LINEAR_BYTES;
    LinearRgba {
        r: table[usize::from(r)],
        g: table[usize::from(g)],
        b: table[usize::from(b)],
        a: 1.0,
    }
}

/// Each sRGB byte's linear value, as [`Srgb::to_linear`] decodes it.
static LINEAR_BYTES: LazyLock<[f64; 256]> = LazyLock::new(|| {
    let mut table = [0.0; 256];
    for byte in 0..=u8::MAX {
        table[usize::from(byte)] = Srgb::from_rgb8(byte, 0, 0).to_linear(1.0).r;
    }
    table
});
