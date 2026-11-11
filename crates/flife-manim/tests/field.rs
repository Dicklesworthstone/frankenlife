//! The field layer and the organisms' halos: what the view draws beyond one
//! dot per organism.

mod common;

use flife_manim::{
    FIELD_PALETTE, FieldLayer, HALO_ALPHA, Look, ParticleCloud, SCALE_DECAY, Viewport, WorldView,
    display_value, halo_factor, lineage_color, palette, raw_value,
};
use flife_render::FieldTexture;
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
    assert_eq!(view.scene_objects(), 4);
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
