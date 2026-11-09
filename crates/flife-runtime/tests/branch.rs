//! FL-039 evidence at the command book (plan §14.4, §16.3): a book forked
//! at a checkpoint into a branch session inherits every command and
//! outcome, records the fork, admits only its own session's commands,
//! round-trips through a checkpoint with its lineage, and continues the
//! parent's world exactly. A book that is not a branch keeps FL-016's
//! command chunk, version 1. A lineage no branch makes, crafted into a
//! checkpoint, is refused.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

mod common;

use common::{SESSION, advance, at, command, ecology, found, inject, reference};
use flife_core::checkpoint::{CheckpointReader, CheckpointWriter, ChunkFamily};
use flife_core::ids::{CommandId, SessionId};
use flife_core::{CoreError, Sha256Digest, Tick};
use flife_engine::{NumericalProfile, World};
use flife_runtime::{
    Admission, CommandBook, CommandStatus, Durability, Fork, JournalContents, Outcome, Rejection,
    SESSION_CHECKPOINT_CHUNKS, checkpoint, restore,
};

const BRANCH: SessionId = SessionId::new(8);
const GRANDCHILD: SessionId = SessionId::new(9);
const CELL: [u32; 2] = [5, 9];

fn profile() -> NumericalProfile {
    NumericalProfile::reference_f32().expect("profile")
}

fn commands_version(bytes: &[u8]) -> u16 {
    CheckpointReader::parse(bytes, &SESSION_CHECKPOINT_CHUNKS)
        .expect("parses")
        .chunk(ChunkFamily::Commands)
        .expect("the commands")
        .version
}

/// A world at tick 4 whose book applied command 1 at tick 2 and holds
/// command 2 pending for tick 6, with its checkpoint's bytes.
fn parent() -> (World, CommandBook, Vec<u8>) {
    let config = ecology(&[]);
    let mut world = found(&config);
    let grid = *world.field().grid();
    let mut book = CommandBook::new(SESSION, config.config_hash(), 16);
    for (id, tick) in [(1, 2), (2, 6)] {
        let admission = book.admit(
            command(id, at(tick), inject(CELL, 1.0)),
            world.tick(),
            &grid,
            false,
        );
        assert!(matches!(
            admission,
            Admission::Accepted { retry: false, .. }
        ));
    }
    while world.tick().get() < 4 {
        advance(&mut world, &mut book);
    }
    let bytes = checkpoint(&world, &book).expect("checkpoints");
    (world, book, bytes)
}

#[test]
fn a_branch_inherits_every_command_and_admits_only_its_own() {
    let (world, book, bytes) = parent();
    assert_eq!(commands_version(&bytes), 1, "a book that is not a branch");
    let digest = Sha256Digest::of(&bytes);
    let tick = world.tick();
    let mut branch = book.branch(BRANCH, tick, digest).expect("branches");
    assert_eq!(branch.session(), BRANCH);
    assert_eq!(
        branch.lineage(),
        [Fork {
            parent: SESSION,
            admitted: 2,
            tick,
            checkpoint: digest,
        }]
    );
    assert_eq!(
        (
            branch.admitted_by(0),
            branch.admitted_by(1),
            branch.admitted_by(2)
        ),
        (SESSION, SESSION, BRANCH)
    );
    assert_eq!(branch.outcome_cursor(), 1);
    assert_eq!(branch.unfinished(), 1);
    assert!(matches!(
        branch.status(CommandId::new(1)),
        Some(CommandStatus::Terminal(Outcome::Applied { .. }))
    ));
    assert_eq!(
        branch.status(CommandId::new(2)),
        Some(&CommandStatus::Pending)
    );
    assert_eq!(branch.history(), book.history(), "the same history");
    assert_eq!(branch.history().accepted.len(), 2);
    assert_eq!(branch.history().outcomes.len(), 1);
    assert_eq!(branch.last_command(), Some(CommandId::new(2)));

    // Its own session only, under identities the inherited ones do not use.
    let grid = *world.field().grid();
    assert!(matches!(
        branch.admit(command(3, at(8), inject(CELL, 1.0)), tick, &grid, false),
        Admission::Rejected(Rejection::WrongSession { session }) if session == BRANCH
    ));
    let mut own = command(2, at(8), inject(CELL, 1.0));
    own.session = BRANCH;
    assert!(matches!(
        branch.admit(own, tick, &grid, false),
        Admission::Rejected(Rejection::ConflictingReuse { .. })
    ));
    own.id = CommandId::new(3);
    let Admission::Accepted { accepted, .. } = branch.admit(own, tick, &grid, false) else {
        panic!("the branch admits its own command");
    };
    assert_eq!(accepted.order, 2, "numbering continues");
    // The parent is unchanged by its branch.
    assert_eq!(book.history().accepted.len(), 2);
    assert!(book.status(CommandId::new(3)).is_none());

    // A session already in the lineage is refused.
    for session in [SESSION, BRANCH] {
        assert!(matches!(
            branch.branch(session, tick, digest),
            Err(CoreError::StateConflict { .. })
        ));
    }
    assert!(matches!(
        book.branch(SESSION, tick, digest),
        Err(CoreError::StateConflict { .. })
    ));

    // A closed book branches into an open one.
    let mut closed = book.clone();
    closed.close(tick);
    let reopened = closed.branch(BRANCH, tick, digest).expect("branches");
    assert!(!reopened.is_closed());
    assert_eq!(reopened.durability(), Durability::Volatile);
}

