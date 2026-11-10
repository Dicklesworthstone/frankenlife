//! Pacing the display one committed state behind the world (FL-020; plan
//! §13.5).
//!
//! When an epoch arrives, the Studio draws its organisms moving from the
//! epoch before it. While the world runs, the motion takes about as long as
//! epochs take to arrive, so the display reaches each committed state as
//! the next one comes; a step taken while paused moves over
//! [`MOTION_MAX`]. A [`Pacer`] measures the interval, smoothed and capped,
//! and turns a frame's time into how far the display has come.

use std::time::{Duration, Instant};

/// The longest the display takes to reach a newly shown epoch. A world
/// stepping slower than this, or one step taken while paused, moves over
/// this long, so a step is seen within G6's 100 ms response budget.
pub const MOTION_MAX: Duration = Duration::from_millis(100);

/// The weight of the newest interval between arrivals in the smoothed one.
pub const ARRIVAL_SMOOTHING: f64 = 0.25;

/// The interval at which epochs arrive, and when the last one did.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Pacer {
    arrived: Option<Instant>,
    /// Seconds: the smoothed interval between arrivals, at most
    /// [`MOTION_MAX`].
    interval: Option<f64>,
}

impl Pacer {
    /// A pacer that has seen nothing arrive.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            arrived: None,
            interval: None,
        }
    }

    /// An epoch arrived at `now`. The time since the previous arrival moves
    /// the smoothed interval by [`ARRIVAL_SMOOTHING`] of the difference (the
    /// first such time is the interval). A time of [`MOTION_MAX`] or more
    /// means epochs had stopped (a pause, a step, a slow world), and the
    /// interval starts again at [`MOTION_MAX`]. An arrival earlier than the
    /// last counts as no time.
    pub fn arrive(&mut self, now: Instant) {
        if let Some(last) = self.arrived {
            let gap = now.saturating_duration_since(last);
            self.interval = Some(if gap >= MOTION_MAX {
                MOTION_MAX.as_secs_f64()
            } else {
                let gap = gap.as_secs_f64();
                match self.interval {
                    Some(interval) => interval + ARRIVAL_SMOOTHING * (gap - interval),
                    None => gap,
                }
            });
        }
        self.arrived = Some(now);
    }

    /// An epoch arrived at `now` on its own rather than in a running
    /// stream (a step while paused): the display moves over the whole of
    /// [`MOTION_MAX`], and the interval starts again there.
    pub fn arrive_alone(&mut self, now: Instant) {
        self.interval = Some(MOTION_MAX.as_secs_f64());
        self.arrived = Some(now);
    }

    /// How far the display has come at `now` from the epoch before the
    /// last arrival towards it: the time since that arrival over the
    /// interval, in `[0, 1]`. It is 1 (at rest) before an interval is known
    /// or when it is zero.
    #[must_use]
    pub fn fraction(&self, now: Instant) -> f64 {
        match (self.arrived, self.interval) {
            (Some(at), Some(interval)) if interval > 0.0 => {
                (now.saturating_duration_since(at).as_secs_f64() / interval).min(1.0)
            }
            _ => 1.0,
        }
    }

    /// The smoothed interval between arrivals, once two have been seen.
    #[must_use]
    pub fn interval(&self) -> Option<Duration> {
        self.interval.map(Duration::from_secs_f64)
    }
}
