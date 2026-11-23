//! Pure read-only (`INV-09`) `field-ecology-v2` visual mapping primitives
//! (`V2-16`, modular `v2-visual-mappings`; also realizes `FL-052`).
//!
//! Every function and layer here is strictly a presentation-only projection of
//! immutable read-only slices:
//!
//! - **`cell-state-v1`** ([`cell_state_rgb8`], [`cell_state_color`]): preserves
//!   the founder lineage's base hue from [`LINEAGE_PALETTE`] and modulates
//!   OKLab lightness, chroma, and subtle secondary hue tint cleanly by the
//!   quantized differentiation state `h_0 ∈ [-1, 1]` (`i8` in `-127..=127`),
//!   making clonal core-vs-sheath, queen-vs-worker, and tip-vs-stalk
//!   differentiation immediately legible.
//! - **`polarity-v1`** ([`polarity_rgb8`], [`polarity_color`]): maps quantized
//!   apical-basal polarity angles (`i16` in `-32767..=32767` for `atan2(p_y, p_x)`
//!   in world coordinates, with [`POLARITY_UNPOLARIZED`] = `i16::MIN` as the
//!   explicit unpolarized sentinel) onto a perceptually uniform cyclic OKLab
//!   hue wheel, paired with [`apical_tick_quad`] / [`apical_ticks_mesh`] for
//!   bulk apical orientation ticks in sphere mode.
//! - **`trail-carrier` & `vascular-conductance` overlays** ([`FieldOverlayKind`],
//!   [`FieldOverlayLayer`], [`build_overlay_texture`],
//!   [`build_composite_overlay_texture`]): perceptual log/linear ramps and
//!   torus-seamless texture builders for persistent Carrier-1 pheromone
//!   highways (`e_1`) and Murray/Physarum vascular arteries (`g_e`), keeping
//!   honest linear grey available in [`Look::Raw`] / [`Look::Sphere`] (Gate G5).
//!
//! Nothing here reads or mutates simulation state, paints synthetic membranes,
//! or depends on host `libm` sRGB transfer functions (`fmn_core::color` uses
//! `fmn_dmath::pow`).

use std::f64::consts::{PI, TAU};
use std::fmt;
use std::str::FromStr;
use std::sync::LazyLock;

use flife_core::CoreError;
pub use flife_render::{
    CELL_STATE_STEPS, POLARITY_ANGLE_SCALE, POLARITY_UNPOLARIZED, quantize_cell_state,
    quantize_polarity,
};
use fmn_core::color::{LinearRgba, Srgb, srgb_to_oklab};
use fmn_render::{SurfaceDraw, SurfaceMesh, SurfaceVertex, Texture, ThreeDDraw};

use crate::field::{SCALE_DECAY, display_value, palette as energy_palette, raw_value};
use crate::mapping::{
    LINEAGE_PALETTE, Legend, Look, lineage_slot, linear, linear_table, linear_with_table,
    most_chroma, ramp,
};
use crate::{HUD_SWATCH, Viewport};

/// Short CLI/HUD name for the clonal cell-state visual mapping (`V2-16`).
pub const CELL_STATE_MAPPING_NAME: &str = "cell-state";

/// Registered `[presentation] color_mapping` identifier for clonal cell-state (`V2-16`).
pub const CELL_STATE_REGISTRY_NAME: &str = "cell-state-v1";

/// Short CLI/HUD name for the apical-basal polarity visual mapping (`V2-16`).
pub const POLARITY_MAPPING_NAME: &str = "polarity";

/// Registered `[presentation] color_mapping` identifier for apical polarity (`V2-16`).
pub const POLARITY_REGISTRY_NAME: &str = "polarity-v1";

/// Field overlay name for Carrier-1 pheromone trail energy (`e_1`, `V2-15`/`V2-16`).
pub const TRAIL_CARRIER_NAME: &str = "trail-carrier";

/// Field overlay name for Murray/Physarum mean incident conductance (`g_e`, `V2-14`/`V2-16`).
pub const VASCULAR_CONDUCTANCE_NAME: &str = "vascular-conductance";

/// OKLab lightness endpoints `[L_low, L_high]` for `cell-state-v1` at `h_0 = -1`
/// and `h_0 = +1`, delivering >50 CIE L* separation within every founder lineage
/// while staying visible on a dark canvas.
pub const CELL_STATE_LIGHTNESS_RANGE: [f64; 2] = [0.36, 0.93];

/// Secondary OKLab hue twist (radians) applied symmetrically at `h_0 = ±1`
/// (`±17.2°`) so adjacent differentiation states separate cleanly while
/// preserving founder lineage hue identity.
pub const CELL_STATE_HUE_TWIST_RADIANS: f64 = 0.30;

/// Sample quantized `h_0` states shown in [`cell_state_legend`].
pub const LEGEND_CELL_STATES: [i8; 5] = [-127, -64, 0, 64, 127];

/// Neutral sRGB colour for unpolarized organisms ([`POLARITY_UNPOLARIZED`]).
pub const POLARITY_UNPOLARIZED_COLOR: [u8; 3] = [0x58, 0x5E, 0x6E];

/// Constant OKLab lightness of the cyclic [`polarity_rgb8`] hue wheel.
pub const POLARITY_WHEEL_LIGHTNESS: f64 = 0.74;

/// Target OKLab chroma of the cyclic [`polarity_rgb8`] hue wheel (gamut-clamped
/// at constant lightness and hue).
pub const POLARITY_WHEEL_CHROMA: f64 = 0.145;

/// Apical tick shaft start as a multiple of dot radius (anchored inside the
/// sphere cap for legibility).
pub const APICAL_TICK_START_RATIO: f64 = 0.35;

/// Apical tick shaft tip as a multiple of dot radius (extending cleanly past
/// the cell rim along the apical vector).
pub const APICAL_TICK_END_RATIO: f64 = 1.45;

/// Half-width of an apical orientation tick as a multiple of dot radius.
pub const APICAL_TICK_HALF_WIDTH_RATIO: f64 = 0.18;

/// High-contrast linear-light colour for apical orientation ticks over spheres.
pub const APICAL_TICK_COLOR: LinearRgba = LinearRgba {
    r: 0.96,
    g: 0.96,
    b: 0.92,
    a: 0.95,
};

/// Logarithmic perceptual gain `k` in `ln(1 + k · x) / ln(1 + k)` used by glow
/// field overlays so faint pheromone trails and fine capillary branches remain
/// legible without saturating main trunks.
pub const OVERLAY_LOG_GAIN: f64 = 15.0;