#[test]
fn a_branch_round_trips_with_its_lineage_and_continues_its_parents_world() {
    let config = ecology(&[]);
    let (world, book, bytes) = parent();
    let digest = Sha256Digest::of(&bytes);
    let tick = world.tick();
    let branch = book.branch(BRANCH, tick, digest).expect("branches");
    let branched = checkpoint(&world, &branch).expect("checkpoints");
    assert_eq!(commands_version(&branched), 2);
    let restored = restore(&branched, profile()).expect("restores");
    assert_eq!(restored.book.session(), BRANCH);
    assert_eq!(restored.book.lineage(), branch.lineage());
    assert_eq!(restored.book.history(), branch.history());
    assert_eq!(
        checkpoint(&restored.world, &restored.book).expect("re-checkpoints"),
        branched,
        "canonical"
    );
    // Only the command closure differs from the parent's checkpoint.
    let parent_chunks =
        CheckpointReader::parse(&bytes, &SESSION_CHECKPOINT_CHUNKS).expect("parses");
    let branch_chunks =
        CheckpointReader::parse(&branched, &SESSION_CHECKPOINT_CHUNKS).expect("parses");
    for family in SESSION_CHECKPOINT_CHUNKS {
        let same = parent_chunks.chunk(family).expect("a chunk").bytes
            == branch_chunks.chunk(family).expect("a chunk").bytes;
        assert_eq!(same, family != ChunkFamily::Commands, "{family:?}");
    }

    // A grandchild carries both forks.
    let deeper = restored
        .book
        .branch(GRANDCHILD, tick, Sha256Digest::of(&branched))
        .expect("branches");
    let nested = restore(
        &checkpoint(&world, &deeper).expect("checkpoints"),
        profile(),
    )
    .expect("restores");
    assert_eq!(
        nested
            .book
            .lineage()
            .iter()
            .map(|fork| fork.parent)
            .collect::<Vec<_>>(),
        [SESSION, BRANCH]
    );

    // Parent and branch continue identically: command 2 applies at tick 6
    // in each, as in the world stepped directly.
    let mut parent_world = restore(&bytes, profile()).expect("restores");
    let mut branch_world = restore(&branched, profile()).expect("restores");
    for restored in [&mut parent_world, &mut branch_world] {
        while restored.world.tick().get() < 8 {
            advance(&mut restored.world, &mut restored.book);
        }
    }
    let direct = reference(
        &config,
        8,
        &[(2, inject(CELL, 1.0)), (6, inject(CELL, 1.0))],
    );
    assert_eq!(parent_world.world.digest(), direct.digest());
    assert_eq!(branch_world.world.digest(), direct.digest());
    assert_eq!(
        parent_world.book.history().outcomes,
        branch_world.book.history().outcomes
    );
}

#[test]
fn a_lineage_that_is_not_one_is_refused() {
    let (world, book, bytes) = parent();
    let digest = Sha256Digest::of(&bytes);
    // A fork after the committed tick.
    let future = book
        .branch(BRANCH, Tick::new(world.tick().get() + 1), digest)
        .expect("the book does not know the world's tick");
    let refused = restore(&checkpoint(&world, &future).expect("encodes"), profile());
    assert!(
        matches!(&refused, Err(CoreError::IncompatibleCheckpoint { reason })
            if reason.contains("is not a lineage")),
        "{refused:?}"
    );

    // Recovery holds the journal to the lineage: an inherited acceptance
    // must be the parent session's, a later one the branch's.
    let branch = book.branch(BRANCH, world.tick(), digest).expect("branches");
    let history = branch.history();
    let mut recovered = branch.clone();
    assert_eq!(
        recovered.recover(&history, world.tick()).expect("recovers"),
        Vec::new()
    );
    let mut wrong: JournalContents = history.clone();
    wrong.accepted[0].0.session = BRANCH;
    assert!(matches!(
        branch.clone().recover(&wrong, world.tick()),
        Err(CoreError::StateConflict { .. })
    ));
    let mut later = history;
    let mut own = command(9, at(10), inject(CELL, 1.0));
    let mut accepted = later.accepted[1].1;
    accepted.order = 2;
    accepted.effective = Tick::new(10);
    later.accepted.push((own, accepted));
    assert!(
        matches!(
            branch.clone().recover(&later, world.tick()),
            Err(CoreError::StateConflict { .. })
        ),
        "a parent-session command after the fork"
    );
    own.session = BRANCH;
    later.accepted[2].0 = own;
    let mut extended = branch.clone();
    extended
        .recover(&later, world.tick())
        .expect("the branch's own");
    assert_eq!(
        extended.status(CommandId::new(9)),
        Some(&CommandStatus::Pending)
    );
}

