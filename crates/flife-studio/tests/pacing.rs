//! FL-020 evidence (plan §13.5): the pacer that shows the display one
//! committed state late. It moves the display over the smoothed interval
//! at which epochs arrive; a wait of `MOTION_MAX` or more (a pause, a step)
//! starts it again at `MOTION_MAX`; and it is at rest before it knows an
//! interval.

use std::time::{Duration, Instant};

use flife_studio::{ARRIVAL_SMOOTHING, MOTION_MAX, Pacer};

fn ms(milliseconds: u64) -> Duration {
    Duration::from_millis(milliseconds)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-12
}

fn seconds(duration: Option<Duration>) -> f64 {
    duration.map_or(f64::NAN, |duration| duration.as_secs_f64())
}

#[test]
fn the_display_is_at_rest_until_an_interval_is_known() {
    let start = Instant::now();
    let mut pacer = Pacer::new();
    assert_eq!(pacer.fraction(start), 1.0);
    assert_eq!(pacer.interval(), None);
    pacer.arrive(start);
    assert_eq!(pacer.fraction(start), 1.0, "one arrival is no interval");
    assert_eq!(pacer.interval(), None);
    assert_eq!(Pacer::default(), Pacer::new());
}

#[test]
fn the_display_moves_over_the_interval_between_arrivals() {
    let start = Instant::now();
    let mut pacer = Pacer::new();
    pacer.arrive(start);
    let second = start + ms(40);
    pacer.arrive(second);
    assert!(close(seconds(pacer.interval()), 0.040));
    for (after, fraction) in [
        (0, 0.0),
        (10, 0.25),
        (20, 0.5),
        (30, 0.75),
        (40, 1.0),
        (400, 1.0),
    ] {
        let got = pacer.fraction(second + ms(after));
        assert!(close(got, fraction), "{after} ms: {got} for {fraction}");
    }
    // A frame timed before the arrival is at the arrival.
    assert_eq!(pacer.fraction(start), 0.0);

    // Each later interval moves the smoothed one by a quarter of the
    // difference.
    let third = second + ms(80);
    pacer.arrive(third);
    let smoothed = 0.040 + ARRIVAL_SMOOTHING * (0.080 - 0.040);
    assert!(close(seconds(pacer.interval()), smoothed));
    assert!(close(pacer.fraction(third + ms(25)), 0.025 / smoothed));
}

#[test]
fn a_long_wait_counts_as_motion_max() {
    let start = Instant::now();
    let mut pacer = Pacer::new();
    pacer.arrive(start);
    // A step five seconds into a pause moves over MOTION_MAX, not five
    // seconds.
    let step = start + Duration::from_secs(5);
    pacer.arrive(step);
    assert_eq!(pacer.interval(), Some(MOTION_MAX));
    assert_eq!(MOTION_MAX, ms(100));
    assert_eq!(pacer.fraction(step + MOTION_MAX), 1.0);
    assert!(close(pacer.fraction(step + ms(50)), 0.5));
    // After running at 10 ms, a pause and a step: the stream of epochs had
    // stopped, so the step moves over the whole of MOTION_MAX, and the
    // interval smooths down again once epochs flow.
    let mut pacer = Pacer::new();
    let mut at = start;
    for _ in 0..64 {
        pacer.arrive(at);
        at += ms(10);
    }
    assert!(close(seconds(pacer.interval()), 0.010));
    let step = at + Duration::from_secs(5);
    pacer.arrive(step);
    assert_eq!(pacer.interval(), Some(MOTION_MAX));
    pacer.arrive(step + ms(99));
    assert!(close(
        seconds(pacer.interval()),
        0.100 + ARRIVAL_SMOOTHING * (0.099 - 0.100)
    ));
    pacer.arrive(step + ms(99) + MOTION_MAX);
    assert_eq!(
        pacer.interval(),
        Some(MOTION_MAX),
        "exactly MOTION_MAX restarts too"
    );
}

#[test]
fn a_step_alone_moves_over_motion_max_whatever_came_before() {
    let start = Instant::now();
    let mut pacer = Pacer::new();
    let mut at = start;
    for _ in 0..16 {
        pacer.arrive(at);
        at += ms(4);
    }
    assert!(close(seconds(pacer.interval()), 0.004));
    // Paused for only 20 ms, then a step: not a stream, so MOTION_MAX.
    let step = at + ms(20);
    pacer.arrive_alone(step);
    assert_eq!(pacer.interval(), Some(MOTION_MAX));
    assert_eq!(pacer.fraction(step), 0.0);
    assert!(close(pacer.fraction(step + ms(25)), 0.25));
    assert_eq!(pacer.fraction(step + MOTION_MAX), 1.0);
    // Running again, the interval smooths down from there.
    pacer.arrive(step + ms(30));
    assert!(close(
        seconds(pacer.interval()),
        0.100 + ARRIVAL_SMOOTHING * (0.030 - 0.100)
    ));
}

#[test]
fn arrivals_with_no_time_between_leave_the_display_at_rest() {
    let start = Instant::now();
    let mut pacer = Pacer::new();
    pacer.arrive(start + ms(5));
    // An arrival stamped earlier than the last counts as no time.
    pacer.arrive(start);
    assert_eq!(pacer.interval(), Some(Duration::ZERO));
    assert_eq!(pacer.fraction(start), 1.0);
    assert_eq!(pacer.fraction(start + ms(1)), 1.0);
}
