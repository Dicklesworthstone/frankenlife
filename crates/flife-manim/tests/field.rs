//! The field layer and the organisms' halos: what the view draws beyond one
//! dot per organism.

mod common;

use flife_manim::{
    FIELD_PALETTE, FieldLayer, HALO_ALPHA, ParticleCloud, SCALE_DECAY, Viewport, WorldView,
    display_value, halo_factor, lineage_color, palette,
};
use flife_render::FieldTexture;
use fmn_render::{SurfaceMaterial, ThreeDDraw};

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

#[test]
fn the_field_grid_covers_the_torus_without_a_seam() {
    let viewport = fit([8.0, 4.0], [160, 80]);
    let mut layer = FieldLayer::new(&viewport);
    assert!(layer.draw().is_none(), "nothing before a field is shown");
    // A ramp along x: the seam joins its brightest and darkest columns.
    let plane: Vec<f32> = (0..32).map(|cell| (cell % 8) as f32).collect();
    layer.update(&texture([8, 4], &plane)).expect("update");
    assert_eq!(layer.scale(), 7.0);
    let Some(ThreeDDraw::Surface(draw)) = layer.draw() else {
        panic!("the layer draws one surface");
    };
    assert!(matches!(draw.material, SurfaceMaterial::VertexColor));
    assert!(!draw.depth_test, "the field stays under the organisms");
    let vertices = draw.mesh.vertices();
    assert_eq!(vertices.len(), 9 * 5, "texel corners, both seams included");
    // The corners span the world exactly.
    let first = vertices[0].point;
    let last = vertices[vertices.len() - 1].point;
    assert_eq!(first, viewport.scene_point([0.0, 0.0]));
    assert_eq!(last, viewport.scene_point([8.0, 4.0]));
    // Wrapping: the left and right edges, and the top and bottom, match.
    for row in 0..5 {
        assert_eq!(
            vertices[row * 9].color,
            vertices[row * 9 + 8].color,
            "row {row}"
        );
    }
    for column in 0..9 {
        assert_eq!(vertices[column].color, vertices[4 * 9 + column].color);
    }
    // The seam corner averages column 7 (bright) with column 0 (dark); the
    // next corner averages columns 0 and 1.
    let shade = |mean: f64| {
        let [r, g, b] = palette(display_value(mean as f32, 7.0));
        fmn_core::color::Srgb::from_rgb8(r, g, b).to_linear(1.0)
    };
    assert_eq!(vertices[9].color, shade(3.5));
    assert_eq!(vertices[9 + 1].color, shade(0.5));
    assert_ne!(vertices[9].color, vertices[9 + 1].color);
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
    assert_eq!(view.scene_objects(), 3);
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
