//! FL-039 evidence (plan §14.4, §15.3, §16.3, §16.7, Appendix B.1): a run
//! directory branched at a durable checkpoint into a control and an
//! intervention branch.
//!
//! - Shared immutable closure: each branch's founding checkpoint links the
//!   parent's verified chunk files (the same inode under two names: field,
//!   genomes, particles and the rest), writing only its command closure;
//!   the parent's retention later removes its own names and the branches
//!   still load and resume.
//! - Independent command histories: each branch is its own session, whose
//!   journal starts with exactly the history it inherited; the control
//!   branch, with the parent's pending commands, reaches the uninterrupted
//!   run's state and outcomes; the intervention branch's brush is in its
//!   journal alone, refused by the other session, and its world matches the
//!   control's until the brush and differs after.

#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use flife_core::canonical::Sha256Digest;
use flife_core::ids::{CommandId, SessionId};
use flife_core::{Tick, ValidatedConfig};
use flife_engine::{Intervention, NumericalProfile};
use flife_runtime::{
    Admission, Command as ClientCommand, Durability as CommandDurability, Journal, JournalContents,
    Rejection, Target, restore,
};
use flife_store::{CommandJournal, Durability};
use serde_json::{Value, json};

const PLAN: &str = include_str!("../../../COMPREHENSIVE_PLAN_FOR_THE_DESIGN_OF_FRANKENLIFE.md");

/// The ticks every run here reaches.
const HORIZON: u64 = 240;

/// Energy brushes at ticks 3, 66, 150 and 230: commands 1 to 4.
const INJECTIONS: [&str; 4] = ["3:5,6:2.0", "66:9,2:3.0", "150:20,11:4.0", "230:1,30:1.0"];

/// Where the parents stop, and the checkpoint the branches start from.
const PARENT_TICKS: u64 = 120;
const FORK: u64 = 112;

/// The intervention branch's brush.
const BRUSH: &str = "130:9,2:30.0";

/// Appendix A with `extra` overrides.
fn appendix_a(extra: &[(&str, &str, &str)]) -> String {
    let appendix = PLAN.find("## Appendix A").map_or("", |at| &PLAN[at..]);
    let start = appendix
        .find("```toml\n")
        .map_or(0, |at| at + "```toml\n".len());
    let length = appendix[start..].find("```").unwrap_or(0);
    let mut lines: Vec<String> = appendix[start..start + length]
        .lines()
        .map(str::to_owned)
        .collect();
    for &(section, key, value) in extra {
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
    lines.join("\n")
}

/// FL-037's 32 × 32 ecology of 48 founders, with `durability`, written to
/// a file.
fn ecology(durability: &str) -> PathBuf {
    let text = appendix_a(&[
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
        ("controller", "evaluation_interval_ticks", "4"),
        (
            "execution",
            "command_durability",
            &format!("\"{durability}\""),
        ),
    ]);
    let path = scratch().join(format!("ecology-{durability}.toml"));
    write_whole(&path, &text);
    path
}

/// Write `text` to `path` through a uniquely named temporary file renamed
/// into place. Tests run in parallel and several write the same
/// configuration path: written in place, a `flife` started by another test
/// could read it truncated (seen on Linux: "missing field `schema`").
fn write_whole(path: &Path, text: &str) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temporary = path.with_extension(format!(
        "{}.{}.partial",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&temporary, text).expect("writes the configuration");
    fs::rename(&temporary, path).expect("renames it into place");
}

fn config_hash(config: &Path) -> Sha256Digest {
    ValidatedConfig::from_toml(&fs::read_to_string(config).expect("reads"))
        .expect("valid")
        .config_hash()
}

fn scratch() -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("flife-branch");
    fs::create_dir_all(&path).expect("creates the scratch directory");
    path
}

/// A run directory path that does not exist yet.
fn fresh(name: &str) -> PathBuf {
    let path = scratch().join(name);
    if path.exists() {
        fs::remove_dir_all(&path).expect("clears the last run's directory");
    }
    path
}

fn text(path: &Path) -> &str {
    path.to_str().expect("UTF-8")
}

fn lines(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line is one JSON record"))
        .collect()
}

/// Run `flife` to completion: its status, stdout records and stderr
/// records.
fn flife(args: &[&str]) -> (Option<i32>, Vec<Value>, Vec<Value>) {
    let output = common::output(Command::new(env!("CARGO_BIN_EXE_flife")).args(args));
    (
        output.status.code(),
        lines(&output.stdout),
        lines(&output.stderr),
    )
}

