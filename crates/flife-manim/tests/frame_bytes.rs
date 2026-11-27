//! FL-043: a frame's preparation is bounded. Over real worlds at their
//! population ceiling and part of it, and one so small its grids dominate,
//! every style, sizes from 640 x 360 to
//! 3840 x 2160, one and two pixels per point, and zooms to 64x,
//! FrankenManim's own accounting of an unlimited preparation is within the
//! view's limit, so the limit leaves nothing out of these frames, and the
//! job compiles within it.

mod common;

use flife_manim::{
    ColorMapping, FieldMapping, Look, PREPARATION_BYTES_FIXED, PREPARATION_BYTES_PER_DOT,
    Particles, Style, WorldView,
};
use fmn_render::{ThreeDDraw, ThreeDJob, Tiling};

/// The dots and halos among `draws`: a true dot each, or a cloud's dots.
fn particles(draws: &[ThreeDDraw<'_>]) -> usize {
    draws
        .iter()
        .map(|draw| match draw {
            ThreeDDraw::DotCloud(cloud) => cloud.dots.len(),
            ThreeDDraw::TrueDot(_) => 1,
            ThreeDDraw::Surface(_) | ThreeDDraw::Vector(_) => 0,
        })
        .sum()
}

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
                        // The most a frame draws besides: a selection's ring
                        // and a brush's disc (FL-022).
                        let first = flife_core::ids::OrganismId::new(epoch.ids()[0]);
                        assert_eq!(view.select(Some(first)).expect("selects"), Some(first));
                        let extent = epoch.extent();
                        let disc = flife_manim::Region {
                            shape: flife_manim::RegionShape::Disc {
                                center: [extent[0] / 2.0, extent[1] / 2.0],
                                radius: extent[0] / 16.0,
                            },
                            effect: flife_manim::RegionEffect::Removes,
                        };
                        assert!(view.set_region(Some(disc)).expect("a region"));
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
/// its ceiling, the triangles of the field's and the density's quads, the
/// ring and a brush's region, and the fixed part, whatever the field or the
/// bins. A view of
/// a world at a quarter of its ceiling is charged the whole ceiling. The
/// layers' textures are not preparation: `texel_bytes` counts them, the
/// field's once the view knows it and the density's at the viewport's
/// bins, and the view reserves each twice, and once the copy of the field's
/// values (FL-033) and compositions (FL-019) its texture was built from,
/// sixteen bytes a texel.
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
    let region = 2 * flife_manim::REGION_SEGMENTS as u64;
    let limit = 2 * 4096 * PREPARATION_BYTES_PER_DOT
        + (2 + 2 + ring + region) * flife_manim::PREPARATION_BYTES_PER_TRIANGLE
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
        2 * x * y * 16 + x * y * (4 + 12),
        "and is reserved twice, its values and compositions once"
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
    // view can make (a dot and a halo each, the field, the density, a
    // brush's region and the ring), and the limit.
    let draws = 2 * 4096 + 4;
    assert_eq!(
        view.frame_bytes(),
        draws * size_of::<fmn_render::ThreeDDraw<'static>>() as u64
            + draws * flife_manim::JOB_BYTES_PER_DRAW
            + limit
    );
}

