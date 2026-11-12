//! FL-024: the Studio with no window ([`Surface::Offscreen`]). The host's
//! whole frame path runs as a window's does: typed input through the
//! controls, intents to the session thread and its statuses back, epochs
//! through the pacer, the camera, picks, the overlay and its colour key,
//! and FrankenManim's Metal 3D route, here rendering to its RGBA8 surface
//! and reading it back. Only AppKit's event delivery and the drawable's
//! presentation are not run. Headless Metal: without a Metal device the
//! tests return early.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "test: a failure is a test failure; figures are printed"
)]

use std::num::{NonZeroU32, NonZeroUsize};
use std::time::{Duration, Instant};

use flife_core::ids::{CommandId, OrganismId};
use flife_core::{Tick, ValidatedConfig};
use flife_engine::{Arrangement, Disposition, NumericalProfile, RegionMeasurement, Stepped, World};
use flife_manim::{
    HUD_TEXT_POINTS, HudFont, REGION_WIDTH_POINTS, RegionEffect, RegionShape, hud_origin, linear,
};
use flife_runtime::Outcome;
use flife_studio::{
    Brush, BrushSettings, ColorMapping, ControllerChoice, FrameTimes, KEY_DIGITS, KEY_E, KEY_EQUAL,
    KEY_G, KEY_I, KEY_M, KEY_MINUS, KEY_PERIOD, KEY_SLASH, KEY_SPACE, KEY_T, KEY_W, KEY_X,
    KEY_ZERO, MeasurePreview, PICK_RADIUS_POINTS, Preview, Status, Stroke, Studio, StudioOptions,
    Supplies, Surface, TimelineRecord, cell_region, drawn, legends, material_for, measure,
    preview_region, scroll_zoom, stroke,
};
use fmn_render::metal::{
    ButtonSet, InputEvent, LogicalPoint, MetalRenderer, Modifiers, PixelDensity, PointerButton,
    ScrollUnit, SurfaceMetrics,
};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// Points of the offscreen content area, and pixels a point.
const SIZE: [u32; 2] = [320, 200];
const SCALE: u32 = 2;

fn config() -> ValidatedConfig {
    ValidatedConfig::from_toml(PRESET).expect("the preset is valid")
}

fn options(run: bool) -> StudioOptions {
    StudioOptions {
        size: Some(SIZE),
        cpu_workers: NonZeroUsize::new(2),
        run_on_open: run,
        controllers: ControllerChoice::Cpu,
        surface: Surface::Offscreen {
            scale: NonZeroU32::new(SCALE).expect("nonzero"),
        },
        ..StudioOptions::default()
    }
}

fn point(at: [f64; 2]) -> LogicalPoint {
    LogicalPoint { x: at[0], y: at[1] }
}

