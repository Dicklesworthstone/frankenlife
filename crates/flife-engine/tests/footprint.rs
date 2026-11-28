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

/// Large-world spatial Pass 1/Pass 2 (`rows >= 8,192`, `cells >= 32,768`),
/// band-grouped 4-plane moment deposition, `Population::ordered_slots` +
/// parallel `check_whole_tick_on` + parallel `apply_checked_tick_on`, and
/// Phase-8 `survivor_energies` must match 1-worker serial bit-for-bit across
/// both `FreeList` and `Compact`, with zero scratch footprint drift and clean
/// fallback on a Phase-1 `RemoveOrganisms` intervention.
#[test]
fn spatial_pass1_and_parallel_population_publish_match_serial_bit_for_bit() {
    use flife_engine::{Disposition, Intervention};

    let toml = PRESET
        .replace("grid_width = 128", "grid_width = 256")
        .replace("grid_height = 128", "grid_height = 256")
        .replace("width = 128.0", "width = 256.0")
        .replace("height = 128.0", "height = 256.0")
        .replace("initial_count = 4096", "initial_count = 8192")
        .replace("maximum_count = 16384", "maximum_count = 9216")
        .replace(
            "evaluation_interval_ticks = 1",
            "evaluation_interval_ticks = 2",
        )
        .replace("hidden_widths = [64, 32]", "hidden_widths = [8]");
    let cfg = ValidatedConfig::from_toml(&toml).expect("valid 8k 256x256 config");

    for arrangement in [Arrangement::FreeList, Arrangement::Compact] {
        let mut serial = World::found(
            &cfg,
            arrangement,
            NumericalProfile::reference_f32().expect("profile"),
        )
        .expect("serial");
        let mut parallel = World::found(
            &cfg,
            arrangement,
            NumericalProfile::reference_f32().expect("profile"),
        )
        .expect("parallel");
        parallel
            .set_cpu_workers(NonZeroUsize::new(4).expect("4"))
            .expect("4 workers");
        parallel.set_controller_backend(Some(Box::new(ExactFlatF32Backend)));

        let founded = parallel.allocated();
        let bound = parallel.footprint();
        assert_eq!(founded.field, bound.field, "{arrangement:?} field bound");
        assert_eq!(
            founded.scratch, bound.scratch,
            "{arrangement:?} scratch bound"
        );

        let mut removed_on_tick3 = 0_usize;
        for tick in 1..=5 {
            if tick == 3 {
                // Remove organisms inside a disk in Phase 1 to verify:
                // 1) spatial Pass 1 cleanly falls back when `rows.len() != population.len()`,
                // 2) FreeList holes + `ordered_slots` retain + column-parallel publish work
                //    bit-identically on tick 3 and subsequent fragmented ticks 4..=5!
                let removal = Intervention::RemoveOrganisms {
                    center: [64.0, 64.0],
                    radius: 16.0,
                    disposition: Disposition::Recycle,
                };
                assert!(matches!(
                    serial
                        .step_with(&[removal])
                        .expect("serial step with removal"),
                    Stepped::Published(_)
                ));
                let Stepped::Published(report) = parallel
                    .step_with(&[removal])
                    .expect("parallel step with removal")
                else {
                    panic!("expected published tick 3");
                };
                removed_on_tick3 = report.deaths.len();
            } else if tick == 4 {
                // Re-introduce organisms so FreeList reuses freed slots while
                // `slots_dense_ascending` is `false` and `ordered_slots` stays
                // strictly ascending in OrganismId order.
                let intro = Intervention::IntroduceOrganisms {
                    cell: [64, 64],
                    count: 32,
                    energy: 1.0,
                };
                assert!(matches!(
                    serial.step_with(&[intro]).expect("serial step with intro"),
                    Stepped::Published(_)
                ));
                assert!(matches!(
                    parallel
                        .step_with(&[intro])
                        .expect("parallel step with intro"),
                    Stepped::Published(_)
                ));
            } else {
                assert!(matches!(
                    serial.step().expect("serial step"),
                    Stepped::Published(_)
                ));
                assert!(matches!(
                    parallel.step().expect("parallel step"),
                    Stepped::Published(_)
                ));
            }
            let now = parallel.allocated();
            assert_eq!(
                now.field, founded.field,
                "{arrangement:?} field grew at tick {tick}"
            );
            assert_eq!(
                now.scratch, founded.scratch,
                "{arrangement:?} scratch grew at tick {tick}"
            );
            assert!(
                now.state <= bound.state,
                "{arrangement:?} state exceeded bound at tick {tick}"
            );
            assert_eq!(
                parallel.digest(),
                serial.digest(),
                "{arrangement:?} spatial Pass1 / parallel publish diverged at tick {tick}"
            );
        }
        assert!(
            removed_on_tick3 > 0,
            "{arrangement:?} expected Phase-1 removal to exercise holes"
        );
        assert!(
            parallel.controller_report().batched > 0 && parallel.controller_report().deferred == 0,
            "{arrangement:?} flat-F32 spatial path did not batch: {:?}",
            parallel.controller_report()
        );
    }
}

