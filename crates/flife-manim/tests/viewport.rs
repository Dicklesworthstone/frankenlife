//! The viewport's exact map between world, scene, drawable and logical
//! coordinates, and the camera it builds, checked against FrankenManim's own
//! CPU 3D renderer; and FL-020's camera, which pans and zooms across the seams.

use flife_core::CoreError;
use flife_manim::{MAXIMUM_ZOOM, Viewport};
use fmn_core::color::LinearRgba;
use fmn_render::{SamplerPolicy, SurfaceDraw, Texture, ThreeDDraw, ThreeDJob, Tiling, TrueDotDraw};

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

/// The coverage-weighted centroid of the bright pixels of a dot of one
/// world unit at `world`, in drawable coordinates with pixel centers at
/// `+0.5`.
fn centroid(viewport: &Viewport, world: [f64; 2]) -> [f64; 2] {
    centroid_of(viewport, world, 1.0)
}

/// [`centroid`] of a dot of `radius` world units.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn centroid_of(viewport: &Viewport, world: [f64; 2], radius: f64) -> [f64; 2] {
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
        radius,
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

// FL-020: the camera.

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn view(viewport: &Viewport, center: [f64; 2], zoom: f64) -> Viewport {
    viewport.with_view(center, zoom).expect("a valid view")
}

/// The periodic distance between two world points, the largest axis.
fn torus_distance(a: [f64; 2], b: [f64; 2], extent: [f64; 2]) -> f64 {
    (0..2)
        .map(|axis| {
            let d = (a[axis] - b[axis]).rem_euclid(extent[axis]);
            d.min(extent[axis] - d)
        })
        .fold(0.0, f64::max)
}

/// Deterministic points spread over the world.
fn points(extent: [f64; 2], count: u32) -> Vec<[f64; 2]> {
    (0..count)
        .map(|i| {
            let t = f64::from(i);
            [
                (t * 0.618_033_988_749_895).fract() * extent[0],
                (t * 0.754_877_666_246_693).fract() * extent[1],
            ]
        })
        .collect()
}

#[test]
fn the_fitted_center_at_zoom_one_is_the_fit_itself() {
    for (extent, drawable) in [
        ([128.0, 128.0], [800, 450]),
        ([64.0, 32.0], [100, 300]),
        ([128.0, 72.0], [1280, 720]),
    ] {
        let fitted = fit(extent, drawable);
        let same = view(&fitted, [extent[0] / 2.0, extent[1] / 2.0], 1.0);
        assert_eq!(same, fitted);
        assert_eq!(fitted.center(), [extent[0] / 2.0, extent[1] / 2.0]);
        assert_eq!(fitted.zoom(), 1.0);
        assert_eq!(fitted.window_start(), [0.0, 0.0]);
        assert_eq!(fitted.fit_scale(), fitted.scale());
        assert_eq!(fitted.resized(drawable).expect("resized"), fitted);
    }
}

/// A panned view draws every world point once, at its image in the window
/// centered on the view's center, and maps the drawable back to the same
/// torus point.
#[test]
fn a_panned_view_draws_every_point_at_its_one_image_and_maps_back() {
    let extent = [128.0, 128.0];
    let viewport = view(&fit(extent, [800, 800]), [10.0, 120.0], 1.0);
    assert_eq!(viewport.window_start(), [-54.0, 56.0]);
    assert_eq!(viewport.world_to_drawable([10.0, 120.0]), [400.0, 400.0]);
    // (100, 10): x is 154 past the window's start, so its image is 26; y
    // is 46 before it, so 82.
    assert_eq!(viewport.image([100.0, 10.0]), [26.0, 82.0]);
    assert_eq!(viewport.world_to_drawable([100.0, 10.0]), [162.5, 512.5]);
    assert_eq!(
        viewport.scene_point([100.0, 10.0]),
        [26.0 - 64.0, 64.0 - 82.0, 0.0]
    );
    for zoom in [1.0, 2.5, 64.0] {
        let zoomed = view(&viewport, viewport.center(), zoom);
        for point in points(extent, 200) {
            let image = zoomed.image(point);
            assert!((0.0..=128.0).contains(&image[0]) && (0.0..=128.0).contains(&image[1]));
            let drawable = zoomed.world_to_drawable(point);
            let back = zoomed.drawable_to_world(drawable).expect("in the window");
            assert!(
                (0.0..128.0).contains(&back[0]) && (0.0..128.0).contains(&back[1]),
                "{back:?}"
            );
            assert!(
                torus_distance(back, point, extent) < 1e-9,
                "zoom {zoom}: {point:?} came back as {back:?}"
            );
        }
    }
}

