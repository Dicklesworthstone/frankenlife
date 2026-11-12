//! The Studio's on-screen text (FL-024's metric overlay, plan §12.2): lines
//! of text fixed to the drawable's top-left corner over a dark plate, drawn
//! by FrankenManim's 3D route as two unlit meshes after everything else.
//!
//! The 3D route draws triangles, not text, so a [`HudFont`] lays the text
//! out with FrankenManim's bundled IBM Plex Sans (`fmn-text`), flattens
//! each glyph's quadratic outline to within [`HUD_FLATNESS_PIXELS`] of the
//! curve at the drawable's pixel size, and ear-clips its contours
//! (`fmn-geom`) into a [`HudText`]: triangles in drawable pixels. Contours
//! nest by containment, an even depth being ink and an odd one a hole, which
//! for a font's non-overlapping contours is its nonzero fill. A glyph the
//! triangulator refuses is left out and counted, never drawn wrong, and the
//! text stops at [`HUD_MAX_VERTICES`] and [`HUD_MAX_TRIANGLES`], which a view
//! that draws text charges whether it shows any or not (FL-043).
//!
//! The meshes are in the view's scene, placed through the camera's own map
//! from drawable pixels ([`hud_scene_point`]), so the text keeps its place
//! and size on screen whatever the pan or zoom.

use std::ops::Range;

use flife_core::CoreError;
use fmn_core::color::LinearRgba;
use fmn_core::types::Vec3;
use fmn_geom::earclip::triangulate;
use fmn_render::{SurfaceMesh, SurfaceVertex};
use fmn_text::layout::layout_text_with_style;
use fmn_text::markup::CharStyle;
use fmn_text::{FontBook, SANS_FAMILY, TextRequest, glyph_quadpath};

use crate::Viewport;

/// The text's em, in logical points.
pub const HUD_TEXT_POINTS: f64 = 13.0;

/// The plate's distance from the drawable's top and left edges, in points.
pub const HUD_MARGIN_POINTS: f64 = 10.0;

/// The plate's padding around the text, in points.
pub const HUD_PADDING_POINTS: f64 = 6.0;

/// The most a glyph's flattened outline departs from its curve, in
/// drawable pixels.
pub const HUD_FLATNESS_PIXELS: f64 = 0.2;

/// The most segments one quadratic is flattened into.
pub const HUD_MAX_CURVE_SEGMENTS: usize = 64;

/// The most vertices a [`HudText`] holds: a quarter as many again as the
/// Studio's longest overlay, its persistent telemetry (plan §14.1), colour
/// key, field key (plan §13.4), brush preview (plan §14.3) and inspector
/// panel included, takes at two pixels a point (33,162, the motion key, the
/// composition field's key and the introduction brush with every count at
/// the largest written in full), rounded up to a multiple of 1,024. A
/// denser screen's finer flattening grows a curve's segments by the square
/// root of its density, so three pixels a point fits too.
pub const HUD_MAX_VERTICES: usize = 41_984;

/// The most triangles a [`HudText`] holds: a quarter as many again as the
/// longest overlay takes (32,082), rounded up to a multiple of 1,024.
pub const HUD_MAX_TRIANGLES: usize = 40_960;

/// The most contours a [`HudText`] holds: each has at least three vertices.
pub const HUD_MAX_RINGS: usize = HUD_MAX_VERTICES / 3;

/// The text's colour, linear light: a light grey, under the brightest
/// organisms so a bloom softens it least.
pub const HUD_TEXT_COLOR: LinearRgba = LinearRgba {
    r: 0.8,
    g: 0.8,
    b: 0.8,
    a: 1.0,
};

/// The plate's colour, linear light.
pub const HUD_PLATE_COLOR: LinearRgba = LinearRgba {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};

/// The plate's opacity: the world shows through it, dimmed.
pub const HUD_PLATE_OPACITY: f64 = 0.6;

/// The character a colour key's swatch is set as: a filled square, which
/// [`HudFont::text_with_swatches`] draws in a swatch's colour.
pub const HUD_SWATCH: char = '■';

/// The most swatches a [`HudText`] colours; a swatch beyond them is drawn
/// in [`HUD_TEXT_COLOR`].
pub const HUD_MAX_SWATCHES: usize = 32;

/// FrankenManim's bundled font book, loaded once.
pub struct HudFont {
    book: FontBook,
}

