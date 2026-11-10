//! FL-025's engine wiring: the real small-world preset (4,096 founders,
//! `[64, 32]` controllers, every organism evaluated every tick) run with its
//! controllers on the CPU backend is the reference world, digest for digest,
//! at every tick through the founder crash, births and mutation, on the
//! world's own lanes.

use std::num::NonZeroUsize;

use flife_backend_cpu::CpuBackend;
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_neural::{ACTION_WIDTH, ControllerBackend, ControllerJob};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// The worker count both worlds run on, and the backend's lanes.
#[expect(clippy::expect_used, reason = "test helper")]
fn workers() -> NonZeroUsize {
    let available = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    NonZeroUsize::new(available.min(16)).expect("nonzero")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure is a test failure"
)]
fn found() -> World {
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset validates");
    let mut world = World::found(
        &config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("the world founds");
    world.set_cpu_workers(workers()).expect("workers");
    world
}

/// Step both worlds; the first tick whose digests differ, the evaluations
/// and the births.
#[expect(clippy::expect_used, clippy::panic, reason = "test helper")]
fn race(reference: &mut World, other: &mut World, ticks: u64) -> (Option<u64>, u64, usize) {
    let (mut difference, mut evaluated, mut births) = (None, 0_u64, 0);
    for _ in 0..ticks {
        let expected = reference.step().expect("the reference steps");
        let got = other.step().expect("the backend's world steps");
        match (expected, got) {
            (Stepped::Published(_), Stepped::Published(report)) => {
                evaluated += report.evaluated as u64;
                births += report.births.len();
            }
            (expected, got) => panic!("unexpected outcomes {expected:?} and {got:?}"),
        }
        if difference.is_none() && reference.digest() != other.digest() {
            difference = Some(other.tick().get());
        }
    }
    (difference, evaluated, births)
}

#[test]
fn the_preset_on_the_cpu_backend_is_the_reference_world_at_every_tick() {
    let backend = CpuBackend::new(workers());
    let description = backend.describe();
    let mut reference = found();
    let mut batched = found();
    batched.set_controller_backend(Some(Box::new(backend)));
    let (difference, evaluated, births) = race(&mut reference, &mut batched, 400);
    let report = batched.controller_report();
    println!("evaluated {evaluated}, births {births}: {report:?}");
    assert_eq!(difference, None, "the worlds diverged");
    assert_eq!(report.backend.as_deref(), Some(description.as_str()));
    assert_eq!(report.failure, None);
    assert_eq!(report.reference_ticks, 0);
    assert_eq!(report.batched + report.deferred, evaluated);
    // The backend, not the fallback, did the work: on the CPU nothing is
    // deferred for being small, and the preset's controllers refuse nothing.
    assert_eq!(report.deferred, 0, "{report:?}");
    assert!(report.batched > 1_000_000, "{report:?}");
    assert!(births > 0, "newborn genomes must be packed mid-run");
}

/// The planted negative: the same backend with one output bit of one
/// organism flipped at one tick must change that tick's digest.
#[derive(Debug)]
struct Flipped {
    inner: CpuBackend,
    calls: u64,
    flip_at: u64,
}

impl ControllerBackend for Flipped {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn forget(&mut self) {
        self.inner.forget();
    }

    fn evaluate(
        &mut self,
        jobs: &[ControllerJob<'_>],
        outputs: &mut Vec<Option<[f32; ACTION_WIDTH]>>,
    ) -> Result<(), CoreError> {
        self.inner.evaluate(jobs, outputs)?;
        self.calls += 1;
        if self.calls == self.flip_at
            && let Some(Some(raw)) = outputs.get_mut(17)
        {
            raw[3] = f32::from_bits(raw[3].to_bits() ^ 1);
        }
        Ok(())
    }
}

#[test]
fn one_flipped_output_bit_changes_the_world() {
    let mut reference = found();
    let mut batched = found();
    batched.set_controller_backend(Some(Box::new(Flipped {
        inner: CpuBackend::new(workers()),
        calls: 0,
        flip_at: 9,
    })));
    let (difference, ..) = race(&mut reference, &mut batched, 12);
    assert_eq!(difference, Some(9));
}