/// FL-033: a frame whose dots are compiled on threads is the frame compiled
/// on one. A view of 4,096 organisms draws a run of 8,192 halos and dots,
/// long enough for FrankenManim to compile on worker threads. In every
/// style, at the fit and at 8x, with a selection's ring and a removal's
/// disc, the job on 8 threads must equal the job on the default one: every
/// draw, bin and count as FrankenManim prints them, and the preparation
/// bytes. In the points style the rendered frames must match bit for bit.
#[test]
fn a_frame_prepared_on_threads_is_the_frame_prepared_on_one() {
    let config = common::config(&[
        ("population", "initial_count", "4096"),
        ("population", "maximum_count", "4096"),
    ]);
    let world = common::found(&config);
    let epoch = common::epoch(&world);
    let eight = std::num::NonZeroUsize::new(8).expect("eight");
    let mut compared = 0;
    for zoom in [1.0, 8.0] {
        for (name, style) in styles() {
            let mut view = WorldView::for_epoch(&epoch, [640, 360], [0, 0, 0]).expect("a view");
            assert_eq!(view.preparation_threads(), std::num::NonZeroUsize::MIN);
            view.set_style(style);
            view.show(&epoch).expect("shows");
            let first = flife_core::ids::OrganismId::new(epoch.ids()[0]);
            assert_eq!(view.select(Some(first)).expect("selects"), Some(first));
            let extent = epoch.extent();
            let disc = flife_manim::Region {
                shape: flife_manim::RegionShape::Disc {
                    center: [extent[0] / 2.0, extent[1] / 2.0],
                    radius: extent[0] / 16.0,
                },
                effect: flife_manim::RegionEffect::Removes,
            };
            assert!(view.set_region(Some(disc)).expect("a region"));
            if zoom > 1.0 {
                let viewport = view
                    .viewport()
                    .zoomed_about([0.37, 0.61], zoom)
                    .expect("zooms");
                view.set_view(viewport).expect("looks");
                view.place(&epoch).expect("places");
            }
            // A job's record: everything it prints before it renders (its
            // tile scratch then depends on the render's threads) but the
            // threads it was prepared on, the one field meant to differ (its
            // clouds follow it); those threads; its preparation bytes and,
            // in the points style, its frame.
            let record = |view: &WorldView| {
                let job = view.job().expect("a job");
                let threads = job.preparation_threads();
                let printed = format!("{job:?}");
                let field = format!(", threads: {threads}, clouds: ");
                assert_eq!(printed.matches(&field).count(), 1, "{name}: {field}");
                let printed = printed.replacen(&field, ", clouds: ", 1);
                let frame =
                    (name == "points").then(|| job.render(2).expect("a frame").as_bytes().to_vec());
                (printed, threads, job.preparation_bytes(), frame)
            };
            let one = record(&view);
            view.set_preparation_threads(eight);
            assert_eq!(view.preparation_threads(), eight);
            let many = record(&view);
            assert!(many.0 == one.0, "{name} at zoom {zoom}: the jobs differ");
            assert_eq!((one.1, many.1), (1, 8), "{name} at zoom {zoom}");
            assert_eq!(many.2, one.2, "{name} at zoom {zoom}");
            assert_eq!(many.3.is_some(), name == "points");
            assert!(many.3 == one.3, "{name} at zoom {zoom}: the frames differ");
            compared += 1;
        }
    }
    assert_eq!(compared, 2 * 4);
}

