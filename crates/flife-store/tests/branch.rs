//! FL-039 evidence in the store (plan §14.4, §16.6, §16.7, Appendix B.1):
//! a checkpoint published from a donor store links the donor's verified
//! chunk files instead of writing them, writes any it cannot share, leaves
//! nothing behind when cancelled, and survives the donor's retention; and a
//! branch's catalog records its lineage and origin and starts its journal
//! with exactly the history it inherited.

#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use flife_core::checkpoint::CheckpointManifest;
use flife_core::ids::{CommandId, ResourceEpoch, SessionId};
use flife_core::{CoreError, Sha256Digest, Tick, ValidatedConfig};
use flife_engine::{Arrangement, Intervention, NumericalProfile, Stepped, World};
use flife_runtime::{
    Accepted, Command, CommandBook, Durability as CommandDurability, Fork, Journal,
    JournalContents, JournalRecord, Outcome, Target, Watermarks, checkpoint, restore,
};
use flife_store::{BranchOrigin, ChunkStore, CommandJournal, Durability, Flow, Step};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");
const PARENT: SessionId = SessionId::new(7);
const BRANCH: SessionId = SessionId::new(8);
const BOTH: [Durability; 2] = [Durability::ProcessCrash, Durability::PowerLoss];

/// FL-013's small ecology from Appendix A.
fn ecology() -> ValidatedConfig {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    let mut lines: Vec<String> = appendix[start..start + length]
        .lines()
        .map(str::to_owned)
        .collect();
    for (section, key, value) in [
        ("world", "width", "32.0"),
        ("world", "height", "32.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", "48"),
        ("population", "maximum_count", "1024"),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "4"),
        ("execution", "command_durability", "\"volatile-in-memory\""),
    ] {
        let header = format!("[{section}]");
        let first = lines
            .iter()
            .position(|line| line.trim() == header)
            .expect("a section");
        let line = lines[first + 1..]
            .iter()
            .take_while(|line| !line.starts_with('['))
            .position(|line| line.split('=').next().map(str::trim) == Some(key))
            .map(|offset| first + 1 + offset)
            .expect("a key");
        lines[line] = format!("{key} = {value}");
    }
    ValidatedConfig::from_toml(&lines.join("\n")).expect("a valid configuration")
}

fn profile() -> NumericalProfile {
    NumericalProfile::reference_f32().expect("profile")
}

/// Checkpoints of the ecology's world after each of `ticks`, with each
/// world's tick and digest.
fn checkpoints(ticks: &[u64]) -> Vec<(Vec<u8>, (u64, Sha256Digest))> {
    let config = ecology();
    let book = CommandBook::new(PARENT, config.config_hash(), 16);
    let mut world =
        World::found(&config, Arrangement::FreeList, profile()).expect("the world founds");
    let mut out = Vec::new();
    for &tick in ticks {
        while world.tick().get() < tick {
            match world.step_with(&[]).expect("the tick advances") {
                Stepped::Published(_) => {}
                Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
            }
        }
        out.push((
            checkpoint(&world, &book).expect("checkpoints"),
            (world.tick().get(), world.digest()),
        ));
    }
    out
}

fn fresh(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("flife-store-branch")
        .join(name);
    if path.exists() {
        fs::remove_dir_all(&path).expect("clears the last run's directory");
    }
    fs::create_dir_all(path.parent().expect("a parent")).expect("creates the parent");
    path
}

/// A catalog path in a directory of its own, with nothing left from an
/// earlier run (its write-ahead log included).
fn catalog(name: &str) -> PathBuf {
    let directory = fresh(name);
    fs::create_dir_all(&directory).expect("creates");
    directory.join("catalog.db")
}

/// What a store's checkpoint `sequence` restores to.
fn restored(store: &ChunkStore, sequence: u64) -> Result<(u64, Sha256Digest), CoreError> {
    let bytes = store.load(sequence)?;
    restore(&bytes, profile())
        .map(|restored| (restored.world.tick().get(), restored.world.digest()))
}

