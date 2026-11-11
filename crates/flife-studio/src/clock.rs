//! The Studio's honest clocks (plan §12.2): achieved scientific steps per
//! wall second and the simulation-time to wall-time ratio, beside the
//! presentation clock's frames, for `[presentation] show_clock_telemetry`.
//!
//! The scientific tick and the presentation clock are separate: the
//! display draws the latest committed world at its own rate, and a slow
//! world is reported slow, never stretched. A [`ClockMeter`] reads them
//! over windows of wall time: how far the session's committed tick (the
//! newest epoch in the render feed) and the presented frames advanced, and
//! how far the shown epoch lags the committed one.

use std::time::Instant;

use flife_core::{Tick, TickSpan, TimeStep};

/// The clocks over one window of wall time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockReading {
    /// The window's length in wall seconds.
    pub wall_seconds: f64,
    /// Scientific ticks committed in it.
    pub ticks: u64,
    /// Ticks committed per wall second.
    pub ticks_per_second: f64,
    /// Simulated seconds per wall second: the window's ticks times `Δt`,
    /// over its wall seconds. 1 is real time.
    pub simulation_rate: f64,
    /// Frames presented in it.
    pub frames: u64,
    /// Frames presented per wall second.
    pub frames_per_second: f64,
    /// The newest committed tick at its end.
    pub committed: Tick,
    /// The tick of the epoch shown at its end.
    pub shown: Option<Tick>,
    /// How many ticks the shown epoch is behind the committed one: the
    /// display's backlog.
    pub behind: Option<u64>,
}

/// Where a window opened.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Mark {
    at: Instant,
    committed: Tick,
    presented: u64,
}

/// Reads the clocks over successive windows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockMeter {
    step: TimeStep,
    opened: Option<Mark>,
}

impl ClockMeter {
    /// A meter for a world of timestep `step` that has read nothing.
    #[must_use]
    pub const fn new(step: TimeStep) -> Self {
        Self { step, opened: None }
    }

    /// Read at `now`, with the session's newest committed tick
    /// `committed`, `presented` frames presented so far and the shown
    /// epoch's tick. The first read opens a window and returns `None`;
    /// each later one closes the window since the previous read into a
    /// reading and opens the next. A read no later than the window's start
    /// returns `None` and keeps the window open.
    pub fn read(
        &mut self,
        now: Instant,
        committed: Tick,
        presented: u64,
        shown: Option<Tick>,
    ) -> Option<ClockReading> {
        let mark = Mark {
            at: now,
            committed,
            presented,
        };
        let Some(opened) = self.opened else {
            self.opened = Some(mark);
            return None;
        };
        let wall_seconds = now.checked_duration_since(opened.at)?.as_secs_f64();
        if wall_seconds <= 0.0 {
            return None;
        }
        self.opened = Some(mark);
        // Both counters only advance; a counter that went back reads zero.
        let ticks = committed.get().saturating_sub(opened.committed.get());
        let frames = presented.saturating_sub(opened.presented);
        Some(ClockReading {
            wall_seconds,
            ticks,
            ticks_per_second: ticks as f64 / wall_seconds,
            simulation_rate: self.step.seconds(TickSpan::new(ticks)) / wall_seconds,
            frames,
            frames_per_second: frames as f64 / wall_seconds,
            committed,
            shown,
            behind: shown.map(|shown| committed.get().saturating_sub(shown.get())),
        })
    }
}