/// FL-033: conservative frustum culling in `WorldView::draws()` at zoom > 1
/// drops offscreen dots and halos without changing a single byte of the
/// rendered frame (including bloom, halos straddling the viewport edge, seam
/// views, and all three looks), while unplaced `set_view` leaves the draw list
/// intact.
#[test]
fn zoomed_offscreen_culling_is_bit_identical_to_unculled_rendering() {
    let config = common::config(&[
        ("population", "initial_count", "1024"),
        ("population", "maximum_count", "1024"),
    ]);
    let mut world = common::found(&config);
    common::advance(&mut world, 8);
    let epoch = common::epoch(&world);
    let drawable = [320, 180];

    for (look, bloom) in [
        (Look::Glow, Some(0.15)),
        (Look::Glow, None),
        (Look::Raw, None),
        (Look::Sphere, None),
    ] {
        let style = Style {
            look,
            ..Style::default()
        };
        for (center, zoom) in [
            ([12.0, 20.0], 2.0),
            ([0.1, 127.9], 4.0),
            ([64.0, 64.0], 16.0),
        ] {
            let mut view = WorldView::for_epoch(&epoch, drawable, [2, 3, 8]).expect("a view");
            view.set_style(style);
            view.set_bloom(bloom).expect("bloom");
            view.show(&epoch).expect("show");

            // Unplaced `set_view` must not cull stale positions.
            let full_particles = view.cloud().halos().dots.len() + view.cloud().draws().dots.len();
            assert_eq!(particles(&view.draws().expect("draws")), full_particles);
            let zoomed = view.viewport().with_view(center, zoom).expect("zoomed");
            view.set_view(zoomed).expect("set_view");
            assert_eq!(
                particles(&view.draws().expect("unplaced draws")),
                full_particles,
                "unplaced set_view must keep all draws"
            );

            // Placing into the zoomed view activates frustum culling.
            view.place(&epoch).expect("place");
            let first = flife_core::ids::OrganismId::new(epoch.ids()[0]);
            view.select(Some(first)).expect("select");
            let culled_draws = view.draws().expect("culled draws");
            let full_particles = view.cloud().halos().dots.len() + view.cloud().draws().dots.len();
            let culled_particles = particles(&culled_draws);
            if zoom >= 4.0 {
                assert!(
                    culled_particles * 4 < full_particles,
                    "{look:?} zoom {zoom}: expected >75% culled, kept {culled_particles}/{full_particles}"
                );
            }
            if zoom >= 16.0 {
                assert!(
                    culled_particles * 20 < full_particles,
                    "{look:?} zoom {zoom}: expected >95% culled, kept {culled_particles}/{full_particles}"
                );
            }
            // Build an unculled reference job directly from the cloud's halos
            // and dots, as the unculled view draws them: as clouds.
            let mut unculled = Vec::new();
            unculled.extend(view.field().draw());
            unculled.push(ThreeDDraw::DotCloud(view.cloud().halos()));
            unculled.push(ThreeDDraw::DotCloud(view.cloud().draws()));
            unculled.extend(view.region_draw());
            unculled.extend(view.selection_draw());
            let mut ref_job = ThreeDJob::new(view.camera().camera(), &unculled, Tiling::default())
                .expect("reference job");
            if let Some(b) = view.frame_bloom() {
                ref_job = ref_job.with_bloom(b.clone());
            }
            let ref_frame = ref_job.render(2).expect("reference frame");
            let mut ref_rgba = vec![0u8; (drawable[0] * drawable[1] * 4) as usize];
            fmn_frame::convert::rgba16f_to_rgba8_slice(&ref_frame, &mut ref_rgba)
                .expect("convert reference");

            let culled_rgba = view.render_rgba8(2).expect("culled frame");
            assert_eq!(
                culled_rgba, ref_rgba,
                "{look:?} bloom {bloom:?} center {center:?} zoom {zoom}: culled frame differs from unculled"
            );
        }
    }
}

/// Planted-negative edge-straddling test: an organism whose center sits outside
/// the viewport while its glow halo reaches inside must NEVER be culled, and
/// must render byte-identically to the unculled frame.
#[test]
fn offscreen_center_with_onscreen_halo_is_preserved_bit_for_bit() {
    let config = common::small();
    let world = common::found(&config);
    let epoch = common::epoch(&world);
    let pos = epoch.positions()[0].map(f64::from);
    let drawable = [160, 160];
    let mut view = WorldView::for_epoch(&epoch, drawable, [0, 0, 0]).expect("view");
    // Zoom 8x: frame width is 32 / 8 = 4 world units (half-width = 2.0).
    // Place the viewport center 2.15 world units east of organism 0 so its
    // center is outside the left edge (-2.15 < -2.0), while its glow halo
    // (radius ~0.35 * 3.5 = 1.225) reaches well inside the frame!
    let center = [(pos[0] + 2.15).rem_euclid(32.0), pos[1]];
    let zoomed = view.viewport().with_view(center, 8.0).expect("zoomed");
    view.set_view(zoomed).expect("set_view");
    view.show(&epoch).expect("show");

    let mut unculled = Vec::new();
    unculled.extend(view.field().draw());
    unculled.push(ThreeDDraw::DotCloud(view.cloud().halos()));
    unculled.push(ThreeDDraw::DotCloud(view.cloud().draws()));
    let ref_frame = ThreeDJob::new(view.camera().camera(), &unculled, Tiling::default())
        .expect("ref job")
        .render(2)
        .expect("ref frame");
    let mut ref_rgba = vec![0u8; (drawable[0] * drawable[1] * 4) as usize];
    fmn_frame::convert::rgba16f_to_rgba8_slice(&ref_frame, &mut ref_rgba).expect("convert");

    let culled_rgba = view.render_rgba8(2).expect("culled frame");
    assert_eq!(
        culled_rgba, ref_rgba,
        "edge-straddling halo must render byte-identically"
    );
}