impl std::fmt::Debug for HudFont {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HudFont")
            .field("family", &SANS_FAMILY)
            .finish_non_exhaustive()
    }
}

/// Text as triangles in drawable pixels, from its block's top-left corner
/// with y down.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HudText {
    vertices: Vec<[f64; 2]>,
    rings: Vec<Range<usize>>,
    triangles: Vec<[u32; 3]>,
    swatches: Vec<(Range<usize>, LinearRgba)>,
    size: [f64; 2],
    glyphs: usize,
    refused: usize,
    left_out: usize,
}

impl HudFont {
    /// FrankenManim's bundled font book.
    ///
    /// # Errors
    /// [`CoreError::MissingCapability`] if a bundled face does not parse
    /// (a corrupt build).
    pub fn bundled() -> Result<Self, CoreError> {
        let book = FontBook::bundled().map_err(|error| CoreError::MissingCapability {
            capability: format!("FrankenManim's bundled fonts: {error}"),
        })?;
        Ok(Self { book })
    }

    /// `text`'s lines (`\n` breaks them) at an em of `em_pixels` drawable
    /// pixels, in IBM Plex Sans.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonpositive or nonfinite em,
    /// or text the bundled faces cannot set (a character none of them
    /// maps); [`CoreError::AllocationFailed`] when the buffers cannot be
    /// reserved.
    pub fn text(&self, text: &str, em_pixels: f64) -> Result<HudText, CoreError> {
        self.text_with_swatches(text, em_pixels, &[])
    }

    /// [`HudFont::text`], each [`HUD_SWATCH`] in `text` taking the next of
    /// `swatches` (a colour key, FL-019): the first [`HUD_MAX_SWATCHES`]
    /// are coloured, and a swatch without a colour is drawn as text.
    ///
    /// # Errors
    /// As [`HudFont::text`].
    pub fn text_with_swatches(
        &self,
        text: &str,
        em_pixels: f64,
        swatches: &[LinearRgba],
    ) -> Result<HudText, CoreError> {
        if !em_pixels.is_finite() || em_pixels <= 0.0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "overlay em",
                detail: format!("{em_pixels} drawable pixels"),
            });
        }
        let style = CharStyle {
            family: Some(SANS_FAMILY.to_owned()),
            ..CharStyle::base()
        };
        let layout = layout_text_with_style(&self.book, &TextRequest::plain(text), &style)
            .map_err(|error| CoreError::NumericalInvalid {
                quantity: "overlay text",
                detail: error.to_string(),
            })?;
        let mut out = HudText::reserved()?;
        let top = layout.height;
        let place = |point: Vec3| [point[0] * em_pixels, (top - point[1]) * em_pixels];
        let mut ink = [0.0_f64, 0.0];
        let mut colors = swatches.iter().take(HUD_MAX_SWATCHES);
        for (index, glyph) in layout.glyphs.iter().enumerate() {
            if glyph.ch.is_whitespace() {
                continue;
            }
            // Each swatch takes its colour in order, drawn or not.
            let color = if glyph.ch == HUD_SWATCH {
                colors.next()
            } else {
                None
            };
            let Ok(path) = glyph_quadpath(&self.book, glyph) else {
                out.refused += 1;
                continue;
            };
            let rings: Vec<Vec<[f64; 2]>> = path
                .subpaths()
                .into_iter()
                .map(|subpath| flatten_subpath(subpath, &place))
                .filter(|ring| ring.len() >= 3)
                .collect();
            let Some(triangles) = triangulate_rings(&rings) else {
                out.refused += 1;
                continue;
            };
            let vertices: usize = rings.iter().map(Vec::len).sum();
            if out.vertices.len() + vertices > HUD_MAX_VERTICES
                || out.triangles.len() + triangles.len() > HUD_MAX_TRIANGLES
                || out.rings.len() + rings.len() > HUD_MAX_RINGS
            {
                out.left_out = layout.glyphs[index..]
                    .iter()
                    .filter(|glyph| !glyph.ch.is_whitespace())
                    .count();
                break;
            }
            let base = u32::try_from(out.vertices.len()).unwrap_or(u32::MAX);
            if let Some(&color) = color {
                out.swatches
                    .push((out.vertices.len()..out.vertices.len() + vertices, color));
            }
            for ring in &rings {
                let start = out.vertices.len();
                for &vertex in ring {
                    ink = [ink[0].max(vertex[0]), ink[1].max(vertex[1])];
                }
                out.vertices.extend_from_slice(ring);
                out.rings.push(start..out.vertices.len());
            }
            out.triangles.extend(
                triangles
                    .iter()
                    .map(|triangle| triangle.map(|corner| base + corner)),
            );
            out.glyphs += 1;
        }
        out.size = [
            (layout.width * em_pixels).max(ink[0]),
            ((layout.height + layout.depth) * em_pixels).max(ink[1]),
        ];
        Ok(out)
    }
}

