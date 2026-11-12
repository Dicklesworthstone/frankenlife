//! FL-024's metric overlay (plan §12.2): text over the world as triangles
//! FrankenManim's 3D route draws.
//!
//! The oracles are independent of the triangulation: each glyph's ink is
//! the nonzero fill of its flattened contours, computed here by winding
//! number; the flattening is checked against the quadratic itself; the
//! overlay's place is checked against the viewport's own map of world
//! points; and a frame drawn with text differs from the frame without it
//! only under the plate, where the ink is the text's colour and the rest
//! is dimmed.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "test helpers: a failure in one is a test failure; exact equality is the claim"
)]

mod common;

use flife_manim::{
    HUD_FLATNESS_PIXELS, HUD_MAX_CURVE_SEGMENTS, HUD_MAX_TRIANGLES, HUD_MAX_VERTICES,
    HUD_PADDING_POINTS, HUD_PLATE_OPACITY, HUD_TEXT_COLOR, HudFont, HudText, JOB_BYTES_PER_DRAW,
    PREPARATION_BYTES_PER_TRIANGLE, Viewport, WorldView, flatten_quadratic, hud_glyphs, hud_origin,
    hud_plate_rect, hud_scene_point, linear,
};
use fmn_render::{SurfaceVertex, ThreeDDraw};

fn font() -> HudFont {
    HudFont::bundled().expect("the bundled fonts")
}

fn bezier(p: [[f64; 2]; 3], t: f64) -> [f64; 2] {
    let s = 1.0 - t;
    [0, 1].map(|axis| s * s * p[0][axis] + 2.0 * s * t * p[1][axis] + t * t * p[2][axis])
}

fn segment_distance(point: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let length = d[0] * d[0] + d[1] * d[1];
    let t = if length == 0.0 {
        0.0
    } else {
        (((point[0] - a[0]) * d[0] + (point[1] - a[1]) * d[1]) / length).clamp(0.0, 1.0)
    };
    (point[0] - a[0] - t * d[0]).hypot(point[1] - a[1] - t * d[1])
}

#[test]
fn a_flattened_quadratic_stays_within_its_tolerance() {
    let mut worst = 0.0_f64;
    for (index, p) in [
        [[0.0, 0.0], [5.0, 9.0], [10.0, 0.0]],
        [[0.0, 0.0], [0.3, 0.2], [1.0, 0.0]],
        [[3.0, -2.0], [-40.0, 17.0], [12.0, 30.0]],
        [[0.0, 0.0], [100.0, 100.0], [0.0, 0.0]],
        [[1.5, 2.5], [1.5, 2.5], [1.5, 2.5]],
        [[0.0, 0.0], [1.0, 1.0], [2.0, 2.0]],
    ]
    .into_iter()
    .enumerate()
    {
        let mut points = vec![p[0]];
        flatten_quadratic(p[0], p[1], p[2], HUD_FLATNESS_PIXELS, &mut points);
        let segments = points.len() - 1;
        assert!(
            (1..=HUD_MAX_CURVE_SEGMENTS).contains(&segments),
            "{index}: {segments}"
        );
        assert_eq!(points.last(), Some(&p[2]), "{index} ends on its anchor");
        if index >= 4 {
            assert_eq!(segments, 1, "{index}: a straight quadratic is one segment");
        }
        // The fourth bends so far that the segment cap binds; the rest must
        // keep to the tolerance.
        if index == 3 {
            continue;
        }
        for step in 0..=4_096 {
            let sample = bezier(p, f64::from(step) / 4_096.0);
            let distance = points
                .windows(2)
                .map(|pair| segment_distance(sample, pair[0], pair[1]))
                .fold(f64::INFINITY, f64::min);
            worst = worst.max(distance);
        }
    }
    assert!(worst <= HUD_FLATNESS_PIXELS * (1.0 + 1e-9), "{worst}");
    assert!(
        worst > HUD_FLATNESS_PIXELS / 8.0,
        "{worst}: suspiciously fine"
    );
}

