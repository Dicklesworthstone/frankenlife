//! The energy field under the organisms (plan §13.3, §9.5): the epoch's
//! bounded [`FieldTexture`] drawn as one texture covering the torus
//! (FL-033).
//!
//! Each texel's energy density maps through a fixed dark-to-bright palette.
//! The display scale follows the brightest texels with a slow decay, so the
//! picture neither flickers nor saturates as the sources wander. The
//! layer's texture holds the colour of every texel corner, the mean of the
//! four texels around it, wrapping across the seam, and
//! [`Viewport::periodic_quad`] puts each on its corner; FrankenManim
//! interpolates them bilinearly, wrapping, so the torus shows no seam. Its
//! CPU and Metal 3D routes both sample textures. The layer is one scene
//! object and one draw of two triangles, whatever the field's size.
//!
//! The texture depends only on the field's values and texels, the display
//! scale and the look; the viewport moves only the quad. The layer keeps a
//! copy of the values its texture was built from, and a placement of
//! bitwise the same values, scale and look keeps the texture and rebuilds
//! only the quad (FL-033): a pan or zoom of one epoch.

use flife_core::CoreError;
use flife_render::FieldTexture;
use fmn_core::color::{LinearRgba, Srgb};
use fmn_render::{SurfaceDraw, SurfaceMesh, Texture, ThreeDDraw};

use crate::Viewport;
use crate::mapping::Look;

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

/// The field layer: a texture of the field's corner colours on one quad.
#[derive(Debug)]
pub struct FieldLayer {
    viewport: Viewport,
    texture: Option<Texture>,
    quad: Option<SurfaceMesh>,
    scale: f64,
    /// What the texture was built from, apart from the values: the look,
    /// the display scale's bits and the texels.
    built: Option<(Look, u64, [u32; 2])>,
    /// The field values the texture was built from, one `f32` a texel.
    built_values: Vec<f32>,
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
            scale: 0.0,
            built: None,
            built_values: Vec::new(),
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

    /// Map `field` into the layer's texture: through the square-root
    /// palette for the glow look, as linear grey ([`raw_value`]) for raw.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for a texture whose values do not fill
    /// its texels; [`CoreError::AllocationFailed`] when the texels cannot be
    /// reserved; [`CoreError::NumericalInvalid`] if FrankenManim refuses the
    /// texture or the quad. The previous texture and quad are kept on
    /// error.
    pub fn update_look(&mut self, field: &FieldTexture, look: Look) -> Result<(), CoreError> {
        let [columns, rows] = field.texels().map(|n| n as usize);
        let values = field.values();
        if values.len() != columns * rows || values.is_empty() {
            return Err(CoreError::LayoutMismatch {
                buffer: "field layer",
                detail: format!("{} values for {columns} x {rows} texels", values.len()),
            });
        }
        let brightest = values
            .iter()
            .copied()
            .filter(|value| value.is_finite())
            .fold(0.0_f32, f32::max);
        self.scale = f64::from(brightest).max(self.scale * SCALE_DECAY);

        self.place_scaled(field, look)
    }

    /// Use `viewport` from the next update or placement.
    pub const fn refit(&mut self, viewport: &Viewport) {
        self.viewport = *viewport;
    }

    /// Place `field` in the layer at the current viewport and display
    /// scale, which does not move: what a pan or zoom redraws. A texture
    /// built from bitwise the same values, at this scale and in this look,
    /// is kept, and only the quad is rebuilt.
    ///
    /// # Errors
    /// As [`FieldLayer::update_look`].
    pub fn place(&mut self, field: &FieldTexture, look: Look) -> Result<(), CoreError> {
        let [columns, rows] = field.texels().map(|n| n as usize);
        let values = field.values();
        if values.len() != columns * rows || values.is_empty() {
            return Err(CoreError::LayoutMismatch {
                buffer: "field layer",
                detail: format!("{} values for {columns} x {rows} texels", values.len()),
            });
        }
        self.place_scaled(field, look)
    }

    fn place_scaled(&mut self, field: &FieldTexture, look: Look) -> Result<(), CoreError> {
        let cells = field.texels();
        let [columns, rows] = cells.map(|n| n as usize);
        let values = field.values();
        let scale = self.scale;
        let key = (look, scale.to_bits(), cells);
        if self.built == Some(key) && same_bits(&self.built_values, values) {
            self.quad = Some(self.viewport.periodic_quad(cells)?);
            return Ok(());
        }
        // The copy is reserved before the texture is built, at exactly the
        // field's size, so the layer never holds two.
        self.built = None;
        self.built_values.clear();
        if self.built_values.capacity() != values.len() {
            self.built_values = Vec::new();
        }
        self.built_values
            .try_reserve_exact(values.len())
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "field layer values",
                bytes: u64::try_from(values.len().saturating_mul(size_of::<f32>()))
                    .unwrap_or(u64::MAX),
            })?;
        let texel = |column: usize, row: usize| values[(row % rows) * columns + column % columns];
        let mut texels = Vec::new();
        texels
            .try_reserve_exact(columns * rows)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "field layer texels",
                bytes: u64::try_from((columns * rows).saturating_mul(size_of::<[f32; 4]>()))
                    .unwrap_or(u64::MAX),
            })?;
        for row in 0..rows {
            for column in 0..columns {
                // Texel (column, row) is the corner between field texels
                // (column - 1, row - 1) and (column, row), wrapping.
                let (left, up) = (column + columns - 1, row + rows - 1);
                let mean = (f64::from(texel(left, up))
                    + f64::from(texel(column, up))
                    + f64::from(texel(left, row))
                    + f64::from(texel(column, row)))
                    / 4.0;
                let color = match look {
                    Look::Glow => {
                        let [r, g, b] = palette(display_value(mean as f32, scale));
                        Srgb::from_rgb8(r, g, b).to_linear(1.0)
                    }
                    Look::Raw => {
                        let grey = raw_value(mean as f32, scale);
                        LinearRgba {
                            r: grey,
                            g: grey,
                            b: grey,
                            a: 1.0,
                        }
                    }
                };
                // Opaque, so premultiplied is straight.
                texels.push([color.r as f32, color.g as f32, color.b as f32, 1.0]);
            }
        }
        let refused = |quantity: &'static str, detail: String| CoreError::NumericalInvalid {
            quantity,
            detail,
        };
        let texture = Texture::from_premultiplied_linear(cells[0], cells[1], texels)
            .map_err(|error| refused("field layer texture", error.to_string()))?;
        let quad = self.viewport.periodic_quad(cells)?;
        self.texture = Some(texture);
        self.quad = Some(quad);
        self.built_values.extend_from_slice(values);
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
