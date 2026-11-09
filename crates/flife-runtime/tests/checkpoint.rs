//! FL-016 evidence (plan §16.3, Appendix B.2, C.13, C.14, INV-14): a
//! committed world and its command closure round-trip through a checkpoint
//! exactly, off-phase and with newborns, pending future commands apply once
//! after a restore, and damaged checkpoints are refused.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

mod common;

use common::{SESSION, advance, at, command, ecology, found, inject, publish, reference};
use flife_core::checkpoint::{CheckpointReader, CheckpointWriter, ChunkFamily};
use flife_core::ids::CommandId;
use flife_core::{CoreError, ResidualBudget, Sha256Digest, Tick, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, TickReport, World};
use flife_runtime::{
    Admission, CommandBook, CommandStatus, Outcome, Rejection, Restored, SESSION_CHECKPOINT_CHUNKS,
    checkpoint, restore,
};

const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");

fn profile() -> NumericalProfile {
    NumericalProfile::reference_f32().expect("profile")
}

/// Checkpoint `world` and `book`, restore them, and check the restored pair
/// is the same committed state and encodes to the same bytes.
fn round_trip(world: &World, book: &CommandBook) -> (Vec<u8>, Restored) {
    let bytes = checkpoint(world, book).expect("checkpoints");
    let restored = restore(&bytes, profile()).expect("restores");
    assert_eq!(
        restored.world.digest(),
        world.digest(),
        "the same committed state"
    );
    assert_eq!(restored.world.tick(), world.tick());
    assert_eq!(restored.world.epoch(), world.epoch());
    assert_eq!(restored.world.events(), world.events());
    assert_eq!(restored.world.ledger().report(), world.ledger().report());
    assert_eq!(restored.world.paths(), world.paths());
    // The digest leaves genome contents out; compare them directly.
    for organism in world.population().iter() {
        let (a, b) = (
            world.population().genome(organism.id).expect("a genome"),
            restored
                .world
                .population()
                .genome(organism.id)
                .expect("restored"),
        );
        assert_eq!(
            (a.id(), a.topology(), a.parent(), a.traits(), a.parameters()),
            (b.id(), b.topology(), b.parent(), b.traits(), b.parameters()),
            "{}'s genome",
            organism.id
        );
        assert_eq!(a.allocated_bytes(), b.allocated_bytes());
    }
    assert_eq!(
        checkpoint(&restored.world, &restored.book).expect("re-checkpoints"),
        bytes,
        "canonical: the restored pair encodes to the same bytes"
    );
    (bytes, restored)
}

fn book_for(config: &ValidatedConfig) -> CommandBook {
    CommandBook::new(SESSION, config.config_hash(), 16)
}