/// The winding number of `rings` about `point`.
fn winding(text: &HudText, point: [f64; 2]) -> i32 {
    let vertices = text.vertices();
    let mut number = 0;
    for ring in text.rings() {
        let ring = &vertices[ring.clone()];
        for (k, &a) in ring.iter().enumerate() {
            let b = ring[(k + 1) % ring.len()];
            let side = (b[0] - a[0]) * (point[1] - a[1]) - (point[0] - a[0]) * (b[1] - a[1]);
            if a[1] <= point[1] {
                if b[1] > point[1] && side > 0.0 {
                    number += 1;
                }
            } else if b[1] <= point[1] && side < 0.0 {
                number -= 1;
            }
        }
    }
    number
}

fn edge_distance(text: &HudText, point: [f64; 2]) -> f64 {
    let vertices = text.vertices();
    let mut nearest = f64::INFINITY;
    for ring in text.rings() {
        let ring = &vertices[ring.clone()];
        for (k, &a) in ring.iter().enumerate() {
            nearest = nearest.min(segment_distance(point, a, ring[(k + 1) % ring.len()]));
        }
    }
    nearest
}

fn in_triangles(text: &HudText, point: [f64; 2]) -> bool {
    let vertices = text.vertices();
    text.triangles().iter().any(|triangle| {
        let [a, b, c] = triangle.map(|corner| vertices[corner as usize]);
        let side = |p: [f64; 2], q: [f64; 2]| {
            (q[0] - p[0]) * (point[1] - p[1]) - (point[0] - p[0]) * (q[1] - p[1])
        };
        let d = [side(a, b), side(b, c), side(c, a)];
        !(d.iter().any(|&x| x < 0.0) && d.iter().any(|&x| x > 0.0))
    })
}

/// The characters the overlay prints, and the rest of printable ASCII.
const CHARACTERS: &str = concat!(
    "!\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`",
    "abcdefghijklmnopqrstuvwxyz{|}~×·–—…°"
);

#[test]
fn every_glyph_is_the_nonzero_fill_of_its_outline() {
    let font = font();
    let mut checked = 0_u64;
    let mut inked = 0_u64;
    for em in [13.0, 26.0] {
        for character in CHARACTERS.chars() {
            let text = font
                .text(&character.to_string(), em)
                .unwrap_or_else(|error| panic!("{character:?}: {error}"));
            assert_eq!(text.glyphs(), 1, "{character:?}");
            assert_eq!(text.refused(), 0, "{character:?}");
            assert_eq!(text.left_out(), 0, "{character:?}");
            assert!(!text.is_empty(), "{character:?} has ink");
            let [width, height] = text.size();
            for vertex in text.vertices() {
                assert!(
                    (-em..=width).contains(&vertex[0])
                        && (-1e-9 * em..=height).contains(&vertex[1]),
                    "{character:?}: {vertex:?} outside {width} x {height}"
                );
            }
            let step = em / 97.0;
            let mut y = -em + step / 3.0;
            while y < height + em {
                let mut x = -em + step / 7.0;
                while x < width + em {
                    let point = [x, y];
                    if edge_distance(&text, point) > em * 1e-9 {
                        let ink = winding(&text, point) != 0;
                        assert_eq!(
                            in_triangles(&text, point),
                            ink,
                            "{character:?} at {em} px: {point:?}, winding {}",
                            winding(&text, point)
                        );
                        checked += 1;
                        inked += u64::from(ink);
                    }
                    x += step;
                }
                y += step;
            }
        }
    }
    // The box is an em wider than each glyph all round, so ink is a small share.
    assert!(inked > checked / 100, "{inked} of {checked}");
}

