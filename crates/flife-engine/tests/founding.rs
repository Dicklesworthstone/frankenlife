//! FL-034: founding on several workers. A hundred thousand founders take
//! seconds on one thread, so `World::found_on` makes them on the threads it
//! is given. The world it founds is the serial founding's bit for bit, its
//! checkpoint (genomes included) the same bytes, and it then steps on the
//! count it was given.

#![allow(
    clippy::expect_used,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::num::NonZeroUsize;

use flife_core::ValidatedConfig;
use flife_core::checkpoint::CheckpointWriter;
use flife_engine::{Arrangement, NumericalProfile, World};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// Founders enough for nine runs of the reference adapter's 128-row minimum,
/// the last one short.
const FOUNDERS: usize = 1100;

/// The real preset with [`FOUNDERS`] founders, on its one configured worker.
fn config() -> ValidatedConfig {
    let toml = PRESET.replace(
        "initial_count = 4096",
        &format!("initial_count = {FOUNDERS}"),
    );
    assert!(toml.contains(&format!("initial_count = {FOUNDERS}")));
    assert!(toml.contains("cpu_workers = 1\n"));
    ValidatedConfig::from_toml(&toml).expect("the preset validates")
}

fn checkpoint(world: &World) -> Vec<u8> {
    let mut writer = CheckpointWriter::new(&world.config().config().model);
    world.write_checkpoint(&mut writer).expect("checkpoints");
    writer.finish().expect("seals")
}

/// Founded on 2, 5, 9 or 64 workers (64 more than the founders' runs), the
/// world is the one-worker founding's: the same digest and the same
/// checkpoint bytes, every genome's parameters and traits among them, and it
/// keeps the worker count it was given.
#[test]
fn a_world_founded_on_several_workers_is_the_serial_world_bit_for_bit() {
    let config = config();
    let profile = || NumericalProfile::reference_f32().expect("profile");
    let serial = World::found(&config, Arrangement::FreeList, profile()).expect("founds");
    assert_eq!(serial.cpu_workers(), 1);
    assert_eq!(serial.population().len(), FOUNDERS);
    let expected = checkpoint(&serial);
    for workers in [2, 5, 9, 64] {
        let count = NonZeroUsize::new(workers).expect("nonzero");
        let parallel =
            World::found_on(&config, Arrangement::FreeList, profile(), count).expect("founds");
        assert_eq!(parallel.cpu_workers(), workers);
        assert_eq!(parallel.population().len(), FOUNDERS);
        assert_eq!(parallel.digest(), serial.digest(), "{workers} workers");
        assert!(
            checkpoint(&parallel) == expected,
            "{workers} workers founded another checkpoint"
        );
    }
}
