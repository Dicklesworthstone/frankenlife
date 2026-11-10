//! FL-030's engine wiring: the real small-world preset (4,096 founders,
//! `[64, 32]` controllers, every organism evaluated every tick) run with its
//! controllers on the Metal backend is the reference world, digest for
//! digest, at every tick through the founder crash, births and mutation.
//! FL-028: every buffer it uses is the owner's, and none is left leased,
//! bound or retiring. FL-026: each genome is uploaded once while it lives,
//! with every controller due each tick or one tick in four, and in shards
//! of a few slots, and no parameter shard is rebuilt.

use flife_backend_metal::{
    AUTO_METAL_MAX_WORKERS, AUTO_TORCH_MAX_WORKERS, ControllerChoice, MetalBackend, SHARD_BYTES,
};
use flife_core::ids::GenomeId;
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_neural::{ACTION_WIDTH, ControllerBackend, ControllerJob};
use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

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

/// What a [`Counted`] backend saw: the genomes it was asked to evaluate,
/// and its uploads, resident slots and shards after the last batch.
#[derive(Debug, Default)]
struct Seen {
    genomes: BTreeSet<GenomeId>,
    uploads: u64,
    resident: usize,
    shards: usize,
}

/// The Metal backend, recording what it saw.
#[derive(Debug)]
struct Counted {
    inner: MetalBackend,
    seen: Arc<Mutex<Seen>>,
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
            operation: "count uploads",
            detail: "poisoned".to_owned(),
        })?;
        seen.genomes.extend(jobs.iter().map(|job| job.genome));
        seen.uploads = self.inner.uploads();
        seen.resident = self.inner.resident_genomes();
        seen.shards = self.inner.shards();
        Ok(())
    }
}

#[test]
fn the_preset_on_metal_is_the_reference_world_at_every_tick() {
    let Some(backend) = metal() else { return };
    let description = backend.describe();
    let owner = backend.owner().clone();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut reference = found();
    let mut metal = found();
    metal.set_controller_backend(Some(Box::new(Counted {
        inner: backend,
        seen: Arc::clone(&seen),
    })));
    let (difference, evaluated, births, live) = race(&mut reference, &mut metal, 400);
    let report = metal.controller_report();
    let seen = seen.lock().expect("not poisoned");
    println!(
        "evaluated {evaluated}, births {births}, live {live}, genomes {}, uploads {}, \
         resident {}, shards {}: {report:?}",
        seen.genomes.len(),
        seen.uploads,
        seen.resident,
        seen.shards
    );
    assert_eq!(difference, None, "the worlds diverged");
    assert_eq!(report.backend.as_deref(), Some(description.as_str()));
    assert_eq!(report.failure, None);
    assert_eq!(report.reference_ticks, 0);
    assert_eq!(report.batched + report.deferred, evaluated);
    // The device, not the fallback, did the work.
    assert!(report.batched > 1_000_000, "{report:?}");
    assert!(births > 0, "newborn genomes must be uploaded mid-run");
    // FL-026: each genome was uploaded once, newborns into dead genomes'
    // slots, so the one shard of the founders' topology holds every genome.
    assert_eq!(seen.uploads, seen.genomes.len() as u64);
    assert!(seen.resident < seen.genomes.len(), "{seen:?}");
    assert_eq!(seen.shards, 1, "{seen:?}");
    // FL-028: after 400 ticks the owner holds exactly the backend's resident
    // buffers (each shard's parameters and job words; the inputs, outputs
    // and fault flags), with no lease, command or retiring buffer left over.
    let usage = owner.usage();
    assert_eq!(
        (
            usage.buffers,
            usage.retiring,
            usage.read_leases,
            usage.write_leases,
            usage.running
        ),
        (2 * seen.shards + 3, 0, [0; 3], 0, 0),
        "{usage:?}"
    );
    // The one shard is the default's byte budget, not the device's largest
    // buffer: it holds every live genome (9,988 parameters each) in at most
    // SHARD_BYTES, and the inputs, outputs, flags and job words take a few
    // MiB.
    assert!(
        usage.bytes > 4_096 * 9_988 * 4 && usage.bytes <= SHARD_BYTES + (8 << 20),
        "{usage:?}"
    );
}