/// The checkpoint `bytes` with its commands chunk replaced by `edit` of it,
/// at `version`, every other chunk as it was: the same bytes when nothing
/// is changed, so a refusal is `edit`'s alone.
fn with_commands(bytes: &[u8], version: u16, edit: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let reader = CheckpointReader::parse(bytes, &SESSION_CHECKPOINT_CHUNKS).expect("parses");
    let mut writer = CheckpointWriter::new(reader.model());
    let families: Vec<ChunkFamily> = reader.families().collect();
    let mut edit = Some(edit);
    for (index, family) in families.iter().enumerate() {
        let chunk = reader.chunk(*family).expect("present");
        let mut data = chunk.bytes.to_vec();
        let mut chunk_version = chunk.version;
        if *family == ChunkFamily::Commands {
            (edit.take().expect("one edit"))(&mut data);
            chunk_version = version;
        }
        let depends: &[ChunkFamily] = match family {
            ChunkFamily::Configuration => &[],
            ChunkFamily::Genomes => &[ChunkFamily::Topology],
            ChunkFamily::Particles => &[ChunkFamily::Genomes],
            ChunkFamily::Held => &[ChunkFamily::Particles],
            ChunkFamily::Ledger => &[ChunkFamily::Field, ChunkFamily::Particles],
            _ => &[ChunkFamily::Configuration],
        };
        let depends: Vec<ChunkFamily> = depends
            .iter()
            .copied()
            .filter(|depend| families[..index].contains(depend))
            .collect();
        writer
            .chunk(*family, chunk_version, chunk.items, data, &depends)
            .expect("rewrites");
    }
    writer.finish().expect("finishes")
}

/// The commands chunk's lineage starts after the session and revision: a
/// count, then each fork's parent, admitted order, tick and digest.
const LINEAGE_AT: usize = 8 + 32;
const FORK_BYTES: usize = 8 + 8 + 8 + 32;

/// An edit of a commands chunk's bytes.
type Edit = Box<dyn FnOnce(&mut Vec<u8>)>;

fn set_u64(data: &mut [u8], at: usize, value: u64) {
    data[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

/// A crafted lineage that no `branch` call makes is refused on restore:
/// a repeated session, the book's own session as a parent, a fork past the
/// admissions, forks out of order, a lineage that attributes an inherited
/// command to another session, a version 2 chunk with no fork, and an
/// unknown version.
#[test]
fn a_crafted_lineage_is_refused_on_restore() {
    let (world, book, bytes) = parent();
    let digest = Sha256Digest::of(&bytes);
    let tick = world.tick();
    let branch = book.branch(BRANCH, tick, digest).expect("branches");
    let grandchild = branch
        .branch(GRANDCHILD, tick, Sha256Digest::of(&bytes))
        .expect("branches");
    let nested = checkpoint(&world, &grandchild).expect("checkpoints");
    assert_eq!(
        with_commands(&nested, 2, |_| ()),
        nested,
        "rewriting changes nothing by itself"
    );
    restore(&nested, profile()).expect("the crafted lineage's original restores");
    let fork = |index: usize| LINEAGE_AT + 8 + index * FORK_BYTES;
    let refusals: [(&str, Edit, u16, &str); 8] = [
        (
            "a repeated session",
            Box::new(move |data| set_u64(data, fork(1), SESSION.get())),
            2,
            "is not a lineage",
        ),
        (
            "the book's own session as a parent",
            Box::new(move |data| set_u64(data, fork(1), GRANDCHILD.get())),
            2,
            "is not a lineage",
        ),
        (
            "a fork past the admissions",
            Box::new(move |data| set_u64(data, fork(1) + 8, 3)),
            2,
            "is not a lineage",
        ),
        (
            "forks out of admission order",
            Box::new(move |data| set_u64(data, fork(1) + 8, 1)),
            2,
            "is not a lineage",
        ),
        (
            "forks out of tick order",
            Box::new(move |data| set_u64(data, fork(1) + 16, tick.get() - 1)),
            2,
            "is not a lineage",
        ),
        (
            "an inherited command the lineage gives the branch",
            Box::new(move |data| set_u64(data, fork(0) + 8, 1)),
            2,
            "is not a command this session admitted in order 1",
        ),
        (
            "a branch chunk with no fork",
            Box::new(move |data| {
                let end = LINEAGE_AT + 8 + 2 * FORK_BYTES;
                data.splice(LINEAGE_AT..end, 0_u64.to_le_bytes());
            }),
            2,
            "carries no fork",
        ),
        (
            "an unknown version",
            Box::new(|_| ()),
            3,
            "this build reads",
        ),
    ];
    for (why, edit, version, expected) in refusals {
        let refused = restore(&with_commands(&nested, version, edit), profile());
        assert!(
            matches!(&refused, Err(CoreError::IncompatibleCheckpoint { reason })
                if reason.contains(expected)),
            "{why}: {refused:?}"
        );
    }
}