/// Regression for `flife-ovd`: an un-culled (`zoom == 1.0`) glow frame at
/// `2^19 = 524,288` organisms with the field drawn produces `1,048,577` draw
/// rows (524,288 halos + 524,288 dots + 1 field quad), exceeding
/// FrankenManim's default `ThreeDPreparationLimits::max_draws` (`1 << 20 =
/// 1,048,576`). `WorldView::job` must size `max_draws` and
/// `max_raster_triangles` from the compiled frame / budgeted geometry while
/// keeping `max_working_bytes = preparation_limit()`.
#[test]
fn glow_frame_at_half_million_organisms_with_field_compiles_past_default_draw_cap() {
    let count = 1_usize << 19; // 524,288 organisms
    let count_str = count.to_string();
    let config = common::config(&[
        ("world", "width", "16.0"),
        ("world", "height", "16.0"),
        ("world", "grid_width", "16"),
        ("world", "grid_height", "16"),
        ("population", "initial_count", &count_str),
        ("population", "maximum_count", &count_str),
        ("controller", "hidden_widths", "[8]"),
        ("controller", "canonical_weight_dtype", "\"f16\""),
        ("architecture_mutation", "enabled", "false"),
        ("execution", "cpu_workers", "8"),
        ("execution", "maximum_logical_genome_bytes", "2147483648"),
        ("execution", "maximum_allocated_genome_bytes", "2147483648"),
        ("execution", "maximum_total_bytes", "8589934592"),
    ]);
    let world = common::found(&config);
    let epoch = flife_render::Epoch::standalone(&world).expect("standalone epoch");
    assert_eq!(epoch.len(), count);

    let mut view = WorldView::for_epoch(&epoch, [640, 360], [0, 0, 0]).expect("a view");
    let threads = std::num::NonZeroUsize::new(8).expect("eight");
    view.set_preparation_threads(threads);
    view.set_style(Style::default());
    view.show(&epoch).expect("shows");

    // FrankenManim's draw rows: one a dot of a cloud, and the field's.
    let draws = view.draws().expect("draws");
    let rows: usize = draws
        .iter()
        .map(|draw| match draw {
            ThreeDDraw::DotCloud(cloud) => cloud.dots.len().max(1),
            ThreeDDraw::TrueDot(_) | ThreeDDraw::Surface(_) | ThreeDDraw::Vector(_) => 1,
        })
        .sum();
    assert_eq!(
        (draws.len(), rows),
        (3, 2 * count + 1),
        "glow look at 2^19 organisms + field: the field and two clouds, 1,048,577 draw rows \
         (> 1 << 20)"
    );
    assert!(rows > (1_usize << 20));

    let job = view
        .job()
        .expect("WorldView::job must compile un-culled 2^19 glow frame with field drawn");
    assert!(
        job.preparation_bytes() <= view.preparation_limit(),
        "compiled preparation {} exceeded limit {}",
        job.preparation_bytes(),
        view.preparation_limit()
    );
}

/// The SHA-256 of each [`pinned_frames`] frame's RGBA8 bytes on the CPU
/// route: the frames at FrankenManim `d1e1757b`, when every dot was a true
/// dot, and at `48e4e2ee` (FL-033's repin), where the view draws its halos
/// and dots as clouds. A FrankenManim repin that changes how FrankenLife
/// looks fails here; one meant to change it updates these with the reason
/// in its commit.
const PINNED_FRAME_DIGESTS: [(&str, &str); 10] = [
    (
        "glow, bloom 0.15",
        "8d2282fcd82e0e996499457f8b1e355dd602af99006c6a2b6df62aa0ae5fa48b",
    ),
    (
        "glow",
        "8f882aac7025b05b9589f5f3c9c450f209e0a086eaebe088dc1dc42377efef45",
    ),
    (
        "raw",
        "54ed9436aa9ed79fd6834a145f03648ea5ce9547a956347dc9e6f117959ec04b",
    ),
    (
        "sphere",
        "b1b6a2662ad5b8b3a95d81eb45a8b590cb4b6c0aeac23f31d7122e8ccfe172da",
    ),
    (
        "density",
        "bd2b7a64fc1c2419fe5f1ba4bd34be0e1a15cc1b8084ae60bf40cec3f5744ce2",
    ),
    (
        "raw density",
        "c835090100f5c25ae363ffd907519f64b9e1d9699fbb5881ae767083edccbb68",
    ),
    (
        "no field",
        "15659575ffe00d91dfafdd20ab1ee88339a9e01efb56f67a6e05ef6d788cc0c6",
    ),
    (
        "composition field",
        "b2a3d412de6bee351c5edff26377036d38adc1041064d81642f3876a52e89026",
    ),
    (
        "energy",
        "ee0675a9a136ed520849dde0adb8f683b647b58241ef7944ef425c55bab866a6",
    ),
    (
        "glow, zoom 4, selected",
        "fb5d3f0628c0159a260a3405091cbe962d5e531e09fe406790feaaa899ba8dd0",
    ),
];

