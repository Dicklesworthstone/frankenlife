//! The viewport's exact map between world, scene, drawable and logical
//! coordinates, and the camera it builds, checked against FrankenManim's own
//! CPU 3D renderer; and FL-020's camera, which pans and zooms across the seams.

use flife_core::CoreError;
use flife_manim::{MAXIMUM_ZOOM, Viewport};
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

fn shade(index: [usize; 2]) -> LinearRgba {
    LinearRgba {
        r: index[0] as f64,
        g: index[1] as f64,
        b: (index[0] * 7 + index[1]) as f64,
        a: 1.0,
    }
}

/// At the fit, the periodic grid is the corner lattice at `L · k / n`. A
/// panned view cuts the grid at the window's edges: both edges are the same
/// torus line and carry the same colours, the linear blend of the cut
/// cell's corners, and the vertices cover the window exactly, in order.
#[test]
fn the_periodic_grid_is_the_lattice_at_the_fit_and_is_cut_at_a_panned_window() {
    let extent = [8.0, 4.0];
    let fitted = fit(extent, [160, 80]);
    let (vertices, indices) = fitted
        .periodic_grid([4, 2], ["v", "i"], shade)
        .expect("a grid");
    assert_eq!(vertices.len(), 5 * 3);
    assert_eq!(indices.len(), 4 * 2 * 6);
    for row in 0..3 {
        for column in 0..5 {
            let vertex = &vertices[row * 5 + column];
            let world = [8.0 * column as f64 / 4.0, 4.0 * row as f64 / 2.0];
            assert_eq!(vertex.point, fitted.scene_point(world));
            assert_eq!(vertex.color, shade([column % 4, row % 2]));
        }
    }

    // Center (0.5, 2): the window starts at x = -3.5, a quarter of the way
    // into cell -2 (corner 2 to corner 3); y = 0 is aligned.
    let panned = view(&fitted, [0.5, 2.0], 1.0);
    let (vertices, indices) = panned
        .periodic_grid([4, 2], ["v", "i"], shade)
        .expect("a grid");
    let columns = 4 + 2;
    assert_eq!(vertices.len(), columns * 3);
    assert_eq!(indices.len(), (columns - 1) * 2 * 6);
    let xs: Vec<f64> = vertices[..columns].iter().map(|v| v.point[0]).collect();
    assert_eq!(xs, vec![-4.0, -2.5, -0.5, 1.5, 3.5, 4.0]);
    for row in 0..3 {
        let first = &vertices[row * columns];
        let last = &vertices[row * columns + columns - 1];
        assert_eq!(first.color, last.color, "the seam's two images agree");
        let (a, b) = (shade([2, row % 2]), shade([3, row % 2]));
        assert!((first.color.r - (0.75 * a.r + 0.25 * b.r)).abs() < 1e-12);
        assert!((first.color.b - (0.75 * a.b + 0.25 * b.b)).abs() < 1e-12);
        let corners: Vec<f64> = vertices[row * columns + 1..row * columns + columns - 1]
            .iter()
            .map(|v| v.color.r)
            .collect();
        assert_eq!(corners, vec![3.0, 0.0, 1.0, 2.0]);
    }
    // Both axes cut: center (0.5, 1.5) also starts the window at y = -0.5,
    // three quarters into cell -1 (corner 1 to corner 0). The window's four
    // corners are one torus point, coloured by the bilinear blend.
    let both = view(&fitted, [0.5, 1.5], 1.0);
    let (cut, _) = both
        .periodic_grid([4, 2], ["v", "i"], shade)
        .expect("a grid");
    assert_eq!(cut.len(), 6 * 4);
    let corner = cut[0].color;
    let blend = |channel: fn(LinearRgba) -> f64| {
        0.75 * 0.25 * channel(shade([2, 1]))
            + 0.75 * 0.75 * channel(shade([2, 0]))
            + 0.25 * 0.25 * channel(shade([3, 1]))
            + 0.25 * 0.75 * channel(shade([3, 0]))
    };
    assert!((corner.g - 0.25).abs() < 1e-12, "{corner:?}");
    assert!((corner.b - blend(|c| c.b)).abs() < 1e-12, "{corner:?}");
    for at in [5, 18, 23] {
        assert_eq!(cut[at].color, corner);
    }
    // Every vertex is in the window, which is centered.
    assert!(
        vertices
            .iter()
            .all(|v| v.point[0].abs() <= 4.0 && v.point[1].abs() <= 2.0)
    );

    assert!(matches!(
        fitted.periodic_grid([0, 2], ["v", "i"], shade),
        Err(CoreError::NumericalInvalid { .. })
    ));
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