fn key(code: u16, text: &str, pressed: bool) -> InputEvent {
    InputEvent::Key {
        code,
        text: text.to_owned(),
        pressed,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

fn click(studio: &mut Studio, at: [f64; 2]) {
    for pressed in [true, false] {
        studio
            .post_input(InputEvent::Button {
                button: PointerButton::Primary,
                pressed,
                position: point(at),
                clicks: 1,
                modifiers: Modifiers::NONE,
            })
            .expect("posts");
    }
}

fn hover(studio: &mut Studio, at: [f64; 2]) {
    studio
        .post_input(InputEvent::PointerMoved {
            position: point(at),
            buttons: ButtonSet::EMPTY,
            modifiers: Modifiers::NONE,
        })
        .expect("posts");
}

/// Press and release a key.
fn tap(studio: &mut Studio, code: u16, text: &str) {
    studio.post_input(key(code, text, true)).expect("posts");
    studio.post_input(key(code, text, false)).expect("posts");
}

/// Frames until `done` holds, for at most 30 s of wall time.
fn until(studio: &mut Studio, what: &str, mut done: impl FnMut(&Studio, &[Status]) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut statuses = Vec::new();
    loop {
        studio.frame().expect("a frame");
        statuses.extend(studio.take_statuses());
        if done(studio, &statuses) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn an_offscreen_studio_draws_its_views_frame() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let mut studio = Studio::open(&config(), &options(false)).expect("opens offscreen");
    assert!(studio.is_offscreen());
    assert_eq!(studio.pixel_density(), PixelDensity::Backing);
    assert!(studio.last_frame().is_none(), "no frame before the first");
    let frame = studio.frame().expect("a frame");
    // The first frame drains the surface's geometry and focus, as a
    // window's first poll does, and draws at its density.
    assert!(frame.refitted && frame.drawn && frame.outcome.is_none());
    assert_eq!(frame.events, 2);
    assert!(studio.controls().focused());
    let drawable = [SIZE[0] * SCALE, SIZE[1] * SCALE];
    assert_eq!(studio.view().viewport().drawable(), drawable);
    let metrics = studio.surface_metrics().expect("metrics");
    assert_eq!([metrics.drawable_width, metrics.drawable_height], drawable);
    assert_eq!(metrics.backing_scale, f64::from(SCALE));
    let tally = studio.tally();
    assert_eq!((tally.frames, tally.rendered, tally.presented), (1, 1, 0));
    assert_eq!(tally.drawn(), 1);
    assert_eq!(
        tally.readback_bytes,
        u64::from(drawable[0]) * u64::from(drawable[1]) * 4
    );
    // The frame read back is the view's job, as another renderer draws it.
    let ours = studio.last_frame().expect("the frame").as_bytes().to_vec();
    assert_eq!(ours.len(), (drawable[0] * drawable[1] * 4) as usize);
    let job = studio.view().job().expect("the job");
    let (theirs, _) = MetalRenderer::new()
        .expect("a renderer")
        .render_three_d_rgba8(&job)
        .expect("renders");
    drop(job);
    assert!(ours == theirs.as_bytes(), "the frame is the view's job");
    // Not a blank frame: the field and the founders are drawn.
    let (pixels, _) = ours.as_chunks::<4>();
    let background = pixels[0];
    assert!(pixels.iter().filter(|pixel| **pixel != background).count() > 1_000);
    let closed = studio.close().expect("closes");
    assert_eq!(closed.frames.rendered, 1);
    assert_eq!(closed.input_dropped, 0);
    assert_eq!(closed.input.focus_gained, 1);
    assert_eq!(closed.input.resizes, 1);
}

#[test]
fn posted_input_drives_an_offscreen_studio_as_a_windows_does() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let mut studio = Studio::open(&config(), &options(false)).expect("opens offscreen");
    studio.frame().expect("the first frame");
    let density = f64::from(SCALE);

    // Key 2: the energy mapping.
    studio
        .post_input(key(KEY_DIGITS[1], "2", true))
        .expect("posts");
    studio
        .post_input(key(KEY_DIGITS[1], "2", false))
        .expect("posts");
    let frame = studio.frame().expect("a frame");
    assert!(frame.restyled && frame.drawn);
    assert_eq!(studio.view().style().mapping, ColorMapping::Energy);

    // A scroll zooms about the pointer by exactly the scroll's factor.
    let before = *studio.view().viewport();
    let anchor = [100.0, 60.0];
    studio
        .post_input(InputEvent::Scroll {
            delta: [0.0, 50.0],
            unit: ScrollUnit::Points,
            position: point(anchor),
            modifiers: Modifiers::NONE,
        })
        .expect("posts");
    let frame = studio.frame().expect("a frame");
    assert!(frame.moved);
    let factor = scroll_zoom(50.0, ScrollUnit::Points).expect("a factor");
    let zoomed = *studio.view().viewport();
    assert!((zoomed.zoom() / before.zoom() - factor).abs() < 1e-12);
    let at = [anchor[0] * density, anchor[1] * density];
    let held = before.drawable_to_world(at).expect("a world point");
    let still = zoomed.drawable_to_world(at).expect("a world point");
    assert!(
        (held[0] - still[0]).hypot(held[1] - still[1]) < 1e-9,
        "the anchor stays put"
    );

    // A click on a drawn organism selects it by identity, and its
    // inspection answers.
    let rows: Vec<(u64, [f64; 2])> = studio.view().shown_rows().collect();
    let centre = [
        f64::from(SIZE[0]) * density / 2.0,
        f64::from(SIZE[1]) * density / 2.0,
    ];
    let distance = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    let (target, spot) = rows
        .iter()
        .map(|&(id, position)| (id, zoomed.world_to_drawable(position)))
        .min_by(|a, b| distance(a.1, centre).total_cmp(&distance(b.1, centre)))
        .expect("organisms are shown");
    let expected = flife_manim::pick(
        &zoomed,
        rows.iter().copied(),
        spot,
        PICK_RADIUS_POINTS * density,
    );
    assert_eq!(expected, Some(OrganismId::new(target)));
    click(&mut studio, [spot[0] / density, spot[1] / density]);
    // The Studio traces its world's ancestry from the founding, so the
    // inspection carries descendants counted from tick 0 (FL-021).
    until(&mut studio, "the inspection", |studio, statuses| {
        studio.view().selected() == expected
            && statuses.iter().any(|status| {
                matches!(status, Status::Inspected { organism, snapshot: Some(snapshot), .. }
                if Some(*organism) == expected
                    && snapshot.descendants.as_ref().is_some_and(|descendants| {
                        descendants.since == Tick::ZERO
                    }))
            })
    });

    // A new geometry, as a resized window reports it: the surface takes it
    // and the view refits to its drawable.
    let resized = SurfaceMetrics {
        logical_width: 400.0,
        logical_height: 250.0,
        backing_scale: density,
        drawable_width: 800,
        drawable_height: 500,
    };
    studio
        .post_input(InputEvent::Resized(resized))
        .expect("posts");
    let frame = studio.frame().expect("a frame");
    assert!(frame.refitted && frame.drawn);
    assert_eq!(studio.surface_metrics().expect("metrics"), resized);
    assert_eq!(studio.view().viewport().drawable(), [800, 500]);
    let pixels = studio.last_frame().expect("the frame").as_bytes().len();
    assert_eq!(pixels, 800 * 500 * 4);

    // Space runs the world: the shown tick moves on.
    studio.post_input(key(KEY_SPACE, " ", true)).expect("posts");
    studio
        .post_input(key(KEY_SPACE, " ", false))
        .expect("posts");
    until(&mut studio, "the world to run", |studio, statuses| {
        statuses
            .iter()
            .any(|status| matches!(status, Status::Running { .. }))
            && studio
                .view()
                .shown()
                .is_some_and(|shown| shown.tick.get() >= 8)
    });
    // The selection's organism is watched: an inspection refreshed while the
    // world runs, as the Studio's binary refreshes it each second, carries
    // what it sensed in a tick before the answer's, the activations of the
    // output it holds, and its transactions up to the answer's tick (FL-021,
    // plan §14.2).
    assert!(studio.inspect_selected(), "the refresh is offered");
    until(&mut studio, "a watched inspection", |_, statuses| {
        statuses.iter().any(|status| {
            matches!(status, Status::Inspected { organism, snapshot: Some(snapshot), .. }
            if Some(*organism) == expected
                && snapshot.sensed.as_ref().is_some_and(|sensed| sensed.tick < snapshot.tick)
                && snapshot.activations.as_ref().is_some_and(|activations| {
                    Some(activations.tick) == snapshot.organism.held.evaluated_at()
                        && activations.hidden.len() == snapshot.hidden_widths.len()
                })
                && snapshot.transactions.last().is_some_and(|newest| {
                    newest.tick.next().ok() == Some(snapshot.tick)
                        && newest.closing.to_bits() == snapshot.organism.energy.to_bits()
                })
                && snapshot.descendants.as_ref().is_some_and(|descendants| {
                    descendants.since == Tick::ZERO
                        && descendants.living <= descendants.born
                        && descendants.children_living <= descendants.children_born
                }))
        })
    });
    // W dumps the selected organism's whole controller (FL-021, plan
    // §14.2), read at the session's committed boundary while it runs.
    tap(&mut studio, KEY_W, "w");
    until(&mut studio, "a weight dump", |_, statuses| {
        statuses.iter().any(|status| {
            matches!(status, Status::Weights { organism, weights: Some(weights), .. }
            if Some(*organism) == expected
                && weights.organism == *organism
                && weights.tick.get() >= 8
                && !weights.parameters.is_empty()
                && weights.layers.last().is_some_and(|layer| layer.end() == weights.parameters.len()))
        })
    });
    let tally = studio.tally();
    assert_eq!(tally.rendered, tally.frames, "every frame was drawn");
    assert_eq!((tally.restyles, tally.selections), (1, 1));
    // A newer epoch was lowered as the world ran. Not one per tick: the
    // feed publishes the latest value, so a frame that comes late lowers
    // only the newest epoch, and a loaded machine skips some.
    assert!(tally.epochs >= 1, "{tally:?}");
    let closed = studio.close().expect("closes");
    assert!(closed.session.boundary.tick.get() >= 8);
    assert_eq!(closed.input_dropped, 0);
}

/// The largest channel difference between each swatch's interior pixels
/// (2 px inside its square) in the Studio's last frame and its sRGB colour,
/// and the pixels compared.
fn swatch_error(studio: &mut Studio, legend: &flife_manim::Legend) -> (u8, usize) {
    studio
        .set_overlay(&legend.text, &legend.swatches)
        .expect("the overlay");
    studio.frame().expect("a frame with the key");
    let frame = studio.last_frame().expect("the frame").as_bytes().to_vec();
    let [width, _] = studio.view().viewport().drawable();
    // The same text, laid out as the Studio lays it out at this density.
    let colors: Vec<_> = legend.swatches.iter().map(|&rgb| linear(rgb)).collect();
    let text = HudFont::bundled()
        .expect("the fonts")
        .text_with_swatches(&legend.text, HUD_TEXT_POINTS * f64::from(SCALE), &colors)
        .expect("the text");
    assert_eq!(text.swatches().len(), legend.swatches.len());
    let origin = hud_origin(studio.view().viewport());
    let (mut largest, mut checked) = (0, 0);
    for ((range, _), rgb) in text.swatches().iter().zip(&legend.swatches) {
        let corners = &text.vertices()[range.clone()];
        let low = corners
            .iter()
            .fold([f64::INFINITY; 2], |[x, y], c| [x.min(c[0]), y.min(c[1])]);
        let high = corners.iter().fold([f64::NEG_INFINITY; 2], |[x, y], c| {
            [x.max(c[0]), y.max(c[1])]
        });
        let mut inside = 0;
        let rows = (origin[1] + low[1] + 2.0).ceil() as u32..(origin[1] + high[1] - 2.0) as u32;
        for row in rows {
            let columns =
                (origin[0] + low[0] + 2.0).ceil() as u32..(origin[0] + high[0] - 2.0) as u32;
            for column in columns {
                let at = ((row * width + column) * 4) as usize;
                for channel in 0..3 {
                    largest = largest.max(frame[at + channel].abs_diff(rgb[channel]));
                }
                inside += 1;
            }
        }
        assert!(inside >= 25, "{inside} pixels inside a swatch");
        checked += inside;
    }
    (largest, checked)
}

/// FL-019's key on the Metal route: unbloomed, every swatch's interior is
/// its colour; the preset's bloom moves it, and by how much is printed.
#[test]
fn the_colour_key_is_drawn_in_its_colours_offscreen() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let bloomed = config();
    let unbloomed = ValidatedConfig::from_toml(
        &PRESET.replace("bloom_enabled = true", "bloom_enabled = false"),
    )
    .expect("valid without the bloom");
    assert!(!unbloomed.config().presentation.bloom_enabled);
    for mapping in [ColorMapping::Generation, ColorMapping::Affinity] {
        let keys = legends(&bloomed).expect("the keys");
        let (_, legend) = keys
            .iter()
            .find(|(key, _)| *key == mapping)
            .expect("the key");
        let mut studio = Studio::open(&unbloomed, &options(false)).expect("opens offscreen");
        studio.frame().expect("the first frame");
        let (error, checked) = swatch_error(&mut studio, legend);
        studio.close().expect("closes");
        assert!(error <= 2, "{mapping}: a swatch is {error} levels off");
        let mut studio = Studio::open(&bloomed, &options(false)).expect("opens offscreen");
        studio.frame().expect("the first frame");
        let (bloom, _) = swatch_error(&mut studio, legend);
        studio.close().expect("closes");
        println!(
            "{mapping} key: {checked} swatch pixels within {error} levels unbloomed; with the \
             preset's bloom within {bloom}"
        );
    }
}

/// G6's input latency, measured offscreen: camera moves posted at a random
/// moment between frames of a 60 Hz loop, while the world runs, each from
/// its post to the end of the frame that drew it. AppKit's delivery and the
/// display's presentation are not in it. Printed, not asserted: the test
/// asserts only that every move reached the next frame.
/// Click the shown organism drawn nearest the drawable point `aim`, press T
/// after it, and run the frame that takes both: whether the camera now
/// tracks that organism.
fn track_nearest(studio: &mut Studio, aim: [f64; 2]) -> bool {
    let density = f64::from(SCALE);
    let viewport = *studio.view().viewport();
    let distance = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    let (target, spot) = studio
        .view()
        .shown_rows()
        .map(|(id, position)| (id, viewport.world_to_drawable(position)))
        .min_by(|a, b| distance(a.1, aim).total_cmp(&distance(b.1, aim)))
        .expect("organisms are shown");
    click(studio, [spot[0] / density, spot[1] / density]);
    tap(studio, KEY_T, "t");
    let frame = studio.frame().expect("a frame");
    let _ = studio.take_statuses();
    frame.tracking && frame.selected == Some(OrganismId::new(target))
}

/// Plan §14.3's camera-follow target: T tracks the selected organism. While
/// the world runs, every frame draws it at the drawable's centre, where that
/// frame draws it (between epochs too), at the zoom it had, and the camera
/// moves with it; only losing the selection stops it then. A zoom keeps
/// tracking; T again, a pan, a reset and a click on nothing each stop it,
/// and T with nothing selected tracks nothing.
#[test]
fn t_keeps_the_selected_organism_centred_while_the_world_runs() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let mut studio = Studio::open(&config(), &options(false)).expect("opens offscreen");
    studio.frame().expect("the first frame");
    let density = f64::from(SCALE);
    let middle = SIZE.map(|points| f64::from(points) * density / 2.0);
    let spot = |studio: &Studio| {
        let position = studio
            .view()
            .selected_position()
            .expect("the selection is drawn");
        studio.view().viewport().world_to_drawable(position)
    };
    let off = |at: [f64; 2], to: [f64; 2]| (at[0] - to[0]).hypot(at[1] - to[1]);
    let scroll = |studio: &mut Studio, at: [f64; 2], delta: f64| {
        studio
            .post_input(InputEvent::Scroll {
                delta: [0.0, delta],
                unit: ScrollUnit::Points,
                position: point(at),
                modifiers: Modifiers::NONE,
            })
            .expect("posts");
    };

    tap(&mut studio, KEY_T, "t");
    let frame = studio.frame().expect("a frame");
    assert!(!frame.tracking && !frame.tracked, "nothing is selected");

    // Zoomed in, an organism away from the centre: tracking it moves the
    // view at once and keeps the zoom.
    scroll(&mut studio, [160.0, 100.0], 80.0);
    studio.frame().expect("a frame");
    let zoom = studio.view().viewport().zoom();
    assert!(zoom > 1.5, "{zoom}");
    assert!(track_nearest(
        &mut studio,
        [middle[0] * 0.5, middle[1] * 0.6]
    ));
    assert!(off(spot(&studio), middle) < 1e-6, "{:?}", spot(&studio));
    assert_eq!(studio.view().viewport().zoom().to_bits(), zoom.to_bits());
    assert_eq!(studio.tally().tracked, 1);
    let frame = studio.frame().expect("a frame");
    assert!(frame.tracking && !frame.tracked, "at rest the view stays");

    // Space runs the world: every frame draws the organism at the centre.
    tap(&mut studio, KEY_SPACE, " ");
    let (mut tracked, mut between, mut losses) = (0, 0, 0);
    let deadline = Instant::now() + Duration::from_secs(30);
    while tracked < 40 {
        assert!(Instant::now() < deadline, "{tracked} tracked frames");
        let frame = studio.frame().expect("a frame");
        let _ = studio.take_statuses();
        if frame.tracking {
            assert!(off(spot(&studio), middle) < 1e-6, "{:?}", spot(&studio));
            assert_eq!(studio.view().viewport().zoom().to_bits(), zoom.to_bits());
            if frame.tracked {
                tracked += 1;
                between += usize::from(frame.motion.is_some_and(|motion| motion.fraction < 1.0));
            }
        } else {
            assert_eq!(frame.selected, None, "only losing the selection stops it");
            losses += 1;
            assert!(losses < 50, "{losses} selections lost");
            while !track_nearest(&mut studio, middle) {
                assert!(Instant::now() < deadline, "no organism to track");
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(between > 0, "no tracked frame was drawn between epochs");
    println!("{tracked} tracked frames, {between} between epochs, {losses} selections lost");

    tap(&mut studio, KEY_SPACE, " ");
    until(&mut studio, "the pause", |_, statuses| {
        statuses
            .iter()
            .any(|status| matches!(status, Status::Paused { .. }))
    });
    // Until the display comes to rest.
    while studio.frame().expect("a frame").motion.is_some() {
        assert!(Instant::now() < deadline, "the display never came to rest");
    }
    if !studio.frame().expect("a frame").tracking {
        assert!(track_nearest(&mut studio, middle));
    }

    // A zoom about another point keeps tracking: about the organism.
    let before = studio.view().viewport().zoom();
    scroll(&mut studio, [60.0, 50.0], 40.0);
    let frame = studio.frame().expect("a frame");
    assert!(frame.moved && frame.tracked && frame.tracking, "{frame:?}");
    assert!(off(spot(&studio), middle) < 1e-6);
    let factor = scroll_zoom(40.0, ScrollUnit::Points).expect("a factor");
    assert!((studio.view().viewport().zoom() / before - factor).abs() < 1e-12);

    // T again stops it; T once more tracks again.
    tap(&mut studio, KEY_T, "t");
    assert!(!studio.frame().expect("a frame").tracking);
    tap(&mut studio, KEY_T, "t");
    assert!(studio.frame().expect("a frame").tracking);

    // A drag pans the view off the organism and stops tracking.
    let (from, to) = ([100.0, 80.0], [130.0, 95.0]);
    for event in [
        InputEvent::Button {
            button: PointerButton::Primary,
            pressed: true,
            position: point(from),
            clicks: 1,
            modifiers: Modifiers::NONE,
        },
        InputEvent::PointerMoved {
            position: point(to),
            buttons: ButtonSet::EMPTY.with(PointerButton::Primary),
            modifiers: Modifiers::NONE,
        },
        InputEvent::Button {
            button: PointerButton::Primary,
            pressed: false,
            position: point(to),
            clicks: 1,
            modifiers: Modifiers::NONE,
        },
    ] {
        studio.post_input(event).expect("posts");
    }
    let frame = studio.frame().expect("a frame");
    assert!(
        frame.moved && !frame.tracked && !frame.tracking,
        "{frame:?}"
    );
    let panned = [
        middle[0] + (to[0] - from[0]) * density,
        middle[1] + (to[1] - from[1]) * density,
    ];
    assert!(off(spot(&studio), panned) < 1e-6, "{:?}", spot(&studio));

    // T centres it again; 0 shows the whole torus and stops tracking.
    tap(&mut studio, KEY_T, "t");
    let frame = studio.frame().expect("a frame");
    assert!(frame.tracked && frame.tracking);
    assert!(off(spot(&studio), middle) < 1e-6);
    tap(&mut studio, KEY_ZERO, "0");
    let frame = studio.frame().expect("a frame");
    assert!(frame.moved && !frame.tracking);
    assert_eq!(studio.view().viewport().zoom(), 1.0);

    // A click on nothing selects nothing and stops tracking.
    tap(&mut studio, KEY_T, "t");
    let frame = studio.frame().expect("a frame");
    assert!(frame.tracked && frame.tracking);
    let viewport = *studio.view().viewport();
    let rows: Vec<(u64, [f64; 2])> = studio.view().shown_rows().collect();
    let empty = (0..40)
        .flat_map(|i| (0..25).map(move |j| [f64::from(i) * 8.0 + 4.0, f64::from(j) * 8.0 + 4.0]))
        .find(|at| {
            flife_manim::pick(
                &viewport,
                rows.iter().copied(),
                at.map(|x| x * density),
                PICK_RADIUS_POINTS * density,
            )
            .is_none()
        })
        .expect("somewhere picks nothing");
    click(&mut studio, empty);
    let frame = studio.frame().expect("a frame");
    assert_eq!(frame.selected, None);
    assert!(!frame.tracking && !frame.tracked);

    let tally = studio.tally();
    assert!(tally.tracked >= 40, "{tally:?}");
    assert_eq!(tally.rendered, tally.frames, "every frame was drawn");
    let closed = studio.close().expect("closes");
    assert_eq!(closed.input_dropped, 0);
}

/// What stepping a paused Studio saw.
#[derive(Default)]
struct Stepping {
    /// Steps offered.
    sent: u64,
    /// Steps published.
    stepped: u64,
    statuses: Vec<Status>,
    /// The frames that handed the selection on.
    handed: Vec<flife_studio::Frame>,
    /// The timeline records of each frame that handed the selection on.
    handed_records: Vec<Vec<TimelineRecord>>,
    /// The timeline records of the other frames stepping ran.
    records: Vec<TimelineRecord>,
}

/// Step the paused `studio` to committed tick `to`, with at most eight
/// steps outstanding, and run frames until it shows that tick at rest.
/// What the timeline recorded before is set aside; each frame's records
/// are kept with it.
fn step_to(studio: &mut Studio, to: u64, seen: &mut Stepping) {
    let deadline = Instant::now() + Duration::from_secs(120);
    studio.take_recorded();
    loop {
        if seen.sent < to && seen.sent < seen.stepped + 8 {
            tap(studio, KEY_PERIOD, ".");
            seen.sent += 1;
        }
        let frame = studio.frame().expect("a frame");
        let records = studio.take_recorded();
        if frame.handed.is_some() {
            seen.handed.push(frame);
            seen.handed_records.push(records);
        } else {
            seen.records.extend(records);
        }
        for status in studio.take_statuses() {
            match status {
                Status::Stepped { .. } => seen.stepped += 1,
                Status::StepHeld { .. } => panic!("a step was held: {status:?}"),
                _ => {}
            }
            seen.statuses.push(status);
        }
        if seen.stepped == to
            && frame.motion.is_none()
            && frame.shown.is_some_and(|shown| shown.tick == Tick::new(to))
        {
            return;
        }
        assert!(Instant::now() < deadline, "stepping to {to}");
    }
}

/// Plan §2.2's journey: pause, select an organism, remove it with the
/// removal brush, and follow its descendants. A direct world finds the
/// first tick at which two live organisms with a child alive each stand
/// alone within half a world unit. Selected without tracking, the first is
/// removed (X, zoomed in about it until the brush's disk there holds it
/// alone): the session's world follows its line, whose passing to its heir
/// the epoch of the removal's tick carries, but the selection ends with
/// it. Selected and tracked (T), the second is removed the same way: the
/// frame that first shows the removal's tick passes the selection to its
/// heir, the direct world's line's (its eldest child alive), drawn at the
/// drawable's centre with tracking going on, inspected, and carried by the
/// shown epoch; the timeline records that frame's selection of the heir, so
/// a replay draws it. Following changes nothing the session commits.
#[test]
fn tracking_passes_the_selection_to_the_heir_of_the_organism_it_followed() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    const UNTIL: u64 = 400;
    const ALONE: f64 = 0.5;
    let config = config();
    let mut direct = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds");
    direct.trace_ancestry().expect("traces");
    let mut parents = std::collections::BTreeSet::new();
    let alone = loop {
        let population = direct.population();
        let alone: Vec<OrganismId> = parents
            .iter()
            .copied()
            .filter(|&parent| {
                population.get(parent).is_some_and(|organism| {
                    population.iter().any(|child| child.parent == Some(parent))
                        && direct
                            .measure_region(organism.position, ALONE)
                            .expect("measures")
                            .organisms
                            == 1
                })
            })
            .take(2)
            .collect();
        if alone.len() == 2 {
            break alone;
        }
        assert!(
            direct.tick().get() < UNTIL,
            "no two parents alone by {UNTIL}"
        );
        let Stepped::Published(report) = direct.step_with(&[]).expect("steps") else {
            panic!("paused");
        };
        parents.extend(report.births.iter().map(|birth| birth.parent));
    };
    let [first, later] = alone[..] else {
        panic!("{alone:?}");
    };
    let at = direct.tick().get();
    println!("{first} and {later} have a child alive at {at}");

    let recorded = StudioOptions {
        record: true,
        ..options(false)
    };
    let mut studio = Studio::open(&config, &recorded).expect("opens offscreen");
    studio.frame().expect("the first frame");
    let mut seen = Stepping::default();
    step_to(&mut studio, at, &mut seen);
    let density = f64::from(SCALE);
    let middle = SIZE.map(|points| f64::from(points) * density / 2.0);
    // Where the shown epoch draws `organism`, a logical point.
    let spot = |studio: &Studio, organism: OrganismId| {
        let (_, position) = studio
            .view()
            .shown_rows()
            .find(|&(id, _)| id == organism.get())
            .expect("the organism is drawn");
        let drawable = studio.view().viewport().world_to_drawable(position);
        [drawable[0] / density, drawable[1] / density]
    };
    let select = |studio: &mut Studio, organism: OrganismId, track: bool| {
        click(studio, spot(studio, organism));
        if track {
            tap(studio, KEY_T, "t");
        }
        let frame = studio.frame().expect("a frame");
        assert_eq!(frame.selected, Some(organism));
        assert_eq!(frame.tracking, track);
        until(studio, "the follow's answer", |_, statuses| {
            statuses.iter().any(|status| {
                matches!(status, Status::Following { line, .. } if line.origin == organism
                    && line.organism == organism && line.handoffs == 0 && line.alive)
            })
        });
    };
    // Zoom in about `organism` until the removal brush's disk there holds it
    // alone in `direct`, click there and put the brush away: the stroke the
    // session took.
    let remove = |studio: &mut Studio, organism: OrganismId, direct: &World| -> Stroke {
        tap(studio, KEY_X, "x");
        let mut zooms = 0;
        let stroke = loop {
            let at = spot(studio, organism);
            hover(studio, at);
            let frame = studio.frame().expect("a frame");
            let Some(Preview::Stroke {
                stroke: stroke @ Stroke::Removal { center, radius, .. },
                ..
            }) = frame.preview
            else {
                panic!("{:?}", frame.preview);
            };
            if direct
                .measure_region(center, radius)
                .expect("measures")
                .organisms
                == 1
            {
                break stroke;
            }
            zooms += 1;
            assert!(zooms < 30, "the brush's disk never held {organism} alone");
            studio
                .post_input(InputEvent::Scroll {
                    delta: [0.0, 80.0],
                    unit: ScrollUnit::Points,
                    position: point(at),
                    modifiers: Modifiers::NONE,
                })
                .expect("posts");
        };
        click(studio, spot(studio, organism));
        until(studio, "the removal's submission", |_, statuses| {
            statuses
                .iter()
                .any(|status| matches!(status, Status::Submitted { stroke: made, .. } if *made == stroke))
        });
        tap(studio, KEY_X, "x");
        stroke
    };
    let apply = |direct: &mut World, organism: OrganismId, stroke: Stroke| {
        direct.follow(Some(organism)).expect("follows");
        let Stepped::Published(report) = direct.step_with(&[stroke.intervention()]).expect("steps")
        else {
            panic!("paused");
        };
        assert_eq!(report.interventions[0].removed, 1);
        assert!(report.deaths.iter().any(|death| death.id == organism));
        let line = direct.followed().expect("a line");
        let eldest = direct
            .population()
            .iter()
            .filter(|child| child.parent == Some(organism))
            .map(|child| child.id)
            .min()
            .expect("a child alive");
        assert_eq!(
            line,
            flife_engine::Followed {
                origin: organism,
                organism: eldest,
                since: report.tick,
                handoffs: 1,
                alive: true,
            }
        );
        line
    };

    // Untracked: the session's world follows the line, the selection ends.
    select(&mut studio, first, false);
    let stroke = remove(&mut studio, first, &direct);
    let line = apply(&mut direct, first, stroke);
    step_to(&mut studio, at + 1, &mut seen);
    assert_eq!(studio.view().selected(), None);
    assert!(seen.handed.is_empty());
    assert_eq!(studio.tally().handoffs, 0);
    assert_eq!(studio.display_pair().1.followed(), Some(line));
    let selection = |organism: Option<OrganismId>| TimelineRecord::Select {
        organism: organism.map(OrganismId::get),
    };
    // A selection a newer epoch ends is not recorded (a replay's view ends
    // it on the same epoch); nothing selects the heir.
    assert!(
        !seen
            .records
            .iter()
            .any(|record| matches!(record, TimelineRecord::Select { .. })),
        "{:?}",
        seen.records
    );
    assert!(!seen.records.contains(&selection(Some(line.organism))));

    // Tracked: the selection and the camera pass to the heir.
    tap(&mut studio, KEY_ZERO, "0");
    studio.frame().expect("a frame");
    select(&mut studio, later, true);
    let stroke = remove(&mut studio, later, &direct);
    let expected = apply(&mut direct, later, stroke);
    let heir = expected.organism;
    step_to(&mut studio, at + 2, &mut seen);
    let [frame] = seen.handed[..] else {
        panic!("{} frames handed the selection on", seen.handed.len());
    };
    assert_eq!(frame.handed, Some(expected));
    assert_eq!(frame.selected, Some(heir));
    assert!(frame.tracking);
    assert!(
        seen.handed_records[0].contains(&selection(Some(heir))),
        "{:?}",
        seen.handed_records[0]
    );
    assert_eq!(
        frame.shown.map(|shown| shown.tick),
        Some(expected.since),
        "the first frame showing the removal's tick"
    );
    assert_eq!(studio.display_pair().1.followed(), Some(expected));
    assert_eq!(studio.view().selected(), Some(heir));
    let frame = studio.frame().expect("a frame");
    assert!(frame.tracking && frame.handed.is_none());
    let position = studio
        .view()
        .selected_position()
        .expect("the heir is drawn");
    let centred = studio.view().viewport().world_to_drawable(position);
    assert!(
        (centred[0] - middle[0]).hypot(centred[1] - middle[1]) < 1e-6,
        "{centred:?}"
    );
    assert_eq!(studio.tally().handoffs, 1);
    let inspected = |statuses: &[Status]| {
        statuses.iter().any(|status| {
            matches!(status, Status::Inspected { organism, snapshot: Some(_), .. } if *organism == heir)
        })
    };
    if !inspected(&seen.statuses) {
        until(&mut studio, "the heir's inspection", |_, statuses| {
            inspected(statuses)
        });
    }
    let closed = studio.close().expect("closes");
    assert_eq!(closed.session.boundary.tick, expected.since);
    assert_eq!(closed.session.world.digest(), direct.digest());
}

#[test]
fn camera_moves_reach_the_next_frame_while_the_world_runs() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let mut studio = Studio::open(&config(), &options(true)).expect("opens offscreen");
    let period = Duration::from_secs(1) / 60;
    let mut frames = FrameTimes::new().expect("frame times");
    let mut latencies = FrameTimes::new().expect("latencies");
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut posted = None;
    let mut moves = 0;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(4) {
        let begun = Instant::now();
        let frame = studio.frame().expect("a frame");
        let _ = studio.take_statuses();
        frames.record(begun.elapsed());
        if let Some(at) = posted.take() {
            assert!(frame.moved, "a posted zoom reached the next frame");
            latencies.record(Instant::now().duration_since(at));
            moves += 1;
        }
        let rest = period.saturating_sub(begun.elapsed());
        // Every third frame, a scroll arrives part of the way through the
        // wait for the next.
        if studio.tally().frames.is_multiple_of(3) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let part = rest.mul_f64((state >> 11) as f64 / (1_u64 << 53) as f64);
            std::thread::sleep(part);
            posted = Some(Instant::now());
            let delta = if moves % 2 == 0 { 4.0 } else { -4.0 };
            studio
                .post_input(InputEvent::Scroll {
                    delta: [0.0, delta],
                    unit: ScrollUnit::Points,
                    position: point([160.0, 100.0]),
                    modifiers: Modifiers::NONE,
                })
                .expect("posts");
            std::thread::sleep(rest.saturating_sub(part));
        } else {
            std::thread::sleep(rest);
        }
    }
    let closed = studio.close().expect("closes");
    assert!(moves >= 20, "{moves} moves");
    let frame = frames.summary().expect("frames");
    let latency = latencies.summary().expect("latencies");
    println!(
        "offscreen {}x{} at {SCALE}x, world running to tick {}: {} frames, frame work p50 {:.1} \
         ms, p95 {:.1}, p99 {:.1}, max {:.1}; {moves} camera moves, post to drawn p50 {:.1} ms, \
         p95 {:.1}, max {:.1}",
        SIZE[0] * SCALE,
        SIZE[1] * SCALE,
        closed.session.boundary.tick.get(),
        frame.frames,
        frame.p50_ms,
        frame.p95_ms,
        frame.p99_ms,
        frame.max_ms,
        latency.p50_ms,
        latency.p95_ms,
        latency.max_ms
    );
}

