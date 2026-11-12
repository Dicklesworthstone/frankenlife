//! FL-018's live end-to-end test: the real 4,096-organism reference world
//! in a real native window, driven only by real AppKit events.
//!
//! Every input here goes through the application's event queue (the
//! `synthetic-input` seam posts NSEvents and Core Graphics events), the
//! presenter's typed input bridge, the Studio's controls, the bounded intent
//! lane, the session thread and back out through render epochs. The test
//! then closes the Studio and checks the committed world is bit-identical to
//! the same configuration stepped directly.
//!
//! It opens a window on the process main thread, so it runs without the
//! libtest harness:
//!
//! ```text
//! cargo test -p flife-studio --features synthetic-input --test live
//! ```

use std::process::ExitCode;
use std::time::{Duration, Instant};

use flife_core::ids::OrganismId;
use flife_core::{Tick, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_render::{Interpolation, interpolate};
use flife_runtime::Outcome;
use flife_studio::{
    Brush, ColorMapping, ControllerChoice, End, Frame, KEY_D, KEY_DIGITS, KEY_E, KEY_ESCAPE, KEY_F,
    KEY_PERIOD, KEY_R, KEY_RIGHT, KEY_SPACE, KEY_ZERO, Look, MOTION_MAX, Motion,
    PICK_RADIUS_POINTS, Particles, Status, Studio, StudioOptions, Style, Surface,
};
use fmn_render::metal::{
    LogicalPoint, Modifiers, PixelDensity, PointerButton, PresentOutcome, SurfaceMetrics,
    SyntheticInput,
};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// How long any one wait may take before the test fails.
const PATIENCE: Duration = Duration::from_secs(30);

fn main() -> ExitCode {
    if !cfg!(target_os = "macos") {
        println!("flife-studio live: skipped: the native Studio needs macOS with a window server");
        return ExitCode::SUCCESS;
    }
    match live() {
        Ok(checks) => {
            for check in &checks {
                println!("ok: {check}");
            }
            println!("flife-studio live: {} checks passed", checks.len());
            ExitCode::SUCCESS
        }
        Err(failure) => {
            eprintln!("flife-studio live: FAILED: {failure}");
            ExitCode::FAILURE
        }
    }
}

struct Live {
    studio: Studio,
    statuses: Vec<Status>,
    checks: Vec<String>,
}

impl Live {
    fn post(&mut self, input: &SyntheticInput) -> Result<(), String> {
        self.studio
            .post(input)
            .map_err(|error| format!("posting {input:?}: {error}"))
    }

    fn key(&mut self, code: u16, text: &str) -> Result<(), String> {
        for pressed in [true, false] {
            self.post(&SyntheticInput::Key {
                code,
                text: text.into(),
                pressed,
                repeat: false,
                modifiers: Modifiers::NONE,
            })?;
        }
        Ok(())
    }

    /// Run frames until `done` holds, failing after [`PATIENCE`].
    fn until(
        &mut self,
        what: &str,
        mut done: impl FnMut(&Studio, &Frame, &[Status]) -> bool,
    ) -> Result<Frame, String> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let frame = self
                .studio
                .frame()
                .map_err(|error| format!("a frame while waiting for {what}: {error}"))?;
            self.statuses.extend(self.studio.take_statuses());
            if done(&self.studio, &frame, &self.statuses) {
                return Ok(frame);
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "timed out waiting for {what}; last frame {frame:?}; statuses {:?}",
                    self.statuses
                ));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn check(&mut self, holds: bool, what: impl Into<String>) -> Result<(), String> {
        let what = what.into();
        if holds {
            self.checks.push(what);
            Ok(())
        } else {
            Err(what)
        }
    }
}

fn shown_tick(frame: &Frame) -> Option<u64> {
    frame.shown.map(|shown| shown.tick.get())
}

/// Checks every frame of a moving display against `interpolate` over the
/// two epochs the Studio holds (FL-020).
#[derive(Default)]
struct MotionAudit {
    oracle: Interpolation,
    moving: u32,
    part_way: u32,
    arrivals: u32,
    faded: usize,
    wrong: Vec<String>,
    last: Option<Motion>,
}