/// The chunk files a store holds: digest to (device, inode).
fn files(root: &Path) -> BTreeMap<String, (u64, u64)> {
    let mut files = BTreeMap::new();
    for shard in fs::read_dir(root.join("chunks")).expect("chunks") {
        for chunk in fs::read_dir(shard.expect("a shard").path()).expect("a shard") {
            let chunk = chunk.expect("a chunk");
            let metadata = chunk.metadata().expect("inspects");
            files.insert(
                chunk.file_name().to_string_lossy().into_owned(),
                (metadata.dev(), metadata.ino()),
            );
        }
    }
    files
}

fn digests(checkpoint: &[u8]) -> Vec<Sha256Digest> {
    CheckpointManifest::parse(checkpoint)
        .expect("parses")
        .entries()
        .iter()
        .map(|entry| entry.digest)
        .collect()
}

#[test]
fn a_donors_verified_chunks_are_linked_not_written() {
    let [(a, a_world), (b, b_world), (c, c_world)] =
        <[_; 3]>::try_from(checkpoints(&[8, 16, 24])).unwrap_or_else(|_| panic!("three"));
    for durability in BOTH {
        let (donor_root, root) = (
            fresh(&format!("donor-{}", durability.name())),
            fresh(&format!("recipient-{}", durability.name())),
        );
        let mut donor = ChunkStore::open(&donor_root, durability).expect("opens");
        for bytes in [&a, &b, &c] {
            donor.publish(bytes).expect("publishes");
        }
        let mut store = ChunkStore::open(&root, durability).expect("opens");

        // Every chunk of `a` is the donor's file under the same name.
        let first = store.publish_sharing(&a, &donor).expect("publishes");
        let count = digests(&a).len();
        assert_eq!(
            (first.written, first.linked, first.shared, first.repaired),
            (count, count, 0, 0)
        );
        assert_eq!(first.bytes_written, 0, "no chunk bytes written");
        let (theirs, ours) = (files(&donor_root), files(&root));
        for digest in digests(&a) {
            assert_eq!(ours[&digest.to_hex()], theirs[&digest.to_hex()], "{digest}");
        }
        assert_eq!(restored(&store, 1).expect("restores"), a_world);

        // `b`: chunks already held are shared, the rest linked; none written.
        let mut steps = Vec::new();
        let second = store
            .publish_sharing_observed(&b, &donor, &mut |step| {
                steps.push(step);
                Flow::Continue
            })
            .expect("publishes");
        assert_eq!(second.written, second.linked);
        assert!(second.linked > 0 && second.shared > 0, "{second:?}");
        assert!(
            steps
                .iter()
                .any(|step| matches!(step, Step::ChunkLinked { .. }))
        );
        assert!(
            !steps
                .iter()
                .any(|step| matches!(step, Step::ChunkWritten { .. }))
        );
        assert_eq!(
            steps
                .iter()
                .filter(|step| matches!(step, Step::ChunkFlushed { .. }))
                .count(),
            if durability == Durability::PowerLoss {
                second.linked
            } else {
                0
            },
            "a linked chunk is flushed as a written one is"
        );
        assert_eq!(restored(&store, 2).expect("restores"), b_world);

        // A chunk the donor holds damaged is written, never linked: the
        // damage stays the donor's.
        let damaged = digests(&c)
            .into_iter()
            .find(|digest| !files(&root).contains_key(&digest.to_hex()))
            .expect("a chunk only c has");
        let path = donor.chunk_path(&damaged);
        let mut bytes = fs::read(&path).expect("reads");
        bytes[0] ^= 1;
        fs::write(&path, bytes).expect("damages the donor's chunk");
        let third = store.publish_sharing(&c, &donor).expect("publishes");
        assert_eq!(third.written, third.linked + 1);
        assert!(third.bytes_written > 0);
        assert_ne!(
            files(&root)[&damaged.to_hex()],
            files(&donor_root)[&damaged.to_hex()],
            "its own file"
        );
        assert_eq!(restored(&store, 3).expect("restores"), c_world);
        assert!(restored(&donor, 3).is_err(), "the donor's copy is damaged");

        // The donor's retention removes its names, not the recipient's (the
        // donor's damaged 3 does not count, so it keeps 2 and drops 1).
        let retention = donor.retain(1, |_, _| Ok(())).expect("retains");
        assert_eq!(retention.removed, [1]);
        assert!(retention.chunks_removed > 0);
        assert!(restored(&donor, 1).is_err(), "gone from the donor");
        assert_eq!(restored(&store, 1).expect("still whole"), a_world);
        assert_eq!(restored(&store, 2).expect("still whole"), b_world);
    }
}