/// Perceptual sRGB palette for Carrier-1 pheromone trail highways (`e_1`):
/// deep void → plum → burnt amber → vivid gold → luminous pale gold, rising
/// monotonically in lightness and visually distinct from [`crate::FIELD_PALETTE`].
pub const TRAIL_CARRIER_PALETTE: [[u8; 3]; 6] = [
    [0x03, 0x02, 0x06],
    [0x24, 0x0E, 0x2E],
    [0x6E, 0x1F, 0x3D],
    [0xB8, 0x4A, 0x24],
    [0xEE, 0x9B, 0x28],
    [0xFF, 0xF0, 0xB8],
];

/// Perceptual sRGB palette for Murray/Physarum vascular conductance (`g_e`):
/// deep void → arterial burgundy → crimson → coral-rose → glowing blush-white,
/// rising monotonically in lightness and distinct from both energy and trails.
pub const VASCULAR_CONDUCTANCE_PALETTE: [[u8; 3]; 6] = [
    [0x04, 0x02, 0x04],
    [0x2E, 0x07, 0x16],
    [0x6B, 0x0F, 0x2B],
    [0xB8, 0x25, 0x43],
    [0xF2, 0x6A, 0x6B],
    [0xFF, 0xD8, 0xD4],
];

/// Number of quantized `cell_state` steps in `-127..=127`.
pub(crate) const CELL_STATE_LUT_LEN: usize = 2 * (CELL_STATE_STEPS as usize) + 1;

/// Power-of-two cyclic lookup table length for `polarity-v1` (`0.35°` per step).
pub(crate) const POLARITY_LUT_LEN: usize = 1024;

/// The `V2-16` particle colour mappings (`cell-state-v1` and `polarity-v1`),
/// pure read-only (`INV-09`) extensions of [`crate::ColorMapping`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum V2ColorMapping {
    /// Clonal cell-state differentiation (`h_0` within founder lineage).
    CellState,
    /// Apical-basal polarity angle on a perceptually uniform cyclic OKLab wheel.
    Polarity,
}

impl V2ColorMapping {
    /// Both `V2-16` particle mappings in Studio key order (`8`, `9`).
    pub const ALL: [Self; 2] = [Self::CellState, Self::Polarity];

    /// Short CLI/HUD name (`cell-state` or `polarity`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::CellState => CELL_STATE_MAPPING_NAME,
            Self::Polarity => POLARITY_MAPPING_NAME,
        }
    }

    /// Versioned `[presentation] color_mapping` name (`cell-state-v1` or `polarity-v1`).
    #[must_use]
    pub const fn registry_name(self) -> &'static str {
        match self {
            Self::CellState => CELL_STATE_REGISTRY_NAME,
            Self::Polarity => POLARITY_REGISTRY_NAME,
        }
    }

    /// Resolve a versioned registry name.
    #[must_use]
    pub fn from_registry(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mapping| mapping.registry_name() == name)
    }

    /// Human-readable description for records and legends.
    #[must_use]
    pub const fn meaning(self) -> &'static str {
        match self {
            Self::CellState => {
                "cell state: founder lineage hue darkened for h0 < 0 and lightened for h0 > 0"
            }
            Self::Polarity => {
                "apical polarity: cyclic perceptual OKLab hue wheel by apical angle, grey when unpolarized"
            }
        }
    }

    /// Compact HUD legend strictly within FL-024's vertex cap.
    #[must_use]
    pub fn legend(self) -> Legend {
        match self {
            Self::CellState => cell_state_legend(),
            Self::Polarity => polarity_legend(),
        }
    }
}

impl fmt::Display for V2ColorMapping {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl FromStr for V2ColorMapping {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|mapping| mapping.name() == text || mapping.registry_name() == text)
            .ok_or_else(|| format!("`{text}` is not a v2 colour mapping: cell-state or polarity"))
    }
}

/// Decode a quantized `Epoch::cell_states` value back to `[-1.0, 1.0]`.
/// `i8::MIN` (`-128`) clamps cleanly to `-1.0`.
#[must_use]
pub fn decode_cell_state(quantized: i8) -> f64 {
    (f64::from(quantized) / f64::from(CELL_STATE_STEPS)).clamp(-1.0, 1.0)
}

#[inline]
pub(crate) fn cell_state_index(quantized: i8) -> usize {
    let clamped =
        i16::from(quantized).clamp(-i16::from(CELL_STATE_STEPS), i16::from(CELL_STATE_STEPS));
    usize::try_from(clamped + i16::from(CELL_STATE_STEPS)).unwrap_or(0)
}

#[inline]
pub(crate) fn linear_cell_state_shades()
-> &'static [[LinearRgba; CELL_STATE_LUT_LEN]; LINEAGE_PALETTE.len()] {
    &LINEAR_CELL_STATE_SHADES
}

/// Precomputed `12 × 255` sRGB shade table for `cell-state-v1`.
static CELL_STATE_SHADES: LazyLock<[[[u8; 3]; CELL_STATE_LUT_LEN]; LINEAGE_PALETTE.len()]> =
    LazyLock::new(|| {
        let mut table = [[[0_u8; 3]; CELL_STATE_LUT_LEN]; LINEAGE_PALETTE.len()];
        let [l_low, l_high] = CELL_STATE_LIGHTNESS_RANGE;
        for (palette_rgb, row) in LINEAGE_PALETTE.iter().zip(table.iter_mut()) {
            let [r, g, b] = *palette_rgb;
            let base = srgb_to_oklab(Srgb::from_rgb8(r, g, b));
            for (index, shade) in row.iter_mut().enumerate() {
                let s = (index as f64 / f64::from(CELL_STATE_STEPS)) - 1.0;
                let t = 0.5 * (s + 1.0);
                let l = l_low + t * (l_high - l_low);
                let (sin, cos) = (s * CELL_STATE_HUE_TWIST_RADIANS).sin_cos();
                let a_rot = base.a * cos - base.b * sin;
                let b_rot = base.a * sin + base.b * cos;
                let chroma_scale = if s < 0.0 {
                    1.0 - 0.15 * s
                } else {
                    1.0 - 0.45 * s
                };
                *shade = most_chroma(l, a_rot * chroma_scale, b_rot * chroma_scale);
            }
        }
        table
    });

/// Linear-light counterpart of [`CELL_STATE_SHADES`], computed once for O(1)
/// per-organism hot loops.
static LINEAR_CELL_STATE_SHADES: LazyLock<
    [[LinearRgba; CELL_STATE_LUT_LEN]; LINEAGE_PALETTE.len()],
> = LazyLock::new(|| {
    let linear_bytes = linear_table();
    std::array::from_fn(|slot| {
        std::array::from_fn(|step| linear_with_table(linear_bytes, CELL_STATE_SHADES[slot][step]))
    })
});