impl MotionAudit {
    /// Audit `frame`; return the previous frame's motion.
    fn see(&mut self, studio: &Studio, frame: &Frame) -> Option<Motion> {
        let before = std::mem::replace(&mut self.last, frame.motion);
        let Some(motion) = frame.motion else {
            return before;
        };
        self.moving += 1;
        let (Some(prior), current) = studio.display_pair() else {
            self.wrong
                .push(format!("no prior epoch held while moving: {motion:?}"));
            return before;
        };
        let gap = interpolate(prior, current, motion.fraction, &mut self.oracle);
        let rows: Vec<(u64, [f64; 2])> = studio.view().shown_rows().collect();
        let expected: Vec<(u64, [f64; 2])> = current
            .ids()
            .iter()
            .zip(self.oracle.positions.iter().zip(&self.oracle.shown))
            .filter(|(_, (_, shown))| **shown)
            .map(|(&id, (&position, _))| (id, position))
            .collect();
        let at_prior = motion.fraction > 0.0
            || rows.iter().all(|&(id, position)| {
                prior
                    .row(OrganismId::new(id))
                    .is_some_and(|row| prior.positions()[row].map(f64::from) == position)
            });
        if gap.as_ref().ok() != Some(&motion.gap)
            || prior.id() != motion.prior.id
            || frame.shown.map(|shown| shown.id) != Some(current.id())
            || motion.fading != self.oracle.fading.len()
            || rows != expected
            || studio.view().cloud().draws().len() != prior.len()
            || !at_prior
        {
            self.wrong.push(format!(
                "a moving frame at fraction {} does not draw the interpolation of epochs {} and {}",
                motion.fraction,
                prior.id(),
                current.id()
            ));
        }
        if motion.fraction == 0.0 {
            self.arrivals += 1;
        } else {
            self.part_way += 1;
        }
        self.faded = self.faded.max(motion.fading);
        before
    }
}

