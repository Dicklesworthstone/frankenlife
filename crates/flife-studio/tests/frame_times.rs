//! FL-024's frame-time histogram (G6 evidence): its percentiles against a
//! sorted list of the same frame times, in fixed memory for any number of
//! frames. And the window's camera-move latency bound, against each move's
//! true latency from a known arrival.

#![allow(
    clippy::expect_used,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "test: a failure is a test failure"
)]

use std::time::{Duration, Instant};

use flife_studio::{FRAME_TIME_BUCKET, FRAME_TIME_RANGE, FrameTimes, MoveLatencyBound};

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

/// The moment `ms_after` milliseconds after `base`.
fn at(base: Instant, ms_after: u64) -> Instant {
    base + ms(ms_after)
}

#[test]
fn a_drawn_move_is_bounded_from_the_previous_poll_to_its_frames_end() {
    let base = Instant::now();
    let mut bound = MoveLatencyBound::new();
    // Polls every 33 ms, each frame's work 20 ms.
    assert_eq!(bound.frame(at(base, 0), at(base, 20), false, true), None);
    assert_eq!(
        bound.frame(at(base, 33), at(base, 53), true, true),
        Some(ms(53)),
        "drained at the 33 ms poll, so it arrived after the 0 ms one"
    );
    assert_eq!(bound.frame(at(base, 66), at(base, 86), false, true), None);
    assert_eq!(
        bound.frame(at(base, 99), at(base, 140), true, true),
        Some(ms(140 - 66))
    );
}

#[test]
fn a_move_in_the_first_frame_has_no_bound_and_none_carries_over() {
    let base = Instant::now();
    let mut bound = MoveLatencyBound::new();
    assert_eq!(bound.frame(at(base, 0), at(base, 20), true, true), None);
    assert_eq!(bound.frame(at(base, 33), at(base, 53), false, true), None);
    assert_eq!(bound, {
        let mut fresh = MoveLatencyBound::new();
        fresh.frame(at(base, 33), at(base, 53), false, true);
        fresh
    });
}

#[test]
fn an_undrawn_move_is_bounded_at_the_next_drawn_frame_from_its_own_poll() {
    let base = Instant::now();
    let mut bound = MoveLatencyBound::new();
    assert_eq!(bound.frame(at(base, 0), at(base, 20), false, true), None);
    // Moved but not drawn (an occluded window): nothing yet.
    assert_eq!(bound.frame(at(base, 33), at(base, 40), true, false), None);
    // A later move does not restart the bound: the earliest move's counts.
    assert_eq!(
        bound.frame(at(base, 66), at(base, 90), true, true),
        Some(ms(90))
    );
    // A frame drawn after an undrawn move draws that move, moved or not.
    assert_eq!(bound.frame(at(base, 99), at(base, 105), true, false), None);
    assert_eq!(
        bound.frame(at(base, 132), at(base, 150), false, true),
        Some(ms(150 - 66))
    );
    assert_eq!(bound.frame(at(base, 165), at(base, 180), false, true), None);
}

#[test]
fn the_bound_is_never_below_a_moves_true_latency() {
    // Frames of varying work and idle; a move arrives at a known moment
    // between two polls, is drained by the later poll, and is drawn by its
    // frame unless that frame is occluded. Its true latency, arrival to the
    // end of the drawing frame, is computed from the arrival alone.
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let mut next = move |modulus: u64| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state % modulus
    };
    let base = Instant::now();
    let mut bound = MoveLatencyBound::new();
    let mut now_us = 0_u64;
    let mut previous_poll_us: Option<u64> = None;
    let mut arrival_us: Option<u64> = None;
    let (mut bounded, mut tightest) = (0, Duration::MAX);
    for _ in 0..5_000 {
        let poll_us = now_us;
        let end_us = poll_us + 1_000 + next(60_000);
        // A move arrives between the previous poll and this one, so this
        // poll drains it (none before the first poll).
        let moved = previous_poll_us.is_some() && next(3) == 0;
        if moved && arrival_us.is_none() {
            let from = previous_poll_us.unwrap_or(0);
            arrival_us = Some(from + next(poll_us - from + 1));
        }
        let drawn = next(10) != 0;
        let got = bound.frame(
            base + Duration::from_micros(poll_us),
            base + Duration::from_micros(end_us),
            moved,
            drawn,
        );
        if drawn {
            let truth = arrival_us
                .take()
                .map(|arrived| Duration::from_micros(end_us - arrived));
            assert_eq!(
                got.is_some(),
                truth.is_some(),
                "a bound exactly when a move is drawn"
            );
            if let (Some(got), Some(truth)) = (got, truth) {
                assert!(got >= truth, "bound {got:?} below the true {truth:?}");
                bounded += 1;
                tightest = tightest.min(got - truth);
            }
        } else {
            assert_eq!(got, None);
        }
        previous_poll_us = Some(poll_us);
        now_us = end_us + next(40_000);
    }
    assert!(bounded > 1_000, "{bounded} moves bounded");
    assert!(
        tightest < Duration::from_millis(1),
        "the bound is reached: {tightest:?}"
    );
}