/// The sRGB colour of `cell-state-v1` for an organism of `lineage` with
/// quantized differentiation state `cell_state` (`-127..=127`).
#[must_use]
pub fn cell_state_rgb8(lineage: u64, cell_state: i8) -> [u8; 3] {
    CELL_STATE_SHADES[lineage_slot(lineage)][cell_state_index(cell_state)]
}

/// [`cell_state_rgb8`] as opaque linear light.
#[must_use]
pub fn cell_state_color(lineage: u64, cell_state: i8) -> LinearRgba {
    LINEAR_CELL_STATE_SHADES[lineage_slot(lineage)][cell_state_index(cell_state)]
}

/// Compact HUD legend for `cell-state-v1` (5 swatches on lineage slot 0).
#[must_use]
pub fn cell_state_legend() -> Legend {
    let swatch = HUD_SWATCH;
    let labels = ["-1", "-0.5", "0", "+0.5", "+1"];
    let mut text = String::from(CELL_STATE_MAPPING_NAME);
    let mut swatches = Vec::with_capacity(LEGEND_CELL_STATES.len());
    for (&state, label) in LEGEND_CELL_STATES.iter().zip(labels) {
        text.push(' ');
        text.push(swatch);
        text.push(' ');
        text.push_str(label);
        swatches.push(CELL_STATE_SHADES[0][cell_state_index(state)]);
    }
    text.push_str(" (h0 in lineage)");
    Legend { text, swatches }
}

/// Lower a slice of `lineages` and optional quantized `cell_states` into linear
/// colours in `out`. When `cell_states` is `None` (e.g., `V2-12` or
/// `v2-visual-mappings` disabled), every row uses `cell_state = 0`.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] if `cell_states` has a different length than
/// `lineages`; [`CoreError::AllocationFailed`] if `out` cannot be reserved.
pub fn lower_cell_state_colors(
    lineages: &[u64],
    cell_states: Option<&[i8]>,
    out: &mut Vec<LinearRgba>,
) -> Result<(), CoreError> {
    if let Some(states) = cell_states
        && states.len() != lineages.len()
    {
        return Err(CoreError::LayoutMismatch {
            buffer: "cell-state visual mapping",
            detail: format!(
                "{} cell states for {} lineages",
                states.len(),
                lineages.len()
            ),
        });
    }
    out.clear();
    if out.capacity() < lineages.len() {
        out.try_reserve_exact(lineages.len())
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "cell-state colours",
                bytes: u64::try_from(lineages.len().saturating_mul(size_of::<LinearRgba>()))
                    .unwrap_or(u64::MAX),
            })?;
    }
    let table = &*LINEAR_CELL_STATE_SHADES;
    match cell_states {
        Some(states) => {
            for (&lineage, &state) in lineages.iter().zip(states) {
                out.push(table[lineage_slot(lineage)][cell_state_index(state)]);
            }
        }
        None => {
            let zero = cell_state_index(0);
            for &lineage in lineages {
                out.push(table[lineage_slot(lineage)][zero]);
            }
        }
    }
    Ok(())
}

/// Decode a quantized `Epoch::polarities` entry back to world-coordinate
/// radians `atan2(p_y, p_x) ∈ [-π, π]`, or `None` for [`POLARITY_UNPOLARIZED`].
#[must_use]
pub fn decode_polarity_radians(quantized: i16) -> Option<f64> {
    if quantized == POLARITY_UNPOLARIZED {
        return None;
    }
    Some(f64::from(quantized) * PI / f64::from(POLARITY_ANGLE_SCALE))
}

/// Convert a quantized world polarity angle to a counter-clockwise screen turn
/// in `[0, 1)` (`0` = east, `0.25` = screen-north, `0.5` = west, `0.75` =
/// screen-south), matching [`crate::heading`].
#[must_use]
pub fn polarity_turn(quantized: i16) -> Option<f64> {
    let theta = decode_polarity_radians(quantized)?;
    let turn = (-theta / TAU).rem_euclid(1.0);
    Some(if turn >= 1.0 { 0.0 } else { turn })
}

/// Unit apical direction vector `[dx, dy]` in FrankenManim scene space (x right,
/// y up), or `None` when [`POLARITY_UNPOLARIZED`].
#[must_use]
pub fn polarity_scene_direction(quantized: i16) -> Option<[f64; 2]> {
    let theta = decode_polarity_radians(quantized)?;
    let (sin, cos) = theta.sin_cos();
    Some([cos, -sin])
}

/// Precomputed 1024-step perceptually uniform cyclic OKLab hue wheel (sRGB).
static POLARITY_WHEEL_RGB8: LazyLock<[[u8; 3]; POLARITY_LUT_LEN]> = LazyLock::new(|| {
    std::array::from_fn(|index| {
        let phi = TAU * (index as f64) / (POLARITY_LUT_LEN as f64);
        let (sin, cos) = phi.sin_cos();
        most_chroma(
            POLARITY_WHEEL_LIGHTNESS,
            POLARITY_WHEEL_CHROMA * cos,
            POLARITY_WHEEL_CHROMA * sin,
        )
    })
});

/// Precomputed linear-light cyclic OKLab hue wheel for `polarity-v1`.
static POLARITY_WHEEL_LINEAR: LazyLock<[LinearRgba; POLARITY_LUT_LEN]> = LazyLock::new(|| {
    let linear_bytes = linear_table();
    std::array::from_fn(|index| linear_with_table(linear_bytes, POLARITY_WHEEL_RGB8[index]))
});

/// Precomputed linear-light colour for [`POLARITY_UNPOLARIZED`].
static POLARITY_UNPOLARIZED_LINEAR: LazyLock<LinearRgba> =
    LazyLock::new(|| linear(POLARITY_UNPOLARIZED_COLOR));

#[inline]
pub(crate) fn polarity_lut_index(turn: f64) -> usize {
    let wrapped = turn.rem_euclid(1.0);
    let step = (wrapped * (POLARITY_LUT_LEN as f64)).round() as usize;
    step & (POLARITY_LUT_LEN - 1)
}

#[inline]
pub(crate) fn polarity_wheel_linear() -> &'static [LinearRgba; POLARITY_LUT_LEN] {
    &POLARITY_WHEEL_LINEAR
}

#[inline]
pub(crate) fn polarity_unpolarized_linear() -> LinearRgba {
    *POLARITY_UNPOLARIZED_LINEAR
}

