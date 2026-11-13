//! FL-030 (plan §10.3, §19.5): the compact-F16 numerical profile. A world
//! whose `controller.canonical_weight_dtype` is `f16` stores every genome's
//! parameters as binary16, each rounded once from its F64 value when it is
//! drawn or mutated, and evaluates them widened exactly to F32 with F32
//! accumulation. It is a distinct evolutionary model ("F16 with declared
//! rounding"): deterministic, resumable bit for bit, and not the F32 world.

use std::num::NonZeroUsize;

use flife_core::binary16::F16;
use flife_core::checkpoint::{CheckpointReader, CheckpointWriter};
use flife_core::{Sha256Digest, Sha256Stream, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, WORLD_CHECKPOINT_CHUNKS, World};
use flife_genome::{WeightDtype, Weights};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// The real preset with 512 founders in weight dtype `dtype`.
#[expect(
    clippy::expect_used,
    reason = "test helper: a failure is a test failure"
)]
fn config(dtype: &str) -> ValidatedConfig {
    let toml = PRESET
        .replace("initial_count = 4096", "initial_count = 512")
        .replace(
            "canonical_weight_dtype = \"f32\"",
            &format!("canonical_weight_dtype = \"{dtype}\""),
        );
    assert!(toml.contains(&format!("canonical_weight_dtype = \"{dtype}\"")));
    ValidatedConfig::from_toml(&toml).expect("the preset validates")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failure is a test failure"
)]
fn found(config: &ValidatedConfig) -> World {
    World::found(
        config,
        Arrangement::FreeList,
        NumericalProfile::reference_f32().expect("profile"),
    )
    .expect("the world founds")
}

/// Publish one tick, returning its births.
#[expect(clippy::expect_used, clippy::panic, reason = "test helper")]
fn publish(world: &mut World) -> usize {
    match world.step().expect("the tick advances") {
        Stepped::Published(report) => report.births.len(),
        Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
    }
}

/// Every live genome's identity and parameter bits, in identity order,
/// with the count of genomes not stored in `dtype` and the logical bytes
/// their parameters take at that dtype.
#[expect(clippy::expect_used, reason = "test helper")]
fn genomes(world: &World, dtype: WeightDtype) -> (Sha256Digest, usize, u64) {
    let mut stream = Sha256Stream::new();
    let (mut other, mut bytes) = (0, 0_u64);
    for organism in world.population().iter() {
        let genome = world.population().genome(organism.id).expect("its genome");
        let parameters = genome.parameters();
        other += usize::from(parameters.dtype() != dtype);
        bytes += parameters.len() as u64 * dtype.bytes();
        let mut row = genome.id().get().to_le_bytes().to_vec();
        row.extend(parameters.bits().flat_map(u32::to_le_bytes));
        stream.update(&row);
    }
    (stream.finish(), other, bytes)
}

#[expect(clippy::expect_used, reason = "test helper")]
fn resumed(world: &World) -> World {
    let mut writer = CheckpointWriter::new(&world.config().config().model);
    world.write_checkpoint(&mut writer).expect("checkpoints");
    let bytes = writer.finish().expect("seals");
    let reader = CheckpointReader::parse(&bytes, &WORLD_CHECKPOINT_CHUNKS).expect("parses");
    World::read_checkpoint(&reader, NumericalProfile::reference_f32().expect("profile"))
        .expect("resumes")
}

