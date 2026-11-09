//! FL-028 evidence (plan §9.5, §10.3): the Metal owner's typed buffers,
//! leases, command bindings and retirement on this host's device. It covers
//! alias and lifetime refusals, a running command's bindings held against
//! another thread, a command the gateway refuses, and consumers that exit.
//! The post-commit failure path is the crate's unit test (see the contract).

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use flife_backend_metal::{Buffer, DeviceOwner, Element, Limits, Purpose, ResourceState};
use flife_core::CoreError;
use flife_core::ids::ResourceEpoch;
use ft_kernel_metal::compute::{Grid, MathMode, Pipeline};

const KERNELS: &str = r"
#include <metal_stdlib>
using namespace metal;

kernel void add_one(device const float *a [[buffer(0)]],
                    device float *b [[buffer(1)]],
                    uint id [[thread_position_in_grid]]) {
    b[id] = a[id] + 1.0f;
}

kernel void add_pair(device const float *a [[buffer(0)]],
                     device const float *c [[buffer(1)]],
                     device float *b [[buffer(2)]],
                     uint id [[thread_position_in_grid]]) {
    b[id] = a[id] + c[id];
}

kernel void spin(device const uint *steps [[buffer(0)]],
                 device const uint *seed [[buffer(1)]],
                 device uint *out [[buffer(2)]],
                 uint id [[thread_position_in_grid]]) {
    uint x = seed[0] + id + 1u;
    for (uint i = 0u; i < steps[0]; ++i) {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
    }
    out[id] = x;
}
";

/// Iterations of `spin`: long enough (about half a second on the M5
/// Ultra) for another thread to act while it runs.
const STEPS: u32 = 1 << 27;

/// The owner, or `None` off macOS after checking its typed refusal.
fn owner(limits: Limits) -> Option<DeviceOwner> {
    match DeviceOwner::open(limits) {
        Ok(owner) => Some(owner),
        Err(CoreError::MissingCapability { capability }) if !cfg!(target_os = "macos") => {
            assert_eq!(capability, "a Metal device");
            None
        }
        Err(error) => panic!("this host should have Metal: {error}"),
    }
}

fn pipeline(owner: &DeviceOwner, name: &str) -> Pipeline {
    owner
        .library(KERNELS, MathMode::Safe)
        .expect("compiles")
        .pipeline(name)
        .expect("a pipeline")
}

fn refused<T: std::fmt::Debug>(result: Result<T, CoreError>, needle: &str) {
    match result {
        Err(CoreError::StateConflict { detail, .. }) => {
            assert!(detail.contains(needle), "{needle:?} not in {detail:?}");
        }
        other => panic!("expected a state conflict naming {needle:?}, got {other:?}"),
    }
}

fn values<T: Element>(owner: &DeviceOwner, buffer: Buffer<T>, count: usize) -> Vec<T> {
    let mut out = vec![T::default(); count];
    owner.read_back(buffer, &mut out).expect("reads back");
    out
}

fn epoch<T: Element>(owner: &DeviceOwner, buffer: Buffer<T>) -> u64 {
    owner.epoch(buffer).expect("a live buffer").get()
}

