//! FL-041 evidence in the store (plan §15.5, Appendix B.1): the experiment
//! catalog keeps one row per run, in run order, durably across reopening;
//! cataloging a run again is a no-op only with the same row; and a catalog
//! of another experiment or another format is refused without being
//! changed.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helpers: a failure in one is a test failure"
)]

use std::fs;
use std::path::{Path, PathBuf};

use flife_core::CoreError;
use flife_store::{
    CATALOG_FORMAT, CatalogedRun, CommandJournal, EXPERIMENT_CATALOG_FORMAT, ExperimentCatalog,
};

const IDENTITY: &str = "a1b2";
const DESIGN: &str = "{\"name\":\"catalog-test\"}";

fn scratch(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("flife-experiment-catalog")
        .join(name);
    if path.exists() {
        fs::remove_dir_all(&path).expect("clears the last run's directory");
    }
    fs::create_dir_all(&path).expect("creates the scratch directory");
    path
}

fn row(ordinal: u64, condition: &str, split: &str, seed: u64, status: &str) -> CatalogedRun {
    CatalogedRun {
        ordinal,
        condition: condition.to_owned(),
        split: split.to_owned(),
        seed,
        config_hash: format!("hash-{condition}-{seed}"),
        status: status.to_owned(),
        result: format!("{{\"status\":\"{status}\",\"ordinal\":{ordinal}}}"),
        manifest: format!("{{\"ordinal\":{ordinal}}}"),
        records_sha256: format!("sha-{ordinal}"),
    }
}

#[test]
fn the_catalog_keeps_each_run_once_in_run_order_across_reopening() {
    let path = scratch("order").join("experiment.sqlite");
    let rows = [
        row(1, "control", "discovery", 1, "reached"),
        row(2, "control", "discovery", 2, "error"),
        row(3, "starved", "discovery", 1, "capacity-paused"),
        // A seed is stored by its bits: the largest u64 comes back intact.
        row(4, "starved", "held-out", u64::MAX, "failed"),
    ];
    {
        let mut catalog = ExperimentCatalog::open(&path, IDENTITY, DESIGN).expect("creates");
        assert_eq!(catalog.design().expect("reads").as_deref(), Some(DESIGN));
        assert!(catalog.runs().expect("reads").is_empty());
        // Recorded out of order; read back in run order.
        for index in [2, 0, 3, 1] {
            catalog.record(&rows[index]).expect("catalogs");
        }
        assert_eq!(catalog.runs().expect("reads"), rows);
    }
    let mut catalog =
        ExperimentCatalog::open(&path, IDENTITY, "ignored on reopening").expect("reopens");
    assert_eq!(catalog.design().expect("reads").as_deref(), Some(DESIGN));
    assert_eq!(
        catalog.runs().expect("reads"),
        rows,
        "every row survives reopening"
    );

    // The same row again is a no-op.
    catalog.record(&rows[1]).expect("the same row");
    assert_eq!(catalog.runs().expect("reads"), rows);

    // Another row under a cataloged ordinal, or a cataloged (condition,
    // split, seed) under a new ordinal, is refused and records nothing.
    let mut changed = rows[1].clone();
    changed.result = "{\"status\":\"reached\"}".to_owned();
    changed.status = "reached".to_owned();
    let mut moved = rows[2].clone();
    moved.ordinal = 9;
    for conflicting in [changed, moved] {
        let refused = catalog.record(&conflicting);
        assert!(
            matches!(refused, Err(CoreError::StateConflict { .. })),
            "{refused:?}"
        );
        assert_eq!(catalog.runs().expect("reads"), rows);
    }
    // The same condition and seed in the other split is another run.
    let other = row(5, "control", "held-out", 1, "reached");
    catalog.record(&other).expect("catalogs");
    assert_eq!(catalog.runs().expect("reads").len(), 5);
}

#[test]
fn a_catalog_of_another_experiment_or_format_is_refused_unchanged() {
    let directory = scratch("refusals");
    let path = directory.join("experiment.sqlite");
    {
        let mut catalog = ExperimentCatalog::open(&path, IDENTITY, DESIGN).expect("creates");
        catalog
            .record(&row(1, "control", "discovery", 1, "reached"))
            .expect("catalogs");
    }
    let refused = ExperimentCatalog::open(&path, "c3d4", DESIGN);
    assert!(
        matches!(&refused, Err(CoreError::StateConflict { detail, .. }) if detail.contains(IDENTITY)),
        "{refused:?}"
    );
    let catalog = ExperimentCatalog::open(&path, IDENTITY, DESIGN).expect("reopens");
    assert_eq!(catalog.runs().expect("reads").len(), 1);
    drop(catalog);

    // A run catalog (FL-036) is another format: refused, and still a run
    // catalog afterwards.
    let journal_path = directory.join("catalog.db");
    drop(CommandJournal::open(&journal_path).expect("creates a run catalog"));
    let refused = ExperimentCatalog::open(&journal_path, IDENTITY, DESIGN);
    assert!(
        matches!(&refused, Err(CoreError::SchemaMismatch { expected, found })
            if expected == EXPERIMENT_CATALOG_FORMAT && found == CATALOG_FORMAT),
        "{refused:?}"
    );
    let journal = CommandJournal::open(&journal_path).expect("still a run catalog");
    assert!(journal.runs().expect("reads").is_empty());
}