#[test]
fn lines_run_down_from_the_top_left_and_spaces_take_no_ink() {
    let font = font();
    let em = 20.0;
    let one = font.text("H", em).expect("one line");
    let two = font.text("H\nH", em).expect("two lines");
    let spaced = font.text("H H", em).expect("a space");
    assert_eq!(two.glyphs(), 2);
    assert_eq!(spaced.glyphs(), 2);
    assert_eq!(two.vertices().len(), 2 * one.vertices().len());
    assert_eq!(two.triangles().len(), 2 * one.triangles().len());
    let below = &two.vertices()[one.vertices().len()..];
    for (upper, lower) in one.vertices().iter().zip(below) {
        assert!((upper[0] - lower[0]).abs() < 1e-9, "{upper:?} {lower:?}");
        // The default baseline skip: 1.2 em.
        assert!(
            (lower[1] - upper[1] - 1.2 * em).abs() < 1e-9,
            "{upper:?} {lower:?}"
        );
    }
    assert!(two.size()[1] > one.size()[1] + em);
    assert!(spaced.size()[0] > 2.0 * one.size()[0]);
    let none = font.text(" \n ", em).expect("only spaces");
    assert!(none.is_empty());
    assert_eq!(none.glyphs(), 0);
    assert!(font.text("H", 0.0).is_err());
    assert!(font.text("H", f64::NAN).is_err());
}

#[test]
fn a_long_text_stops_at_its_budget() {
    let font = font();
    // Glyphs with holes take more triangles than vertices, so the triangle
    // ceiling binds first; glyphs without take fewer, so the vertex one does.
    for (glyphs, vertices_bind) in [("@%&$#8BgQ", false), ("SCWMJsczr", true)] {
        let line = glyphs.repeat(12);
        let text = font
            .text(&[line.as_str(); 20].join("\n"), 26.0)
            .expect("a long text");
        let count = 20 * line.chars().count();
        let (vertices, triangles) = (text.vertices().len(), text.triangles().len());
        assert!(text.left_out() > 0, "{glyphs}: the budget never bound");
        assert_eq!(text.glyphs() + text.left_out() + text.refused(), count);
        assert!(
            vertices <= HUD_MAX_VERTICES,
            "{glyphs}: {vertices} vertices"
        );
        assert!(
            triangles <= HUD_MAX_TRIANGLES,
            "{glyphs}: {triangles} triangles"
        );
        assert_eq!(
            vertices > triangles,
            vertices_bind,
            "{glyphs}: {vertices}, {triangles}"
        );
        let bound = if vertices_bind {
            vertices > HUD_MAX_VERTICES * 9 / 10
        } else {
            triangles > HUD_MAX_TRIANGLES * 9 / 10
        };
        assert!(
            bound,
            "{glyphs} stopped early: {vertices} vertices, {triangles} triangles"
        );
    }
}

#[test]
fn the_overlay_is_where_the_camera_draws_its_pixels() {
    let extent = [32.0, 18.0];
    for drawable in [[480, 270], [300, 300], [1_000, 200]] {
        let fit = Viewport::fit(extent, drawable).expect("fits");
        for viewport in [
            fit,
            fit.with_view([3.0, 17.5], 2.5).expect("zoomed over a seam"),
            fit.with_pixels_per_point(2.0).expect("2x"),
            fit.with_view([31.0, 1.0], 7.0).expect("zoomed in a corner"),
        ] {
            for point in [
                [0.5, 0.5],
                [16.0, 9.0],
                [31.9, 17.9],
                [3.0, 17.5],
                [2.0, 0.1],
            ] {
                let scene = viewport.scene_point(point);
                let mapped = hud_scene_point(&viewport, viewport.world_to_drawable(point));
                for axis in 0..3 {
                    assert!(
                        (scene[axis] - mapped[axis]).abs() <= 1e-9 * extent[0],
                        "{drawable:?} {viewport:?} {point:?}: {scene:?} {mapped:?}"
                    );
                }
            }
        }
    }
}