/// The surface keeps at most a native queue's events between frames and
/// counts the rest as dropped, as the native queue does.
#[test]
fn an_offscreen_queue_keeps_a_native_queues_capacity() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let mut studio = Studio::open(&config(), &options(false)).expect("opens offscreen");
    for _ in 0..fmn_render::metal::INPUT_QUEUE_CAPACITY {
        studio
            .post_input(InputEvent::PointerMoved {
                position: point([10.0, 10.0]),
                buttons: ButtonSet::EMPTY,
                modifiers: Modifiers::NONE,
            })
            .expect("posts");
    }
    assert_eq!(
        studio.input_dropped(),
        2,
        "the opening geometry and focus fill two slots"
    );
    let frame = studio.frame().expect("a frame");
    assert_eq!(frame.events, fmn_render::metal::INPUT_QUEUE_CAPACITY);
    studio.close().expect("closes");
}

/// FL-022's host brush path, as a window's: keys arm the brushes and set
/// their amounts, and a click becomes a stroke through the frame's view at
/// that amount and is sent as a command; the material brush takes the
/// selected organism's affinity from its inspection. Paused at tick 0, both
/// apply at the next step, each moving the energy that the world stepped
/// directly with the same two strokes at tick 0 reports, bit for bit, and
/// the Studio closes on that world.
#[test]
fn brush_clicks_offscreen_apply_where_they_were_made() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let config = config();
    let mut studio = Studio::open(&config, &options(false)).expect("opens offscreen");
    studio.frame().expect("the first frame");
    let density = f64::from(SCALE);
    let logical = SIZE.map(f64::from);
    let world = &config.config().world;
    let cells = [world.grid_width, world.grid_height];
    let viewport = *studio.view().viewport();

    // Select the shown organism nearest the centre, as a click does, and
    // wait for its inspection: the material brush's composition.
    let rows: Vec<(u64, [f64; 2])> = studio.view().shown_rows().collect();
    let centre = [logical[0] * density / 2.0, logical[1] * density / 2.0];
    let distance = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    let spot = rows
        .iter()
        .map(|&(_, position)| viewport.world_to_drawable(position))
        .min_by(|a, b| distance(*a, centre).total_cmp(&distance(*b, centre)))
        .expect("organisms are shown");
    click(&mut studio, [spot[0] / density, spot[1] / density]);
    let mut affinity = None;
    until(&mut studio, "the inspection", |studio, statuses| {
        affinity = statuses.iter().find_map(|status| match status {
            Status::Inspected {
                organism,
                snapshot: Some(snapshot),
                ..
            } if studio.view().selected() == Some(*organism) => Some(*snapshot.traits.affinity()),
            _ => None,
        });
        affinity.is_some()
    });
    let material = material_for(&affinity.expect("inspected"));
    assert!(material.is_some(), "the organism has an affinity direction");

    // Arm each brush by its key, set its amount (FL-022: `=` doubles, `-`
    // halves), and click with it, away from the organism.
    let mut strokes = Vec::new();
    let mut settings = BrushSettings::DEFAULT;
    for (code, text, brush, scale, at) in [
        (
            KEY_E,
            "e",
            Brush::Energy,
            [(KEY_EQUAL, "="); 2],
            [110.0, 70.0],
        ),
        (
            KEY_M,
            "m",
            Brush::Material,
            [(KEY_MINUS, "-"); 2],
            [210.0, 130.0],
        ),
    ] {
        studio.post_input(key(code, text, true)).expect("posts");
        studio.post_input(key(code, text, false)).expect("posts");
        assert_eq!(studio.frame().expect("a frame").brush, Some(brush));
        for (code, text) in scale {
            tap(&mut studio, code, text);
            settings = settings.scaled(brush, code == KEY_EQUAL);
        }
        let supplies = Supplies {
            material,
            founder_energy: config.config().population.initial_energy,
            settings,
        };
        let made = stroke(brush, &viewport, logical, cells, at, &supplies).expect("a stroke there");
        // Its preview there is that stroke, the selected organism's
        // material included.
        hover(&mut studio, at);
        let previewed = studio.frame().expect("a frame").preview;
        assert!(
            matches!(previewed, Some(Preview::Stroke { stroke, .. }) if stroke == made),
            "{previewed:?}"
        );
        strokes.push(made);
        click(&mut studio, at);
    }
    // 64 doubled twice and halved twice.
    assert!(
        matches!(strokes[0], Stroke::Energy { amount, .. } if amount == 256.0),
        "{:?}",
        strokes[0]
    );
    assert!(
        matches!(strokes[1], Stroke::Material { amount, .. } if amount == 16.0),
        "{:?}",
        strokes[1]
    );
    let mut submitted = Vec::new();
    until(&mut studio, "the submissions", |_, statuses| {
        submitted = statuses
            .iter()
            .filter_map(|status| match status {
                Status::Submitted {
                    command,
                    stroke,
                    seen,
                    effective,
                } => Some((*command, *stroke, *seen, *effective)),
                _ => None,
            })
            .collect();
        submitted.len() == 2
    });
    assert_eq!(
        submitted,
        [
            (CommandId::new(1), strokes[0], Tick::ZERO, Tick::ZERO),
            (CommandId::new(2), strokes[1], Tick::ZERO, Tick::ZERO),
        ]
    );

    // One step: both apply at the boundary of tick 0.
    studio
        .post_input(key(KEY_PERIOD, ".", true))
        .expect("posts");
    studio
        .post_input(key(KEY_PERIOD, ".", false))
        .expect("posts");
    let mut applied = Vec::new();
    until(&mut studio, "the outcomes", |_, statuses| {
        applied = statuses
            .iter()
            .filter_map(|status| match status {
                Status::Finished {
                    command,
                    outcome:
                        Outcome::Applied {
                            tick,
                            published,
                            applied,
                            ..
                        },
                } => Some((*command, *tick, *published, *applied)),
                _ => None,
            })
            .collect();
        applied.len() == 2
    });
    applied.sort_by_key(|(command, ..)| *command);
    let mut direct = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds");
    let interventions: Vec<_> = strokes.iter().map(|stroke| stroke.intervention()).collect();
    let report = match direct.step_with(&interventions).expect("steps") {
        Stepped::Published(report) => report,
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    };
    assert_eq!(report.interventions.len(), 2);
    for ((command, tick, published, energy), expected) in applied.iter().zip(&report.interventions)
    {
        assert_eq!(
            (*tick, *published),
            (Tick::ZERO, Tick::new(1)),
            "{command:?}"
        );
        assert_eq!(energy.to_bits(), expected.applied.to_bits(), "{command:?}");
        assert!(*energy > 0.0, "{command:?}");
    }
    let tally = studio.tally();
    assert_eq!((tally.strokes, tally.unstroked), (2, 0));
    let closed = studio.close().expect("closes");
    assert_eq!(closed.session.boundary.tick, Tick::new(1));
    assert_eq!(closed.session.world.digest(), direct.digest());
}