/// C.13 at the ecology's scale: with a four-tick controller cadence, a
/// world checkpointed off-phase (tick 5: outputs held from ticks 2 to 5,
/// none due again until its phase), at a tick that committed newborns
/// (their outputs invalid, a forced evaluation due next tick) and at later
/// ticks with births, deaths and freed slots, continues exactly as the
/// uninterrupted world: every later report and digest is the same.
#[test]
fn a_restored_world_publishes_every_later_tick_as_the_uninterrupted_one_does() {
    let config = ecology(&[("controller", "evaluation_interval_ticks", "4")]);
    let book = book_for(&config);
    let horizon = 480_u64;
    let mut world = found(&config);
    let mut reports: Vec<TickReport> = Vec::new();
    let mut checkpoints: Vec<(u64, Vec<u8>, &str)> = Vec::new();
    let mut newborns_taken = false;
    while world.tick().get() < horizon {
        let tick = world.tick().get();
        let newborn_tick = !newborns_taken
            && tick >= 6
            && reports
                .last()
                .is_some_and(|report| !report.births.is_empty());
        let late = reports
            .last()
            .is_some_and(|report| !report.deaths.is_empty())
            && checkpoints.iter().all(|&(_, _, why)| why != "after deaths");
        if tick == 0 || tick == 5 || newborn_tick || late || tick == 401 {
            let why = match tick {
                0 => "founding",
                5 => "off-phase",
                401 => "late",
                _ if newborn_tick => "newborns",
                _ => "after deaths",
            };
            if newborn_tick {
                newborns_taken = true;
                let invalid = world
                    .population()
                    .iter()
                    .filter(|o| o.birth_tick.get() == tick && !o.held.is_valid())
                    .count();
                assert!(invalid > 0, "newborns at tick {tick} hold no output yet");
            }
            if tick == 5 {
                let held: Vec<Option<u64>> = world
                    .population()
                    .iter()
                    .map(|o| o.held.evaluated_at().map(Tick::get))
                    .collect();
                assert!(
                    held.contains(&Some(4)) && held.contains(&Some(2)),
                    "outputs held from earlier ticks: {held:?}"
                );
            }
            // Restores to the same state, genomes included, and re-encodes
            // to the same bytes.
            if why == "after deaths" {
                assert!(
                    world.population().iter().any(|o| o.parent.is_some()),
                    "children's genomes cross this checkpoint"
                );
            }
            let (bytes, _) = round_trip(&world, &book);
            checkpoints.push((tick, bytes, why));
        }
        reports.push(publish(&mut world, &[]));
    }
    assert!(newborns_taken, "a tick with newborns was checkpointed");
    let births: usize = reports.iter().map(|report| report.births.len()).sum();
    let deaths: usize = reports.iter().map(|report| report.deaths.len()).sum();
    assert!(
        births > 0 && deaths > 0,
        "{births} births and {deaths} deaths"
    );
    assert!(
        checkpoints.iter().any(|&(_, _, why)| why == "after deaths"),
        "a tick after deaths was checkpointed"
    );

    for (tick, bytes, why) in &checkpoints {
        let Restored {
            world: mut restored,
            book: restored_book,
        } = restore(bytes, profile()).expect("restores");
        assert_eq!(restored.tick().get(), *tick);
        assert_eq!(restored_book.unfinished(), 0);
        while restored.tick().get() < horizon {
            let at = usize::try_from(restored.tick().get()).expect("small");
            let report = publish(&mut restored, &[]);
            assert!(
                report == reports[at],
                "{why} checkpoint at tick {tick}: tick {} differs from the uninterrupted run",
                report.tick.get()
            );
        }
        assert_eq!(
            restored.digest(),
            world.digest(),
            "{why} checkpoint at {tick}"
        );
        println!(
            "ok: the {why} checkpoint at tick {tick} ({} bytes) continues to tick {horizon} \
             with every report identical",
            bytes.len()
        );
    }
}

/// C.14's cursor case across a checkpoint: A accepted for tick 40 before B
/// for tick 10, C cancelled, checkpoint after B applied. After the restore
/// A is still pending, B and C keep their outcomes, A applies once at tick
/// 40, and both runs end at the directly stepped world. A retry of A is
/// recognised; reusing A's identity for other content is refused.
#[test]
fn pending_commands_cross_a_checkpoint_and_apply_once() {
    let config = ecology(&[]);
    let grid = *found(&config).field().grid();
    let (a, b, c) = (
        command(1, at(40), inject([3, 4], 2.0)),
        command(2, at(10), inject([20, 9], 1.5)),
        command(3, at(30), inject([7, 7], 0.5)),
    );
    let mut world = found(&config);
    let mut book = book_for(&config);
    for cmd in [a, b, c] {
        assert!(matches!(
            book.admit(cmd, world.tick(), &grid, false),
            Admission::Accepted { retry: false, .. }
        ));
    }
    while world.tick().get() < 12 {
        advance(&mut world, &mut book);
    }
    assert!(matches!(
        book.cancel(c.id, world.tick()),
        flife_runtime::Cancellation::Cancelled { .. }
    ));
    let (
        _,
        Restored {
            world: mut restored,
            book: mut restored_book,
        },
    ) = round_trip(&world, &book);

    assert_eq!(restored_book.status(a.id), Some(&CommandStatus::Pending));
    assert_eq!(restored_book.status(b.id), book.status(b.id));
    assert_eq!(restored_book.status(c.id), book.status(c.id));
    assert_eq!(restored_book.outcome_stream(), book.outcome_stream());
    assert_eq!(
        restored_book.schedule().collect::<Vec<_>>(),
        book.schedule().collect::<Vec<_>>()
    );
    assert_eq!(restored_book.admitted(), 3);

    while world.tick().get() < 45 {
        let original = advance(&mut world, &mut book);
        let again = advance(&mut restored, &mut restored_book);
        assert!(original == again, "tick {} differs", original.tick.get());
    }
    let applied = |book: &CommandBook| match book.status(a.id) {
        Some(CommandStatus::Terminal(Outcome::Applied { tick, .. })) => tick.get(),
        other => panic!("A is {other:?}"),
    };
    assert_eq!((applied(&book), applied(&restored_book)), (40, 40));
    assert_eq!(restored_book.outcome_stream(), book.outcome_stream());
    assert_eq!(
        restored_book
            .outcome_stream()
            .iter()
            .filter(|id| **id == a.id)
            .count(),
        1,
        "A applied exactly once"
    );
    let direct = reference(&config, 45, &[(10, b.intervention), (40, a.intervention)]);
    assert_eq!(restored.digest(), direct.digest());
    assert_eq!(world.digest(), direct.digest());

    assert!(matches!(
        restored_book.admit(a, restored.tick(), &grid, false),
        Admission::Accepted { retry: true, .. }
    ));
    let mut reuse = a;
    reuse.intervention = inject([3, 4], 2.5);
    assert!(matches!(
        restored_book.admit(reuse, restored.tick(), &grid, false),
        Admission::Rejected(Rejection::ConflictingReuse { .. })
    ));
}