/// [`PINNED_FRAME_DIGESTS`]' frames: 1,024 organisms after 8 ticks at
/// 320 x 180 in the default lineage mapping, in each look, the density view,
/// without the field, with the composition field, in the energy mapping, and
/// zoomed with the first organism selected: dots, halos, bloom, the field's
/// texture, the density texture and the selection ring. The frames at the
/// fit draw the dots as clouds; the zoomed frame, culled, as true dots.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure here is a test failure"
)]
fn pinned_frames() -> Vec<(&'static str, String)> {
    let config = common::config(&[
        ("population", "initial_count", "1024"),
        ("population", "maximum_count", "1024"),
    ]);
    let mut world = common::found(&config);
    common::advance(&mut world, 8);
    let epoch = common::epoch(&world);
    let raw = Style {
        look: Look::Raw,
        ..Style::default()
    };
    let cases = [
        ("glow, bloom 0.15", Style::default(), Some(0.15)),
        ("glow", Style::default(), None),
        ("raw", raw, None),
        (
            "sphere",
            Style {
                look: Look::Sphere,
                ..Style::default()
            },
            None,
        ),
        (
            "density",
            Style {
                particles: Particles::Density,
                ..Style::default()
            },
            None,
        ),
        (
            "raw density",
            Style {
                particles: Particles::Density,
                ..raw
            },
            None,
        ),
        (
            "no field",
            Style {
                field: false,
                ..Style::default()
            },
            None,
        ),
        (
            "composition field",
            Style {
                field_mapping: FieldMapping::Composition,
                ..Style::default()
            },
            None,
        ),
        (
            // The default mapping is lineage; this is the energy ramp.
            "energy",
            Style {
                mapping: ColorMapping::Energy,
                ..Style::default()
            },
            None,
        ),
    ];
    let digest = |view: &WorldView| {
        flife_core::Sha256Digest::of(&view.render_rgba8(2).expect("a frame")).to_hex()
    };
    let mut frames = Vec::new();
    for (label, style, bloom) in cases {
        let mut view = WorldView::for_epoch(&epoch, [320, 180], [2, 3, 8]).expect("a view");
        view.set_style(style);
        view.set_bloom(bloom).expect("bloom");
        view.show(&epoch).expect("show");
        frames.push((label, digest(&view)));
    }
    let mut view = WorldView::for_epoch(&epoch, [320, 180], [2, 3, 8]).expect("a view");
    view.show(&epoch).expect("show");
    let zoomed = view
        .viewport()
        .with_view([64.0, 64.0], 4.0)
        .expect("zoomed");
    view.set_view(zoomed).expect("set_view");
    view.place(&epoch).expect("place");
    let first = flife_core::ids::OrganismId::new(epoch.ids()[0]);
    view.select(Some(first)).expect("select");
    frames.push(("glow, zoom 4, selected", digest(&view)));
    frames
}

#[test]
#[expect(
    clippy::print_stdout,
    reason = "test: every digest is printed before the comparison, for a repin's record"
)]
fn frames_keep_their_pinned_digests() {
    let frames = pinned_frames();
    for (label, digest) in &frames {
        println!("frame digest {label}: {digest}");
    }
    let pinned: Vec<(&str, String)> = PINNED_FRAME_DIGESTS
        .iter()
        .map(|&(label, digest)| (label, digest.to_owned()))
        .collect();
    assert_eq!(frames, pinned);
}