/// FL-022 (plan §2.2: "draw an intervention region, remove part of a
/// colony"): with the removal brush armed and sent out of the world by
/// `/`, a drag draws its disk and pans nothing. Each frame's preview and
/// outlined region are the disk a release there would apply, centred where
/// the press went down, and the release submits exactly that disk with
/// that disposition. Paused at tick 0, it applies at the next step: the
/// world stepped directly with the same removal removes the same energy,
/// bit for bit, all of it to the ledger's removed term, and organisms the
/// frame showed inside the disk, and the Studio closes on that world. A
/// drag pressed off the world draws and submits nothing.
#[test]
fn a_drawn_removal_disk_applies_where_its_preview_outlined() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let config = config();
    let mut studio = Studio::open(&config, &options(false)).expect("opens offscreen");
    studio.frame().expect("the first frame");
    let density = f64::from(SCALE);
    let logical = SIZE.map(f64::from);
    let viewport = *studio.view().viewport();
    let extent = viewport.extent();
    let button = |studio: &mut Studio, pressed: bool, at: [f64; 2]| {
        studio
            .post_input(InputEvent::Button {
                button: PointerButton::Primary,
                pressed,
                position: point(at),
                clicks: 1,
                modifiers: Modifiers::NONE,
            })
            .expect("posts");
    };
    let drag = |studio: &mut Studio, at: [f64; 2]| {
        studio
            .post_input(InputEvent::PointerMoved {
                position: point(at),
                buttons: ButtonSet::EMPTY.with(PointerButton::Primary),
                modifiers: Modifiers::NONE,
            })
            .expect("posts");
    };
    tap(&mut studio, KEY_X, "x");
    // Sent out of the modeled world (FL-022's disposition): `/` twice, past
    // dissipated.
    tap(&mut studio, KEY_SLASH, "/");
    tap(&mut studio, KEY_SLASH, "/");
    studio.frame().expect("a frame");
    assert_eq!(
        studio.controls().settings().disposition(),
        Disposition::Remove
    );

    // Pressed in the letterbox margin: no disk, nothing outlined, and the
    // release makes no stroke.
    button(&mut studio, true, [5.0, 100.0]);
    drag(&mut studio, [40.0, 100.0]);
    let frame = studio.frame().expect("a frame");
    assert_eq!(frame.preview, Some(Preview::Drawn { stroke: None }));
    assert_eq!(studio.view().region(), None);
    button(&mut studio, false, [40.0, 100.0]);
    studio.frame().expect("a frame");
    assert_eq!(studio.tally().unstroked, 1);

    // Drawn out from the shown organism nearest the centre.
    let rows: Vec<(u64, [f64; 2])> = studio.view().shown_rows().collect();
    let middle = [logical[0] * density / 2.0, logical[1] * density / 2.0];
    let distance = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    let spot = rows
        .iter()
        .map(|&(_, position)| viewport.world_to_drawable(position))
        .min_by(|a, b| distance(*a, middle).total_cmp(&distance(*b, middle)))
        .expect("organisms are shown");
    let centre = [spot[0] / density, spot[1] / density];
    button(&mut studio, true, centre);
    let edges = [
        [centre[0] + 9.0, centre[1]],
        [centre[0] + 18.0, centre[1] + 12.0],
    ];
    for edge in edges {
        drag(&mut studio, edge);
        let frame = studio.frame().expect("a frame");
        let expected = drawn(&viewport, logical, centre, edge)
            .expect("a disk")
            .disposed(Disposition::Remove);
        assert_eq!(
            frame.preview,
            Some(Preview::Drawn {
                stroke: Some(expected)
            })
        );
        let outlined = studio.view().region().expect("an outlined disk");
        assert_eq!(
            Some(outlined),
            preview_region(frame.preview.as_ref().expect("a preview"))
        );
        assert_eq!(outlined.effect, RegionEffect::Removes);
        assert_eq!(
            *studio.view().viewport(),
            viewport,
            "a drawing pans nothing"
        );
    }
    let made = drawn(&viewport, logical, centre, edges[1])
        .expect("a disk")
        .disposed(Disposition::Remove);
    let Stroke::Removal {
        center,
        radius,
        disposition: Disposition::Remove,
    } = made
    else {
        panic!("{made:?}");
    };
    button(&mut studio, false, edges[1]);

    // The organisms the frame showed inside the disk, and on its edge, on
    // the torus.
    let apart = |a: [f64; 2]| {
        let [dx, dy] = [0, 1].map(|axis| {
            let d = (a[axis] - center[axis]).abs() % extent[axis];
            d.min(extent[axis] - d)
        });
        dx.hypot(dy)
    };
    let inside = rows
        .iter()
        .filter(|&&(_, at)| apart(at) < radius * (1.0 - 1e-4))
        .count();
    let reached = rows
        .iter()
        .filter(|&&(_, at)| apart(at) <= radius * (1.0 + 1e-4))
        .count();
    assert!(inside >= 1, "the organism pressed on is inside");

    let mut submitted = None;
    until(&mut studio, "the submission", |_, statuses| {
        submitted = statuses.iter().find_map(|status| match status {
            Status::Submitted {
                command,
                stroke,
                seen,
                effective,
            } => Some((*command, *stroke, *seen, *effective)),
            _ => None,
        });
        submitted.is_some()
    });
    assert_eq!(
        submitted,
        Some((CommandId::new(1), made, Tick::ZERO, Tick::ZERO))
    );

    // One step: it applies at the boundary of tick 0.
    tap(&mut studio, KEY_PERIOD, ".");
    let mut applied = None;
    until(&mut studio, "the outcome", |_, statuses| {
        applied = statuses.iter().find_map(|status| match status {
            Status::Finished {
                outcome:
                    Outcome::Applied {
                        tick,
                        published,
                        applied,
                        ..
                    },
                ..
            } => Some((*tick, *published, *applied)),
            _ => None,
        });
        applied.is_some()
    });
    let mut direct = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds");
    let report = match direct.step_with(&[made.intervention()]).expect("steps") {
        Stepped::Published(report) => report,
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    };
    let [removal] = report.interventions.as_slice() else {
        panic!("{:?}", report.interventions);
    };
    let (tick, published, energy) = applied.expect("applied");
    assert_eq!((tick, published), (Tick::ZERO, Tick::new(1)));
    assert_eq!(energy.to_bits(), removal.applied.to_bits());
    assert!(energy > 0.0);
    // Out of the world: the ledger's removed term is that energy, and none
    // of it reached the field.
    assert_eq!(report.ledger.removed.to_bits(), energy.to_bits());
    let removed = usize::try_from(removal.removed).expect("a count");
    println!(
        "a disk of {radius} world units removed {removed} organisms ({inside} to {reached} shown \
         in it), {energy} energy"
    );
    assert!(
        (inside..=reached).contains(&removed),
        "{removed} removed, {inside} to {reached} shown in the disk"
    );
    let tally = studio.tally();
    assert_eq!((tally.strokes, tally.unstroked), (1, 1));
    let closed = studio.close().expect("closes");
    assert_eq!(closed.session.boundary.tick, Tick::new(1));
    assert_eq!(closed.session.world.digest(), direct.digest());
}