fn surface_vertices(draw: &ThreeDDraw<'_>) -> Vec<SurfaceVertex> {
    match draw {
        ThreeDDraw::Surface(surface) => surface.mesh.vertices().to_vec(),
        _ => panic!("not a surface"),
    }
}

#[test]
fn a_view_that_draws_text_charges_for_it_and_draws_it_last() {
    let config = common::small();
    let world = common::found(&config);
    let lease = common::epoch(&world);
    let mut view = WorldView::for_epoch(&lease, [480, 270], [0, 0, 0]).expect("view");
    view.show(&lease).expect("show");
    let font = font();
    let text = font.text("tick 1,024", 26.0).expect("text");
    assert!(
        view.set_overlay(Some(text.clone())).is_err(),
        "a view not opened for text refuses it"
    );
    let (reserved, preparation, frame) = (
        view.reserved_bytes(),
        view.preparation_limit(),
        view.frame_bytes(),
    );
    let without = view.draws().expect("draws").len();
    view.enable_overlay();
    assert!(view.has_overlay());
    let vertex = size_of::<SurfaceVertex>() as u64;
    let mesh = |vertices: u64, triangles: u64| vertices * vertex + triangles * 12;
    assert_eq!(
        view.reserved_bytes() - reserved,
        2 * (HudText::reserved_bytes()
            + mesh(4, 2)
            + mesh(HUD_MAX_VERTICES as u64, HUD_MAX_TRIANGLES as u64))
    );
    assert_eq!(
        view.preparation_limit() - preparation,
        (2 + HUD_MAX_TRIANGLES as u64) * PREPARATION_BYTES_PER_TRIANGLE
    );
    assert_eq!(
        view.frame_bytes() - frame,
        view.preparation_limit() - preparation
            + 2 * (size_of::<ThreeDDraw<'static>>() as u64 + JOB_BYTES_PER_DRAW)
    );
    assert_eq!(
        view.draws().expect("draws").len(),
        without,
        "nothing shown yet"
    );

    view.set_overlay(Some(text.clone())).expect("text");
    assert_eq!(view.overlay(), Some(&text));
    let draws = view.draws().expect("draws");
    assert_eq!(draws.len(), without + 2);
    let plate = surface_vertices(&draws[without]);
    let glyphs = surface_vertices(&draws[without + 1]);
    assert_eq!(plate.len(), 4);
    assert!(
        plate
            .iter()
            .all(|corner| corner.opacity == HUD_PLATE_OPACITY)
    );
    assert_eq!(glyphs.len(), text.vertices().len());
    let origin = hud_origin(view.viewport());
    for (vertex, pixel) in glyphs.iter().zip(text.vertices()) {
        let expected = hud_scene_point(
            view.viewport(),
            [origin[0] + pixel[0], origin[1] + pixel[1]],
        );
        assert_eq!(vertex.point, expected);
    }
    // The ink keeps the padding inside the plate.
    let [[left, top], [right, bottom]] = hud_plate_rect(view.viewport(), &text);
    let padding = HUD_PADDING_POINTS * view.viewport().pixels_per_point();
    for pixel in text.vertices() {
        let [x, y] = [origin[0] + pixel[0], origin[1] + pixel[1]];
        assert!(
            x >= left + padding - 1e-6
                && x <= right - padding + 1e-6
                && y >= top + padding - 1e-6
                && y <= bottom - padding + 1e-6,
            "{pixel:?} at {x},{y} is in the padding of {left},{top} to {right},{bottom}"
        );
    }
    drop(draws);

    // A zoom moves the scene under the text, not the text on the screen.
    let zoomed = view.viewport().with_view([5.0, 5.0], 3.0).expect("zoomed");
    view.set_view(zoomed).expect("zoom");
    let draws = view.draws().expect("draws");
    let moved = surface_vertices(&draws[without + 1]);
    let scale = zoomed.scale();
    for (vertex, pixel) in moved.iter().zip(text.vertices()) {
        let drawable = [
            vertex.point[0] * scale + 240.0 - origin[0],
            135.0 - vertex.point[1] * scale - origin[1],
        ];
        assert!((drawable[0] - pixel[0]).abs() < 1e-9 && (drawable[1] - pixel[1]).abs() < 1e-9);
    }
    drop(draws);
    view.set_overlay(None).expect("none");
    assert_eq!(view.draws().expect("draws").len(), without);
    view.set_overlay(Some(font.text("  ", 26.0).expect("blank")))
        .expect("blank");
    assert_eq!(view.overlay(), None, "a blank text draws nothing");
}

fn srgb8_to_linear(value: u8) -> f64 {
    let encoded = f64::from(value) / 255.0;
    if encoded <= 0.040_45 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb8(value: f64) -> f64 {
    let encoded = if value <= 0.003_130_8 {
        12.92 * value
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    encoded * 255.0
}

#[test]
fn a_frame_with_text_differs_only_under_its_plate() {
    let config = common::small();
    let mut world = common::found(&config);
    common::advance(&mut world, 4);
    let lease = common::epoch(&world);
    let drawable = [480_u32, 270];
    let mut view = WorldView::for_epoch(&lease, drawable, [0, 0, 0]).expect("view");
    view.show(&lease).expect("show");
    let bare = view.render_rgba8(2).expect("a frame");
    view.enable_overlay();
    let em = 80.0;
    let text = font().text("Hg8", em).expect("text");
    view.set_overlay(Some(text.clone())).expect("text");
    let drawn = view.render_rgba8(2).expect("a frame with text");
    assert_eq!(bare.len(), drawn.len());

    let [[left, top], [right, bottom]] = hud_plate_rect(view.viewport(), &text);
    let origin = hud_origin(view.viewport());
    let ink = linear_to_srgb8(0.8);
    let (mut inked, mut dimmed, mut differing) = (0, 0, 0);
    for row in 0..drawable[1] {
        for column in 0..drawable[0] {
            let at = 4 * (row * drawable[0] + column) as usize;
            let (before, after) = (&bare[at..at + 3], &drawn[at..at + 3]);
            let center = [f64::from(column) + 0.5, f64::from(row) + 0.5];
            let under = center[0] > left - 1.0
                && center[0] < right + 1.0
                && center[1] > top - 1.0
                && center[1] < bottom + 1.0;
            if !under {
                assert_eq!(before, after, "pixel {column},{row} outside the plate");
                continue;
            }
            differing += usize::from(before != after);
            let local = [center[0] - origin[0], center[1] - origin[1]];
            let inside_plate = center[0] > left + 1.0
                && center[0] < right - 1.0
                && center[1] > top + 1.0
                && center[1] < bottom - 1.0;
            if !inside_plate || edge_distance(&text, local) < 1.5 {
                continue;
            }
            if winding(&text, local) != 0 {
                for channel in after {
                    assert!(
                        (f64::from(*channel) - ink).abs() <= 1.5,
                        "ink at {column},{row}: {after:?}"
                    );
                }
                inked += 1;
            } else {
                // The plate over the world: its black at its opacity, mixed
                // in linear light.
                for (b, a) in before.iter().zip(after) {
                    let expected = linear_to_srgb8((1.0 - HUD_PLATE_OPACITY) * srgb8_to_linear(*b));
                    assert!(
                        (f64::from(*a) - expected).abs() <= 1.5,
                        "plate at {column},{row}: {before:?} -> {after:?}, {expected:.1} expected"
                    );
                }
                dimmed += 1;
            }
        }
    }
    assert!(inked > 500, "{inked} ink pixels checked");
    assert!(dimmed > 2_000, "{dimmed} plate pixels checked");
    assert!(differing > inked, "{differing} pixels changed");
}

/// FL-019's colour key: each swatch takes the next colour, a swatch without
/// one is drawn as text, and a frame shows each in its colour.
#[test]
fn swatches_are_drawn_in_their_colours() {
    let font = font();
    let coral = [0xFF, 0x6F, 0x59];
    let sky = [0x4E, 0xA8, 0xDE];
    let em = 60.0;
    let text = font
        .text_with_swatches("■a■■", em, &[linear(coral), linear(sky)])
        .expect("text");
    assert_eq!(text.glyphs(), 4);
    let swatches = text.swatches();
    assert_eq!(swatches.len(), 2, "the third swatch has no colour");
    // Each swatch is one square ring; the `a` lies between the first two.
    assert_eq!(swatches[0].0, 0..4);
    assert!(swatches[1].0.start > 4);
    assert_eq!(swatches[1].0.len(), 4);
    let third = swatches[1].0.end..text.vertices().len();
    assert_eq!(third.len(), 4);
    // The same text without colours is the same triangles.
    let plain = font.text("■a■■", em).expect("plain");
    assert_eq!(plain.vertices(), text.vertices());
    assert_eq!(plain.triangles(), text.triangles());
    assert!(plain.swatches().is_empty());

    let config = common::small();
    let mut world = common::found(&config);
    common::advance(&mut world, 4);
    let lease = common::epoch(&world);
    let drawable = [480_u32, 270];
    let mut view = WorldView::for_epoch(&lease, drawable, [0, 0, 0]).expect("view");
    view.show(&lease).expect("show");
    view.enable_overlay();
    view.set_overlay(Some(text.clone())).expect("text");

    // The glyph mesh colours exactly the swatches' vertices.
    let mesh = hud_glyphs(view.viewport(), &text)
        .expect("a mesh")
        .expect("glyphs");
    for (index, vertex) in mesh.vertices().iter().enumerate() {
        let expected = if swatches[0].0.contains(&index) {
            linear(coral)
        } else if swatches[1].0.contains(&index) {
            linear(sky)
        } else {
            HUD_TEXT_COLOR
        };
        assert_eq!(vertex.color, expected, "vertex {index}");
    }

    // In the frame, every pixel well inside a square is its colour.
    let frame = view.render_rgba8(2).expect("a frame");
    let origin = hud_origin(view.viewport());
    let ink = linear_to_srgb8(0.8);
    let expectations = [
        (swatches[0].0.clone(), coral.map(f64::from)),
        (swatches[1].0.clone(), sky.map(f64::from)),
        (third, [ink; 3]),
    ];
    for (range, expected) in expectations {
        let corners = &text.vertices()[range];
        let low = corners
            .iter()
            .fold([f64::INFINITY; 2], |[x, y], c| [x.min(c[0]), y.min(c[1])]);
        let high = corners.iter().fold([f64::NEG_INFINITY; 2], |[x, y], c| {
            [x.max(c[0]), y.max(c[1])]
        });
        let mut checked = 0;
        for row in 0..drawable[1] {
            for column in 0..drawable[0] {
                let local = [
                    f64::from(column) + 0.5 - origin[0],
                    f64::from(row) + 0.5 - origin[1],
                ];
                let inside = local[0] > low[0] + 2.0
                    && local[0] < high[0] - 2.0
                    && local[1] > low[1] + 2.0
                    && local[1] < high[1] - 2.0;
                if !inside {
                    continue;
                }
                let at = 4 * (row * drawable[0] + column) as usize;
                for (channel, want) in frame[at..at + 3].iter().zip(expected) {
                    assert!(
                        (f64::from(*channel) - want).abs() <= 1.5,
                        "{column},{row}: {:?}, {expected:?} expected",
                        &frame[at..at + 3]
                    );
                }
                checked += 1;
            }
        }
        assert!(checked > 400, "{checked} pixels of a square checked");
    }
}
