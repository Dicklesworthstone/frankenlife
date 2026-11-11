//! FL-033: a pan, a zoom or a frame between epochs copies none of the
//! layers' texels to the device, measured headlessly on a Metal device
//! without a window: FrankenManim keeps them there. Its own binary, apart
//! from `frame_upload.rs`, whose heap measurement counts every thread's
//! allocations.

#![allow(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test: a failure is a test failure"
)]

use flife_core::ValidatedConfig;
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_manim::{Look, Particles, Style, WorldView, density_bins};
use flife_render::Epoch;
use fmn_render::metal::MetalRenderer;

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

#[test]
fn a_pan_or_zoom_copies_no_layer_texels_to_the_device() {
    // A pan or zoom keeps the layers' textures (flife-manim), and
    // FrankenManim's Metal route keeps a frame's texels on the device while
    // the next frame samples textures of the same content. So for a pan, a
    // zoom or a frame between epochs, the Studio's renderer copies no layer
    // texels: its upload is a fresh renderer's less exactly the texels of
    // the layers drawn (the field's for points, the field's and the
    // density's, the view's texel bytes, for density, and the density's
    // alone for raw density, which draws no field), and the frames are
    // byte-equal. A later epoch's textures are copied again.
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    // The preset's 4,096 organisms.
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset");
    let mut world = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds");
    // Thirty ticks apart, so the raw density's bin counts change too (three
    // ticks left each bin's count, so its grey texture, bitwise the same,
    // and FrankenManim kept it). The first is thirty ticks in, with a field
    // that varies: the founding field's texels did not change when a pan
    // moved its display scale.
    let advance = |world: &mut World| {
        for _ in 0..30 {
            assert!(matches!(
                world.step().expect("steps"),
                Stepped::Published(_)
            ));
        }
        Epoch::standalone(world).expect("an epoch")
    };
    let epoch = advance(&mut world);
    let later = advance(&mut world);
    assert_eq!(epoch.len(), 4_096, "the preset's organisms");
    let values = epoch.field().values();
    assert!(
        values.iter().any(|value| value != &values[0]),
        "the field varies"
    );
    assert_ne!(values, later.field().values(), "the field moved");
    let mut renderer = MetalRenderer::new().expect("a Metal renderer");
    for style in [
        Style::default(),
        Style {
            particles: Particles::Density,
            ..Style::default()
        },
        Style {
            particles: Particles::Density,
            look: Look::Raw,
            ..Style::default()
        },
    ] {
        let name = format!("{} {:?}", style.particles.name(), style.look);
        let mut view = WorldView::for_epoch(&epoch, [1280, 720], [0, 0, 0]).expect("a view");
        view.set_style(style);
        view.show(&epoch).expect("shows");
        // Premultiplied linear f32 RGBA: 16 bytes a texel.
        let [columns, rows] = density_bins(view.viewport()).map(u64::from);
        let density = 16 * columns * rows;
        let field = view.texel_bytes() - density;
        let layers = match (style.particles, style.look) {
            (Particles::Density, Look::Raw) => density,
            (Particles::Density, Look::Glow) => field + density,
            _ => field,
        };
        let mut frame = |view: &WorldView, kept: bool, step: &str| {
            let job = view.job().expect("a job");
            let (ours, report) = renderer.render_three_d_rgba8(&job).expect("renders");
            let (theirs, fresh) = MetalRenderer::new()
                .expect("a Metal renderer")
                .render_three_d_rgba8(&job)
                .expect("renders");
            assert_eq!(
                ours.as_bytes(),
                theirs.as_bytes(),
                "{name}, {step}: the frame"
            );
            let copied = u64::try_from(report.upload_bytes).expect("fits");
            let saved = if kept { layers } else { 0 };
            assert_eq!(
                copied + saved,
                u64::try_from(fresh.upload_bytes).expect("fits"),
                "{name}, {step}: the copy"
            );
        };
        frame(&view, false, "shown");
        let panned = view.viewport().panned([17.0, -9.0]).expect("pans");
        view.set_view(panned).expect("looks");
        view.place(&epoch).expect("places");
        frame(&view, true, "a pan");
        let zoomed = view
            .viewport()
            .zoomed_about([0.37, 0.61], 16.0)
            .expect("zooms");
        view.set_view(zoomed).expect("looks");
        view.place(&epoch).expect("places");
        frame(&view, true, "a zoom");
        view.show(&later).expect("shows");
        frame(&view, false, "a later epoch");
        frame(&view, true, "that epoch again");
    }
}
