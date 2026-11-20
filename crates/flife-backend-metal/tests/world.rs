//! FL-030's engine wiring: the real small-world preset (4,096 founders,
//! `[64, 32]` controllers, every organism evaluated every tick) run with its
//! controllers on the Metal backend is the reference world, digest for
//! digest, at every tick through the founder crash, births and mutation.
//! FL-028: every buffer it uses is the owner's, and none is left leased,
//! bound or retiring. FL-026: each genome is uploaded once while it lives,
//! with every controller due each tick or one tick in four, and in shards
//! of a few slots, and no parameter shard is rebuilt.

use flife_backend_metal::{
    AUTO_METAL_MAX_WORKERS, AUTO_TORCH_MAX_WORKERS, AUTO_TORCH_MIN_ORGANISMS, BackendBudget,
    ControllerChoice, MetalBackend, SHARD_BYTES, TORCH_AT_SCALE,
};
use flife_core::ids::GenomeId;
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_neural::{ACTION_WIDTH, ControllerBackend, ControllerJob};
use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

    fn resident_bytes(&self) -> u64 {
        self.inner.resident_bytes()
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

/// FL-030: the preset in compact F16 on Metal. Each binary16 genome is
/// uploaded as binary16 into a half shard and the half kernel widens it
/// exactly to the F32 the reference reads, so the world is the reference F16
/// world at every tick, newborns included, and the device does the work.
#[test]
fn an_f16_world_on_metal_is_the_reference_world_at_every_tick() {
    let Some(backend) = metal() else { return };
    let preset = PRESET.replace(
        "canonical_weight_dtype = \"f32\"",
        "canonical_weight_dtype = \"f16\"",
    );
    assert_ne!(preset, PRESET, "the preset declares its weight dtype");
    let mut reference = found_from(&preset);
    let mut metal = found_from(&preset);
    metal.set_controller_backend(Some(Box::new(backend)));
    let (difference, evaluated, births, live) = race(&mut reference, &mut metal, 400);
    let report = metal.controller_report();
    println!("evaluated {evaluated}, births {births}, live {live}: {report:?}");
    assert_eq!(difference, None, "the worlds diverged");
    assert_eq!(report.failure, None);
    assert_eq!(report.reference_ticks, 0);
    assert_eq!(report.batched + report.deferred, evaluated);
    assert!(report.batched > 1_000_000, "{report:?}");
    assert!(
        births > 0,
        "newborn binary16 genomes must be uploaded mid-run"
    );
}

/// FL-043: under half of what an unbounded backend holds after 40 ticks,
/// shards that would not fit are not made and their jobs go to the
/// reference, so the world is the reference world at every tick while the
/// device owner and the backend's host vectors hold no more than the
/// budget. A backend that ignored the budget would hold twice as much.
#[test]
fn under_half_its_bytes_metal_defers_and_the_world_is_the_reference() {
    let Some(backend) = metal() else { return };
    let mut probe = found();
    probe.set_controller_backend(Some(Box::new(backend)));
    let mut reference = found();
    let (difference, ..) = race(&mut reference, &mut probe, 40);
    assert_eq!(difference, None);
    let unbounded = probe.controller_report().resident_peak_bytes;
    assert!(unbounded > 0);
    let budget = BackendBudget {
        bytes: unbounded / 2,
        organisms: probe.population().capacity() as u64,
    };
    drop(probe);

    let backend = MetalBackend::with_budget(u64::MAX, budget).expect("a budgeted backend");
    let owner = backend.owner().clone();
    // The owner refuses any device buffer past the budget too.
    assert_eq!(owner.limits().bytes, budget.bytes);
    let mut reference = found();
    let mut tight = found();
    tight.set_controller_backend(Some(Box::new(backend)));
    let (difference, evaluated, births, _) = race(&mut reference, &mut tight, 400);
    let report = tight.controller_report();
    let usage = owner.usage();
    println!(
        "unbounded {unbounded}, budget {budget:?}: evaluated {evaluated}, births {births}, \
         owner {usage:?}: {report:?}"
    );
    assert_eq!(difference, None, "the worlds diverged");
    assert_eq!(report.failure, None);
    assert_eq!(report.reference_ticks, 0);
    assert_eq!(report.batched + report.deferred, evaluated);
    assert!(report.deferred > 0 && report.batched > 0, "{report:?}");
    assert!(births > 0, "newborns must be uploaded under the budget");
    assert!(usage.bytes <= budget.bytes, "{usage:?} over {budget:?}");
    assert!(
        report.resident_peak_bytes <= budget.bytes,
        "the backend held {} of a {}-byte budget",
        report.resident_peak_bytes,
        budget.bytes
    );
}

/// At K = 4 a controller is due one tick in four, so a batch never holds
/// most of the live genomes. In shards of 64 slots, so that a tick runs
/// dozens of kernels in one command, the world is still the reference
/// world, and each genome is uploaded exactly once while it lives.
/// The Metal backend, summing its receipts' device seconds
/// ([`MetalBackend::last_command`]) as each batch returns.
#[derive(Debug)]
struct Timed {
    inner: MetalBackend,
    seconds: Arc<Mutex<f64>>,
}

impl ControllerBackend for Timed {
    fn describe(&self) -> String {
        self.inner.describe()
    }

    fn resident_bytes(&self) -> u64 {
        self.inner.resident_bytes()
    }

    fn forget(&mut self) {
        self.inner.forget();
    }

    fn last_device_nanoseconds(&self) -> u64 {
        self.inner.last_device_nanoseconds()
    }

    fn evaluate(
        &mut self,
        jobs: &[ControllerJob<'_>],
        outputs: &mut Vec<Option<[f32; ACTION_WIDTH]>>,
    ) -> Result<(), CoreError> {
        self.inner.evaluate(jobs, outputs)?;
        let mut seconds = self.seconds.lock().map_err(|_| CoreError::StateConflict {
            operation: "sum device seconds",
            detail: "poisoned".to_owned(),
        })?;
        *seconds += self.inner.last_command().map_or(0.0, |(_, time)| time);
        Ok(())
    }
}

/// Step `world` `ticks` times on `backend`. After each tick, the report's
/// device time grew if the backend returned evaluations, and stayed where
/// it was if it returned none. At the end it is the receipts' seconds, to
/// a nanosecond a batch, and within the steps' wall time: submission waits
/// for each command buffer. Returns the ticks that batched something and
/// those that batched nothing.
#[expect(clippy::expect_used, reason = "test helper")]
fn device_time_follows_the_batches(
    world: &mut World,
    backend: MetalBackend,
    ticks: u64,
) -> (u64, u64) {
    let seconds = Arc::new(Mutex::new(0.0));
    world.set_controller_backend(Some(Box::new(Timed {
        inner: backend,
        seconds: Arc::clone(&seconds),
    })));
    let (mut batching, mut idle) = (0, 0);
    let mut last = world.controller_report().clone();
    assert_eq!(last.device_nanoseconds, 0, "no batch yet");
    let start = Instant::now();
    for _ in 0..ticks {
        world.step().expect("the metal world steps");
        let report = world.controller_report().clone();
        if report.batched > last.batched {
            assert!(
                report.device_nanoseconds > last.device_nanoseconds,
                "tick {}: {} evaluations on the device took no device time: {report:?}",
                world.tick().get(),
                report.batched - last.batched
            );
            batching += 1;
        } else {
            assert_eq!(
                report.device_nanoseconds,
                last.device_nanoseconds,
                "tick {}: a tick that batched nothing adds no device time",
                world.tick().get()
            );
            idle += 1;
        }
        last = report;
    }
    let wall = start.elapsed();
    let device = Duration::from_nanos(last.device_nanoseconds);
    let receipts = Duration::from_secs_f64(*seconds.lock().expect("not poisoned"));
    println!(
        "{ticks} ticks ({batching} batching, {idle} idle): device {device:?}, receipts \
         {receipts:?}, wall {wall:?}"
    );
    assert!(
        device.abs_diff(receipts) <= Duration::from_nanos(batching + 1),
        "the report's {device:?} is not the receipts' {receipts:?}"
    );
    assert!(device <= wall, "device {device:?} > wall {wall:?}");
    (batching, idle)
}

#[test]
fn the_report_sums_the_devices_time_on_its_batches() {
    let Some(backend) = metal() else { return };
    // The preset: every founder due every tick, so every tick batches.
    let mut world = found();
    assert_eq!(
        device_time_follows_the_batches(&mut world, backend, 40),
        (40, 0)
    );

    // One founder, due one tick in four: the ticks between batch nothing,
    // and the backend's last command must not be counted again for them.
    let Some(backend) = metal() else { return };
    let preset = PRESET
        .replace("initial_count = 4096\n", "initial_count = 1\n")
        .replace(
            "evaluation_interval_ticks = 1\n",
            "evaluation_interval_ticks = 4\n",
        );
    assert_ne!(preset, PRESET);
    let mut world = found_from(&preset);
    let (batching, idle) = device_time_follows_the_batches(&mut world, backend, 40);
    assert!(batching > 0 && idle > 0, "{batching} batching, {idle} idle");
}

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

    fn resident_bytes(&self) -> u64 {
        self.inner.resident_bytes()
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
/// with a device. Without one, or above that, it takes the FrankenTorch CPU
/// route up to `AUTO_TORCH_MAX_WORKERS`, or, on a host where
/// `TORCH_AT_SCALE`, for a world of at least `AUTO_TORCH_MIN_ORGANISMS`, and
/// the CPU reference otherwise. `cpu` never opens a device, `torch` is the
/// FrankenTorch CPU route on the world's workers on any host, and `metal`
/// ignores the worker count.
#[test]
fn auto_takes_metal_only_up_to_its_worker_ceiling() {
    let device = metal().is_some();
    let described = |choice: ControllerChoice, workers: usize, organisms: usize| {
        choice
            .backend(workers, organisms, BackendBudget::UNBOUNDED)
            .map(|backend| backend.map(|backend| backend.describe()))
    };
    let torch =
        |workers: usize| format!("cpu (frankentorch population_affine_f32_into, {workers} lanes)");
    // The rule on both kinds of host, wherever this runs: the preset's
    // 4,096 organisms, just below the floor, at it, and the Desktop
    // ecology preset's 100,000.
    use ControllerChoice::{Cpu, Metal, Torch};
    let small = [
        (1, Metal, Torch),
        (2, Metal, Torch),
        (AUTO_TORCH_MAX_WORKERS, Metal, Torch),
        (AUTO_TORCH_MAX_WORKERS + 1, Metal, Cpu),
        (AUTO_METAL_MAX_WORKERS, Metal, Cpu),
        (AUTO_METAL_MAX_WORKERS + 1, Cpu, Cpu),
        (16, Cpu, Cpu),
        (64, Cpu, Cpu),
    ];
    let large = small.map(|(workers, with_device, _)| {
        let with_device = if with_device == Metal { Metal } else { Torch };
        (workers, with_device, Torch)
    });
    assert_eq!(
        TORCH_AT_SCALE,
        cfg!(all(target_os = "macos", target_arch = "aarch64")),
        "the floor holds only where it was measured faster"
    );
    for (organisms, at_scale) in [
        (4_096, small),
        (AUTO_TORCH_MIN_ORGANISMS - 1, small),
        (AUTO_TORCH_MIN_ORGANISMS, large),
        (100_000, large),
    ] {
        // A host where torch is not faster at scale keeps the small rule.
        for (torch_at_scale, rule) in [(true, at_scale), (false, small)] {
            for (workers, with_device, without) in rule {
                assert_eq!(
                    ControllerChoice::auto_on(workers, true, torch_at_scale, organisms),
                    with_device,
                    "{workers} workers, {organisms} organisms, torch at scale {torch_at_scale}"
                );
                assert_eq!(
                    ControllerChoice::auto_on(workers, false, torch_at_scale, organisms),
                    without,
                    "{workers} workers, {organisms} organisms, torch at scale {torch_at_scale}"
                );
            }
        }
        for (workers, ..) in small {
            // `auto_for` is the rule for this build's kind of host.
            for metal_device in [true, false] {
                assert_eq!(
                    ControllerChoice::auto_for(workers, metal_device, organisms),
                    ControllerChoice::auto_on(workers, metal_device, TORCH_AT_SCALE, organisms),
                );
            }
            // And this host's `auto` is the rule's choice for it.
            let expected = described(
                ControllerChoice::auto_for(workers, device, organisms),
                workers,
                organisms,
            )
            .expect("the rule's choice opens");
            let chosen =
                described(ControllerChoice::Auto, workers, organisms).expect("auto never refuses");
            assert_eq!(
                chosen, expected,
                "auto at {workers} workers, {organisms} organisms"
            );
            match chosen {
                Some(name) if device && workers <= AUTO_METAL_MAX_WORKERS => {
                    assert!(name.starts_with("metal"), "{name}");
                }
                Some(name) => {
                    assert!(
                        workers <= AUTO_TORCH_MAX_WORKERS
                            || (TORCH_AT_SCALE && organisms >= AUTO_TORCH_MIN_ORGANISMS),
                        "auto at {workers} workers, {organisms} organisms: {name}"
                    );
                    assert_eq!(name, torch(workers));
                }
                None => assert!(
                    workers > AUTO_TORCH_MAX_WORKERS
                        && !(TORCH_AT_SCALE && organisms >= AUTO_TORCH_MIN_ORGANISMS)
                        && !(device && workers <= AUTO_METAL_MAX_WORKERS),
                    "auto at {workers} workers, {organisms} organisms is the reference"
                ),
            }
        }
    }
    assert!(
        described(ControllerChoice::Cpu, 1, 4_096)
            .expect("cpu")
            .is_none()
    );
    for workers in [1, 4, 64] {
        let chosen = described(ControllerChoice::Torch, workers, 4_096)
            .expect("torch never refuses")
            .expect("torch is a backend");
        assert_eq!(chosen, torch(workers));
    }
    assert_eq!(
        "torch".parse::<ControllerChoice>(),
        Ok(ControllerChoice::Torch)
    );
    assert!("gpu".parse::<ControllerChoice>().is_err());
    let forced = described(ControllerChoice::Metal, 64, 4_096);
    if device {
        assert!(forced.expect("metal on a device").is_some());
    } else {
        assert!(matches!(forced, Err(CoreError::MissingCapability { .. })));
    }
}

/// FL-043: when organisms die in a live `World`, `Candidate::publish` notifies
/// the installed controller backend through `release_slots`, reclaiming device
/// shards once their occupants are all dead.
#[test]
fn deaths_reclaim_emptied_device_shards_in_a_world() {
    let Some(backend) = (match MetalBackend::with_shard_slots(8) {
        Ok(backend) => Some(backend),
        Err(CoreError::MissingCapability { .. }) if !cfg!(target_os = "macos") => None,
        Err(error) => panic!("this host should have Metal: {error}"),
    }) else {
        return;
    };
    let owner = backend.owner().clone();
    // 24 founders, 8 per shard -> 3 shards; high basal cost starves everyone
    // within a few ticks with zero sources.
    let toml = PRESET
        .replace("initial_count = 4096", "initial_count = 24")
        .replace("power_per_source = 960.0", "power_per_source = 1.0e-6")
        .replace(
            "basal_energy_per_second = 0.8",
            "basal_energy_per_second = 100.0",
        );
    assert!(
        toml.contains("power_per_source = 1.0e-6")
            && toml.contains("basal_energy_per_second = 100.0")
    );
    let mut world = found_from(&toml);
    world.set_controller_backend(Some(Box::new(backend)));

    // Tick 1 evaluates all 24 founders across 3 shards before starvation finishes.
    let first = world.step().expect("tick 1");
    assert!(matches!(first, Stepped::Published(_)));
    let peak_bytes = owner.usage().bytes;
    let peak_buffers = owner.usage().buffers;
    assert!(
        peak_buffers >= 2 * 3 + 3,
        "expected 3 shards on device after tick 1, got {peak_buffers} buffers"
    );

    // Step until all 24 organisms die.
    for _ in 0..20 {
        if world.population().is_empty() {
            break;
        }
        assert!(matches!(world.step().expect("step"), Stepped::Published(_)));
    }
    assert!(
        world.population().is_empty(),
        "high basal cost must starve all 24 founders"
    );
    let after = owner.usage();
    assert_eq!(
        after.buffers, 3,
        "all 3 emptied shards must drop from device once all occupants die ({after:?})"
    );
    assert!(
        after.bytes < peak_bytes,
        "device bytes must drop after extinction ({} vs {peak_bytes})",
        after.bytes
    );
}