/// With the seam under the view's center, two organisms a fifth of a unit
/// apart across it are drawn a fifth of a unit apart, not at opposite edges
/// of the drawable as the fitted view draws them.
#[test]
fn points_across_the_seam_are_drawn_side_by_side_under_a_view_centered_on_it() {
    let extent = [128.0, 128.0];
    let fitted = fit(extent, [800, 800]);
    let (west, east) = ([127.9, 64.0], [0.1, 64.0]);
    let apart = |viewport: &Viewport| {
        viewport.world_to_drawable(east)[0] - viewport.world_to_drawable(west)[0]
    };
    assert!((apart(&fitted) + 127.8 * 6.25).abs() < 1e-9);
    let seam = view(&fitted, [0.0, 64.0], 1.0);
    assert!((apart(&seam) - 0.2 * 6.25).abs() < 1e-9);
    let deep = view(&fitted, [0.0, 64.0], 100.0);
    assert!((apart(&deep) - 0.2 * 625.0).abs() < 1e-6);
    assert!((deep.world_to_drawable(west)[0] - (400.0 - 0.1 * 625.0)).abs() < 1e-6);
}

/// Zooming about an anchor keeps the world point under it; the zoom is
/// clamped to `[1, MAXIMUM_ZOOM]`, and the frame shrinks with it.
#[test]
fn zooming_about_the_pointer_keeps_the_world_under_it() {
    let extent = [128.0, 128.0];
    let mut viewport = fit(extent, [1600, 900]);
    let frame = viewport.frame_shape();
    for (anchor, factor) in [
        ([200.0, 100.0], 2.0),
        ([1500.0, 850.0], 3.5),
        ([800.0, 450.0], 0.75),
        ([10.0, 890.0], 40.0),
        ([1210.0, 333.0], 1.25),
    ] {
        let under = viewport.drawable_to_world(anchor);
        let zoomed = viewport.zoomed_about(anchor, factor).expect("zoomed");
        assert_eq!(
            zoomed.zoom(),
            (viewport.zoom() * factor).clamp(1.0, MAXIMUM_ZOOM)
        );
        if let Some(under) = under {
            let after = zoomed
                .drawable_to_world(anchor)
                .expect("still in the window");
            assert!(
                torus_distance(after, under, extent) < 1e-9,
                "{anchor:?} x {factor}: {under:?} became {after:?}"
            );
        }
        let ratio = viewport.zoom() / zoomed.zoom();
        assert!((zoomed.frame_shape()[1] - viewport.frame_shape()[1] * ratio).abs() < 1e-9);
        viewport = zoomed;
    }
    let out = viewport.zoomed_about([5.0, 5.0], 1e-9).expect("zoomed out");
    assert_eq!(out.zoom(), 1.0);
    assert_eq!(out.frame_shape(), frame);
    let deepest = viewport.zoomed_about([5.0, 5.0], 1e9).expect("zoomed in");
    assert_eq!(deepest.zoom(), MAXIMUM_ZOOM);
}

/// A drag moves the world with the pointer; a drag of the whole world
/// returns to the same view.
#[test]
fn panning_moves_the_world_with_the_drag() {
    let extent = [128.0, 64.0];
    let viewport = view(&fit(extent, [1280, 640]), [30.0, 20.0], 4.0);
    let point = [31.0, 19.5];
    let before = viewport.world_to_drawable(point);
    let panned = viewport.panned([37.0, -12.5]).expect("panned");
    let after = panned.world_to_drawable(point);
    assert!((after[0] - before[0] - 37.0).abs() < 1e-9);
    assert!((after[1] - before[1] + 12.5).abs() < 1e-9);
    let around = viewport
        .panned([extent[0] * viewport.scale(), -extent[1] * viewport.scale()])
        .expect("panned");
    assert!(torus_distance(around.center(), viewport.center(), extent) < 1e-9);
    assert!((0.0..128.0).contains(&around.center()[0]));
}

