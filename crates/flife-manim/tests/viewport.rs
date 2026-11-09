//! The viewport's exact map between world, scene, drawable and logical
//! coordinates, and the camera it builds, checked against FrankenManim's own
//! CPU 3D renderer.

use flife_core::CoreError;
use flife_manim::Viewport;
use fmn_core::color::LinearRgba;
use fmn_render::{ThreeDDraw, ThreeDJob, Tiling, TrueDotDraw};

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn fit(extent: [f64; 2], drawable: [u32; 2]) -> Viewport {
    Viewport::fit(extent, drawable).expect("a valid viewport")
}

#[test]
fn the_torus_is_fitted_whole_centered_and_letterboxed() {
    // Wider than the world: margins left and right.
    let wide = fit([128.0, 128.0], [800, 450]);
    assert_eq!(wide.scale(), 450.0 / 128.0);
    assert_eq!(wide.origin(), [(800.0 - 450.0) / 2.0, 0.0]);
    // Taller than the world: margins above and below.
    let tall = fit([64.0, 32.0], [100, 300]);
    assert_eq!(tall.scale(), 100.0 / 64.0);
    assert_eq!(tall.origin(), [0.0, (300.0 - 50.0) / 2.0]);
    // Exactly the world's aspect: no margin.
    let exact = fit([128.0, 72.0], [1280, 720]);
    assert_eq!(exact.scale(), 10.0);
    assert_eq!(exact.origin(), [0.0, 0.0]);
    assert_eq!(exact.world_to_drawable([128.0, 72.0]), [1280.0, 720.0]);
}

#[test]
fn world_and_drawable_points_round_trip_and_the_margin_is_not_the_world() {
    let viewport = fit([128.0, 128.0], [800, 450]);
    for world in [[0.0, 0.0], [12.5, 100.25], [64.0, 64.0], [127.75, 0.5]] {
        let drawable = viewport.world_to_drawable(world);
        let back = viewport.drawable_to_world(drawable);
        assert_eq!(back, Some(world), "round trip of {world:?}");
    }
    // The left margin, the far edges (the world is half-open) and outside.
    assert_eq!(viewport.drawable_to_world([10.0, 200.0]), None);
    assert_eq!(
        viewport.drawable_to_world(viewport.world_to_drawable([128.0, 64.0])),
        None
    );
    assert_eq!(
        viewport.drawable_to_world(viewport.world_to_drawable([64.0, 128.0])),
        None
    );
    assert_eq!(viewport.drawable_to_world([400.0, -0.5]), None);
}

#[test]
fn logical_points_scale_by_the_drawable_ratio() {
    let viewport = fit([128.0, 128.0], [1600, 900]);
    // A Retina-style window: 800 x 450 points over 1600 x 900 pixels.
    let world = [40.0, 90.0];
    let pixel = viewport.world_to_drawable(world);
    let point = [pixel[0] / 2.0, pixel[1] / 2.0];
    assert_eq!(
        viewport.logical_to_world(point, [800.0, 450.0]),
        Some(world)
    );
    // Before the content area has a size, nothing is under the pointer.
    assert_eq!(viewport.logical_to_world(point, [0.0, 450.0]), None);
    assert_eq!(viewport.logical_to_world(point, [f64::NAN, 450.0]), None);
}

#[test]
fn scene_points_center_the_world_with_y_up_and_the_frame_is_the_whole_drawable() {
    let viewport = fit([128.0, 64.0], [400, 400]);
    assert_eq!(viewport.scene_point([0.0, 0.0]), [-64.0, 32.0, 0.0]);
    assert_eq!(viewport.scene_point([128.0, 64.0]), [64.0, -32.0, 0.0]);
    assert_eq!(viewport.scene_point([64.0, 32.0]), [0.0, 0.0, 0.0]);
    let scale = 400.0 / 128.0;
    assert_eq!(viewport.frame_shape(), [400.0 / scale, 400.0 / scale]); // Wider than the world: the frame is wider than the world too.
    let wide = fit([128.0, 128.0], [800, 450]);
    assert_eq!(wide.frame_shape(), [800.0 / wide.scale(), 128.0]);
    assert!(wide.frame_shape()[0] > 128.0);
    // Taller than the world: the frame is taller.
    let tall = fit([128.0, 128.0], [450, 800]);
    assert_eq!(tall.frame_shape(), [128.0, 800.0 / tall.scale()]);
    assert!(tall.frame_shape()[1] > 128.0);
}