/// A boundary sealed into a candidate that was not published (a capacity
/// pause or a discarded candidate) stays sealed across a checkpoint, and
/// the identical set then applies.
#[test]
fn a_sealed_boundary_stays_sealed_across_a_checkpoint() {
    let config = ecology(&[]);
    let grid = *found(&config).field().grid();
    let mut world = found(&config);
    let mut book = book_for(&config);
    while world.tick().get() < 8 {
        advance(&mut world, &mut book);
    }
    let due = command(9, at(8), inject([1, 2], 1.0));
    book.admit(due, world.tick(), &grid, false);
    let sealed = book.seal(world.tick()).expect("seals");
    assert_eq!(sealed.len(), 1);
    let (
        _,
        Restored {
            world: mut restored,
            book: mut restored_book,
        },
    ) = round_trip(&world, &book);
    assert_eq!(restored_book.sealed(), Some(Tick::new(8)));
    assert_eq!(restored_book.status(due.id), Some(&CommandStatus::Sealed));
    assert!(matches!(
        restored_book.admit(
            command(10, at(8), inject([1, 2], 1.0)),
            restored.tick(),
            &grid,
            false
        ),
        Admission::Rejected(Rejection::Late { .. })
    ));
    let reseal = restored_book.seal(restored.tick()).expect("reseals");
    assert_eq!(reseal, sealed);
    let report = publish(&mut restored, &[due.intervention]);
    restored_book.published(&report, &reseal).expect("recorded");
    let original = publish(&mut world, &[due.intervention]);
    book.published(&original, &sealed).expect("recorded");
    assert!(report == original);
    assert_eq!(restored_book.outcome_stream(), book.outcome_stream());
}

/// The 4,096-organism reference preset at tick 64, checkpointed and
/// restored, publishes ticks 65 to 96 exactly as the uninterrupted world.
#[test]
fn the_reference_preset_round_trips_at_its_scale() {
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset validates");
    let book = book_for(&config);
    let mut world = found(&config);
    for _ in 0..64 {
        publish(&mut world, &[]);
    }
    let clock = std::time::Instant::now();
    let bytes = checkpoint(&world, &book).expect("checkpoints");
    let written = clock.elapsed();
    let clock = std::time::Instant::now();
    let Restored {
        world: mut restored,
        ..
    } = restore(&bytes, profile()).expect("restores");
    let read = clock.elapsed();
    assert_eq!(restored.digest(), world.digest());
    assert_eq!(restored.population().len(), world.population().len());
    for _ in 0..32 {
        let original = publish(&mut world, &[]);
        let again = publish(&mut restored, &[]);
        assert!(original == again, "tick {} differs", original.tick.get());
    }
    println!(
        "ok: {} organisms at tick 64: {} bytes ({:.1} MiB), written in {written:?}, verified \
         and restored in {read:?}; ticks 65-96 identical",
        world.population().len(),
        bytes.len(),
        bytes.len() as f64 / 1_048_576.0
    );
}

/// Every chunk of `bytes`, with `family`'s replaced by `edit`, re-sealed so
/// that only `edit`'s change can be refused.
fn rewritten(bytes: &[u8], family: ChunkFamily, edit: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let reader = CheckpointReader::parse(bytes, &[]).expect("parses");
    let mut writer = CheckpointWriter::new(reader.model());
    let mut edit = Some(edit);
    let families: Vec<ChunkFamily> = reader.families().collect();
    for (index, current) in families.iter().enumerate() {
        let chunk = reader.chunk(*current).expect("present");
        let mut data = chunk.bytes.to_vec();
        if *current == family {
            (edit.take().expect("one edit"))(&mut data);
        }
        writer
            .chunk(
                *current,
                chunk.version,
                chunk.items,
                data,
                &dependencies(*current, &families[..index]),
            )
            .expect("rewrites");
    }
    writer.finish().expect("finishes")
}