/// On an axis where the window is narrower than the drawable, the view
/// shows one image of the torus and letterbox beyond it, wherever it is
/// centered.
#[test]
fn a_panned_letterboxed_axis_shows_the_torus_once() {
    let extent = [128.0, 128.0];
    let viewport = view(&fit(extent, [800, 450]), [0.0, 64.0], 1.0);
    let scale = viewport.scale();
    assert_eq!(viewport.origin(), [(800.0 - 450.0) / 2.0, 0.0]);
    // The left margin is margin; just inside it is the world at the
    // window's start, x = 128 - 64 = 64.
    assert_eq!(viewport.drawable_to_world([100.0, 200.0]), None);
    let start = viewport
        .drawable_to_world([175.0 + 1e-9, 200.0])
        .expect("inside");
    assert!((start[0] - 64.0).abs() < 1e-6);
    assert_eq!(
        viewport.drawable_to_world([175.0 + 128.0 * scale, 200.0]),
        None
    );
    for point in points(extent, 100) {
        let at = viewport.world_to_drawable(point);
        assert!(
            (175.0..=175.0 + 128.0 * scale).contains(&at[0]),
            "{point:?} at {at:?}"
        );
    }
}

#[test]
fn views_outside_the_rules_are_refused() {
    let viewport = fit([32.0, 32.0], [100, 100]);
    for (center, zoom) in [
        ([f64::NAN, 1.0], 1.0),
        ([1.0, f64::INFINITY], 1.0),
        ([1.0, 1.0], 0.5),
        ([1.0, 1.0], MAXIMUM_ZOOM * 2.0),
        ([1.0, 1.0], f64::NAN),
    ] {
        assert!(
            matches!(
                viewport.with_view(center, zoom),
                Err(CoreError::NumericalInvalid { .. })
            ),
            "{center:?} {zoom}"
        );
    }
    assert_eq!(
        view(&viewport, [1.0, 1.0], MAXIMUM_ZOOM).zoom(),
        MAXIMUM_ZOOM
    );
    assert_eq!(view(&viewport, [-1.0, 33.0], 1.0).center(), [31.0, 1.0]);
    for factor in [0.0, -2.0, f64::NAN, f64::INFINITY] {
        assert!(matches!(
            viewport.zoomed_about([5.0, 5.0], factor),
            Err(CoreError::NumericalInvalid { .. })
        ));
    }
    assert!(matches!(
        viewport.zoomed_about([f64::NAN, 5.0], 2.0),
        Err(CoreError::NumericalInvalid { .. })
    ));
    assert!(matches!(
        viewport.panned([1.0, f64::NAN]),
        Err(CoreError::NumericalInvalid { .. })
    ));
}

/// A texture whose texels are all different, opaque:
/// `[column / 16, row / 16, (7 · column + row) / 64, 1]`.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn numbered(cells: [u32; 2]) -> Texture {
    let texels = (0..cells[1])
        .flat_map(|row| {
            (0..cells[0]).map(move |column| {
                [
                    column as f32 / 16.0,
                    row as f32 / 16.0,
                    (7 * column + row) as f32 / 64.0,
                    1.0,
                ]
            })
        })
        .collect();
    Texture::from_premultiplied_linear(cells[0], cells[1], texels).expect("a texture")
}

fn close(a: LinearRgba, b: LinearRgba, within: f64) -> bool {
    [(a.r, b.r), (a.g, b.g), (a.b, b.b), (a.a, b.a)]
        .iter()
        .all(|(x, y)| (x - y).abs() <= within)
}

/// The windows the quad tests look through: the fit; one starting a
/// quarter of the way into a cell; one cut on both axes; and one zoomed
/// across both seams.
const WINDOWS: [([f64; 2], f64); 4] = [
    ([4.0, 2.0], 1.0),
    ([0.5, 2.0], 1.0),
    ([0.5, 1.5], 1.0),
    ([7.9, 0.1], 3.0),
];

