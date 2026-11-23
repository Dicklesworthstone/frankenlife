//! The field under the organisms (plan §13.3, §9.5): the epoch's bounded
//! [`FieldTexture`] drawn as one texture covering the torus (FL-033).
//!
//! In the energy mapping each texel's energy density maps through a fixed
//! dark-to-bright palette (the raw look: linear grey). In the composition
//! mapping (plan §13.4, "Local field": a fixed projection) a corner takes
//! the affinity mapping's colour of its texels' mean composition, weighted
//! by their densities, scaled in linear light by what the energy shows
//! there: hue is chemistry, brightness is energy, and a corner with no
//! energy is black. A component mapping (§13.4's chosen components) does
//! the same with one moment channel per unit energy, its texels'
//! density-weighted mean through the diverging [`COMPONENT_PALETTE`].
//! The display scale follows the brightest texels with a slow decay, so the
//! picture neither flickers nor saturates as the sources wander. The
//! layer's texture holds the colour of every texel corner, the mean of the
//! four texels around it, wrapping across the seam, and
//! [`Viewport::periodic_quad`] puts each on its corner; FrankenManim
//! interpolates them bilinearly, wrapping, so the torus shows no seam. Its
//! CPU and Metal 3D routes both sample textures. The layer is one scene
//! object and one draw of two triangles, whatever the field's size.
//!
//! The texture depends only on the field's values (and compositions, in
//! the composition mapping, or the chosen channel's components, in a
//! component mapping) and texels, the display scale, the look and the
//! mapping; the viewport moves only the quad. The layer keeps a copy of
//! what its texture was built from, and a placement of bitwise the same
//! values, compositions or components, scale, look and mapping keeps the
//! texture and rebuilds only the quad (FL-033): a pan or zoom of one epoch.

use flife_core::CoreError;
use flife_render::{COMPONENT_STEPS, FieldTexture};
use fmn_render::{SurfaceDraw, SurfaceMesh, Texture, ThreeDDraw};

use crate::Viewport;
use crate::mapping::{FieldMapping, Look, affinity_rgb8, linear_table, linear_with_table};

/// The field palette, dark to bright, as sRGB stops evenly spaced in the
/// mapped value.
pub const FIELD_PALETTE: [[u8; 3]; 6] = [
    [0x02, 0x03, 0x08],
    [0x07, 0x16, 0x33],
    [0x0B, 0x33, 0x5C],
    [0x13, 0x5E, 0x76],
    [0x3A, 0x93, 0x86],
    [0xB8, 0xD8, 0x9C],
];

/// The component mappings' diverging palette, as sRGB stops evenly spaced
/// from −1 (a channel wholly against its energy) through 0 to +1 (wholly
/// along it): ColorBrewer's five-class RdYlBu, blue to red.
pub const COMPONENT_PALETTE: [[u8; 3]; 5] = [
    [0x2C, 0x7B, 0xB6],
    [0xAB, 0xD9, 0xE9],
    [0xFF, 0xFF, 0xBF],
    [0xFD, 0xAE, 0x61],
    [0xD7, 0x19, 0x1C],
];

