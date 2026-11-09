//! FL-029 evidence (plan §10.4, §22.4): several ordered kernels submitted
//! through the Metal owner as one command. It covers each kernel reading
//! what earlier ones wrote, bindings checked as one command before anything
//! runs, a kernel the gateway refuses part-way through a batch, and a
//! command buffer that fails, through the gateway's own fault seam.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use flife_backend_metal::{
    Buffer, DeviceOwner, Element, Kernel, Limits, Purpose, ResourceState, Usage,
};
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
";

const N: usize = 4096;

/// The owner, or `None` off macOS after checking its typed refusal.
fn owner() -> Option<DeviceOwner> {
    match DeviceOwner::open(Limits::UNBOUNDED) {
        Ok(owner) => Some(owner),
        Err(CoreError::MissingCapability { capability }) if !cfg!(target_os = "macos") => {
            assert_eq!(capability, "a Metal device");
            None
        }
        Err(error) => panic!("this host should have Metal: {error}"),
    }
}

fn pipelines(owner: &DeviceOwner) -> (Pipeline, Pipeline) {
    let library = owner.library(KERNELS, MathMode::Safe).expect("compiles");
    (
        library.pipeline("add_one").expect("add_one"),
        library.pipeline("add_pair").expect("add_pair"),
    )
}

fn refused<T: std::fmt::Debug>(result: Result<T, CoreError>, needle: &str) {
    match result {
        Err(CoreError::StateConflict { detail, .. }) => {
            assert!(detail.contains(needle), "{needle:?} not in {detail:?}");
        }
        other => panic!("expected a state conflict naming {needle:?}, got {other:?}"),
    }
}

fn values<T: Element>(owner: &DeviceOwner, buffer: Buffer<T>) -> Vec<T> {
    let mut out = vec![T::default(); N];
    owner.read_back(buffer, &mut out).expect("reads back");
    out
}

/// Each buffer's state and epoch.
fn account(owner: &DeviceOwner, buffers: &[Buffer<f32>]) -> Vec<(ResourceState, u64)> {
    buffers
        .iter()
        .map(|&buffer| {
            (
                owner.state(buffer).expect("live"),
                owner.epoch(buffer).expect("live").get(),
            )
        })
        .collect()
}

fn idle(owner: &DeviceOwner) {
    let Usage {
        read_leases,
        write_leases,
        running,
        ..
    } = owner.usage();
    assert_eq!((read_leases, write_leases, running), ([0; 3], 0, 0));
}

fn start() -> Vec<f32> {
    (0..N).map(|i| (i % 1000) as f32).collect()
}

/// `b = a + 1`, `c = b + 1`, then `a = b + c` in one command: every kernel
/// sees its predecessors' writes, the receipt counts three kernels, and each
/// written buffer advances one epoch, even `a`, which the batch reads first.
#[test]
fn a_batch_runs_its_kernels_in_order_as_one_command() {
    let Some(owner) = owner() else { return };
    let (add_one, add_pair) = pipelines(&owner);
    let grid = Grid::linear(N / 256, 256);
    let a = owner.upload(&start()).expect("a");
    let b = owner.zeroed::<f32>(N).expect("b");
    let c = owner.zeroed::<f32>(N).expect("c");

    let receipt = owner
        .submit_batch(&[
            Kernel {
                pipeline: &add_one,
                bindings: &[a.reads(), b.writes()],
                grid,
            },
            Kernel {
                pipeline: &add_one,
                bindings: &[b.reads(), c.writes()],
                grid,
            },
            Kernel {
                pipeline: &add_pair,
                bindings: &[b.reads(), c.reads(), a.writes()],
                grid,
            },
        ])
        .expect("runs");
    assert_eq!(receipt.kernels, 3);
    assert_eq!(
        receipt.written,
        [2, 1, 1].map(ResourceEpoch::new),
        "a, b and c in order of first binding, one epoch each"
    );
    assert!(receipt.device_seconds > 0.0);

    let x = start();
    assert_eq!(
        values(&owner, b),
        x.iter().map(|v| v + 1.0).collect::<Vec<_>>()
    );
    assert_eq!(
        values(&owner, c),
        x.iter().map(|v| v + 2.0).collect::<Vec<_>>()
    );
    assert_eq!(
        values(&owner, a),
        x.iter().map(|v| 2.0 * v + 3.0).collect::<Vec<_>>()
    );
    assert_eq!(
        account(&owner, &[a, b, c]),
        [
            (ResourceState::Committed, 2),
            (ResourceState::Committed, 1),
            (ResourceState::Committed, 1)
        ]
    );
    idle(&owner);
}

/// Asserts that a refused batch changed nothing: every buffer keeps its
/// state and epoch, `b` keeps its values, and nothing is held or running.
fn untouched(owner: &DeviceOwner, buffers: [Buffer<f32>; 3], before: &[(ResourceState, u64)]) {
    assert_eq!(account(owner, &buffers), before);
    assert_eq!(values(owner, buffers[1]), vec![0.0; N], "a kernel ran");
    let Usage {
        write_leases,
        running,
        ..
    } = owner.usage();
    assert_eq!((write_leases, running), (0, 0));
}

