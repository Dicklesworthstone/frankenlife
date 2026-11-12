//! The field layer and the organisms' halos: what the view draws beyond one
//! dot per organism.

mod common;

use flife_manim::{
    COMPONENT_PALETTE, FIELD_COMPONENTS, FIELD_PALETTE, FieldComponent, FieldLayer, FieldMapping,
    HALO_ALPHA, Look, ParticleCloud, Particles, SCALE_DECAY, Style, Viewport, WorldView,
    affinity_rgb8, component_rgb8, corner_component, corner_composition, display_value,
    halo_factor, lineage_color, palette, raw_value,
};
use flife_render::{AFFINITY_AXES, FieldTexture};
use fmn_core::color::{LinearRgba, Srgb};
use fmn_render::{SurfaceMaterial, TextureWrap, ThreeDDraw};

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn fit(extent: [f64; 2], drawable: [u32; 2]) -> Viewport {
    Viewport::fit(extent, drawable).expect("a valid viewport")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn texture(cells: [u32; 2], plane: &[f32]) -> FieldTexture {
    FieldTexture::from_plane(cells, [1.0, 1.0], plane).expect("a texture")
}

#[test]
fn the_palette_runs_dark_to_bright_and_the_scale_is_a_square_root() {
    assert_eq!(palette(0.0), FIELD_PALETTE[0]);
    assert_eq!(palette(1.0), FIELD_PALETTE[FIELD_PALETTE.len() - 1]);
    assert_eq!(palette(f64::NAN), FIELD_PALETTE[0]);
    let luma = |[r, g, b]: [u8; 3]| u32::from(r) * 3 + u32::from(g) * 6 + u32::from(b);
    let mut previous = 0;
    for step in 0..=100 {
        let current = luma(palette(f64::from(step) / 100.0));
        assert!(current >= previous, "brightness falls at {step}%");
        previous = current;
    }
    assert_eq!(display_value(0.25, 1.0), 0.5);
    assert_eq!(display_value(4.0, 1.0), 1.0);
    assert_eq!(display_value(-1.0, 1.0), 0.0);
    assert_eq!(display_value(f32::NAN, 1.0), 0.0);
    assert_eq!(display_value(1.0, 0.0), 0.0);
}

/// FL-033: the field is one texture of its corner colours on the
/// viewport's periodic quad, drawn as an image: unlit, repeating, under the
/// organisms. Texel `(column, row)` is the palette colour of the mean of
/// the four field texels around that corner, wrapping, so the seam joins
/// its two sides' colours.
#[test]
fn the_field_texture_covers_the_torus_without_a_seam() {
    let viewport = fit([8.0, 4.0], [160, 80]);
    let mut layer = FieldLayer::new(&viewport);
    assert!(layer.draw().is_none(), "nothing before a field is shown");
    assert!(layer.texture().is_none());
    // A ramp along x: the seam joins its brightest and darkest columns.
    let plane: Vec<f32> = (0..32).map(|cell| (cell % 8) as f32).collect();
    layer.update(&texture([8, 4], &plane)).expect("update");
    assert_eq!(layer.scale(), 7.0);
    let Some(ThreeDDraw::Surface(draw)) = layer.draw() else {
        panic!("the layer draws one surface");
    };
    let SurfaceMaterial::Texture(material) = draw.material else {
        panic!("the field is textured");
    };
    assert!(material.dark.is_none() && !material.discard_transparent);
    assert_eq!(
        (material.sampler.wrap_u, material.sampler.wrap_v),
        (TextureWrap::Repeat, TextureWrap::Repeat),
        "the texture wraps with the torus"
    );
    assert_eq!(draw.shading, [0.0; 3], "unlit");
    assert!(!draw.depth_test, "the field stays under the organisms");
    let field = layer.texture().expect("the layer's texture");
    assert!(std::ptr::eq(material.light, field));
    assert_eq!(
        (field.width(), field.height()),
        (8, 4),
        "a texel per corner"
    );
    assert_eq!(
        draw.mesh,
        &viewport.periodic_quad([8, 4]).expect("a quad"),
        "on the viewport's periodic quad"
    );

    let shade = |mean: f64| {
        let [r, g, b] = palette(display_value(mean as f32, 7.0));
        let linear = fmn_core::color::Srgb::from_rgb8(r, g, b).to_linear(1.0);
        [linear.r, linear.g, linear.b].map(|channel| f64::from(channel as f32))
    };
    for row in 0..4 {
        for column in 0..8 {
            // Columns column - 1 and column, wrapping, in every row.
            let mean = f64::from((column + 7) % 8 + column) / 2.0;
            let texel = field.texel(column, row).expect("a texel");
            assert_eq!(
                [texel.r, texel.g, texel.b],
                shade(mean),
                "corner ({column}, {row})"
            );
            assert_eq!(texel.a, 1.0, "opaque");
        }
    }
    // The seam corner averages column 7 (bright) with column 0 (dark); the
    // next corner averages columns 0 and 1.
    assert_eq!(shade((7.0 + 0.0) / 2.0), shade(3.5));
    assert_ne!(field.texel(0, 1), field.texel(1, 1));

    // A pan moves the quad's texture coordinates, not the texture.
    let panned = viewport.with_view([1.0, 2.0], 2.0).expect("a view");
    layer.refit(&panned);
    layer
        .place(&texture([8, 4], &plane), Look::Glow)
        .expect("placed");
    let Some(ThreeDDraw::Surface(draw)) = layer.draw() else {
        panic!("the layer draws one surface");
    };
    assert_eq!(draw.mesh, &panned.periodic_quad([8, 4]).expect("a quad"));
    assert_eq!(layer.scale(), 7.0, "a placement keeps the scale");
}

#[test]
fn the_display_scale_follows_the_field_down_slowly() {
    let viewport = fit([4.0, 4.0], [40, 40]);
    let mut layer = FieldLayer::new(&viewport);
    layer.update(&texture([4, 4], &[10.0; 16])).expect("bright");
    assert_eq!(layer.scale(), 10.0);
    layer.update(&texture([4, 4], &[1.0; 16])).expect("dim");
    assert_eq!(layer.scale(), 10.0 * SCALE_DECAY);
    layer
        .update(&texture([4, 4], &[20.0; 16]))
        .expect("brighter");
    assert_eq!(layer.scale(), 20.0);
}

/// The colours a field layer draws for `plane` on `cells` at `scale` in
/// `look`, rows top to bottom: each corner's colour from the mean of the
/// four field texels around it, wrapping.
fn corner_colours(cells: [u32; 2], plane: &[f32], scale: f64, look: Look) -> Vec<[f64; 3]> {
    let [columns, rows] = cells.map(|n| n as usize);
    let at =
        |column: usize, row: usize| f64::from(plane[(row % rows) * columns + column % columns]);
    let mut colours = Vec::new();
    for row in 0..rows {
        for column in 0..columns {
            let (left, up) = (column + columns - 1, row + rows - 1);
            let mean = (at(left, up) + at(column, up) + at(left, row) + at(column, row)) / 4.0;
            let linear = match look {
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
            colours.push([linear.r, linear.g, linear.b].map(|channel| f64::from(channel as f32)));
        }
    }
    colours
}

/// The layer's texture as colours, rows top to bottom, and its size.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn drawn(layer: &FieldLayer) -> (Vec<[f64; 3]>, [u32; 2]) {
    let texture = layer.texture().expect("a texture");
    let colours = (0..texture.height())
        .flat_map(|row| {
            (0..texture.width()).map(move |column| {
                let texel = texture.texel(column, row).expect("a texel");
                [texel.r, texel.g, texel.b]
            })
        })
        .collect();
    (colours, [texture.width(), texture.height()])
}

/// FL-033: a placement keeps the layer's texture only when it would build
/// the same one, from bitwise the same values on the same texels at the same
/// display scale in the same look. Whatever the layer built before, every
/// placement draws the colours of what it was given, on the viewport's quad.
#[test]
fn a_placement_keeps_the_texture_only_for_the_same_values_scale_and_look() {
    let viewport = fit([8.0, 4.0], [160, 80]);
    let mut layer = FieldLayer::new(&viewport);
    let plane: Vec<f32> = (0..32).map(|cell| (cell % 8) as f32).collect();
    layer.update(&texture([8, 4], &plane)).expect("update");
    assert_eq!(layer.scale(), 7.0);
    let glow = corner_colours([8, 4], &plane, 7.0, Look::Glow);
    assert_eq!(drawn(&layer), (glow.clone(), [8, 4]));

    // The same values from another texture, panned: the colours do not
    // move, the quad does.
    let panned = viewport.with_view([1.0, 2.0], 2.0).expect("a view");
    layer.refit(&panned);
    layer
        .place(&texture([8, 4], &plane), Look::Glow)
        .expect("placed");
    assert_eq!(drawn(&layer), (glow.clone(), [8, 4]));
    let Some(ThreeDDraw::Surface(draw)) = layer.draw() else {
        panic!("the layer draws one surface");
    };
    assert_eq!(draw.mesh, &panned.periodic_quad([8, 4]).expect("a quad"));

    // Other values: one texel brighter.
    let mut other = plane.clone();
    other[9] = 6.0;
    let other_glow = corner_colours([8, 4], &other, 7.0, Look::Glow);
    assert_ne!(other_glow, glow, "the brighter texel shows");
    layer
        .place(&texture([8, 4], &other), Look::Glow)
        .expect("placed");
    assert_eq!(drawn(&layer), (other_glow.clone(), [8, 4]));

    // The other look.
    let other_raw = corner_colours([8, 4], &other, 7.0, Look::Raw);
    assert_ne!(other_raw, other_glow);
    layer
        .place(&texture([8, 4], &other), Look::Raw)
        .expect("placed");
    assert_eq!(drawn(&layer), (other_raw, [8, 4]));

    // The same values on other texels.
    layer
        .place(&texture([4, 8], &other), Look::Raw)
        .expect("placed");
    assert_eq!(
        drawn(&layer),
        (corner_colours([4, 8], &other, 7.0, Look::Raw), [4, 8])
    );

    // The same values at another scale: a paused world shown again while the
    // display scale falls. Raw is linear, so every lit texel shows the fall.
    layer
        .update_look(&texture([8, 4], &[20.0; 32]), Look::Raw)
        .expect("bright");
    for _ in 0..2 {
        layer
            .update_look(&texture([8, 4], &plane), Look::Raw)
            .expect("dim");
    }
    let scale = 20.0 * SCALE_DECAY * SCALE_DECAY;
    assert_eq!(layer.scale(), scale);
    let fallen = corner_colours([8, 4], &plane, scale, Look::Raw);
    assert_ne!(
        fallen,
        corner_colours([8, 4], &plane, 20.0 * SCALE_DECAY, Look::Raw)
    );
    assert_eq!(drawn(&layer), (fallen, [8, 4]));

    // A larger field, one texel changed near its end.
    let ramp: Vec<f32> = (0..128).map(|cell| (cell % 16) as f32).collect();
    layer.update(&texture([16, 8], &ramp)).expect("update");
    let scale = layer.scale();
    let mut late = ramp.clone();
    late[100] = 0.5;
    let late_glow = corner_colours([16, 8], &late, scale, Look::Glow);
    assert_ne!(late_glow, corner_colours([16, 8], &ramp, scale, Look::Glow));
    layer
        .place(&texture([16, 8], &late), Look::Glow)
        .expect("placed");
    assert_eq!(drawn(&layer), (late_glow, [16, 8]));
}

/// A field's 33 planes on `cells`: `cell(x, y)` is its energy and, if any,
/// the projection axis (0 to 2) its composition lies along at full
/// strength, with a sign: `m = ±e h_k / √32`, row `AFFINITY_AXES[k]` of the
/// Hadamard matrix.
fn planes(cells: [u32; 2], cell: impl Fn(u32, u32) -> (f32, Option<(usize, f32)>)) -> Vec<f32> {
    let count = (cells[0] * cells[1]) as usize;
    let at = |i: usize| cell(i as u32 % cells[0], i as u32 / cells[0]);
    let mut planes: Vec<f32> = (0..count).map(|i| at(i).0).collect();
    for channel in 0..32_usize {
        planes.extend((0..count).map(|i| match at(i) {
            (energy, Some((axis, sign))) => {
                let even = (AFFINITY_AXES[axis] & channel)
                    .count_ones()
                    .is_multiple_of(2);
                let h = if even { 1.0 } else { -1.0 };
                sign * energy * h / 32.0_f32.sqrt()
            }
            (_, None) => 0.0,
        }));
    }
    planes
}

/// The colours the composition mapping draws for `field` at `scale` in
/// `look`, rows top to bottom, computed here: each corner the affinity
/// colour of the density-weighted mean of the four texels' compositions
/// around it, wrapping, in linear light times what the mean density shows.
fn composed_colours(field: &FieldTexture, scale: f64, look: Look) -> Vec<[f64; 3]> {
    let [columns, rows] = field.texels().map(|n| n as usize);
    let index = |column: usize, row: usize| (row % rows) * columns + column % columns;
    let mut colours = Vec::new();
    for row in 0..rows {
        for column in 0..columns {
            let (left, up) = (column + columns - 1, row + rows - 1);
            let corners = [(left, up), (column, up), (left, row), (column, row)]
                .map(|(column, row)| index(column, row));
            let densities = corners.map(|at| f64::from(field.values()[at]));
            let mean = densities.iter().sum::<f64>() / 4.0;
            let weight: f64 = densities.iter().sum();
            let mut q = [0.0_f64; 3];
            for (at, density) in corners.into_iter().zip(densities) {
                for (total, component) in q.iter_mut().zip(field.composition()[at]) {
                    *total += density * f64::from(component);
                }
            }
            let q = if weight > 0.0 {
                q.map(|total| (total / weight) as f32)
            } else {
                [0.0; 3]
            };
            let level = match look {
                Look::Glow => display_value(mean as f32, scale),
                Look::Raw => raw_value(mean as f32, scale),
            };
            let [r, g, b] = affinity_rgb8(q);
            let hue = Srgb::from_rgb8(r, g, b).to_linear(1.0);
            colours.push(
                [hue.r * level, hue.g * level, hue.b * level]
                    .map(|channel| f64::from(channel as f32)),
            );
        }
    }
    colours
}

/// The colours channel `channel`'s component mapping draws for `field` at
/// `scale` in `look`, rows top to bottom, computed here: each corner the
/// [`component_rgb8`] colour of the density-weighted mean of the four
/// texels' steps around it, wrapping, over 127, in linear light times what
/// the mean density shows.
fn component_colours(
    field: &FieldTexture,
    channel: usize,
    scale: f64,
    look: Look,
) -> Vec<[f64; 3]> {
    let [columns, rows] = field.texels().map(|n| n as usize);
    let index = |column: usize, row: usize| (row % rows) * columns + column % columns;
    let mut colours = Vec::new();
    for row in 0..rows {
        for column in 0..columns {
            let (left, up) = (column + columns - 1, row + rows - 1);
            let corners = [(left, up), (column, up), (left, row), (column, row)]
                .map(|(column, row)| index(column, row));
            let densities = corners.map(|at| f64::from(field.values()[at]));
            let mean = densities.iter().sum::<f64>() / 4.0;
            let weight: f64 = densities.iter().filter(|d| **d > 0.0).sum();
            let total: f64 = corners
                .into_iter()
                .zip(densities)
                .filter(|(_, density)| *density > 0.0)
                .map(|(at, density)| density * f64::from(field.components()[at][channel]))
                .sum();
            let value = if weight > 0.0 {
                total / weight / 127.0
            } else {
                0.0
            };
            let level = match look {
                Look::Glow => display_value(mean as f32, scale),
                Look::Raw => raw_value(mean as f32, scale),
            };
            let [r, g, b] = component_rgb8(value);
            let hue = Srgb::from_rgb8(r, g, b).to_linear(1.0);
            colours.push(
                [hue.r * level, hue.g * level, hue.b * level]
                    .map(|channel| f64::from(channel as f32)),
            );
        }
    }
    colours
}

/// A field's 33 planes on `cells`: `cell(x, y)` is its energy and its
/// moment channels' values, each `(channel, value)`, the rest zero.
fn channel_planes(
    cells: [u32; 2],
    cell: impl Fn(u32, u32) -> (f32, Vec<(usize, f32)>),
) -> Vec<f32> {
    let count = (cells[0] * cells[1]) as usize;
    let at = |i: usize| cell(i as u32 % cells[0], i as u32 / cells[0]);
    let mut planes: Vec<f32> = (0..count).map(|i| at(i).0).collect();
    for channel in 0..32_usize {
        planes.extend((0..count).map(|i| {
            at(i)
                .1
                .iter()
                .find(|(c, _)| *c == channel)
                .map_or(0.0, |(_, value)| *value)
        }));
    }
    planes
}

/// The component palette: its stops at −1, −½, 0, ½ and 1, a byte-rounded
/// sRGB mix between two stops, clamped beyond ±1, and 0 for what is not
/// finite.
#[test]
fn the_component_palette_runs_blue_through_pale_to_red() {
    for (value, stop) in [-1.0, -0.5, 0.0, 0.5, 1.0]
        .into_iter()
        .zip(COMPONENT_PALETTE)
    {
        assert_eq!(component_rgb8(value), stop, "{value}");
    }
    let between = |a: [u8; 3], b: [u8; 3], f: f64| {
        [0, 1, 2].map(|i| (f64::from(a[i]) + (f64::from(b[i]) - f64::from(a[i])) * f).round() as u8)
    };
    assert_eq!(
        component_rgb8(-0.75),
        between(COMPONENT_PALETTE[0], COMPONENT_PALETTE[1], 0.5)
    );
    assert_eq!(
        component_rgb8(0.6),
        between(COMPONENT_PALETTE[3], COMPONENT_PALETTE[4], 0.2)
    );
    assert_eq!(component_rgb8(-3.0), COMPONENT_PALETTE[0]);
    assert_eq!(component_rgb8(2.0), COMPONENT_PALETTE[4]);
    assert_eq!(component_rgb8(f64::NAN), COMPONENT_PALETTE[2]);
    // Blue at -1, red at +1: the ends are told apart by their dominant
    // channel, and the middle is pale.
    let [low, high] = [COMPONENT_PALETTE[0], COMPONENT_PALETTE[4]];
    assert!(low[2] > low[0] && high[0] > high[2]);
    assert!(COMPONENT_PALETTE[2].iter().all(|&c| c > 0xB0));
}

/// Plan §13.4's chosen components: in channel 3's mapping a corner's hue is
/// its texels' mean channel 3 per unit energy, weighted by their densities,
/// through the component palette, and its brightness is what the energy
/// shows: a region at +1 is red, one at −1 blue, one with energy but none
/// of the channel pale, one with no energy black; a bright texel pulls a
/// shared corner toward its value. Another channel draws its own values.
/// A placement keeps the texture only for the same channel's values, so a
/// field whose channel changed, with the same energy, is drawn anew.
#[test]
fn a_component_mapping_colours_one_channel_and_lights_it_by_energy() {
    let viewport = fit([8.0, 4.0], [160, 80]);
    let mut layer = FieldLayer::new(&viewport);
    let three = FieldMapping::Component(FieldComponent::new(3).expect("a channel"));
    let five = FieldMapping::Component(FieldComponent::new(5).expect("a channel"));
    // Left half: 4 energy wholly along channel 3; right half: 1 energy
    // wholly against it; a 2 x 2 hole with no energy at columns 5-6, rows
    // 1-2; cell (3, 3) holds energy in channel 5 alone.
    let cell = |x: u32, y: u32| -> (f32, Vec<(usize, f32)>) {
        if (5..=6).contains(&x) && (1..=2).contains(&y) {
            (0.0, Vec::new())
        } else if x == 3 && y == 3 {
            (4.0, vec![(5, 4.0)])
        } else if x < 4 {
            (4.0, vec![(3, 4.0)])
        } else {
            (1.0, vec![(3, -1.0)])
        }
    };
    let field = FieldTexture::from_planes([8, 4], [1.0, 1.0], &channel_planes([8, 4], cell))
        .expect("a texture");
    layer
        .update_mapped(&field, Look::Glow, three)
        .expect("update");
    assert_eq!(layer.scale(), 4.0);
    let glow = component_colours(&field, 3, 4.0, Look::Glow);
    assert_eq!(drawn(&layer), (glow.clone(), [8, 4]));
    let linear = |[r, g, b]: [u8; 3], level: f64| {
        let hue = Srgb::from_rgb8(r, g, b).to_linear(1.0);
        [hue.r * level, hue.g * level, hue.b * level].map(|channel| f64::from(channel as f32))
    };
    let at = |colours: &[[f64; 3]], column: usize, row: usize| colours[row * 8 + column];
    // Inside the left half: red at full brightness.
    assert_eq!(at(&glow, 2, 2), linear(COMPONENT_PALETTE[4], 1.0));
    // The middle of the hole: black.
    assert_eq!(at(&glow, 6, 2), [0.0; 3]);
    // Inside the right half, away from the hole: blue at half brightness
    // (the square root of a quarter of the scale).
    assert_eq!(at(&glow, 5, 0), linear(COMPONENT_PALETTE[0], 0.5));
    // Corner (4, 1) joins two left texels (density 4, +1) and two right
    // ones (density 1, -1): (8 - 2) / 10 = 0.6, not 0.
    let joined = corner_component([(4.0, 127), (1.0, -127), (4.0, 127), (1.0, -127)]);
    assert!((joined - 0.6).abs() < 1e-12, "{joined}");
    assert_eq!(
        corner_component([(0.0, 127); 4]),
        0.0,
        "no density, no value"
    );
    assert_eq!(
        corner_component([(f32::NAN, 127), (1.0, -127), (0.0, 127), (-1.0, 127)]),
        -1.0
    );
    // Corner (4, 4) is corner (4, 0) wrapped: cell (3, 3) has no channel 3,
    // so its pale is mixed in.
    assert_ne!(at(&glow, 4, 0), at(&glow, 4, 2));

    // Channel 5 is lit only around cell (3, 3); elsewhere its value is 0.
    layer
        .place_mapped(&field, Look::Glow, five)
        .expect("placed");
    let fifth = component_colours(&field, 5, 4.0, Look::Glow);
    assert_ne!(fifth, glow);
    assert_eq!(drawn(&layer), (fifth.clone(), [8, 4]));
    assert_eq!(at(&fifth, 1, 1), linear(COMPONENT_PALETTE[2], 1.0));

    // Raw is linear in the density.
    layer
        .place_mapped(&field, Look::Raw, three)
        .expect("placed");
    let raw = component_colours(&field, 3, 4.0, Look::Raw);
    assert_ne!(raw, glow);
    assert_eq!(drawn(&layer), (raw.clone(), [8, 4]));

    // The same energy with channel 3 reversed: drawn anew, not kept.
    let reversed = FieldTexture::from_planes(
        [8, 4],
        [1.0, 1.0],
        &channel_planes([8, 4], |x, y| {
            let (energy, moments) = cell(x, y);
            (
                energy,
                moments
                    .into_iter()
                    .map(|(c, value)| if c == 3 { (c, -value) } else { (c, value) })
                    .collect(),
            )
        }),
    )
    .expect("a texture");
    assert_eq!(reversed.values(), field.values());
    layer
        .place_mapped(&reversed, Look::Raw, three)
        .expect("placed");
    let reversed_raw = component_colours(&reversed, 3, 4.0, Look::Raw);
    assert_ne!(reversed_raw, raw);
    assert_eq!(drawn(&layer), (reversed_raw, [8, 4]));
    // Channel 5's values are unchanged there, and drawn as before.
    layer
        .place_mapped(&reversed, Look::Glow, five)
        .expect("placed");
    assert_eq!(drawn(&layer), (fifth, [8, 4]));

    // Energy alone has no component: every lit corner is the pale middle.
    let alone = texture([8, 4], field.values());
    let mut pale = FieldLayer::new(&viewport);
    pale.update_mapped(&alone, Look::Glow, three)
        .expect("update");
    assert_eq!(
        drawn(&pale).0,
        component_colours(&alone, 3, 4.0, Look::Glow)
    );
    assert_eq!(at(&drawn(&pale).0, 2, 2), linear(COMPONENT_PALETTE[2], 1.0));
}

/// Plan §13.4's "Local field": in the composition mapping a corner's hue is
/// its texels' mean composition by the affinity mapping's colours, weighted
/// by their densities, and its brightness is what the energy shows there:
/// a region with no energy is black, a region with no composition is grey,
/// and a bright texel pulls a shared corner toward its chemistry. A
/// placement keeps the texture only for the same compositions and the same
/// mapping; the energy mapping is unchanged.
#[test]
fn the_composition_mapping_colours_chemistry_and_lights_it_by_energy() {
    let viewport = fit([8.0, 4.0], [160, 80]);
    let mut layer = FieldLayer::new(&viewport);
    // Left half: 4 energy along axis 0; right half: 1 energy along axis 1;
    // a 2 x 2 hole with no energy at columns 5-6, rows 1-2; column 3 of row
    // 3 holds energy but no composition.
    let cell = |x: u32, y: u32| {
        if (5..=6).contains(&x) && (1..=2).contains(&y) {
            (0.0, None)
        } else if x == 3 && y == 3 {
            (4.0, None)
        } else if x < 4 {
            (4.0, Some((0, 1.0)))
        } else {
            (1.0, Some((1, 1.0)))
        }
    };
    let field =
        FieldTexture::from_planes([8, 4], [1.0, 1.0], &planes([8, 4], cell)).expect("a texture");
    layer
        .update_mapped(&field, Look::Glow, FieldMapping::Composition)
        .expect("update");
    assert_eq!(layer.scale(), 4.0);
    let glow = composed_colours(&field, 4.0, Look::Glow);
    assert_eq!(drawn(&layer), (glow.clone(), [8, 4]));

    // Corner (2, 2) is inside the left half: the affinity colour of axis 0
    // at full brightness.
    let [r, g, b] = affinity_rgb8([1.0, 0.0, 0.0]);
    let full = Srgb::from_rgb8(r, g, b).to_linear(1.0);
    assert_eq!(
        glow[2 * 8 + 2],
        [full.r, full.g, full.b].map(|channel| f64::from(channel as f32))
    );
    // Corner (6, 2) is the middle of the hole: black.
    assert_eq!(glow[2 * 8 + 6], [0.0; 3]);
    // Corner (4, 1) joins two left texels (density 4, axis 0) and two
    // right ones (density 1, axis 1): weighted 0.8 and 0.2, not half each.
    let joined = corner_composition([
        (4.0, [1.0, 0.0, 0.0]),
        (1.0, [0.0, 1.0, 0.0]),
        (4.0, [1.0, 0.0, 0.0]),
        (1.0, [0.0, 1.0, 0.0]),
    ]);
    assert!(
        (joined[0] - 0.8).abs() < 1e-6 && (joined[1] - 0.2).abs() < 1e-6,
        "{joined:?}"
    );
    let at = |colours: &[[f64; 3]], column: usize, row: usize| colours[row * 8 + column];
    assert_eq!(
        corner_composition([(0.0, [1.0, 0.0, 0.0]); 4]),
        [0.0; 3],
        "no density, no composition"
    );
    let (q4, q5) = (field.composition()[4], field.composition()[5]);
    assert!(q4[1] > 0.99 && q5[1] > 0.99, "{q4:?} {q5:?}");
    assert_ne!(at(&glow, 4, 1), at(&glow, 2, 1));

    // Energy alone has no composition: every lit corner is grey.
    let alone = texture([8, 4], field.values());
    let mut grey = FieldLayer::new(&viewport);
    grey.update_mapped(&alone, Look::Glow, FieldMapping::Composition)
        .expect("update");
    for [r, g, b] in drawn(&grey).0 {
        assert!(r == g && g == b, "{r} {g} {b}");
    }

    // Raw is linear in the density.
    layer
        .place_mapped(&field, Look::Raw, FieldMapping::Composition)
        .expect("placed");
    let raw = composed_colours(&field, 4.0, Look::Raw);
    assert_ne!(raw, glow);
    assert_eq!(drawn(&layer), (raw, [8, 4]));

    // The same energy with other chemistry: the right half against axis 2.
    let swapped = FieldTexture::from_planes(
        [8, 4],
        [1.0, 1.0],
        &planes([8, 4], |x, y| match cell(x, y) {
            (energy, Some((1, _))) => (energy, Some((2, -1.0))),
            other => other,
        }),
    )
    .expect("a texture");
    assert_eq!(swapped.values(), field.values());
    layer
        .place_mapped(&swapped, Look::Raw, FieldMapping::Composition)
        .expect("placed");
    let swapped_raw = composed_colours(&swapped, 4.0, Look::Raw);
    assert_ne!(swapped_raw, composed_colours(&field, 4.0, Look::Raw));
    assert_eq!(drawn(&layer), (swapped_raw, [8, 4]));

    // The same values in the energy mapping: its own colours.
    layer
        .place_mapped(&swapped, Look::Raw, FieldMapping::Energy)
        .expect("placed");
    assert_eq!(
        drawn(&layer),
        (
            corner_colours([8, 4], swapped.values(), 4.0, Look::Raw),
            [8, 4]
        )
    );
    layer.place(&swapped, Look::Glow).expect("placed");
    assert_eq!(
        drawn(&layer),
        (
            corner_colours([8, 4], swapped.values(), 4.0, Look::Glow),
            [8, 4]
        )
    );
}

/// The view draws the field in the style's mapping, whenever the style
/// draws a field: not under raw density, which is opaque.
#[test]
fn the_view_draws_the_field_in_the_styles_mapping() {
    let config = common::small();
    let mut world = common::found(&config);
    common::advance(&mut world, 24);
    let lease = common::epoch(&world);
    assert!(
        lease
            .field()
            .composition()
            .iter()
            .any(|q| q.iter().any(|c| c.abs() > 0.01)),
        "the small world's field holds chemistry"
    );
    let mut view = WorldView::for_epoch(&lease, [96, 96], [0, 0, 0]).expect("view");
    let mut frames = Vec::new();
    for field_mapping in FieldMapping::ALL {
        view.set_style(Style {
            field_mapping,
            ..Style::default()
        });
        view.show(&lease).expect("show");
        let (colours, _) = drawn(view.field());
        let scale = view.field().scale();
        let expected = match field_mapping {
            FieldMapping::Energy => corner_colours(
                lease.field().texels(),
                lease.field().values(),
                scale,
                Look::Glow,
            ),
            FieldMapping::Composition => composed_colours(lease.field(), scale, Look::Glow),
            FieldMapping::Component(component) => component_colours(
                lease.field(),
                usize::from(component.channel()),
                scale,
                Look::Glow,
            ),
        };
        assert_eq!(colours, expected, "{field_mapping}");
        frames.push(view.render_rgba8(2).expect("an offline frame"));
    }
    assert_ne!(frames[0], frames[1], "the mapping changes the frame");
    assert_eq!(frames.len(), 34);
    assert!(
        frames[2..].iter().all(|frame| frame != &frames[0]),
        "every component's frame differs from the energy's"
    );
    assert!(
        lease
            .field()
            .components()
            .iter()
            .any(|texel| texel.iter().any(|&steps| steps.unsigned_abs() > 10)),
        "the small world's field holds components"
    );
    assert!(
        frames[2..].windows(2).any(|pair| pair[0] != pair[1]),
        "the channels draw different frames"
    );

    for (particles, look, drawn_field) in [
        (Particles::Points, Look::Glow, true),
        (Particles::Points, Look::Raw, true),
        (Particles::Density, Look::Glow, true),
        (Particles::Density, Look::Raw, false),
    ] {
        for field in [true, false] {
            let style = Style {
                particles,
                look,
                field,
                field_mapping: FieldMapping::Composition,
                ..Style::default()
            };
            assert_eq!(style.field_drawn(), field && drawn_field, "{style:?}");
            view.set_style(style);
            view.show(&lease).expect("show");
            let field_draw = view.field().draw().expect("a field draw");
            let drawn_now = view.draws().expect("draws").iter().any(|draw| {
                matches!(
                    (draw, &field_draw),
                    (ThreeDDraw::Surface(a), ThreeDDraw::Surface(b)) if std::ptr::eq(a.mesh, b.mesh)
                )
            });
            assert_eq!(drawn_now, style.field_drawn(), "{style:?}");
        }
    }
}

/// The field mappings' names, as flags and records spell them.
#[test]
fn field_mappings_are_named_and_parsed() {
    assert_eq!(FieldMapping::default(), FieldMapping::Energy);
    assert_eq!(Style::default().field_mapping, FieldMapping::Energy);
    assert_eq!(FieldMapping::ALL.len(), 2 + usize::from(FIELD_COMPONENTS));
    assert_eq!(FIELD_COMPONENTS, 32);
    for (at, mapping) in FieldMapping::ALL.into_iter().enumerate() {
        assert_eq!(mapping.name().parse::<FieldMapping>(), Ok(mapping));
        assert_eq!(mapping.to_string(), mapping.name());
        // C cycles energy, composition and the chosen component.
        let chosen = mapping.component().unwrap_or_default();
        assert_eq!(mapping.next(chosen).next(chosen).next(chosen), mapping);
        assert_ne!(mapping.next(chosen), mapping);
        assert_eq!(
            mapping
                .component()
                .map(|component| usize::from(component.channel()) + 2),
            (at >= 2).then_some(at)
        );
    }
    assert_eq!(FieldMapping::ALL[0], FieldMapping::Energy);
    assert_eq!(FieldMapping::ALL[1], FieldMapping::Composition);
    let seven = FieldComponent::new(7).expect("a channel");
    assert_eq!(seven.channel(), 7);
    assert_eq!(
        "component-7".parse::<FieldMapping>(),
        Ok(FieldMapping::Component(seven))
    );
    assert_eq!(FieldMapping::Component(seven).name(), "component-7");
    assert_eq!(
        FieldMapping::Composition.next(seven),
        FieldMapping::Component(seven)
    );
    assert_eq!(
        FieldMapping::Component(seven).next(seven),
        FieldMapping::Energy
    );
    assert_eq!(FieldComponent::new(32), None);
    let last = FieldComponent::new(31).expect("the last channel");
    let first = FieldComponent::default();
    assert_eq!(first.channel(), 0);
    assert_eq!((last.next(), first.previous()), (first, last));
    assert_eq!((seven.next().channel(), seven.previous().channel()), (8, 6));
    let refused = |text: &str| {
        Err(format!(
            "`{text}` is not a field mapping: energy, composition or component-C for a channel C \
             from 0 to 31"
        ))
    };
    for text in [
        "chemistry",
        "component-32",
        "component-07",
        "component-",
        "component",
    ] {
        assert_eq!(text.parse::<FieldMapping>(), refused(text), "{text}");
    }
}

#[test]
fn every_dot_has_one_halo_sized_by_its_energy_in_the_same_order() {
    let config = common::small();
    let mut world = common::found(&config);
    common::advance(&mut world, 12);
    let lease = common::epoch(&world);
    let viewport = fit(lease.extent(), [320, 320]);
    let mut cloud = ParticleCloud::for_epoch(&lease).expect("cloud");
    cloud.update(&lease, &viewport).expect("update");
    let (dots, halos) = (cloud.draws(), cloud.halos());
    assert_eq!(halos.len(), dots.len());
    for (row, (dot, halo)) in dots.iter().zip(halos).enumerate() {
        let (ThreeDDraw::TrueDot(dot), ThreeDDraw::TrueDot(halo)) = (dot, halo) else {
            panic!("row {row} is not two dots");
        };
        assert_eq!(halo.center, dot.center, "row {row}");
        assert_eq!(halo.radius, dot.radius * halo_factor(lease.energies()[row]));
        assert!(halo.glow_factor > 0.0 && dot.glow_factor == 0.0);
        let lineage = lineage_color(lease.lineages()[row]);
        assert_eq!(
            [halo.color.r, halo.color.g, halo.color.b, halo.color.a],
            [lineage.r, lineage.g, lineage.b, HALO_ALPHA]
        );
    }
    assert_eq!(halo_factor(0.0), 2.0);
    assert!(halo_factor(1.0e9) < 4.5 && halo_factor(1.0e9) > 4.49);
    assert_eq!(halo_factor(f32::NAN), 2.0);
    assert_eq!(halo_factor(-3.0), 2.0);

    // A resize keeps the halos proportional to the dots.
    let before: Vec<f64> = halos
        .iter()
        .map(|halo| match halo {
            ThreeDDraw::TrueDot(halo) => halo.radius,
            _ => 0.0,
        })
        .collect();
    let small = fit(lease.extent(), [40, 40]);
    let ratio = flife_manim::dot_radius(&small) / flife_manim::dot_radius(&viewport);
    assert!(ratio > 1.0, "the pixel floor binds on the small drawable");
    cloud.refit(&small);
    for (halo, before) in cloud.halos().iter().zip(before) {
        let ThreeDDraw::TrueDot(halo) = halo else {
            panic!("not a halo");
        };
        assert!((halo.radius - before * ratio).abs() <= 1e-12 * before * ratio);
    }
}

#[test]
fn a_frame_draws_the_field_then_the_halos_then_the_dots() {
    let config = common::small();
    let mut world = common::found(&config);
    common::advance(&mut world, 4);
    let lease = common::epoch(&world);
    let mut view = WorldView::for_epoch(&lease, [96, 96], [0, 0, 0]).expect("view");
    view.show(&lease).expect("show");
    assert!(view.field().draw().is_some());
    assert_eq!(view.scene_objects(), 5);
    let rgba = view.render_rgba8(2).expect("an offline frame");
    assert_eq!(rgba.len(), 96 * 96 * 4);
    let (pixels, rest) = rgba.as_chunks::<4>();
    assert!(rest.is_empty());
    assert!(pixels.iter().all(|pixel| pixel[3] == 0xFF), "opaque");
    // The field lights the frame beyond the black background.
    let lit = pixels
        .iter()
        .filter(|pixel| pixel[..3].iter().any(|&channel| channel > 8))
        .count();
    assert!(lit > 96 * 96 / 4, "only {lit} lit pixels");
}