/// FL-033: the periodic quad is the window, centered, and its texture
/// coordinates put every texel of a periodic texture on its corner of the
/// torus, `L · k / n`, wherever the window's image of that corner falls.
/// Where the window cuts a cell, its edges sample the cut cell's corners
/// blended linearly, and the two edges, one torus line, agree.
#[test]
fn the_periodic_quad_puts_every_texel_on_its_corner_in_any_window() {
    let fitted = fit([8.0, 4.0], [160, 80]);
    let cells = [4, 2];
    let texture = numbered(cells);
    // As `SurfaceDraw::image` samples: repeating both ways.
    let policy = SamplerPolicy::default();
    for (center, zoom) in WINDOWS {
        let viewport = view(&fitted, center, zoom);
        let quad = viewport.periodic_quad(cells).expect("a quad");
        let vertices = quad.vertices();
        assert_eq!(vertices.len(), 4);
        assert_eq!(quad.indices(), &[0, 1, 3, 0, 3, 2]);
        let points: Vec<[f64; 3]> = vertices.iter().map(|vertex| vertex.point).collect();
        assert_eq!(
            points,
            vec![
                [-4.0, 2.0, 0.0],
                [4.0, 2.0, 0.0],
                [-4.0, -2.0, 0.0],
                [4.0, -2.0, 0.0]
            ]
        );
        assert!(
            vertices
                .iter()
                .all(|vertex| vertex.normal == [0.0, 0.0, 1.0] && vertex.opacity == 1.0)
        );
        // One texture span across the window each way, rows downward.
        let [top_left, top_right, bottom_left, bottom_right] =
            [0, 1, 2, 3].map(|at| vertices[at].uv);
        assert_eq!(top_right, [top_left[0] + 1.0, top_left[1]]);
        assert_eq!(bottom_left, [top_left[0], top_left[1] + 1.0]);
        assert_eq!(bottom_right, [top_left[0] + 1.0, top_left[1] + 1.0]);
        // Texture coordinates are affine across the quad.
        let uv_at = |scene: [f64; 3]| {
            [
                top_left[0] + (scene[0] + 4.0) / 8.0,
                top_left[1] + (2.0 - scene[1]) / 4.0,
            ]
        };
        for row in 0..cells[1] {
            for column in 0..cells[0] {
                let corner = [8.0 * f64::from(column) / 4.0, 4.0 * f64::from(row) / 2.0];
                let got = texture.sample(uv_at(viewport.scene_point(corner)), policy);
                let want = texture.texel(column, row).expect("a texel");
                assert!(
                    close(got, want, 1e-12),
                    "{center:?} x{zoom}: corner ({column}, {row}) samples {got:?}, not {want:?}"
                );
            }
        }
        let [left, right] = [top_left, top_right].map(|uv| texture.sample(uv, policy));
        assert!(
            close(left, right, 1e-12),
            "{center:?}: the seam's two images"
        );
    }

    // Center (0.5, 2): the window starts at x = -3.5, which is 4.5 on the
    // torus, a quarter of the way from corner 2 (x = 4) to corner 3; y = 0
    // is aligned. Center (0.5, 1.5) also starts it at y = -0.5, three
    // quarters of the way from corner 1 (y = -2) to corner 0.
    let texel = |column, row| texture.texel(column, row).expect("a texel");
    let lerp = |a: LinearRgba, b: LinearRgba, t: f64| LinearRgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    };
    let start = |center| {
        let quad = view(&fitted, center, 1.0)
            .periodic_quad(cells)
            .expect("a quad");
        texture.sample(quad.vertices()[0].uv, policy)
    };
    let cut = lerp(texel(2, 0), texel(3, 0), 0.25);
    assert!(close(start([0.5, 2.0]), cut, 1e-12));
    let both = lerp(
        lerp(texel(2, 1), texel(3, 1), 0.25),
        lerp(texel(2, 0), texel(3, 0), 0.25),
        0.75,
    );
    assert!(close(start([0.5, 1.5]), both, 1e-12));

    for empty in [[0, 2], [4, 0]] {
        assert!(matches!(
            fitted.periodic_quad(empty),
            Err(CoreError::NumericalInvalid { .. })
        ));
    }
}

/// FL-033, against FrankenManim's CPU 3D route: a periodic texture on the
/// periodic quad, drawn as the field and density layers draw it, shows at
/// every pixel the bilinear blend of the torus corners around the world
/// point the viewport maps the pixel to, through the window's cuts and
/// across the seams. The blend is computed from world coordinates alone.
#[test]
fn frankenmanim_draws_the_periodic_quad_bilinear_between_the_torus_corners() {
    let fitted = fit([8.0, 4.0], [160, 80]);
    let cells = [4, 2];
    let texture = numbered(cells);
    let texel = |column: i64, row: i64| {
        let column = u32::try_from(column.rem_euclid(4)).expect("wrapped");
        let row = u32::try_from(row.rem_euclid(2)).expect("wrapped");
        texture.texel(column, row).expect("a texel")
    };
    let black = LinearRgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    let mut worst = 0.0_f64;
    for (center, zoom) in WINDOWS {
        let viewport = view(&fitted, center, zoom);
        let quad = viewport.periodic_quad(cells).expect("a quad");
        let camera = viewport.camera(black).expect("a camera");
        let draws = [ThreeDDraw::Surface(SurfaceDraw::image(&quad, &texture))];
        let frame = ThreeDJob::new(camera.camera(), &draws, Tiling::default())
            .expect("a job")
            .render(2)
            .expect("a frame");
        let stride = frame.layout().stride(0);
        let bytes = frame.plane(0);
        for row in 0..80_usize {
            for column in 0..160_usize {
                let centre = [column as f64 + 0.5, row as f64 + 0.5];
                let world = viewport.drawable_to_world(centre).expect("in the window");
                // The corners around the world point, and how far it is
                // between them, on the torus.
                let [x, y] = [world[0] * 4.0 / 8.0, world[1] * 2.0 / 4.0];
                let [x0, y0] = [x.floor(), y.floor()];
                let [fx, fy] = [x - x0, y - y0];
                let [x0, y0] = [x0 as i64, y0 as i64];
                let blend = |channel: fn(LinearRgba) -> f64| {
                    let top = channel(texel(x0, y0)) * (1.0 - fx) + channel(texel(x0 + 1, y0)) * fx;
                    let bottom = channel(texel(x0, y0 + 1)) * (1.0 - fx)
                        + channel(texel(x0 + 1, y0 + 1)) * fx;
                    top * (1.0 - fy) + bottom * fy
                };
                let at = row * stride + column * 8;
                for (channel, offset) in [
                    (blend(|c| c.r), 0),
                    (blend(|c| c.g), 2),
                    (blend(|c| c.b), 4),
                ] {
                    let drawn = half(bytes, at + offset);
                    worst = worst.max((drawn - channel).abs());
                    assert!(
                        (drawn - channel).abs() <= 2e-3,
                        "{center:?} x{zoom}: pixel ({column}, {row}) at world {world:?}: \
                         {drawn} != {channel}"
                    );
                }
            }
        }
    }
    println!("the largest difference from the torus blend: {worst:.2e}");
}