/// Plan §14.3's temporary measurement region: with the measuring tool armed
/// (G), each frame outlines in the measurement colour what a click at the
/// pointer, or the release of a disk being drawn, would measure; a click or
/// a drag places that disk, and the session reports what it holds at the
/// committed boundary: exactly what the same world measured directly
/// holds, and measured again, the same. A removal drawn the same way then
/// removes exactly the organisms it counted and moves exactly their
/// energy, and the measurements changed nothing: the session closes on the
/// direct world's digest. Putting the tool away takes its disk away.
#[test]
fn the_measuring_tool_measures_what_a_removal_of_its_disk_takes() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let config = config();
    let mut studio = Studio::open(&config, &options(false)).expect("opens offscreen");
    studio.frame().expect("the first frame");
    let density = f64::from(SCALE);
    let logical = SIZE.map(f64::from);
    let button = |studio: &mut Studio, pressed: bool, at: [f64; 2]| {
        studio
            .post_input(InputEvent::Button {
                button: PointerButton::Primary,
                pressed,
                position: point(at),
                clicks: 1,
                modifiers: Modifiers::NONE,
            })
            .expect("posts");
    };
    let drag = |studio: &mut Studio, at: [f64; 2]| {
        studio
            .post_input(InputEvent::PointerMoved {
                position: point(at),
                buttons: ButtonSet::EMPTY.with(PointerButton::Primary),
                modifiers: Modifiers::NONE,
            })
            .expect("posts");
    };
    let measured = |statuses: &[Status]| -> Vec<(Tick, RegionMeasurement)> {
        statuses
            .iter()
            .filter_map(|status| match status {
                Status::Measured { seen, measurement } => Some((*seen, *measurement)),
                _ => None,
            })
            .collect()
    };
    // A few ticks first, so the sources have put energy in the field.
    const STEPS: u64 = 4;
    let mut direct = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("founds");
    for _ in 0..STEPS {
        assert!(matches!(
            direct.step_with(&[]).expect("steps"),
            Stepped::Published(_)
        ));
        tap(&mut studio, KEY_PERIOD, ".");
    }
    until(&mut studio, "the steps", |studio, _| {
        studio
            .view()
            .shown()
            .is_some_and(|shown| shown.tick == Tick::new(STEPS))
    });
    let viewport = *studio.view().viewport();
    let exactly = |a: &RegionMeasurement, b: &RegionMeasurement| {
        assert_eq!(
            (a.tick, a.center, a.radius, a.organisms, a.lineages, a.cells),
            (b.tick, b.center, b.radius, b.organisms, b.lineages, b.cells)
        );
        assert_eq!(a.organism_energy.to_bits(), b.organism_energy.to_bits());
        assert_eq!(a.field_energy.to_bits(), b.field_energy.to_bits());
    };

    tap(&mut studio, KEY_G, "g");
    // Off the world: nothing outlined, and a click there places nothing.
    let margin = [5.0, 100.0];
    hover(&mut studio, margin);
    let frame = studio.frame().expect("a frame");
    assert_eq!(frame.brush, None);
    assert_eq!(frame.measuring, Some(MeasurePreview::Click(None)));
    assert_eq!(studio.view().region(), None);
    click(&mut studio, margin);
    studio.frame().expect("a frame");
    assert_eq!(studio.measured(), None);
    assert_eq!(studio.tally().unmeasured, 1);

    // Over the shown organism nearest the centre: the click's disk.
    let rows: Vec<(u64, [f64; 2])> = studio.view().shown_rows().collect();
    let middle = [logical[0] * density / 2.0, logical[1] * density / 2.0];
    let distance = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    let spot = rows
        .iter()
        .map(|&(_, position)| viewport.world_to_drawable(position))
        .min_by(|a, b| distance(*a, middle).total_cmp(&distance(*b, middle)))
        .expect("organisms are shown");
    let centre = [spot[0] / density, spot[1] / density];
    hover(&mut studio, centre);
    let frame = studio.frame().expect("a frame");
    let clicked = measure::clicked(&viewport, logical, centre).expect("a disk");
    assert_eq!(frame.measuring, Some(MeasurePreview::Click(Some(clicked))));
    let outlined = studio.view().region().expect("an outlined disk");
    assert_eq!(outlined, clicked.region());
    assert_eq!(outlined.effect, RegionEffect::Measures);
    click(&mut studio, centre);
    let frame = studio.frame().expect("a frame");
    assert_eq!(frame.measuring, Some(MeasurePreview::Placed(clicked)));
    assert_eq!(studio.measured(), Some(clicked));
    let mut answers = Vec::new();
    until(&mut studio, "the click's measurement", |_, statuses| {
        answers = measured(statuses);
        !answers.is_empty()
    });
    let &[(seen, first)] = answers.as_slice() else {
        panic!("{answers:?}");
    };
    assert_eq!(seen, Tick::new(STEPS));
    exactly(
        &first,
        &direct
            .measure_region(clicked.center, clicked.radius)
            .expect("measures"),
    );
    assert!(first.organisms >= 1, "the organism under the click");

    // A drag draws a disk out from the same press, panning nothing.
    button(&mut studio, true, centre);
    let edges = [
        [centre[0] + 9.0, centre[1]],
        [centre[0] + 18.0, centre[1] + 12.0],
    ];
    for edge in edges {
        drag(&mut studio, edge);
        let frame = studio.frame().expect("a frame");
        let disk = measure::drawn(&viewport, logical, centre, edge).expect("a disk");
        assert_eq!(frame.measuring, Some(MeasurePreview::Release(Some(disk))));
        assert_eq!(studio.view().region(), Some(disk.region()));
        assert_eq!(
            *studio.view().viewport(),
            viewport,
            "a drawing pans nothing"
        );
        // The click's disk stays placed until the release.
        assert_eq!(studio.measured(), Some(clicked));
    }
    let disk = measure::drawn(&viewport, logical, centre, edges[1]).expect("a disk");
    assert_ne!(disk, clicked);
    button(&mut studio, false, edges[1]);
    let frame = studio.frame().expect("a frame");
    assert_eq!(frame.measuring, Some(MeasurePreview::Placed(disk)));
    assert_eq!(studio.view().region(), Some(disk.region()));
    // Measured again, as the binary does while the world moves on: the
    // same answer, the world unmoved.
    assert!(studio.measure_again());
    until(
        &mut studio,
        "the drawn disk's measurements",
        |_, statuses| {
            answers = measured(statuses);
            answers.len() == 2
        },
    );
    let truth = direct
        .measure_region(disk.center, disk.radius)
        .expect("measures");
    for (seen, answer) in &answers {
        assert_eq!(*seen, Tick::new(STEPS));
        exactly(answer, &truth);
    }
    assert_eq!(truth.tick, Tick::new(STEPS));
    assert!(truth.field_energy > 0.0, "{truth:?}");
    // A drag longer than the click's radius holds all the click's disk did.
    assert!(truth.organisms >= first.organisms, "{truth:?}");

    // The removal brush puts the tool and its disk away.
    tap(&mut studio, KEY_X, "x");
    let frame = studio.frame().expect("a frame");
    assert_eq!(frame.measuring, None);
    assert_eq!(studio.measured(), None);
    assert!(!studio.measure_again(), "nothing placed");
    // The same drag with it removes that disk.
    button(&mut studio, true, centre);
    for edge in edges {
        drag(&mut studio, edge);
    }
    button(&mut studio, false, edges[1]);
    let made = drawn(&viewport, logical, centre, edges[1]).expect("a disk");
    assert_eq!(
        made,
        Stroke::Removal {
            center: disk.center,
            radius: disk.radius,
            disposition: Disposition::Recycle,
        }
    );
    let mut submitted = false;
    until(&mut studio, "the submission", |_, statuses| {
        submitted |= statuses
            .iter()
            .any(|status| matches!(status, Status::Submitted { stroke, .. } if *stroke == made));
        submitted
    });
    tap(&mut studio, KEY_PERIOD, ".");
    let mut applied = None;
    until(&mut studio, "the outcome", |_, statuses| {
        applied = statuses.iter().find_map(|status| match status {
            Status::Finished {
                outcome: Outcome::Applied { applied, .. },
                ..
            } => Some(*applied),
            _ => None,
        });
        applied.is_some()
    });
    let report = match direct.step_with(&[made.intervention()]).expect("steps") {
        Stepped::Published(report) => report,
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    };
    let [removal] = report.interventions.as_slice() else {
        panic!("{:?}", report.interventions);
    };
    let energy = applied.expect("applied");
    println!(
        "a measured disk of {} world units held {} organisms of {} energy in {} lineages, and \
         {} energy in {} cells; its removal took {} organisms, {energy} energy",
        disk.radius,
        truth.organisms,
        truth.organism_energy,
        truth.lineages,
        truth.field_energy,
        truth.cells,
        removal.removed
    );
    assert_eq!(removal.removed, truth.organisms);
    assert_eq!(energy.to_bits(), truth.organism_energy.to_bits());
    assert_eq!(removal.applied.to_bits(), truth.organism_energy.to_bits());
    let tally = studio.tally();
    assert_eq!((tally.measures, tally.unmeasured), (2, 1));
    assert_eq!((tally.strokes, tally.unstroked), (1, 0));
    let closed = studio.close().expect("closes");
    assert_eq!(closed.session.boundary.tick, Tick::new(STEPS + 1));
    assert_eq!(closed.session.world.digest(), direct.digest());
}

