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
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_manim::{HUD_TEXT_POINTS, HudFont, REGION_WIDTH_POINTS, RegionShape, hud_origin, linear};
use flife_runtime::Outcome;
use flife_studio::{
    Brush, ColorMapping, ControllerChoice, FrameTimes, KEY_DIGITS, KEY_E, KEY_I, KEY_M, KEY_PERIOD,
    KEY_SPACE, KEY_X, PICK_RADIUS_POINTS, Preview, Status, Studio, StudioOptions, Supplies,
    Surface, cell_region, legends, material_for, preview_region, scroll_zoom, stroke,
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

/// FL-022's host brush path, as a window's: keys arm the brushes, and a
/// click becomes a stroke through the frame's view and is sent as a
/// command; the material brush takes the selected organism's affinity from
/// its inspection. Paused at tick 0, both apply at the next step, each
/// moving the energy that the world stepped directly with the same two
/// strokes at tick 0 reports, bit for bit, and the Studio closes on that
/// world.
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

    // Arm each brush by its key and click with it, away from the organism.
    let mut strokes = Vec::new();
    for (code, text, brush, at) in [
        (KEY_E, "e", Brush::Energy, [110.0, 70.0]),
        (KEY_M, "m", Brush::Material, [210.0, 130.0]),
    ] {
        studio.post_input(key(code, text, true)).expect("posts");
        studio.post_input(key(code, text, false)).expect("posts");
        assert_eq!(studio.frame().expect("a frame").brush, Some(brush));
        let supplies = Supplies {
            material,
            founder_energy: config.config().population.initial_energy,
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
        assert_eq!(outlined.removes, brush == Brush::Removal);
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