#[test]
fn typed_buffers_hold_their_elements_within_the_owners_budget() {
    let Some(owner) = owner(Limits {
        bytes: 64,
        leases_per_purpose: u32::MAX,
    }) else {
        return;
    };
    let floats = owner.upload(&[1.5_f32, -2.0, 3.25]).expect("uploads");
    let words = owner.zeroed::<u32>(4).expect("allocates");
    assert_eq!(values(&owner, floats, 3), [1.5, -2.0, 3.25]);
    assert_eq!(values(&owner, words, 4), [0; 4]);
    assert_eq!(owner.elements(floats), Ok(3));
    assert_eq!(owner.elements(words), Ok(4));
    assert_eq!(owner.state(floats), Ok(ResourceState::Committed));
    assert_eq!(owner.state(words), Ok(ResourceState::Available));
    assert_eq!((epoch(&owner, floats), epoch(&owner, words)), (1, 0));
    let usage = owner.usage();
    assert_eq!((usage.buffers, usage.bytes), (2, 28));

    // The budget: 36 bytes are left, and a refusal allocates nothing.
    assert_eq!(
        owner.zeroed::<f32>(10),
        Err(CoreError::BudgetExhausted {
            budget: "metal owner bytes",
            required: 40,
            available: 36,
        })
    );
    let fills = owner.zeroed::<u32>(9).expect("36 bytes fit");
    assert!(matches!(
        owner.upload(&[1_u32]),
        Err(CoreError::BudgetExhausted {
            required: 4,
            available: 0,
            ..
        })
    ));
    assert_eq!(owner.usage().bytes, 64);
    assert_eq!(owner.retire(fills), Ok(true), "unused, so freed at once");
    assert_eq!(owner.usage().bytes, 28);
    owner.upload(&[1_u32]).expect("fits again");

    // No elements; more than the device allocates.
    assert!(matches!(
        owner.zeroed::<f32>(0),
        Err(CoreError::LayoutMismatch { .. })
    ));
    let Some(unbounded) = self::owner(Limits::UNBOUNDED) else {
        return;
    };
    let past = unbounded.max_buffer_bytes() / 4 + 1;
    assert!(
        matches!(
            unbounded.zeroed::<f32>(past),
            Err(CoreError::AllocationFailed { .. })
        ),
        "the device refuses {past} f32s"
    );
    assert_eq!(unbounded.usage().buffers, 0, "nothing was allocated");
}

#[test]
fn a_buffer_has_many_readers_or_one_writer() {
    let Some(owner) = owner(Limits {
        bytes: u64::MAX,
        leases_per_purpose: 2,
    }) else {
        return;
    };
    let add = pipeline(&owner, "add_one");
    let grid = Grid::linear(1, 4);
    let source = owner.upload(&[1.0_f32, 2.0, 3.0, 4.0]).expect("uploads");
    let target = owner.zeroed::<f32>(4).expect("allocates");

    // Readers share.
    let rendering = owner.read(source, Purpose::Rendering).expect("a reader");
    let checkpointing = owner
        .read(source, Purpose::Checkpointing)
        .expect("another reader");
    assert_eq!(owner.state(source), Ok(ResourceState::Checkpointing));
    refused(owner.write(source), "unread and unwritten");
    refused(
        owner.submit(&add, &[target.reads(), source.writes()], grid),
        "unread and unwritten",
    );
    owner
        .submit(&add, &[source.reads(), target.writes()], grid)
        .expect("a command reads what leases read");
    assert_eq!(values(&owner, target, 4), [2.0, 3.0, 4.0, 5.0]);

    // Each purpose holds at most two leases.
    let again = owner.read(source, Purpose::Rendering).expect("a second");
    assert!(matches!(
        owner.read(source, Purpose::Rendering),
        Err(CoreError::BudgetExhausted {
            budget: "metal read leases",
            ..
        })
    ));
    let usage = owner.usage();
    assert_eq!(usage.read_leases, [2, 1, 0]);
    drop(again);
    let third = owner.read(source, Purpose::Rendering).expect("room again");
    assert_eq!(third.epoch(), ResourceEpoch::new(1));
    drop((rendering, checkpointing, third));
    assert_eq!(owner.state(source), Ok(ResourceState::Committed));

    // One writer, and nothing else.
    let mut writer = owner.write(source).expect("the writer");
    assert_eq!(owner.state(source), Ok(ResourceState::Computing));
    refused(owner.write(source), "unread and unwritten");
    refused(owner.read(source, Purpose::Rendering), "being written");
    refused(
        owner.submit(&add, &[source.reads(), target.writes()], grid),
        "being written",
    );
    writer.write(1, &[20.0, 30.0]).expect("writes");
    refused(writer.write(3, &[1.0, 2.0]), "write of bytes");
    drop(writer);
    assert_eq!(epoch(&owner, source), 2);
    let lease = owner.read(source, Purpose::Readback).expect("a reader");
    assert_eq!(lease.epoch(), ResourceEpoch::new(2));
    let mut seen = [0.0_f32; 4];
    lease.read(&mut seen).expect("reads");
    assert_eq!(seen, [1.0, 20.0, 30.0, 4.0]);
}

