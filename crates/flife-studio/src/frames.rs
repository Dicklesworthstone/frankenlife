//! Frame times (FL-024's G6 evidence, plan §13.7): how long each frame's
//! work takes, as a bounded histogram, so a run of any length reports its
//! percentiles in fixed memory.

use std::time::Duration;

use flife_core::CoreError;
use serde::Serialize;

/// The histogram's resolution: a frame's time is counted in the bucket of
/// whole tenths of a millisecond below it.
pub const FRAME_TIME_BUCKET: Duration = Duration::from_micros(100);

/// The longest frame time the histogram resolves; longer frames are counted
/// above it, and the longest is kept exactly.
pub const FRAME_TIME_RANGE: Duration = Duration::from_secs(1);

const BUCKETS: usize = 10_000;

/// Each frame's time, counted in [`FRAME_TIME_BUCKET`]s up to
/// [`FRAME_TIME_RANGE`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameTimes {
    buckets: Vec<u64>,
    above: u64,
    frames: u64,
    longest: Duration,
}

/// A [`FrameTimes`]' percentiles ([`FrameTimes::percentile`]), in
/// milliseconds: each at most [`FRAME_TIME_BUCKET`] above the frame time
/// it stands for.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct FrameTimeSummary {
    /// Frames counted.
    pub frames: u64,
    /// The median.
    pub p50_ms: f64,
    /// The 95th percentile.
    pub p95_ms: f64,
    /// The 99th percentile.
    pub p99_ms: f64,
    /// The longest frame, exactly.
    pub max_ms: f64,
}

impl FrameTimes {
    /// An empty histogram.
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when its buckets cannot be reserved.
    pub fn new() -> Result<Self, CoreError> {
        let mut buckets = Vec::new();
        buckets
            .try_reserve_exact(BUCKETS)
            .map_err(|_| CoreError::AllocationFailed {
                buffer: "frame-time buckets",
                bytes: (BUCKETS * size_of::<u64>()) as u64,
            })?;
        buckets.resize(BUCKETS, 0);
        Ok(Self {
            buckets,
            above: 0,
            frames: 0,
            longest: Duration::ZERO,
        })
    }

    /// Count one frame that took `time`.
    pub fn record(&mut self, time: Duration) {
        self.frames += 1;
        self.longest = self.longest.max(time);
        let index = time.as_nanos() / FRAME_TIME_BUCKET.as_nanos();
        match usize::try_from(index) {
            Ok(index) if index < BUCKETS => self.buckets[index] += 1,
            _ => self.above += 1,
        }
    }

    /// Frames counted.
    #[must_use]
    pub const fn frames(&self) -> u64 {
        self.frames
    }

    /// The longest frame's time.
    #[must_use]
    pub const fn longest(&self) -> Duration {
        self.longest
    }

    /// The time a fraction `p` of frames take at most: the upper edge of
    /// the bucket holding the `⌈p · frames⌉`-th shortest frame (or the
    /// longest frame's time, when that is shorter), and the longest frame's
    /// time when that frame took [`FRAME_TIME_RANGE`] or more. `None`
    /// before a frame, or for a `p` outside (0, 1].
    #[must_use]
    pub fn percentile(&self, p: f64) -> Option<Duration> {
        if self.frames == 0 || !(p > 0.0 && p <= 1.0) {
            return None;
        }
        // The rank of the frame the percentile stands for, from 1.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "p is in (0, 1], so the rank is in [1, frames]"
        )]
        let rank = ((p * self.frames as f64).ceil() as u64).clamp(1, self.frames);
        let mut seen = 0;
        for (index, &count) in self.buckets.iter().enumerate() {
            seen += count;
            if seen >= rank {
                let edge = FRAME_TIME_BUCKET * u32::try_from(index + 1).unwrap_or(u32::MAX);
                return Some(edge.min(self.longest));
            }
        }
        Some(self.longest)
    }

    /// The median, 95th and 99th percentiles and the longest frame.
    /// `None` before a frame.
    #[must_use]
    pub fn summary(&self) -> Option<FrameTimeSummary> {
        let ms = |time: Duration| time.as_secs_f64() * 1e3;
        Some(FrameTimeSummary {
            frames: self.frames,
            p50_ms: ms(self.percentile(0.5)?),
            p95_ms: ms(self.percentile(0.95)?),
            p99_ms: ms(self.percentile(0.99)?),
            max_ms: ms(self.longest),
        })
    }
}
