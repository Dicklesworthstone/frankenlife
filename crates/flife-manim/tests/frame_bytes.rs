//! FL-043: a frame's preparation is bounded. Over real worlds at their
//! population ceiling and part of it, and one so small its grids dominate,
//! every style, sizes from 640 x 360 to
//! 3840 x 2160, one and two pixels per point, and zooms to 64x,
//! FrankenManim's own accounting of an unlimited preparation is within the
//! view's limit, so the limit leaves nothing out of these frames, and the
//! job compiles within it.

mod common;

use flife_manim::{
    Look, PREPARATION_BYTES_FIXED, PREPARATION_BYTES_PER_DOT, Particles, Style, WorldView,
};
use fmn_render::{ThreeDJob, Tiling};

fn styles() -> [(&'static str, Style); 4] {
    [
        ("points", Style::default()),
        (
            "points, no field",
            Style {
                field: false,
                ..Style::default()
            },
        ),
        (
            "density",
            Style {
                particles: Particles::Density,
                ..Style::default()
            },
        ),
        (
            "raw density",
            Style {
                particles: Particles::Density,
                look: Look::Raw,
                ..Style::default()
            },
        ),
    ]
}

#[test]
#[expect(
    clippy::print_stdout,
    clippy::cast_precision_loss,
    reason = "test: a failure is a test failure; figures are printed"
)]
fn a_frames_preparation_is_within_the_views_limit() {
    let mut frames = 0;
    let mut closest = 0.0_f64;
    let mut gridded = 0.0_f64;
    for (live, ceiling) in [("1024", "16384"), ("16384", "16384"), ("64", "64")] {
        let config = common::config(&[
            ("population", "initial_count", live),
            ("population", "maximum_count", ceiling),
        ]);
        let world = common::found(&config);
        let epoch = common::epoch(&world);
        for drawable in [[640, 360], [1920, 1080], [3840, 2160]] {
            for density in [1.0, 2.0] {
                for zoom in [1.0, 8.0, 64.0] {
                    for (name, style) in styles() {
                        let mut view =
                            WorldView::for_epoch(&epoch, drawable, [0, 0, 0]).expect("a view");
                        view.set_pixels_per_point(density).expect("a density");
                        view.set_style(style);
                        view.show(&epoch).expect("shows");
                        if zoom > 1.0 {
                            let viewport = view
                                .viewport()
                                .zoomed_about([0.37, 0.61], zoom)
                                .expect("zooms");
                            view.set_view(viewport).expect("looks");
                            view.place(&epoch).expect("places");
                        }
                        let draws = view.draws().expect("draws");
                        let unlimited =
                            ThreeDJob::new(view.camera().camera(), &draws, Tiling::default())
                                .expect("compiles")
                                .preparation_bytes();
                        let limit = view.preparation_limit();
                        let held = view.job().expect("compiles within the limit");
                        assert!(
                            unlimited <= limit,
                            "{live} live, {drawable:?} at {density}x, zoom {zoom}, {name}: \
                             {unlimited} > {limit}"
                        );
                        assert_eq!(held.preparation_bytes(), unlimited, "nothing left out");
                        closest = closest.max(unlimited as f64 / limit as f64);
                        if ceiling == "64" {
                            gridded = gridded.max(unlimited as f64 / limit as f64);
                        }
                        frames += 1;
                    }
                }
            }
        }
    }
    println!(
        "{frames} frames; the fullest used {:.1}% of its limit, {:.1}% with a ceiling of 64",
        closest * 100.0,
        gridded * 100.0
    );
    assert_eq!(frames, 3 * 3 * 2 * 3 * 4);
}

/// The limit is what the view can draw: a dot and a halo per organism of
/// its ceiling, the triangles of the field's and the density's quads and
/// the ring, and the fixed part, whatever the field or the bins. A view of
/// a world at a quarter of its ceiling is charged the whole ceiling. The
/// layers' textures are not preparation: `texel_bytes` counts them, the
/// field's once the view knows it and the density's at the viewport's
/// bins, and the view reserves each twice.
#[test]
fn the_limit_counts_the_ceiling_and_the_quads_and_the_texels_the_textures() {
    let config = common::config(&[
        ("population", "initial_count", "1024"),
        ("population", "maximum_count", "4096"),
    ]);
    let world = common::found(&config);
    let epoch = common::epoch(&world);
    let capacity = epoch.capacity();
    let mut view = WorldView::new(epoch.extent(), capacity, [640, 360], [0, 0, 0]).expect("a view");
    let ring = 2 * flife_manim::SELECTION_RING_SEGMENTS as u64;
    let limit = 2 * 4096 * PREPARATION_BYTES_PER_DOT
        + (2 + 2 + ring) * flife_manim::PREPARATION_BYTES_PER_TRIANGLE
        + PREPARATION_BYTES_FIXED;
    assert_eq!(capacity, 4096);
    assert_eq!(view.preparation_limit(), limit);
    // Before an epoch: no field texture yet; the density's at the
    // viewport's bins.
    let [columns, rows] = flife_manim::density_bins(view.viewport()).map(u64::from);
    assert_eq!(view.texel_bytes(), columns * rows * 16);
    let (reserved, frame) = (view.reserved_bytes(), view.frame_bytes());

    view.show(&epoch).expect("shows");
    let [x, y] = epoch.field().texels().map(u64::from);
    assert_eq!(
        view.preparation_limit(),
        limit,
        "the field adds no preparation"
    );
    assert_eq!(view.frame_bytes(), frame);
    assert_eq!(
        view.texel_bytes(),
        (columns * rows + x * y) * 16,
        "the field's texture counts once shown"
    );
    assert_eq!(
        view.reserved_bytes() - reserved,
        2 * x * y * 16,
        "and is reserved twice"
    );
    let made = WorldView::for_epoch(&epoch, [640, 360], [0, 0, 0]).expect("a view");
    assert_eq!(
        (made.texel_bytes(), made.reserved_bytes()),
        (view.texel_bytes(), view.reserved_bytes()),
        "a view made for an epoch knows its field"
    );
    assert_eq!(made.preparation_limit(), limit);

    // Two pixels per point on the same drawable: bins of twice the
    // pixels, so half as many each way. The texels follow them; the limit
    // does not.
    let (texels, reserved) = (view.texel_bytes(), view.reserved_bytes());
    view.set_pixels_per_point(2.0).expect("a density");
    let [wide, high] = flife_manim::density_bins(view.viewport()).map(u64::from);
    assert_eq!([wide, high], [columns / 2, rows / 2]);
    assert_eq!(view.preparation_limit(), limit);
    let fewer = (columns * rows - wide * high) * 16;
    assert_eq!(texels - view.texel_bytes(), fewer);
    // The bins' counts and colours go too: 4 and 24 bytes a bin.
    assert_eq!(
        reserved - view.reserved_bytes(),
        2 * fewer + (columns * rows - wide * high) * (4 + 24)
    );
    assert!(view.reserved_bytes() > 0);
    // A frame: the draw list and the job's own bytes for every draw the
    // view can make, and the limit.
    let draws = 2 * 4096 + 3;
    assert_eq!(
        view.frame_bytes(),
        draws * size_of::<fmn_render::ThreeDDraw<'static>>() as u64
            + draws * flife_manim::JOB_BYTES_PER_DRAW
            + limit
    );
}
