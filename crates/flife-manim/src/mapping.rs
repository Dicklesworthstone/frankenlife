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

use std::fmt;
use std::str::FromStr;

use flife_core::registry::{self, Role};
use flife_core::{CoreError, ValidatedConfig};
use flife_render::Epoch;
use fmn_core::color::{LinearRgba, Oklab, Srgb, oklab_to_srgb};

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
}

impl ColorMapping {
    /// Every mapping, in the Studio's key order (1 to 6).
    pub const ALL: [Self; 6] = [
        Self::Lineage,
        Self::Energy,
        Self::Generation,
        Self::Motion,
        Self::Topology,
        Self::Affinity,
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
        }
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
                    "`{text}` is not a colour mapping: lineage, energy, generation, motion, topology or affinity"
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
}

impl Look {
    /// The look's name, as flags and records spell it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Glow => "glow",
            Self::Raw => "raw",
        }
    }
}

impl FromStr for Look {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "glow" => Ok(Self::Glow),
            "raw" => Ok(Self::Raw),
            _ => Err(format!("`{text}` is not a look: glow or raw")),
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

/// A whole frame's drawing choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Style {
    /// What a particle's colour means.
    pub mapping: ColorMapping,
    /// Glow or raw.
    pub look: Look,
    /// Points or density.
    pub particles: Particles,
    /// Whether the energy field is drawn under the organisms.
    pub field: bool,
}

impl Default for Style {
    /// Lineage colours, glowing points over the field.
    fn default() -> Self {
        Self {
            mapping: ColorMapping::Lineage,
            look: Look::Glow,
            particles: Particles::Points,
            field: true,
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
}

/// Every row of `epoch` as [`Traits`], in identity order.
pub fn traits(epoch: &Epoch) -> impl Iterator<Item = Traits> + '_ {
    epoch
        .lineages()
        .iter()
        .zip(epoch.energies())
        .zip(epoch.generations())
        .zip(epoch.topologies())
        .zip(epoch.velocities())
        .zip(epoch.affinities())
        .map(
            |(((((&lineage, &energy), &generation), &topology), &velocity), &affinity)| Traits {
                lineage,
                energy,
                generation,
                topology,
                velocity,
                affinity,
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
/// third coordinate moves it either way.
pub const AFFINITY_LIGHTNESS: [f64; 2] = [0.74, 0.10];

/// The OKLab chroma the first two coordinates approach at full strength.
pub const AFFINITY_CHROMA: f64 = 0.16;

/// The colour of affinity coordinates `q`. They are scaled by
/// [`AFFINITY_GAIN`] to `g` and squashed radially to `s = g / √(1 + |g|²)`,
/// so `|s| < 1` and nothing refits. Then OKLab
/// `L = 0.74 + 0.10 s₃`, `a = 0.16 s₁`, `b = 0.16 s₂` is converted to sRGB,
/// each channel clamped to the gamut. No affinity is a neutral grey; the
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
    oklab_to_srgb(Oklab {
        l: lightness + swing * s[2],
        a: AFFINITY_CHROMA * s[0],
        b: AFFINITY_CHROMA * s[1],
    })
    .to_rgb8()
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
    }
}

/// [`mapped_rgb8`] as opaque linear light.
#[must_use]
pub fn mapped_color(mapping: ColorMapping, traits: &Traits, maximum_speed: f64) -> LinearRgba {
    linear(mapped_rgb8(mapping, traits, maximum_speed))
}

/// Opaque linear light from sRGB bytes.
#[must_use]
pub fn linear([r, g, b]: [u8; 3]) -> LinearRgba {
    Srgb::from_rgb8(r, g, b).to_linear(1.0)
}
