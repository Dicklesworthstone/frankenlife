//! FL-030's engine wiring: the real small-world preset (4,096 founders,
//! `[64, 32]` controllers, every organism evaluated every tick) run with its
//! controllers on the Metal backend is the reference world, digest for
//! digest, at every tick through the founder crash, births and mutation.

use flife_backend_metal::{AUTO_METAL_MAX_WORKERS, ControllerChoice, MetalBackend};
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_neural::{ACTION_WIDTH, ControllerBackend, ControllerJob};
use std::num::NonZeroUsize;

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

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
    // Threads change no result; they only make the reference side quick.
    let workers = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    world
        .set_cpu_workers(NonZeroUsize::new(workers.min(16)).expect("nonzero"))
        .expect("workers");
    world
}

/// `None` off macOS, where there is no Metal device: then `open` must say
/// so with its typed refusal.
#[expect(clippy::panic, reason = "test helper")]
fn metal() -> Option<MetalBackend> {
    match MetalBackend::open() {
        Ok(backend) => Some(backend),
        Err(CoreError::MissingCapability { capability }) if !cfg!(target_os = "macos") => {
            assert_eq!(capability, "a Metal device");
            None
        }
        Err(error) => panic!("this host should have Metal: {error}"),
    }
}

/// Step both worlds; the first tick whose digests differ, the evaluations,
/// births and the batched world's last live count.
#[expect(clippy::expect_used, clippy::panic, reason = "test helper")]
fn race(reference: &mut World, metal: &mut World, ticks: u64) -> (Option<u64>, u64, usize, usize) {
    let (mut difference, mut evaluated, mut births, mut live) = (None, 0_u64, 0, 0);
    for _ in 0..ticks {
        let expected = reference.step().expect("the reference steps");
        let got = metal.step().expect("the metal world steps");
        match (expected, got) {
            (Stepped::Published(_), Stepped::Published(report)) => {
                evaluated += report.evaluated as u64;
                births += report.births.len();
                live = report.live;
            }
            (expected, got) => panic!("unexpected outcomes {expected:?} and {got:?}"),
        }
        if difference.is_none() && reference.digest() != metal.digest() {
            difference = Some(metal.tick().get());
        }
    }
    (difference, evaluated, births, live)
}

#[test]
fn the_preset_on_metal_is_the_reference_world_at_every_tick() {
    let Some(backend) = metal() else { return };
    let description = backend.describe();
    let mut reference = found();
    let mut metal = found();
    metal.set_controller_backend(Some(Box::new(backend)));
    let (difference, evaluated, births, live) = race(&mut reference, &mut metal, 400);
    let report = metal.controller_report();
    println!("evaluated {evaluated}, births {births}, live {live}: {report:?}");
    assert_eq!(difference, None, "the worlds diverged");
    assert_eq!(report.backend.as_deref(), Some(description.as_str()));
    assert_eq!(report.failure, None);
    assert_eq!(report.reference_ticks, 0);
    assert_eq!(report.batched + report.deferred, evaluated);
    // The device, not the fallback, did the work.
    assert!(report.batched > 1_000_000, "{report:?}");
    assert!(births > 0, "newborn genomes must be uploaded mid-run");
}

/// The planted negative: the same backend with one output bit of one
/// organism flipped at one tick must change that tick's digest.
#[derive(Debug)]
struct Flipped {
    inner: MetalBackend,
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
fn one_flipped_metal_output_bit_changes_the_world() {
    let Some(inner) = metal() else { return };
    let mut reference = found();
    let mut metal = found();
    metal.set_controller_backend(Some(Box::new(Flipped {
        inner,
        calls: 0,
        flip_at: 9,
    })));
    let (difference, ..) = race(&mut reference, &mut metal, 12);
    assert_eq!(difference, Some(9));
}

/// `auto` takes Metal up to `AUTO_METAL_MAX_WORKERS` CPU workers on a host
/// with a device, and the CPU reference above that or without one; `cpu`
/// never opens a device, and `metal` ignores the worker count.
#[test]
fn auto_takes_metal_only_up_to_its_worker_ceiling() {
    let device = metal().is_some();
    let described = |choice: ControllerChoice, workers: usize| {
        choice
            .backend(workers)
            .map(|backend| backend.map(|backend| backend.describe()))
    };
    for workers in [1, 2, AUTO_METAL_MAX_WORKERS] {
        let chosen = described(ControllerChoice::Auto, workers).expect("auto never refuses");
        assert_eq!(chosen.is_some(), device, "auto at {workers} workers");
        if let Some(name) = chosen {
            assert!(name.starts_with("metal"), "{name}");
        }
    }
    for workers in [AUTO_METAL_MAX_WORKERS + 1, 16, 64] {
        assert!(
            described(ControllerChoice::Auto, workers)
                .expect("auto never refuses")
                .is_none(),
            "auto at {workers} workers is the reference"
        );
    }
    assert!(described(ControllerChoice::Cpu, 1).expect("cpu").is_none());
    let forced = described(ControllerChoice::Metal, 64);
    if device {
        assert!(forced.expect("metal on a device").is_some());
    } else {
        assert!(matches!(forced, Err(CoreError::MissingCapability { .. })));
    }
}