/// The registry's evidence for `f16`: two F16 worlds of one seed, one on a
/// single worker and one on four, publish the same state at every tick; a
/// world resumed from a mid-run checkpoint follows its original bit for
/// bit, genomes included; every genome is binary16; the quota charges two
/// bytes a parameter; and births (so mutation) happen.
#[test]
fn an_f16_world_is_deterministic_and_resumes_bit_for_bit() {
    let config = config("f16");
    let mut serial = found(&config);
    let mut parallel = found(&config);
    parallel
        .set_cpu_workers(NonZeroUsize::new(4).expect("nonzero"))
        .expect("workers");
    let (half, ticks) = (150, 400);
    for _ in 0..half {
        publish(&mut serial);
        publish(&mut parallel);
        assert_eq!(
            serial.digest(),
            parallel.digest(),
            "at {}",
            serial.tick().get()
        );
    }
    let mut resumed = resumed(&serial);
    assert_eq!(resumed.digest(), serial.digest());
    assert_eq!(
        genomes(&resumed, WeightDtype::F16),
        genomes(&serial, WeightDtype::F16)
    );
    // The births after the checkpoint: the resumed world mutates binary16
    // children of its own.
    let mut births = 0;
    for _ in half..ticks {
        births += publish(&mut serial);
        publish(&mut parallel);
        publish(&mut resumed);
        assert_eq!(
            serial.digest(),
            parallel.digest(),
            "at {}",
            serial.tick().get()
        );
        assert_eq!(
            serial.digest(),
            resumed.digest(),
            "at {}",
            serial.tick().get()
        );
    }
    let (digest, other, bytes) = genomes(&serial, WeightDtype::F16);
    assert_eq!(genomes(&parallel, WeightDtype::F16).0, digest);
    assert_eq!(genomes(&resumed, WeightDtype::F16).0, digest);
    assert_eq!(other, 0, "every genome is binary16");
    assert_eq!(serial.quota().dtype(), WeightDtype::F16);
    assert_eq!(
        serial.quota().logical_bytes(),
        bytes,
        "two bytes a parameter"
    );
    println!("{births} births after tick {half}");
    assert!(
        births > 0,
        "children are born after resuming, so mutation runs"
    );

    // The weight dump names the dtype and gives each binary16 widened
    // exactly; admission counts the logical quota in binary16 parameters.
    let organism = serial.population().ids().last().expect("a live organism");
    let dump = serial
        .organism_weights(organism)
        .expect("dumps")
        .expect("it lives");
    let stored = serial
        .population()
        .genome(organism)
        .expect("its genome")
        .parameters()
        .as_f16()
        .expect("binary16");
    assert_eq!(dump.dtype, WeightDtype::F16);
    assert!(
        dump.parameters
            .iter()
            .zip(stored)
            .all(|(wide, half)| wide.to_bits() == half.to_f32().to_bits())
    );
    assert_eq!(dump.parameters.len(), stored.len());
    let admission = serial.admit_memory(false, 0, 0).expect("admits");
    assert_eq!(
        admission.logical_parameters,
        config.config().execution.maximum_logical_genome_bytes / 2
    );
}

/// An F16 founder is the F32 founder's own F64 draw rounded once to
/// binary16: wherever it is not the binary16 nearest the F32 value, that
/// value lies within one F32 ulp of the midpoint between the two (the draw
/// rounded one way to F32 and the other to binary16). Biases are zero in
/// both. And F16 is a distinct model: its world leaves the F32 world.
#[test]
fn f16_founders_are_their_draws_rounded_once_and_the_world_is_its_own() {
    let mut compact = found(&config("f16"));
    let mut reference = found(&config("f32"));
    let (mut compared, mut nearest, mut double) = (0_u64, 0_u64, 0_u64);
    for organism in reference.population().iter() {
        let wide = reference
            .population()
            .genome(organism.id)
            .expect("its genome")
            .parameters();
        let half = compact
            .population()
            .genome(organism.id)
            .expect("the same founder")
            .parameters();
        let (Weights::F32(wide), Weights::F16(half)) = (wide, half) else {
            unreachable!("the configured dtypes");
        };
        assert_eq!(wide.len(), half.len());
        for (&single, &chosen) in wide.iter().zip(half) {
            compared += 1;
            let closest = F16::from_f64(f64::from(single));
            if chosen == closest {
                nearest += 1;
                continue;
            }
            double += 1;
            let midpoint = f64::midpoint(chosen.to_f64(), closest.to_f64());
            let ulp = f64::from(single.abs().next_up()) - f64::from(single.abs());
            assert!(
                (f64::from(single) - midpoint).abs() <= ulp,
                "{single:e} became {:e}, not {:e}",
                chosen.to_f64(),
                closest.to_f64()
            );
        }
    }
    println!("{compared} parameters: {nearest} nearest, {double} at a midpoint");
    assert!(compared > 1_000_000);
    assert_eq!(compared, nearest + double);
    // Rounding the F32 founder again would make every F16 founder the
    // nearest: the midpoint cases are what rounding once leaves behind.
    assert!(double > 0, "an F16 founder rounded twice, through F32");

    let mut left = None;
    for _ in 0..40 {
        publish(&mut compact);
        publish(&mut reference);
        if left.is_none() && compact.digest() != reference.digest() {
            left = Some(compact.tick());
        }
    }
    assert!(left.is_some(), "the F16 world is not the F32 world");
}