/// `flife run` of `config` to `ticks` in `run_dir` with [`INJECTIONS`],
/// checkpointing every 7 ticks.
fn run(config: &Path, run_dir: &Path, ticks: u64) -> Vec<Value> {
    let ticks = ticks.to_string();
    let mut args = vec![
        "run",
        text(config),
        "--ticks",
        &ticks,
        "--every",
        "0",
        "--run-dir",
        text(run_dir),
        "--checkpoint-every",
        "7",
        "--checkpoint-durability",
        "process-crash",
    ];
    for injection in INJECTIONS {
        args.extend(["--inject", injection]);
    }
    let (status, out, err) = flife(&args);
    assert_eq!(status, Some(0), "{err:?}");
    out
}

/// `flife resume` of `run_dir` to `until`, checkpointing every 7 ticks.
fn resume(run_dir: &Path, until: u64) -> Vec<Value> {
    let (status, out, err) = flife(&[
        "resume",
        text(run_dir),
        "--until",
        &until.to_string(),
        "--every",
        "0",
        "--checkpoint-every",
        "7",
        "--checkpoint-durability",
        "process-crash",
    ]);
    assert_eq!(status, Some(0), "{err:?}");
    out
}

/// `flife branch` of `parent` at `at` into `run_dir` with `extra` flags:
/// the branch record.
fn branch(parent: &Path, run_dir: &Path, at: u64, extra: &[&str]) -> Value {
    let at = at.to_string();
    let mut args = vec![
        "branch",
        text(parent),
        text(run_dir),
        "--at",
        &at,
        "--checkpoint-durability",
        "process-crash",
    ];
    args.extend(extra);
    let (status, out, err) = flife(&args);
    assert_eq!(status, Some(0), "{err:?}");
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["record"], "branch");
    out[0].clone()
}

/// The one record of `kind`.
fn one<'a>(records: &'a [Value], kind: &str) -> &'a Value {
    let found: Vec<&Value> = records
        .iter()
        .filter(|record| record["record"] == kind)
        .collect();
    assert_eq!(found.len(), 1, "one {kind} record in {records:?}");
    found[0]
}

/// The checkpoint record at `tick`.
fn checkpoint_at(records: &[Value], tick: u64) -> &Value {
    records
        .iter()
        .find(|record| record["record"] == "checkpoint" && record["tick"] == tick)
        .unwrap_or_else(|| panic!("a checkpoint at tick {tick}"))
}

/// The committed digest a run reached, checking it reached `tick`.
fn reached(records: &[Value], tick: u64) -> Value {
    let end = one(records, "end");
    assert_eq!(end["stop"]["end"], "reached");
    assert_eq!(end["tick"], tick);
    end["digest"].clone()
}

/// What the uninterrupted write-ahead run reaches: its committed digest at
/// [`HORIZON`] and the journal it holds.
struct Uninterrupted {
    digest: Value,
    journal: JournalContents,
}

fn uninterrupted() -> &'static Uninterrupted {
    static REFERENCE: OnceLock<Uninterrupted> = OnceLock::new();
    REFERENCE.get_or_init(|| {
        let config = ecology("write-ahead-before-apply");
        let run_dir = fresh("uninterrupted");
        let out = run(&config, &run_dir, HORIZON);
        Uninterrupted {
            digest: reached(&out, HORIZON),
            journal: journal(&run_dir, flife_cli::RUN_SESSION, config_hash(&config)),
        }
    })
}

/// What `run_dir`'s catalog journals for `session`.
fn journal(run_dir: &Path, session: SessionId, hash: Sha256Digest) -> JournalContents {
    CommandJournal::open(&run_dir.join("catalog.db"))
        .expect("the catalog opens")
        .open_run(session, hash)
        .expect("the run is recorded")
}

