//! FL-024's frame-time histogram (G6 evidence): its percentiles against a
//! sorted list of the same frame times, in fixed memory for any number of
//! frames.

#![allow(
    clippy::expect_used,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "test: a failure is a test failure"
)]

use std::time::Duration;

use flife_studio::{FRAME_TIME_BUCKET, FRAME_TIME_RANGE, FrameTimes};

fn ms(milliseconds: u64) -> Duration {
    Duration::from_millis(milliseconds)
}

/// The order statistic a percentile stands for: the `⌈p · n⌉`-th shortest.
fn order_statistic(sorted: &[Duration], p: f64) -> Duration {
    let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

#[test]
fn an_empty_histogram_has_no_percentiles() {
    let times = FrameTimes::new().expect("buckets");
    assert_eq!(times.frames(), 0);
    assert_eq!(times.longest(), Duration::ZERO);
    assert_eq!(times.percentile(0.5), None);
    assert_eq!(times.summary(), None);
}

#[test]
fn percentiles_are_the_upper_edge_of_their_frames_bucket() {
    let mut times = FrameTimes::new().expect("buckets");
    // 1 ms to 100 ms, in reverse: the order counted in does not matter.
    for frame in (1..=100).rev() {
        times.record(ms(frame));
    }
    assert_eq!(times.frames(), 100);
    assert_eq!(times.longest(), ms(100));
    // The 50th of 100 frames is the 50 ms one, in the bucket [50, 50.1).
    assert_eq!(times.percentile(0.5), Some(ms(50) + FRAME_TIME_BUCKET));
    assert_eq!(times.percentile(0.95), Some(ms(95) + FRAME_TIME_BUCKET));
    assert_eq!(times.percentile(0.99), Some(ms(99) + FRAME_TIME_BUCKET));
    // The 100th is the longest, and no percentile exceeds it.
    assert_eq!(times.percentile(1.0), Some(ms(100)));
    // A fraction between ranks takes the next: 0.501 of 100 is the 51st.
    assert_eq!(times.percentile(0.501), Some(ms(51) + FRAME_TIME_BUCKET));
    // The smallest fraction is the shortest frame's bucket.
    assert_eq!(times.percentile(1e-9), Some(ms(1) + FRAME_TIME_BUCKET));
    let summary = times.summary().expect("a summary");
    assert_eq!(summary.frames, 100);
    assert!((summary.p50_ms - 50.1).abs() < 1e-9);
    assert!((summary.p95_ms - 95.1).abs() < 1e-9);
    assert!((summary.p99_ms - 99.1).abs() < 1e-9);
    assert!((summary.max_ms - 100.0).abs() < 1e-9);
    // Fractions outside (0, 1] have none.
    for p in [0.0, -0.5, 1.5, f64::NAN, f64::INFINITY] {
        assert_eq!(times.percentile(p), None, "{p}");
    }
}

#[test]
fn frames_past_the_range_are_counted_and_the_longest_is_exact() {
    let mut times = FrameTimes::new().expect("buckets");
    times.record(ms(5));
    let long = FRAME_TIME_RANGE + Duration::from_micros(1_234_567);
    for _ in 0..3 {
        times.record(FRAME_TIME_RANGE);
    }
    times.record(long);
    assert_eq!(times.frames(), 5);
    assert_eq!(times.longest(), long);
    assert_eq!(times.percentile(0.2), Some(ms(5) + FRAME_TIME_BUCKET));
    // Every later frame took the range or more: the longest stands for them.
    assert_eq!(times.percentile(0.4), Some(long));
    assert_eq!(times.percentile(1.0), Some(long));
    // A frame just under the range is in the last bucket.
    let mut edge = FrameTimes::new().expect("buckets");
    edge.record(FRAME_TIME_RANGE - Duration::from_nanos(1));
    assert_eq!(
        edge.percentile(1.0),
        Some(FRAME_TIME_RANGE - Duration::from_nanos(1))
    );
}

#[test]
fn every_percentile_is_within_a_bucket_above_its_frame() {
    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut times = FrameTimes::new().expect("buckets");
    let mut all = Vec::new();
    for _ in 0..20_000 {
        // Mostly 5 to 30 ms, a few long frames, some past the range.
        let nanos = match next() % 100 {
            0 => 1_000_000_000 + next() % 500_000_000,
            1..=4 => 30_000_000 + next() % 400_000_000,
            _ => 5_000_000 + next() % 25_000_000,
        };
        let time = Duration::from_nanos(nanos);
        times.record(time);
        all.push(time);
    }
    all.sort_unstable();
    assert_eq!(times.longest(), *all.last().expect("frames"));
    for p in [0.001, 0.25, 0.5, 0.9, 0.95, 0.99, 0.995, 0.999, 1.0] {
        let truth = order_statistic(&all, p);
        let got = times.percentile(p).expect("a percentile");
        if truth >= FRAME_TIME_RANGE {
            assert_eq!(got, times.longest(), "{p}");
        } else {
            assert!(
                got >= truth && got <= truth + FRAME_TIME_BUCKET,
                "{p}: {got:?} for {truth:?}"
            );
            assert!(got <= times.longest());
        }
    }
}
