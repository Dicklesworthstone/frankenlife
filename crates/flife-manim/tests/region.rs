//! FL-022, plan §14.3 ("Every intervention preview shows its units,
//! affected region, and whether it adds/removes energy"): the region a
//! brush would act on, outlined on the world just outside it, in the colour
//! of whether its stroke adds energy or removes it, at a fixed width on
//! screen.

mod common;

use flife_core::CoreError;
use flife_core::ids::OrganismId;
use flife_manim::{
    REGION_ADDS_COLOR, REGION_REMOVES_COLOR, REGION_SEGMENTS, REGION_WIDTH_POINTS, Region,
    RegionShape, Viewport, WorldView, region_outline,
};
use fmn_render::{SurfaceMesh, ThreeDDraw};

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn view(
    extent: [f64; 2],
    drawable: [u32; 2],
    density: f64,
    center: [f64; 2],
    zoom: f64,
) -> Viewport {
    Viewport::fit(extent, drawable)
        .expect("a viewport")
        .with_pixels_per_point(density)
        .expect("a density")
        .with_view(center, zoom)
        .expect("a view")
}

/// The outline's width in world units at `viewport`: its points at the
/// viewport's density.
fn width(viewport: &Viewport) -> f64 {
    REGION_WIDTH_POINTS * viewport.pixels_per_point() / viewport.scale()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

/// The summed areas of a mesh's triangles in the scene's plane, and
/// whether every one winds the same way.
fn area(mesh: &SurfaceMesh) -> (f64, bool) {
    let vertices = mesh.vertices();
    let mut sum = 0.0;
    let mut signs = [false; 2];
    for triangle in mesh.indices().as_chunks::<3>().0 {
        let [a, b, c] = [0, 1, 2].map(|k| vertices[triangle[k] as usize].point);
        let twice = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
        signs[usize::from(twice > 0.0)] = true;
        sum += twice.abs() / 2.0;
    }
    (sum, !(signs[0] && signs[1]))
}

/// A cell's outline: its inner corners are the cell's corners where the
/// viewport draws them, each outer corner is [`REGION_WIDTH_POINTS`] out
/// on both axes, and its eight triangles, wound alike, cover exactly the
/// band around the cell, at fit and zoomed, at one and two pixels a point.
#[test]
fn a_cells_outline_is_a_band_just_outside_it_at_a_fixed_width_on_screen() {
    let extent = [64.0, 48.0];
    let bounds = [[12.0, 13.0], [30.0, 31.0]];
    let center = [12.5, 30.5];
    for viewport in [
        view(extent, [640, 480], 1.0, [32.0, 24.0], 1.0),
        view(extent, [640, 480], 2.0, [32.0, 24.0], 1.0),
        view(extent, [1280, 720], 2.0, center, 16.0),
    ] {
        let region = Region {
            shape: RegionShape::Rectangle(bounds),
            removes: false,
        };
        let mesh = region_outline(&viewport, region).expect("an outline");
        let w = width(&viewport);
        assert!(close(
            w * viewport.scale(),
            1.5 * viewport.pixels_per_point()
        ));
        let [cx, cy, _] = viewport.scene_point(center);
        let vertices = mesh.vertices();
        assert_eq!(vertices.len(), 8);
        assert_eq!(mesh.indices().len(), 8 * 3);
        let mut corners = Vec::new();
        for pair in vertices.as_chunks::<2>().0 {
            let (outer, inner) = (pair[0].point, pair[1].point);
            assert_eq!(pair[0].color, REGION_ADDS_COLOR);
            assert_eq!(pair[1].color, REGION_ADDS_COLOR);
            // The inner corner is one of the cell's, where it is drawn.
            let x = if inner[0] < cx { 12.0 } else { 13.0 };
            let y = if inner[1] > cy { 30.0 } else { 31.0 };
            let drawn = viewport.scene_point([x, y]);
            assert!(
                close(inner[0], drawn[0]) && close(inner[1], drawn[1]),
                "{inner:?} {drawn:?}"
            );
            corners.push([x, y].map(f64::to_bits));
            // The outer corner is w further out on both axes.
            for axis in 0..2 {
                let out = (inner[axis] - [cx, cy][axis]).signum();
                assert!(
                    close(outer[axis], inner[axis] + out * w),
                    "{outer:?} {inner:?} {w}"
                );
            }
        }
        corners.sort_unstable();
        corners.dedup();
        assert_eq!(corners.len(), 4, "every corner of the cell");
        let [cw, ch] = [1.0, 1.0];
        let (covered, alike) = area(&mesh);
        assert!(alike, "the triangles wind alike");
        assert!(
            close(covered, (cw + 2.0 * w) * (ch + 2.0 * w) - cw * ch),
            "{covered} is the band's area"
        );
    }
}

/// A removal's outline: an annulus [`REGION_WIDTH_POINTS`] wide from the
/// disc's edge outward, in the removal's colour, its triangles covering
/// the band between the two polygons.
#[test]
fn a_discs_outline_is_an_annulus_from_its_edge_in_the_removals_colour() {
    let extent = [64.0, 48.0];
    let center = [40.25, 10.75];
    let radius = 2.5;
    for viewport in [
        view(extent, [640, 480], 1.0, [32.0, 24.0], 1.0),
        view(extent, [1280, 720], 2.0, center, 8.0),
    ] {
        let region = Region {
            shape: RegionShape::Disc { center, radius },
            removes: true,
        };
        let mesh = region_outline(&viewport, region).expect("an outline");
        let w = width(&viewport);
        let [cx, cy, _] = viewport.scene_point(center);
        let vertices = mesh.vertices();
        assert_eq!(vertices.len(), 2 * REGION_SEGMENTS);
        assert_eq!(mesh.indices().len(), 2 * REGION_SEGMENTS * 3);
        for (at, vertex) in vertices.iter().enumerate() {
            let distance = (vertex.point[0] - cx).hypot(vertex.point[1] - cy);
            let expected = if at % 2 == 0 { radius + w } else { radius };
            assert!(
                close(distance, expected),
                "vertex {at}: {distance} not {expected}"
            );
            assert_eq!(vertex.color, REGION_REMOVES_COLOR);
        }
        let n = REGION_SEGMENTS as f64;
        let polygon = |r: f64| n / 2.0 * r * r * (std::f64::consts::TAU / n).sin();
        let (covered, alike) = area(&mesh);
        assert!(alike, "the triangles wind alike");
        assert!(
            close(covered, polygon(radius + w) - polygon(radius)),
            "{covered}"
        );
    }
}

/// A region that is not one is refused: an empty or inverted rectangle, a
/// radius that is not positive, anything not finite.
#[test]
fn a_region_that_is_not_one_is_refused() {
    let viewport = view([64.0, 48.0], [640, 480], 1.0, [32.0, 24.0], 1.0);
    let shapes = [
        RegionShape::Rectangle([[3.0, 3.0], [1.0, 2.0]]),
        RegionShape::Rectangle([[3.0, 4.0], [2.0, 1.0]]),
        RegionShape::Rectangle([[f64::NAN, 4.0], [1.0, 2.0]]),
        RegionShape::Rectangle([[3.0, f64::INFINITY], [1.0, 2.0]]),
        RegionShape::Disc {
            center: [1.0, 1.0],
            radius: 0.0,
        },
        RegionShape::Disc {
            center: [1.0, 1.0],
            radius: -1.0,
        },
        RegionShape::Disc {
            center: [1.0, 1.0],
            radius: f64::NAN,
        },
        RegionShape::Disc {
            center: [f64::INFINITY, 1.0],
            radius: 1.0,
        },
    ];
    for shape in shapes {
        let outline = region_outline(
            &viewport,
            Region {
                shape,
                removes: false,
            },
        );
        assert!(
            matches!(outline, Err(CoreError::NumericalInvalid { .. })),
            "{shape:?}: {outline:?}"
        );
    }
}

/// The view outlines its region over the organisms and under the selection
/// ring, keeps it through a pan, a zoom and a new pixel density (rebuilt
/// at its width on screen), and drops it for none. Setting the same region
/// again changes nothing; a refused region leaves the view as it was.
#[test]
fn the_view_outlines_its_region_over_the_organisms_and_under_the_ring() {
    let config = common::small();
    let world = common::found(&config);
    let epoch = common::epoch(&world);
    let mut view_ = WorldView::for_epoch(&epoch, [640, 640], [0, 0, 0]).expect("a view");
    view_.show(&epoch).expect("show");
    let bare = view_.draws().expect("draws").len();
    assert_eq!(view_.region(), None);
    assert!(view_.region_draw().is_none());

    let cell = Region {
        shape: RegionShape::Rectangle([[2.0, 3.0], [5.0, 6.0]]),
        removes: false,
    };
    assert!(view_.set_region(Some(cell)).expect("a region"));
    assert!(!view_.set_region(Some(cell)).expect("the same region"));
    assert_eq!(view_.region(), Some(cell));
    let outline = |view_: &WorldView| -> Vec<[f64; 3]> {
        match view_.region_draw() {
            Some(ThreeDDraw::Surface(surface)) => {
                surface.mesh.vertices().iter().map(|v| v.point).collect()
            }
            _ => panic!("no region draw"),
        }
    };
    let expected = |view_: &WorldView, region: Region| -> Vec<[f64; 3]> {
        region_outline(view_.viewport(), region)
            .expect("an outline")
            .vertices()
            .iter()
            .map(|v| v.point)
            .collect()
    };
    {
        let draws = view_.draws().expect("draws");
        assert_eq!(draws.len(), bare + 1);
        let Some(ThreeDDraw::Surface(last)) = draws.last() else {
            panic!("the region is the last draw without a selection");
        };
        assert_eq!(last.mesh.vertices().len(), 8);
        assert!(!last.depth_test, "over everything drawn before it");
    }
    assert_eq!(outline(&view_), expected(&view_, cell));

    // With a selection, the ring is drawn over the region.
    let first = OrganismId::new(epoch.ids()[0]);
    view_.select(Some(first)).expect("select");
    {
        let draws = view_.draws().expect("draws");
        assert_eq!(draws.len(), bare + 2);
        let Some(ThreeDDraw::Surface(region)) = draws.get(draws.len() - 2) else {
            panic!("the region is under the ring");
        };
        assert_eq!(region.mesh.vertices().len(), 8);
    }
    view_
        .job()
        .expect("FrankenManim takes the frame with its region and ring");

    // A zoom and a new density rebuild the outline at its width on screen.
    let fitted = outline(&view_);
    let zoomed = view_.viewport().with_view([2.5, 5.5], 8.0).expect("a view");
    assert!(view_.set_view(zoomed).expect("looks"));
    assert_eq!(view_.region(), Some(cell));
    assert_ne!(outline(&view_), fitted);
    assert_eq!(outline(&view_), expected(&view_, cell));
    assert!(view_.set_pixels_per_point(2.0).expect("a density"));
    assert_eq!(outline(&view_), expected(&view_, cell));

    // A removal's disc replaces it; a refused region changes nothing.
    let disc = Region {
        shape: RegionShape::Disc {
            center: [2.5, 5.5],
            radius: 0.75,
        },
        removes: true,
    };
    assert!(view_.set_region(Some(disc)).expect("a disc"));
    assert_eq!(outline(&view_).len(), 2 * REGION_SEGMENTS);
    let refused = Region {
        shape: RegionShape::Disc {
            center: [2.5, 5.5],
            radius: 0.0,
        },
        removes: true,
    };
    assert!(view_.set_region(Some(refused)).is_err());
    assert_eq!(view_.region(), Some(disc));
    assert_eq!(outline(&view_), expected(&view_, disc));
    view_
        .job()
        .expect("FrankenManim takes the frame with its disc");

    // None removes it.
    assert!(view_.set_region(None).expect("none"));
    assert!(!view_.set_region(None).expect("none again"));
    assert!(view_.region_draw().is_none());
    assert_eq!(view_.draws().expect("draws").len(), bare + 1);
}

/// On the CPU route, the outline is drawn: just outside a cell's left edge
/// the frame turns amber, just outside a disc's right edge red, and inside
/// both the frame is the frame without them; a new region leaves no trace
/// of the old.
#[test]
fn a_rendered_frame_outlines_the_region_and_leaves_its_inside() {
    let config = common::small();
    let world = common::found(&config);
    let epoch = common::epoch(&world);
    let mut view_ = WorldView::for_epoch(&epoch, [400, 400], [0, 0, 0]).expect("a view");
    view_.set_pixels_per_point(2.0).expect("a density");
    let zoomed = view_.viewport().with_view([4.0, 4.0], 4.0).expect("a view");
    view_.set_view(zoomed).expect("looks");
    view_.show(&epoch).expect("show");
    let plain = view_.render_rgba8(2).expect("a frame");
    let [width_px, _] = view_.viewport().drawable();
    let pixel = |rgba: &[u8], at: [f64; 2]| -> [u8; 3] {
        let [x, y] = at.map(|value| value.floor() as usize);
        let index = 4 * (y * width_px as usize + x);
        [rgba[index], rgba[index + 1], rgba[index + 2]]
    };
    // The band is 3 pixels wide at 2 pixels a point: its middle pixel lies
    // wholly inside it.
    let band = REGION_WIDTH_POINTS * 2.0;

    let cell = Region {
        shape: RegionShape::Rectangle([[3.0, 4.0], [3.0, 4.0]]),
        removes: false,
    };
    view_.set_region(Some(cell)).expect("a region");
    let outlined = view_.render_rgba8(2).expect("a frame");
    let edge = view_.viewport().world_to_drawable([3.0, 3.5]);
    let [r, g, b] = pixel(&outlined, [edge[0] - band / 2.0, edge[1]]);
    // Amber at 0.9 over anything: linear green at least 0.56, blue at most
    // 0.21, red above both.
    assert!(
        r > g && g > b && g >= 150,
        "amber outside the left edge: {:?}",
        [r, g, b]
    );
    let inside = view_.viewport().world_to_drawable([3.5, 3.5]);
    assert_eq!(
        pixel(&outlined, inside),
        pixel(&plain, inside),
        "the cell's inside"
    );

    let disc = Region {
        shape: RegionShape::Disc {
            center: [5.0, 5.0],
            radius: 0.5,
        },
        removes: true,
    };
    view_.set_region(Some(disc)).expect("a disc");
    let removing = view_.render_rgba8(2).expect("a frame");
    let right = view_.viewport().world_to_drawable([5.5, 5.0]);
    let [r, g, b] = pixel(&removing, [right[0] + band / 2.0, right[1]]);
    // Red at 0.9 over anything: linear red at least 0.9, green and blue at
    // most 0.21.
    assert!(
        r >= 200 && g <= 140 && b <= 140,
        "red outside the disc: {:?}",
        [r, g, b]
    );
    let middle = view_.viewport().world_to_drawable([5.0, 5.0]);
    assert_eq!(
        pixel(&removing, middle),
        pixel(&plain, middle),
        "the disc's inside"
    );
    assert_eq!(
        pixel(&removing, [edge[0] - band / 2.0, edge[1]]),
        pixel(&plain, [edge[0] - band / 2.0, edge[1]]),
        "the cell's outline is gone"
    );
}