/// The chunk files a store holds: name to (device, inode).
fn chunk_files(run_dir: &Path) -> BTreeMap<String, (u64, u64)> {
    let mut files = BTreeMap::new();
    for shard in fs::read_dir(run_dir.join("store/chunks")).expect("lists") {
        for chunk in fs::read_dir(shard.expect("a shard").path()).expect("lists") {
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

/// How many of `a`'s chunk files are the very files (same inode) `b`
/// holds under the same name.
fn same_files(a: &BTreeMap<String, (u64, u64)>, b: &BTreeMap<String, (u64, u64)>) -> usize {
    a.iter()
        .filter(|(name, file)| b.get(*name) == Some(*file))
        .count()
}

fn sha(record: &Value) -> Sha256Digest {
    let hex = record.as_str().expect("a hex digest");
    let mut bytes = [0_u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * index..2 * index + 2], 16).expect("hex");
    }
    Sha256Digest::from_bytes(bytes)
}

fn session_of(record: &Value) -> SessionId {
    SessionId::new(record["session"].as_u64().expect("a session"))
}

fn brush(session: SessionId, id: u64, tick: u64) -> ClientCommand {
    ClientCommand {
        session,
        id: CommandId::new(id),
        target: Target::At {
            tick: Tick::new(tick),
            retarget: false,
        },
        expected_revision: None,
        intervention: Intervention::InjectEnergy {
            cell: [1, 1],
            amount: 1.0,
        },
    }
}

/// The write-ahead case, end to end.
#[test]
fn a_control_branch_continues_as_its_parent_while_an_intervention_branch_diverges() {
    let reference = uninterrupted();
    let config = ecology("write-ahead-before-apply");
    let hash = config_hash(&config);
    let parent = fresh("parent");
    let out = run(&config, &parent, PARENT_TICKS);
    let fork = checkpoint_at(&out, FORK).clone();
    let fork_sha = sha(&fork["sha256"]);
    let fork_digest = {
        let store =
            common::open_store(parent.join("store"), Durability::ProcessCrash).expect("opens");
        let bytes = store
            .load(fork["sequence"].as_u64().expect("a sequence"))
            .expect("the fork checkpoint loads");
        restore(&bytes, NumericalProfile::reference_f32().expect("profile"))
            .expect("restores")
            .world
            .digest()
            .to_hex()
    };

    // Two branches of one checkpoint: a control, and an intervention with
    // one brush of its own at tick 130.
    let control = fresh("control");
    let made = branch(&parent, &control, FORK, &[]);
    let lineage = json!([{
        "parent": 1, "admitted": 4, "tick": FORK, "sha256": fork["sha256"],
    }]);
    assert_eq!(made["name"], "control");
    assert_eq!(
        session_of(&made),
        flife_cli::branch_session(fork_sha, "control"),
        "the branch's identity is its checkpoint and name"
    );
    assert_eq!(
        made["parent"],
        json!({
            "run_dir": text(&parent), "session": 1, "sequence": fork["sequence"],
            "tick": FORK, "sha256": fork["sha256"],
        })
    );
    assert_eq!(made["lineage"], lineage);
    // Commands 1 and 2 applied at ticks 3 and 66; 3 and 4 are pending.
    assert_eq!(
        made["inherited"],
        json!({"admitted": 4, "outcomes": 2, "pending": 2})
    );
    assert_eq!(made["commands"], json!([]));
    assert_eq!(made["checkpoint"], json!({"sequence": 1, "tick": FORK}));
    assert_eq!(made["digest"], fork_digest, "the parent's world");
    assert_eq!(
        made["chunks"],
        json!({"written": 1, "linked": 10}),
        "every world chunk linked, the command closure written"
    );
    assert_ne!(made["sha256"], fork["sha256"], "the book records the fork");

    let intervention = fresh("intervention");
    let treated = branch(&parent, &intervention, FORK, &["--inject", BRUSH]);
    assert_eq!(treated["name"], "intervention");
    assert_eq!(treated["lineage"], lineage);
    assert_eq!(treated["commands"], json!([5]), "numbered after command 4");
    assert_eq!(treated["chunks"], json!({"written": 1, "linked": 10}));
    let (control_session, treated_session) = (session_of(&made), session_of(&treated));
    assert_ne!(control_session, treated_session);
    for session in [control_session, treated_session] {
        assert_ne!(session, flife_cli::RUN_SESSION);
    }

    // The shared closure: the branches' world chunks are the parent's files.
    let (parent_files, control_files, treated_files) = (
        chunk_files(&parent),
        chunk_files(&control),
        chunk_files(&intervention),
    );
    assert_eq!(control_files.len(), 11);
    assert_eq!(same_files(&control_files, &parent_files), 10);
    assert_eq!(same_files(&treated_files, &parent_files), 10);
    assert_eq!(same_files(&control_files, &treated_files), 10);

    // The parent goes on alone to the uninterrupted state, retiring the
    // fork checkpoint and removing its names for the chunks only it held.
    let out = resume(&parent, HORIZON);
    assert_eq!(reached(&out, HORIZON), reference.digest);
    let after = chunk_files(&parent);
    let removed = control_files
        .keys()
        .filter(|name| parent_files.contains_key(*name) && !after.contains_key(*name))
        .count();
    assert!(
        removed > 0,
        "the parent's retention removed its names for chunks the branches share"
    );
    {
        let store =
            common::open_store(control.join("store"), Durability::ProcessCrash).expect("opens");
        store
            .load(1)
            .expect("the control's founding checkpoint still loads whole");
    }
    let parent_journal = journal(&parent, flife_cli::RUN_SESSION, hash);
    assert_eq!(
        parent_journal, reference.journal,
        "the parent's history is its own: no branch command reached it"
    );

    // Paired runs: identical until the brush at 130, different after.
    let control_129 = reached(&resume(&control, 129), 129);
    let treated_129 = reached(&resume(&intervention, 129), 129);
    assert_eq!(control_129, treated_129, "no difference before the brush");

    // A branch of the control at 129 carries both forks.
    let deeper = fresh("deeper");
    let nested = branch(&control, &deeper, 129, &[]);
    let control_129_checkpoint =
        common::open_store(control.join("store"), Durability::ProcessCrash)
            .expect("opens")
            .latest_valid(|_, bytes| Ok(Sha256Digest::of(bytes).to_hex()))
            .expect("lists")
            .latest
            .expect("a checkpoint")
            .1;
    assert_eq!(
        nested["lineage"],
        json!([
            lineage[0],
            {"parent": control_session.get(), "admitted": 4, "tick": 129,
             "sha256": control_129_checkpoint},
        ])
    );
    assert_eq!(nested["digest"], control_129);
    assert_eq!(
        same_files(&chunk_files(&deeper), &chunk_files(&control)),
        10
    );

    let control_end = reached(&resume(&control, HORIZON), HORIZON);
    let treated_end = reached(&resume(&intervention, HORIZON), HORIZON);
    assert_eq!(
        control_end, reference.digest,
        "the control branch, with the parent's pending commands, is the uninterrupted run"
    );
    assert_ne!(treated_end, reference.digest, "the brush changed the world");
    assert_eq!(
        reached(&resume(&deeper, HORIZON), HORIZON),
        reference.digest
    );

    // Independent command histories.
    let control_journal = journal(&control, control_session, hash);
    assert_eq!(
        control_journal, reference.journal,
        "the control journal holds the inherited commands, applied exactly as uninterrupted"
    );
    let treated_journal = journal(&intervention, treated_session, hash);
    assert_eq!(treated_journal.accepted.len(), 5);
    assert_eq!(
        treated_journal.accepted[..4],
        reference.journal.accepted[..],
        "the same inherited acceptances"
    );
    let (own, accepted) = treated_journal.accepted[4];
    assert_eq!((own.session, own.id.get()), (treated_session, 5));
    assert_eq!((accepted.order, accepted.effective.get()), (4, 130));
    assert_eq!(treated_journal.outcomes.len(), 5);
    assert!(
        treated_journal.outcomes.iter().any(|(id, _)| id.get() == 5),
        "the brush applied in the intervention branch"
    );
    let mut catalog = CommandJournal::open(&deeper.join("catalog.db")).expect("opens");
    let nested_session = session_of(&nested);
    catalog.open_run(nested_session, hash).expect("recorded");
    let forks = catalog.lineage().expect("reads");
    assert_eq!(
        forks.iter().map(|fork| fork.parent).collect::<Vec<_>>(),
        [flife_cli::RUN_SESSION, control_session]
    );
    let origin = catalog.branch_origin().expect("reads").expect("a branch");
    assert_eq!(
        (origin.name.as_str(), origin.parent_run.as_str()),
        ("deeper", text(&control))
    );

    // Each branch admits its own session's commands only.
    let store =
        common::open_store(intervention.join("store"), Durability::ProcessCrash).expect("opens");
    let latest = store.sequences().expect("lists");
    let bytes = store.load(*latest.last().expect("one")).expect("loads");
    let mut restored =
        restore(&bytes, NumericalProfile::reference_f32().expect("profile")).expect("restores");
    assert_eq!(restored.book.session(), treated_session);
    assert_eq!(restored.book.lineage().len(), 1);
    assert_eq!(
        restored
            .book
            .accepted(CommandId::new(5))
            .map(|accepted| accepted.durability),
        Some(CommandDurability::WriteAhead),
        "the branch's own command was accepted write-ahead"
    );
    let grid = *restored.world.field().grid();
    let at = restored.world.tick();
    assert!(matches!(
        restored
            .book
            .admit(brush(control_session, 6, 300), at, &grid, false),
        Admission::Rejected(Rejection::WrongSession { .. })
    ));
    assert!(matches!(
        restored
            .book
            .admit(brush(treated_session, 3, 300), at, &grid, false),
        Admission::Rejected(Rejection::ConflictingReuse { .. })
    ));
    assert!(matches!(
        restored
            .book
            .admit(brush(treated_session, 6, 300), at, &grid, false),
        Admission::Accepted { retry: false, .. }
    ));
}

/// The volatile case: the same shared closure and control trajectory, with
/// nothing journaled.
#[test]
fn a_volatile_branch_shares_its_parents_chunks_and_journals_nothing() {
    let reference = uninterrupted();
    let config = ecology("volatile-in-memory");
    let hash = config_hash(&config);
    let parent = fresh("volatile-parent");
    let out = run(&config, &parent, PARENT_TICKS);
    let fork = checkpoint_at(&out, FORK).clone();
    let control = fresh("volatile-control");
    let made = branch(&parent, &control, FORK, &[]);
    assert_eq!(
        made["inherited"],
        json!({"admitted": 4, "outcomes": 2, "pending": 2})
    );
    assert_eq!(made["chunks"], json!({"written": 1, "linked": 10}));
    assert_eq!(
        same_files(&chunk_files(&control), &chunk_files(&parent)),
        10
    );
    assert_eq!(
        reached(&resume(&control, HORIZON), HORIZON),
        reference.digest,
        "the same trajectory: durability is not scientific state"
    );
    let session = session_of(&made);
    assert_eq!(
        made["name"], "volatile-control",
        "named after its directory"
    );
    assert_eq!(
        session,
        flife_cli::branch_session(sha(&fork["sha256"]), "volatile-control")
    );
    let mut catalog = CommandJournal::open(&control.join("catalog.db")).expect("opens");
    assert_eq!(
        catalog.open_run(session, hash).expect("recorded"),
        JournalContents::default(),
        "a volatile branch journals nothing"
    );
    assert_eq!(catalog.lineage().expect("reads").len(), 1);
}

/// Refusals, each with nothing made, and a branch's identity.
#[test]
fn branching_refuses_what_it_cannot_branch() {
    let config = ecology("write-ahead-before-apply");
    let parent = fresh("refusals-parent");
    let out = run(&config, &parent, 30);
    let fork = checkpoint_at(&out, 28).clone();
    let refused = |args: &[&str], expected: &str| {
        let (status, out, err) = flife(args);
        assert_eq!(status, Some(2), "{args:?}: {out:?}");
        assert!(out.is_empty(), "{out:?}");
        let detail = err[0]["detail"].as_str().expect("a detail");
        assert!(detail.contains(expected), "{args:?}: {detail}");
    };
    let target = fresh("refused");
    let (parent_text, target_text) = (text(&parent), text(&target));
    refused(
        &["branch", parent_text, target_text, "--at", "27"],
        "holds no checkpoint that restores at tick 27",
    );
    refused(
        &[
            "branch",
            parent_text,
            target_text,
            "--at",
            "28",
            "--inject",
            "27:1,1:1.0",
        ],
        "was not admitted to the branch at tick 28",
    );
    refused(
        &["branch", parent_text, target_text, "--name", ""],
        "a branch name is 1 to 256 bytes",
    );
    refused(
        &["branch", target_text, text(&fresh("refused-too"))],
        "is not a run directory",
    );
    {
        let _held = common::open_store(parent.join("store"), Durability::ProcessCrash)
            .expect("the test holds the parent");
        refused(
            &["branch", parent_text, target_text],
            "another writer holds",
        );
    }
    assert!(!target.exists(), "no refused branch made its directory");
    fs::create_dir_all(&target).expect("creates");
    fs::write(target.join("other"), b"x").expect("writes");
    refused(&["branch", parent_text, target_text], "is not empty");

    // The same branch made twice is the same session and founding bytes;
    // another name is another session.
    let first = branch(&parent, &fresh("same-1"), 28, &["--name", "trial"]);
    let second = branch(&parent, &fresh("same-2"), 28, &["--name", "trial"]);
    let other = branch(&parent, &fresh("same-3"), 28, &["--name", "trial-2"]);
    assert_eq!(first["session"], second["session"]);
    assert_eq!(first["sha256"], second["sha256"]);
    assert_ne!(first["session"], other["session"]);
    assert_eq!(first["parent"]["sha256"], fork["sha256"]);
    // Without --at, the newest checkpoint that restores.
    let newest = branch(&parent, &fresh("newest"), 30, &[]);
    let latest = flife(&["branch", parent_text, text(&fresh("default"))]).1;
    assert_eq!(latest[0]["parent"], newest["parent"]);
    assert_eq!(latest[0]["parent"]["tick"], 30);

    // A branch whose founding checkpoint is damaged has no founding world
    // to fall back to. Only its own manifest is damaged: the chunks are
    // the parent's files too.
    let damaged = fresh("damaged-branch");
    branch(&parent, &damaged, 28, &[]);
    fs::write(
        damaged.join("store/checkpoints/0000000000000001.manifest"),
        b"x",
    )
    .expect("damages the manifest");
    refused(
        &["resume", text(&damaged), "--until", "40"],
        "a branch has no founding world to fall back to",
    );
    let (status, _, err) = flife(&["resume", parent_text, "--until", "40", "--every", "0"]);
    assert_eq!(status, Some(0), "the parent is untouched: {err:?}");

    // Another session's checkpoint in a branch's store is passed over.
    let control = fresh("passed-over");
    branch(&parent, &control, 28, &[]);
    let foreign = fresh("foreign");
    branch(&parent, &foreign, 28, &["--inject", "35:1,1:1.0"]);
    for (name, _) in chunk_files(&foreign) {
        let from = foreign.join("store/chunks").join(&name[..2]).join(&name);
        let to = control.join("store/chunks").join(&name[..2]).join(&name);
        if !to.exists() {
            fs::create_dir_all(to.parent().expect("a shard")).expect("creates");
            fs::copy(&from, &to).expect("copies");
        }
    }
    fs::copy(
        foreign.join("store/checkpoints/0000000000000001.manifest"),
        control.join("store/checkpoints/0000000000000009.manifest"),
    )
    .expect("copies");
    let out = resume(&control, 40);
    let resumed = one(&out, "resume");
    assert_eq!(resumed["checkpoint"], json!({"sequence": 1, "tick": 28}));
    let skipped = resumed["skipped"].as_array().expect("a list");
    assert_eq!(skipped.len(), 1, "{resumed}");
    assert_eq!(skipped[0]["sequence"], 9);
    assert!(
        skipped[0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("holds the commands of")),
        "{resumed}"
    );
}

/// FL-022 and FL-048's H4 design: a removal branch is an intervention
/// branch like a brush branch. Branched at [`FORK`] with `--remove` (a
/// disk of radius 10 at the centre at tick 125), it numbers the removal
/// after the inherited commands, matches its control branch until the
/// removal's boundary, and differs after it, having removed organisms.
#[test]
fn a_removal_branch_matches_its_control_until_the_removal_and_differs_after() {
    let config = ecology("write-ahead-before-apply");
    let parent = fresh("removal-parent");
    run(&config, &parent, FORK);
    let control = fresh("removal-control");
    branch(&parent, &control, FORK, &[]);
    let treated = fresh("removal-treated");
    let made = branch(
        &parent,
        &treated,
        FORK,
        &["--remove", "125:16,16:10:remove"],
    );
    assert_eq!(made["commands"], json!([5]), "numbered after command 4");

    assert_eq!(
        reached(&resume(&control, 125), 125),
        reached(&resume(&treated, 125), 125),
        "no difference before the removal's boundary"
    );
    let control_after = resume(&control, 126);
    let treated_after = resume(&treated, 126);
    assert_ne!(reached(&control_after, 126), reached(&treated_after, 126));
    let removed = |records: &[Value]| {
        one(records, "end")["tally"]["deaths_by_stage"]["removed"]
            .as_u64()
            .unwrap_or(0)
    };
    assert_eq!(removed(&control_after), 0);
    assert!(
        removed(&treated_after) > 0,
        "{}",
        one(&treated_after, "end")
    );
}