/// Cyclic OKLab perceptual wheel colour at counter-clockwise screen turn
/// `turn ∈ [0, 1)`; non-finite turns return [`POLARITY_UNPOLARIZED_COLOR`].
#[must_use]
pub fn polarity_wheel_rgb8(turn: f64) -> [u8; 3] {
    if !turn.is_finite() {
        return POLARITY_UNPOLARIZED_COLOR;
    }
    POLARITY_WHEEL_RGB8[polarity_lut_index(turn)]
}

/// The sRGB colour of `polarity-v1` for a quantized polarity entry (`i16`).
/// `+32767` and `-32767` (`±π`) map to the exact same colour across the branch
/// cut, and [`POLARITY_UNPOLARIZED`] maps to [`POLARITY_UNPOLARIZED_COLOR`].
#[must_use]
pub fn polarity_rgb8(polarity: i16) -> [u8; 3] {
    polarity_turn(polarity).map_or(POLARITY_UNPOLARIZED_COLOR, |turn| {
        POLARITY_WHEEL_RGB8[polarity_lut_index(turn)]
    })
}

/// [`polarity_rgb8`] as opaque linear light.
#[must_use]
pub fn polarity_color(polarity: i16) -> LinearRgba {
    polarity_turn(polarity).map_or(*POLARITY_UNPOLARIZED_LINEAR, |turn| {
        POLARITY_WHEEL_LINEAR[polarity_lut_index(turn)]
    })
}

/// Compact HUD legend for `polarity-v1` (4 cardinal angles + unpolarized grey).
#[must_use]
pub fn polarity_legend() -> Legend {
    let swatch = HUD_SWATCH;
    let cardinals = [
        ("east", 0.0_f64),
        ("north", 0.25),
        ("west", 0.5),
        ("south", 0.75),
    ];
    let mut text = String::from(POLARITY_MAPPING_NAME);
    let mut swatches = Vec::with_capacity(5);
    for (label, turn) in cardinals {
        text.push(' ');
        text.push(swatch);
        text.push(' ');
        text.push_str(label);
        swatches.push(polarity_wheel_rgb8(turn));
    }
    text.push(' ');
    text.push(swatch);
    text.push_str(" none (apical)");
    swatches.push(POLARITY_UNPOLARIZED_COLOR);
    Legend { text, swatches }
}

/// Lower `count` organisms' optional quantized `polarities` into linear colours
/// in `out`. When `polarities` is `None`, fills `out` with the unpolarized
/// neutral colour.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] if `polarities` has length `!= count`;
/// [`CoreError::AllocationFailed`] if `out` cannot be reserved.
pub fn lower_polarity_colors(
    count: usize,
    polarities: Option<&[i16]>,
    out: &mut Vec<LinearRgba>,
) -> Result<(), CoreError> {
    if let Some(angles) = polarities
        && angles.len() != count
    {
        return Err(CoreError::LayoutMismatch {
            buffer: "polarity visual mapping",
            detail: format!("{} polarities for {count} organisms", angles.len()),
        });
    }
    out.clear();
    if out.capacity() < count {
        out.try_reserve_exact(count)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "polarity colours",
                bytes: u64::try_from(count.saturating_mul(size_of::<LinearRgba>()))
                    .unwrap_or(u64::MAX),
            })?;
    }
    let wheel = &*POLARITY_WHEEL_LINEAR;
    let unpolarized = *POLARITY_UNPOLARIZED_LINEAR;
    match polarities {
        Some(angles) => {
            for &angle in angles {
                let color = polarity_turn(angle)
                    .map_or(unpolarized, |turn| wheel[polarity_lut_index(turn)]);
                out.push(color);
            }
        }
        None => {
            out.resize(count, unpolarized);
        }
    }
    Ok(())
}

/// Build one short apical orientation tick quad in FrankenManim scene space
/// (`[SurfaceVertex; 4]` and clockwise triangle indices `[0, 1, 2, 0, 2, 3]`)
/// anchored on `center`, sized proportionally to `dot_radius`, and pointing
/// along `polarity`'s apical vector.
///
/// Returns `None` for [`POLARITY_UNPOLARIZED`], non-finite `center`, or
/// non-finite / non-positive `dot_radius`.
#[must_use]
pub fn apical_tick_quad(
    center: [f64; 3],
    dot_radius: f64,
    polarity: i16,
    color: LinearRgba,
) -> Option<([SurfaceVertex; 4], [u32; 6])> {
    if !center.iter().all(|c| c.is_finite()) || !(dot_radius.is_finite() && dot_radius > 0.0) {
        return None;
    }
    let [dx, dy] = polarity_scene_direction(polarity)?;
    let start = dot_radius * APICAL_TICK_START_RATIO;
    let end = dot_radius * APICAL_TICK_END_RATIO;
    let half_w = dot_radius * APICAL_TICK_HALF_WIDTH_RATIO;
    if !(start.is_finite() && end.is_finite() && half_w.is_finite()) {
        return None;
    }
    // Left perpendicular (90° CCW from direction) in scene XY (y up):
    // ordering (back-right, back-left, front-left, front-right) is clockwise.
    let [lx, ly] = [-dy, dx];
    let [cx, cy, cz] = center;
    let sx = cx + dx * start;
    let sy = cy + dy * start;
    let ex = cx + dx * end;
    let ey = cy + dy * end;
    let corners = [
        [sx - lx * half_w, sy - ly * half_w, cz],
        [sx + lx * half_w, sy + ly * half_w, cz],
        [ex + lx * half_w, ey + ly * half_w, cz],
        [ex - lx * half_w, ey - ly * half_w, cz],
    ];
    if !corners
        .iter()
        .all(|p| p[0].is_finite() && p[1].is_finite() && p[2].is_finite())
    {
        return None;
    }
    let normal = [0.0, 0.0, 1.0];
    let vertices = corners.map(|point| SurfaceVertex::colored(point, normal, color));
    Some((vertices, [0, 1, 2, 0, 2, 3]))
}

