//! The energy field under the organisms (plan §13.3, §9.5): the epoch's
//! bounded [`FieldTexture`] drawn as one smoothly shaded grid covering the
//! torus.
//!
//! Each texel's energy density maps through a fixed dark-to-bright palette.
//! The display scale follows the brightest texels with a slow decay, so the
//! picture neither flickers nor saturates as the sources wander. The grid's
//! vertices sit on texel corners, each coloured by the mean of the four
//! texels around it, wrapping across the seam, so the torus shows no seam.
//! FrankenManim interpolates the colours across each cell (Gouraud
//! shading), which both its CPU and its Metal 3D routes draw. The layer is
//! one scene object and one draw, whatever the field's size.

use flife_core::CoreError;
use flife_render::FieldTexture;
use fmn_core::color::{LinearRgba, Srgb};
use fmn_render::{SurfaceDraw, SurfaceMesh, SurfaceVertex, ThreeDDraw};

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

/// The field layer: one vertex-coloured grid.
#[derive(Debug)]
pub struct FieldLayer {
    viewport: Viewport,
    mesh: Option<SurfaceMesh>,
    scale: f64,
}

impl FieldLayer {
    /// A layer covering `viewport`'s world, empty until a field is shown.
    #[must_use]
    pub const fn new(viewport: &Viewport) -> Self {
        Self {
            viewport: *viewport,
            mesh: None,
            scale: 0.0,
        }
    }

    /// Map `field` through the palette into the layer's grid (the glow
    /// look).
    ///
    /// # Errors
    /// As [`FieldLayer::update_look`].
    pub fn update(&mut self, field: &FieldTexture) -> Result<(), CoreError> {
        self.update_look(field, Look::Glow)
    }

    /// Map `field` into the layer's grid: through the square-root palette
    /// for the glow look, as linear grey ([`raw_value`]) for raw.
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for a texture whose values do not fill
    /// its texels; [`CoreError::NumericalInvalid`] if FrankenManim refuses
    /// the grid. The previous grid is kept on error.
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

        let [width, height] = self.viewport.extent();
        let texel = |column: usize, row: usize| values[(row % rows) * columns + column % columns];
        let mut vertices = Vec::new();
        vertices
            .try_reserve_exact((columns + 1) * (rows + 1))
            .map_err(|_| allocation("field layer vertices", (columns + 1) * (rows + 1)))?;
        for row in 0..=rows {
            for column in 0..=columns {
                // The corner between texels (column - 1, row - 1) and
                // (column, row), wrapping.
                let (left, up) = (column + columns - 1, row + rows - 1);
                let mean = (f64::from(texel(left, up))
                    + f64::from(texel(column, up))
                    + f64::from(texel(left, row))
                    + f64::from(texel(column, row)))
                    / 4.0;
                let color = match look {
                    Look::Glow => {
                        let [r, g, b] = palette(display_value(mean as f32, self.scale));
                        Srgb::from_rgb8(r, g, b).to_linear(1.0)
                    }
                    Look::Raw => {
                        let grey = raw_value(mean as f32, self.scale);
                        LinearRgba {
                            r: grey,
                            g: grey,
                            b: grey,
                            a: 1.0,
                        }
                    }
                };
                let world = [
                    width * column as f64 / columns as f64,
                    height * row as f64 / rows as f64,
                ];
                vertices.push(SurfaceVertex::colored(
                    self.viewport.scene_point(world),
                    [0.0, 0.0, 1.0],
                    color,
                ));
            }
        }
        let mut indices = Vec::new();
        indices
            .try_reserve_exact(columns * rows * 6)
            .map_err(|_| allocation("field layer indices", columns * rows * 6))?;
        let stride = columns + 1;
        for row in 0..rows {
            for column in 0..columns {
                let at = |c: usize, r: usize| u32::try_from(r * stride + c).unwrap_or(u32::MAX);
                let (a, b, c, d) = (
                    at(column, row),
                    at(column + 1, row),
                    at(column + 1, row + 1),
                    at(column, row + 1),
                );
                indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
        let mesh =
            SurfaceMesh::new(vertices, indices).map_err(|error| CoreError::NumericalInvalid {
                quantity: "field layer grid",
                detail: error.to_string(),
            })?;
        self.mesh = Some(mesh);
        Ok(())
    }

    /// The current display scale: the energy density shown at full
    /// brightness.
    #[must_use]
    pub const fn scale(&self) -> f64 {
        self.scale
    }

    /// The layer's draw, once a field has been shown: unlit, in painter
    /// order under the organisms.
    #[must_use]
    pub fn draw(&self) -> Option<ThreeDDraw<'_>> {
        self.mesh.as_ref().map(|mesh| {
            ThreeDDraw::Surface(SurfaceDraw {
                shading: [0.0; 3],
                depth_test: false,
                ..SurfaceDraw::new(mesh)
            })
        })
    }
}

fn allocation(buffer: &'static str, count: usize) -> CoreError {
    CoreError::AllocationFailed {
        buffer,
        bytes: u64::try_from(count.saturating_mul(size_of::<SurfaceVertex>())).unwrap_or(u64::MAX),
    }
}
