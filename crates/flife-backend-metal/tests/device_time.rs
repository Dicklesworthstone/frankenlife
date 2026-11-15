//! FL-029: a device owner sums the device time of every command it
//! completes ([`DeviceOwner::device_time`]), whichever clone submitted it,
//! so every Metal stage on one owner is timed together. A refused command
//! adds nothing; `batch.rs` checks a failed one. Its own binary, away from
//! `batch.rs`'s injected command-buffer fault.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test: a failure is a test failure"
)]

use std::time::{Duration, Instant};

use flife_backend_metal::{DeviceOwner, DeviceTime, Kernel, Limits};
use flife_core::CoreError;
use ft_kernel_metal::compute::{Grid, MathMode};

const KERNELS: &str = r"
#include <metal_stdlib>
using namespace metal;

kernel void add_one(device const float *a [[buffer(0)]],
                    device float *b [[buffer(1)]],
                    uint id [[thread_position_in_grid]]) {
    b[id] = a[id] + 1.0f;
}
";

const N: usize = 1 << 16;

#[test]
fn an_owner_sums_its_commands_device_time_across_its_clones() {
    let owner = match DeviceOwner::open(Limits::UNBOUNDED) {
        Ok(owner) => owner,
        Err(CoreError::MissingCapability { .. }) if !cfg!(target_os = "macos") => {
            println!("no Metal device: skipped");
            return;
        }
        Err(error) => panic!("this host should have Metal: {error}"),
    };
    let add_one = owner
        .library(KERNELS, MathMode::Safe)
        .expect("compiles")
        .pipeline("add_one")
        .expect("add_one");
    let grid = Grid::linear(N / 256, 256);
    let a = owner.upload(&vec![1.0_f32; N]).expect("a");
    let b = owner.zeroed::<f32>(N).expect("b");
    let c = owner.zeroed::<f32>(N).expect("c");
    assert_eq!(
        owner.device_time(),
        DeviceTime::default(),
        "compiling and uploading run no command"
    );

    // Eight commands, through the owner and a clone, one and two kernels.
    let clone = owner.clone();
    let mut seconds = 0.0;
    let start = Instant::now();
    for round in 0..8 {
        let by = if round % 2 == 0 { &owner } else { &clone };
        let receipt = if round < 4 {
            by.submit(&add_one, &[a.reads(), b.writes()], grid)
        } else {
            by.submit_batch(&[
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
            ])
        }
        .expect("runs");
        assert!(receipt.device_seconds > 0.0);
        seconds += receipt.device_seconds;
    }
    let wall = start.elapsed();
    let time = owner.device_time();
    let device = Duration::from_nanos(time.nanoseconds);
    let receipts = Duration::from_secs_f64(seconds);
    println!("8 commands: device {device:?}, receipts {receipts:?}, wall {wall:?}");
    assert_eq!(time.commands, 8);
    assert_eq!(clone.device_time(), time, "clones share one account");
    assert!(
        device.abs_diff(receipts) <= Duration::from_nanos(8 + 1),
        "{device:?} is not the receipts' {receipts:?}"
    );
    assert!(device <= wall, "device {device:?} > wall {wall:?}");

    // A refused command runs nothing and adds nothing.
    assert!(matches!(
        owner.submit(&add_one, &[b.reads(), b.writes()], grid),
        Err(CoreError::StateConflict { .. })
    ));
    assert_eq!(owner.device_time(), time);

    // Another owner keeps its own account.
    let other = DeviceOwner::open(Limits::UNBOUNDED).expect("another owner");
    assert_eq!(other.device_time(), DeviceTime::default());
}
