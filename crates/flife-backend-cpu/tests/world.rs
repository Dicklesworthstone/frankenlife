//! FL-025's engine wiring: the real small-world preset (4,096 founders,
//! `[64, 32]` controllers, every organism evaluated every tick) run with its
//! controllers on the CPU backend is the reference world, digest for digest,
//! at every tick through the founder crash, births and mutation, on the
//! world's own lanes.
//!
//! FL-026: with each controller evaluated every fourth tick, and in shards
//! of a few slots, it is still the reference world, and the backend packs
//! each genome exactly once.

use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use flife_backend_cpu::CpuBackend;
use flife_core::ids::GenomeId;
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

fn found() -> World {
    found_from(PRESET)
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure is a test failure"
)]
fn found_from(preset: &str) -> World {
    let config = ValidatedConfig::from_toml(preset).expect("the preset validates");
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

/// The CPU backend, recording which genomes it was asked to evaluate and
/// how many it has packed.
#[derive(Debug)]
struct Counted {
    inner: CpuBackend,
    seen: Arc<Mutex<(BTreeSet<GenomeId>, u64, usize)>>,
}

impl ControllerBackend for Counted {
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
        let mut seen = self.seen.lock().map_err(|_| CoreError::StateConflict {
            operation: "count packs",
            detail: "poisoned".to_owned(),
        })?;
        seen.0.extend(jobs.iter().map(|job| job.genome));
        seen.1 = self.inner.packs();
        seen.2 = self.inner.resident();
        Ok(())
    }
}

/// At K = 4 a controller is due one tick in four, so the batch never holds
/// most of the live genomes. In shards of `shard_slots`, the world is still
/// the reference world, and every genome is packed exactly once while it
/// lives, newborns included.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure is a test failure"
)]
fn packed_once_at_interval_four(shard_slots: u64) {
    let preset = PRESET.replace(
        "evaluation_interval_ticks = 1\n",
        "evaluation_interval_ticks = 4\n",
    );
    assert_ne!(preset, PRESET, "the preset names its interval");
    let seen = Arc::new(Mutex::new((BTreeSet::new(), 0, 0)));
    let mut reference = found_from(&preset);
    let mut batched = found_from(&preset);
    batched.set_controller_backend(Some(Box::new(Counted {
        inner: CpuBackend::with_shard_slots(workers(), shard_slots),
        seen: Arc::clone(&seen),
    })));
    let (difference, evaluated, births) = race(&mut reference, &mut batched, 400);
    let report = batched.controller_report();
    let (genomes, packs, resident) = {
        let seen = seen.lock().expect("not poisoned");
        (seen.0.len() as u64, seen.1, seen.2)
    };
    println!(
        "{shard_slots} slots to a shard: evaluated {evaluated}, births {births}, \
         genomes {genomes}, packs {packs}, resident {resident}: {report:?}"
    );
    assert_eq!(difference, None, "the worlds diverged");
    assert_eq!(report.failure, None);
    assert_eq!(report.reference_ticks, 0);
    assert_eq!(report.batched + report.deferred, evaluated);
    assert!(births > 0, "newborns must be packed mid-run");
    // Each genome was evaluated about once per four ticks, and packed once.
    assert!(
        evaluated > 3 * genomes,
        "{evaluated} evaluations of {genomes} genomes"
    );
    assert_eq!(packs, genomes, "a genome is packed once while it lives");
    // Dead genomes' slots were reused: the slabs follow the peak, not every
    // genome ever seen.
    let population = batched.population();
    assert!(resident as u64 >= population.len() as u64);
    assert!(
        (resident as u64) < genomes,
        "{resident} resident of {genomes} genomes"
    );
}

#[test]
fn at_interval_four_each_genome_is_packed_once_and_the_world_is_the_reference() {
    packed_once_at_interval_four(256);
}

#[test]
fn in_shards_of_three_slots_the_world_is_the_reference() {
    packed_once_at_interval_four(3);
}
