//! The world's footprint (FL-043, plan §19.4): what a founded world holds
//! is what [`World::footprint`] charges for its fields and scratch, at most
//! what it charges for its state, and stepping never grows either, at any
//! worker count. The run-level preflight and the host heap measurement are
//! in `flife-cli`'s `memory_admission` test.

use std::num::NonZeroUsize;

use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Arrangement, Footprint, NumericalProfile, Stepped, World};
use flife_field::settle::{
    ChemistryConstants, ChemistryOrganism, ChemistryOutcome, ChemistryPacket, EmissionOrganism,
    EmissionOutcome, MotionConstants, MotionOrganism, MotionOutcome, MutationBirth,
    MutationConstants, MutationCounts, MutationValues, ProposalConstants, ProposalOrganism,
    ProposalOutcome, SettleBackend,
};
use flife_field::{FieldGrid, FieldState};
use flife_neural::{
    ControllerBackend, ControllerJob, ControllerScratch, FlatControllerJob, FlatOutputs, RawOutputs,
};

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

#[derive(Debug)]
struct ExactFlatF32Backend;

impl ControllerBackend for ExactFlatF32Backend {
    fn describe(&self) -> String {
        "exact-flat-f32-reference".to_owned()
    }

    fn resident_bytes(&self) -> u64 {
        0
    }

    fn forget(&mut self) {}

