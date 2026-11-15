//! FL-043: a world's restore streams its genome chunk. Read a window at a
//! time from its source and verified at its end, it restores the world the
//! whole container restores, holding the rest of the container and one
//! window beside it, not the chunk. A restore is admitted as a phase of its
//! own: a world restored from a whole container is refused when that phase
//! is over the budget, and the same checkpoint, streamed, is admitted.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::io::{Cursor, Read};

use flife_core::checkpoint::{
    CHECKPOINT_PAGE_BYTES, CheckpointManifest, CheckpointReader, CheckpointWriter, ChunkFamily,
};
use flife_core::{CoreError, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, WORLD_CHECKPOINT_CHUNKS, World};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

/// The preset's budget, `maximum_total_bytes`.
const BUDGET: u64 = 2_147_483_648;

/// The real preset with `founders` founders in weight dtype `dtype`, under
/// a budget of `budget` bytes.
fn config(founders: u32, dtype: &str, budget: u64) -> ValidatedConfig {
    let toml = PRESET
        .replace(
            "initial_count = 4096",
            &format!("initial_count = {founders}"),
        )
        .replace(
            "canonical_weight_dtype = \"f32\"",
            &format!("canonical_weight_dtype = \"{dtype}\""),
        )
        .replace(
            &format!("maximum_total_bytes = {BUDGET}"),
            &format!("maximum_total_bytes = {budget}"),
        );
    assert!(toml.contains(&format!("initial_count = {founders}")));
    assert!(toml.contains(&format!("canonical_weight_dtype = \"{dtype}\"")));
    assert!(toml.contains(&format!("maximum_total_bytes = {budget}")));
    ValidatedConfig::from_toml(&toml).expect("the preset validates")
}

fn profile() -> NumericalProfile {
    NumericalProfile::reference_f32().expect("profile")
}

/// The world `config` founds, two ticks on, and its checkpoint.
fn checkpointed(config: &ValidatedConfig) -> (World, Vec<u8>) {
    let mut world = World::found(config, Arrangement::FreeList, profile()).expect("founds");
    for _ in 0..2 {
        if let Stepped::Paused(shortfall) = world.step().expect("steps") {
            panic!("paused: {shortfall:?}");
        }
    }
    let mut writer = CheckpointWriter::new(&config.config().model);
    world.write_checkpoint(&mut writer).expect("checkpoints");
    (world, writer.finish().expect("seals"))
}

/// The world `bytes` hold, restored from the whole container.
fn whole(bytes: &[u8]) -> World {
    let reader = CheckpointReader::parse(bytes, &WORLD_CHECKPOINT_CHUNKS).expect("parses");
    World::read_checkpoint(&reader, profile()).expect("restores")
}

/// `bytes` without their genome chunk, and that chunk.
fn split(bytes: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let manifest = CheckpointManifest::parse(bytes).expect("a manifest");
    let payload = &bytes[manifest.sealed_length()..];
    let mut held = bytes[..manifest.sealed_length()].to_vec();
    let mut genomes = Vec::new();
    for entry in manifest.entries() {
        let start = usize::try_from(entry.offset).expect("small");
        let end = start + usize::try_from(entry.length).expect("small");
        if entry.family == ChunkFamily::Genomes {
            genomes.extend_from_slice(&payload[start..end]);
        } else {
            held.extend_from_slice(&payload[start..end]);
        }
    }
    (held, genomes)
}

/// The world restored from `held` and the genome chunk `genomes`, streamed.
fn streamed(held: &[u8], genomes: Vec<u8>) -> Result<World, CoreError> {
    streamed_from(held, Box::new(Cursor::new(genomes)))
}

fn streamed_from(held: &[u8], source: Box<dyn Read>) -> Result<World, CoreError> {
    let reader = CheckpointReader::parse_streamed(
        held,
        &WORLD_CHECKPOINT_CHUNKS,
        ChunkFamily::Genomes,
        Some(source),
    )?;
    World::read_checkpoint(&reader, profile())
}

/// A source that gives at most 97 bytes a read, so the window a restore
/// fills ends wherever a read does, not where a page does: a genome's fixed
/// part or parameters can begin anywhere in it.
struct Short(Cursor<Vec<u8>>);

impl Read for Short {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let length = out.len().min(97);
        self.0.read(&mut out[..length])
    }
}

