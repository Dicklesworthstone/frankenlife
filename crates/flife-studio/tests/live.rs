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

use flife_core::{Tick, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_studio::{
    End, Frame, KEY_ESCAPE, KEY_PERIOD, KEY_RIGHT, KEY_SPACE, Status, Studio, StudioOptions,
};
use fmn_render::metal::{LogicalPoint, Modifiers, PointerButton, SyntheticInput};

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

fn live() -> Result<Vec<String>, String> {
    let config = ValidatedConfig::from_toml(PRESET).map_err(|error| error.to_string())?;
    let founders = config.config().population.initial_count;
    let ceiling = config.config().population.maximum_count;
    let options = StudioOptions {
        title: "FrankenLife Studio (live test)".into(),
        size: Some([640, 400]),
        resizable: true,
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
        live.studio.view().scene_objects() == 1
            && live.studio.view().cloud().capacity() == ceiling as usize,
        format!("one scene object, its draws reserved once for the {ceiling}-organism ceiling"),
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
    let frame = live.until("the paused boundary on screen", |_, frame, _| {
        shown_tick(frame) == Some(paused)
    })?;
    live.check(
        !live.studio.controls().running() && shown_tick(&frame) == Some(paused),
        format!("Space pauses at tick {paused}, which is what the screen shows"),
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

    // → and `.` each step exactly one tick.
    live.key(KEY_RIGHT, "\u{f703}")?;
    live.until("the right-arrow step", |_, frame, statuses| {
        statuses.contains(&Status::Stepped {
            tick: Tick::new(paused + 1),
        }) && shown_tick(frame) == Some(paused + 1)
    })?;
    live.key(KEY_PERIOD, ".")?;
    live.until("the period step", |_, frame, statuses| {
        statuses.contains(&Status::Stepped {
            tick: Tick::new(paused + 2),
        }) && shown_tick(frame) == Some(paused + 2)
    })?;
    live.check(
        true,
        format!("→ and . each step one tick, to {}", paused + 2),
    )?;

    // A real resize refits the viewport and camera to the new content area.
    live.post(&SyntheticInput::Resize {
        width: 800.0,
        height: 450.0,
    })?;
    live.until("the resize", |studio, _, _| {
        studio.view().viewport().drawable() == [800, 450]
    })?;
    let viewport = *live.studio.view().viewport();
    let scale = (800.0_f64 / 128.0).min(450.0 / 128.0);
    live.check(
        viewport.scale() == scale
            && viewport.origin() == [(800.0 - 128.0 * scale) / 2.0, 0.0]
            && live.studio.tally().refits == 1
            && live.studio.view().cloud().reserved_bytes() == reserved,
        format!("a real resize refits the viewport to 800 x 450 at {scale} px per world unit, with no new draw storage"),
    )?;
    let after = live.until("a frame at the new size", |studio, frame, _| {
        frame.outcome.is_some()
            && studio.surface_metrics().is_ok_and(|metrics| {
                metrics.drawable_width == 800 && metrics.drawable_height == 450
            })
    })?;
    live.check(
        after.outcome.is_some(),
        "the next frame is presented at the new drawable size",
    )?;

    // The pointer reports the world point under it.
    let world = [32.0, 96.0];
    let at = viewport.world_to_drawable(world);
    live.post(&SyntheticInput::PointerMoved {
        position: LogicalPoint { x: at[0], y: at[1] },
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
        position: LogicalPoint { x: at[0], y: at[1] },
    })?;
    live.until("the scroll", |studio, _, _| {
        studio.controls().tally().scrolls >= 1
    })?;
    live.check(true, "a real scroll arrives at the controls")?;

    // Losing focus with a button held loses the capture first.
    live.post(&SyntheticInput::Button {
        button: PointerButton::Primary,
        pressed: true,
        position: LogicalPoint { x: at[0], y: at[1] },
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
        "real focus loss with a button held loses the pointer capture and clears the drag",
    )?;
    live.post(&SyntheticInput::Focus(true))?;
    live.until("focus regained", |studio, _, _| studio.controls().focused())?;
    live.check(
        live.studio.controls().tally().focus_gained > tally.focus_gained,
        "real focus regain arrives as FocusGained",
    )?;

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
    let final_tick = paused + 2;
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
    for _ in 0..final_tick {
        match direct.step().map_err(|error| error.to_string())? {
            Stepped::Published(_) => {}
            Stepped::Paused(shortfall) => {
                return Err(format!("the direct world paused: {shortfall:?}"));
            }
        }
    }
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