#[test]
fn frankenmanim_draws_each_world_point_where_a_panned_zoomed_view_says() {
    let fitted = fit([32.0, 32.0], [160, 90]);
    for (center, zoom) in [([1.0, 31.0], 1.0), ([0.5, 16.0], 3.0), ([30.0, 2.0], 8.0)] {
        let viewport = view(&fitted, center, zoom);
        for offset in [[0.0, 0.0], [0.6, -0.4], [-1.2, 0.9], [1.5, 1.5]] {
            // Divided by the zoom, so the dot stays well inside the frame.
            let world = [
                (center[0] + offset[0] / zoom).rem_euclid(32.0),
                (center[1] + offset[1] / zoom).rem_euclid(32.0),
            ];
            let expected = viewport.world_to_drawable(world);
            // A dot of the same few pixels at every zoom: FrankenManim's
            // perspective frame bulges a dot large against its focal
            // distance (the cloud caps its dots for that reason).
            let drawn = centroid_of(&viewport, world, 1.0 / zoom);
            assert!(
                (drawn[0] - expected[0]).abs() < 0.05 && (drawn[1] - expected[1]).abs() < 0.05,
                "{center:?} x {zoom}, world {world:?}: drawn at {drawn:?}, expected {expected:?}"
            );
        }
    }
}

/// FL-020: a viewport's pixels per point changes no mapping, survives every
/// view change, and comes back with a refit; a density that is not finite
/// and positive is refused.
#[test]
fn pixels_per_point_changes_no_mapping_and_survives_every_view() {
    let one = fit([64.0, 32.0], [640, 320]);
    assert_eq!(one.pixels_per_point().to_bits(), 1.0_f64.to_bits());
    let two = one.with_pixels_per_point(2.0).expect("2x");
    assert_eq!(two.pixels_per_point().to_bits(), 2.0_f64.to_bits());
    for point in [[0.0, 0.0], [12.5, 30.0], [63.9, 0.1], [32.0, 16.0]] {
        assert_eq!(two.world_to_drawable(point), one.world_to_drawable(point));
        assert_eq!(two.scene_point(point), one.scene_point(point));
    }
    assert_eq!(
        (two.scale(), two.origin(), two.center(), two.zoom()),
        (one.scale(), one.origin(), one.center(), one.zoom())
    );
    let moved = two
        .zoomed_about([100.0, 50.0], 3.0)
        .and_then(|view| view.panned([7.0, -4.0]))
        .and_then(|view| view.with_view([3.0, 31.0], 5.0))
        .and_then(|view| view.resized([1280, 640]))
        .expect("moved");
    assert_eq!(moved.pixels_per_point().to_bits(), 2.0_f64.to_bits());
    assert_eq!(
        moved.refit(),
        fit([64.0, 32.0], [1280, 640])
            .with_pixels_per_point(2.0)
            .expect("2x")
    );
    let zoomed = one.zoomed_about([10.0, 10.0], 4.0).expect("zoomed");
    assert_eq!(zoomed.refit(), one);
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(
            matches!(
                one.with_pixels_per_point(bad),
                Err(CoreError::NumericalInvalid { .. })
            ),
            "{bad}"
        );
    }
}