#[test]
fn a_command_binds_only_what_it_may_and_a_refused_one_changes_nothing() {
    let Some(owner) = owner(Limits::UNBOUNDED) else {
        return;
    };
    let (add, pair) = (pipeline(&owner, "add_one"), pipeline(&owner, "add_pair"));
    let grid = Grid::linear(1, 4);
    let a = owner.upload(&[1.0_f32, 2.0, 3.0, 4.0]).expect("uploads");
    let b = owner.zeroed::<f32>(4).expect("allocates");

    let receipt = owner
        .submit(&add, &[a.reads(), b.writes()], grid)
        .expect("runs");
    assert_eq!(receipt.written, [ResourceEpoch::new(1)]);
    assert_eq!(values(&owner, b, 4), [2.0, 3.0, 4.0, 5.0]);
    assert_eq!(epoch(&owner, a), 1, "a read buffer keeps its epoch");
    // One buffer read twice is no alias.
    owner
        .submit(&pair, &[a.reads(), a.reads(), b.writes()], grid)
        .expect("runs");
    assert_eq!(values(&owner, b, 4), [2.0, 4.0, 6.0, 8.0]);
    assert_eq!(epoch(&owner, b), 2);

    // Aliases are refused before anything is bound or run.
    refused(owner.submit(&add, &[a.reads(), a.writes()], grid), "alias");
    refused(
        owner.submit(&pair, &[a.reads(), b.writes(), b.writes()], grid),
        "alias",
    );
    refused(
        owner.submit(&pair, &[b.writes(), a.reads(), b.reads()], grid),
        "alias",
    );
    let other = self::owner(Limits::UNBOUNDED).expect("a second owner");
    let foreign = other.upload(&[0.0_f32; 4]).expect("uploads");
    refused(
        owner.submit(&add, &[foreign.reads(), b.writes()], grid),
        "belongs to owner",
    );
    assert_eq!((epoch(&owner, a), epoch(&owner, b)), (1, 2));
    assert_eq!(owner.usage().running, 0);
    assert_eq!(owner.state(b), Ok(ResourceState::Committed));

    // The gateway refuses a threadgroup above the pipeline's maximum before
    // encoding anything: the bindings are released and nothing changes.
    refused(
        owner.submit(&add, &[a.reads(), b.writes()], Grid::linear(1, 4096)),
        "exceeds the pipeline maximum",
    );
    assert_eq!(epoch(&owner, b), 2);
    assert_eq!(owner.state(b), Ok(ResourceState::Committed));
    assert_eq!(values(&owner, b, 4), [2.0, 4.0, 6.0, 8.0]);
    assert_eq!(owner.usage().running, 0);
    drop(owner.write(b).expect("released"));
}

#[test]
fn a_running_command_keeps_what_it_binds_from_every_other_thread() {
    let Some(owner) = owner(Limits::UNBOUNDED) else {
        return;
    };
    let spin = Arc::new(pipeline(&owner, "spin"));
    let steps = owner.upload(&[STEPS]).expect("uploads");
    let seed = owner.upload(&[41_u32]).expect("uploads");
    let out = owner.zeroed::<u32>(1).expect("allocates");
    let start = Arc::new(Barrier::new(2));
    let runner = {
        let (owner, spin, start) = (owner.clone(), Arc::clone(&spin), Arc::clone(&start));
        std::thread::spawn(move || {
            start.wait();
            owner.submit(
                &spin,
                &[steps.reads(), seed.reads(), out.writes()],
                Grid::linear(1, 1),
            )
        })
    };
    start.wait();
    let deadline = Instant::now() + Duration::from_secs(30);
    while owner.state(out) != Ok(ResourceState::Computing) {
        assert!(
            !runner.is_finished(),
            "the command ended before it was seen running: raise STEPS"
        );
        assert!(Instant::now() < deadline, "the command never started");
        std::thread::yield_now();
    }

    // While it runs, on this thread:
    assert_eq!(owner.usage().running, 1);
    refused(owner.read(out, Purpose::Readback), "being written");
    refused(owner.write(steps), "unread and unwritten");
    refused(owner.write(seed), "unread and unwritten");
    refused(
        owner.submit(
            &spin,
            &[steps.reads(), seed.reads(), out.writes()],
            Grid::linear(1, 1),
        ),
        "unread and unwritten",
    );
    drop(
        owner
            .read(seed, Purpose::Rendering)
            .expect("readers share a buffer with a running command"),
    );
    assert_eq!(
        owner.retire(seed),
        Ok(false),
        "a buffer a running command reads is not freed"
    );
    assert_eq!(owner.state(seed), Ok(ResourceState::Retiring));
    assert_eq!(
        owner.state(out),
        Ok(ResourceState::Computing),
        "every check above ran while the command did"
    );

    let receipt = runner
        .join()
        .expect("the runner joins")
        .expect("the command completes");
    assert_eq!(receipt.written, [ResourceEpoch::new(1)]);
    refused(owner.state(seed), "has been freed");
    let mut x = 42_u32;
    for _ in 0..STEPS {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
    }
    assert_eq!(values(&owner, out, 1), [x]);
    assert_eq!(owner.usage().running, 0);
}

