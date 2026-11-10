//! FL-035 evidence (plan §16.4, §16.6, §16.7, Appendix B.2): checkpoints of
//! a real world published to the chunk store load back byte for byte, and
//! a crash (a real `SIGKILL`) or cancellation at every publication step,
//! damaged manifests and chunks, and refused storage operations all leave
//! the last valid checkpoint recoverable. FL-038: a checkpoint streamed into
//! the store as it is encoded is stored exactly as the whole one, survives a
//! crash or cancellation at every step the same way, and retention by
//! verification decides as retention by restoring.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use flife_core::checkpoint::{CheckpointManifest, ChunkFamily, ChunkSink};
use flife_core::ids::SessionId;
use flife_core::{CoreError, Sha256Digest, ValidatedConfig};
use flife_engine::{Arrangement, NumericalProfile, Stepped, World};
use flife_runtime::{CommandBook, checkpoint, checkpoint_into, restore};
use flife_store::{ChunkStore, Durability, Flow, Observer, Publication, STORE_FORMAT, Step};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");
const PRESET: &str = include_str!("../../../presets/reference-small-world.toml");
const SESSION: SessionId = SessionId::new(7);
const CHILD: &str = "FLIFE_STORE_CHILD";
const CHILD_LIMITED: &str = "FLIFE_STORE_CHILD_LIMITED";
const STREAM_CHILD: &str = "FLIFE_STORE_STREAM_CHILD";
const BOTH: [Durability; 2] = [Durability::ProcessCrash, Durability::PowerLoss];

