//! FL-043, plan §19.4 ("use device memory limits and actual allocation
//! results"), read from the device itself: a device owner admits no more
//! than the device's headroom as it opens, a backend's budget past it is cut
//! to it, and what the device holds for the controller backend is measured
//! against what its owner counts. One test in a process of its own, so
//! nothing else allocates on the device while it measures.

use flife_backend_metal::{BackendBudget, DeviceOwner, Limits, MetalBackend};
use flife_core::CoreError;
use flife_core::ValidatedConfig;
use flife_core::memory::RESERVE_BYTES;
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use std::num::NonZeroUsize;

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// The most the driver holds for one buffer beyond its length. FrankenTorch's
/// `ft-kernel-metal` tests/device_memory.rs measures it on the device: a
/// buffer of 64 KiB or more takes whole 16 KiB pages, less than 64 KiB past
/// its length, and a smaller one comes from a pool the device holds, which
/// grows by less than 128 KiB for it.
const ROUNDING_BYTES: u64 = 128 << 10;

/// Ballast a first owner holds while the backend opens, so the headroom
/// the backend's owner reads is the working set less something.
const BALLAST_FLOATS: usize = 1 << 20;

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
    let workers = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    world
        .set_cpu_workers(NonZeroUsize::new(workers.min(16)).expect("nonzero"))
        .expect("workers");
    world
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / f64::from(1 << 20)
}

#[test]
fn the_device_holds_what_the_owner_counts_within_its_headroom() {
    let probe = match DeviceOwner::open(Limits::UNBOUNDED) {
        Ok(probe) => probe,
        Err(CoreError::MissingCapability { .. }) if !cfg!(target_os = "macos") => {
            println!("no Metal device: skipped");
            return;
        }
        Err(error) => panic!("this host should have Metal: {error}"),
    };
    let working_set = probe.device_working_set_bytes();
    assert!(working_set > 0, "a Metal device recommends a working set");
    let ballast = probe
        .zeroed::<f32>(BALLAST_FLOATS)
        .expect("the probe's ballast");
    let before = probe.device_allocated_bytes();
    assert!(
        before >= 4 * BALLAST_FLOATS as u64,
        "the device holds the ballast: {before}"
    );
    assert!(probe.limits().bytes <= working_set);

    // An unbounded request: the owner admits the headroom it read as it
    // opened, after the ballast and before its kernels compiled, and the
    // backend's budget is cut to it.
    let mut world = found();
    let organisms = world.population().capacity() as u64;
    let backend = MetalBackend::with_budget(
        u64::MAX,
        BackendBudget {
            bytes: u64::MAX - 1,
            organisms,
        },
    )
    .expect("a backend");
    let owner = backend.owner().clone();
    let compiled = owner.device_allocated_bytes();
    let limit = owner.limits().bytes;
    println!(
        "working set {working_set} ({:.1} MiB); held before the backend {before}; \
         limit {limit}; headroom before {} and after its kernels {}",
        mib(working_set),
        working_set - before,
        working_set - compiled
    );
    assert!(
        (working_set - compiled..=working_set - before).contains(&limit),
        "the owner admits the headroom it read as it opened"
    );
    assert_eq!(
        backend.budget(),
        BackendBudget {
            bytes: limit,
            organisms
        },
        "a budget past the headroom is cut to it"
    );
    let kernels = compiled - before;
    println!(
        "the controller kernels hold {kernels} ({:.2} MiB) on the device",
        mib(kernels)
    );
    assert!(
        kernels <= RESERVE_BYTES,
        "the compiled kernels are within the reserve"
    );

    // Run it against the reference; then the device holds the owner's
    // buffers, as the driver rounds them.
    world.set_controller_backend(Some(Box::new(backend)));
    let mut reference = found();
    for _ in 0..40 {
        let expected = reference.step().expect("the reference steps");
        let got = world.step().expect("the metal world steps");
        assert!(matches!(
            (expected, got),
            (Stepped::Published(_), Stepped::Published(_))
        ));
        assert_eq!(reference.digest(), world.digest());
    }
    let report = world.controller_report();
    assert!(report.failure.is_none(), "{:?}", report.failure);
    assert!(report.batched > 0 && report.deferred == 0, "{report:?}");
    let usage = owner.usage();
    let held = owner.device_allocated_bytes() - compiled;
    println!(
        "buffers {}: the owner counts {} ({:.2} MiB) and the device holds {} ({:.2} MiB) for them; \
         backend resident peak {} ({:.2} MiB)",
        usage.buffers,
        usage.bytes,
        mib(usage.bytes),
        held,
        mib(held),
        report.resident_peak_bytes,
        mib(report.resident_peak_bytes)
    );
    assert!(usage.buffers > 0 && usage.bytes > 0, "{usage:?}");
    assert!(
        held <= usage.bytes + usage.buffers as u64 * ROUNDING_BYTES,
        "the device holds no more than the owner counts and the driver's rounding"
    );
    assert!(
        usage.bytes <= report.resident_peak_bytes,
        "the backend's resident peak covers its device buffers"
    );

    // A budget within the headroom is kept whole, and an unbounded one
    // stays unbounded: its owner alone refuses past the headroom.
    let small = BackendBudget {
        bytes: 64 << 20,
        organisms,
    };
    let kept = MetalBackend::with_budget(1, small).expect("a backend");
    assert_eq!(kept.budget(), small);
    assert_eq!(kept.owner().limits().bytes, small.bytes);
    let unbounded = MetalBackend::with_budget(1, BackendBudget::UNBOUNDED).expect("a backend");
    assert_eq!(unbounded.budget(), BackendBudget::UNBOUNDED);
    assert!(unbounded.owner().limits().bytes <= working_set);
    assert!(
        probe.retire(ballast).expect("the ballast retires"),
        "nothing leases the ballast, so it is freed at once"
    );
}
