//! FL-043 and FL-033, read from the device: what FrankenManim's Metal route
//! holds on the device for the Studio's frames, measured headlessly through
//! one renderer without a window, is within the part of the Studio's render
//! term (`flife_studio::memory::observer_bytes`) that an offscreen frame
//! holds on the device: the copy of the frame's tables and texels, the
//! route's two surfaces of [`DEVICE_BYTES_PER_PIXEL`] (all of it but the
//! window's three drawables), a status word a tile and a bloom's row sums.
//! The drawables are not measured: no window is opened. Its own binary, so
//! nothing else allocates on the device while it measures.

#![allow(
    clippy::expect_used,
    clippy::print_stdout,
    clippy::cast_precision_loss,
    reason = "test: a failure is a test failure; figures are printed"
)]

use flife_backend_metal::{DeviceOwner, Limits};
use flife_core::ValidatedConfig;
use flife_engine::{Arrangement, NumericalProfile, World};
use flife_manim::{Look, Particles, Style, WorldView};
use flife_render::Epoch;
use flife_studio::memory::{DEVICE_BYTES_PER_PIXEL, observer_bytes};
use fmn_render::metal::MetalRenderer;

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// The window's three BGRA8 drawables, of [`DEVICE_BYTES_PER_PIXEL`]'s
/// charge, which an offscreen frame does not hold.
const DRAWABLE_BYTES_PER_PIXEL: u64 = 3 * 4;

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

fn mib(bytes: u64) -> f64 {
    bytes as f64 / f64::from(1 << 20)
}

/// The part of the Studio's render term for `view` at `drawable` that an
/// offscreen frame holds on the device: [`observer_bytes`] less what it
/// charges the host (the feed's epoch slots, the view's own buffers, one
/// frame's job and the host half of the upload, the tables and texels
/// flattened before they are copied) and the window's drawables.
fn offscreen_device_term(
    world: &World,
    founding: &Epoch,
    view: &WorldView,
    drawable: [u32; 2],
) -> u64 {
    let slots = u64::from(world.config().config().execution.render_queue_capacity);
    let host = founding.reserved_bytes() * slots
        + view.reserved_bytes()
        + view.frame_bytes()
        + view.preparation_limit()
        + view.texel_bytes();
    let [width, height] = drawable.map(u64::from);
    observer_bytes(world, founding, view, drawable)
        - host
        - width * height * DRAWABLE_BYTES_PER_PIXEL
}

/// What one pass of frames found.
struct Pass {
    frames: u32,
    blooming: u32,
    /// The most the device held, as a share of the largest term so far.
    fullest: f64,
    /// The device's holdings and the largest term after the last frame.
    held: u64,
    term: u64,
    /// The most a bloom frame held beyond the same frame without it.
    bloom_grew: u64,
}