/// Appendix A with `overrides`, each `(section, key, value)`.
fn config(overrides: &[(&str, &str, &str)]) -> ValidatedConfig {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    let mut lines: Vec<String> = appendix[start..start + length]
        .lines()
        .map(str::to_owned)
        .collect();
    for &(section, key, value) in overrides {
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

/// FL-013's small ecology (32 × 32, 48 founders, four sources).
fn ecology() -> ValidatedConfig {
    config(&[
        ("world", "width", "32.0"),
        ("world", "height", "32.0"),
        ("world", "grid_width", "32"),
        ("world", "grid_height", "32"),
        ("population", "initial_count", "48"),
        ("population", "maximum_count", "1024"),
        ("population", "initial_energy", "3.0"),
        ("sources", "count", "4"),
        ("sources", "power_per_source", "120.0"),
        ("sources", "sigma_world_units", "3.0"),
        ("sources", "path_direction_interval_seconds", "2.0"),
        ("chemistry", "uptake_capacity_per_second", "8.0"),
        ("costs", "basal_energy_per_second", "0.6"),
        (
            "reproduction",
            "minimum_parent_energy_before_attempt",
            "2.2",
        ),
        ("reproduction", "maximum_attempt_rate_per_second", "4.0"),
        ("reproduction", "cooldown_seconds", "0.25"),
        ("execution", "command_durability", "\"volatile-in-memory\""),
    ])
}

fn profile() -> NumericalProfile {
    NumericalProfile::reference_f32().expect("profile")
}

/// A checkpoint of `config`'s world after each of `ticks`, with what a
/// restore of it must reproduce: its tick and digest.
fn checkpoints(config: &ValidatedConfig, ticks: &[u64]) -> Vec<(Vec<u8>, (u64, Sha256Digest))> {
    let book = CommandBook::new(SESSION, config.config_hash(), 16);
    let mut world =
        World::found(config, Arrangement::FreeList, profile()).expect("the world founds");
    let mut out = Vec::new();
    for &tick in ticks {
        while world.tick().get() < tick {
            match world.step_with(&[]).expect("the tick advances") {
                Stepped::Published(_) => {}
                Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
            }
        }
        let bytes = checkpoint(&world, &book).expect("checkpoints");
        out.push((bytes, (world.tick().get(), world.digest())));
    }
    out
}

/// What a restored checkpoint is: its world's tick and digest.
type Restored = (u64, Sha256Digest);

/// What a restored checkpoint is: its world's tick and digest.
fn restored(_sequence: u64, bytes: &[u8]) -> Result<(u64, Sha256Digest), CoreError> {
    restore(bytes, profile()).map(|session| (session.world.tick().get(), session.world.digest()))
}

fn valid(sequence: u64, bytes: &[u8]) -> Result<(), CoreError> {
    restored(sequence, bytes).map(|_| ())
}

/// The newest checkpoint that restores, and how many newer ones did not.
fn latest(store: &ChunkStore) -> (Option<(u64, Restored)>, Vec<String>) {
    let recovered = store.latest_valid(restored).expect("lists");
    let skipped = recovered
        .skipped
        .iter()
        .map(|skip| format!("{} {}: {}", skip.sequence, skip.kind, skip.reason))
        .collect();
    (recovered.latest, skipped)
}

/// A path for one test, with nothing left from an earlier run.
fn fresh(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("flife-store")
        .join(name);
    if path.is_dir() {
        fs::remove_dir_all(&path).expect("clears the last run's directory");
    } else if path.exists() {
        fs::remove_file(&path).expect("clears the last run's file");
    }
    fs::create_dir_all(path.parent().expect("a parent")).expect("creates the parent");
    path
}

fn temporaries(root: &Path) -> usize {
    fs::read_dir(root.join("tmp")).expect("tmp").count()
}

fn chunk_files(root: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for shard in fs::read_dir(root.join("chunks")).expect("chunks") {
        for chunk in fs::read_dir(shard.expect("a shard").path()).expect("a shard") {
            names.insert(
                chunk
                    .expect("a chunk")
                    .file_name()
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    names
}

fn digests(checkpoint: &[u8]) -> BTreeSet<String> {
    CheckpointManifest::parse(checkpoint)
        .expect("parses")
        .entries()
        .iter()
        .map(|entry| entry.digest.to_hex())
        .collect()
}

/// The digest of `family`'s chunk in `checkpoint`.
fn chunk_digest(checkpoint: &[u8], family: ChunkFamily) -> Sha256Digest {
    CheckpointManifest::parse(checkpoint)
        .expect("parses")
        .entries()
        .iter()
        .find(|entry| entry.family == family)
        .expect("the family")
        .digest
}

#[test]
fn published_checkpoints_load_back_byte_for_byte_and_restore() {
    let config = ecology();
    let [(a, a_world), (b, b_world)] = <[_; 2]>::try_from(checkpoints(&config, &[8, 16]))
        .unwrap_or_else(|_| panic!("two checkpoints"));
    for durability in BOTH {
        let root = fresh(&format!("roundtrip-{}", durability.name()));
        let mut store = ChunkStore::open(&root, durability).expect("creates");
        assert_eq!(
            fs::read_to_string(root.join("FLIFESTORE")).expect("a marker"),
            format!("{STORE_FORMAT}\n")
        );
        assert_eq!(latest(&store), (None, Vec::new()), "an empty store");
        let first = store.publish(&a).expect("publishes");
        let chunks = CheckpointManifest::parse(&a)
            .expect("parses")
            .entries()
            .len();
        assert_eq!(
            (first.sequence, first.written, first.shared, first.repaired),
            (1, chunks, 0, 0)
        );
        assert_eq!(store.load(1).expect("loads"), a, "byte for byte");
        let second = store.publish(&b).expect("publishes");
        let common = digests(&a).intersection(&digests(&b)).count();
        assert!(common >= 3, "configuration, random and topology are shared");
        assert_eq!(
            (second.sequence, second.shared, second.written),
            (2, common, chunks - common)
        );
        assert_eq!(store.load(2).expect("loads"), b);
        assert_eq!(store.load(1).expect("still loads"), a);

        // A damaged checkpoint is refused before anything is stored.
        let files = chunk_files(&root);
        let mut damaged = b.clone();
        let last = damaged.len() - 1;
        damaged[last] ^= 0x01;
        assert!(matches!(
            store.publish(&damaged),
            Err(CoreError::IncompatibleCheckpoint { .. })
        ));
        assert_eq!(store.sequences().expect("lists"), [1, 2]);
        assert_eq!(chunk_files(&root), files);
        assert_eq!(temporaries(&root), 0);
        assert_eq!(latest(&store), (Some((2, b_world)), Vec::new()));
        assert_eq!(
            chunk_files(&root),
            &digests(&a) | &digests(&b),
            "one file per distinct chunk"
        );

        // One writer at a time; the lock goes with the store.
        assert!(matches!(
            ChunkStore::open(&root, durability),
            Err(CoreError::StateConflict { .. })
        ));
        drop(store);
        let store = ChunkStore::open(&root, durability).expect("reopens");
        assert_eq!((store.next_sequence(), store.swept()), (3, 0));
        assert_eq!(latest(&store).0, Some((2, b_world)));
        assert_ne!(a_world, b_world);
    }

    let foreign = fresh("foreign");
    fs::create_dir_all(&foreign).expect("creates");
    fs::write(foreign.join("notes.txt"), b"not a store").expect("writes");
    assert!(matches!(
        ChunkStore::open(&foreign, Durability::ProcessCrash),
        Err(CoreError::StateConflict { .. })
    ));
    let other = fresh("other-format");
    drop(ChunkStore::open(&other, Durability::ProcessCrash).expect("creates"));
    fs::write(other.join("FLIFESTORE"), b"frankenlife.chunk-store.v9\n").expect("writes");
    assert!(matches!(
        ChunkStore::open(&other, Durability::ProcessCrash),
        Err(CoreError::SchemaMismatch { .. })
    ));
}

/// Run by [`a_crash_at_every_step_leaves_the_last_valid_checkpoint`] in a
/// child process: publish a checkpoint, and at the planned step report it
/// and wait to be killed. Does nothing in an ordinary test run.
#[test]
fn child_publishes_until_its_planned_crash() {
    let Ok(plan) = std::env::var(CHILD) else {
        return;
    };
    let parts: Vec<&str> = plan.split('|').collect();
    let [root, checkpoint, stop, durability] = parts[..] else {
        panic!("a plan of four parts: {plan}");
    };
    let stop: usize = stop.parse().expect("a step");
    let durability = BOTH
        .into_iter()
        .find(|candidate| candidate.name() == durability)
        .expect("a durability");
    let bytes = fs::read(checkpoint).expect("the checkpoint");
    let mut store = ChunkStore::open(root, durability).expect("opens");
    let mut count = 0;
    let mut out = std::io::stdout();
    store
        .publish_observed(&bytes, &mut |step| {
            if count == stop {
                writeln!(out, "FLIFE-STEP {step:?}").expect("reports");
                out.flush().expect("flushes");
                loop {
                    std::thread::sleep(Duration::from_secs(60));
                }
            }
            count += 1;
            Flow::Continue
        })
        .expect("publishes");
    println!("FLIFE-DONE");
}

/// Start a child that publishes `checkpoint` into `root` and kill it
/// (`SIGKILL`) right after step `stop`. The step, or `None` if the
/// publication has fewer steps. The child's stderr goes to a file beside
/// `root`, so a child that fails says why.
fn crash_at(root: &Path, checkpoint: &Path, stop: usize, durability: Durability) -> Option<String> {
    let stderr_path = root.with_extension("stderr");
    let stderr = fs::File::create(&stderr_path).expect("creates the child's stderr file");
    let mut child = Command::new(std::env::current_exe().expect("this test binary"))
        .args([
            "--exact",
            "child_publishes_until_its_planned_crash",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env(
            CHILD,
            format!(
                "{}|{}|{stop}|{}",
                root.display(),
                checkpoint.display(),
                durability.name()
            ),
        )
        .stdout(Stdio::piped())
        .stderr(stderr)
        .spawn()
        .expect("spawns");
    let stdout = child.stdout.take().expect("a pipe");
    for line in BufReader::new(stdout).lines() {
        let line = line.expect("a line");
        if let Some(at) = line.find("FLIFE-STEP ") {
            child.kill().expect("kills");
            child.wait().expect("reaps");
            return Some(line[at + "FLIFE-STEP ".len()..].trim().to_owned());
        }
        if line.contains("FLIFE-DONE") {
            assert!(child.wait().expect("waits").success());
            return None;
        }
    }
    let status = child.wait();
    panic!(
        "the child ended without a report: {status:?}; its stderr:\n{}",
        fs::read_to_string(&stderr_path).unwrap_or_else(|error| format!("unreadable: {error}"))
    );
}

/// §16.4: a publication killed after any step leaves the store with the
/// last valid checkpoint (the previous one before the manifest is linked,
/// the new one after), no damaged manifest, and no temporary file once
/// reopened; and the interrupted checkpoint can be published again.
#[test]
fn a_crash_at_every_step_leaves_the_last_valid_checkpoint() {
    let config = ecology();
    let [(a, a_world), (b, b_world)] = <[_; 2]>::try_from(checkpoints(&config, &[8, 16]))
        .unwrap_or_else(|_| panic!("two checkpoints"));
    let b_path = fresh("crash-b.ckpt");
    fs::write(&b_path, &b).expect("writes");
    for durability in BOTH {
        // Every step of publishing b into a store holding a.
        let mut expected = Vec::new();
        {
            let root = fresh(&format!("crash-steps-{}", durability.name()));
            let mut store = ChunkStore::open(&root, durability).expect("creates");
            store.publish(&a).expect("publishes");
            store
                .publish_observed(&b, &mut |step| {
                    expected.push(format!("{step:?}"));
                    Flow::Continue
                })
                .expect("publishes");
        }
        let mut killed = Vec::new();
        let mut after_publication = 0;
        for stop in 0.. {
            let root = fresh(&format!("crash-{}", durability.name()));
            ChunkStore::open(&root, durability)
                .expect("creates")
                .publish(&a)
                .expect("publishes");
            let Some(step) = crash_at(&root, &b_path, stop, durability) else {
                break;
            };
            let mut store = ChunkStore::open(&root, durability).expect("reopens after a kill");
            assert_eq!(temporaries(&root), 0, "{step}: temporaries swept");
            let (recovered, skipped) = latest(&store);
            assert!(skipped.is_empty(), "{step}: {skipped:?}");
            let published = step == "Published" || step == "DirectoryFlushed";
            after_publication += usize::from(published);
            let world = if published { b_world } else { a_world };
            assert_eq!(
                recovered.map(|(_, world)| world),
                Some(world),
                "killed after {step}"
            );
            store.publish(&b).expect("publishes again");
            assert_eq!(latest(&store).0.map(|(_, world)| world), Some(b_world));
            killed.push(step);
        }
        assert_eq!(
            killed,
            expected,
            "{}: a kill after every step",
            durability.name()
        );
        assert_eq!(
            after_publication,
            if durability == Durability::PowerLoss {
                2
            } else {
                1
            }
        );
        println!(
            "ok: {}: killed after each of {} steps; the last valid checkpoint survived each",
            durability.name(),
            killed.len()
        );
    }
}

/// A cancellation at any step before the manifest is linked leaves the
/// store as it was and removes the attempt's temporary files; from then on
/// it is too late and the checkpoint is published.
#[test]
fn cancellation_at_every_step_leaves_the_store_as_it_was() {
    let config = ecology();
    let [(a, a_world), (b, b_world)] = <[_; 2]>::try_from(checkpoints(&config, &[8, 16]))
        .unwrap_or_else(|_| panic!("two checkpoints"));
    for durability in BOTH {
        let mut cancelled = 0;
        for stop in 0.. {
            let root = fresh(&format!("cancel-{}", durability.name()));
            let mut store = ChunkStore::open(&root, durability).expect("creates");
            store.publish(&a).expect("publishes");
            let mut count = 0;
            let mut last = None;
            let outcome = store.publish_observed(&b, &mut |step| {
                let stop_here = count == stop;
                count += 1;
                if stop_here {
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
            let late = matches!(step, Step::Published | Step::DirectoryFlushed);
            if late {
                outcome.expect("too late to cancel: published");
                assert_eq!(latest(&store).0.map(|(_, world)| world), Some(b_world));
            } else {
                assert!(
                    matches!(outcome, Err(CoreError::Cancelled { .. })),
                    "{step:?}: {outcome:?}"
                );
                cancelled += 1;
                assert_eq!(temporaries(&root), 0, "{step:?}: no temporaries left");
                assert_eq!(store.sequences().expect("lists"), [1]);
                assert_eq!(latest(&store), (Some((1, a_world)), Vec::new()));
                store.publish(&b).expect("publishes after a cancellation");
                assert_eq!(latest(&store).0.map(|(_, world)| world), Some(b_world));
            }
        }
        assert!(cancelled >= 10, "{cancelled}");
        println!(
            "ok: {}: cancelled at each of {cancelled} steps; the store was unchanged each time",
            durability.name()
        );
    }
}

/// Flip one byte of a file.
fn flip(path: &Path, at: usize) {
    let mut bytes = fs::read(path).expect("reads");
    bytes[at] ^= 0x01;
    fs::write(path, bytes).expect("writes");
}

/// §16.7, B.2: a damaged or missing chunk or manifest makes its
/// checkpoint invalid, never a different world; recovery names why and
/// falls back; republishing repairs a damaged shared chunk.
#[test]
fn damage_falls_back_to_the_last_valid_checkpoint_and_publishing_repairs_it() {
    let config = ecology();
    let [(a, a_world), (b, b_world), (c, c_world)] =
        <[_; 3]>::try_from(checkpoints(&config, &[8, 16, 24]))
            .unwrap_or_else(|_| panic!("three checkpoints"));
    let root = fresh("damage");
    let mut store = ChunkStore::open(&root, Durability::ProcessCrash).expect("creates");
    store.publish(&a).expect("publishes");
    store.publish(&b).expect("publishes");
    let unique = store.chunk_path(&chunk_digest(&b, ChunkFamily::Particles));
    assert!(!digests(&a).contains(&chunk_digest(&b, ChunkFamily::Particles).to_hex()));
    let original = fs::read(&unique).expect("reads");
    let falls_back = |store: &ChunkStore, why: &str| {
        let (recovered, skipped) = latest(store);
        assert_eq!(recovered, Some((1, a_world)), "{why}");
        assert_eq!(skipped.len(), 1, "{why}: {skipped:?}");
        assert!(
            skipped[0].starts_with("2 incompatible-checkpoint"),
            "{why}: {skipped:?}"
        );
        skipped[0].clone()
    };

    flip(&unique, original.len() / 2);
    // The store's own check, before any restore sees the bytes.
    let particles = chunk_digest(&b, ChunkFamily::Particles);
    match store.load(2) {
        Err(CoreError::IncompatibleCheckpoint { reason }) => assert!(
            reason.contains(&format!("({particles}) hash mismatch")),
            "{reason}"
        ),
        other => panic!("loaded a damaged chunk: {other:?}"),
    }
    assert!(falls_back(&store, "a flipped chunk byte").contains("particles"));
    assert!(falls_back(&store, "a flipped chunk byte").contains("hash mismatch"));
    fs::write(&unique, &original[..original.len() / 2]).expect("truncates");
    assert!(falls_back(&store, "a truncated chunk").contains("the manifest declares"));
    fs::remove_file(&unique).expect("removes");
    assert!(falls_back(&store, "a missing chunk").contains("is missing"));
    fs::write(&unique, &original).expect("restores");
    assert_eq!(latest(&store).0, Some((2, b_world)));

    let manifest = store.manifest_path(2);
    let sealed = fs::read(&manifest).expect("reads");
    flip(&manifest, 40);
    assert!(falls_back(&store, "a flipped manifest byte").contains("hash mismatch"));
    fs::write(&manifest, &sealed[..sealed.len() - 1]).expect("truncates");
    falls_back(&store, "a truncated manifest");
    let mut longer = sealed.clone();
    longer.push(0);
    fs::write(&manifest, &longer).expect("extends");
    assert!(falls_back(&store, "a byte after the seal").contains("after its seal"));
    fs::write(&manifest, &sealed).expect("restores");

    // A manifest that declares a huge chunk, re-sealed so only the size is
    // wrong, is refused from the chunk's stored length before anything is
    // allocated for it.
    let mut huge = sealed.clone();
    let model = usize::from(u16::from_le_bytes([huge[13], huge[14]]));
    let mut at = 19 + model + 2;
    let count = CheckpointManifest::parse(&sealed)
        .expect("parses")
        .entries()
        .len();
    for _ in 1..count {
        at += 69 + usize::from(huge[at + 68]);
    }
    for field in [at + 20, at + 28] {
        huge[field..field + 8].copy_from_slice(&(1_u64 << 60).to_le_bytes());
    }
    let seal = huge.len() - 32;
    let digest = Sha256Digest::of(&huge[..seal]);
    huge[seal..].copy_from_slice(digest.as_bytes());
    fs::write(store.manifest_path(3), &huge).expect("writes");
    let (recovered, skipped) = latest(&store);
    assert_eq!(recovered, Some((2, b_world)));
    assert!(
        skipped[0].contains("the manifest declares 1152921504606846976"),
        "{skipped:?}"
    );
    fs::write(store.manifest_path(4), b"not a manifest").expect("writes");
    assert_eq!(latest(&store).1.len(), 2, "both newer ones skipped");
    drop(store);
    let mut store = ChunkStore::open(&root, Durability::ProcessCrash).expect("reopens");
    assert_eq!(
        store.next_sequence(),
        5,
        "an invalid manifest's number is not reused"
    );

    // A damaged chunk both checkpoints share leaves none valid; the next
    // publication that needs it stores it again, which repairs both.
    let shared = store.chunk_path(&chunk_digest(&a, ChunkFamily::Configuration));
    flip(&shared, 3);
    let (recovered, skipped) = latest(&store);
    assert_eq!((recovered, skipped.len()), (None, 4));
    let publication = store.publish(&c).expect("publishes");
    assert_eq!(publication.repaired, 1);
    assert_eq!(latest(&store).0, Some((5, c_world)));
    assert_eq!(
        restored(2, &store.load(2).expect("repaired")).expect("restores"),
        b_world
    );
    assert_eq!(
        restored(1, &store.load(1).expect("repaired")).expect("restores"),
        a_world
    );
}

/// Run by [`a_write_that_fails_part_way_leaves_the_last_valid_checkpoint`]
/// in a child process whose file size limit is far below a chunk's: the
/// publication fails while writing, and no temporary file is left.
#[test]
fn child_publishes_under_a_file_size_limit() {
    let Ok(plan) = std::env::var(CHILD_LIMITED) else {
        return;
    };
    let parts: Vec<&str> = plan.split('|').collect();
    let [root, checkpoint, durability] = parts[..] else {
        panic!("a plan of three parts: {plan}");
    };
    let durability = BOTH
        .into_iter()
        .find(|candidate| candidate.name() == durability)
        .expect("a durability");
    let bytes = fs::read(checkpoint).expect("the checkpoint");
    let mut store = ChunkStore::open(root, durability).expect("opens");
    let refused = store.publish(&bytes);
    let tmp = temporaries(Path::new(root));
    match refused {
        Err(CoreError::StorageFailed {
            operation: "write temporary file",
            reason,
            ..
        }) => println!("FLIFE-REFUSED {tmp} {reason}"),
        other => println!("FLIFE-UNEXPECTED {other:?}"),
    }
}

/// A write the host refuses part way through a chunk (the file size limit
/// stands in for a full disk: the same short write) fails the publication
/// with `storage-failed`, removes the partial file at once, and leaves the
/// last valid checkpoint.
#[test]
fn a_write_that_fails_part_way_leaves_the_last_valid_checkpoint() {
    let config = ecology();
    let [(a, a_world), (b, _)] = <[_; 2]>::try_from(checkpoints(&config, &[8, 16]))
        .unwrap_or_else(|_| panic!("two checkpoints"));
    let b_path = fresh("limited-b.ckpt");
    fs::write(&b_path, &b).expect("writes");
    let largest = CheckpointManifest::parse(&b)
        .expect("parses")
        .entries()
        .iter()
        .filter(|entry| !digests(&a).contains(&entry.digest.to_hex()))
        .map(|entry| entry.length)
        .max()
        .expect("a new chunk");
    assert!(
        largest > 64 * 1024,
        "a new chunk above the limit: {largest}"
    );
    for durability in BOTH {
        let root = fresh(&format!("limited-{}", durability.name()));
        ChunkStore::open(&root, durability)
            .expect("creates")
            .publish(&a)
            .expect("publishes");
        // 64 blocks: 32 KiB or 64 KiB depending on the shell's unit.
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg("trap '' XFSZ; ulimit -f 64; exec \"$0\" \"$@\"")
            .arg(std::env::current_exe().expect("this test binary"))
            .args([
                "--exact",
                "child_publishes_under_a_file_size_limit",
                "--nocapture",
                "--test-threads",
                "1",
            ])
            .env(
                CHILD_LIMITED,
                format!(
                    "{}|{}|{}",
                    root.display(),
                    b_path.display(),
                    durability.name()
                ),
            )
            .output()
            .expect("runs");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let report = stdout
            .lines()
            .find_map(|line| line.find("FLIFE-").map(|at| &line[at..]))
            .unwrap_or_else(|| {
                panic!(
                    "no report ({}): {stdout}\nstderr:\n{}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                )
            });
        assert!(
            report.starts_with("FLIFE-REFUSED 0 "),
            "{}: {report}",
            durability.name()
        );
        let store = ChunkStore::open(&root, durability).expect("reopens");
        assert_eq!(store.swept(), 0, "nothing left to sweep");
        assert_eq!(latest(&store), (Some((1, a_world)), Vec::new()));
        println!("ok: {}: {report}", durability.name());
    }
}

/// Device and path failures: an operation the host refuses fails the
/// publication with `storage-failed` and leaves the last valid checkpoint.
#[test]
fn refused_storage_operations_leave_the_last_valid_checkpoint() {
    let config = ecology();
    let [(a, a_world), (b, b_world)] = <[_; 2]>::try_from(checkpoints(&config, &[8, 16]))
        .unwrap_or_else(|_| panic!("two checkpoints"));
    for durability in BOTH {
        let root = fresh(&format!("refused-{}", durability.name()));
        let mut store = ChunkStore::open(&root, durability).expect("creates");
        store.publish(&a).expect("publishes");

        // A directory where one of b's chunks goes: storing it fails.
        let blocked = store.chunk_path(&chunk_digest(&b, ChunkFamily::Particles));
        fs::create_dir_all(blocked.join("occupied")).expect("blocks");
        let refused = store.publish(&b);
        assert!(
            matches!(
                &refused,
                Err(CoreError::StorageFailed {
                    operation: "store chunk",
                    ..
                })
            ),
            "{refused:?}"
        );
        assert_eq!(temporaries(&root), 0);
        assert_eq!(latest(&store), (Some((1, a_world)), Vec::new()));
        fs::remove_dir_all(&blocked).expect("unblocks");

        // A directory where the next manifest goes: publishing it fails.
        let next = store.manifest_path(store.next_sequence());
        fs::create_dir_all(next.join("occupied")).expect("blocks");
        let refused = store.publish(&b);
        assert!(
            matches!(
                &refused,
                Err(CoreError::StorageFailed {
                    operation: "publish manifest",
                    ..
                })
            ),
            "{refused:?}"
        );
        assert_eq!(temporaries(&root), 0);
        assert_eq!(
            latest(&store).0,
            Some((1, a_world)),
            "the blocked one is skipped"
        );
        fs::remove_dir_all(&next).expect("unblocks");
        let publication = store.publish(&b).expect("publishes once unblocked");
        assert_eq!(latest(&store).0, Some((publication.sequence, b_world)));
    }
}

/// §16.7: retention keeps the newest valid checkpoints whole and removes
/// older manifests and exactly the chunks no remaining manifest names.
#[test]
fn retention_keeps_the_newest_valid_checkpoints_and_the_chunks_they_name() {
    let config = ecology();
    let published = checkpoints(&config, &[0, 4, 8, 12, 16, 20]);
    let root = fresh("retain");
    let mut store = ChunkStore::open(&root, Durability::ProcessCrash).expect("creates");
    for (bytes, _) in &published {
        store.publish(bytes).expect("publishes");
    }
    let before = chunk_files(&root);
    assert!(matches!(
        store.retain(0, valid),
        Err(CoreError::StateConflict { .. })
    ));
    let everything = store.retain(10, valid).expect("retains");
    assert!(everything.removed.is_empty(), "fewer valid than the limit");
    assert_eq!(chunk_files(&root), before);

    let retention = store.retain(4, valid).expect("retains");
    assert_eq!(
        (retention.valid, retention.removed),
        (vec![6, 5, 4, 3], vec![1, 2])
    );
    let kept: BTreeSet<String> = published[2..]
        .iter()
        .flat_map(|(bytes, _)| digests(bytes))
        .collect();
    assert_eq!(
        chunk_files(&root),
        kept,
        "exactly the chunks the kept ones name"
    );
    assert_eq!(retention.chunks_kept, kept.len());
    assert_eq!(retention.chunks_removed, before.len() - kept.len());
    assert!(retention.chunks_removed > 0);
    for (sequence, (bytes, world)) in (3..).zip(&published[2..]) {
        assert_eq!(&store.load(sequence).expect("loads"), bytes);
        assert_eq!(restored(sequence, bytes).expect("restores"), *world);
    }

    // The newest is damaged: it is not counted, and it stays.
    let damaged = store.chunk_path(&chunk_digest(&published[5].0, ChunkFamily::Particles));
    flip(&damaged, 0);
    let retention = store.retain(2, valid).expect("retains");
    assert_eq!((retention.valid, retention.removed), (vec![5, 4], vec![3]));
    assert_eq!(store.sequences().expect("lists"), [4, 5, 6]);
    let (recovered, skipped) = latest(&store);
    assert_eq!(recovered, Some((5, published[4].1)));
    assert_eq!(skipped.len(), 1);
}

/// The 4,096-organism preset at its scale, under both durability
/// profiles: publish, share unchanged chunks with the next tick's
/// checkpoint, and recover.
#[test]
fn the_reference_preset_publishes_and_recovers_at_its_scale() {
    let config = ValidatedConfig::from_toml(PRESET).expect("the preset validates");
    let [(first, first_world), (second, second_world)] =
        <[_; 2]>::try_from(checkpoints(&config, &[64, 65]))
            .unwrap_or_else(|_| panic!("two checkpoints"));
    for durability in BOTH {
        let root = fresh(&format!("preset-{}", durability.name()));
        let mut store = ChunkStore::open(&root, durability).expect("creates");
        let clock = Instant::now();
        let one = store.publish(&first).expect("publishes");
        let written = clock.elapsed();
        let clock = Instant::now();
        let two = store.publish(&second).expect("publishes");
        let next = clock.elapsed();
        let clock = Instant::now();
        let (recovered, skipped) = latest(&store);
        let read = clock.elapsed();
        assert_eq!(recovered, Some((2, second_world)));
        assert!(skipped.is_empty());
        assert_eq!(store.load(1).expect("loads"), first);
        assert_ne!(first_world, second_world);
        println!(
            "ok: {}: {:.1} MiB published in {written:?} ({} chunks); the next tick's in {next:?} \
             ({} shared, {:.1} MiB written); loaded, verified and restored in {read:?}",
            durability.name(),
            one.bytes_written as f64 / 1_048_576.0,
            one.written,
            two.shared,
            two.bytes_written as f64 / 1_048_576.0,
        );
    }
}

// ---- FL-038: streamed publication (plan §16.4, §16.5) ----

/// `config`'s world and book at committed tick `tick`.
fn world_at(config: &ValidatedConfig, tick: u64) -> (World, CommandBook) {
    let book = CommandBook::new(SESSION, config.config_hash(), 16);
    let mut world =
        World::found(config, Arrangement::FreeList, profile()).expect("the world founds");
    while world.tick().get() < tick {
        match world.step_with(&[]).expect("the tick advances") {
            Stepped::Published(_) => {}
            Stepped::Paused(shortfall) => panic!("paused: {shortfall:?}"),
        }
    }
    (world, book)
}

/// Stream `world`'s checkpoint into `store` and commit it.
fn stream_into(
    store: &mut ChunkStore,
    world: &World,
    book: &CommandBook,
    observe: Observer,
) -> Result<Publication, CoreError> {
    let mut stream = store.stream_observed(observe)?;
    let sealed = checkpoint_into(world, book, &mut stream)?;
    store.commit(stream, &sealed)
}

fn continue_all() -> Observer {
    Box::new(|_| Flow::Continue)
}

/// FL-038: a checkpoint streamed into the store as it is encoded is stored
/// exactly as one published from the whole container, under both
/// profiles: the same steps, publication, manifest and chunk files,
/// loading back to the in-memory container byte for byte; and
/// [`ChunkStore::verify`] returns that container's digest without holding
/// it.
#[test]
fn a_streamed_checkpoint_is_stored_as_the_whole_one_byte_for_byte() {
    for durability in BOTH {
        streams_as_the_whole_one(durability);
    }
}

fn streams_as_the_whole_one(durability: Durability) {
    let config = ecology();
    let streamed_root = fresh(&format!("stream-identity-{}", durability.name()));
    let whole_root = fresh(&format!("stream-identity-whole-{}", durability.name()));
    let mut streamed = ChunkStore::open(&streamed_root, durability).expect("creates");
    let mut whole = ChunkStore::open(&whole_root, durability).expect("creates");
    for (round, tick) in [0_u64, 8, 16].into_iter().enumerate() {
        let (world, book) = world_at(&config, tick);
        let bytes = checkpoint(&world, &book).expect("checkpoints");
        let steps = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = std::sync::Arc::clone(&steps);
        let by_stream = stream_into(
            &mut streamed,
            &world,
            &book,
            Box::new(move |step| {
                seen.lock().expect("unpoisoned").push(format!("{step:?}"));
                Flow::Continue
            }),
        )
        .expect("streams");
        let mut whole_steps = Vec::new();
        let by_whole = whole
            .publish_observed(&bytes, &mut |step| {
                whole_steps.push(format!("{step:?}"));
                Flow::Continue
            })
            .expect("publishes");
        assert_eq!(by_stream, by_whole, "tick {tick}: the same publication");
        assert_eq!(
            *steps.lock().expect("unpoisoned"),
            whole_steps,
            "tick {tick}: the same steps"
        );
        if round > 0 {
            assert!(by_stream.shared > 0, "tick {tick}: unchanged chunks shared");
        }
        assert_eq!(streamed.load(by_stream.sequence).expect("loads"), bytes);
        assert_eq!(
            streamed.verify(by_stream.sequence).expect("verifies"),
            Sha256Digest::of(&bytes)
        );
        assert_eq!(
            fs::read(streamed.manifest_path(by_stream.sequence)).expect("reads"),
            fs::read(whole.manifest_path(by_whole.sequence)).expect("reads"),
            "tick {tick}: the same manifest"
        );
        assert_eq!(temporaries(&streamed_root), 0);
    }
    assert_eq!(chunk_files(&streamed_root), chunk_files(&whole_root));
}

/// A cancellation at any step of a streamed publication before its
/// manifest is linked leaves the store as it was and removes the
/// attempt's temporary files; from then on the checkpoint is published.
#[test]
fn cancellation_at_every_streamed_step_leaves_the_store_as_it_was() {
    let config = ecology();
    let (a_world, a_book) = world_at(&config, 8);
    let (b_world, b_book) = world_at(&config, 16);
    let a = checkpoint(&a_world, &a_book).expect("checkpoints");
    let b_restored = (16, b_world.digest());
    for durability in BOTH {
        let mut cancelled = 0;
        for stop in 0.. {
            let root = fresh(&format!("stream-cancel-{}", durability.name()));
            let mut store = ChunkStore::open(&root, durability).expect("creates");
            store.publish(&a).expect("publishes");
            let last = std::sync::Arc::new(std::sync::Mutex::new(None));
            let seen = std::sync::Arc::clone(&last);
            let mut count = 0;
            let outcome = stream_into(
                &mut store,
                &b_world,
                &b_book,
                Box::new(move |step| {
                    let stop_here = count == stop;
                    count += 1;
                    if stop_here {
                        *seen.lock().expect("unpoisoned") = Some(step);
                        Flow::Cancel
                    } else {
                        Flow::Continue
                    }
                }),
            );
            let Some(step) = *last.lock().expect("unpoisoned") else {
                outcome.expect("publishes");
                break;
            };
            if matches!(step, Step::Published | Step::DirectoryFlushed) {
                outcome.expect("too late to cancel: published");
                assert_eq!(latest(&store).0.map(|(_, world)| world), Some(b_restored));
            } else {
                assert!(
                    matches!(outcome, Err(CoreError::Cancelled { .. })),
                    "{step:?}: {outcome:?}"
                );
                cancelled += 1;
                assert_eq!(temporaries(&root), 0, "{step:?}: no temporaries left");
                assert_eq!(store.sequences().expect("lists"), [1]);
                assert_eq!(latest(&store).0.map(|(sequence, _)| sequence), Some(1));
                stream_into(&mut store, &b_world, &b_book, continue_all())
                    .expect("streams after a cancellation");
                assert_eq!(latest(&store).0.map(|(_, world)| world), Some(b_restored));
            }
        }
        assert!(cancelled >= 10, "{cancelled}");
        println!(
            "ok: {}: a streamed publication cancelled at each of {cancelled} steps; the store \
             was unchanged each time",
            durability.name()
        );
    }
}

/// Run by [`a_crash_at_every_streamed_step_leaves_the_last_valid_checkpoint`]
/// in a child process: stream the ecology's tick-16 checkpoint, and at the
/// planned step report it and wait to be killed. Does nothing in an
/// ordinary test run.
#[test]
fn child_streams_until_its_planned_crash() {
    let Ok(plan) = std::env::var(STREAM_CHILD) else {
        return;
    };
    let parts: Vec<&str> = plan.split('|').collect();
    let [root, stop, durability] = parts[..] else {
        panic!("a plan of three parts: {plan}");
    };
    let stop: usize = stop.parse().expect("a step");
    let durability = BOTH
        .into_iter()
        .find(|candidate| candidate.name() == durability)
        .expect("a durability");
    let (world, book) = world_at(&ecology(), 16);
    let mut store = ChunkStore::open(root, durability).expect("opens");
    let mut count = 0;
    stream_into(
        &mut store,
        &world,
        &book,
        Box::new(move |step| {
            if count == stop {
                let mut out = std::io::stdout();
                writeln!(out, "FLIFE-STEP {step:?}").expect("reports");
                out.flush().expect("flushes");
                loop {
                    std::thread::sleep(Duration::from_secs(60));
                }
            }
            count += 1;
            Flow::Continue
        }),
    )
    .expect("streams");
    println!("FLIFE-DONE");
}

/// §16.4 for a streamed publication: killed (`SIGKILL`) after any step, the
/// store keeps the last valid checkpoint (the previous one before the
/// manifest is linked, the new one after), and once reopened has no
/// temporary file; the interrupted checkpoint streams again.
#[test]
fn a_crash_at_every_streamed_step_leaves_the_last_valid_checkpoint() {
    let config = ecology();
    let (a_world, a_book) = world_at(&config, 8);
    let (b_world, b_book) = world_at(&config, 16);
    let a = checkpoint(&a_world, &a_book).expect("checkpoints");
    let (a_restored, b_restored) = ((8, a_world.digest()), (16, b_world.digest()));
    for durability in BOTH {
        let mut expected = Vec::new();
        {
            let root = fresh(&format!("stream-crash-steps-{}", durability.name()));
            let mut store = ChunkStore::open(&root, durability).expect("creates");
            store.publish(&a).expect("publishes");
            let steps = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let seen = std::sync::Arc::clone(&steps);
            stream_into(
                &mut store,
                &b_world,
                &b_book,
                Box::new(move |step| {
                    seen.lock().expect("unpoisoned").push(format!("{step:?}"));
                    Flow::Continue
                }),
            )
            .expect("streams");
            expected.extend(steps.lock().expect("unpoisoned").iter().cloned());
        }
        let mut killed = Vec::new();
        for stop in 0.. {
            let root = fresh(&format!("stream-crash-{}", durability.name()));
            ChunkStore::open(&root, durability)
                .expect("creates")
                .publish(&a)
                .expect("publishes");
            let Some(step) = stream_crash_at(&root, stop, durability) else {
                break;
            };
            let mut store = ChunkStore::open(&root, durability).expect("reopens after a kill");
            assert_eq!(temporaries(&root), 0, "{step}: temporaries swept");
            let (recovered, skipped) = latest(&store);
            assert!(skipped.is_empty(), "{step}: {skipped:?}");
            let published = step == "Published" || step == "DirectoryFlushed";
            let world = if published { b_restored } else { a_restored };
            assert_eq!(
                recovered.map(|(_, world)| world),
                Some(world),
                "killed after {step}"
            );
            stream_into(&mut store, &b_world, &b_book, continue_all()).expect("streams again");
            assert_eq!(latest(&store).0.map(|(_, world)| world), Some(b_restored));
            killed.push(step);
        }
        assert_eq!(
            killed,
            expected,
            "{}: a kill after every step",
            durability.name()
        );
        println!(
            "ok: {}: a streamed publication killed after each of {} steps; the last valid \
             checkpoint survived each",
            durability.name(),
            killed.len()
        );
    }
}

/// Start a child that streams the ecology's tick-16 checkpoint into `root`
/// and kill it right after step `stop`; the step, or `None` if the
/// publication has fewer steps.
fn stream_crash_at(root: &Path, stop: usize, durability: Durability) -> Option<String> {
    let stderr_path = root.with_extension("stderr");
    let stderr = fs::File::create(&stderr_path).expect("creates the child's stderr file");
    let mut child = Command::new(std::env::current_exe().expect("this test binary"))
        .args([
            "--exact",
            "child_streams_until_its_planned_crash",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env(
            STREAM_CHILD,
            format!("{}|{stop}|{}", root.display(), durability.name()),
        )
        .stdout(Stdio::piped())
        .stderr(stderr)
        .spawn()
        .expect("spawns");
    let stdout = child.stdout.take().expect("a pipe");
    for line in BufReader::new(stdout).lines() {
        let line = line.expect("a line");
        if let Some(at) = line.find("FLIFE-STEP ") {
            child.kill().expect("kills");
            child.wait().expect("reaps");
            return Some(line[at + "FLIFE-STEP ".len()..].trim().to_owned());
        }
        if line.contains("FLIFE-DONE") {
            assert!(child.wait().expect("waits").success());
            return None;
        }
    }
    let status = child.wait();
    panic!(
        "the child ended without a report: {status:?}; its stderr:\n{}",
        fs::read_to_string(&stderr_path).unwrap_or_else(|error| format!("unreadable: {error}"))
    );
}

/// A commit publishes only the checkpoint its stream stored: a sealed
/// prefix naming other chunks, a stream of another store, a stream whose
/// chunk was removed before the commit, a stream that stored only some of
/// the sealed chunks, one with a chunk still open, and a stream that
/// failed are each refused, and nothing is published; a chunk begun out of
/// order is refused.
#[test]
fn a_commit_refuses_what_its_stream_did_not_store() {
    let config = ecology();
    let (a_world, a_book) = world_at(&config, 8);
    let (b_world, b_book) = world_at(&config, 16);
    let root = fresh("stream-refusals");
    let other_root = fresh("stream-refusals-other");
    let mut store = ChunkStore::open(&root, Durability::ProcessCrash).expect("creates");
    let mut other = ChunkStore::open(&other_root, Durability::ProcessCrash).expect("creates");

    // Another checkpoint's sealed prefix.
    let mut stream = store.stream().expect("streams");
    checkpoint_into(&a_world, &a_book, &mut stream).expect("encodes");
    let mut elsewhere = other.stream().expect("streams");
    let b_sealed = checkpoint_into(&b_world, &b_book, &mut elsewhere).expect("encodes");
    let refused = store.commit(stream, &b_sealed);
    assert!(
        matches!(refused, Err(CoreError::IncompatibleCheckpoint { .. })),
        "{refused:?}"
    );

    // A stream of another store.
    let refused = store.commit(elsewhere, &b_sealed);
    assert!(
        matches!(refused, Err(CoreError::StateConflict { .. })),
        "{refused:?}"
    );

    // A chunk removed after it was stored.
    let mut stream = store.stream().expect("streams");
    let sealed = checkpoint_into(&a_world, &a_book, &mut stream).expect("encodes");
    let field = CheckpointManifest::parse(&sealed)
        .expect("parses")
        .entries()
        .iter()
        .find(|entry| entry.family == ChunkFamily::Field)
        .expect("a field chunk")
        .digest;
    fs::remove_file(store.chunk_path(&field)).expect("removes the chunk");
    let refused = store.commit(stream, &sealed);
    assert!(
        matches!(refused, Err(CoreError::IncompatibleCheckpoint { .. })),
        "{refused:?}"
    );

    // A stream that stored only the first chunk of the checkpoint sealed,
    // fed by hand; and a stream whose chunks begin out of order.
    let whole = checkpoint(&a_world, &a_book).expect("checkpoints");
    let manifest = CheckpointManifest::parse(&whole).expect("parses");
    let first = &manifest.entries()[0];
    let start = manifest.sealed_length() + usize::try_from(first.offset).expect("fits");
    let end = start + usize::try_from(first.length).expect("fits");
    let mut partial = store.stream().expect("streams");
    partial.begin(0, first.family).expect("begins");
    partial.page(&whole[start..end]).expect("writes");
    partial.end(first.length, first.digest).expect("ends");
    let refused = store.commit(partial, &whole[..manifest.sealed_length()]);
    assert!(
        matches!(refused, Err(CoreError::IncompatibleCheckpoint { .. })),
        "{refused:?}"
    );
    let mut disordered = store.stream().expect("streams");
    let refused = disordered.begin(1, manifest.entries()[1].family);
    assert!(
        matches!(refused, Err(CoreError::StateConflict { .. })),
        "{refused:?}"
    );
    // A stream committed with its first chunk still open.
    let mut unfinished = store.stream().expect("streams");
    unfinished.begin(0, first.family).expect("begins");
    unfinished.page(&whole[start..end]).expect("writes");
    let refused = store.commit(unfinished, &whole[..manifest.sealed_length()]);
    assert!(
        matches!(&refused, Err(CoreError::StateConflict { detail, .. }) if detail.contains("open")),
        "{refused:?}"
    );

    // A stream the observer cancelled stays cancelled.
    let mut stream = store
        .stream_observed(Box::new(|_| Flow::Cancel))
        .expect("streams");
    let failed = checkpoint_into(&a_world, &a_book, &mut stream);
    assert!(
        matches!(failed, Err(CoreError::Cancelled { .. })),
        "{failed:?}"
    );
    let sealed = {
        let mut elsewhere = other.stream().expect("streams");
        checkpoint_into(&a_world, &a_book, &mut elsewhere).expect("encodes")
    };
    let refused = store.commit(stream, &sealed);
    assert!(
        matches!(refused, Err(CoreError::Cancelled { .. })),
        "{refused:?}"
    );

    assert!(
        store.sequences().expect("lists").is_empty(),
        "nothing published"
    );
    assert_eq!(temporaries(&root), 0);
    let published = stream_into(&mut store, &a_world, &a_book, continue_all()).expect("streams");
    assert_eq!(
        store.load(published.sequence).expect("loads"),
        checkpoint(&a_world, &a_book).expect("checkpoints")
    );
}

/// FL-038's retention counts a checkpoint valid when it verifies (seal,
/// lengths and hashes, read a buffer at a time) rather than when it
/// decodes. On damaged and undamaged stores it keeps and removes exactly
/// what the decoding retention does.
#[test]
fn retention_by_verification_decides_as_retention_by_restoring() {
    let config = ecology();
    let published = checkpoints(&config, &[0, 4, 8, 12, 16, 20]);
    let make = |name: &str| {
        let root = fresh(name);
        let mut store = ChunkStore::open(&root, Durability::ProcessCrash).expect("creates");
        for (bytes, _) in &published {
            store.publish(bytes).expect("publishes");
        }
        (root, store)
    };
    for damage in [None, Some(5_usize), Some(3)] {
        let (intact_root, mut by_hash) = make("retain-intact-hash");
        let (decode_root, mut by_decode) = make("retain-intact-decode");
        if let Some(index) = damage {
            let digest = chunk_digest(&published[index].0, ChunkFamily::Particles);
            flip(&by_hash.chunk_path(&digest), 0);
            flip(&by_decode.chunk_path(&digest), 0);
        }
        for keep in [4, 2] {
            let hashed = by_hash.retain_intact(keep).expect("retains");
            let decoded = by_decode.retain(keep, valid).expect("retains");
            assert_eq!(hashed, decoded, "damage {damage:?}, keep {keep}");
            assert_eq!(chunk_files(&intact_root), chunk_files(&decode_root));
        }
    }
}