/// In each weight dtype, the preset with 512 founders, two ticks on: the
/// world restored with its genome chunk streamed has the digest of the
/// world restored from the whole container, which is the original's, from
/// a source that reads it whole or 97 bytes at a time. The whole restore
/// held the container; the streamed one held the rest of it and one
/// page-sized window (each genome's record is smaller than a page), though
/// the chunk spans several. A changed parameter byte decodes, and the
/// restore is refused when the stream ends, by the chunk's hash.
#[test]
fn a_streamed_restore_is_the_whole_one_and_holds_one_window() {
    for dtype in ["f32", "f16"] {
        let (world, bytes) = checkpointed(&config(512, dtype, BUDGET));
        let restored = whole(&bytes);
        assert_eq!(restored.digest(), world.digest(), "{dtype}");
        assert_eq!(restored.restored_bytes(), bytes.len() as u64, "{dtype}");

        let (held, genomes) = split(&bytes);
        println!(
            "{dtype}: a container of {} bytes, its genome chunk {}",
            bytes.len(),
            genomes.len()
        );
        assert!(
            genomes.len() > 2 * CHECKPOINT_PAGE_BYTES,
            "{dtype}: the chunk spans several windows"
        );
        for restored in [
            streamed(&held, genomes.clone()).expect("restores streamed"),
            streamed_from(&held, Box::new(Short(Cursor::new(genomes.clone()))))
                .expect("restores from short reads"),
        ] {
            assert_eq!(restored.digest(), world.digest(), "{dtype}");
            assert_eq!(
                restored.restored_bytes(),
                (held.len() + CHECKPOINT_PAGE_BYTES) as u64,
                "{dtype}: the held bytes and one window"
            );
        }

        let mut tampered = genomes;
        let last = tampered.len() - 4;
        tampered[last] ^= 1;
        match streamed(&held, tampered) {
            Err(CoreError::IncompatibleCheckpoint { reason }) => assert!(
                reason.contains("chunk genomes hash mismatch"),
                "{dtype}: {reason}"
            ),
            Err(other) => panic!("{dtype}: another error: {other}"),
            Ok(_) => panic!("{dtype}: a changed genome chunk restored"),
        }
    }
}

/// The preset, two ticks on, restored from its whole container: the
/// restore's phase (the genome quota, state, field, scratch, the container
/// and the reserve) is larger than the run's, and is the admission's peak.
/// At a budget one byte under it, the world is refused, by that phase's
/// figures; at exactly it, admitted. The same checkpoint at that short
/// budget, its genome chunk streamed, is admitted: its restore held the rest
/// of the container and one window, and its peak is the run's phase.
/// Every budget has as many digits as the preset's, so the container, which
/// holds the configuration's text, is as long under each.
#[test]
fn a_restore_over_the_budget_is_refused_by_its_own_phase() {
    let (_, bytes) = checkpointed(&config(4096, "f32", BUDGET));
    let probe = whole(&bytes).admit_memory(false, 0, 0).expect("admitted");
    let (restoring, running) = (probe.restoring_bytes(), probe.fixed_bytes());
    println!(
        "the preset at tick 2, restored whole: restoring {restoring} bytes (its container {}), \
         running {running}",
        bytes.len()
    );
    assert_eq!(probe.restore, bytes.len() as u64);
    assert!(running < restoring, "{probe:?}");
    assert_eq!(probe.peak_bytes, restoring, "{probe:?}");
    let digits = |budget: u64| budget.to_string().len();
    assert_eq!(digits(restoring - 1), digits(BUDGET));

    let (_, short) = checkpointed(&config(4096, "f32", restoring - 1));
    assert_eq!(short.len(), bytes.len());
    match whole(&short).admit_memory(false, 0, 0) {
        Err(CoreError::BudgetExhausted {
            budget,
            required,
            available,
        }) => {
            assert_eq!(budget, "peak memory while restoring (maximum_total_bytes)");
            assert_eq!((required, available), (restoring, restoring - 1));
        }
        other => panic!("refused for the restore, not {other:?}"),
    }
    let (held, genomes) = split(&short);
    let admitted = streamed(&held, genomes)
        .expect("restores streamed")
        .admit_memory(false, 0, 0)
        .expect("the streamed restore is admitted");
    assert_eq!(
        admitted.restore,
        (held.len() + CHECKPOINT_PAGE_BYTES) as u64
    );
    assert!(admitted.restoring_bytes() < admitted.fixed_bytes());
    assert_eq!(admitted.peak_bytes, admitted.fixed_bytes());

    let (_, exact) = checkpointed(&config(4096, "f32", restoring));
    let admitted = whole(&exact)
        .admit_memory(false, 0, 0)
        .expect("admitted at exactly its restore's phase");
    assert_eq!(
        (admitted.budget_bytes, admitted.peak_bytes),
        (restoring, restoring)
    );
}