/// CPU-reference worlds at `>= 8,192` organisms and `>= 32,768` cells use the
/// 8×8 spatial macro-tile reference path (`decide_all_spatial`) and fused L2
/// Phase-6 chemistry (`spatial_chemistry_prepass_fused`) with zero extra heap
/// allocation and bit-for-bit identical digests to serial `w = 1` across both
/// `Compact` and `FreeList` slot arrangements (including mid-run removals and
/// re-introductions).
#[test]
fn cpu_reference_spatial_decide_and_fused_chemistry_match_serial_and_hold_footprint() {
    use flife_engine::{Disposition, Intervention};

    let toml = PRESET
        .replace("grid_width = 128", "grid_width = 256")
        .replace("grid_height = 128", "grid_height = 256")
        .replace("width = 128.0", "width = 256.0")
        .replace("height = 128.0", "height = 256.0")
        .replace("initial_count = 4096", "initial_count = 8192")
        .replace("maximum_count = 16384", "maximum_count = 9216")
        .replace(
            "evaluation_interval_ticks = 1",
            "evaluation_interval_ticks = 2",
        )
        .replace("hidden_widths = [64, 32]", "hidden_widths = [8]");
    let cfg = ValidatedConfig::from_toml(&toml).expect("spatial cpu ref config");

    for arrangement in [Arrangement::Compact, Arrangement::FreeList] {
        let mut serial = World::found(
            &cfg,
            arrangement,
            NumericalProfile::reference_f32().expect("profile"),
        )
        .expect("serial world");
        let mut parallel = World::found(
            &cfg,
            arrangement,
            NumericalProfile::reference_f32().expect("profile"),
        )
        .expect("parallel world");
        parallel
            .set_cpu_workers(NonZeroUsize::new(4).expect("4"))
            .expect("4 workers");

        let bound = parallel.footprint();
        let founded = parallel.allocated();
        assert_eq!(founded.field, bound.field, "{arrangement:?} field bound");
        assert_eq!(
            founded.scratch, bound.scratch,
            "{arrangement:?} scratch bound"
        );

        let mut removed_on_tick3 = 0_usize;
        for tick in 1..=5 {
            if tick == 3 {
                let removal = Intervention::RemoveOrganisms {
                    center: [64.0, 64.0],
                    radius: 16.0,
                    disposition: Disposition::Recycle,
                };
                assert!(matches!(
                    serial.step_with(&[removal]).expect("serial rem"),
                    Stepped::Published(_)
                ));
                let Stepped::Published(report) =
                    parallel.step_with(&[removal]).expect("parallel rem")
                else {
                    panic!("expected published tick 3");
                };
                removed_on_tick3 = report.deaths.len();
            } else if tick == 4 {
                let intro = Intervention::IntroduceOrganisms {
                    cell: [64, 64],
                    count: 32,
                    energy: 1.0,
                };
                assert!(matches!(
                    serial.step_with(&[intro]).expect("serial intro"),
                    Stepped::Published(_)
                ));
                assert!(matches!(
                    parallel.step_with(&[intro]).expect("parallel intro"),
                    Stepped::Published(_)
                ));
            } else {
                assert!(matches!(
                    serial.step().expect("serial step"),
                    Stepped::Published(_)
                ));
                assert!(matches!(
                    parallel.step().expect("parallel step"),
                    Stepped::Published(_)
                ));
            }
            let now = parallel.allocated();
            assert_eq!(
                now.field, founded.field,
                "{arrangement:?} tick {tick} field grew"
            );
            assert_eq!(
                now.scratch, founded.scratch,
                "{arrangement:?} tick {tick} scratch grew"
            );
            assert!(
                now.state <= bound.state,
                "{arrangement:?} tick {tick} state exceeded bound"
            );
            assert_eq!(
                parallel.digest(),
                serial.digest(),
                "{arrangement:?} tick {tick}: spatial CPU ref diverged from serial"
            );
        }
        assert!(
            removed_on_tick3 > 0,
            "{arrangement:?} expected Phase-1 removal to exercise holes"
        );
        assert_eq!(parallel.controller_report().batched, 0);
    }
}

/// A `1024 × 1024` field (Screenshot-scale grid geometry) with `>= 8,192`
/// organisms uses `16 × 16` spatial macro-tiles (`64 × 64` cells, `66 × 66 =
/// 4,356 <= 4,608` halo cells) so fused L2 Phase-6 chemistry executes on
/// `1024²` with zero heap growth and bit-for-bit identical digests to serial
/// `w = 1`.
#[test]
fn screenshot_grid_1024_spatial_fused_chemistry_matches_serial_and_holds_footprint() {
    let toml = PRESET
        .replace("grid_width = 128", "grid_width = 1024")
        .replace("grid_height = 128", "grid_height = 1024")
        .replace("width = 128.0", "width = 1024.0")
        .replace("height = 128.0", "height = 1024.0")
        .replace("initial_count = 4096", "initial_count = 8192")
        .replace("maximum_count = 16384", "maximum_count = 9216")
        .replace("hidden_widths = [64, 32]", "hidden_widths = [8]")
        .replace(
            "maximum_total_bytes = 2147483648",
            "maximum_total_bytes = 4294967296",
        );
    let cfg = ValidatedConfig::from_toml(&toml).expect("1024^2 spatial config");

    let mut serial = World::found(
        &cfg,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("serial 1024^2 world");
    let mut parallel = World::found(
        &cfg,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("parallel 1024^2 world");
    parallel
        .set_cpu_workers(NonZeroUsize::new(4).expect("4"))
        .expect("4 workers");

    let founded = parallel.allocated();
    let bound = parallel.footprint();
    assert_eq!(founded.field, bound.field);
    assert_eq!(founded.scratch, bound.scratch);

    for tick in 1..=2 {
        assert!(matches!(
            serial.step().expect("serial step"),
            Stepped::Published(_)
        ));
        assert!(matches!(
            parallel.step().expect("parallel step"),
            Stepped::Published(_)
        ));
        let now = parallel.allocated();
        assert_eq!(now.field, founded.field, "tick {tick} field grew");
        assert_eq!(now.scratch, founded.scratch, "tick {tick} scratch grew");
        assert_eq!(
            parallel.digest(),
            serial.digest(),
            "tick {tick}: 1024x1024 16x16-tile fused chemistry diverged from serial"
        );
    }
}