/// A cancellation at any step of a sharing publication before the manifest
/// is linked leaves the recipient as it was and the donor untouched.
#[test]
fn cancelling_a_sharing_publication_at_any_step_leaves_both_stores_as_they_were() {
    let [(a, a_world), (b, b_world)] =
        <[_; 2]>::try_from(checkpoints(&[8, 16])).unwrap_or_else(|_| panic!("two"));
    for durability in BOTH {
        let donor_root = fresh(&format!("cancel-donor-{}", durability.name()));
        let mut donor = ChunkStore::open(&donor_root, durability).expect("opens");
        donor.publish(&b).expect("publishes");
        let donor_files = files(&donor_root);
        let mut cancelled = 0;
        for stop in 0.. {
            let root = fresh(&format!("cancel-recipient-{}", durability.name()));
            let mut store = ChunkStore::open(&root, durability).expect("opens");
            store.publish(&a).expect("publishes");
            let before = files(&root);
            let mut count = 0;
            let mut last = None;
            let outcome = store.publish_sharing_observed(&b, &donor, &mut |step| {
                let here = count == stop;
                count += 1;
                if here {
                    last = Some(step);
                    Flow::Cancel
                } else {
                    Flow::Continue
                }
            });
            let Some(step) = last else {
                outcome.expect("publishes");
                break;
            };
            if matches!(step, Step::Published | Step::DirectoryFlushed) {
                outcome.expect("too late to cancel");
                assert_eq!(restored(&store, 2).expect("published"), b_world);
                continue;
            }
            assert!(
                matches!(outcome, Err(CoreError::Cancelled { .. })),
                "{step:?}: {outcome:?}"
            );
            cancelled += 1;
            assert_eq!(
                fs::read_dir(root.join("tmp")).expect("tmp").count(),
                0,
                "{step:?}: no temporaries left"
            );
            assert_eq!(store.sequences().expect("lists"), [1]);
            assert_eq!(restored(&store, 1).expect("restores"), a_world);
            // Linked chunks stored before the cancellation stay, unreferenced
            // and harmless, as written ones do.
            assert!(before.keys().all(|name| files(&root).contains_key(name)));
        }
        assert!(cancelled >= 3, "{cancelled}");
        assert_eq!(files(&donor_root), donor_files, "the donor is untouched");
        assert_eq!(restored(&donor, 1).expect("restores"), b_world);
    }
}

fn brush(session: SessionId, id: u64, tick: u64) -> Command {
    Command {
        session,
        id: CommandId::new(id),
        target: Target::At {
            tick: Tick::new(tick),
            retarget: false,
        },
        expected_revision: None,
        intervention: Intervention::InjectEnergy {
            cell: [1, 2],
            amount: 1.0,
        },
    }
}

const fn accepted(order: u64, tick: u64) -> Accepted {
    Accepted {
        order,
        effective: Tick::new(tick),
        retargeted: false,
        durability: CommandDurability::WriteAhead,
    }
}