/// Build one bounded bulk [`SurfaceMesh`] containing short apical orientation
/// ticks for every polarized organism in `(positions, polarities)` through
/// `viewport` (never one scene object per organism).
///
/// Unpolarized entries ([`POLARITY_UNPOLARIZED`]) are omitted; returns
/// `Ok(None)` when no polarized organisms are present.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] if `positions.len() != polarities.len()`;
/// [`CoreError::NumericalInvalid`] if `dot_radius` is not finite and positive
/// or any polarized organism has a non-finite position;
/// [`CoreError::Overflow`] if vertex count exceeds `u32::MAX`;
/// [`CoreError::AllocationFailed`] if the mesh buffers cannot be reserved.
pub fn apical_ticks_mesh(
    viewport: &Viewport,
    positions: &[[f32; 2]],
    polarities: &[i16],
    dot_radius: f64,
    color: LinearRgba,
) -> Result<Option<SurfaceMesh>, CoreError> {
    if positions.len() != polarities.len() {
        return Err(CoreError::LayoutMismatch {
            buffer: "apical ticks",
            detail: format!(
                "{} positions for {} polarities",
                positions.len(),
                polarities.len()
            ),
        });
    }
    if !(dot_radius.is_finite()
        && dot_radius > 0.0
        && (dot_radius * APICAL_TICK_END_RATIO).is_finite())
    {
        return Err(CoreError::NumericalInvalid {
            quantity: "apical tick radius",
            detail: format!("dot radius {dot_radius} must be finite, positive and non-overflowing"),
        });
    }
    let polarized = polarities
        .iter()
        .filter(|&&p| p != POLARITY_UNPOLARIZED)
        .count();
    if polarized == 0 {
        return Ok(None);
    }
    let vertex_count = polarized.checked_mul(4).ok_or(CoreError::Overflow {
        quantity: "apical tick vertices",
    })?;
    if u32::try_from(vertex_count).is_err() {
        return Err(CoreError::Overflow {
            quantity: "apical tick vertices",
        });
    }
    let index_count = polarized.checked_mul(6).ok_or(CoreError::Overflow {
        quantity: "apical tick indices",
    })?;
    let mut vertices = Vec::new();
    vertices
        .try_reserve_exact(vertex_count)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: "apical tick vertices",
            bytes: u64::try_from(vertex_count.saturating_mul(size_of::<SurfaceVertex>()))
                .unwrap_or(u64::MAX),
        })?;
    let mut indices = Vec::new();
    indices
        .try_reserve_exact(index_count)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: "apical tick indices",
            bytes: u64::try_from(index_count.saturating_mul(size_of::<u32>())).unwrap_or(u64::MAX),
        })?;
    let (start, extent, half) = viewport.scene_transform();
    for (&position, &polarity) in positions.iter().zip(polarities) {
        if polarity == POLARITY_UNPOLARIZED {
            continue;
        }
        let world = [f64::from(position[0]), f64::from(position[1])];
        if !world.iter().all(|c| c.is_finite()) {
            return Err(CoreError::NumericalInvalid {
                quantity: "apical tick position",
                detail: format!("position {position:?} must be finite"),
            });
        }
        let center = Viewport::scene_point_with(start, extent, half, world);
        if let Some((quad_verts, quad_idx)) = apical_tick_quad(center, dot_radius, polarity, color)
        {
            let base = u32::try_from(vertices.len()).map_err(|_| CoreError::Overflow {
                quantity: "apical tick vertices",
            })?;
            vertices.extend_from_slice(&quad_verts);
            indices.extend(quad_idx.map(|i| base + i));
        }
    }
    SurfaceMesh::new(vertices, indices)
        .map(Some)
        .map_err(|error| CoreError::NumericalInvalid {
            quantity: "apical ticks",
            detail: error.to_string(),
        })
}

/// Build one bounded bulk [`SurfaceMesh`] of apical orientation ticks from
/// [`crate::WorldView`]'s drawn `f64` world positions, `shown` visibility flags
/// and quantized `polarities`, with optional conservative frustum culling
/// `(half_fw, half_fh, focal)` at `zoom > 1`.
pub(crate) fn apical_ticks_mesh_from_drawn(
    viewport: &Viewport,
    positions: &[[f64; 2]],
    shown: &[bool],
    polarities: &[i16],
    dot_radius: f64,
    color: LinearRgba,
    cull: Option<(f64, f64, f64)>,
) -> Result<Option<SurfaceMesh>, CoreError> {
    if positions.len() != polarities.len() || shown.len() != polarities.len() {
        return Err(CoreError::LayoutMismatch {
            buffer: "apical ticks",
            detail: format!(
                "{} positions, {} visibilities for {} polarities",
                positions.len(),
                shown.len(),
                polarities.len()
            ),
        });
    }
    if !(dot_radius.is_finite()
        && dot_radius > 0.0
        && (dot_radius * APICAL_TICK_END_RATIO).is_finite())
    {
        return Err(CoreError::NumericalInvalid {
            quantity: "apical tick radius",
            detail: format!("dot radius {dot_radius} must be finite, positive and non-overflowing"),
        });
    }
    let (start, extent, half) = viewport.scene_transform();
    let reach = 1.5 * dot_radius * APICAL_TICK_END_RATIO;
    let visible = |center: [f64; 3]| -> bool {
        let Some((half_fw, half_fh, focal)) = cull else {
            return true;
        };
        if !focal.is_finite() || focal <= 0.0 || reach >= focal {
            return true;
        }
        let margin = 1.0 + reach / focal;
        center[0].abs() <= half_fw * margin + reach && center[1].abs() <= half_fh * margin + reach
    };
    let mut emitted = 0_usize;
    for ((&position, &is_shown), &polarity) in positions.iter().zip(shown).zip(polarities) {
        if !is_shown || polarity == POLARITY_UNPOLARIZED {
            continue;
        }
        if !position.iter().all(|c| c.is_finite()) {
            return Err(CoreError::NumericalInvalid {
                quantity: "apical tick position",
                detail: format!("position {position:?} must be finite"),
            });
        }
        let center = Viewport::scene_point_with(start, extent, half, position);
        if visible(center) {
            emitted += 1;
        }
    }
    if emitted == 0 {
        return Ok(None);
    }
    let vertex_count = emitted.checked_mul(4).ok_or(CoreError::Overflow {
        quantity: "apical tick vertices",
    })?;
    if u32::try_from(vertex_count).is_err() {
        return Err(CoreError::Overflow {
            quantity: "apical tick vertices",
        });
    }
    let index_count = emitted.checked_mul(6).ok_or(CoreError::Overflow {
        quantity: "apical tick indices",
    })?;
    let mut vertices = Vec::new();
    vertices
        .try_reserve_exact(vertex_count)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: "apical tick vertices",
            bytes: u64::try_from(vertex_count.saturating_mul(size_of::<SurfaceVertex>()))
                .unwrap_or(u64::MAX),
        })?;
    let mut indices = Vec::new();
    indices
        .try_reserve_exact(index_count)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: "apical tick indices",
            bytes: u64::try_from(index_count.saturating_mul(size_of::<u32>())).unwrap_or(u64::MAX),
        })?;
    for ((&position, &is_shown), &polarity) in positions.iter().zip(shown).zip(polarities) {
        if !is_shown || polarity == POLARITY_UNPOLARIZED {
            continue;
        }
        let center = Viewport::scene_point_with(start, extent, half, position);
        if !visible(center) {
            continue;
        }
        if let Some((quad_verts, quad_idx)) = apical_tick_quad(center, dot_radius, polarity, color)
        {
            let base = u32::try_from(vertices.len()).map_err(|_| CoreError::Overflow {
                quantity: "apical tick vertices",
            })?;
            vertices.extend_from_slice(&quad_verts);
            indices.extend(quad_idx.map(|i| base + i));
        }
    }
    if vertices.is_empty() {
        return Ok(None);
    }
    SurfaceMesh::new(vertices, indices)
        .map(Some)
        .map_err(|error| CoreError::NumericalInvalid {
            quantity: "apical ticks",
            detail: error.to_string(),
        })
}