/// A batch's bindings are checked as one command before any kernel runs: a
/// conflict in its last kernel refuses the first, and so does a kernel the
/// gateway refuses after earlier kernels were accepted.
#[test]
fn a_batch_is_checked_as_one_command_and_a_refused_one_runs_nothing() {
    let Some(owner) = owner() else { return };
    let (add_one, add_pair) = pipelines(&owner);
    let grid = Grid::linear(N / 256, 256);
    let a = owner.upload(&start()).expect("a");
    let b = owner.zeroed::<f32>(N).expect("b");
    let c = owner.upload(&vec![7.0; N]).expect("c");
    let all = [a, b, c];
    let first = Kernel {
        pipeline: &add_one,
        bindings: &[a.reads(), b.writes()],
        grid,
    };
    let before = account(&owner, &all);

    refused(owner.submit_batch(&[]), "a command of no kernels");
    untouched(&owner, all, &before);

    // An alias in the second kernel refuses the first.
    refused(
        owner.submit_batch(&[
            first,
            Kernel {
                pipeline: &add_pair,
                bindings: &[c.reads(), b.reads(), c.writes()],
                grid,
            },
        ]),
        "bound twice and written: an alias",
    );
    untouched(&owner, all, &before);

    // The last kernel writes a buffer a renderer holds.
    let lease = owner.read(c, Purpose::Rendering).expect("a lease");
    let leased = account(&owner, &all);
    refused(
        owner.submit_batch(&[
            first,
            Kernel {
                pipeline: &add_one,
                bindings: &[b.reads(), c.writes()],
                grid,
            },
        ]),
        "a command writes it only unread and unwritten",
    );
    untouched(&owner, all, &leased);
    // Reading it, twice in one kernel, beside the renderer is compatible.
    owner
        .submit_batch(&[Kernel {
            pipeline: &add_pair,
            bindings: &[c.reads(), c.reads(), b.writes()],
            grid,
        }])
        .expect("two reads beside a renderer");
    drop(lease);
    assert_eq!(values(&owner, b), vec![14.0; N]);
    owner
        .write(b)
        .expect("a writer")
        .write(0, &[0.0; N])
        .expect("reset b");

    // The last kernel reads a buffer the host is writing.
    let writer = owner.write(c).expect("a writer");
    let before = account(&owner, &all);
    refused(
        owner.submit_batch(&[
            first,
            Kernel {
                pipeline: &add_one,
                bindings: &[c.reads(), b.writes()],
                grid,
            },
        ]),
        "is being written",
    );
    assert_eq!(account(&owner, &all), before);
    assert_eq!(owner.usage().running, 0);
    drop(writer);

    // The gateway refuses the second kernel after accepting the first:
    // nothing was submitted, so the first kernel's output is untouched.
    let before = account(&owner, &all);
    let too_wide = add_one.max_threads_per_threadgroup() + 1;
    match owner.submit_batch(&[
        first,
        Kernel {
            pipeline: &add_one,
            bindings: &[b.reads(), c.writes()],
            grid: Grid::linear(1, too_wide),
        },
    ]) {
        Err(CoreError::StateConflict { operation, detail }) => {
            assert_eq!(operation, "metal dispatch");
            assert!(detail.contains("exceeds the pipeline maximum"), "{detail}");
        }
        other => panic!("expected the gateway's refusal, got {other:?}"),
    }
    untouched(&owner, all, &before);
    idle(&owner);
}

/// A command buffer that ends without completing, through the gateway's
/// fault seam (the next command buffer is left uncommitted and reports
/// `NotEnqueued`, as one a GPU restart aborted does): every buffer the batch
/// writes becomes undefined at its next epoch and refuses reads; what it only
/// read is unchanged. A later batch may rewrite an undefined buffer and read
/// it in a following kernel, but not read it first.
///
/// The seam exists only in builds with debug assertions (`cargo test` without
/// `--release`); a release build checks that it is absent and stops there.
#[test]
fn a_failed_command_buffer_leaves_what_the_batch_writes_undefined() {
    let Some(owner) = owner() else { return };
    let (add_one, _) = pipelines(&owner);
    let grid = Grid::linear(N / 256, 256);
    let a = owner.upload(&start()).expect("a");
    let b = owner.zeroed::<f32>(N).expect("b");
    let c = owner.zeroed::<f32>(N).expect("c");
    let chain = [
        Kernel {
            pipeline: &add_one,
            bindings: &[a.reads(), b.writes()],
            grid,
        },
        Kernel {
            pipeline: &add_one,
            bindings: &[b.reads(), c.writes()],
            grid,
        },
    ];

    let armed = ft_kernel_metal::inject_command_buffer_faults(1);
    assert_eq!(
        armed,
        cfg!(debug_assertions),
        "the gateway's fault seam exists exactly when debug assertions do"
    );
    if !armed {
        return;
    }
    match owner.submit_batch(&chain) {
        Err(CoreError::StateConflict { operation, detail }) => {
            assert_eq!(operation, "metal dispatch");
            assert!(detail.contains("NotEnqueued"), "{detail}");
        }
        other => panic!("expected the failed command buffer, got {other:?}"),
    }
    assert_eq!(
        account(&owner, &[a, b, c]),
        [
            (ResourceState::Committed, 1),
            (ResourceState::Available, 1),
            (ResourceState::Available, 1)
        ]
    );
    refused(owner.read(b, Purpose::Readback), "is undefined");
    refused(owner.read(c, Purpose::Rendering), "is undefined");
    assert_eq!(values(&owner, a), start(), "a read-only input changed");
    idle(&owner);

    // Reading an undefined buffer first is refused, and changes nothing.
    refused(
        owner.submit_batch(&[Kernel {
            pipeline: &add_one,
            bindings: &[c.reads(), a.writes()],
            grid,
        }]),
        "is undefined",
    );
    // Rewriting it in one kernel and reading it in the next is admitted.
    let receipt = owner.submit_batch(&chain).expect("rewrites b, then c");
    assert_eq!(receipt.written, [2, 2].map(ResourceEpoch::new));
    let x = start();
    assert_eq!(
        values(&owner, c),
        x.iter().map(|v| v + 2.0).collect::<Vec<_>>()
    );
    idle(&owner);
}