/// A channel's value per unit energy, `value ∈ [−1, 1]`, through
/// [`COMPONENT_PALETTE`], interpolated in sRGB bytes; clamped, and `0` for
/// a value that is not finite.
#[must_use]
pub fn component_rgb8(value: f64) -> [u8; 3] {
    let value = if value.is_finite() {
        value.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let spans = (COMPONENT_PALETTE.len() - 1) as f64;
    let position = (value + 1.0) / 2.0 * spans;
    let low = (position.floor() as usize).min(COMPONENT_PALETTE.len() - 2);
    let fraction = position - low as f64;
    let (a, b) = (COMPONENT_PALETTE[low], COMPONENT_PALETTE[low + 1]);
    let mix =
        |x: u8, y: u8| (f64::from(x) + (f64::from(y) - f64::from(x)) * fraction).round() as u8;
    [mix(a[0], b[0]), mix(a[1], b[1]), mix(a[2], b[2])]
}

/// The mean value per unit energy of a corner's `texels`, each `(density,
/// steps)` with the channel's [`FieldTexture::components`] steps, weighted
/// by their densities: the channel per unit energy of their energy
/// together, as each texel's is its block's, in `[−1, 1]`. A texel whose
/// density is not finite and positive has no weight; with none, `0`.
#[must_use]
pub fn corner_component(texels: [(f32, i8); 4]) -> f64 {
    let mut weight = 0.0_f64;
    let mut sum = 0.0_f64;
    for (density, steps) in texels {
        let density = f64::from(density);
        if density.is_finite() && density > 0.0 {
            weight += density;
            sum += density * f64::from(steps);
        }
    }
    if weight > 0.0 {
        sum / weight / f64::from(COMPONENT_STEPS)
    } else {
        0.0
    }
}

/// How much of the previous display scale survives each update when the
/// field dims: the scale falls by at most 2% per epoch.
pub const SCALE_DECAY: f64 = 0.98;

/// The display value of a density `value` against `scale`: a square root,
/// so faint gradients stay visible, clamped to `[0, 1]`.
#[must_use]
pub fn display_value(value: f32, scale: f64) -> f64 {
    if scale.is_nan() || scale <= 0.0 || !value.is_finite() || value <= 0.0 {
        return 0.0;
    }
    (f64::from(value) / scale).sqrt().clamp(0.0, 1.0)
}

/// The raw display value of a density `value` against `scale`: linear,
/// clamped to `[0, 1]`, so the grey's linear light is proportional to the
/// density.
#[must_use]
pub fn raw_value(value: f32, scale: f64) -> f64 {
    if scale.is_nan() || scale <= 0.0 || !value.is_finite() || value <= 0.0 {
        return 0.0;
    }
    (f64::from(value) / scale).clamp(0.0, 1.0)
}

/// `t ∈ [0, 1]` through [`FIELD_PALETTE`], interpolated in sRGB bytes.
#[must_use]
pub fn palette(t: f64) -> [u8; 3] {
    let t = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let spans = (FIELD_PALETTE.len() - 1) as f64;
    let position = t * spans;
    let low = (position.floor() as usize).min(FIELD_PALETTE.len() - 2);
    let fraction = position - low as f64;
    let (a, b) = (FIELD_PALETTE[low], FIELD_PALETTE[low + 1]);
    let mix =
        |x: u8, y: u8| (f64::from(x) + (f64::from(y) - f64::from(x)) * fraction).round() as u8;
    [mix(a[0], b[0]), mix(a[1], b[1]), mix(a[2], b[2])]
}

/// Look class that affects field texels (`Look::Raw` and `Look::Sphere` produce
/// identical linear grey / linear brightness texels).
#[inline]
const fn field_look(look: Look) -> Look {
    match look {
        Look::Glow => Look::Glow,
        Look::Raw | Look::Sphere => Look::Raw,
    }
}

/// Key identifying the geometry and UVs of a cached [`Viewport::periodic_quad`].
type QuadKey = ([u64; 2], [u64; 2], [u32; 2]);

#[inline]
fn quad_key(viewport: &Viewport, cells: [u32; 2]) -> QuadKey {
    (
        viewport.window_start().map(f64::to_bits),
        viewport.extent().map(f64::to_bits),
        cells,
    )
}

/// The field layer: a texture of the field's corner colours on one quad.
#[derive(Debug)]
pub struct FieldLayer {
    viewport: Viewport,
    texture: Option<Texture>,
    quad: Option<SurfaceMesh>,
    /// Viewport/grid key `quad` was built for, so unchanged views keep it.
    quad_key: Option<QuadKey>,
    scale: f64,
    /// What the texture was built from, apart from the values: the normalized
    /// look, the mapping, the display scale's bits and the texels.
    built: Option<(Look, FieldMapping, u64, [u32; 2])>,
    /// The field values the texture was built from, one `f32` a texel.
    built_values: Vec<f32>,
    /// The compositions it was built from in the composition mapping; empty
    /// in the others.
    built_composition: Vec<[f32; 3]>,
    /// The chosen channel's steps it was built from in a component
    /// mapping, one a texel; empty in the others.
    built_component: Vec<i8>,
}

/// `field`'s values, compositions and components fill its texels.
pub(crate) fn check_layout(field: &FieldTexture) -> Result<(), CoreError> {
    let [columns, rows] = field.texels().map(|n| n as usize);
    let (values, composition, components) =
        (field.values(), field.composition(), field.components());
    if columns.checked_mul(rows) != Some(values.len())
        || values.is_empty()
        || composition.len() != values.len()
        || components.len() != values.len()
    {
        return Err(CoreError::LayoutMismatch {
            buffer: "field layer",
            detail: format!(
                "{} values, {} compositions and {} components for {columns} x {rows} texels",
                values.len(),
                composition.len(),
                components.len()
            ),
        });
    }
    Ok(())
}

/// Display scale updated for `field`'s brightest finite texel.
fn next_scale(field: &FieldTexture, previous: f64) -> f64 {
    let brightest = field
        .values()
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .fold(0.0_f32, f32::max);
    f64::from(brightest).max(previous * SCALE_DECAY)
}

/// The mean composition of a corner's `texels`, each `(density,
/// composition)`, weighted by their densities: the composition of their
/// energy together, as each texel's is its block's. A texel whose density
/// is not finite and positive has no weight; with none, there is no
/// composition (`[0; 3]`).
#[must_use]
pub fn corner_composition(texels: [(f32, [f32; 3]); 4]) -> [f32; 3] {
    let mut weight = 0.0_f64;
    let mut sum = [0.0_f64; 3];
    for (density, composition) in texels {
        let density = f64::from(density);
        if density.is_finite() && density > 0.0 {
            weight += density;
            for (total, component) in sum.iter_mut().zip(composition) {
                *total += density * f64::from(component);
            }
        }
    }
    if weight > 0.0 {
        sum.map(|total| (total / weight) as f32)
    } else {
        [0.0; 3]
    }
}

/// Whether `a` and `b` hold the same values bit for bit.
fn same_bits(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len()
        && a.chunks(64).zip(b.chunks(64)).all(|(a, b)| {
            a.iter()
                .zip(b)
                .fold(0, |differ, (x, y)| differ | (x.to_bits() ^ y.to_bits()))
                == 0
        })
}

impl FieldLayer {
    /// A layer covering `viewport`'s world, empty until a field is shown.
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
            built_composition: Vec::new(),
            built_component: Vec::new(),
        }
    }

    /// Map `field` through the palette into the layer's texture (the glow
    /// look).
    ///
    /// # Errors
    /// As [`FieldLayer::update_look`].
    pub fn update(&mut self, field: &FieldTexture) -> Result<(), CoreError> {
        self.update_look(field, Look::Glow)
    }

    /// Map `field`'s energy into the layer's texture: through the
    /// square-root palette for the glow look, as linear grey
    /// ([`raw_value`]) for raw.
    ///
    /// # Errors
    /// As [`FieldLayer::update_mapped`].
    pub fn update_look(&mut self, field: &FieldTexture, look: Look) -> Result<(), CoreError> {
        self.update_mapped(field, look, FieldMapping::Energy)
    }

    /// Update only the display scale for `field`, without building an unused
    /// texture when the style does not draw the field.
    pub(crate) fn track_scale(&mut self, field: &FieldTexture) -> Result<(), CoreError> {
        check_layout(field)?;
        self.scale = next_scale(field, self.scale);
        Ok(())
    }

    /// Map `field` into the layer's texture by `mapping` in `look`, and let
    /// the display scale follow its brightest texel. Energy: through the
    /// square-root palette for the glow look, as linear grey
    /// ([`raw_value`]) for raw. Composition: each corner the
    /// [`affinity_rgb8`] colour of its texels' density-weighted mean
    /// composition, scaled in linear light by the energy's
    /// [`display_value`] for the glow look or [`raw_value`] for raw. A
    /// component: each corner the [`component_rgb8`] colour of its texels'
    /// density-weighted mean channel per unit energy
    /// ([`corner_component`]), scaled the same way.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for a texture whose values or
    /// compositions do not fill its texels; [`CoreError::AllocationFailed`]
    /// when the texels cannot be reserved; [`CoreError::NumericalInvalid`]
    /// if FrankenManim refuses the texture or the quad. The previous texture,
    /// quad and display scale are kept on error.
    pub fn update_mapped(
        &mut self,
        field: &FieldTexture,
        look: Look,
        mapping: FieldMapping,
    ) -> Result<(), CoreError> {
        check_layout(field)?;
        let scale = next_scale(field, self.scale);
        self.place_at_scale(field, look, mapping, scale)
    }

    /// Use `viewport` from the next update or placement.
    pub const fn refit(&mut self, viewport: &Viewport) {
        self.viewport = *viewport;
    }

    /// Place `field`'s energy in the layer at the current viewport and
    /// display scale, which does not move: what a pan or zoom redraws.
    ///
    /// # Errors
    /// As [`FieldLayer::update_mapped`].
    pub fn place(&mut self, field: &FieldTexture, look: Look) -> Result<(), CoreError> {
        self.place_mapped(field, look, FieldMapping::Energy)
    }

    /// Place `field` in the layer by `mapping` at the current viewport and
    /// display scale, which does not move: what a pan or zoom redraws. A
    /// texture built from bitwise the same values (and, in the composition
    /// mapping, compositions, or in a component mapping, that channel's
    /// components), at this scale, in this look and mapping, is kept, and
    /// only the quad is rebuilt.
    ///
    /// # Errors
    /// As [`FieldLayer::update_mapped`].
    pub fn place_mapped(
        &mut self,
        field: &FieldTexture,
        look: Look,
        mapping: FieldMapping,
    ) -> Result<(), CoreError> {
        check_layout(field)?;
        self.place_at_scale(field, look, mapping, self.scale)
    }

    fn place_at_scale(
        &mut self,
        field: &FieldTexture,
        look: Look,
        mapping: FieldMapping,
        scale: f64,
    ) -> Result<(), CoreError> {
        let cells = field.texels();
        let [columns, rows] = cells.map(|n| n as usize);
        let values = field.values();
        let composition = field.composition();
        let composed = mapping == FieldMapping::Composition;
        let channel = mapping
            .component()
            .map(|component| component.channel() as usize);
        let steps = |texel: &[i8; 32]| channel.map_or(0, |channel| texel[channel]);
        let normalized_look = field_look(look);
        let key = (normalized_look, mapping, scale.to_bits(), cells);
        let want_quad = quad_key(&self.viewport, cells);
        if self.built == Some(key)
            && same_bits(&self.built_values, values)
            && (!composed
                || same_bits(
                    self.built_composition.as_flattened(),
                    composition.as_flattened(),
                ))
            && (channel.is_none()
                || self
                    .built_component
                    .iter()
                    .copied()
                    .eq(field.components().iter().map(steps)))
        {
            if self.quad.is_none() || self.quad_key != Some(want_quad) {
                self.quad = Some(self.viewport.periodic_quad(cells)?);
                self.quad_key = Some(want_quad);
            }
            self.scale = scale;
            return Ok(());
        }

        let mut texels = Vec::new();
        texels
            .try_reserve_exact(columns * rows)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "field layer texels",
                bytes: u64::try_from((columns * rows).saturating_mul(size_of::<[f32; 4]>()))
                    .unwrap_or(u64::MAX),
            })?;
        let linear = linear_table();
        // Hoist the mapping/look dispatch and row/column wrap out of the inner
        // loop so each corner avoids integer divisions while preserving the
        // exact corner sum and colour arithmetic bit for bit.
        match (mapping, normalized_look) {
            (FieldMapping::Energy, Look::Glow) => {
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
                        let color =
                            linear_with_table(linear, palette(display_value(mean as f32, scale)));
                        texels.push([color.r as f32, color.g as f32, color.b as f32, 1.0]);
                    }
                }
            }
            (FieldMapping::Energy, Look::Raw | Look::Sphere) => {
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
                        texels.push([grey, grey, grey, 1.0]);
                    }
                }
            }
            (FieldMapping::Composition, _) => {
                let glow = normalized_look == Look::Glow;
                for row in 0..rows {
                    let up_row = if row == 0 { rows - 1 } else { row - 1 };
                    let up_base = up_row * columns;
                    let row_base = row * columns;
                    for column in 0..columns {
                        let left = if column == 0 { columns - 1 } else { column - 1 };
                        let corners = [
                            up_base + left,
                            up_base + column,
                            row_base + left,
                            row_base + column,
                        ];
                        let mean = (f64::from(values[corners[0]])
                            + f64::from(values[corners[1]])
                            + f64::from(values[corners[2]])
                            + f64::from(values[corners[3]]))
                            / 4.0;
                        let level = if glow {
                            display_value(mean as f32, scale)
                        } else {
                            raw_value(mean as f32, scale)
                        };
                        let hue = linear_with_table(
                            linear,
                            affinity_rgb8(corner_composition(
                                corners.map(|at| (values[at], composition[at])),
                            )),
                        );
                        texels.push([
                            (hue.r * level) as f32,
                            (hue.g * level) as f32,
                            (hue.b * level) as f32,
                            1.0,
                        ]);
                    }
                }
            }
            (FieldMapping::Component(component), _) => {
                let glow = normalized_look == Look::Glow;
                let ch = usize::from(component.channel());
                let components = field.components();
                for row in 0..rows {
                    let up_row = if row == 0 { rows - 1 } else { row - 1 };
                    let up_base = up_row * columns;
                    let row_base = row * columns;
                    for column in 0..columns {
                        let left = if column == 0 { columns - 1 } else { column - 1 };
                        let corners = [
                            up_base + left,
                            up_base + column,
                            row_base + left,
                            row_base + column,
                        ];
                        let mean = (f64::from(values[corners[0]])
                            + f64::from(values[corners[1]])
                            + f64::from(values[corners[2]])
                            + f64::from(values[corners[3]]))
                            / 4.0;
                        let level = if glow {
                            display_value(mean as f32, scale)
                        } else {
                            raw_value(mean as f32, scale)
                        };
                        let hue = linear_with_table(
                            linear,
                            component_rgb8(corner_component(
                                corners.map(|at| (values[at], components[at][ch])),
                            )),
                        );
                        texels.push([
                            (hue.r * level) as f32,
                            (hue.g * level) as f32,
                            (hue.b * level) as f32,
                            1.0,
                        ]);
                    }
                }
            }
        }
        let refused = |quantity: &'static str, detail: String| CoreError::NumericalInvalid {
            quantity,
            detail,
        };
        let texture = Texture::from_premultiplied_linear(cells[0], cells[1], texels)
            .map_err(|error| refused("field layer texture", error.to_string()))?;
        let new_quad = if self.quad.is_none() || self.quad_key != Some(want_quad) {
            Some(self.viewport.periodic_quad(cells)?)
        } else {
            None
        };
        // Prepare any resized cache buffers before mutating `self` so an
        // allocation failure leaves the previous texture, quad and cache
        // intact. In steady state (`capacity() == len`) no allocation occurs.
        let mut new_values = if self.built_values.capacity() == values.len() {
            None
        } else {
            let mut buf = Vec::new();
            buf.try_reserve_exact(values.len()).map_err(|_| {
                CoreError::AllocationFailed {
                    buffer: "field layer values",
                    bytes: u64::try_from(values.len().saturating_mul(size_of::<f32>()))
                        .unwrap_or(u64::MAX),
                }
            })?;
            Some(buf)
        };
        let kept_comp = if composed { composition.len() } else { 0 };
        let mut new_composition = if self.built_composition.capacity() == kept_comp {
            None
        } else {
            let mut buf = Vec::new();
            buf.try_reserve_exact(kept_comp).map_err(|_| {
                CoreError::AllocationFailed {
                    buffer: "field layer compositions",
                    bytes: u64::try_from(kept_comp.saturating_mul(size_of::<[f32; 3]>()))
                        .unwrap_or(u64::MAX),
                }
            })?;
            Some(buf)
        };
        let kept_channel = if channel.is_some() { values.len() } else { 0 };
        let mut new_component = if self.built_component.capacity() == kept_channel {
            None
        } else {
            let mut buf = Vec::new();
            buf.try_reserve_exact(kept_channel).map_err(|_| {
                CoreError::AllocationFailed {
                    buffer: "field layer components",
                    bytes: u64::try_from(kept_channel).unwrap_or(u64::MAX),
                }
            })?;
            Some(buf)
        };

        // Commit atomically: nothing below can fail.
        self.texture = Some(texture);
        if let Some(quad) = new_quad {
            self.quad = Some(quad);
            self.quad_key = Some(want_quad);
        }
        self.scale = scale;
        if let Some(buf) = new_values.take() {
            self.built_values = buf;
        } else {
            self.built_values.clear();
        }
        self.built_values.extend_from_slice(values);
        if let Some(buf) = new_composition.take() {
            self.built_composition = buf;
        } else {
            self.built_composition.clear();
        }
        if composed {
            self.built_composition.extend_from_slice(composition);
        }
        if let Some(buf) = new_component.take() {
            self.built_component = buf;
        } else {
            self.built_component.clear();
        }
        if channel.is_some() {
            self.built_component
                .extend(field.components().iter().map(steps));
        }
        self.built = Some(key);
        Ok(())
    }

    /// The current display scale: the energy density shown at full
    /// brightness.
    #[must_use]
    pub const fn scale(&self) -> f64 {
        self.scale
    }

    /// The layer's texture once a field has been shown: one texel per
    /// field texel corner, rows top to bottom, premultiplied linear and
    /// opaque.
    #[must_use]
    pub const fn texture(&self) -> Option<&Texture> {
        self.texture.as_ref()
    }

    /// The layer's draw, once a field has been shown: its texture on its
    /// quad as an image, unlit, repeating, without a depth test, in painter
    /// order under the organisms.
    #[must_use]
    pub fn draw(&self) -> Option<ThreeDDraw<'_>> {
        let (Some(quad), Some(texture)) = (&self.quad, &self.texture) else {
            return None;
        };
        Some(ThreeDDraw::Surface(SurfaceDraw::image(quad, texture)))
    }
}