/// FL-022's preview (plan §14.3): while a brush is armed, each frame says
/// what a click at the pointer would do, and a click there then submits
/// exactly that stroke. The frame outlines its region on the world (on the
/// Metal route, amber just outside the energy brush's cell), and outlines
/// nothing off the world, without material or disarmed.
#[test]
fn an_armed_brushs_preview_is_the_stroke_a_click_there_submits() {
    if !MetalRenderer::is_available() {
        println!("no Metal device: skipped");
        return;
    }
    let config = config();
    let mut studio = Studio::open(&config, &options(false)).expect("opens offscreen");
    assert_eq!(studio.frame().expect("the first frame").preview, None);
    let logical = SIZE.map(f64::from);
    let world = &config.config().world;
    let cells = [world.grid_width, world.grid_height];
    let viewport = *studio.view().viewport();
    let supplies = Supplies {
        material: None,
        founder_energy: config.config().population.initial_energy,
        settings: BrushSettings::DEFAULT,
    };

    // Armed before the pointer has been anywhere: off the world.
    tap(&mut studio, KEY_E, "e");
    let frame = studio.frame().expect("a frame");
    assert_eq!(
        (frame.brush, frame.preview),
        (Some(Brush::Energy), Some(Preview::OffWorld))
    );
    assert_eq!(studio.view().region(), None);
    let mut shown = Vec::new();
    for (code, text, brush, at) in [
        (KEY_E, "e", Brush::Energy, [110.0, 70.0]),
        (KEY_X, "x", Brush::Removal, [160.0, 100.0]),
        (KEY_I, "i", Brush::Introduction, [205.0, 150.0]),
    ] {
        if brush != Brush::Energy {
            tap(&mut studio, code, text);
        }
        hover(&mut studio, at);
        let frame = studio.frame().expect("a frame");
        assert_eq!(frame.brush, Some(brush));
        let expected = stroke(brush, &viewport, logical, cells, at, &supplies).expect("a stroke");
        let Some(Preview::Stroke {
            stroke: made,
            region,
        }) = frame.preview
        else {
            panic!("{brush:?} at {at:?}: {:?}", frame.preview);
        };
        assert_eq!(made, expected, "{brush:?}");
        assert_eq!(
            region,
            expected
                .cell()
                .map(|cell| cell_region(viewport.extent(), cells, cell))
        );
        // The world point under the pointer is in the cell it names.
        if let Some([[x0, x1], [y0, y1]]) = region {
            let [x, y] = frame.hover.expect("over the world");
            assert!(
                x0 <= x && x < x1 && y0 <= y && y < y1,
                "{x}, {y} in {region:?}"
            );
        }
        // The frame outlined it.
        let outlined = studio.view().region().expect("an outlined region");
        assert_eq!(
            Some(outlined),
            preview_region(frame.preview.as_ref().expect("a preview"))
        );
        assert_eq!(
            outlined.effect == RegionEffect::Removes,
            brush == Brush::Removal
        );
        assert_ne!(outlined.effect, RegionEffect::Measures);
        if let RegionShape::Rectangle([[x0, _], [y0, y1]]) = outlined.shape
            && brush == Brush::Energy
        {
            let edge = viewport.world_to_drawable([x0, (y0 + y1) / 2.0]);
            let band = REGION_WIDTH_POINTS * viewport.pixels_per_point();
            let rgba = studio.last_frame().expect("the frame").as_bytes();
            let [width, _] = viewport.drawable();
            let [px, py] = [edge[0] - band / 2.0, edge[1]].map(|value| value.floor() as usize);
            let at = 4 * (py * width as usize + px);
            let [r, g, b] = [rgba[at], rgba[at + 1], rgba[at + 2]];
            assert!(
                r > g && g > b && g >= 120,
                "amber just outside the cell on the Metal route: {:?}",
                [r, g, b]
            );
        }
        click(&mut studio, at);
        shown.push(made);
    }

    // The material brush with no organism selected has no material; in the
    // letterbox margin it is off the world, as every brush is.
    tap(&mut studio, KEY_M, "m");
    hover(&mut studio, [160.0, 100.0]);
    let frame = studio.frame().expect("a frame");
    assert_eq!(
        (frame.brush, frame.preview),
        (Some(Brush::Material), Some(Preview::NoMaterial))
    );
    assert_eq!(studio.view().region(), None);
    hover(&mut studio, [5.0, 100.0]);
    assert_eq!(
        studio.frame().expect("a frame").preview,
        Some(Preview::OffWorld)
    );
    assert_eq!(studio.view().region(), None);
    // Disarmed: no preview, no region.
    tap(&mut studio, KEY_M, "m");
    let frame = studio.frame().expect("a frame");
    assert_eq!((frame.brush, frame.preview), (None, None));
    assert_eq!(studio.view().region(), None);

    let mut submitted = Vec::new();
    until(&mut studio, "the submissions", |_, statuses| {
        submitted = statuses
            .iter()
            .filter_map(|status| match status {
                Status::Submitted { stroke, .. } => Some(*stroke),
                _ => None,
            })
            .collect();
        submitted.len() == 3
    });
    assert_eq!(submitted, shown);
    studio.close().expect("closes");
}