/// Unlit depth-test-free [`ThreeDDraw`] for an apical tick [`SurfaceMesh`],
/// drawn directly over sphere-mode organisms.
#[must_use]
pub fn apical_tick_draw(mesh: &SurfaceMesh) -> ThreeDDraw<'_> {
    ThreeDDraw::Surface(SurfaceDraw {
        shading: [0.0; 3],
        depth_test: false,
        ..SurfaceDraw::new(mesh)
    })
}

/// Which `V2-16` secondary field overlay a texture or layer displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldOverlayKind {
    /// Persistent Carrier-1 ant pheromone highways (`e_1`, `V2-15`).
    TrailCarrier,
    /// Self-carving Murray/Physarum vascular conductance (`g_e`, `V2-14`).
    VascularConductance,
}

impl FieldOverlayKind {
    /// Both `V2-16` field overlay modes in canonical Studio `C`-key order.
    pub const ALL: [Self; 2] = [Self::TrailCarrier, Self::VascularConductance];

    /// Flag / HUD name (`trail-carrier` or `vascular-conductance`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::TrailCarrier => TRAIL_CARRIER_NAME,
            Self::VascularConductance => VASCULAR_CONDUCTANCE_NAME,
        }
    }

    /// The six-stop perceptual sRGB palette for this overlay.
    #[must_use]
    pub const fn palette(self) -> &'static [[u8; 3]; 6] {
        match self {
            Self::TrailCarrier => &TRAIL_CARRIER_PALETTE,
            Self::VascularConductance => &VASCULAR_CONDUCTANCE_PALETTE,
        }
    }

    /// Interpolate `t ∈ [0, 1]` along this overlay's sRGB palette.
    #[must_use]
    pub fn rgb8(self, t: f64) -> [u8; 3] {
        ramp(self.palette(), t)
    }

    /// Compact HUD legend strictly within FL-024's vertex cap.
    #[must_use]
    pub fn legend(self) -> Legend {
        let swatch = HUD_SWATCH;
        let stops = [0.0, 0.25, 0.5, 0.75, 1.0];
        let swatches = stops.map(|t| self.rgb8(t)).to_vec();
        let suffix = match self {
            Self::TrailCarrier => "max (e1 pheromone)",
            Self::VascularConductance => "max (vessel g_e)",
        };
        let text = format!(
            "field {} {swatch} 0 {swatch} {swatch} {swatch} {swatch} {suffix}",
            self.name()
        );
        Legend { text, swatches }
    }
}

impl fmt::Display for FieldOverlayKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl FromStr for FieldOverlayKind {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.name() == text)
            .ok_or_else(|| {
                format!("`{text}` is not a field overlay: trail-carrier or vascular-conductance")
            })
    }
}

/// Logarithmic perceptual display level `ln(1 + k · x) / ln(1 + k)` (`k =
/// OVERLAY_LOG_GAIN`) of a nonnegative overlay density `value` against `scale`,
/// clamped to `[0, 1]`; `0` for negative, non-finite, or non-positive inputs.
#[must_use]
pub fn overlay_log_value(value: f32, scale: f64) -> f64 {
    if !scale.is_finite() || scale <= 0.0 || !value.is_finite() || value <= 0.0 {
        return 0.0;
    }
    let norm = (f64::from(value) / scale).clamp(0.0, 1.0);
    ((OVERLAY_LOG_GAIN * norm).ln_1p() / OVERLAY_LOG_GAIN.ln_1p()).clamp(0.0, 1.0)
}

/// Display level for a field overlay under `look`: logarithmic perceptual ramp
/// in [`Look::Glow`], honest unstyled linear [`raw_value`] in [`Look::Raw`] and
/// [`Look::Sphere`].
#[must_use]
pub fn overlay_level(value: f32, scale: f64, look: Look) -> f64 {
    match look {
        Look::Glow => overlay_log_value(value, scale),
        Look::Raw | Look::Sphere => raw_value(value, scale),
    }
}

/// `t ∈ [0, 1]` through [`TRAIL_CARRIER_PALETTE`].
#[must_use]
pub fn trail_carrier_rgb8(t: f64) -> [u8; 3] {
    ramp(&TRAIL_CARRIER_PALETTE, t)
}

/// `t ∈ [0, 1]` through [`VASCULAR_CONDUCTANCE_PALETTE`].
#[must_use]
pub fn vascular_conductance_rgb8(t: f64) -> [u8; 3] {
    ramp(&VASCULAR_CONDUCTANCE_PALETTE, t)
}