impl HudText {
    /// Empty buffers that can hold the most a text holds.
    fn reserved() -> Result<Self, CoreError> {
        let refused = |buffer: &'static str, bytes: usize| CoreError::AllocationFailed {
            buffer,
            bytes: u64::try_from(bytes).unwrap_or(u64::MAX),
        };
        let mut text = Self::default();
        text.vertices
            .try_reserve_exact(HUD_MAX_VERTICES)
            .map_err(|_| refused("overlay vertices", HUD_MAX_VERTICES * size_of::<[f64; 2]>()))?;
        text.rings
            .try_reserve_exact(HUD_MAX_RINGS)
            .map_err(|_| refused("overlay rings", HUD_MAX_RINGS * size_of::<Range<usize>>()))?;
        text.triangles
            .try_reserve_exact(HUD_MAX_TRIANGLES)
            .map_err(|_| {
                refused(
                    "overlay triangles",
                    HUD_MAX_TRIANGLES * size_of::<[u32; 3]>(),
                )
            })?;
        text.swatches
            .try_reserve_exact(HUD_MAX_SWATCHES)
            .map_err(|_| {
                refused(
                    "overlay swatches",
                    HUD_MAX_SWATCHES * size_of::<(Range<usize>, LinearRgba)>(),
                )
            })?;
        Ok(text)
    }

    /// The bytes a text's buffers hold: the most it can hold.
    #[must_use]
    pub const fn reserved_bytes() -> u64 {
        (HUD_MAX_VERTICES * size_of::<[f64; 2]>()
            + HUD_MAX_RINGS * size_of::<Range<usize>>()
            + HUD_MAX_TRIANGLES * size_of::<[u32; 3]>()
            + HUD_MAX_SWATCHES * size_of::<(Range<usize>, LinearRgba)>()) as u64
    }

    /// The flattened outline's vertices, in drawable pixels from the
    /// block's top-left corner, y down.
    #[must_use]
    pub fn vertices(&self) -> &[[f64; 2]] {
        &self.vertices
    }

    /// Each contour's range of [`HudText::vertices`], wound as the font
    /// winds it.
    #[must_use]
    pub fn rings(&self) -> &[Range<usize>] {
        &self.rings
    }

    /// The triangles, as indices into [`HudText::vertices`]: their union is
    /// the ink.
    #[must_use]
    pub fn triangles(&self) -> &[[u32; 3]] {
        &self.triangles
    }

    /// Each coloured swatch's range of [`HudText::vertices`] and colour, in
    /// order; every other vertex is drawn in [`HUD_TEXT_COLOR`].
    #[must_use]
    pub fn swatches(&self) -> &[(Range<usize>, LinearRgba)] {
        &self.swatches
    }

    /// The block's width and height in drawable pixels: the layout's box,
    /// widened to any ink beyond it.
    #[must_use]
    pub const fn size(&self) -> [f64; 2] {
        self.size
    }

    /// The glyphs drawn.
    #[must_use]
    pub const fn glyphs(&self) -> usize {
        self.glyphs
    }

    /// The glyphs left out because their outline could not be read or
    /// triangulated.
    #[must_use]
    pub const fn refused(&self) -> usize {
        self.refused
    }

    /// The glyphs left out beyond the text's budget.
    #[must_use]
    pub const fn left_out(&self) -> usize {
        self.left_out
    }

    /// Whether there is nothing to draw.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }
}