#[test]
fn degenerate_worlds_and_drawables_are_refused() {
    for extent in [
        [0.0, 1.0],
        [-1.0, 1.0],
        [f64::INFINITY, 1.0],
        [1.0, f64::NAN],
    ] {
        assert!(
            matches!(
                Viewport::fit(extent, [10, 10]),
                Err(CoreError::NumericalInvalid { .. })
            ),
            "{extent:?}"
        );
    }
    for drawable in [[0, 10], [10, 0]] {
        assert!(
            matches!(
                Viewport::fit([1.0, 1.0], drawable),
                Err(CoreError::NumericalInvalid { .. })
            ),
            "{drawable:?}"
        );
    }
}

/// The half-float at byte `at`.
fn half(bytes: &[u8], at: usize) -> f64 {
    let bits = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let sign = if bits >> 15 == 1 { -1.0 } else { 1.0 };
    let exponent = i32::from((bits >> 10) & 0x1F);
    let fraction = f64::from(bits & 0x3FF);
    match exponent {
        0 => sign * fraction * 2f64.powi(-24),
        31 => f64::NAN,
        _ => sign * (1.0 + fraction / 1024.0) * 2f64.powi(exponent - 15),
    }
}

/// The coverage-weighted centroid of `frame`'s bright pixels, in drawable
/// coordinates with pixel centers at `+0.5`.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn centroid(viewport: &Viewport, world: [f64; 2]) -> [f64; 2] {
    let camera = viewport
        .camera(LinearRgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        })
        .expect("a camera");
    let draws = [ThreeDDraw::TrueDot(TrueDotDraw::new(
        viewport.scene_point(world),
        1.0,
        LinearRgba {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        },
    ))];
    let job = ThreeDJob::new(camera.camera(), &draws, Tiling::default()).expect("a job");
    let frame = job.render(2).expect("a frame");
    let layout = frame.layout();
    let [width, height] = viewport.drawable();
    assert_eq!((layout.width(), layout.height()), (width, height));
    let bytes = frame.plane(0);
    let stride = layout.stride(0);
    let (mut sum, mut x, mut y) = (0.0, 0.0, 0.0);
    for row in 0..height as usize {
        for column in 0..width as usize {
            let red = half(bytes, row * stride + column * 8);
            if red > 0.0 {
                sum += red;
                x += red * (column as f64 + 0.5);
                y += red * (row as f64 + 0.5);
            }
        }
    }
    assert!(sum > 0.0, "the dot at {world:?} drew nothing");
    [x / sum, y / sum]
}

#[test]
fn frankenmanim_draws_each_world_point_where_the_viewport_says() {
    // Letterboxed both ways: wider than the world (margins left and right)
    // and taller (margins above and below); points near every edge.
    for drawable in [[160, 90], [90, 160]] {
        let viewport = fit([32.0, 32.0], drawable);
        for world in [
            [4.0, 4.0],
            [28.0, 8.0],
            [16.0, 16.0],
            [8.0, 29.0],
            [30.5, 30.5],
        ] {
            let expected = viewport.world_to_drawable(world);
            let drawn = centroid(&viewport, world);
            assert!(
                (drawn[0] - expected[0]).abs() < 0.05 && (drawn[1] - expected[1]).abs() < 0.05,
                "{drawable:?}, world {world:?}: drawn at {drawn:?}, expected {expected:?}"
            );
        }
    }
}