/// At K = 4 a controller is due one tick in four, so a batch never holds
/// most of the live genomes. In shards of 64 slots, so that a tick runs
/// dozens of kernels in one command, the world is still the reference
/// world, and each genome is uploaded exactly once while it lives.
#[test]
fn at_interval_four_in_small_shards_each_genome_is_uploaded_once() {
    let Ok(inner) = MetalBackend::with_shard_slots(64) else {
        assert!(metal().is_none(), "this host should have Metal");
        return;
    };
    let preset = PRESET.replace(
        "evaluation_interval_ticks = 1\n",
        "evaluation_interval_ticks = 4\n",
    );
    assert_ne!(preset, PRESET, "the preset names its interval");
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut reference = found_from(&preset);
    let mut metal = found_from(&preset);
    metal.set_controller_backend(Some(Box::new(Counted {
        inner,
        seen: Arc::clone(&seen),
    })));
    let (difference, evaluated, births, live) = race(&mut reference, &mut metal, 400);
    let report = metal.controller_report();
    let seen = seen.lock().expect("not poisoned");
    println!(
        "evaluated {evaluated}, births {births}, live {live}, genomes {}, uploads {}, \
         resident {}, shards {}: {report:?}",
        seen.genomes.len(),
        seen.uploads,
        seen.resident,
        seen.shards
    );
    assert_eq!(difference, None, "the worlds diverged");
    assert_eq!(report.failure, None);
    assert_eq!(report.reference_ticks, 0);
    assert!(births > 0, "newborn genomes must be uploaded mid-run");
    let genomes = seen.genomes.len() as u64;
    assert!(
        evaluated > 3 * genomes,
        "{evaluated} evaluations of {genomes} genomes"
    );
    assert_eq!(
        seen.uploads, genomes,
        "a genome is uploaded once while it lives"
    );
    assert!(seen.resident < seen.genomes.len(), "{seen:?}");
    assert_eq!(seen.shards, 4_096 / 64, "{seen:?}");
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
/// with a device. Without one it takes the FrankenTorch CPU route up to
/// `AUTO_TORCH_MAX_WORKERS`, and the CPU reference above that; above
/// `AUTO_METAL_MAX_WORKERS` it is the reference on any host. `cpu` never
/// opens a device, `torch` is the FrankenTorch CPU route on the world's
/// workers on any host, and `metal` ignores the worker count.
#[test]
fn auto_takes_metal_only_up_to_its_worker_ceiling() {
    let device = metal().is_some();
    let described = |choice: ControllerChoice, workers: usize| {
        choice
            .backend(workers)
            .map(|backend| backend.map(|backend| backend.describe()))
    };
    // The rule on both kinds of host, wherever this runs.
    use ControllerChoice::{Cpu, Metal, Torch};
    let rule = [
        (1, Metal, Torch),
        (2, Metal, Torch),
        (AUTO_TORCH_MAX_WORKERS, Metal, Torch),
        (AUTO_TORCH_MAX_WORKERS + 1, Metal, Cpu),
        (AUTO_METAL_MAX_WORKERS, Metal, Cpu),
        (AUTO_METAL_MAX_WORKERS + 1, Cpu, Cpu),
        (64, Cpu, Cpu),
    ];
    for (workers, with_device, without) in rule {
        assert_eq!(
            ControllerChoice::auto_for(workers, true),
            with_device,
            "{workers}"
        );
        assert_eq!(
            ControllerChoice::auto_for(workers, false),
            without,
            "{workers}"
        );
        // And this host's `auto` is the rule's choice for it.
        let expected = described(ControllerChoice::auto_for(workers, device), workers)
            .expect("the rule's choice opens");
        assert_eq!(
            described(ControllerChoice::Auto, workers).expect("auto never refuses"),
            expected,
            "auto at {workers} workers"
        );
    }
    for workers in [
        1,
        2,
        AUTO_TORCH_MAX_WORKERS,
        AUTO_TORCH_MAX_WORKERS + 1,
        AUTO_METAL_MAX_WORKERS,
    ] {
        let chosen = described(ControllerChoice::Auto, workers).expect("auto never refuses");
        let torch = format!("cpu (frankentorch population_affine_f32_into, {workers} lanes)");
        match chosen {
            Some(name) if device => assert!(name.starts_with("metal"), "{name}"),
            Some(name) => {
                assert!(
                    workers <= AUTO_TORCH_MAX_WORKERS,
                    "auto at {workers} workers: {name}"
                );
                assert_eq!(name, torch);
            }
            None => assert!(
                !device && workers > AUTO_TORCH_MAX_WORKERS,
                "auto at {workers} workers is the reference"
            ),
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
    for workers in [1, 4, 64] {
        let torch = described(ControllerChoice::Torch, workers)
            .expect("torch never refuses")
            .expect("torch is a backend");
        assert_eq!(
            torch,
            format!("cpu (frankentorch population_affine_f32_into, {workers} lanes)")
        );
    }
    assert_eq!(
        "torch".parse::<ControllerChoice>(),
        Ok(ControllerChoice::Torch)
    );
    assert!("gpu".parse::<ControllerChoice>().is_err());
    let forced = described(ControllerChoice::Metal, 64);
    if device {
        assert!(forced.expect("metal on a device").is_some());
    } else {
        assert!(matches!(forced, Err(CoreError::MissingCapability { .. })));
    }
}
