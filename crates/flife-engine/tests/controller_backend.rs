//! The controller backend port (FL-030): a world whose due evaluations go
//! through a batch backend is the reference world, digest for digest, at
//! every tick, whatever the backend defers, and whether or not it fails.
//! These run on every host with a CPU batch backend; the Metal backend's own
//! world test is in `flife-backend-metal`.

use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_neural::{ACTION_WIDTH, ControllerBackend, ControllerJob, ControllerScratch};

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
        outputs: &mut Vec<Option<[f32; ACTION_WIDTH]>>,
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
            match job.controller.evaluate(job.inputs, &mut self.scratch) {
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
