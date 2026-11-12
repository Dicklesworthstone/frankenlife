//! FL-043 and FL-033: the host heap the Studio's Metal route holds to
//! upload a frame (FrankenManim's flattened rows and bins and the layers'
//! texels), and the bytes it copies to the device, measured headlessly on
//! a Metal device without a window, are each within the view's preparation
//! limit plus its texel bytes, the Studio's upload term. The Studio's frame
//! tally counts the same reports' copies. The device buffers the one
//! renderer keeps its tables in between frames (FrankenManim 9b116eee),
//! which never shrink, stay within the largest term of the frames so far.

#![allow(
    clippy::expect_used,
    clippy::print_stdout,
    clippy::cast_precision_loss,
    reason = "test: a failure is a test failure; figures are printed"
)]

use flife_core::ValidatedConfig;
use flife_engine::{Arrangement, NumericalProfile, World};
use flife_manim::{Look, Particles, Style, WorldView};
use flife_render::Epoch;
use flife_studio::FrameTally;
use fmn_render::metal::MetalRenderer;
use peak_alloc::PeakAlloc;

#[global_allocator]
static ALLOC: PeakAlloc = PeakAlloc;

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

fn preset_with(pairs: &[(&str, u64)]) -> ValidatedConfig {
    let mut lines: Vec<String> = PRESET.lines().map(str::to_owned).collect();
    for &(key, value) in pairs {
        let line = lines
            .iter()
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .expect("the key is in the preset");
        lines[line] = format!("{key} = {value}");
    }
    ValidatedConfig::from_toml(&lines.join("\n")).expect("still valid")
}

#[test]
fn a_frames_metal_upload_holds_and_copies_no_more_than_its_preparation_and_texels() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let mut renderer = MetalRenderer::new().expect("a Metal renderer");
    let mut frames = 0;
    let (mut closest, mut copied, mut largest) = (0.0_f64, 0.0_f64, 0);
    let mut tally = FrameTally::default();
    let (mut uploaded, mut read_back) = (0_u64, 0_u64);
    // The largest upload term so far, and the fullest the kept tables have
    // been of it.
    let (mut term, mut kept_most) = (0_u64, 0.0_f64);
    for live in [4_096, 16_384] {
        let config = preset_with(&[("initial_count", live), ("maximum_count", 16_384)]);
        let world = World::found(
            &config,
            Arrangement::FreeList,
            NumericalProfile::reference_f32().expect("profile"),
        )
        .expect("founds");
        let epoch = Epoch::standalone(&world).expect("an epoch");
        assert_eq!(
            epoch.len() as u64,
            live,
            "the epoch shows the founded world"
        );
        for drawable in [[1280, 720], [3840, 2160]] {
            for density in [1.0, 2.0] {
                for zoom in [1.0, 16.0] {
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
                        let job = view.job().expect("a job");
                        let base = ALLOC.current_usage();
                        ALLOC.reset_peak_usage();
                        let (frame, report) = renderer.render_three_d_rgba8(&job).expect("renders");
                        let peak = ALLOC.peak_usage().saturating_sub(base);
                        let held = u64::try_from(peak.saturating_sub(frame.as_bytes().len()))
                            .expect("fits");
                        let upload = u64::try_from(report.upload_bytes).expect("fits");
                        let limit = view.preparation_limit() + view.texel_bytes();
                        let name = style.particles.name();
                        assert!(
                            held <= limit,
                            "{live} live, {drawable:?} at {density}x, zoom {zoom}, {name}: \
                             upload held {held} > {limit}"
                        );
                        assert!(
                            upload <= limit,
                            "{live} live, {drawable:?} at {density}x, zoom {zoom}, {name}: \
                             upload copied {upload} > {limit}"
                        );
                        assert_eq!(report.readback_bytes, frame.as_bytes().len());
                        term = term.max(limit);
                        let kept = u64::try_from(renderer.resident_table_bytes()).expect("fits");
                        assert!(
                            kept > 0 && kept <= term,
                            "{live} live, {drawable:?} at {density}x, zoom {zoom}, {name}: \
                             tables kept {kept} > {term}"
                        );
                        kept_most = kept_most.max(kept as f64 / term as f64);
                        tally.count_copies(&report);
                        uploaded += upload;
                        read_back += u64::try_from(report.readback_bytes).expect("fits");
                        closest = closest.max(held as f64 / limit as f64);
                        copied = copied.max(upload as f64 / limit as f64);
                        largest = largest.max(upload);
                        frames += 1;
                    }
                }
            }
        }
    }
    println!(
        "{frames} frames uploaded; the fullest held {:.1}% and copied {:.1}% of its preparation \
         limit plus texels; the largest copy {largest} bytes; the tables kept on the device at \
         most {:.1}% of the largest term so far, {} bytes at the end",
        closest * 100.0,
        copied * 100.0,
        kept_most * 100.0,
        renderer.resident_table_bytes()
    );
    // The Studio's frame records: the copies its tally counts from the same
    // reports.
    assert_eq!(
        (
            tally.upload_bytes,
            tally.peak_upload_bytes,
            tally.readback_bytes
        ),
        (uploaded, largest, read_back)
    );
    assert!(tally.peak_upload_bytes < tally.upload_bytes);
}