/// The dependencies `checkpoint` writes, restricted to `before`.
fn dependencies(family: ChunkFamily, before: &[ChunkFamily]) -> Vec<ChunkFamily> {
    let all: &[ChunkFamily] = match family {
        ChunkFamily::Configuration => &[],
        ChunkFamily::Genomes => &[ChunkFamily::Topology],
        ChunkFamily::Particles => &[ChunkFamily::Genomes],
        ChunkFamily::Held => &[ChunkFamily::Particles],
        ChunkFamily::Ledger => &[ChunkFamily::Field, ChunkFamily::Particles],
        _ => &[ChunkFamily::Configuration],
    };
    all.iter().copied().filter(|d| before.contains(d)).collect()
}

fn refused(bytes: &[u8]) -> String {
    match restore(bytes, profile()) {
        Err(CoreError::IncompatibleCheckpoint { reason }) => reason,
        Err(other) => panic!("refused with another class: {other}"),
        Ok(_) => panic!("accepted a damaged checkpoint"),
    }
}

/// Damage is refused as `incompatible-checkpoint`, never accepted and
/// never a panic: every truncation of the header and manifest and a sample
/// through the payload; a trailing byte; every single-byte change in the
/// header, manifest and seal and a sample through the payload; and
/// re-sealed checkpoints whose chunks disagree with each other or with
/// this build.
#[test]
fn damaged_checkpoints_are_refused() {
    let config = ecology(&[]);
    let grid = *found(&config).field().grid();
    let mut world = found(&config);
    let mut book = book_for(&config);
    // Command 0 is cancelled (the outcome stream holds it); command 1 is
    // pending, the last record before the outcome stream.
    for id in [0, 1] {
        book.admit(
            command(id, at(30), inject([2, 2], 1.0)),
            world.tick(),
            &grid,
            false,
        );
    }
    book.cancel(CommandId::new(0), world.tick());
    while world.tick().get() < 20 {
        advance(&mut world, &mut book);
    }
    let bytes = checkpoint(&world, &book).expect("checkpoints");
    let model = usize::from(u16::from_le_bytes([bytes[13], bytes[14]]));
    let manifest = u32::from_le_bytes(
        bytes[15 + model..19 + model]
            .try_into()
            .expect("four bytes"),
    );
    let sealed = 19 + model + usize::try_from(manifest).expect("small") + 32;

    let mut refusals = 0_usize;
    let stride = 97;
    let positions = (0..sealed).chain((sealed..bytes.len()).step_by(stride));
    for length in positions.clone() {
        refused(&bytes[..length]);
        refusals += 1;
    }
    let mut longer = bytes.clone();
    longer.push(0);
    assert!(refused(&longer).contains("payload"));
    for position in positions {
        let mut damaged = bytes.clone();
        damaged[position] ^= 0x10;
        refused(&damaged);
        refusals += 1;
    }
    println!(
        "ok: {refusals} truncations and byte changes of a {}-byte checkpoint were refused",
        bytes.len()
    );

    // Re-sealed, so only the chunk content can be wrong.
    let reason = refused(&rewritten(&bytes, ChunkFamily::Particles, |data| {
        data[..8].copy_from_slice(&u64::MAX.to_le_bytes());
    }));
    assert!(reason.contains("declares"), "{reason}");

    let reason = refused(&rewritten(&bytes, ChunkFamily::Ledger, |data| {
        let last = f64::from_le_bytes(data[16..24].try_into().expect("eight"));
        data[16..24].copy_from_slice(&last.next_up().to_le_bytes());
    }));
    assert!(reason.contains("ledger closed"), "{reason}");

    let reason = refused(&rewritten(&bytes, ChunkFamily::Lifecycle, |data| {
        // The genome quota's live count is the last value.
        let at = data.len() - 8;
        let live = u64::from_le_bytes(data[at..].try_into().expect("eight"));
        data[at..].copy_from_slice(&(live + 1).to_le_bytes());
    }));
    assert!(reason.contains("quota"), "{reason}");

    let reason = refused(&rewritten(&bytes, ChunkFamily::Random, |data| {
        let at = data.len() - 8;
        data[at..].copy_from_slice(&12345_u64.to_le_bytes());
    }));
    assert!(reason.contains("seed"), "{reason}");

    let reason = refused(&rewritten(&bytes, ChunkFamily::Configuration, |data| {
        data[8] ^= 0x01;
    }));
    assert!(reason.contains("configuration"), "{reason}");

    let reason = refused(&rewritten(&bytes, ChunkFamily::Genomes, |data| {
        // The first genome's first parameter, after its id, topology,
        // parent flag, 66 traits and parameter count.
        let at = 8 + 8 + 8 + 1 + 66 * 8 + 8;
        data[at..at + 4].copy_from_slice(&f32::MAX.to_le_bytes());
    }));
    assert!(reason.contains("parameters"), "{reason}");

    let reason = refused(&rewritten(&bytes, ChunkFamily::Held, |data| {
        // The first organism's phase follows its id, a valid evaluation
        // tick and 36 outputs.
        let at = 8 + 8 + 9 + 36 * 4;
        data[at] ^= 0x01;
    }));
    assert!(reason.contains("phase"), "{reason}");

    let reason = refused(&rewritten(&bytes, ChunkFamily::Commands, |data| {
        // Claim the pending command is sealed: its status byte precedes
        // the outcome count and the one outcome.
        let at = data.len() - 16 - 1;
        data[at] = 1;
    }));
    assert!(reason.contains("sealed"), "{reason}");

    let reason = refused(&rewritten(&bytes, ChunkFamily::Commands, |data| {
        // Seal a boundary the world is not at: the sealed flag follows the
        // session, revision, capacity and admission count.
        data[56] = 1;
        data.splice(57..57, 21_u64.to_le_bytes());
    }));
    assert!(
        reason.contains("is sealed in a world at tick 20"),
        "{reason}"
    );

    let reason = refused(&rewritten(&bytes, ChunkFamily::Commands, |data| {
        // Name the pending command as the stream's one outcome.
        let at = data.len() - 8;
        data[at..].copy_from_slice(&1_u64.to_le_bytes());
    }));
    assert!(reason.contains("did not end there"), "{reason}");

    // A manifest whose item count disagrees with its chunk.
    let reader = CheckpointReader::parse(&bytes, &[]).expect("parses");
    let mut writer = CheckpointWriter::new(reader.model());
    let families: Vec<ChunkFamily> = reader.families().collect();
    for (index, family) in families.iter().enumerate() {
        let chunk = reader.chunk(*family).expect("present");
        let items = chunk.items + u64::from(*family == ChunkFamily::Particles);
        writer
            .chunk(
                *family,
                chunk.version,
                items,
                chunk.bytes.to_vec(),
                &dependencies(*family, &families[..index]),
            )
            .expect("rewrites");
    }
    let reason = refused(&writer.finish().expect("finishes"));
    assert!(reason.contains("manifest counts"), "{reason}");

    // A missing chunk, and a checkpoint read under another profile.
    let reader = CheckpointReader::parse(&bytes, &[]).expect("parses");
    let mut writer = CheckpointWriter::new(reader.model());
    for family in reader.families().filter(|f| *f != ChunkFamily::Commands) {
        let chunk = reader.chunk(family).expect("present");
        writer
            .chunk(
                family,
                chunk.version,
                chunk.items,
                chunk.bytes.to_vec(),
                &[],
            )
            .expect("rewrites");
    }
    let reason = refused(&writer.finish().expect("finishes"));
    assert!(reason.contains("missing chunk commands"), "{reason}");
    let strict = NumericalProfile {
        residual: ResidualBudget::EXACT,
        ..profile()
    };
    assert!(matches!(
        restore(&bytes, strict),
        Err(CoreError::IncompatibleCheckpoint { .. })
    ));
    assert_eq!(SESSION_CHECKPOINT_CHUNKS.len(), ChunkFamily::ALL.len());
}

/// A checkpoint binds its world to its configuration: a book from another
/// configuration is refused, and a world founded with the compact
/// arrangement restores as compact.
#[test]
fn a_checkpoint_binds_the_world_to_its_configuration_and_arrangement() {
    let config = ecology(&[]);
    let world = World::found(&config, Arrangement::Compact, profile()).expect("founds");
    let other = CommandBook::new(SESSION, Sha256Digest::of(b"another configuration"), 16);
    assert!(matches!(
        checkpoint(&world, &other),
        Err(CoreError::StateConflict { .. })
    ));
    let (_, restored) = round_trip(&world, &book_for(&config));
    assert_eq!(
        restored.world.population().arrangement(),
        Arrangement::Compact
    );
    assert_eq!(restored.world.config(), &config);
}
