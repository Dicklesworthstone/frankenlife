//! The controller backend port (FL-030): a world whose due evaluations go
//! through a batch backend is the reference world, digest for digest, at
//! every tick, whatever the backend defers, and whether or not it fails.
//! These run on every host with a CPU batch backend; the Metal backend's own
//! world test is in `flife-backend-metal`.

use std::num::NonZeroUsize;

use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_neural::{
    ControllerBackend, ControllerJob, ControllerScratch, FlatControllerJob, FlatOutputs, RawOutputs,
};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// The real preset with 512 founders, so the founder crash, births and
/// mutation all happen within a few hundred ticks.
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

/// The reference controller as a batch backend, deferring every
/// `defer_every`-th job; optionally flipping one output bit of the first
/// job of call `flip_at`, or failing call `fail_at`.
#[derive(Debug, Default)]
struct ReferenceBatch {
    defer_every: usize,
    flip_at: Option<u64>,
    fail_at: Option<u64>,
    calls: u64,
    scratch: ControllerScratch,
}

impl ControllerBackend for ReferenceBatch {
    fn describe(&self) -> String {
        "reference batch (test)".to_owned()
    }

    fn resident_bytes(&self) -> u64 {
        self.scratch.allocated_bytes()
    }

    fn forget(&mut self) {}

    fn evaluate(
        &mut self,
        jobs: &[ControllerJob<'_>],
        outputs: &mut Vec<Option<RawOutputs>>,
    ) -> Result<(), CoreError> {
        self.calls += 1;
        if self.fail_at == Some(self.calls) {
            return Err(CoreError::StateConflict {
                operation: "test backend",
                detail: "a planted device failure".to_owned(),
            });
        }
        outputs.clear();
        for (index, job) in jobs.iter().enumerate() {
            if index % self.defer_every == 0 {
                outputs.push(None);
                continue;
            }
            match job.controller.evaluate_raw(job.inputs, &mut self.scratch) {
                Ok(mut raw) => {
                    if index == 1 && self.flip_at == Some(self.calls) {
                        raw[0] = f32::from_bits(raw[0].to_bits() ^ 1);
                    }
                    outputs.push(Some(raw));
                }
                Err(_) => outputs.push(None),
            }
        }
        Ok(())
    }
}

#[test]
fn installing_a_backend_reserves_exactly_its_batched_bytes() {
    let mut world = found(&config());
    let before = world.allocated();
    let batched = world.batched_bytes();
    assert!(batched > 0);
    world.set_controller_backend(Some(Box::new(ReferenceBatch {
        defer_every: 3,
        ..ReferenceBatch::default()
    })));
    assert_eq!(world.controller_report().failure, None);
    let installed = world.allocated();
    assert_eq!(installed.scratch, before.scratch + batched);
    assert_eq!(
        (installed.state, installed.field),
        (before.state, before.field)
    );
    for _ in 0..60 {
        assert!(matches!(world.step(), Ok(Stepped::Published(_))));
        assert_eq!(
            world.allocated().scratch,
            installed.scratch,
            "the batched path grew"
        );
    }
    world.set_controller_backend(None);
    assert_eq!(world.allocated().scratch, before.scratch);
}

/// Step both worlds `ticks` times, returning the first tick whose digests
/// differ, the evaluations the batched world reported, and its births.
#[expect(clippy::expect_used, clippy::panic, reason = "test helper")]
fn race(reference: &mut World, batched: &mut World, ticks: u64) -> (Option<u64>, u64, usize) {
    let mut first_difference = None;
    let mut evaluated = 0_u64;
    let mut births = 0;
    for _ in 0..ticks {
        let expected = reference.step().expect("the reference steps");
        let got = batched.step().expect("the batched world steps");
        match (expected, got) {
            (Stepped::Published(_), Stepped::Published(report)) => {
                evaluated += report.evaluated as u64;
                births += report.births.len();
            }
            (expected, got) => panic!("unexpected outcomes {expected:?} and {got:?}"),
        }
        if first_difference.is_none() && reference.digest() != batched.digest() {
            first_difference = Some(batched.tick().get());
        }
    }
    (first_difference, evaluated, births)
}

#[test]
fn a_batched_world_is_the_reference_world_at_every_tick() {
    let config = config();
    let mut reference = found(&config);
    let mut batched = found(&config);
    batched.set_controller_backend(Some(Box::new(ReferenceBatch {
        defer_every: 7,
        ..ReferenceBatch::default()
    })));
    let (difference, evaluated, births) = race(&mut reference, &mut batched, 300);
    assert_eq!(difference, None, "the worlds diverged");
    assert!(births > 0, "the run must exercise newborn genomes");
    let report = batched.controller_report();
    assert_eq!(report.backend.as_deref(), Some("reference batch (test)"));
    assert_eq!(report.failure, None);
    assert_eq!(report.reference_ticks, 0);
    // Every evaluation went through the backend, one in seven deferred.
    assert_eq!(report.batched + report.deferred, evaluated);
    println!("evaluated {evaluated}: {report:?}");
    // Each tick defers ⌈n/7⌉ of its n evaluations.
    assert!(report.deferred > 0 && report.batched > 4 * report.deferred);
    assert_eq!(reference.controller_report().backend, None);
}

/// The planted negative: one output bit of one organism, once, must show
/// in the world digest at that very tick, or the test above proves nothing.
#[test]
fn one_flipped_output_bit_changes_the_world() {
    let config = config();
    let mut reference = found(&config);
    let mut batched = found(&config);
    batched.set_controller_backend(Some(Box::new(ReferenceBatch {
        defer_every: 7,
        flip_at: Some(5),
        ..ReferenceBatch::default()
    })));
    let (difference, _, _) = race(&mut reference, &mut batched, 12);
    assert_eq!(difference, Some(5));
}

/// A backend that fails is dropped, the tick is redone on the reference, and
/// the run continues unchanged.
#[test]
fn a_failing_backend_is_dropped_and_the_world_is_unchanged() {
    let config = config();
    let mut reference = found(&config);
    let mut batched = found(&config);
    batched.set_controller_backend(Some(Box::new(ReferenceBatch {
        defer_every: 3,
        fail_at: Some(4),
        ..ReferenceBatch::default()
    })));
    let (difference, evaluated, _) = race(&mut reference, &mut batched, 20);
    assert_eq!(difference, None);
    let report = batched.controller_report().clone();
    assert_eq!(report.reference_ticks, 1);
    assert!(
        report
            .failure
            .as_deref()
            .is_some_and(|failure| failure.contains("a planted device failure")),
        "{report:?}"
    );
    // Three batched ticks, then the reference alone.
    assert!(report.batched + report.deferred < evaluated);
    assert!(report.batched + report.deferred > 0);
}

/// Reference controller implementing the Round-2 `supports_flat_f32` /
/// `evaluate_flat_f32` fast path, deferring every `defer_every`-th job.
#[derive(Debug, Default)]
struct FlatReferenceBatch {
    defer_every: usize,
    scratch: ControllerScratch,
}

impl ControllerBackend for FlatReferenceBatch {
    fn describe(&self) -> String {
        "flat f32 reference batch (test)".to_owned()
    }