/// The points along the quadratic from `p0` through control `p1` to `p2`
/// after `p0`, evenly in its parameter: enough of them that no chord
/// departs from the curve by more than `tolerance` (a parabola's chord over
/// a parameter step `h` departs by at most `|p0 − 2p1 + p2| h² / 4`), at
/// most [`HUD_MAX_CURVE_SEGMENTS`].
pub fn flatten_quadratic(
    p0: [f64; 2],
    p1: [f64; 2],
    p2: [f64; 2],
    tolerance: f64,
    out: &mut Vec<[f64; 2]>,
) {
    let bend = (p0[0] - 2.0 * p1[0] + p2[0]).hypot(p0[1] - 2.0 * p1[1] + p2[1]);
    let wanted = (bend / (4.0 * tolerance)).sqrt().ceil();
    let segments = if wanted.is_finite() && wanted >= 1.0 {
        (wanted as usize).min(HUD_MAX_CURVE_SEGMENTS)
    } else {
        1
    };
    for step in 1..=segments {
        let t = step as f64 / segments as f64;
        let s = 1.0 - t;
        out.push([0, 1].map(|axis| s * s * p0[axis] + 2.0 * s * t * p1[axis] + t * t * p2[axis]));
    }
}

/// One subpath in FrankenManim's shared-anchor layout (anchor, handle,
/// anchor, ...) as a ring, mapped by `place`: no closing repeat, no
/// repeated neighbours.
fn flatten_subpath(subpath: &[Vec3], place: &impl Fn(Vec3) -> [f64; 2]) -> Vec<[f64; 2]> {
    let mut ring = Vec::new();
    let Some(&first) = subpath.first() else {
        return ring;
    };
    ring.push(place(first));
    let mut points = Vec::new();
    for curve in 0..subpath.len().saturating_sub(1) / 2 {
        let [p0, p1, p2] = [0, 1, 2].map(|k| place(subpath[2 * curve + k]));
        points.clear();
        flatten_quadratic(p0, p1, p2, HUD_FLATNESS_PIXELS, &mut points);
        for &point in &points {
            if ring.last() != Some(&point) {
                ring.push(point);
            }
        }
    }
    if ring.len() > 1 && ring.first() == ring.last() {
        ring.pop();
    }
    ring
}

/// Whether `point` is inside `ring`, by the crossing rule.
fn contains(ring: &[[f64; 2]], point: [f64; 2]) -> bool {
    let mut inside = false;
    let mut previous = ring[ring.len() - 1];
    for &vertex in ring {
        if (vertex[1] > point[1]) != (previous[1] > point[1]) {
            let x = vertex[0]
                + (point[1] - vertex[1]) * (previous[0] - vertex[0]) / (previous[1] - vertex[1]);
            if point[0] < x {
                inside = !inside;
            }
        }
        previous = vertex;
    }
    inside
}

/// The triangles of a glyph's rings, as indices into their concatenation:
/// each ring at an even depth of containment with the rings one deeper
/// inside it as its holes. `None` when the triangulator refuses one.
fn triangulate_rings(rings: &[Vec<[f64; 2]>]) -> Option<Vec<[u32; 3]>> {
    let mut offsets = Vec::with_capacity(rings.len());
    let mut at = 0_u32;
    for ring in rings {
        offsets.push(at);
        at = at.checked_add(u32::try_from(ring.len()).ok()?)?;
    }
    let depth: Vec<usize> = rings
        .iter()
        .enumerate()
        .map(|(index, ring)| {
            rings
                .iter()
                .enumerate()
                .filter(|&(other, outer)| other != index && contains(outer, ring[0]))
                .count()
        })
        .collect();
    let mut triangles = Vec::new();
    for (outer, ring) in rings.iter().enumerate() {
        if depth[outer] % 2 == 1 {
            continue;
        }
        let holes: Vec<usize> = (0..rings.len())
            .filter(|&hole| depth[hole] == depth[outer] + 1 && contains(ring, rings[hole][0]))
            .collect();
        let hole_rings: Vec<&[[f64; 2]]> =
            holes.iter().map(|&hole| rings[hole].as_slice()).collect();
        let local = triangulate(ring, &hole_rings).ok()?;
        // The triangulator's indices run over the outer ring, then each hole.
        let mut global = Vec::with_capacity(ring.len());
        global.extend((0..ring.len()).map(|k| offsets[outer] + k as u32));
        for &hole in &holes {
            global.extend((0..rings[hole].len()).map(|k| offsets[hole] + k as u32));
        }
        for triangle in local {
            triangles.push(triangle.map(|corner| global[corner as usize]));
        }
    }
    Some(triangles)
}