/// Pure linear-light convex blend of `overlay` over `base` weighted by
/// `opacity ∈ [0, 1]`. Non-finite channels or opacities clamp safely to `0.0`,
/// and every output channel stays finite in `[0, 1]`.
#[must_use]
pub fn overlay_blend_linear(base: LinearRgba, overlay: LinearRgba, opacity: f64) -> LinearRgba {
    let unit = |x: f64| {
        if x.is_finite() {
            x.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    let alpha = unit(opacity) * unit(overlay.a);
    let inv = 1.0 - alpha;
    let br = unit(base.r);
    let bg = unit(base.g);
    let bb = unit(base.b);
    let ba = unit(base.a);
    let or = unit(overlay.r);
    let og = unit(overlay.g);
    let ob = unit(overlay.b);
    LinearRgba {
        r: (br * inv + or * alpha).clamp(0.0, 1.0),
        g: (bg * inv + og * alpha).clamp(0.0, 1.0),
        b: (bb * inv + ob * alpha).clamp(0.0, 1.0),
        a: (ba + alpha * (1.0 - ba)).clamp(0.0, 1.0),
    }
}

fn check_overlay_plane(
    texels: [u32; 2],
    values: &[f32],
    buffer: &'static str,
) -> Result<(), CoreError> {
    let [columns, rows] = texels.map(|n| n as usize);
    if columns.checked_mul(rows) != Some(values.len()) || values.is_empty() {
        return Err(CoreError::LayoutMismatch {
            buffer,
            detail: format!("{} values for {columns} x {rows} texels", values.len()),
        });
    }
    Ok(())
}

fn overlay_next_scale(values: &[f32], previous: f64) -> f64 {
    let brightest = values
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .fold(0.0_f32, f32::max);
    f64::from(brightest).max(previous * SCALE_DECAY)
}

#[inline]
const fn normalized_overlay_look(look: Look) -> Look {
    match look {
        Look::Glow => Look::Glow,
        Look::Raw | Look::Sphere => Look::Raw,
    }
}

/// Build one torus-seamless premultiplied-linear [`Texture`] for a `V2-16`
/// field overlay plane (`trail_energy` or `vascular_conductance`).
///
/// Each texel `(column, row)` is the corner average of the four surrounding
/// cells (wrapping seamlessly across torus seams), mapped through
/// [`overlay_level`] and either `kind`'s palette (`Look::Glow`) or honest
/// linear grey (`Look::Raw` / `Look::Sphere`).
///
/// # Errors
/// [`CoreError::LayoutMismatch`] if `values.len() != texels[0] * texels[1]` or
/// empty; [`CoreError::AllocationFailed`] if texel memory cannot be reserved;
/// [`CoreError::NumericalInvalid`] if FrankenManim refuses the texture.
pub fn build_overlay_texture(
    texels: [u32; 2],
    values: &[f32],
    scale: f64,
    look: Look,
    kind: FieldOverlayKind,
) -> Result<Texture, CoreError> {
    check_overlay_plane(texels, values, "field overlay")?;
    let [columns, rows] = texels.map(|n| n as usize);
    let cells = columns * rows;
    let mut out = Vec::new();
    out.try_reserve_exact(cells)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: "field overlay texels",
            bytes: u64::try_from(cells.saturating_mul(size_of::<[f32; 4]>())).unwrap_or(u64::MAX),
        })?;
    let table = linear_table();
    let stops = kind.palette();
    match normalized_overlay_look(look) {
        Look::Glow => {
            for row in 0..rows {
                let up_row = if row == 0 { rows - 1 } else { row - 1 };
                let up_slice = &values[up_row * columns..up_row * columns + columns];
                let row_slice = &values[row * columns..row * columns + columns];
                for column in 0..columns {
                    let left = if column == 0 { columns - 1 } else { column - 1 };
                    let mean = (f64::from(up_slice[left])
                        + f64::from(up_slice[column])
                        + f64::from(row_slice[left])
                        + f64::from(row_slice[column]))
                        / 4.0;
                    let level = overlay_log_value(mean as f32, scale);
                    let color = linear_with_table(table, ramp(stops, level));
                    out.push([color.r as f32, color.g as f32, color.b as f32, 1.0]);
                }
            }
        }
        Look::Raw | Look::Sphere => {
            for row in 0..rows {
                let up_row = if row == 0 { rows - 1 } else { row - 1 };
                let up_slice = &values[up_row * columns..up_row * columns + columns];
                let row_slice = &values[row * columns..row * columns + columns];
                for column in 0..columns {
                    let left = if column == 0 { columns - 1 } else { column - 1 };
                    let mean = (f64::from(up_slice[left])
                        + f64::from(up_slice[column])
                        + f64::from(row_slice[left])
                        + f64::from(row_slice[column]))
                        / 4.0;
                    let grey = raw_value(mean as f32, scale) as f32;
                    out.push([grey, grey, grey, 1.0]);
                }
            }
        }
    }
    Texture::from_premultiplied_linear(texels[0], texels[1], out).map_err(|error| {
        CoreError::NumericalInvalid {
            quantity: "field overlay texture",
            detail: error.to_string(),
        }
    })
}

/// Composite a secondary overlay (`trail_energy` or `vascular_conductance`)
/// over a primary Carrier-0 `base_energy` plane in linear light, producing one
/// torus-seamless [`Texture`].
///
/// Each corner's overlay blend factor is `opacity * overlay_level`, so cells
/// with zero trail/conductance show the exact unblended base energy colour.
///
/// # Errors
/// [`CoreError::LayoutMismatch`] if either plane does not match `texels`;
/// [`CoreError::NumericalInvalid`] if `opacity` is outside `[0, 1]` or
/// FrankenManim refuses the texture; [`CoreError::AllocationFailed`] on OOM.
#[expect(
    clippy::too_many_arguments,
    reason = "explicit pure read-only inputs for base and overlay planes without hidden state"
)]
pub fn build_composite_overlay_texture(
    texels: [u32; 2],
    base_energy: &[f32],
    base_scale: f64,
    overlay_values: &[f32],
    overlay_scale: f64,
    kind: FieldOverlayKind,
    opacity: f64,
    look: Look,
) -> Result<Texture, CoreError> {
    check_overlay_plane(texels, base_energy, "base energy field")?;
    check_overlay_plane(texels, overlay_values, "field overlay")?;
    if !(opacity.is_finite() && (0.0..=1.0).contains(&opacity)) {
        return Err(CoreError::NumericalInvalid {
            quantity: "field overlay opacity",
            detail: format!("opacity {opacity} must be finite in [0, 1]"),
        });
    }
    let [columns, rows] = texels.map(|n| n as usize);
    let cells = columns * rows;
    let mut out = Vec::new();
    out.try_reserve_exact(cells)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: "composite field overlay texels",
            bytes: u64::try_from(cells.saturating_mul(size_of::<[f32; 4]>())).unwrap_or(u64::MAX),
        })?;
    let table = linear_table();
    let stops = kind.palette();
    let glow = normalized_overlay_look(look) == Look::Glow;
    for row in 0..rows {
        let up_row = if row == 0 { rows - 1 } else { row - 1 };
        let up_base = up_row * columns;
        let cur_base = row * columns;
        for column in 0..columns {
            let left = if column == 0 { columns - 1 } else { column - 1 };
            let c0 = up_base + left;
            let c1 = up_base + column;
            let c2 = cur_base + left;
            let c3 = cur_base + column;
            let base_mean = (f64::from(base_energy[c0])
                + f64::from(base_energy[c1])
                + f64::from(base_energy[c2])
                + f64::from(base_energy[c3]))
                / 4.0;
            let ov_mean = (f64::from(overlay_values[c0])
                + f64::from(overlay_values[c1])
                + f64::from(overlay_values[c2])
                + f64::from(overlay_values[c3]))
                / 4.0;
            let (base_col, ov_col, ov_lvl) = if glow {
                let bl = display_value(base_mean as f32, base_scale);
                let ol = overlay_log_value(ov_mean as f32, overlay_scale);
                (
                    linear_with_table(table, energy_palette(bl)),
                    linear_with_table(table, ramp(stops, ol)),
                    ol,
                )
            } else {
                let bg = raw_value(base_mean as f32, base_scale);
                let og = raw_value(ov_mean as f32, overlay_scale);
                (
                    LinearRgba {
                        r: bg,
                        g: bg,
                        b: bg,
                        a: 1.0,
                    },
                    LinearRgba {
                        r: og,
                        g: og,
                        b: og,
                        a: 1.0,
                    },
                    og,
                )
            };
            let blended = overlay_blend_linear(base_col, ov_col, opacity * ov_lvl);
            out.push([blended.r as f32, blended.g as f32, blended.b as f32, 1.0]);
        }
    }
    Texture::from_premultiplied_linear(texels[0], texels[1], out).map_err(|error| {
        CoreError::NumericalInvalid {
            quantity: "composite field overlay texture",
            detail: error.to_string(),
        }
    })
}