/// Render the preset's frames through one fresh renderer, at 4,096 and
/// 16,384 organisms, 1280 x 720 and 3840 x 2160, one and two pixels per
/// point, zooms 1 and 16, in points, density and raw density, each with
/// every bloom of `blooms`; the view with the Studio's overlay when
/// `overlay`. After each frame, the device holds no more beyond what it
/// held before the renderer opened than the largest offscreen device term
/// so far: the renderer keeps its tables between frames and never shrinks
/// them, and a Studio admits its opening drawable once.
fn pass(device: &DeviceOwner, overlay: bool, blooms: &[Option<f64>]) -> Pass {
    let before = device.device_allocated_bytes();
    let mut renderer = MetalRenderer::new().expect("a Metal renderer");
    println!(
        "overlay {overlay}: the renderer opens holding {} bytes on the device",
        device.device_allocated_bytes() - before
    );
    let mut found = Pass {
        frames: 0,
        blooming: 0,
        fullest: 0.0,
        held: 0,
        term: 0,
        bloom_grew: 0,
    };
    for live in [4_096, 16_384] {
        let config = preset_with(&[("initial_count", live), ("maximum_count", 16_384)]);
        let world = World::found(
            &config,
            Arrangement::FreeList,
            NumericalProfile::reference_f32().expect("profile"),
        )
        .expect("founds");
        let epoch = Epoch::standalone(&world).expect("an epoch");
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
                        // The same frame without its bloom, rendered just
                        // before it: what the device held and the term.
                        let mut plain = None;
                        for &bloom in blooms {
                            let mut view =
                                WorldView::for_epoch(&epoch, drawable, [0, 0, 0]).expect("a view");
                            if overlay {
                                view.enable_overlay();
                            }
                            view.set_pixels_per_point(density).expect("a density");
                            view.set_style(style);
                            view.set_bloom(bloom).expect("a bloom");
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
                            renderer.render_three_d_rgba8(&job).expect("renders");
                            let held = device.device_allocated_bytes() - before;
                            let own = offscreen_device_term(&world, &epoch, &view, drawable);
                            let term = found.term.max(own);
                            let name = style.particles.name();
                            assert!(
                                held <= term,
                                "overlay {overlay}, {live} live, {drawable:?} at {density}x, \
                                 zoom {zoom}, {name}, bloom {bloom:?}: the device holds {held} \
                                 ({:.2} MiB) > the render term's offscreen device part {term} \
                                 ({:.2} MiB)",
                                mib(held),
                                mib(term)
                            );
                            // A bloom holds no more on the device than the
                            // term charges it beyond the same frame without.
                            let mut grew = found.bloom_grew;
                            match (bloom, plain) {
                                (None, _) => plain = Some((held, own)),
                                (Some(_), Some((plain_held, plain_own))) => {
                                    let more = held.saturating_sub(plain_held);
                                    let charged = own - plain_own;
                                    assert!(
                                        more <= charged,
                                        "{live} live, {drawable:?} at {density}x, zoom {zoom}, \
                                         {name}: the bloom holds {more} more on the device than \
                                         the frame without it, > the {charged} charged"
                                    );
                                    grew = grew.max(more);
                                }
                                (Some(_), None) => {}
                            }
                            found = Pass {
                                frames: found.frames + 1,
                                blooming: found.blooming
                                    + u32::from(view.bloom_bytes(drawable) > 0),
                                fullest: found.fullest.max(held as f64 / term as f64),
                                held,
                                term,
                                bloom_grew: grew,
                            };
                        }
                    }
                }
            }
        }
    }
    println!(
        "overlay {overlay}: {} frames, {} of them charged a bloom: the device held at most \
         {:.1}% of the largest offscreen device term so far; after the last {} bytes \
         ({:.2} MiB) against {} ({:.2} MiB); a bloom frame held at most {} bytes ({:.2} MiB) \
         more than the same frame without",
        found.frames,
        found.blooming,
        found.fullest * 100.0,
        found.held,
        mib(found.held),
        found.term,
        mib(found.term),
        found.bloom_grew,
        mib(found.bloom_grew)
    );
    found
}

#[test]
fn the_device_holds_no_more_for_the_studios_frames_than_its_render_term() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let device = DeviceOwner::open(Limits::UNBOUNDED).expect("a device owner");
    println!(
        "{DEVICE_BYTES_PER_PIXEL} bytes a drawable pixel are charged, {} of them offscreen",
        DEVICE_BYTES_PER_PIXEL - DRAWABLE_BYTES_PER_PIXEL
    );
    // The frames `frame_upload.rs` measures on the host: no overlay and no
    // bloom, so the term is the least and the check the tightest.
    let bare = pass(&device, false, &[None]);
    assert_eq!((bare.frames, bare.blooming), (48, 0));
    // The view as the Studio builds it: with its overlay, and with and
    // without the preset's bloom.
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset");
    let presentation = &config.config().presentation;
    assert!(presentation.bloom_enabled, "the preset blooms");
    let studio = pass(&device, true, &[None, Some(presentation.bloom_strength)]);
    assert_eq!((studio.frames, studio.blooming), (96, 48));
    assert!(
        studio.bloom_grew > 0,
        "a bloom holds something on the device"
    );
}