const fn applied(tick: u64, cursor: u64) -> Outcome {
    Outcome::Applied {
        tick: Tick::new(tick),
        published: Tick::new(tick + 1),
        epoch: ResourceEpoch::new(tick + 1),
        applied: 1.0,
        cursor,
    }
}

#[test]
fn a_branch_catalog_records_its_lineage_and_starts_with_its_inherited_history() {
    let revision = ecology().config_hash();
    let fork = Fork {
        parent: PARENT,
        admitted: 2,
        tick: Tick::new(4),
        checkpoint: Sha256Digest::of(b"the parent checkpoint"),
    };
    let origin = BranchOrigin {
        name: "control".into(),
        parent_run: "runs/parent".into(),
        parent_sequence: 3,
        created_unix_seconds: 1_800_000_000,
    };
    let history = JournalContents {
        accepted: vec![
            (brush(PARENT, 1, 2), accepted(0, 2)),
            (brush(PARENT, 2, 6), accepted(1, 6)),
        ],
        outcomes: vec![(CommandId::new(1), applied(2, 0))],
    };
    let path = catalog("catalog");
    {
        let mut journal = CommandJournal::open(&path).expect("opens");
        journal
            .open_branch(BRANCH, revision, &[fork], &origin, &history)
            .expect("records the branch");
        assert_eq!(
            journal.watermarks().expect("reads"),
            Watermarks {
                admitted: 2,
                outcomes: 1
            }
        );
        // The branch appends its own acceptance after the inherited ones,
        // and the inherited pending command's outcome.
        journal
            .append(&JournalRecord::Accepted {
                command: brush(BRANCH, 3, 9),
                accepted: accepted(2, 9),
            })
            .expect("its own");
        journal
            .append(&JournalRecord::Ended {
                id: CommandId::new(2),
                outcome: applied(6, 1),
            })
            .expect("an inherited command ends in the branch");
        assert!(matches!(
            journal.append(&JournalRecord::Accepted {
                command: brush(PARENT, 4, 9),
                accepted: accepted(3, 9),
            }),
            Err(CoreError::StateConflict { .. })
        ));
        assert!(matches!(
            journal.open_branch(BRANCH, revision, &[fork], &origin, &history),
            Err(CoreError::StateConflict { .. })
        ));
    }
    let mut journal = CommandJournal::open(&path).expect("reopens");
    let contents = journal
        .open_run(BRANCH, revision)
        .expect("the branch's run");
    assert_eq!(
        contents
            .accepted
            .iter()
            .map(|(command, accepted)| (command.session, command.id.get(), accepted.order))
            .collect::<Vec<_>>(),
        [(PARENT, 1, 0), (PARENT, 2, 1), (BRANCH, 3, 2)]
    );
    assert_eq!(contents.outcomes.len(), 2);
    assert_eq!(journal.lineage().expect("reads"), [fork]);
    assert_eq!(
        journal.branch_origin().expect("reads"),
        Some(origin.clone())
    );
    assert_eq!(journal.runs().expect("lists"), [(BRANCH, revision)]);

    // Refusals, each recording nothing.
    let other = SessionId::new(11);
    let mut wrong = history.clone();
    wrong.accepted[1].0.session = other;
    let mut misnumbered = history.clone();
    misnumbered.outcomes[0].1 = applied(2, 5);
    for (index, (lineage, history, why)) in [
        (&[][..], &history, "no fork"),
        (
            &[fork][..],
            &wrong,
            "an inherited acceptance of the branch's session",
        ),
        (&[fork][..], &misnumbered, "an outcome out of turn"),
    ]
    .into_iter()
    .enumerate()
    {
        let path = catalog(&format!("refused-{index}"));
        let mut journal = CommandJournal::open(&path).expect("opens");
        assert!(
            matches!(
                journal.open_branch(other, revision, lineage, &origin, history),
                Err(CoreError::StateConflict { .. })
            ),
            "{why}"
        );
        assert_eq!(journal.runs().expect("lists"), [], "{why}");
    }
}