/// Key identifying the geometry and UVs of a cached [`Viewport::periodic_quad`].
type OverlayQuadKey = ([u64; 2], [u64; 2], [u32; 2]);

#[inline]
fn overlay_quad_key(viewport: &Viewport, texels: [u32; 2]) -> OverlayQuadKey {
    (
        viewport.window_start().map(f64::to_bits),
        viewport.extent().map(f64::to_bits),
        texels,
    )
}

fn same_bits(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len()
        && a.chunks(64).zip(b.chunks(64)).all(|(a, b)| {
            a.iter()
                .zip(b)
                .fold(0_u32, |differ, (x, y)| differ | (x.to_bits() ^ y.to_bits()))
                == 0
        })
}

/// Cached torus-seamless field overlay layer for `trail-carrier` (`e_1`) and
/// `vascular-conductance` (`g_e`), mirroring [`crate::FieldLayer`]: tracks a
/// decaying display scale, skips redundant `Texture` rebuilds/SHA-256 on
/// bitwise-identical values, shares identical `Look::Raw`/`Look::Sphere`
/// textures, and reuses the cached periodic quad across unchanged viewports.
#[derive(Debug)]
pub struct FieldOverlayLayer {
    viewport: Viewport,
    texture: Option<Texture>,
    quad: Option<SurfaceMesh>,
    quad_key: Option<OverlayQuadKey>,
    scale: f64,
    built: Option<(Look, FieldOverlayKind, u64, [u32; 2])>,
    built_values: Vec<f32>,
}

impl FieldOverlayLayer {
    /// An empty overlay layer covering `viewport`'s world.
    #[must_use]
    pub const fn new(viewport: &Viewport) -> Self {
        Self {
            viewport: *viewport,
            texture: None,
            quad: None,
            quad_key: None,
            scale: 0.0,
            built: None,
            built_values: Vec::new(),
        }
    }

    /// Use `viewport` from the next update or placement.
    pub const fn refit(&mut self, viewport: &Viewport) {
        self.viewport = *viewport;
    }

    /// Track display scale while the overlay is hidden without building an
    /// unused texture.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if `values` does not fill `texels`.
    pub fn track_scale(&mut self, texels: [u32; 2], values: &[f32]) -> Result<(), CoreError> {
        check_overlay_plane(texels, values, "field overlay")?;
        self.scale = overlay_next_scale(values, self.scale);
        Ok(())
    }

    /// Update the overlay's display scale and build/reuse its texture and quad
    /// for `(texels, values, look, kind)`.
    ///
    /// # Errors
    /// As [`build_overlay_texture`]. Previous state is preserved on error.
    pub fn update(
        &mut self,
        texels: [u32; 2],
        values: &[f32],
        look: Look,
        kind: FieldOverlayKind,
    ) -> Result<(), CoreError> {
        check_overlay_plane(texels, values, "field overlay")?;
        let scale = overlay_next_scale(values, self.scale);
        self.place_at_scale(texels, values, look, kind, scale)
    }

    /// Re-place `(texels, values)` at the current display scale without
    /// advancing scale decay (for pans, zooms, and style toggles).
    ///
    /// # Errors
    /// As [`build_overlay_texture`]. Previous state is preserved on error.
    pub fn place(
        &mut self,
        texels: [u32; 2],
        values: &[f32],
        look: Look,
        kind: FieldOverlayKind,
    ) -> Result<(), CoreError> {
        check_overlay_plane(texels, values, "field overlay")?;
        self.place_at_scale(texels, values, look, kind, self.scale)
    }

    fn place_at_scale(
        &mut self,
        texels: [u32; 2],
        values: &[f32],
        look: Look,
        kind: FieldOverlayKind,
        scale: f64,
    ) -> Result<(), CoreError> {
        let normalized_look = normalized_overlay_look(look);
        let key = (normalized_look, kind, scale.to_bits(), texels);
        let want_quad = overlay_quad_key(&self.viewport, texels);
        if self.built == Some(key) && same_bits(&self.built_values, values) {
            if self.quad.is_none() || self.quad_key != Some(want_quad) {
                self.quad = Some(self.viewport.periodic_quad(texels)?);
                self.quad_key = Some(want_quad);
            }
            self.scale = scale;
            return Ok(());
        }
        let texture = build_overlay_texture(texels, values, scale, look, kind)?;
        let new_quad = if self.quad.is_none() || self.quad_key != Some(want_quad) {
            Some(self.viewport.periodic_quad(texels)?)
        } else {
            None
        };
        let new_values = if self.built_values.capacity() == values.len() {
            None
        } else {
            let mut buf = Vec::new();
            buf.try_reserve_exact(values.len())
                .map_err(|_| CoreError::AllocationFailed {
                    buffer: "field overlay values",
                    bytes: u64::try_from(values.len().saturating_mul(size_of::<f32>()))
                        .unwrap_or(u64::MAX),
                })?;
            Some(buf)
        };
        self.texture = Some(texture);
        if let Some(quad) = new_quad {
            self.quad = Some(quad);
            self.quad_key = Some(want_quad);
        }
        self.scale = scale;
        if let Some(buf) = new_values {
            self.built_values = buf;
        } else {
            self.built_values.clear();
        }
        self.built_values.extend_from_slice(values);
        self.built = Some(key);
        Ok(())
    }

    /// The current display scale.
    #[must_use]
    pub const fn scale(&self) -> f64 {
        self.scale
    }

    /// The built texture, if any.
    #[must_use]
    pub const fn texture(&self) -> Option<&Texture> {
        self.texture.as_ref()
    }

    /// The overlay draw (unlit repeating image on the periodic quad), if built.
    #[must_use]
    pub fn draw(&self) -> Option<ThreeDDraw<'_>> {
        let (quad, texture) = (self.quad.as_ref()?, self.texture.as_ref()?);
        Some(ThreeDDraw::Surface(SurfaceDraw::image(quad, texture)))
    }
}