#[test]
fn retired_buffers_free_when_released_and_their_handles_stay_stale() {
    let Some(owner) = owner(Limits::UNBOUNDED) else {
        return;
    };
    let add = pipeline(&owner, "add_one");
    let x = owner.upload(&[7.0_f32; 4]).expect("uploads");
    let y = owner.zeroed::<f32>(4).expect("allocates");
    assert!(format!("{x:?}").contains("#0 gen 0"), "{x:?}");
    let lease = owner
        .read(x, Purpose::Checkpointing)
        .expect("a checkpoint reads it");
    let before = owner.usage();
    assert_eq!(owner.retire(x), Ok(false));
    assert_eq!(owner.state(x), Ok(ResourceState::Retiring));
    refused(owner.read(x, Purpose::Rendering), "retiring");
    refused(owner.write(x), "retiring");
    refused(
        owner.submit(&add, &[x.reads(), y.writes()], Grid::linear(1, 4)),
        "retiring",
    );
    assert_eq!(owner.retire(x), Ok(false), "retiring again changes nothing");
    let usage = owner.usage();
    assert_eq!(
        (usage.buffers, usage.retiring, usage.bytes),
        (before.buffers, 1, before.bytes),
        "a retiring buffer still counts"
    );
    let mut seen = [0.0_f32; 4];
    lease.read(&mut seen).expect("the lease still reads");
    assert_eq!(seen, [7.0; 4]);
    drop(lease);
    refused(owner.state(x), "has been freed");
    assert_eq!(owner.usage().bytes, before.bytes - 16);

    // Its slot is reused under a new generation, and the old handle stays
    // stale.
    let z = owner.upload(&[8.0_f32; 4]).expect("uploads");
    assert!(format!("{z:?}").contains("#0 gen 1"), "{z:?}");
    refused(owner.state(x), "has been freed");
    refused(owner.read(x, Purpose::Readback), "has been freed");
    refused(owner.retire(x), "has been freed");
    assert_eq!(values(&owner, z, 4), [8.0; 4]);

    // An owned buffer is retired when dropped.
    {
        let _scratch = owner.zeroed_owned::<u32>(8).expect("allocates");
        assert_eq!(owner.usage().buffers, 3);
    }
    assert_eq!(owner.usage().buffers, 2);

    // Another owner's handles are refused; a lease outlives the owner
    // handle that issued it.
    let other = self::owner(Limits::UNBOUNDED).expect("a second owner");
    refused(other.state(z), "belongs to owner");
    let kept = other
        .read(other.upload(&[3_u32]).expect("uploads"), Purpose::Rendering)
        .expect("a reader");
    drop(other);
    let mut word = [0_u32];
    kept.read(&mut word).expect("still reads");
    assert_eq!(word, [3]);
    drop(kept);

    // A consumer that unwinds releases its lease, committing what it wrote.
    let w = owner.upload(&[1.0_f32]).expect("uploads");
    let exited = std::thread::spawn({
        let owner = owner.clone();
        move || {
            let mut writer = owner.write(w).expect("the writer");
            writer.write(0, &[2.0]).expect("writes");
            panic!("the consumer exits while holding its lease");
        }
    })
    .join();
    assert!(exited.is_err());
    assert_eq!(owner.usage().write_leases, 0);
    assert_eq!(owner.state(w), Ok(ResourceState::Committed));
    assert_eq!(epoch(&owner, w), 2);
    assert_eq!(values(&owner, w, 1), [2.0]);
}
