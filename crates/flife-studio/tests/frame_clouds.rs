//! FL-033: the view draws its halos and dots as two FrankenManim dot clouds,
//! which FrankenManim's Metal route (the Studio's) compiles where it derives
//! the frame. Measured headlessly on a Metal device without a window, the
//! frame of the view's job is the frame of the same job with each cloud
//! replaced by its dots' true dots, byte for byte: in every look, with a
//! bloom, and through a zoomed camera the cloud was not placed for, where
//! dots leave the frame and the clip volume cuts those across its edges.

#![allow(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test: a failure is a test failure"
)]

use flife_core::ValidatedConfig;
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_manim::{Look, Style, WorldView};
use flife_render::Epoch;
use fmn_render::metal::MetalRenderer;
use fmn_render::{ThreeDDraw, ThreeDJob, Tiling};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

#[test]
fn the_metal_frame_of_the_views_clouds_is_the_frame_of_their_true_dots() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    // The preset's 4,096 organisms, a few ticks in.
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset");
    let mut world = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds");
    for _ in 0..8 {
        assert!(matches!(
            world.step().expect("steps"),
            Stepped::Published(_)
        ));
    }
    let epoch = Epoch::standalone(&world).expect("an epoch");
    let mut renderer = MetalRenderer::new().expect("a Metal renderer");
    let mut frames = 0;
    for (look, bloom) in [
        (Look::Glow, None),
        (Look::Glow, Some(0.15)),
        (Look::Raw, None),
        (Look::Sphere, None),
    ] {
        for zoom in [1.0, 3.0] {
            let mut view = WorldView::for_epoch(&epoch, [640, 360], [0, 0, 0]).expect("a view");
            view.set_style(Style {
                look,
                ..Style::default()
            });
            view.set_bloom(bloom).expect("a bloom");
            view.show(&epoch).expect("shows");
            if zoom > 1.0 {
                // Not placed: the view draws every dot, as clouds.
                let zoomed = view
                    .viewport()
                    .zoomed_about([0.37, 0.61], zoom)
                    .expect("zooms");
                view.set_view(zoomed).expect("looks");
            }
            let draws = view.draws().expect("draws");
            let clouds = draws
                .iter()
                .filter(|draw| matches!(draw, ThreeDDraw::DotCloud(_)))
                .count();
            assert_eq!(clouds, if look == Look::Glow { 2 } else { 1 }, "{look:?}");
            let true_dots: Vec<ThreeDDraw<'_>> = draws
                .iter()
                .flat_map(|draw| match draw {
                    ThreeDDraw::DotCloud(cloud) => cloud
                        .dots
                        .iter()
                        .map(|&dot| ThreeDDraw::TrueDot(cloud.true_dot(dot)))
                        .collect(),
                    other => vec![*other],
                })
                .collect();
            assert_eq!(
                true_dots.len(),
                draws.len() - clouds + epoch.len() * clouds,
                "{look:?}: a true dot a dot"
            );
            let mut reference =
                ThreeDJob::new(view.camera().camera(), &true_dots, Tiling::default())
                    .expect("the true dots' job");
            if let Some(bloom) = view.frame_bloom() {
                reference = reference.with_bloom(bloom.clone());
            }
            let job = view.job().expect("the view's job");
            let (ours, _) = renderer.render_three_d_rgba8(&job).expect("renders");
            let (theirs, _) = renderer
                .render_three_d_rgba8(&reference)
                .expect("renders the true dots");
            assert!(
                ours.as_bytes() == theirs.as_bytes(),
                "{look:?}, bloom {bloom:?}, zoom {zoom}: the clouds' frame is not their true dots'"
            );
            frames += 1;
        }
    }
    assert_eq!(frames, 8);
}