/// The scene point the view's camera draws at the drawable position
/// `position`: the camera's frame is the whole drawable, centred on the
/// scene's origin, `scale` drawable pixels a scene unit, y up.
#[must_use]
pub fn hud_scene_point(viewport: &Viewport, position: [f64; 2]) -> Vec3 {
    let [width, height] = viewport.drawable().map(f64::from);
    let scale = viewport.scale();
    [
        (position[0] - width / 2.0) / scale,
        (height / 2.0 - position[1]) / scale,
        0.0,
    ]
}

/// The drawable position of the text block's top-left corner.
#[must_use]
pub fn hud_origin(viewport: &Viewport) -> [f64; 2] {
    let inset = (HUD_MARGIN_POINTS + HUD_PADDING_POINTS) * viewport.pixels_per_point();
    [inset, inset]
}

/// The plate's drawable rectangle under `text`: its top-left and
/// bottom-right corners.
#[must_use]
pub fn hud_plate_rect(viewport: &Viewport, text: &HudText) -> [[f64; 2]; 2] {
    let pixels = viewport.pixels_per_point();
    let margin = HUD_MARGIN_POINTS * pixels;
    let padding = HUD_PADDING_POINTS * pixels;
    let [width, height] = text.size();
    [
        [margin, margin],
        [
            margin + 2.0 * padding + width,
            margin + 2.0 * padding + height,
        ],
    ]
}

fn mesh(
    vertices: Vec<SurfaceVertex>,
    indices: Vec<u32>,
    quantity: &'static str,
) -> Result<SurfaceMesh, CoreError> {
    SurfaceMesh::new(vertices, indices).map_err(|error| CoreError::NumericalInvalid {
        quantity,
        detail: error.to_string(),
    })
}

fn reserve<T>(buffer: &'static str, count: usize) -> Result<Vec<T>, CoreError> {
    let mut items = Vec::new();
    items
        .try_reserve_exact(count)
        .map_err(|_| CoreError::AllocationFailed {
            buffer,
            bytes: u64::try_from(count.saturating_mul(size_of::<T>())).unwrap_or(u64::MAX),
        })?;
    Ok(items)
}

/// The plate under `text` in the view's scene: two triangles, clockwise in
/// the scene as the view's other meshes are, at [`HUD_PLATE_OPACITY`].
///
/// # Errors
/// [`CoreError::AllocationFailed`] when the mesh cannot be reserved;
/// [`CoreError::NumericalInvalid`] when FrankenManim refuses it.
pub fn hud_plate(viewport: &Viewport, text: &HudText) -> Result<SurfaceMesh, CoreError> {
    let [[left, top], [right, bottom]] = hud_plate_rect(viewport, text);
    let mut vertices = reserve("overlay plate vertices", 4)?;
    for corner in [[left, top], [right, top], [right, bottom], [left, bottom]] {
        vertices.push(SurfaceVertex {
            opacity: HUD_PLATE_OPACITY,
            ..SurfaceVertex::colored(
                hud_scene_point(viewport, corner),
                [0.0, 0.0, 1.0],
                HUD_PLATE_COLOR,
            )
        });
    }
    let mut indices = reserve("overlay plate indices", 6)?;
    indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    mesh(vertices, indices, "overlay plate")
}

/// `text`'s triangles in the view's scene at [`hud_origin`], in
/// [`HUD_TEXT_COLOR`] and each swatch in its colour: clockwise in the scene, since the triangulator's
/// counter-clockwise triangles in y-down pixels flip with y. `None` for an
/// empty text.
///
/// # Errors
/// As [`hud_plate`].
pub fn hud_glyphs(viewport: &Viewport, text: &HudText) -> Result<Option<SurfaceMesh>, CoreError> {
    if text.is_empty() {
        return Ok(None);
    }
    let origin = hud_origin(viewport);
    let mut vertices = reserve("overlay glyph vertices", text.vertices.len())?;
    vertices.extend(text.vertices.iter().enumerate().map(|(index, vertex)| {
        let color = text
            .swatches
            .iter()
            .find(|(range, _)| range.contains(&index))
            .map_or(HUD_TEXT_COLOR, |&(_, color)| color);
        SurfaceVertex::colored(
            hud_scene_point(viewport, [origin[0] + vertex[0], origin[1] + vertex[1]]),
            [0.0, 0.0, 1.0],
            color,
        )
    }));
    let mut indices = reserve("overlay glyph indices", 3 * text.triangles.len())?;
    indices.extend(text.triangles.iter().flatten());
    mesh(vertices, indices, "overlay glyphs").map(Some)
}