    fn resident_bytes(&self) -> u64 {
        self.scratch.allocated_bytes()
    }

    fn forget(&mut self) {}

    fn supports_flat_f32(&self) -> bool {
        true
    }

    fn evaluate(
        &mut self,
        _jobs: &[ControllerJob<'_>],
        _outputs: &mut Vec<Option<RawOutputs>>,
    ) -> Result<(), CoreError> {
        Err(CoreError::StateConflict {
            operation: "test flat backend",
            detail: "supports_flat_f32 backend should be called via evaluate_flat_f32".to_owned(),
        })
    }

    fn evaluate_flat_f32(
        &mut self,
        jobs: &[FlatControllerJob<'_>],
        input_width: usize,
        inputs_f32: &[f32],
        inputs_finite: &[u8],
        outputs: &mut FlatOutputs,
    ) -> Result<(), CoreError> {
        let Some(first) = jobs.first() else {
            outputs.clear();
            return Ok(());
        };
        let out_w = first.controller.topology().raw_output_width();
        outputs.prepare(jobs.len(), out_w)?;
        let mut wide = vec![0.0_f64; input_width];
        for (index, (job, &finite)) in jobs.iter().zip(inputs_finite).enumerate() {
            if finite == 0 || (self.defer_every > 0 && index % self.defer_every == 0) {
                continue;
            }
            let src = &inputs_f32[index * input_width..(index + 1) * input_width];
            for (dst, &v) in wide.iter_mut().zip(src) {
                *dst = f64::from(v);
            }
            if let Ok(raw) = job.controller.evaluate_raw(&wide, &mut self.scratch) {
                let dst = &mut outputs.values[index * out_w..(index + 1) * out_w];
                dst.copy_from_slice(raw.as_slice());
                outputs.valid[index] = 1;
            }
        }
        Ok(())
    }
}

#[test]
fn flat_f32_batched_world_matches_reference_bit_for_bit_and_never_grows() {
    let config = config();
    for workers in [1, 4] {
        let w = NonZeroUsize::new(workers).expect("nonzero");
        let mut reference = found(&config);
        let mut batched = found(&config);
        reference.set_cpu_workers(w).expect("workers");
        batched.set_cpu_workers(w).expect("workers");

        let before = batched.allocated();
        let flat_batched_bytes = batched.flat_f32_batched_bytes();
        assert!(
            flat_batched_bytes > 0 && flat_batched_bytes < batched.batched_bytes(),
            "flat-F32 staging ({flat_batched_bytes}) must be strictly smaller than legacy ({})",
            batched.batched_bytes()
        );
        batched.set_controller_backend(Some(Box::new(FlatReferenceBatch {
            defer_every: 5,
            ..FlatReferenceBatch::default()
        })));
        assert_eq!(batched.controller_report().failure, None);
        let installed = batched.allocated();
        assert_eq!(
            installed.scratch,
            before.scratch + flat_batched_bytes,
            "workers={workers}"
        );
        // Resizing workers while a flat-F32 backend is installed must not
        // allocate unused legacy per-lane Observation buffers.
        batched.set_cpu_workers(w).expect("re-set workers");
        assert_eq!(
            batched.allocated().scratch,
            installed.scratch,
            "workers={workers}: set_cpu_workers must preserve flat-F32 scratch"
        );

        let (difference, evaluated, births) = race(&mut reference, &mut batched, 200);
        assert_eq!(difference, None, "workers={workers}: flat-f32 diverged");
        assert!(births > 0, "workers={workers}: must exercise newborns");
        assert_eq!(
            batched.allocated().scratch,
            installed.scratch,
            "workers={workers}: flat-f32 scratch grew"
        );
        let report = batched.controller_report();
        assert_eq!(report.failure, None);
        assert_eq!(report.reference_ticks, 0);
        assert_eq!(report.batched + report.deferred, evaluated);
        assert!(report.deferred > 0 && report.batched > 3 * report.deferred);

        batched.set_controller_backend(None);
        assert_eq!(batched.allocated().scratch, before.scratch);
    }
}