fn live() -> Result<Vec<String>, String> {
    let config = ValidatedConfig::from_toml(PRESET).map_err(|error| error.to_string())?;
    let founders = config.config().population.initial_count;
    let ceiling = config.config().population.maximum_count;
    let options = StudioOptions {
        title: "FrankenLife Studio (live test)".into(),
        size: Some([640, 400]),
        resizable: true,
        // Several threads, so the closing digest check below also shows the
        // parallel tick computes the directly stepped (one-thread) world.
        cpu_workers: std::num::NonZeroUsize::new(4),
        run_on_open: false,
        // Metal where the host has it: the closing digest check then also
        // shows the GPU-evaluated world is the directly stepped one.
        controllers: ControllerChoice::Auto,
        style: Style::default(),
        surface: Surface::Window,
        record: false,
        run: None,
    };
    let studio = Studio::open(&config, &options).map_err(|error| format!("opening: {error}"))?;
    let mut live = Live {
        studio,
        statuses: Vec::new(),
        checks: Vec::new(),
    };

    // The window's first poll reports its geometry, and its focus when it
    // opened as the key window. Another application may hold the keyboard
    // (this machine is shared), so if the window is not key yet it is made
    // key: that too is a real AppKit key-window change.
    live.until("the window's first geometry", |studio, _, _| {
        studio.controls().metrics().is_some()
    })?;
    if !live.studio.controls().focused() {
        live.post(&SyntheticInput::Focus(true))?;
    }
    let frame = live.until("the window's focus", |studio, _, _| {
        studio.controls().focused()
    })?;
    let metrics = live
        .studio
        .controls()
        .metrics()
        .ok_or("no surface metrics")?;
    live.check(
        metrics.logical_width == 640.0 && metrics.logical_height == 400.0,
        format!("a real Resized event reports the 640 x 400 content area: {metrics:?}"),
    )?;
    // FL-020: frames are drawn at the screen's density, one drawable pixel
    // per device pixel, and the window keeps its size in points. Every
    // position below is a drawable one, posted at `density` pixels per point.
    let density = metrics.backing_scale;
    let backing = metrics
        .backing_size()
        .ok_or_else(|| format!("no backing size for {metrics:?}"))?;
    let dense = live.until("a frame at the screen's density", |studio, frame, _| {
        frame.outcome.is_some()
            && studio
                .surface_metrics()
                .is_ok_and(|now| [now.drawable_width, now.drawable_height] == backing)
    })?;
    let now = live
        .studio
        .surface_metrics()
        .map_err(|error| error.to_string())?;
    let view = *live.studio.view().viewport();
    live.check(
        dense.outcome == Some(PresentOutcome::Presented)
            && live.studio.pixel_density() == PixelDensity::Backing
            && now.logical_width == 640.0
            && now.logical_height == 400.0
            && view.drawable() == backing
            && view.pixels_per_point() == density
            && now.to_drawable(LogicalPoint { x: 10.5, y: 3.0 })
                == Some([
                    10.5 * f64::from(backing[0]) / 640.0,
                    3.0 * f64::from(backing[1]) / 400.0,
                ]),
        format!(
            "frames are drawn at the {density}x screen's density: {} x {} pixels in the 640 x 400 \
             point window, which keeps its size, the view at {} pixels per point",
            backing[0],
            backing[1],
            view.pixels_per_point()
        ),
    )?;
    let to_logical = |at: [f64; 2]| LogicalPoint {
        x: at[0] / density,
        y: at[1] / density,
    };
    live.check(
        live.studio.controls().tally().focus_gained >= 1,
        "a real FocusGained event arrives when the window becomes key",
    )?;
    live.check(
        shown_tick(&frame) == Some(0)
            && live.studio.view().cloud().draws().len() == founders as usize,
        format!("the founding epoch is on screen as {founders} dots in one cloud"),
    )?;
    let reserved = live.studio.view().cloud().reserved_bytes();
    live.check(
        live.studio.view().scene_objects() == 4
            && live.studio.view().cloud().capacity() == ceiling as usize,
        format!(
            "four scene objects (the field, one cloud, its density layer and the selection \
             ring), the cloud reserved once for the {ceiling}-organism ceiling"
        ),
    )?;

    // Space runs: the live world advances on screen.
    live.key(KEY_SPACE, " ")?;
    live.until("the run to start", |_, _, statuses| {
        statuses.contains(&Status::Running {
            run: 1,
            from: Tick::ZERO,
        })
    })?;
    let frame = live.until("live epochs past tick 3", |studio, frame, _| {
        shown_tick(frame).is_some_and(|tick| tick >= 3) && studio.controls().running()
    })?;
    live.check(
        frame.outcome.is_some(),
        format!(
            "Space runs the session and live epochs reach the screen (tick {:?})",
            shown_tick(&frame)
        ),
    )?;

    // Space again pauses at a committed boundary.
    live.key(KEY_SPACE, " ")?;
    live.until("the pause and the end of run 1", |_, _, statuses| {
        let paused = statuses
            .iter()
            .any(|status| matches!(status, Status::Paused { .. }));
        let ended = statuses.iter().any(|status| {
            matches!(
                status,
                Status::Ended {
                    run: 1,
                    why: End::Paused,
                    ..
                }
            )
        });
        paused && ended
    })?;
    let paused = live
        .statuses
        .iter()
        .find_map(|status| match status {
            Status::Paused { at } => Some(at.get()),
            _ => None,
        })
        .ok_or("no pause boundary")?;
    let frame = live.until("the paused boundary on screen, at rest", |_, frame, _| {
        shown_tick(frame) == Some(paused) && frame.motion.is_none()
    })?;
    live.check(
        !live.studio.controls().running()
            && shown_tick(&frame) == Some(paused)
            && live.studio.display_pair().0.is_none(),
        format!(
            "Space pauses at tick {paused}, where the display comes to rest, holding only that \
             epoch's lease"
        ),
    )?;
    // Paused stays paused: no newer epoch arrives.
    let settled = live.studio.tally().epochs;
    for _ in 0..10 {
        live.until("a paused frame", |_, _, _| true)?;
    }
    live.check(
        live.studio.tally().epochs == settled,
        "no epoch arrives while paused",
    )?;

    // → and `.` each step exactly one tick. FL-020: a step is shown one
    // committed state late, over MOTION_MAX. Every frame while the display
    // moves draws the organisms exactly where `interpolate` puts them
    // between the epochs the Studio holds, at the frame's own fraction. The
    // `.` is pressed as soon as the → step is on screen, so it arrives while
    // the display is still moving: the display starts again from the → step's
    // committed state, again over MOTION_MAX, and comes to rest on the `.`
    // step.
    live.key(KEY_RIGHT, "\u{f703}")?;
    let mut audit = MotionAudit::default();
    let first = live.until(
        "the right-arrow step on screen",
        |studio, frame, statuses| {
            audit.see(studio, frame);
            statuses.contains(&Status::Stepped {
                tick: Tick::new(paused + 1),
            }) && shown_tick(frame) == Some(paused + 1)
        },
    )?;
    live.key(KEY_PERIOD, ".")?;
    let mut second: Option<(Option<Motion>, Option<Duration>)> = None;
    let rest = live.until("the period step, at rest", |studio, frame, statuses| {
        let before = audit.see(studio, frame);
        if second.is_none() && shown_tick(frame) == Some(paused + 2) {
            second = Some((before, studio.pacer().interval()));
        }
        statuses.contains(&Status::Stepped {
            tick: Tick::new(paused + 2),
        }) && shown_tick(frame) == Some(paused + 2)
            && frame.motion.is_none()
    })?;
    let (held, current) = live.studio.display_pair();
    let resting = live.studio.view().shown_rows().eq(current
        .ids()
        .iter()
        .zip(current.positions())
        .map(|(&id, p)| (id, p.map(f64::from))));
    live.check(
        audit.wrong.is_empty()
            && first.motion.map(|motion| motion.fraction) == Some(0.0)
            && audit.arrivals >= 2
            && audit.part_way > 0
            && rest.motion.is_none()
            && held.is_none()
            && resting,
        format!(
            "real steps are shown one committed state late: each arrival frame draws every \
             organism at its previous committed position, then {} moving frames ({} part way) \
             each draw exactly what interpolate places by identity at the frame's fraction, with \
             up to {} dead fading and the born waiting, until the display rests on tick {} with \
             only that epoch held ({:?})",
            audit.moving,
            audit.part_way,
            audit.faded,
            paused + 2,
            audit.wrong
        ),
    )?;
    let (interrupted, interval) = second.unwrap_or((None, None));
    live.check(
        interrupted.is_some_and(|motion| motion.prior.tick.get() == paused)
            && interval == Some(MOTION_MAX),
        format!(
            "a step taken while paused moves over MOTION_MAX even when it arrives while the \
             previous step is still moving (from tick {paused}, at fraction {:?}): interval \
             {interval:?}",
            interrupted.map(|motion| motion.fraction)
        ),
    )?;
    live.check(
        true,
        format!("→ and . each step one tick, to {}", paused + 2),
    )?;

    // FL-019: style keys redraw the shown epoch in the new style while
    // paused, through the live Metal presenter, and never reach the world.
    let at = paused + 2;
    let epochs = live.studio.tally().epochs;
    let organisms = live.studio.view().cloud().draws().len();
    let colours = |studio: &Studio| -> Vec<[u64; 3]> {
        studio
            .view()
            .cloud()
            .draws()
            .iter()
            .filter_map(|draw| match draw {
                fmn_render::ThreeDDraw::TrueDot(dot) => {
                    Some([dot.color.r, dot.color.g, dot.color.b].map(f64::to_bits))
                }
                _ => None,
            })
            .collect()
    };
    let lineage = colours(&live.studio);
    live.key(KEY_DIGITS[1], "2")?;
    live.until("the energy colours on screen", |studio, frame, _| {
        frame.restyled
            && frame.outcome.is_some()
            && studio.view().style().mapping == ColorMapping::Energy
    })?;
    let energy = colours(&live.studio);
    live.check(
        energy.len() == organisms && energy != lineage,
        format!("2 recolours the {organisms} dots of tick {at} by energy"),
    )?;
    live.key(KEY_R, "r")?;
    live.until("the raw look on screen", |studio, frame, _| {
        frame.outcome.is_some() && studio.view().style().look == Look::Raw
    })?;
    live.check(
        live.studio.view().cloud().halos().is_empty()
            && live.studio.view().cloud().draws().len() == organisms,
        "R draws the raw look: the dots without halos",
    )?;
    live.key(KEY_D, "d")?;
    let frame = live.until("the density view on screen", |studio, frame, _| {
        frame.outcome.is_some() && studio.view().style().particles == Particles::Density
    })?;
    let counted: u64 = live
        .studio
        .view()
        .density()
        .counts()
        .iter()
        .map(|&count| u64::from(count))
        .sum();
    live.check(
        live.studio.view().cloud().draws().is_empty()
            && counted == organisms as u64
            && frame.outcome.is_some(),
        format!(
            "D draws density on the Metal route: {:?} bins counting all {organisms} organisms",
            live.studio.view().density().bins()
        ),
    )?;
    live.key(KEY_F, "f")?;
    live.until("the field hidden", |studio, frame, _| {
        frame.outcome.is_some() && !studio.view().style().field
    })?;
    for (code, text) in [
        (KEY_DIGITS[0], "1"),
        (KEY_R, "r"),
        (KEY_D, "d"),
        (KEY_F, "f"),
    ] {
        live.key(code, text)?;
    }
    let restored = live.until("the default style again", |studio, frame, _| {
        frame.outcome.is_some() && studio.view().style() == Style::default()
    })?;
    live.check(
        colours(&live.studio) == lineage
            && live.studio.tally().epochs == epochs
            && live.studio.tally().restyles >= 5
            && shown_tick(&restored) == Some(at)
            && !live.studio.controls().running(),
        format!(
            "1, R, D and F restore the lineage colours bit for bit; all {} restyles kept tick {at} \
             on screen with no new epoch and no run",
            live.studio.tally().restyles
        ),
    )?;

    // A real resize refits the viewport and camera to the new content area,
    // at the screen's density.
    let refits = live.studio.tally().refits;
    live.post(&SyntheticInput::Resize {
        width: 800.0,
        height: 450.0,
    })?;
    let resized = SurfaceMetrics {
        logical_width: 800.0,
        logical_height: 450.0,
        ..metrics
    }
    .backing_size()
    .ok_or("no backing size for 800 x 450 points")?;
    live.until("the resize", |studio, _, _| {
        studio.view().viewport().drawable() == resized
    })?;
    let viewport = *live.studio.view().viewport();
    let [wide, high] = resized.map(f64::from);
    let scale = (wide / 128.0).min(high / 128.0);
    live.check(
        viewport.scale() == scale
            && viewport.origin() == [(wide - 128.0 * scale) / 2.0, 0.0]
            && viewport.pixels_per_point() == density
            && live.studio.tally().refits == refits + 1
            && live.studio.view().cloud().reserved_bytes() == reserved,
        format!(
            "a real resize to 800 x 450 points refits the viewport to {} x {} pixels at {scale} px \
             per world unit, with no new draw storage",
            resized[0], resized[1]
        ),
    )?;
    let after = live.until("a frame at the new size", |studio, frame, _| {
        frame.outcome.is_some()
            && studio.surface_metrics().is_ok_and(|metrics| {
                [metrics.drawable_width, metrics.drawable_height] == resized
                    && metrics.logical_width == 800.0
            })
    })?;
    live.check(
        after.outcome.is_some(),
        "the next frame is presented at the new drawable size, the window at 800 x 450 points",
    )?;

    // The pointer reports the world point under it.
    let world = [32.0, 96.0];
    let at = viewport.world_to_drawable(world);
    live.post(&SyntheticInput::PointerMoved {
        position: to_logical(at),
        dragging: None,
        modifiers: Modifiers::NONE,
    })?;
    let frame = live.until("the hover point", |_, frame, _| frame.hover.is_some())?;
    let hover = frame.hover.ok_or("no hover")?;
    live.check(
        (hover[0] - world[0]).abs() <= 1.0 / scale && (hover[1] - world[1]).abs() <= 1.0 / scale,
        format!("real pointer motion hovers world point {hover:?}, within a pixel of {world:?}"),
    )?;
    live.post(&SyntheticInput::Scroll {
        delta: [0, -3],
        position: to_logical(at),
    })?;
    live.until("the scroll", |studio, _, _| {
        studio.controls().tally().scrolls >= 1
    })?;
    live.check(true, "a real scroll arrives at the controls")?;

    // Losing focus with a button held loses the capture first.
    live.post(&SyntheticInput::Button {
        button: PointerButton::Primary,
        pressed: true,
        position: to_logical(at),
        clicks: 1,
        modifiers: Modifiers::NONE,
    })?;
    live.until("the button press", |studio, _, _| {
        studio.controls().buttons().contains(PointerButton::Primary)
    })?;
    let before = live.studio.controls().tally();
    live.post(&SyntheticInput::Focus(false))?;
    live.until("focus loss", |studio, _, _| !studio.controls().focused())?;
    let tally = live.studio.controls().tally();
    live.check(
        tally.capture_lost == before.capture_lost + 1
            && tally.focus_lost == before.focus_lost + 1
            && live.studio.controls().buttons().is_empty(),
        format!(
            "real focus loss with a button held loses the pointer capture and clears the drag \
             (before {before:?}, after {tally:?}, buttons {:?})",
            live.studio.controls().buttons()
        ),
    )?;
    live.post(&SyntheticInput::Focus(true))?;
    live.until("focus regained", |studio, _, _| studio.controls().focused())?;
    live.check(
        live.studio.controls().tally().focus_gained > tally.focus_gained,
        "real focus regain arrives as FocusGained",
    )?;

    // FL-020: the camera and selection through real pointer input, posted
    // at the logical point of each drawable position.
    let fitted = *live.studio.view().viewport();
    let point = to_logical;
    let torus = |a: [f64; 2], b: [f64; 2]| {
        let axis = |d: f64| d.abs().min(128.0 - d.abs());
        axis(a[0] - b[0]).max(axis(a[1] - b[1]))
    };
    let world = [32.0, 96.0];
    let at = fitted.world_to_drawable(world);
    let camera_moves = live.studio.tally().camera_moves;
    live.post(&SyntheticInput::Scroll {
        delta: [0, 50],
        position: point(at),
    })?;
    live.until("the zoom", |studio, frame, _| {
        frame.outcome.is_some() && studio.view().viewport().zoom() > 1.0
    })?;
    let zoomed = *live.studio.view().viewport();
    let under = zoomed
        .drawable_to_world(at)
        .ok_or("no world under the pointer")?;
    live.check(
        torus(under, world) < 1e-9 && live.studio.tally().camera_moves == camera_moves + 1,
        format!(
            "a real scroll up zooms {}x about the pointer, which stays over world point {world:?} \
             ({under:?})",
            zoomed.zoom()
        ),
    )?;
    // The paused epoch is placed again in the new view: every dot is at
    // its organism's scene point there.
    let placed = live
        .studio
        .view()
        .shown_rows()
        .zip(live.studio.view().cloud().draws())
        .all(|((_, position), draw)| match draw {
            fmn_render::ThreeDDraw::TrueDot(dot) => {
                let [x, y, z] = zoomed.scene_point(position);
                dot.center[0] == x && dot.center[1] == y && dot.center[2] == z
            }
            _ => false,
        });
    live.check(
        placed
            && live.studio.view().cloud().draws().len() == live.studio.view().shown_rows().count(),
        "the paused epoch is placed again in the zoomed view: every dot at its scene point there",
    )?;

    // The window's center, and a drag of (40, 25) points from it.
    let press = [400.0 * density, 225.0 * density];
    let grabbed = zoomed
        .drawable_to_world(press)
        .ok_or("no world under the press")?;
    let release = [press[0] + 40.0 * density, press[1] + 25.0 * density];
    let before = live.studio.tally();
    let buttons = live.studio.controls().tally().buttons;
    live.post(&SyntheticInput::Button {
        button: PointerButton::Primary,
        pressed: true,
        position: point(press),
        clicks: 1,
        modifiers: Modifiers::NONE,
    })?;
    for step in [[20.0, 10.0], [40.0, 25.0]] {
        live.post(&SyntheticInput::PointerMoved {
            position: point([press[0] + step[0] * density, press[1] + step[1] * density]),
            dragging: Some(PointerButton::Primary),
            modifiers: Modifiers::NONE,
        })?;
    }
    live.post(&SyntheticInput::Button {
        button: PointerButton::Primary,
        pressed: false,
        position: point(release),
        clicks: 1,
        modifiers: Modifiers::NONE,
    })?;
    live.until("the drag", |studio, frame, _| {
        frame.outcome.is_some()
            && studio.controls().tally().buttons >= buttons + 2
            && studio.controls().buttons().is_empty()
    })?;
    let panned = *live.studio.view().viewport();
    let held = panned
        .drawable_to_world(release)
        .ok_or("no world under the release")?;
    live.check(
        torus(held, grabbed) < 1e-9
            && panned.zoom() == zoomed.zoom()
            && live.studio.tally().camera_moves > before.camera_moves
            && live.studio.tally().selections == before.selections,
        format!(
            "a real primary drag of (40, 25) points pans the view: world point {grabbed:?} \
             followed the pointer to the release ({held:?}), and selects nothing"
        ),
    )?;

    // A click selects the organism drawn under it, by identity.
    let rows: Vec<(u64, [f64; 2])> = live.studio.view().shown_rows().collect();
    let distance = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    let (target, spot) = rows
        .iter()
        .map(|&(id, position)| (id, panned.world_to_drawable(position)))
        .min_by(|a, b| distance(a.1, press).total_cmp(&distance(b.1, press)))
        .ok_or("no organisms shown")?;
    // The Studio picks within PICK_RADIUS_POINTS points: that many pixels
    // times the density.
    let radius = PICK_RADIUS_POINTS * density;
    let expected = flife_manim::pick(&panned, rows.iter().copied(), spot, radius);
    let click = |live: &mut Live, at: [f64; 2]| -> Result<(), String> {
        for pressed in [true, false] {
            live.post(&SyntheticInput::Button {
                button: PointerButton::Primary,
                pressed,
                position: point(at),
                clicks: 1,
                modifiers: Modifiers::NONE,
            })?;
        }
        Ok(())
    };
    let selections = live.studio.tally().selections;
    click(&mut live, spot)?;
    let frame = live.until("the selection", |studio, frame, _| {
        frame.outcome.is_some() && studio.tally().selections > selections
    })?;
    let ringed = matches!(
        live.studio
            .view()
            .draws()
            .map_err(|error| error.to_string())?
            .last(),
        Some(fmn_render::ThreeDDraw::Surface(_))
    );
    live.check(
        expected == Some(OrganismId::new(target))
            && frame.selected == expected
            && ringed
            && live.studio.view().selected_position().is_some(),
        format!(
            "a real click selects organism {target}, drawn under it, by identity, and rings it \
             as the frame's last draw"
        ),
    )?;

    // A step: the selection is kept only if the new epoch has the organism.
    live.key(KEY_RIGHT, "\u{f703}")?;
    let frame = live.until("the step after selecting, at rest", |_, frame, statuses| {
        statuses.contains(&Status::Stepped {
            tick: Tick::new(paused + 3),
        }) && shown_tick(frame) == Some(paused + 3)
            && frame.motion.is_none()
    })?;
    let alive = live.studio.view().shown_rows().any(|(id, _)| id == target);
    live.check(
        frame.selected == alive.then_some(OrganismId::new(target)),
        format!(
            "after a step the selection is held by identity: organism {target} {}",
            if alive {
                "lives and stays selected"
            } else {
                "died and is deselected"
            }
        ),
    )?;

    // A click where nothing is drawn selects nothing.
    let viewport = *live.studio.view().viewport();
    let shown: Vec<(u64, [f64; 2])> = live.studio.view().shown_rows().collect();
    let empty = (2..44)
        .flat_map(|y| {
            (2..80).map(move |x| [f64::from(x) * 10.0 * density, f64::from(y) * 10.0 * density])
        })
        .find(|&at| flife_manim::pick(&viewport, shown.iter().copied(), at, radius).is_none())
        .ok_or("no empty spot in the window")?;
    let selections = live.studio.tally().selections;
    click(&mut live, empty)?;
    let frame = live.until("the deselection", |studio, frame, _| {
        frame.outcome.is_some() && studio.tally().selections > selections
    })?;
    live.check(
        frame.selected.is_none() && live.studio.view().selection_draw().is_none(),
        format!("a real click on an empty spot {empty:?} selects nothing and drops the ring"),
    )?;

    // 0 shows the whole torus again.
    live.key(KEY_ZERO, "0")?;
    live.until("the reset", |studio, frame, _| {
        frame.outcome.is_some() && studio.view().viewport().zoom() == 1.0
    })?;
    live.check(
        *live.studio.view().viewport() == fitted,
        "0 shows the whole torus again: the fitted viewport, bit for bit",
    )?;

    // FL-022: E arms the energy brush, and a real click becomes a command
    // for the next boundary, its cell the one under the click through the
    // view as it stood; the next step applies it.
    live.key(KEY_E, "e")?;
    live.until("the energy brush", |_, frame, _| {
        frame.brush == Some(Brush::Energy)
    })?;
    let metrics = live.studio.controls().metrics().ok_or("no metrics")?;
    let logical = [metrics.logical_width, metrics.logical_height];
    let grid = [
        config.config().world.grid_width,
        config.config().world.grid_height,
    ];
    let at = fitted.world_to_drawable([70.3, 41.6]);
    let expected = flife_studio::stroke(
        Brush::Energy,
        &fitted,
        logical,
        grid,
        [at[0] / density, at[1] / density],
        &flife_studio::Supplies {
            material: None,
            founder_energy: config.config().population.initial_energy,
            settings: flife_studio::BrushSettings::default(),
        },
    )
    .ok_or("no stroke under the click")?;
    let selections = live.studio.tally().selections;
    click(&mut live, at)?;
    live.until("the stroke's acceptance", |_, _, statuses| {
        statuses
            .iter()
            .any(|status| matches!(status, Status::Submitted { .. }))
    })?;
    let submitted = live.statuses.iter().find_map(|status| match status {
        Status::Submitted {
            command,
            stroke,
            seen,
            effective,
        } => Some((command.get(), *stroke, seen.get(), effective.get())),
        _ => None,
    });
    live.check(
        submitted == Some((1, expected, paused + 3, paused + 3))
            && live.studio.tally().selections == selections
            && live.studio.tally().strokes == 1,
        format!(
            "a real click with the energy brush armed is command 1, {expected:?}, for the \
             boundary of tick {}, and selects nothing",
            paused + 3
        ),
    )?;
    live.key(KEY_RIGHT, "\u{f703}")?;
    live.until("the stroke applied", |_, _, statuses| {
        statuses
            .iter()
            .any(|status| matches!(status, Status::Finished { .. }))
    })?;
    let applied = live.statuses.iter().find_map(|status| match status {
        Status::Finished {
            outcome:
                Outcome::Applied {
                    tick,
                    published,
                    applied,
                    ..
                },
            ..
        } => Some((tick.get(), published.get(), *applied)),
        _ => None,
    });
    let Some((applied_tick, published_tick, applied_energy)) = applied else {
        return Err(format!("the stroke did not apply: {:?}", live.statuses));
    };
    live.check(
        (applied_tick, published_tick) == (paused + 3, paused + 4),
        format!(
            "the next step applies it at tick {applied_tick}, published at {published_tick}, \
             moving {applied_energy} energy"
        ),
    )?;
    live.key(KEY_E, "e")?;
    live.until("the brush disarmed", |_, frame, _| frame.brush.is_none())?;

    // Esc closes.
    live.key(KEY_ESCAPE, "\u{1b}")?;
    live.until("the close request", |studio, _, _| studio.should_close())?;
    let refused = live.studio.tally().refused;
    let dropped = live.studio.input_dropped();
    live.check(
        refused == 0 && dropped == 0,
        "no intent was refused and no input was dropped",
    )?;
    let mut checks = std::mem::take(&mut live.checks);
    let closed = live
        .studio
        .close()
        .map_err(|error| format!("closing: {error}"))?;
    let final_tick = paused + 4;
    if closed.session.boundary.tick.get() != final_tick {
        return Err(format!(
            "closed at tick {}, expected {final_tick}",
            closed.session.boundary.tick.get()
        ));
    }
    checks.push(format!("Esc closes the session at tick {final_tick}"));

    // The live session's world is the directly stepped world, bit for bit.
    let mut direct = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    // The brush's stroke at its tick, as the session applied it.
    for tick in 0..final_tick {
        let interventions = if tick == applied_tick {
            vec![expected.intervention()]
        } else {
            Vec::new()
        };
        match direct
            .step_with(&interventions)
            .map_err(|error| error.to_string())?
        {
            Stepped::Published(report) => {
                if let Some(applied) = report.interventions.first()
                    && applied.applied.to_bits() != applied_energy.to_bits()
                {
                    return Err(format!(
                        "the direct world's stroke moved {}, the session's {applied_energy}",
                        applied.applied
                    ));
                }
            }
            Stepped::Paused(shortfall) => {
                return Err(format!("the direct world paused: {shortfall:?}"));
            }
        }
    }
    checks.push(format!(
        "the direct world given the stroke at tick {applied_tick} moves the same {applied_energy}, \
         bit for bit"
    ));
    let live_digest = closed.session.world.digest();
    let direct_digest = direct.digest();
    if live_digest != direct_digest {
        return Err(format!(
            "the live world {} is not the directly stepped world {}",
            live_digest.to_hex(),
            direct_digest.to_hex()
        ));
    }
    checks.push(format!(
        "the committed world after {final_tick} live ticks is the directly stepped world ({})",
        live_digest.to_hex()
    ));
    Ok(checks)
}
