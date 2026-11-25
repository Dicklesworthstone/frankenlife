//! The world's footprint (FL-043, plan §19.4): what a founded world holds
//! is what [`World::footprint`] charges for its fields and scratch, at most
//! what it charges for its state, and stepping never grows either, at any
//! worker count. The run-level preflight and the host heap measurement are
//! in `flife-cli`'s `memory_admission` test.

use std::num::NonZeroUsize;

use flife_core::ValidatedConfig;
use flife_engine::{Arrangement, Footprint, NumericalProfile, Stepped, World};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// The real preset with 512 founders, so births and deaths happen within a
/// few hundred ticks.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure is a test failure"
)]
fn config() -> ValidatedConfig {
    let toml = PRESET.replace("initial_count = 4096", "initial_count = 512");
    assert_ne!(toml, PRESET, "the preset declares its founders");
    ValidatedConfig::from_toml(&toml).expect("the preset validates")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure is a test failure"
)]
fn found(config: &ValidatedConfig) -> World {
    World::found(
        config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("the world founds")
}

/// Step `ticks` times, checking after each that the fields and scratch hold
/// exactly `fixed`'s and the state no more than `bound`'s; returns the
/// births and deaths seen.
#[expect(clippy::expect_used, clippy::panic, reason = "test helper")]
fn step_within(
    world: &mut World,
    ticks: u64,
    fixed: &Footprint,
    bound: &Footprint,
) -> (usize, usize) {
    let mut births = 0;
    let mut deaths = 0;
    for _ in 0..ticks {
        match world.step().expect("the world steps") {
            Stepped::Published(report) => {
                births += report.births.len();
                deaths += report.deaths.len();
            }
            other => panic!("unexpected outcome {other:?}"),
        }
        let now = world.allocated();
        let tick = world.tick().get();
        assert_eq!(now.field, fixed.field, "the fields grew at tick {tick}");
        assert_eq!(
            now.scratch, fixed.scratch,
            "the scratch grew at tick {tick}"
        );
        assert!(
            now.state <= bound.state,
            "the state holds {} > {} at tick {tick}",
            now.state,
            bound.state
        );
        assert_eq!(now.staging, 0, "no staging outlives a step");
    }
    (births, deaths)
}

#[test]
fn a_founded_world_holds_its_footprint_and_stepping_never_grows_it() {
    let mut world = found(&config());
    let founded = world.allocated();
    let footprint = world.footprint();
    println!("founded {founded:?}\nfootprint {footprint:?}");
    // The fields and the tick's scratch are reserved in full at founding.
    assert_eq!(founded.field, footprint.field);
    assert_eq!(founded.scratch, footprint.scratch);
    assert!(
        founded.state < footprint.state,
        "512 of 16384 hold less than the bound"
    );
    assert!(footprint.staging > 0);
    assert_eq!(
        footprint.total(),
        footprint.state + footprint.field + footprint.scratch + footprint.staging
    );
    let (births, deaths) = step_within(&mut world, 300, &founded, &footprint);
    assert!(
        births > 0 && deaths > 0,
        "the run must exercise births ({births}) and deaths ({deaths})"
    );
    // The bound does not move as the population does.
    assert_eq!(world.footprint(), footprint);
}

#[test]
fn changing_workers_keeps_the_footprint_reserved() {
    let mut world = found(&config());
    let one = world.footprint();
    let four = NonZeroUsize::new(4).expect("four");
    world.set_cpu_workers(four).expect("four workers");
    let wide = world.allocated();
    assert_eq!(wide.scratch, world.footprint().scratch);
    assert!(
        wide.scratch > one.scratch,
        "three more lanes hold more scratch"
    );
    assert!(wide.field > one.field, "three more transport scratches");
    let bound = world.footprint();
    let (births, _) = step_within(&mut world, 300, &wide, &bound);
    assert!(births > 0, "the run must exercise births at four workers");
    // Back to one worker, the lanes' scratch is released to one lane's.
    world
        .set_cpu_workers(NonZeroUsize::MIN)
        .expect("one worker");
    assert_eq!(world.allocated().scratch, one.scratch);
    let narrow = world.allocated();
    let bound = world.footprint();
    step_within(&mut world, 30, &narrow, &bound);
}

#[test]
fn large_world_settle_tiles_hold_exact_footprint_and_match_serial_bit_for_bit() {
    // 24,576 organisms hits `SETTLE_TILE_THRESHOLD` so the reusable work-stealing
    // `Scratch::settle_tiles` path runs and must neither allocate in steady state
    // nor diverge by a single bit from 1-worker serial execution.
    let toml = PRESET
        .replace("initial_count = 4096", "initial_count = 24576")
        .replace("maximum_count = 16384", "maximum_count = 24576")
        .replace("hidden_widths = [64, 32]", "hidden_widths = [8]");
    let cfg = ValidatedConfig::from_toml(&toml).expect("valid 24k config");

    let mut serial = found(&cfg);
    let mut parallel = found(&cfg);
    parallel
        .set_cpu_workers(NonZeroUsize::new(4).expect("4"))
        .expect("4 workers");
    let founded = parallel.allocated();
    let bound = parallel.footprint();
    assert_eq!(founded.field, bound.field);
    assert_eq!(founded.scratch, bound.scratch);

    for tick in 1..=3 {
        assert!(matches!(
            serial.step().expect("serial"),
            Stepped::Published(_)
        ));
        assert!(matches!(
            parallel.step().expect("parallel"),
            Stepped::Published(_)
        ));
        let now = parallel.allocated();
        assert_eq!(
            now.scratch, founded.scratch,
            "large-world settle tiles grew scratch at tick {tick}"
        );
        assert_eq!(
            parallel.digest(),
            serial.digest(),
            "4-worker reusable settle tiles diverged from serial at tick {tick}"
        );
    }

    // Scaling back down to 1 worker releases `settle_tiles` back to exact serial footprint.
    parallel
        .set_cpu_workers(NonZeroUsize::MIN)
        .expect("1 worker");
    assert_eq!(parallel.allocated().scratch, serial.footprint().scratch);
}