    fn evaluate(
        &mut self,
        jobs: &[ControllerJob<'_>],
        outputs: &mut Vec<Option<RawOutputs>>,
    ) -> Result<(), CoreError> {
        outputs.clear();
        outputs.resize(jobs.len(), None);
        Ok(())
    }

    fn supports_flat_f32(&self) -> bool {
        true
    }

    fn evaluate_flat_f32(
        &mut self,
        jobs: &[FlatControllerJob<'_>],
        input_width: usize,
        inputs_f32: &[f32],
        inputs_finite: &[u8],
        outputs: &mut FlatOutputs,
    ) -> Result<(), CoreError> {
        let out_width = jobs
            .first()
            .map_or(0, |j| j.controller.topology().output_width());
        outputs.output_width = out_width;
        outputs.values.clear();
        outputs.values.resize(jobs.len() * out_width, 0.0_f32);
        outputs.valid.clear();
        outputs.valid.resize(jobs.len(), 0);
        for (idx, job) in jobs.iter().enumerate() {
            if inputs_finite.get(idx).copied().unwrap_or(0) == 0 {
                continue;
            }
            let in_row = &inputs_f32[idx * input_width..(idx + 1) * input_width];
            let wide: Vec<f64> = in_row.iter().map(|&v| f64::from(v)).collect();
            let mut scratch = ControllerScratch::new(job.controller.topology())?;
            let raw = job.controller.evaluate_raw(&wide, &mut scratch)?;
            outputs.values[idx * out_width..(idx + 1) * out_width].copy_from_slice(raw.as_slice());
            outputs.valid[idx] = 1;
        }
        Ok(())
    }
}

#[test]
fn flat_f32_multirate_streaming_sample_and_dedup_compositions_match_serial_bit_for_bit() {
    // Exercises:
    // - `PyramidView::sample_batch_each` (> 512 organisms per tick across 4 workers),
    // - direct-to-`in_f32` encoding for due organisms,
    // - `observe_gradients_only` for the 75% non-due organisms at `k = 4`,
    // - deduplicated per-cell `compositions` + zero-copy `Chemistry::settle_indexed`.
    let toml = PRESET
        .replace("initial_count = 4096", "initial_count = 1536")
        .replace("maximum_count = 16384", "maximum_count = 2048")
        .replace(
            "evaluation_interval_ticks = 1",
            "evaluation_interval_ticks = 4",
        )
        .replace("hidden_widths = [64, 32]", "hidden_widths = [8]");
    let cfg = ValidatedConfig::from_toml(&toml).expect("valid multirate config");

    let mut serial = found(&cfg);
    let mut batched = found(&cfg);
    batched
        .set_cpu_workers(NonZeroUsize::new(4).expect("4"))
        .expect("4 workers");
    batched.set_controller_backend(Some(Box::new(ExactFlatF32Backend)));

    let founded = batched.allocated();
    let bound = batched.footprint();
    assert_eq!(founded.scratch, bound.scratch);

    for tick in 1..=8 {
        assert!(matches!(
            serial.step().expect("serial"),
            Stepped::Published(_)
        ));
        assert!(matches!(
            batched.step().expect("batched"),
            Stepped::Published(_)
        ));
        assert_eq!(
            batched.allocated().scratch,
            founded.scratch,
            "batched multirate tick {tick} grew scratch"
        );
        assert_eq!(
            batched.digest(),
            serial.digest(),
            "batched flat-F32 multirate diverged from serial at tick {tick}"
        );
    }
    assert!(
        batched.controller_report().batched > 0 && batched.controller_report().deferred == 0,
        "flat-F32 path must have executed batched evaluations: {:?}",
        batched.controller_report()
    );
}

#[derive(Debug)]
struct AlwaysFailingSettleBackend;

impl SettleBackend for AlwaysFailingSettleBackend {
    fn describe(&self) -> String {
        "always-failing-settle".to_owned()
    }

    fn chemistry(
        &mut self,
        _observed: &FieldState,
        _constants: &ChemistryConstants,
        _organisms: &[ChemistryOrganism],
        _packets: &[ChemistryPacket],
        _out: &mut Vec<ChemistryOutcome>,
    ) -> Result<(), CoreError> {
        Err(CoreError::MissingCapability {
            capability: "simulated device drop".into(),
        })
    }

    fn motion(
        &mut self,
        _constants: &MotionConstants,
        _organisms: &[MotionOrganism],
        _out: &mut Vec<MotionOutcome>,
    ) -> Result<(), CoreError> {
        Err(CoreError::MissingCapability {
            capability: "simulated device drop".into(),
        })
    }

    fn emission(
        &mut self,
        _grid: &FieldGrid,
        _efficiency: f64,
        _organisms: &[EmissionOrganism],
        _out: &mut Vec<EmissionOutcome>,
    ) -> Result<(), CoreError> {
        Err(CoreError::MissingCapability {
            capability: "simulated device drop".into(),
        })
    }

    fn proposal(
        &mut self,
        _constants: &ProposalConstants,
        _organisms: &[ProposalOrganism],
        _out: &mut Vec<ProposalOutcome>,
    ) -> Result<(), CoreError> {
        Err(CoreError::MissingCapability {
            capability: "simulated device drop".into(),
        })
    }

    fn mutation(
        &mut self,
        _constants: &MutationConstants,
        _births: &[MutationBirth],
        _values: MutationValues<'_>,
        _out: &mut Vec<MutationCounts>,
    ) -> Result<(), CoreError> {
        Err(CoreError::MissingCapability {
            capability: "simulated device drop".into(),
        })
    }
}

#[test]
fn failing_settle_backend_drop_followed_by_controller_backend_keeps_fixed_scratch() {
    // Regression test: on a large multi-worker world (`capacity >= 24,576`),
    // installing a `SettleBackend` empties `settle_tiles` and fits `lanes`
    // settle buffers. If that `SettleBackend` subsequently fails and drops
    // itself, calling `set_controller_backend` must NOT wipe `lanes` settle
    // buffers while `settle_tiles` is still empty.
    let toml = PRESET
        .replace("initial_count = 4096", "initial_count = 1024")
        .replace("maximum_count = 16384", "maximum_count = 24576")
        .replace(
            "evaluation_interval_ticks = 1",
            "evaluation_interval_ticks = 4",
        )
        .replace("hidden_widths = [64, 32]", "hidden_widths = [8]");
    let cfg = ValidatedConfig::from_toml(&toml).expect("valid 24k config");

    let mut serial = found(&cfg);
    let mut world = found(&cfg);
    world
        .set_cpu_workers(NonZeroUsize::new(4).expect("4"))
        .expect("4 workers");
    world.set_settle_backend(Some(Box::new(AlwaysFailingSettleBackend)));

    // Tick 1 triggers the failing settle backend, which drops itself and falls
    // back to the CPU lane buffers.
    assert!(matches!(
        serial.step().expect("serial 1"),
        Stepped::Published(_)
    ));
    assert!(matches!(
        world.step().expect("world 1"),
        Stepped::Published(_)
    ));
    assert!(
        world.settle_report().failure.is_some(),
        "settle backend should have recorded failure and dropped"
    );
    assert_eq!(world.digest(), serial.digest(), "tick 1 bit-exact");

    // Installing a flat-F32 controller backend afterwards must keep lane settle
    // buffers intact so subsequent ticks hold an invariant scratch footprint.
    world.set_controller_backend(Some(Box::new(ExactFlatF32Backend)));
    let after_ctrl = world.allocated();
    assert_eq!(after_ctrl.scratch, world.footprint().scratch);

    for tick in 2..=5 {
        assert!(matches!(
            serial.step().expect("serial"),
            Stepped::Published(_)
        ));
        assert!(matches!(
            world.step().expect("world"),
            Stepped::Published(_)
        ));
        assert_eq!(
            world.allocated().scratch,
            after_ctrl.scratch,
            "scratch grew at tick {tick} after settle backend drop + set_controller_backend"
        );
        assert_eq!(
            world.digest(),
            serial.digest(),
            "digest diverged at tick {tick}"
        );
    }
}
